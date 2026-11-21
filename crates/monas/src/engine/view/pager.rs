// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The pager: the pages it holds, each with the transmission it came in,
//! as the pager's screen shows them.
//!
//! What the screen shows and nothing more: only the pages addressed to its
//! own address when it answers to one, and with the text masked, only each
//! page's kind and length, the same words the screen uses.

use serde::Serialize;

use super::{Clock, Group, Reading};
use crate::engine::testimony::{Staleness, Testimony};
use crate::signal::pocsag::page::{Content, Decided, End};
use crate::state::{PagerFeed, SdrMetrics};

/// A sample feed's account: what it took, what it lost, and how hard its
/// worker works.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Feed {
    pub blocks_in: u64,
    /// Breaks in the stream the worker saw.
    pub gaps: u64,
    /// Blocks lost inside those breaks.
    pub blocks_lost: u64,
    /// Blocks the feed had no room for, lately and over the session.
    pub refused: u64,
    pub refused_session: u64,
    /// The worker's time per block over the stream time it covered: above 1
    /// it falls behind.
    pub load: Option<f64>,
}

impl Feed {
    pub fn of(f: &PagerFeed) -> Feed {
        Feed {
            blocks_in: f.blocks_in,
            gaps: f.gaps,
            blocks_lost: f.blocks_lost,
            refused: f.refused,
            refused_session: f.refused_session,
            load: f.load,
        }
    }
}

/// The pager's summary, for the snapshot.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Pager {
    /// Pages received this session, including any the inbox let go.
    pub received_session: u64,
    /// Pages held, and of those the ones shown.
    pub held: usize,
    pub shown: usize,
    /// The rate a receiver is locked at now: a transmission is being read.
    pub locked_baud: Option<u32>,
    pub masked: bool,
    /// The address the pager answers to; none for every address.
    pub mine: Option<u32>,
    pub feed: Feed,
}

/// The pages, for their own route.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Pages {
    pub masked: bool,
    pub mine: Option<u32>,
    /// Oldest first, each with its number; `since` leaves out the ones
    /// numbered up to it.
    pub pages: Vec<PageView>,
}

/// One page and the transmission it came in.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PageView {
    /// Its number this session, rising.
    pub n: u64,
    pub unix_s: f64,
    pub centre_hz: u64,
    /// The 21-bit address.
    pub ric: u32,
    /// `A` to `D`.
    pub function: char,
    /// `tone`, `numeric` or `alpha`.
    pub kind: &'static str,
    /// Digits or characters; none for a tone.
    pub length: Option<usize>,
    /// The text as the screen shows it, or why it is not shown.
    pub text: Reading<String>,
    /// How the kind was decided: `by_function`, `over_function`,
    /// `by_content`, `ambiguous`, or `no_message` (an address alone).
    pub decided: &'static str,
    /// How the page ended: `address`, `idle`, `undecipherable` or `cut`.
    pub end: &'static str,
    pub bits_repaired: u32,
    pub codewords_refused: usize,
    pub link: Link,
}

/// The transmission's physics.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Link {
    pub baud: u32,
    /// The rate it ran at, by this radio's clock.
    pub measured_baud: f64,
    pub clock_ppm: f64,
    pub shift_hz: f64,
    pub offset_hz: f64,
    pub decision_snr_db: Option<f64>,
    pub level_dbfs: f64,
    pub preamble_bits: u32,
    pub sync_errors: u32,
    pub batches: u32,
}

/// A page's kind and length, as the screen names them.
pub fn kind_and_length(content: &Content) -> (&'static str, Option<usize>) {
    match content {
        Content::Tone => ("tone", None),
        Content::Numeric(c) => ("numeric", Some(c.len())),
        Content::Alpha(c) => ("alpha", Some(c.len())),
    }
}

/// What a masked page shows instead of its text: its kind and length.
/// `None` for a tone, which has no text to mask.
pub fn masked_text(content: &Content) -> Option<String> {
    match kind_and_length(content) {
        (_, None) => None,
        ("numeric", Some(n)) => Some(format!("numeric, {n} digits")),
        (kind, Some(n)) => Some(format!("{kind}, {n} characters")),
    }
}

/// The pager's group for the snapshot. Stale when the radio is not
/// streaming; dated by the last block its feed took.
pub fn pager(m: &SdrMetrics, clock: Clock) -> Group<Pager> {
    let p = &m.pager;
    Group {
        testimony: testimony(m, clock),
        readings: Pager {
            received_session: p.received,
            held: p.inbox.len(),
            shown: p.shown().len(),
            locked_baud: p.locked,
            masked: p.view.masked,
            mine: p.view.mine,
            feed: Feed::of(&p.feed),
        },
    }
}

/// The pages shown, numbered past `since`, with the pager's testimony.
pub fn pages(m: &SdrMetrics, clock: Clock, since: Option<u64>) -> Group<Pages> {
    let p = &m.pager;
    let masked = p.view.masked;
    let pages = p
        .shown()
        .into_iter()
        .filter(|e| since.is_none_or(|s| e.n > s))
        .map(|e| {
            let (kind, length) = kind_and_length(&e.page.content);
            let text = match (&e.page.content, masked) {
                (Content::Tone, _) => Reading::Absent("a tone page carries no text".into()),
                (content, true) => Reading::Absent(format!(
                    "masked: {}",
                    masked_text(content).unwrap_or_default()
                )),
                (_, false) => Reading::Value(e.page.text()),
            };
            PageView {
                n: e.n,
                unix_s: e.unix_s,
                centre_hz: e.centre_hz,
                ric: e.page.ric,
                function: e.page.function.letter(),
                kind,
                length,
                text,
                decided: match e.page.decided {
                    Decided::ByFunction => "by_function",
                    Decided::OverFunction => "over_function",
                    Decided::ByContent => "by_content",
                    Decided::Ambiguous => "ambiguous",
                    Decided::NoMessage => "no_message",
                },
                end: match e.page.end {
                    End::Address => "address",
                    End::Idle => "idle",
                    End::Undecipherable => "undecipherable",
                    End::Cut => "cut",
                },
                bits_repaired: e.page.repairs(),
                codewords_refused: e.page.refused(),
                link: Link {
                    baud: e.link.baud,
                    measured_baud: e.link.measured_baud,
                    clock_ppm: e.link.clock_ppm,
                    shift_hz: e.link.shift_hz,
                    offset_hz: e.link.offset_hz,
                    decision_snr_db: e.link.decision_snr_db,
                    level_dbfs: e.link.level_dbfs,
                    preamble_bits: e.link.preamble_bits,
                    sync_errors: e.link.sync_errors,
                    batches: e.link.batches,
                },
            }
        })
        .collect();
    Group {
        testimony: testimony(m, clock),
        readings: Pages {
            masked,
            mine: p.view.mine,
            pages,
        },
    }
}

fn testimony(m: &SdrMetrics, clock: Clock) -> Testimony {
    Testimony::resolve(
        m,
        Staleness::NotStreaming,
        None,
        m.pager.feed.last_block.map(|t| clock.unix_ms_of(t)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::pocsag::demod::Link as Air;
    use crate::signal::pocsag::page::{Function, Page};

    fn clock() -> Clock {
        Clock {
            now: std::time::Instant::now(),
            unix_ms: 1_788_632_561_000,
        }
    }

    fn with_pages(pages: &[(u32, &str)]) -> SdrMetrics {
        let mut m = SdrMetrics::fixture().streaming();
        for (ric, text) in pages {
            m.pager.push(
                Page {
                    ric: *ric,
                    function: Function::D,
                    content: Content::Alpha(text.bytes().map(Some).collect()),
                    decided: Decided::ByFunction,
                    end: End::Idle,
                    codewords: Vec::new(),
                },
                Air {
                    baud: 1200,
                    measured_baud: 1200.0,
                    clock_ppm: 0.0,
                    shift_hz: 4500.0,
                    offset_hz: 0.0,
                    decision_snr_db: Some(30.0),
                    level_dbfs: -38.0,
                    preamble_bits: 576,
                    sync_errors: 0,
                    one_is_higher: false,
                    batches: 1,
                },
                439_987_500,
                std::time::Instant::now(),
                0.0,
            );
        }
        m
    }

    #[test]
    fn a_masked_page_is_its_kind_and_length() {
        assert_eq!(
            masked_text(&Content::Numeric(vec![None; 7])),
            Some("numeric, 7 digits".into())
        );
        assert_eq!(
            masked_text(&Content::Alpha(vec![Some(b'A'); 12])),
            Some("alpha, 12 characters".into())
        );
        assert_eq!(masked_text(&Content::Tone), None);
    }

    #[test]
    fn with_no_pages_the_list_is_empty_and_the_pager_stale() {
        let g = pages(&SdrMetrics::fixture(), clock(), None);
        assert!(g.readings.pages.is_empty());
        assert!(g.testimony.stale);
    }

    /// The API shows what the screen shows: the text, or with the pager
    /// masked, its kind and length; only the pager's own address when it
    /// answers to one; and only the pages after `since`.
    #[test]
    fn the_pages_are_what_the_screen_shows() {
        let mut m = with_pages(&[(1234, "HELLO WORLD!"), (5678, "SECOND")]);
        let all = pages(&m, clock(), None).readings.pages;
        assert_eq!(all.len(), 2);
        assert_eq!(
            all[0].text.value().map(String::as_str),
            Some("HELLO WORLD!")
        );
        assert_eq!(all[0].link.baud, 1200);

        m.pager.view.masked = true;
        let masked = pages(&m, clock(), None).readings.pages;
        assert_eq!(
            masked[0].text.absent(),
            Some("masked: alpha, 12 characters")
        );
        let json = serde_json::to_string(&masked).unwrap();
        assert!(!json.contains("HELLO"), "{json}");

        m.pager.view.mine = Some(5678);
        let mine = pages(&m, clock(), None).readings.pages;
        assert_eq!(mine.iter().map(|p| p.ric).collect::<Vec<_>>(), vec![5678]);

        m.pager.view.mine = None;
        let first = all[0].n;
        let since = pages(&m, clock(), Some(first)).readings.pages;
        assert_eq!(
            since.iter().map(|p| p.n).collect::<Vec<_>>(),
            vec![all[1].n]
        );
    }
}
