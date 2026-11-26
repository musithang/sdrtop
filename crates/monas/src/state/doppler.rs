// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The doppler lab's carriers and what has been measured on them.
//!
//! **Bounded in values, not rows.** A row's length is its carrier's own
//! (`2 · span / resolution`), so a fixed row count would hold four hours of
//! the ionosphere and ten seconds of a satellite. [`HISTORY_VALUES`] holds
//! either for as long as the same memory allows, and what it lets go is
//! counted, so a client asking for rows older than the history is told how
//! many it missed rather than handed a silent gap.

use std::collections::VecDeque;
use std::time::Instant;

use crate::signal::doppler::{CarrierSetting, Line, Row};
use crate::state::PagerFeed;

/// Bin values kept per carrier: four hours of the ionosphere's 500 bins every
/// 5 s, or about forty seconds of a satellite's 2000 every 50 ms.
pub const HISTORY_VALUES: usize = 1_500_000;

/// Differential rows kept: the ionosphere's four hours.
pub const DIFFERENTIAL_ROWS: usize = 3_000;

/// One measured window, and when it was published.
#[derive(Clone, Debug)]
pub struct StoredRow {
    pub row: Row,
    pub at: Instant,
    /// The same moment on the wall clock, for a reader on another machine.
    pub at_unix_ms: u64,
}

/// One carrier: its setting, why it is not being measured if it is not, and
/// its rows.
#[derive(Clone, Debug)]
pub struct Track {
    pub setting: CarrierSetting,
    /// Why the tuning cannot hold this carrier, when it cannot.
    pub refused: Option<String>,
    pub rows: VecDeque<StoredRow>,
    /// Rows let go to the value bound since the carrier was chosen.
    pub evicted: u64,
    /// Windows thrown away because the tuning changed inside them.
    pub discarded: u64,
}

impl Track {
    pub fn new(setting: CarrierSetting) -> Track {
        Track {
            setting,
            refused: None,
            rows: VecDeque::new(),
            evicted: 0,
            discarded: 0,
        }
    }

    /// Keep `row`, letting the oldest go while the history holds more than
    /// [`HISTORY_VALUES`].
    pub fn push(&mut self, row: Row, at: Instant, at_unix_ms: u64) {
        let per = row.bins_db.len().max(1);
        self.rows.push_back(StoredRow {
            row,
            at,
            at_unix_ms,
        });
        while self.rows.len() * per > HISTORY_VALUES {
            self.rows.pop_front();
            self.evicted += 1;
        }
    }
}

/// A carrier's shift against the first's, for one window.
#[derive(Clone, Debug)]
pub struct DiffRow {
    pub seq: u64,
    /// The carrier differenced against the first.
    pub fi_hz: u64,
    pub line: Result<Line, String>,
    pub at: Instant,
    pub at_unix_ms: u64,
}

/// The doppler lab's state.
#[derive(Clone, Debug, Default)]
pub struct DopplerState {
    /// The carriers chosen, in order: the first is the one the others are
    /// differenced against.
    pub carriers: Vec<CarrierSetting>,
    pub tracks: Vec<Track>,
    pub differential: VecDeque<DiffRow>,
    /// Why the carriers cannot be differenced, when they cannot.
    pub differential_refused: Option<String>,
    /// The feed's account, kept as the pager's is.
    pub feed: PagerFeed,
    /// The highest window number given out, kept across new carriers and
    /// the radio opened again, so numbers only rise and `since` stays good.
    pub last_seq: Option<u64>,
}

impl DopplerState {
    /// Choose the carriers: each starts a fresh history.
    pub fn set_carriers(&mut self, carriers: Vec<CarrierSetting>) {
        self.tracks = carriers.iter().map(|c| Track::new(*c)).collect();
        self.differential.clear();
        self.differential_refused = crate::signal::doppler::pair(&carriers).err();
        self.carriers = carriers;
    }

    /// Keep a differential row, the oldest let go past [`DIFFERENTIAL_ROWS`].
    pub fn push_differential(&mut self, d: DiffRow) {
        self.differential.push_back(d);
        while self.differential.len() > DIFFERENTIAL_ROWS {
            self.differential.pop_front();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::doppler::{CarrierSetting, Row};
    use std::time::Instant;

    fn row(seq: u64, bins: usize) -> Row {
        Row {
            seq,
            end_pair: seq,
            window_s: 0.1,
            bin_hz: 10.0,
            start_hz: -10_000.0,
            bins_db: vec![0.0; bins],
            line: Err("no line above the noise".into()),
            snr_db: None,
            feed_loss: false,
        }
    }

    /// The history is bounded in values, not rows, because a row's length is
    /// its carrier's own; what it let go is counted.
    #[test]
    fn history_is_bounded_in_values_and_counts_what_it_let_go() {
        let mut t = Track::new(CarrierSetting {
            hz: 1,
            span_hz: 10_000.0,
            resolution_hz: 10.0,
        });
        for seq in 0..800 {
            t.push(row(seq, 2000), Instant::now(), 0);
        }
        assert_eq!(t.rows.len(), 750);
        assert_eq!(t.evicted, 50);
        assert_eq!(t.rows.front().unwrap().row.seq, 50);
    }

    /// Choosing carriers starts each one's history afresh.
    #[test]
    fn choosing_carriers_starts_their_histories() {
        let mut d = DopplerState::default();
        let s = CarrierSetting {
            hz: 9_996_000,
            span_hz: 25.0,
            resolution_hz: 0.1,
        };
        d.set_carriers(vec![s]);
        d.tracks[0].push(row(1, 500), Instant::now(), 0);
        d.set_carriers(vec![
            s,
            CarrierSetting {
                hz: 10_000_000,
                ..s
            },
        ]);
        assert_eq!(d.tracks.len(), 2);
        assert!(d.tracks.iter().all(|t| t.rows.is_empty()));
    }
}
