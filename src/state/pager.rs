// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! What the pager receiver has heard, and what its feed did.
//!
//! The pages are written by `signal::pocsag::worker` and read by the Radio
//! views. Each is held behind an `Arc`: the whole state is cloned for every
//! frame, and a page carries every codeword it was read from.
//!
//! **What the view shows is the user's** ([`PagerView`]), written only by
//! the keys: which page, which screen of it, masked or not, whose pager, day
//! or night. The worker never touches it.

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

/// What the pager view shows, set by its keys.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PagerView {
    /// The page on the display, by its number: a new page arriving does not
    /// move it. `None` follows the newest, as a pager shows what just came.
    pub selected: Option<u64>,
    /// Which screen of a long page READ is on.
    pub screen: usize,
    /// Text shown as its kind and length only, everywhere at once.
    pub masked: bool,
    /// The address this pager answers to. `None` is monitor mode: every
    /// address.
    pub mine: Option<u32>,
    /// The night backlight.
    pub night: bool,
    /// The display's colour by name (`theme::display_names`); `None` is the
    /// theme's own.
    pub display: Option<String>,
    /// Pages read, by number: opened with READ.
    pub read: std::collections::BTreeSet<u64>,
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
    pub view: PagerView,
}

impl PagerState {
    /// The pages this pager shows, oldest first: every one, or only its own
    /// address's.
    pub fn shown(&self) -> Vec<&Arc<PageEntry>> {
        self.inbox
            .iter()
            .filter(|e| self.view.mine.is_none_or(|ric| e.page.ric == ric))
            .collect()
    }

    /// The page on the display: the selected one while it is still shown,
    /// else the newest shown.
    pub fn current(&self) -> Option<&Arc<PageEntry>> {
        let shown = self.shown();
        self.view
            .selected
            .and_then(|n| shown.iter().find(|e| e.n == n).copied())
            .or_else(|| shown.last().copied())
    }

    /// Shown pages not yet read.
    pub fn unread(&self) -> usize {
        self.shown()
            .iter()
            .filter(|e| !self.view.read.contains(&e.n))
            .count()
    }

    /// The page before or after the current one, and its first screen. At
    /// either end it stays: a list that wraps under the reader loses them.
    pub fn step(&mut self, forward: bool) {
        let shown: Vec<u64> = self.shown().iter().map(|e| e.n).collect();
        let Some(at) = self
            .current()
            .and_then(|c| shown.iter().position(|&n| n == c.n))
        else {
            return;
        };
        let next = if forward {
            (at + 1).min(shown.len() - 1)
        } else {
            at.saturating_sub(1)
        };
        // Back on the newest is following again.
        self.view.selected = (next + 1 < shown.len()).then(|| shown[next]);
        self.view.screen = 0;
    }

    /// READ: the page marked read, and on to its next screen. How many
    /// screens a page has depends on the display's size, which only the face
    /// knows; it takes this count modulo its own ([`Self::screen_of`]).
    pub fn read_on(&mut self) {
        let Some(n) = self.current().map(|e| e.n) else {
            return;
        };
        self.view.read.insert(n);
        self.view.screen = self.view.screen.wrapping_add(1);
    }

    /// The screen READ is on, for a page of `screens`.
    pub fn screen_of(&self, screens: usize) -> usize {
        self.view.screen % screens.max(1)
    }

    /// Make the current page's address this pager's, or go back to every
    /// address.
    pub fn toggle_mine(&mut self) {
        self.view.mine = match self.view.mine {
            Some(_) => None,
            None => self.current().map(|e| e.page.ric),
        };
        self.view.selected = None;
        self.view.screen = 0;
    }

    /// The next display colour: the theme's own, then each of
    /// `theme::display_names` in turn, and round again.
    pub fn next_display(&mut self) {
        let names = crate::theme::display_names();
        let at = self
            .view
            .display
            .as_deref()
            .and_then(|d| names.iter().position(|n| *n == d));
        self.view.display = match at {
            None => names.first().map(|n| n.to_string()),
            Some(i) => names.get(i + 1).map(|n| n.to_string()),
        };
    }

    /// Empty the inbox. The session's count stays: those pages were received.
    pub fn clear(&mut self) {
        self.inbox.clear();
        self.view.selected = None;
        self.view.screen = 0;
        self.view.read.clear();
    }

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
            if let Some(gone) = self.inbox.pop_front() {
                self.view.read.remove(&gone.n);
            }
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

    fn with(rics: &[u32]) -> PagerState {
        let mut s = PagerState::default();
        let now = Instant::now();
        for &ric in rics {
            let mut p = page();
            p.ric = ric;
            s.push(p, link(), 439_987_500, now, 0.0);
        }
        s
    }

    /// The display follows the newest until a page is chosen, and a chosen
    /// page stays chosen when a new one arrives.
    #[test]
    fn the_display_follows_until_a_page_is_chosen() {
        let mut s = with(&[8, 16, 24]);
        assert_eq!(s.current().map(|e| e.n), Some(3));
        s.step(false);
        assert_eq!(s.current().map(|e| e.n), Some(2));
        s.push(page(), link(), 0, Instant::now(), 0.0);
        assert_eq!(
            s.current().map(|e| e.n),
            Some(2),
            "a new page does not move it"
        );
        s.step(true);
        s.step(true);
        assert_eq!(
            (s.current().map(|e| e.n), s.view.selected),
            (Some(4), None),
            "following again"
        );
        s.step(true);
        assert_eq!(s.current().map(|e| e.n), Some(4), "stays at the end");
    }

    /// My pager: only its address's pages, counted unread until read.
    #[test]
    fn my_pager_shows_its_own_pages() {
        let mut s = with(&[8, 16, 8]);
        s.toggle_mine();
        assert_eq!(s.view.mine, Some(8));
        assert_eq!(s.shown().len(), 2);
        assert_eq!(s.unread(), 2);
        s.read_on();
        assert_eq!(s.unread(), 1);
        s.toggle_mine();
        assert_eq!((s.view.mine, s.shown().len()), (None, 3));
    }

    /// READ steps through a long page's screens and round again.
    #[test]
    fn read_steps_through_the_screens() {
        let mut s = with(&[8]);
        s.read_on();
        s.read_on();
        assert_eq!(s.screen_of(3), 2);
        s.read_on();
        assert_eq!(s.screen_of(3), 0);
    }

    /// The display colours go round, the theme's own first and last.
    #[test]
    fn the_display_colours_go_round() {
        let mut s = PagerState::default();
        let mut seen = vec![s.view.display.clone()];
        for _ in 0..crate::theme::display_names().len() + 1 {
            s.next_display();
            seen.push(s.view.display.clone());
        }
        assert_eq!(seen.first(), Some(&None));
        assert_eq!(seen.last(), Some(&None), "back to the theme's own");
        assert_eq!(seen[1].as_deref(), Some("green"));
    }

    /// Clearing empties the inbox and keeps the session's count.
    #[test]
    fn clearing_keeps_the_count() {
        let mut s = with(&[8, 16]);
        s.clear();
        assert!(s.inbox.is_empty());
        assert_eq!(s.received, 2);
        assert!(s.current().is_none());
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
