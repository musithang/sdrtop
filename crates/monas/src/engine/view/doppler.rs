// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The doppler lab's readings: each carrier's rows, and the differential.
//!
//! Every frequency here is an offset from a carrier's nominal frequency as
//! received through our oscillator, and the group says so in
//! [`RELATIVE_TO`], on every answer, because a wandering line in one
//! carrier's rows may be our crystal warming. Only the differential is free
//! of it.

use serde::Serialize;

use super::pager::Feed;
use super::{Clock, Group, Reading};
use crate::engine::testimony::{Staleness, Testimony};
use crate::signal::doppler::Line;
use crate::state::SdrMetrics;

/// What every frequency in this group is relative to.
pub const RELATIVE_TO: &str =
    "each carrier's nominal frequency as received through our oscillator, \
whose error is in every carrier's rows and cancels only in the differential";

/// A line: its offset in Hz, and that offset's standard uncertainty.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct LineView {
    pub hz: f64,
    pub sigma_hz: f64,
}

impl LineView {
    fn of(line: &Result<Line, String>) -> Reading<LineView> {
        match line {
            Ok(l) => Reading::Value(LineView {
                hz: l.hz,
                sigma_hz: l.sigma_hz,
            }),
            Err(why) => Reading::Absent(why.clone()),
        }
    }
}

/// One window of one carrier.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RowView {
    /// The window's number; carriers of one resolution share it.
    pub seq: u64,
    /// When the row was published, milliseconds since the Unix epoch.
    pub at_unix_ms: u64,
    pub window_s: f64,
    /// The width of one value of `bins_db`: the resolution.
    pub bin_hz: f64,
    /// The offset the first value of `bins_db` starts at, Hz.
    pub start_hz: f64,
    /// Power per resolution bin, dB over the row's median.
    pub bins_db: Vec<f32>,
    /// The strongest line in the span, which is the carrier only when nothing
    /// louder is inside it.
    pub line: Reading<LineView>,
    /// The strongest bin over the noise per bin, dB.
    pub snr_db: Option<f64>,
    /// Whether the feed lost samples inside this window.
    pub feed_loss: bool,
}

/// One carrier: its settings, why it is not measured if it is not, and its
/// rows.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CarrierView {
    pub hz: u64,
    pub span_hz: f64,
    pub resolution_hz: f64,
    /// Why the tuning cannot hold this carrier, when it cannot.
    pub refused: Option<String>,
    /// Windows thrown away because the tuning changed inside them.
    pub discarded: u64,
    /// Oldest first; `since` leaves out the ones numbered up to it.
    pub rows: Vec<RowView>,
}

/// One carrier's shift against the first's, for one window.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DiffView {
    pub seq: u64,
    pub at_unix_ms: u64,
    /// The carrier differenced against the first.
    pub fi_hz: u64,
    pub line: Reading<LineView>,
}

/// The doppler lab's readings, for their route.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Doppler {
    /// What every frequency here is relative to.
    pub relative_to: &'static str,
    pub carriers: Vec<CarrierView>,
    /// Oldest first; free of our oscillator.
    pub differential: Vec<DiffView>,
    /// Why there is no differential, when there cannot be one.
    pub differential_refused: Option<String>,
    /// Rows newer than `since` the history had already let go: a client
    /// polling too slowly is told what it missed rather than handed a gap.
    pub missed: u64,
    pub feed: Feed,
}

/// The doppler lab's readings newer than `since`, with their testimony.
pub fn doppler(m: &SdrMetrics, clock: Clock, since: Option<u64>) -> Group<Doppler> {
    let d = &m.doppler;
    let newer = |seq: u64| since.is_none_or(|s| seq > s);
    let carriers = d
        .tracks
        .iter()
        .map(|t| CarrierView {
            hz: t.setting.hz,
            span_hz: t.setting.span_hz,
            resolution_hz: t.setting.resolution_hz,
            refused: t.refused.clone(),
            discarded: t.discarded,
            rows: t
                .rows
                .iter()
                .filter(|r| newer(r.row.seq))
                .map(|r| RowView {
                    seq: r.row.seq,
                    at_unix_ms: clock.unix_ms_of(r.at),
                    window_s: r.row.window_s,
                    bin_hz: r.row.bin_hz,
                    start_hz: r.row.start_hz,
                    bins_db: r.row.bins_db.clone(),
                    line: LineView::of(&r.row.line),
                    snr_db: r.row.snr_db,
                    feed_loss: r.row.feed_loss,
                })
                .collect(),
        })
        .collect();
    let differential = d
        .differential
        .iter()
        .filter(|r| newer(r.seq))
        .map(|r| DiffView {
            seq: r.seq,
            at_unix_ms: clock.unix_ms_of(r.at),
            fi_hz: r.fi_hz,
            line: LineView::of(&r.line),
        })
        .collect();
    // Window numbers run one a hop, so the rows between `since` and the
    // oldest held are the ones let go.
    let missed = since.map_or(0, |s| {
        d.tracks
            .iter()
            .filter_map(|t| t.rows.front().map(|r| r.row.seq.saturating_sub(s + 1)))
            .max()
            .unwrap_or(0)
    });
    let mut testimony = Testimony::resolve(
        m,
        Staleness::NotStreaming,
        None,
        d.feed.last_block.map(|t| clock.unix_ms_of(t)),
    );
    testimony.absent = if d.carriers.is_empty() {
        Some("no carrier chosen: send the doppler command with one to four frequencies".to_string())
    } else if !m.mode.feeds.doppler {
        Some(format!(
            "not computing: the mode is {}; these rows are as they were when it left lab_doppler",
            if m.mode.view.is_empty() {
                "spectrum"
            } else {
                &m.mode.view
            }
        ))
    } else {
        None
    };
    Group {
        testimony,
        readings: Doppler {
            relative_to: RELATIVE_TO,
            carriers,
            differential,
            differential_refused: d.differential_refused.clone(),
            missed,
            feed: Feed::of(&d.feed),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::doppler::{CarrierSetting, Line, Row};
    use crate::state::doppler::DiffRow;
    use std::time::Instant;

    fn clock() -> Clock {
        Clock {
            now: Instant::now(),
            unix_ms: 1_795_000_000_000,
        }
    }

    fn row(seq: u64, line: Result<Line, String>) -> Row {
        Row {
            seq,
            end_pair: seq * 3_840_000,
            window_s: 10.0,
            bin_hz: 0.1,
            start_hz: -25.0,
            bins_db: vec![0.0; 500],
            line,
            snr_db: Some(30.0),
            feed_loss: false,
        }
    }

    fn watching(seqs: std::ops::Range<u64>) -> SdrMetrics {
        let mut m = SdrMetrics::fixture().streaming();
        m.mode = crate::engine::mode::Mode::named("lab_doppler").unwrap();
        let s = CarrierSetting {
            hz: 9_996_000,
            span_hz: 25.0,
            resolution_hz: 0.1,
        };
        m.doppler.set_carriers(vec![
            s,
            CarrierSetting {
                hz: 10_000_000,
                ..s
            },
        ]);
        for seq in seqs {
            for t in m.doppler.tracks.iter_mut() {
                t.push(
                    row(
                        seq,
                        Ok(Line {
                            hz: -0.9,
                            sigma_hz: 0.004,
                        }),
                    ),
                    Instant::now(),
                    0,
                );
            }
            m.doppler.push_differential(DiffRow {
                seq,
                fi_hz: 10_000_000,
                line: Ok(Line {
                    hz: -0.3,
                    sigma_hz: 0.006,
                }),
                at: Instant::now(),
                at_unix_ms: 0,
            });
        }
        m
    }

    /// The rows say, in so many words, that our oscillator is in them and
    /// only the differential is free of it.
    #[test]
    fn the_rows_say_what_they_are_relative_to() {
        let v = doppler(&watching(1..3), clock(), None).readings;
        assert!(v.relative_to.contains("our oscillator") && v.relative_to.contains("differential"));
    }

    #[test]
    fn no_carrier_is_absent_with_how_to_choose_one() {
        let mut m = SdrMetrics::fixture().streaming();
        m.mode = crate::engine::mode::Mode::named("lab_doppler").unwrap();
        let g = doppler(&m, clock(), None);
        let why = g.testimony.absent.unwrap();
        assert!(
            why.contains("no carrier chosen") && why.contains("doppler command"),
            "{why}"
        );
        assert!(g.readings.carriers.is_empty());
    }

    #[test]
    fn since_returns_newer_rows_only() {
        let v = doppler(&watching(1..6), clock(), Some(3)).readings;
        let seqs: Vec<u64> = v.carriers[0].rows.iter().map(|r| r.seq).collect();
        assert_eq!(seqs, [4, 5]);
        assert_eq!(
            v.differential.iter().map(|d| d.seq).collect::<Vec<_>>(),
            [4, 5]
        );
        assert_eq!(v.missed, 0);
    }

    /// A client asking from before the history is told how many rows it
    /// missed, rather than handed a silent gap.
    #[test]
    fn a_since_older_than_the_history_says_what_it_missed() {
        let v = doppler(&watching(40..45), clock(), Some(10)).readings;
        assert_eq!(v.carriers[0].rows.first().unwrap().seq, 40);
        assert_eq!(v.missed, 29, "rows 11 to 39");
    }

    #[test]
    fn a_refused_line_is_absent_with_its_reason() {
        let mut m = watching(1..2);
        m.doppler.tracks[0].push(
            row(2, Err("no line above the noise".into())),
            Instant::now(),
            0,
        );
        let v = doppler(&m, clock(), Some(1)).readings;
        let r = &v.carriers[0].rows[0];
        assert_eq!(r.line.absent(), Some("no line above the noise"));
        assert_eq!(v.carriers[0].rows[0].bins_db.len(), 500);
    }

    /// Off its mode the rows are kept and served, and the group says it is
    /// not computing, so old rows are not read as live.
    #[test]
    fn off_its_mode_the_group_says_it_is_not_computing() {
        let mut m = watching(1..3);
        m.mode = crate::engine::mode::Mode::named("spectrum").unwrap();
        let g = doppler(&m, clock(), None);
        let why = g.testimony.absent.unwrap();
        assert!(
            why.contains("not computing") && why.contains("lab_doppler"),
            "{why}"
        );
        assert_eq!(g.readings.carriers[0].rows.len(), 2);
    }
}
