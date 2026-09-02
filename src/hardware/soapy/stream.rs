// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The RX stream and the thread that drives it.
//!
//! `readStream` is a blocking pull, like librtlsdr's async read, so it gets an
//! owned thread. **`rtlsdr/mod.rs` is the worked example**: the same
//! `AtomicBool` plus `JoinHandle` shape, the same rule that `stop_rx` joins
//! before returning so no block can arrive afterwards, and the same `Drop` that
//! stops a running stream before the handle goes.
//!
//! Three things differ from RTL-SDR, and each has bitten someone before:
//!
//! - **The buffer is ours.** librtlsdr hands its callback a buffer it owns; here
//!   we allocate and `readStream` fills it.
//! - **`readStream` counts elements, not bytes.** One element is one I/Q pair,
//!   so the byte length is `count * bytes_per_pair`. Getting this wrong produces
//!   a spectrum that looks plausible and is wrong.
//! - **A timeout is not an error.** It is what a quiet bus looks like, and
//!   logging one every time would bury the failures that matter.
//!
//! The decisions are split out as pure functions so they can be tested without a
//! radio, which is the rule for this whole backend.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;

use super::api::{self, SoapyApi, SoapySDRDevice, SoapySDRStream};
use super::caps::READ_PAIRS;
use crate::hardware::process::process_block;
use crate::hardware::RxContext;

/// How long one `readStream` waits before giving up on this round.
///
/// Generous against the data rate and short against a human: 16384 pairs is
/// about 2 ms at 8 Msps and 16 ms at 1 Msps, so a timeout really does mean
/// nothing arrived. It is also how long `stop_rx` can take to be noticed, which
/// is why it is not a second.
const READ_TIMEOUT_US: i64 = 100_000;

/// Consecutive timeouts before the log says so.
///
/// Ten of them is a second of silence. One is a quiet moment; a second of them
/// means the stream is dead and the user is staring at a frozen spectrum
/// wondering why.
const TIMEOUTS_BEFORE_COMPLAINT: u32 = 10;

/// What one `readStream` return code means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// This many I/Q pairs are in the buffer.
    Pairs(usize),
    /// Nothing arrived in the window. Normal.
    Timeout,
    /// The driver's buffer filled and samples were lost.
    Overflow,
    /// Anything else, carrying the code so the log can name it.
    Error(i32),
}

/// Classify a `readStream` return.
///
/// Pure, and separated because the alternative is a `match` buried in a thread
/// nobody can run in a test. Note the unknown-negative case: a code this build
/// has never heard of is an error, **not** a sample count, and a naive
/// `if code > 0 { .. } else { .. }` that fell through to "carry on" would read
/// a buffer nobody filled.
pub fn outcome(code: i32) -> Outcome {
    match code {
        n if n > 0 => Outcome::Pairs(n as usize),
        0 => Outcome::Timeout,
        api::ERR_TIMEOUT => Outcome::Timeout,
        api::ERR_OVERFLOW => Outcome::Overflow,
        other => Outcome::Error(other),
    }
}

/// Bytes occupied by `pairs` I/Q pairs of a given geometry.
///
/// One line, and it has a test, because it is the conversion between the two
/// units this file straddles.
pub fn byte_len(pairs: usize, bytes_per_pair: usize) -> usize {
    pairs.saturating_mul(bytes_per_pair)
}

/// Pairs to ask for in one read, given what we want and what the driver allows.
///
/// An MTU of zero means the driver declines to say, which is not the same as
/// "zero samples": treat it as no constraint rather than reading nothing
/// forever.
pub fn read_size(want: usize, mtu: usize) -> usize {
    if mtu == 0 {
        want
    } else {
        want.min(mtu)
    }
}

/// A running RX stream, or nothing.
#[derive(Default)]
pub struct Streaming {
    pub active: Arc<AtomicBool>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl Streaming {
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::SeqCst)
    }

    /// Set the stream up, activate it, and hand it to an owned thread.
    ///
    /// The thread owns the stream for its whole life and tears it down itself,
    /// so there is exactly one place that closes it and no window where another
    /// thread could touch a stream that is going away.
    pub fn start(
        &self,
        api: &'static SoapyApi,
        dev: *mut SoapySDRDevice,
        format: String,
        ctx: Arc<RxContext>,
    ) -> anyhow::Result<()> {
        if self.active.swap(true, Ordering::SeqCst) {
            return Ok(()); // already streaming
        }
        // Safety: `dev` is live for as long as the SoapyDevice that owns it, and
        // `stop` joins this thread before that device drops.
        let stream = match unsafe { api.setup_stream(dev, &format) } {
            Ok(s) => s,
            Err(e) => {
                self.active.store(false, Ordering::SeqCst);
                anyhow::bail!("SoapySDR could not open a {format} stream: {e}");
            }
        };
        if let Err(e) = unsafe { api.activate_stream(dev, stream) } {
            unsafe { api.close_stream(dev, stream) };
            self.active.store(false, Ordering::SeqCst);
            anyhow::bail!("SoapySDR could not start the stream: {e}");
        }

        // Cross-check our idea of the wire format against the library's own.
        // These are two sources for one fact, and if they ever disagree we would
        // be slicing the buffer on the wrong stride and drawing a spectrum out
        // of misaligned bytes. Better to refuse and say so.
        let bytes_per_pair = ctx.geometry.bytes_per_pair();
        let library_says = api.format_size(&format);
        if library_says != bytes_per_pair {
            unsafe {
                api.deactivate_stream(dev, stream);
                api.close_stream(dev, stream);
            }
            self.active.store(false, Ordering::SeqCst);
            anyhow::bail!(
                "SoapySDR says {format} is {library_says} bytes per sample, sdrtop expects \
                 {bytes_per_pair}. Refusing to stream rather than misread the buffer."
            );
        }

        // The MTU bounds one read. It is 131072 elements on a HackRF, far above
        // our block, but a driver with a smaller one would silently return short
        // reads and the timing panel's expected block size would be wrong. Clamp
        // and say so instead.
        let mtu = unsafe { api.stream_mtu(dev, stream) };
        let pairs_per_read = read_size(READ_PAIRS as usize, mtu);
        if pairs_per_read != READ_PAIRS as usize {
            log(
                &ctx,
                format!(
                    "SoapySDR: this driver's stream MTU is {mtu} samples, so blocks are \
                     {pairs_per_read} rather than {READ_PAIRS}. Timing figures follow the \
                     smaller block."
                ),
            );
        }

        let active = Arc::clone(&self.active);
        // Raw pointers are not `Send`, and these two are only ever touched by
        // this thread once it starts. The same usize hop `rtlsdr/mod.rs` makes.
        let dev_addr = dev as usize;
        let stream_addr = stream as usize;
        let handle = std::thread::spawn(move || {
            let dev = dev_addr as *mut SoapySDRDevice;
            let stream = stream_addr as *mut SoapySDRStream;
            run(
                api,
                dev,
                stream,
                pairs_per_read,
                bytes_per_pair,
                &active,
                &ctx,
            );
            // Teardown belongs to the thread that owned the stream.
            unsafe {
                api.deactivate_stream(dev, stream);
                api.close_stream(dev, stream);
            }
            active.store(false, Ordering::SeqCst);
        });
        *self.thread.lock().unwrap_or_else(|e| e.into_inner()) = Some(handle);
        Ok(())
    }

    /// Stop and **join**, so no block can land after this returns.
    pub fn stop(&self) {
        self.active.store(false, Ordering::SeqCst);
        if let Some(h) = self.thread.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = h.join();
        }
    }
}

impl Drop for Streaming {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The read loop.
#[allow(clippy::too_many_arguments)]
fn run(
    api: &SoapyApi,
    dev: *mut SoapySDRDevice,
    stream: *mut SoapySDRStream,
    pairs_per_read: usize,
    bytes_per_pair: usize,
    active: &AtomicBool,
    ctx: &RxContext,
) {
    let mut buf = vec![0u8; byte_len(pairs_per_read, bytes_per_pair)];
    let mut quiet_reads: u32 = 0;

    while active.load(Ordering::SeqCst) {
        // Safety: the handles are live for the life of this thread, and `buf`
        // holds `pairs_per_read` elements of the stream's own format.
        let code = unsafe {
            api.read_stream(
                dev,
                stream,
                buf.as_mut_ptr() as *mut c_void,
                pairs_per_read,
                READ_TIMEOUT_US,
            )
        };
        // Taken here rather than after the work, so jitter measures the true
        // interval between reads and not read-plus-processing.
        let now = Instant::now();

        match outcome(code) {
            Outcome::Pairs(pairs) => {
                quiet_reads = 0;
                let len = byte_len(pairs, bytes_per_pair).min(buf.len());
                process_block(&buf[..len], ctx.geometry, 0, ctx, now);
            }
            Outcome::Timeout => {
                quiet_reads += 1;
                if quiet_reads == TIMEOUTS_BEFORE_COMPLAINT {
                    log(
                        ctx,
                        "SoapySDR: no samples for a second. Is the device still there?",
                    );
                }
            }
            Outcome::Overflow => {
                quiet_reads = 0;
                // **A floor, not a measurement.** An overflow says the driver's
                // buffer filled; it does not say by how much, and this API has
                // no way to ask. One read's worth is the smallest number that is
                // certainly not an overstatement, and it is far better than
                // zero: zero would leave the STREAM panel calling a dropping
                // link healthy.
                process_block(&[], ctx.geometry, READ_PAIRS, ctx, now);
            }
            Outcome::Error(code) => {
                log(
                    ctx,
                    format!(
                        "SoapySDR stream error {code} ({}). Stopping RX.",
                        api.err_to_str(code)
                    ),
                );
                break;
            }
        }
    }
}

/// Push a line into the app's log, recovering a poisoned lock.
///
/// The read thread must not die on a mutex a panicking UI poisoned; dropping the
/// message is the right answer, the same as the other two backends do.
fn log(ctx: &RxContext, msg: impl Into<String>) {
    if let Ok(mut m) = ctx.metrics.lock() {
        m.push_log(msg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The classification, including the two codes that are not failures and the
    /// one that is easy to mistake for a sample count.
    #[test]
    fn a_read_return_code_says_what_happened() {
        assert_eq!(outcome(4096), Outcome::Pairs(4096));
        assert_eq!(outcome(0), Outcome::Timeout, "nothing read is nothing read");
        assert_eq!(outcome(-1), Outcome::Timeout, "SOAPY_SDR_TIMEOUT");
        assert_eq!(outcome(-4), Outcome::Overflow, "SOAPY_SDR_OVERFLOW");
        assert_eq!(outcome(-2), Outcome::Error(-2), "STREAM_ERROR");
        assert_eq!(outcome(-3), Outcome::Error(-3), "CORRUPTION");
    }

    /// A driver returning a code this build has never heard of must be treated
    /// as an error and not as a sample count. The naive `if code > 0 ... else`
    /// would fall through to reading a buffer nobody filled.
    #[test]
    fn an_unknown_negative_code_is_never_mistaken_for_data() {
        for code in [-99, -42, i32::MIN] {
            assert_eq!(outcome(code), Outcome::Error(code));
        }
    }

    /// Elements to bytes, at both widths. This is the conversion the module
    /// comment warns about, so it gets its own assertions rather than being
    /// implied by another test.
    #[test]
    fn elements_convert_to_bytes_by_the_geometrys_stride() {
        assert_eq!(byte_len(16_384, 2), 32_768, "CS8 is two bytes a pair");
        assert_eq!(byte_len(16_384, 4), 65_536, "CS16 is four");
        assert_eq!(byte_len(0, 4), 0);
    }

    /// A driver claiming an absurd count must not overflow the multiplication
    /// into a small number and then be trusted as a buffer length.
    #[test]
    fn a_nonsense_pair_count_saturates_rather_than_wrapping() {
        assert_eq!(byte_len(usize::MAX, 4), usize::MAX);
    }

    /// The block size against the driver's own limit. The zero case is the one
    /// worth pinning: a driver that will not say must not be read as one that
    /// allows nothing, or the loop would spin reading zero samples forever.
    #[test]
    fn the_read_size_respects_an_mtu_without_believing_a_zero() {
        assert_eq!(
            read_size(16_384, 131_072),
            16_384,
            "a HackRF, unconstrained"
        );
        assert_eq!(read_size(16_384, 4_096), 4_096, "a smaller driver wins");
        assert_eq!(
            read_size(16_384, 0),
            16_384,
            "no answer is not a limit of zero"
        );
    }
}
