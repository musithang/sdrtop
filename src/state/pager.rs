// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! What the pager receiver has heard, and what its feed did.
//!
//! The pages are written by `signal::pocsag::worker` and read by the Radio
//! views. Each is held behind an `Arc`: the whole state is cloned for every
//! frame, and a page carries every codeword it was read from.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

use crate::signal::pocsag::demod::Link;
use crate::signal::pocsag::page::Page;

/// Pages kept: the newest this many, the oldest let go. A pager keeps about
/// twenty; a bench session keeps more, and a page is small.
pub const INBOX_MAX: usize = 500;

/// One page as it was received.
#[derive(Clone, Debug, PartialEq)]
pub struct PageEntry {
    /// Its number in this session, from 1, never reused.
    pub n: u64,
    /// When it was received: for the display (wall clock) and for staleness
    /// (monotonic).
    pub unix_s: f64,
    pub at: Instant,
    /// The tuning it was received at.
    pub centre_hz: u64,
    pub page: Page,
    pub link: Link,
}

/// The pager feed's account: what reached the worker, what did not, and
/// what the work costs. The same quantities the NET feed counts
/// (`state::NetDecodeHealth`), without the 2.4 GHz decoders' own funnels.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PagerFeed {
    pub blocks_in: u64,
    /// Times the stream did not continue the block before: a gap or a new
    /// stream. Not the first block of a run.
    pub gaps: u64,
    /// A floor on blocks lost, summed over those gaps.
    pub blocks_lost: u64,
    /// Blocks the bounded feed refused in the last poll window, and since
    /// the radio was opened.
    pub refused: u64,
    pub refused_session: u64,
    pub peak_depth: u64,
    pub last_block: Option<Instant>,
    /// When anything was last lost: a gap, or a refusal.
    pub last_loss: Option<Instant>,
    /// The share of real time the work takes: channel and decoder over the
    /// stream time they covered. Above `1.0` the worker falls behind and the
    /// feed starts refusing. `None` until measured.
    pub load: Option<f64>,
}

#[derive(Clone, Debug, Default)]
pub struct PagerState {
    /// Received pages, oldest first, at most [`INBOX_MAX`].
    pub inbox: VecDeque<Arc<PageEntry>>,
    /// Pages received this session, including any let go from the inbox.
    pub received: u64,
    /// The rate a receiver is locked at now: a transmission is being read.
    pub locked: Option<u32>,
    /// The channel's actual sample rate, once a block has built it.
    pub channel_rate: Option<f64>,
    pub feed: PagerFeed,
}

impl PagerState {
    /// Add a received page, numbering it and letting the oldest go past
    /// [`INBOX_MAX`].
    pub fn push(&mut self, page: Page, link: Link, centre_hz: u64, at: Instant, unix_s: f64) {
        self.received += 1;
        self.inbox.push_back(Arc::new(PageEntry {
            n: self.received,
            unix_s,
            at,
            centre_hz,
            page,
            link,
        }));
        while self.inbox.len() > INBOX_MAX {
            self.inbox.pop_front();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::pocsag::page::{Content, Decided, End, Function};

    fn page() -> Page {
        Page {
            ric: 8,
            function: Function::A,
            content: Content::Tone,
            decided: Decided::NoMessage,
            end: End::Idle,
            codewords: Vec::new(),
        }
    }

    fn link() -> Link {
        Link {
            baud: 1200,
            measured_baud: 1200.0,
            clock_ppm: 0.0,
            shift_hz: 4500.0,
            offset_hz: 0.0,
            decision_snr_db: None,
            level_dbfs: -30.0,
            preamble_bits: 576,
            sync_errors: 0,
            one_is_higher: false,
            batches: 1,
        }
    }

    /// Numbered from one, never reused, the oldest let go past the cap and
    /// still counted.
    #[test]
    fn the_inbox_numbers_and_caps() {
        let mut s = PagerState::default();
        let now = Instant::now();
        for _ in 0..INBOX_MAX + 3 {
            s.push(page(), link(), 439_987_500, now, 0.0);
        }
        assert_eq!(s.received, INBOX_MAX as u64 + 3);
        assert_eq!(s.inbox.len(), INBOX_MAX);
        assert_eq!(s.inbox.front().map(|e| e.n), Some(4));
        assert_eq!(s.inbox.back().map(|e| e.n), Some(INBOX_MAX as u64 + 3));
    }
}
