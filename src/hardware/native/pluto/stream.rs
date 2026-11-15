// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The R2's receive stream: a connection of its own, a thread that keeps two
//! `READBUF`s in flight, and the samples it could not get, counted.
//!
//! **A connection of its own.** `iiod` answers one command at a time per
//! connection, and a `READBUF` holds the connection until it is answered. So
//! the stream has its own, and tuning and gain go on the device's, never
//! waiting behind a block.
//!
//! **Whole blocks, two in flight.** The server refills one kernel block per
//! round and sends each waiting client at most what it asked for: a request
//! smaller than the block loses the rest without a word (measured on the R2:
//! a quarter of the stream arrived). So every request is exactly one block.
//! With one request in flight the link sat idle between them; with two it
//! carried the most the R2's USB network gave (about 5.7 MS/s).
//!
//! **Losses counted from the clock.** Neither the protocol nor the samples
//! say when the radio's DMA ring overflowed, and the core's overflow bit is
//! cleared by the next refill. What is certain is arithmetic: a radio at `r`
//! samples per second produces `r` of them a second, and a block that took
//! its full time to arrive means the reader has caught up. At those moments
//! the samples received are compared with the samples the time accounts for;
//! the shortfall is what the ring dropped, in whole blocks, because the kernel
//! drops whole blocks ([`Losses`]).
//!
//! **The mask the server sends is the mask.** Another client asking for more
//! channels widens what every client receives; RX1's I and Q are picked out by
//! the mask on each reply ([`pick_rx1`]).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::iiod::{Client, Timeouts};
use crate::hardware::RxContext;

/// The receive device.
pub const RX: &str = "cf-ad9361-lpc";
/// RX1 I and Q (`voltage0`, `voltage1`), the four channels' mask.
const RX1_MASK: &str = "00000003";
/// Pairs per kernel block and per request.
pub const BLOCK_PAIRS: usize = 65_536;
/// RX1 alone: two int16 per pair.
const PAIR_BYTES: usize = 4;
/// Requests in flight.
const IN_FLIGHT: usize = 2;
/// Above this shortfall (as a share of a block) the ring is taken to have
/// dropped a block; below it is the clocks' own jitter.
const DROP_THRESHOLD: f64 = 0.5;
/// How long a quiet stretch of agreement runs before the reckoning starts
/// afresh, so the two clocks' few ppm never add up to a block.
const REANCHOR: Duration = Duration::from_secs(60);

/// Picks RX1's I and Q out of a reply whose `mask` may name more channels.
/// `None` when the mask lacks RX1 or cannot be read.
pub fn pick_rx1<'a>(mask: &str, data: &'a [u8], scratch: &'a mut Vec<u8>) -> Option<&'a [u8]> {
    let bits = u32::from_str_radix(mask.get(mask.len().saturating_sub(8)..)?, 16).ok()?;
    if bits & 0b11 != 0b11 {
        return None;
    }
    let channels = bits.count_ones() as usize;
    if channels == 2 {
        return Some(data);
    }
    // RX1's I and Q are the two lowest channels, so the first two of every
    // sample's interleaved channels.
    let stride = 2 * channels;
    scratch.clear();
    for sample in data.chunks_exact(stride) {
        scratch.extend_from_slice(&sample[..PAIR_BYTES]);
    }
    Some(scratch)
}

/// The reckoning of samples against time.
#[derive(Debug)]
pub struct Losses {
    rate: f64,
    block: u64,
    anchor: Option<(Instant, u64)>,
    received: u64,
}

impl Losses {
    pub fn new(rate: f64, block: u64) -> Self {
        Self {
            rate,
            block,
            anchor: None,
            received: 0,
        }
    }

    /// One block arrived at `now` with `pairs` in it, after `waited` for it
    /// to be filled. Returns the pairs the ring dropped before it, whole
    /// blocks only.
    pub fn arrived(&mut self, pairs: u64, waited: Duration, now: Instant) -> u64 {
        self.received += pairs;
        // A block that came at once was waiting in the ring: the reader is
        // behind, and the shortfall it shows would be its own lag.
        let block_time = self.block as f64 / self.rate;
        if waited.as_secs_f64() < 0.5 * block_time {
            return 0;
        }
        let Some((at, had)) = self.anchor else {
            self.anchor = Some((now, self.received));
            return 0;
        };
        let expected = now.duration_since(at).as_secs_f64() * self.rate;
        let got = (self.received - had) as f64;
        let short = expected - got;
        if short > DROP_THRESHOLD * self.block as f64 {
            let blocks = (short / self.block as f64).round().max(1.0) as u64;
            let dropped = blocks * self.block;
            // Counted: from here the reckoning carries on as if they came.
            self.received += dropped;
            return dropped;
        }
        if now.duration_since(at) > REANCHOR {
            self.anchor = Some((now, self.received));
        }
        0
    }
}

/// Why a stream ended by itself.
#[derive(Clone, Debug, PartialEq)]
pub enum Ended {
    /// The connection fell silent or broke.
    Lost(String),
    /// The server refused the buffer or the request.
    Refused(String),
}

/// A running stream, or none.
#[derive(Default)]
pub struct Streaming {
    active: Arc<AtomicBool>,
    thread: Mutex<Option<JoinHandle<()>>>,
    ended: Arc<Mutex<Option<Ended>>>,
    rate: Arc<AtomicU64>,
    /// Microseconds waiting for and working on replies, cumulative.
    wait_us: Arc<AtomicU64>,
    work_us: Arc<AtomicU64>,
}

impl Streaming {
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::SeqCst)
    }

    pub fn ended(&self) -> Option<Ended> {
        self.ended.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn clock(&self) -> (u64, u64) {
        (
            self.wait_us.load(Ordering::Relaxed),
            self.work_us.load(Ordering::Relaxed),
        )
    }

    /// The rate the radio runs at, for the reckoning.
    pub fn set_rate(&self, hz: f64) {
        self.rate.store(hz.to_bits(), Ordering::Relaxed);
    }

    /// Connects, opens the buffer and starts the thread.
    pub fn start(&self, address: &str, t: Timeouts, ctx: Arc<RxContext>) -> anyhow::Result<()> {
        if self.active.load(Ordering::SeqCst) {
            return Ok(());
        }
        self.stop();
        let mut c = Client::connect(address, t)
            .map_err(|e| anyhow::anyhow!("the stream's connection to {address}: {e}"))?;
        c.open(RX, BLOCK_PAIRS, RX1_MASK)
            .map_err(|e| anyhow::anyhow!("the receive buffer would not open: {e}"))?;
        *self.ended.lock().unwrap_or_else(|e| e.into_inner()) = None;
        self.active.store(true, Ordering::SeqCst);
        let active = Arc::clone(&self.active);
        let ended = Arc::clone(&self.ended);
        let rate = Arc::clone(&self.rate);
        let (wait_us, work_us) = (Arc::clone(&self.wait_us), Arc::clone(&self.work_us));
        let handle = std::thread::Builder::new()
            .name("pluto-reader".into())
            .spawn(move || {
                let end = run(&mut c, &active, &rate, &ctx, &wait_us, &work_us);
                if let Some(why) = &end {
                    log(&ctx, format!("PlutoSky: the stream stopped: {}", text(why)));
                } else {
                    // Asked to stop: the buffer closed, the session ended.
                    let _ = c.close(RX);
                }
                c.exit();
                *ended.lock().unwrap_or_else(|e| e.into_inner()) = end;
                active.store(false, Ordering::SeqCst);
            })?;
        *self.thread.lock().unwrap_or_else(|e| e.into_inner()) = Some(handle);
        Ok(())
    }

    /// Stops and joins, so no block arrives after this returns.
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

pub fn text(e: &Ended) -> String {
    match e {
        Ended::Lost(why) => format!(
            "the radio stopped answering ({why}); if it was unplugged or restarted, \
             open it again from the menu"
        ),
        Ended::Refused(why) => format!("the radio refused the stream ({why})"),
    }
}

fn log(ctx: &RxContext, msg: String) {
    if let Ok(mut m) = ctx.metrics.lock() {
        m.push_log(msg);
    }
}

/// The read loop. `None` when asked to stop; the reason otherwise.
fn run(
    c: &mut Client,
    active: &AtomicBool,
    rate: &AtomicU64,
    ctx: &RxContext,
    wait_us: &AtomicU64,
    work_us: &AtomicU64,
) -> Option<Ended> {
    let want = BLOCK_PAIRS * PAIR_BYTES;
    let lost = |e: super::iiod::Error| Some(Ended::Lost(e.to_string()));
    for _ in 0..IN_FLIGHT {
        if let Err(e) = c.request(RX, want) {
            return lost(e);
        }
    }
    let mut losses = Losses::new(
        f64::from_bits(rate.load(Ordering::Relaxed)),
        BLOCK_PAIRS as u64,
    );
    let mut known_rate = rate.load(Ordering::Relaxed);
    let mut buf = Vec::with_capacity(want);
    let mut scratch = Vec::new();
    let mut in_flight = IN_FLIGHT;
    let mut empty_said = false;
    while in_flight > 0 {
        let asked = Instant::now();
        let mask = match c.reply(want, &mut buf) {
            Ok(m) => m,
            Err(super::iiod::Error::Server(code)) => {
                return Some(Ended::Refused(format!("the server answered {code}")))
            }
            Err(e) => return lost(e),
        };
        in_flight -= 1;
        let now = Instant::now();
        wait_us.fetch_add(
            now.duration_since(asked).as_micros() as u64,
            Ordering::Relaxed,
        );
        if active.load(Ordering::SeqCst) {
            if let Err(e) = c.request(RX, want) {
                return lost(e);
            }
            in_flight += 1;
        }
        let r = rate.load(Ordering::Relaxed);
        if r != known_rate {
            known_rate = r;
            losses = Losses::new(f64::from_bits(r), BLOCK_PAIRS as u64);
        }
        let Some(mask) = mask else {
            // An empty reply: measured when another client widened the
            // buffer, after a stall of seconds.
            if !empty_said {
                log(
                    ctx,
                    "PlutoSky: the radio answered a block with nothing, as it does when \
                     another program changes the stream's channels; the gap is counted"
                        .into(),
                );
                empty_said = true;
            }
            continue;
        };
        let Some(pairs) = pick_rx1(&mask, &buf, &mut scratch) else {
            return Some(Ended::Refused(format!(
                "a reply without RX1 in its channels (mask {mask})"
            )));
        };
        let n = (pairs.len() / PAIR_BYTES) as u64;
        let dropped = losses.arrived(n, now.duration_since(asked), now);
        crate::hardware::intake::deliver(ctx, pairs, dropped, now);
        work_us.fetch_add(now.elapsed().as_micros() as u64, Ordering::Relaxed);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLOCK: u64 = 65_536;

    fn at(t0: Instant, ms: u64) -> Instant {
        t0 + Duration::from_millis(ms)
    }

    /// A reader keeping up loses nothing, whatever the jitter.
    #[test]
    fn a_reader_that_keeps_up_counts_nothing() {
        // 65536 pairs at 3 MS/s: 21.8 ms a block.
        let mut l = Losses::new(3e6, BLOCK);
        let t0 = Instant::now();
        let wait = Duration::from_millis(20);
        for k in 0..2000u64 {
            let jitter = (k % 3) * 2;
            assert_eq!(
                l.arrived(BLOCK, wait, at(t0, k * 21_845 / 1000 + jitter)),
                0,
                "block {k}"
            );
        }
    }

    /// A stall longer than the ring holds: the blocks it spans are counted,
    /// once, when the reader has caught up again.
    #[test]
    fn a_stall_beyond_the_ring_is_counted_in_whole_blocks() {
        let mut l = Losses::new(3e6, BLOCK);
        let t0 = Instant::now();
        let full = Duration::from_millis(21);
        let quick = Duration::from_millis(0);
        let mut t = 0u64;
        for _ in 0..10 {
            assert_eq!(l.arrived(BLOCK, full, at(t0, t)), 0);
            t += 22;
        }
        // 300 ms away: about 13.7 blocks made, the ring kept 4 of them.
        t += 300;
        for _ in 0..4 {
            assert_eq!(
                l.arrived(BLOCK, quick, at(t0, t)),
                0,
                "drained from the ring"
            );
        }
        t += 22;
        let dropped = l.arrived(BLOCK, full, at(t0, t));
        assert_eq!(dropped % BLOCK, 0, "whole blocks");
        assert!(
            (9 * BLOCK..=11 * BLOCK).contains(&dropped),
            "{}",
            dropped / BLOCK
        );
        t += 22;
        assert_eq!(l.arrived(BLOCK, full, at(t0, t)), 0, "said once");
    }

    /// A radio clock a few ppm off never adds up to a block.
    #[test]
    fn a_few_ppm_between_the_clocks_never_adds_up() {
        let mut l = Losses::new(3e6, BLOCK);
        let t0 = Instant::now();
        // The radio 10 ppm slow against the host, for six hours.
        let period_us = 65_536.0 / 3e6 * 1e6 * (1.0 + 10e-6);
        let wait = Duration::from_millis(20);
        for k in 0..(6 * 3600 * 46u64) {
            let now = t0 + Duration::from_micros((k as f64 * period_us) as u64);
            assert_eq!(l.arrived(BLOCK, wait, now), 0, "block {k}");
        }
    }

    #[test]
    fn rx1_is_picked_out_of_a_wider_mask() {
        let mut scratch = Vec::new();
        let two: Vec<u8> = (0..8).collect();
        assert_eq!(pick_rx1("00000003", &two, &mut scratch).unwrap(), &two[..]);
        // Four channels per sample: RX1 I, Q, then RX2 I, Q.
        let four: Vec<u8> = (0..16).collect();
        assert_eq!(
            pick_rx1("0000000f", &four, &mut scratch).unwrap(),
            &[0, 1, 2, 3, 8, 9, 10, 11]
        );
        assert!(
            pick_rx1("0000000c", &four, &mut scratch).is_none(),
            "no RX1"
        );
        assert!(pick_rx1("zz", &four, &mut scratch).is_none());
    }
}
