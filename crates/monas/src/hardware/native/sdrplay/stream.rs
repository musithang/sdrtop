// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The stream: what SDRplay's API calls back with, on its own thread.
//!
//! **Samples.** The API hands I and Q in two arrays of int16; they are
//! interleaved into one buffer and delivered with the samples lost before
//! them.
//!
//! **Gathered, not passed on block by block.** The API's blocks are 1008
//! samples (0.5 ms at 2 MSPS) and arrive in bursts: SDRplay's service holds
//! the stream back for up to 40 ms and then hands over everything at once
//! (measured at 2, 6 and 10 MSPS; 69 ms at worst). Handed to the intake one by one, a burst
//! overran its queue of eight and sdrtop lost 2 % of a 2 MSPS stream with
//! the CPU at 11 % (measured live). So a run of blocks is gathered into one
//! delivery of [`GATHER_MS`] of signal, whatever the rate, which makes the
//! queue's eight deliveries (160 ms) more than twice the longest hold. Counting the span
//! in samples rather than waiting on the clock matters: a fixed count of
//! 16384 samples is 1.6 ms at 10 MSPS, and a burst there still overran the
//! queue. A gap or a new run delivers what was gathered before it first, so
//! every loss is still placed where it happened.
//!
//! **Positions.** Each block carries the position of its first sample in
//! the API's own count (`firstSampleNum`). A block that does not start where
//! the last one ended lost the difference, at a known place, and it is
//! delivered as the driver's loss there. The count wraps at 2^32 (about 7
//! minutes at 10 MSPS), so the difference is taken modulo 2^32. A block
//! flagged `reset` starts a new run, from a new origin (the API does so at
//! every rate change): the run's positions begin again.
//!
//! **The block that is not samples.** When the radio leaves the bus, the API
//! sends one more full block whose position is 0, with no reset and a level
//! ten times the stream's (measured, pulling the cable). Like any block that
//! starts behind where the run has got to, it is counted and never
//! delivered.
//!
//! **Events.** The radio's own overload flag is acknowledged at once, as
//! SDRplay's API requires, and counted for [`super::device::SdrplayDevice`]
//! to report; a correction with no detection before it (the API sends one at
//! `Uninit`) only clears the flag. A removed or failed radio is marked once
//! (the API repeats `DeviceRemoved` every second until `Uninit`), and the
//! stream then reads as stopped.

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use libc::{c_int, c_void};

use crate::hardware::RxContext;

use super::ffi;
use super::layout::{CallbackFnsT, EventParamsT, StreamCbParamsT};

/// `sdrplay_api_Update_Ctrl_OverloadMsgAck`.
pub const REASON_OVERLOAD_ACK: u32 = 0x0400_0000;
/// `sdrplay_api_Update_Ext1_None`.
pub const REASON_EXT1_NONE: u32 = 0;

/// `sdrplay_api_EventT`.
const EVENT_GAIN_CHANGE: c_int = 0;
const EVENT_POWER_OVERLOAD: c_int = 1;
const EVENT_DEVICE_REMOVED: c_int = 2;
const EVENT_DEVICE_FAILURE: c_int = 4;
/// `sdrplay_api_Overload_Detected`.
const OVERLOAD_DETECTED: c_int = 0;

/// How much signal one delivery gathers, in milliseconds of the output rate.
pub const GATHER_MS: f64 = 20.0;

/// The samples [`GATHER_MS`] holds at `output_hz`, and never less than one.
pub fn gather_pairs(output_hz: f64) -> usize {
    ((output_hz * GATHER_MS / 1000.0) as usize).max(1)
}

/// The gRdB the radio last reported (`GainChange`), or -1 before any: what
/// its AGC has the IF at, while the AGC runs. Shared like [`Overload`].
pub struct GainReport {
    pub gr_db: AtomicI32,
}

impl Default for GainReport {
    fn default() -> Self {
        Self {
            gr_db: AtomicI32::new(-1),
        }
    }
}

/// The radio's overload flag as the events left it; shared by the device and
/// every stream it starts, so a report survives the stream that saw it.
#[derive(Default)]
pub struct Overload {
    pub on: AtomicBool,
    pub events: AtomicU64,
}

/// Where the stream stands between two callbacks.
#[derive(Default)]
struct Position {
    /// Where the next block should start, once a block has come.
    expected: Option<u32>,
    /// The samples gathered for the next delivery, interleaved.
    gathered: Vec<i16>,
    /// Lost just before the first gathered sample.
    gathered_lost: u64,
}

impl Position {
    /// Hands what was gathered to the intake, stamped `now`.
    fn deliver(&mut self, rx: &RxContext, now: std::time::Instant) -> usize {
        if self.gathered.is_empty() {
            return 0;
        }
        // SAFETY: an i16 slice viewed as its bytes, in the native order the
        // geometry (int16) reads.
        let bytes = unsafe {
            std::slice::from_raw_parts(self.gathered.as_ptr().cast::<u8>(), self.gathered.len() * 2)
        };
        crate::hardware::intake::deliver(rx, bytes, self.gathered_lost, now);
        let pairs = self.gathered.len() / 2;
        self.gathered.clear();
        self.gathered_lost = 0;
        pairs
    }
}

/// Everything the API's thread reaches through the callbacks' context
/// pointer. Kept alive by the device until `Uninit` has returned.
pub struct StreamCtx {
    pub rx: Arc<RxContext>,
    api: &'static ffi::SdrplayApi,
    /// The selected radio's handle, for acknowledging an overload.
    handle: usize,
    position: Mutex<Position>,
    /// Samples one delivery gathers, from [`gather_pairs`]. A stream runs at
    /// one rate: a new rate is a new stream.
    gather: usize,
    pub overload: Arc<Overload>,
    pub gain: Arc<GainReport>,
    /// The radio left the bus or failed; the stream is over.
    pub ended: AtomicBool,
    /// Why it ended: [`ENDED_REMOVED`], [`ENDED_FAILED`], or
    /// [`ENDED_SILENT`] when the device found it silent.
    pub ended_by: std::sync::atomic::AtomicU8,
    /// When the stream started, and the last block came, ms after it.
    born: std::time::Instant,
    last_block_ms: AtomicU64,
    /// Samples in the last delivery: the gathered block the intake and the
    /// timing bench see.
    pub block: AtomicU32,
    /// Blocks counted and not delivered, because they were not samples.
    pub junk: AtomicU64,
    /// What `Init` was given; the API keeps the pointers.
    pub fns: CallbackFnsT,
}

impl StreamCtx {
    pub fn new(
        rx: Arc<RxContext>,
        api: &'static ffi::SdrplayApi,
        handle: *mut c_void,
        overload: Arc<Overload>,
        gain: Arc<GainReport>,
        output_hz: f64,
    ) -> Self {
        Self {
            gain,
            rx,
            api,
            handle: handle as usize,
            position: Mutex::new(Position::default()),
            gather: gather_pairs(output_hz),
            overload,
            ended: AtomicBool::new(false),
            ended_by: std::sync::atomic::AtomicU8::new(0),
            born: std::time::Instant::now(),
            last_block_ms: AtomicU64::new(0),
            block: AtomicU32::new(0),
            junk: AtomicU64::new(0),
            fns: CallbackFnsT {
                stream_a: Some(stream_callback),
                stream_b: None,
                event: Some(event_callback),
            },
        }
    }
}

/// Why a stream ended (`StreamCtx::ended_by`).
pub const ENDED_REMOVED: u8 = 1;
pub const ENDED_FAILED: u8 = 2;
pub const ENDED_SILENT: u8 = 3;

impl StreamCtx {
    /// How long since the last block came, or since the stream began.
    pub fn silent_for(&self) -> std::time::Duration {
        let since = self.born.elapsed();
        since.saturating_sub(std::time::Duration::from_millis(
            self.last_block_ms.load(Ordering::Relaxed),
        ))
    }
}

/// What to do with a block, from where it starts and where the last one
/// ended: deliver it with the samples lost before it, or not at all.
#[derive(Debug, PartialEq)]
pub enum Placed {
    /// Deliver, `lost` samples after the last block.
    Deliver { lost: u64, new_run: bool },
    /// Not samples: counted, never delivered.
    Junk,
}

/// Where a block starting at `first` stands, the last block having ended at
/// `expected` (`None` before the first), flagged `reset` or not.
pub fn place(expected: Option<u32>, first: u32, reset: bool) -> Placed {
    match expected {
        _ if reset => Placed::Deliver {
            lost: 0,
            new_run: expected.is_some(),
        },
        None => Placed::Deliver {
            lost: 0,
            new_run: false,
        },
        Some(e) => {
            let ahead = first.wrapping_sub(e);
            if ahead < 1 << 31 {
                Placed::Deliver {
                    lost: u64::from(ahead),
                    new_run: false,
                }
            } else {
                // A block from before the last one is not a continuation:
                // the API's last block as the radio leaves (position 0,
                // behind any run) is one of these.
                Placed::Junk
            }
        }
    }
}

unsafe extern "C" fn stream_callback(
    xi: *mut i16,
    xq: *mut i16,
    params: *mut StreamCbParamsT,
    num_samples: u32,
    reset: u32,
    ctx: *mut c_void,
) {
    // Stamped first, so the timing bench measures the interval between
    // deliveries and not entry plus our own work.
    let now = std::time::Instant::now();
    // No Rust panic may cross into C.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if xi.is_null() || xq.is_null() || params.is_null() || ctx.is_null() {
            return;
        }
        // SAFETY: the context is the StreamCtx the device keeps alive until
        // Uninit returns; the arrays hold `num_samples` samples each and the
        // parameters are valid for this call, as SDRplay's API documents.
        let (ctx, i, q, first) = unsafe {
            let n = num_samples as usize;
            (
                &*(ctx as *const StreamCtx),
                std::slice::from_raw_parts(xi, n),
                std::slice::from_raw_parts(xq, n),
                (*params).first_sample_num,
            )
        };
        ctx.last_block_ms.store(
            now.saturating_duration_since(ctx.born).as_millis() as u64,
            Ordering::Relaxed,
        );
        let mut pos = ctx.position.lock().unwrap_or_else(|e| e.into_inner());
        let placed = place(pos.expected, first, reset != 0);
        let Placed::Deliver { lost, new_run } = placed else {
            // What was gathered before it was samples: delivered, since
            // nothing may come after a block like this one.
            let pairs = pos.deliver(&ctx.rx, now);
            if pairs > 0 {
                ctx.block.store(pairs as u32, Ordering::Relaxed);
            }
            ctx.junk.fetch_add(1, Ordering::Relaxed);
            return;
        };
        // What was gathered belongs before a new run, and before a loss.
        if new_run || lost > 0 {
            let pairs = pos.deliver(&ctx.rx, now);
            if pairs > 0 {
                ctx.block.store(pairs as u32, Ordering::Relaxed);
            }
        }
        if new_run {
            ctx.rx.begin_stream();
        }
        pos.expected = Some(first.wrapping_add(num_samples));
        if pos.gathered.is_empty() {
            pos.gathered_lost = lost;
        }
        pos.gathered
            .extend(i.iter().zip(q).flat_map(|(&i, &q)| [i, q]));
        if pos.gathered.len() / 2 >= ctx.gather {
            let pairs = pos.deliver(&ctx.rx, now);
            ctx.block.store(pairs as u32, Ordering::Relaxed);
        }
    }));
}

unsafe extern "C" fn event_callback(
    event: c_int,
    tuner: c_int,
    params: *mut EventParamsT,
    ctx: *mut c_void,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if ctx.is_null() {
            return;
        }
        // SAFETY: as in the stream callback.
        let ctx = unsafe { &*(ctx as *const StreamCtx) };
        match event {
            EVENT_GAIN_CHANGE if !params.is_null() => {
                // SAFETY: a gain event carries the gain variant.
                let gr = unsafe { (*params).gain_params.gr_db };
                ctx.gain
                    .gr_db
                    .store(i32::try_from(gr).unwrap_or(-1), Ordering::Relaxed);
            }
            EVENT_POWER_OVERLOAD => {
                // SAFETY: an overload event carries the overload variant.
                let detected = !params.is_null()
                    && unsafe { (*params).power_overload_change_type } == OVERLOAD_DETECTED;
                ctx.overload.on.store(detected, Ordering::Relaxed);
                if detected {
                    ctx.overload.events.fetch_add(1, Ordering::Relaxed);
                }
                // SAFETY: the radio is selected while its stream runs.
                unsafe {
                    (ctx.api.update)(
                        ctx.handle as *mut c_void,
                        tuner,
                        REASON_OVERLOAD_ACK,
                        REASON_EXT1_NONE,
                    )
                };
            }
            EVENT_DEVICE_REMOVED | EVENT_DEVICE_FAILURE => {
                let by = if event == EVENT_DEVICE_REMOVED {
                    ENDED_REMOVED
                } else {
                    ENDED_FAILED
                };
                ctx.ended_by.store(by, Ordering::Relaxed);
                ctx.ended.store(true, Ordering::Relaxed);
            }
            _ => {}
        }
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A delivery is twenty milliseconds of signal at every rate, so the
    /// intake's eight deliveries outlast twice the service's longest hold
    /// (69 ms, measured), and the slowest rate still delivers something.
    #[test]
    fn a_delivery_is_twenty_milliseconds_at_every_rate() {
        assert_eq!(gather_pairs(2_000_000.0), 40_000);
        assert_eq!(gather_pairs(10_000_000.0), 200_000);
        assert_eq!(gather_pairs(62_500.0), 1_250);
        assert_eq!(gather_pairs(0.0), 1);
        const { assert!(8.0 * GATHER_MS > 2.0 * 69.0) };
    }

    /// A block that starts where the last ended loses nothing; one that
    /// starts later lost the difference; a reset starts a new run.
    #[test]
    fn a_block_is_placed_by_where_it_starts() {
        assert_eq!(
            place(None, 254, true),
            Placed::Deliver {
                lost: 0,
                new_run: false
            }
        );
        assert_eq!(
            place(Some(1262), 1262, false),
            Placed::Deliver {
                lost: 0,
                new_run: false
            }
        );
        assert_eq!(
            place(Some(1262), 2270, false),
            Placed::Deliver {
                lost: 1008,
                new_run: false
            }
        );
        assert_eq!(
            place(Some(9_907_886), 37_550, true),
            Placed::Deliver {
                lost: 0,
                new_run: true
            }
        );
    }

    /// **The count wraps at 2^32** (about seven minutes at 10 MSPS): a block
    /// across the wrap continues, with what was lost across it.
    #[test]
    fn the_count_wraps_without_a_loss() {
        let e = u32::MAX - 1000;
        assert_eq!(
            place(Some(e), e.wrapping_add(2016), false),
            Placed::Deliver {
                lost: 2016,
                new_run: false
            }
        );
        assert_eq!(
            place(Some(e), e, false),
            Placed::Deliver {
                lost: 0,
                new_run: false
            }
        );
    }

    /// **The block the API sends as the radio leaves**: position 0, no reset,
    /// mid-run. Never samples; nor is a block from before the last one.
    #[test]
    fn a_block_at_zero_without_a_reset_is_not_samples() {
        assert_eq!(place(Some(19_160_318), 0, false), Placed::Junk);
        assert_eq!(place(Some(19_160_318), 19_000_000, false), Placed::Junk);
    }
}
