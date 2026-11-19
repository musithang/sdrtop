// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The fourth feed: raw blocks to the IQ recorder, while one runs.
//!
//! The FFT, demod and NET feeds each drop a block they cannot take, and this
//! one does too, for the same reason: the USB callback must never wait for a
//! disk. What it does differently is **say so in-band**. A refused block still
//! goes down the channel, as a few integers without its bytes, so the writer
//! knows to the sample where the file has a hole and why, rather than inferring
//! it from two positions afterwards.
//!
//! The budget is in bytes, not blocks, because a block is 16 kB on one backend
//! and 256 kB on another: a count that suits one is either a trickle or a
//! gigabyte on the other.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

/// How much raw IQ may wait for the disk at once.
///
/// 32 MiB is 0.8 s at 20 Msps of 8-bit pairs and about seven seconds from an
/// RTL-SDR: long enough to ride out a disk's stall, and small enough that a
/// slow SD card cannot take a Raspberry Pi's memory with it.
pub const RECORD_QUEUE_BYTES: usize = 32 << 20;

/// Where one block sits in the stream, and what the radio was doing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlockAt {
    /// Stream position of the block's first pair (`StreamBlock::first_pair`).
    pub first_pair: u64,
    pub pairs: u64,
    /// Pairs the driver reported lost just before this block.
    pub driver_dropped: u64,
    pub centre_hz: u64,
    pub rate_hz: f64,
    /// When the block reached sdrtop, on the system clock, in Unix seconds.
    ///
    /// **The one account of time that does not come from the sample count.**
    /// A radio that delivers fewer samples than its rate and reports no drop
    /// (a HackRF short of USB bandwidth does exactly that) leaves positions
    /// that look unbroken and a count that runs slow; only a clock read as
    /// the blocks arrive can tell.
    pub arrived_unix: f64,
}

/// One reading of the radio's own drop count, as the rx loop hands it over:
/// how many drops since the last reading, and the stretch of the stream they
/// can lie in. See `hardware::SdrDevice::radio_drops`.
#[derive(Clone, Debug, PartialEq)]
pub enum RadioNote {
    Counted {
        events: u32,
        from_pair: u64,
        to_pair: u64,
        longest_bytes: u32,
    },
    /// The radio keeps a count and it could not be read.
    Unreadable(String),
}

/// What travels to the writer.
pub enum RecordMsg {
    /// A block to write, exactly as the radio delivered it, with the gain the
    /// app had set when it arrived.
    Block {
        at: BlockAt,
        bytes: Vec<u8>,
        gains: Vec<f64>,
        boost: bool,
    },
    /// A block the queue had no room for: where it was, not what it held.
    Refused { at: BlockAt },
    /// A reading of the radio's own drop count.
    Radio(RadioNote),
}

/// The recorder's end of `process_block`.
#[derive(Default)]
pub struct RecordTap {
    /// Read without the lock on every block, so an idle recorder costs one
    /// atomic load.
    armed: AtomicBool,
    tx: Mutex<Option<crossbeam_channel::Sender<RecordMsg>>>,
    /// Bytes handed to the writer and not yet written.
    in_flight: AtomicUsize,
}

impl RecordTap {
    pub fn armed(&self) -> bool {
        self.armed.load(Ordering::Relaxed)
    }

    /// Start sending to `tx`. A fresh recording starts with an empty budget:
    /// whatever the last one left queued went with its channel.
    pub fn arm(&self, tx: crossbeam_channel::Sender<RecordMsg>) {
        let mut slot = self.tx.lock().unwrap_or_else(|e| e.into_inner());
        self.in_flight.store(0, Ordering::Relaxed);
        *slot = Some(tx);
        self.armed.store(true, Ordering::Relaxed);
    }

    /// Stop sending. Dropping the sender is what tells the writer the stream
    /// of blocks has ended, once it has drained what was already queued.
    pub fn disarm(&self) {
        let mut slot = self.tx.lock().unwrap_or_else(|e| e.into_inner());
        self.armed.store(false, Ordering::Relaxed);
        *slot = None;
    }

    /// The writer has put `bytes` on disk, or thrown them away.
    pub fn release(&self, bytes: usize) {
        // Saturating, because a recording started after a disarm reset the
        // budget while an old writer may still be releasing into it. Written
        // as the compare-exchange loop `fetch_update` is: Rust 1.99 deprecates
        // that name for `try_update`, which the 1.88 floor does not have.
        let mut current = self.in_flight.load(Ordering::Relaxed);
        loop {
            let next = current.saturating_sub(bytes);
            match self.in_flight.compare_exchange_weak(
                current,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
    }

    /// Hand a reading of the radio's own drop count to a running recording.
    /// A few integers, so it never waits on the byte budget.
    pub fn radio(&self, note: RadioNote) {
        let slot = self.tx.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(tx) = slot.as_ref() {
            let _ = tx.send(RecordMsg::Radio(note));
        }
    }

    /// Hand one raw block over, or say where it was if there is no room.
    ///
    /// The copy is made only once the budget has room for it, so a full queue
    /// costs the callback nothing but the message.
    pub fn offer(&self, at: BlockAt, raw: &[u8], gains: Vec<f64>, boost: bool) {
        let slot = self.tx.lock().unwrap_or_else(|e| e.into_inner());
        let Some(tx) = slot.as_ref() else {
            return;
        };
        let len = raw.len();
        let before = self.in_flight.fetch_add(len, Ordering::Relaxed);
        let msg = if before + len > RECORD_QUEUE_BYTES {
            self.in_flight.fetch_sub(len, Ordering::Relaxed);
            RecordMsg::Refused { at }
        } else {
            RecordMsg::Block {
                at,
                bytes: raw.to_vec(),
                gains,
                boost,
            }
        };
        if let Err(e) = tx.send(msg) {
            // The writer has gone. Nothing will release these bytes.
            if let RecordMsg::Block { bytes, .. } = e.0 {
                self.release(bytes.len());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(first_pair: u64, pairs: u64) -> BlockAt {
        BlockAt {
            first_pair,
            pairs,
            driver_dropped: 0,
            centre_hz: 100_000_000,
            rate_hz: 2e6,
            arrived_unix: 0.0,
        }
    }

    /// Nothing is sent, and nothing copied, while no recording runs.
    #[test]
    fn an_idle_tap_sends_nothing() {
        let tap = RecordTap::default();
        assert!(!tap.armed());
        tap.offer(at(0, 4), &[0; 8], vec![], false);
        let (tx, rx) = crossbeam_channel::unbounded();
        tap.arm(tx);
        tap.disarm();
        tap.offer(at(4, 4), &[0; 8], vec![], false);
        assert!(rx.try_recv().is_err());
    }

    /// Past the byte budget a block goes as its position alone, and the budget
    /// comes back as the writer releases what it wrote.
    #[test]
    fn a_block_past_the_budget_is_refused_by_position() {
        let tap = RecordTap::default();
        let (tx, rx) = crossbeam_channel::unbounded();
        tap.arm(tx);
        let big = vec![7u8; RECORD_QUEUE_BYTES - 8];
        tap.offer(at(0, big.len() as u64 / 2), &big, vec![20.0], true);
        tap.offer(at(1, 8), &[1; 16], vec![20.0], true);
        match rx.try_recv().unwrap() {
            RecordMsg::Block {
                bytes,
                gains,
                boost,
                ..
            } => {
                assert_eq!(bytes.len(), big.len());
                assert_eq!((gains, boost), (vec![20.0], true));
            }
            RecordMsg::Refused { .. } | RecordMsg::Radio(_) => panic!("the first fits"),
        }
        match rx.try_recv().unwrap() {
            RecordMsg::Refused { at: a } => assert_eq!(a, at(1, 8)),
            RecordMsg::Block { .. } | RecordMsg::Radio(_) => panic!("16 bytes over the budget"),
        }
        tap.release(big.len());
        tap.offer(at(9, 8), &[1; 16], vec![], false);
        assert!(matches!(rx.try_recv().unwrap(), RecordMsg::Block { .. }));
    }

    /// Releasing more than is in flight (an old writer releasing into a
    /// budget a new recording reset) stops at zero rather than wrapping
    /// round to a budget that refuses everything.
    #[test]
    fn a_release_past_zero_stops_at_zero() {
        let tap = RecordTap::default();
        tap.in_flight.store(10, Ordering::Relaxed);
        tap.release(25);
        assert_eq!(tap.in_flight.load(Ordering::Relaxed), 0);
        tap.in_flight.store(10, Ordering::Relaxed);
        tap.release(4);
        assert_eq!(tap.in_flight.load(Ordering::Relaxed), 6);
    }
}
