// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Between the driver's thread and the work: a short queue and one thread.
//!
//! **The driver's thread must never wait for sdrtop.** libhackrf calls back on
//! its USB transfer thread, and a transfer is not handed back to the radio
//! until the callback returns. While it runs, the HackRF's own buffer, a few
//! milliseconds deep at 20 Msps, fills with nowhere to go, and what does not
//! fit is dropped inside the radio. The firmware counts those drops; the host
//! is never told. That is what `process_block` running on the callback thread
//! did, at five to seven milliseconds a block against a block of 6.55: a third
//! of the samples at 20 Msps and one in twenty at 10 went missing, and every
//! position downstream still read as unbroken.
//!
//! So the callback does [`super::process::arrive`], copies the block into a
//! recycled buffer and returns. This thread does
//! [`super::process::digest`]. When it falls behind, the queue refuses a
//! block, and that loss is sdrtop's own: counted, and at a known place in the
//! stream, because the positions were stamped before the queue. A loss that
//! is counted and placed is one every reader downstream can handle; a loss
//! inside the radio was one none of them could see.
//!
//! RTL-SDR and SoapySDR go through here too. Their drivers have deeper
//! buffers, but the rule is the same, and one path for all three is one path
//! to get right.

use std::sync::{Arc, OnceLock};
use std::time::Instant;

use crossbeam_channel::{Receiver, Sender, TrySendError};

use super::process::{arrive, digest, Arrival};
use super::traits::RxContext;

/// Blocks that may wait for the intake thread: about 50 ms of a HackRF at
/// 20 Msps, which rides out the longest stall measured on the callback
/// (24 ms) twice over, and a few tens of megabytes at most.
pub const INTAKE_DEPTH: usize = 8;

/// One block waiting to be digested.
struct Raw {
    bytes: Vec<u8>,
    arrival: Arrival,
}

/// The queue into the intake thread, and the buffers that travel it.
pub struct Intake {
    /// Set once the thread exists. Until then, and in the tests that build a
    /// context by hand, a block is digested where it arrives.
    tx: OnceLock<Sender<Raw>>,
    /// Emptied buffers coming back, so the callback copies into memory it
    /// already has instead of asking the allocator for 256 kB every block.
    pool_tx: Sender<Vec<u8>>,
    pool_rx: Receiver<Vec<u8>>,
}

impl Default for Intake {
    fn default() -> Self {
        let (pool_tx, pool_rx) = crossbeam_channel::bounded(INTAKE_DEPTH + 2);
        Self {
            tx: OnceLock::new(),
            pool_tx,
            pool_rx,
        }
    }
}

/// Start the intake thread for `ctx`. Once per context; a second call does
/// nothing.
pub fn spawn(ctx: &Arc<RxContext>) {
    let (tx, rx) = crossbeam_channel::bounded::<Raw>(INTAKE_DEPTH);
    if ctx.intake.tx.set(tx).is_err() {
        return;
    }
    let worker = Arc::clone(ctx);
    let started = std::thread::Builder::new()
        .name("rx-intake".to_string())
        .spawn(move || run(&worker, &rx));
    if let Err(e) = started {
        // Without the thread nothing drains the queue, and every block from
        // here on is lost: that is worth a line in the log.
        let mut m = ctx.metrics.lock().unwrap_or_else(|e| e.into_inner());
        m.push_log(format!("ERROR: cannot start the rx intake thread: {e}"));
    }
}

/// Hand one block over from the driver's thread.
pub fn deliver(ctx: &RxContext, buf: &[u8], dropped_pairs: u64, now: Instant) {
    let arrival = arrive(buf, ctx.geometry, dropped_pairs, ctx, now);
    let Some(tx) = ctx.intake.tx.get() else {
        digest(buf, ctx.geometry, &arrival, 0, ctx);
        return;
    };
    let mut bytes = ctx.intake.pool_rx.try_recv().unwrap_or_default();
    bytes.clear();
    bytes.extend_from_slice(buf);
    match tx.try_send(Raw { bytes, arrival }) {
        Ok(()) => {}
        // Full: this block is lost here, and the next one to be digested
        // finds the hole by its position.
        Err(TrySendError::Full(raw)) | Err(TrySendError::Disconnected(raw)) => {
            let _ = ctx.intake.pool_tx.try_send(raw.bytes);
        }
    }
}

/// Pairs missing between the block expected next and the one that came,
/// beyond what the driver reported: the blocks the queue refused. `None` for
/// the first block, and for a stream that started again from zero.
fn lost_before(expected: Option<u64>, arrival: &Arrival) -> u64 {
    match expected {
        Some(e) if arrival.first_pair >= e => {
            (arrival.first_pair - e).saturating_sub(arrival.dropped_pairs)
        }
        _ => 0,
    }
}

fn run(ctx: &RxContext, rx: &Receiver<Raw>) {
    let bytes_per_pair = ctx.geometry.bytes_per_pair() as u64;
    let mut expected: Option<u64> = None;
    for raw in rx.iter() {
        let lost = lost_before(expected, &raw.arrival);
        digest(&raw.bytes, ctx.geometry, &raw.arrival, lost, ctx);
        expected = Some(raw.arrival.first_pair + raw.bytes.len() as u64 / bytes_per_pair);
        let _ = ctx.intake.pool_tx.try_send(raw.bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::process::tests::{eight_bit, rx_ctx};

    fn arrival_at(ctx: &RxContext, first_pair: u64, dropped_pairs: u64) -> Arrival {
        let mut a = arrive(&[], eight_bit(), 0, ctx, Instant::now());
        a.first_pair = first_pair;
        a.dropped_pairs = dropped_pairs;
        a
    }

    /// The pairs the queue refused are the gap the driver did not explain;
    /// nothing is lost before the first block or across a restart.
    #[test]
    fn what_the_queue_lost_is_the_gap_the_driver_did_not_report() {
        let (ctx, ..) = rx_ctx();
        assert_eq!(lost_before(None, &arrival_at(&ctx, 5_000, 0)), 0);
        assert_eq!(lost_before(Some(100), &arrival_at(&ctx, 100, 0)), 0);
        assert_eq!(
            lost_before(Some(100), &arrival_at(&ctx, 130, 30)),
            0,
            "the driver's own"
        );
        assert_eq!(lost_before(Some(100), &arrival_at(&ctx, 400, 30)), 270);
        assert_eq!(
            lost_before(Some(100_000), &arrival_at(&ctx, 64, 0)),
            0,
            "a new stream"
        );
    }

    /// A stretch the queue lost is counted with the driver's drops, and the
    /// feeds are told a gap came before the block.
    #[test]
    fn a_lost_stretch_is_a_drop_and_a_gap_downstream() {
        let (ctx, _fft, _demod, net) = rx_ctx();
        let block = vec![1u8; 64];
        let a = arrive(&block, eight_bit(), 0, &ctx, Instant::now());
        digest(&block, eight_bit(), &a, 96, &ctx);
        assert_eq!(ctx.metrics.lock().unwrap().acc.drops, 96);
        assert_eq!(
            ctx.metrics.lock().unwrap().signal.driver_drops_session,
            0,
            "the queue's loss, not the driver's"
        );
        assert!(net.try_recv().unwrap().gap_before);
    }

    /// The radio's delivery is counted when the block arrives, so the USB
    /// figure is what crossed the link even when the work falls behind.
    #[test]
    fn what_the_radio_delivered_is_counted_on_arrival() {
        let (ctx, ..) = rx_ctx();
        arrive(&[0u8; 512], eight_bit(), 0, &ctx, Instant::now());
        assert_eq!(ctx.metrics.lock().unwrap().radio.bytes_since_last_poll, 512);
    }

    /// With the thread running, blocks are digested off the caller's thread,
    /// in order, at the positions stamped when they arrived.
    #[test]
    fn delivered_blocks_reach_the_feeds_in_order_from_the_intake_thread() {
        let (ctx, _fft, _demod, net) = rx_ctx();
        spawn(&ctx);
        for _ in 0..3 {
            deliver(&ctx, &[2u8; 64], 0, Instant::now());
        }
        let wait = std::time::Duration::from_secs(5);
        let firsts: Vec<u64> = (0..3)
            .map(|_| net.recv_timeout(wait).unwrap().first_pair)
            .collect();
        assert_eq!(firsts, vec![0, 32, 64]);
    }
}
