// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The floor lab's readings: the exposure, its depth, the lines chance does
//! not explain and the grids they fall on, and what each line's power does
//! from frame to frame.
//!
//! Every level here is an excess over a floor the worker estimated from the
//! band itself, and the group says so in [`RELATIVE_TO`], on every answer:
//! a line at +3 dB is twice the floor around it, not a power at the antenna.

use serde::Serialize;

use super::pager::Feed;
use super::{Clock, Group, Reading};
use crate::engine::testimony::{Staleness, Testimony};
use crate::signal::floor::spread::Steadiness;
use crate::state::floor::{Cell, FloorLine};
use crate::state::SdrMetrics;

/// What every level in this group is relative to.
pub const RELATIVE_TO: &str = "the local floor, estimated per kilohertz from the quieter 80 % of \
its bins; levels are not absolute";

/// A frequency span, Hz, absolute.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Span {
    pub from_hz: f64,
    pub to_hz: f64,
}

impl Span {
    fn of((from_hz, to_hz): (f64, f64)) -> Span {
        Span { from_hz, to_hz }
    }
}

/// A stretch of the band, summarised.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CellView {
    pub from_hz: f64,
    pub to_hz: f64,
    /// The least, the median and the most of its bins' excess, dB.
    pub min_db: f32,
    pub median_db: f32,
    pub max_db: f32,
    /// Its bins that behave like chance, like clicks, and like a steady line.
    pub chance: u32,
    pub impulsive: u32,
    pub steady: u32,
}

impl CellView {
    fn of(c: &Cell) -> CellView {
        CellView {
            from_hz: c.from_hz,
            to_hz: c.to_hz,
            min_db: c.min_db,
            median_db: c.median_db,
            max_db: c.max_db,
            chance: c.chance,
            impulsive: c.impulsive,
            steady: c.steady,
        }
    }
}

/// A line chance does not explain.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LineView {
    /// Its frequency, Hz, absolute, and that frequency's standard
    /// uncertainty.
    pub hz: f64,
    pub hz_sigma: f64,
    /// How far it stands over the floor, dB, and that level's uncertainty.
    pub excess_db: f64,
    pub excess_sigma_db: f64,
    /// Bins it spans.
    pub width_bins: usize,
    /// The exposure at which it first stood out, seconds.
    pub first_seen_s: f64,
    /// Its peak bin's spread (one for chance), and that spread's error.
    pub spread: f64,
    pub spread_sigma: f64,
    /// `steady` or `fading`, or why that cannot be told yet.
    pub steadiness: Reading<&'static str>,
    /// The grids it is a member of, as indices into `grids`.
    pub grids: Vec<usize>,
}

impl LineView {
    fn of(l: &FloorLine, grids: Vec<usize>) -> LineView {
        LineView {
            hz: l.line.hz,
            hz_sigma: l.line.hz_sigma,
            excess_db: l.line.excess_db,
            excess_sigma_db: l.line.excess_sigma_db,
            width_bins: l.line.width_bins,
            first_seen_s: l.line.first_seen_s,
            spread: l.spread,
            spread_sigma: l.spread_sigma,
            steadiness: match &l.steadiness {
                Ok(Steadiness::Steady) => Reading::Value("steady"),
                Ok(Steadiness::Fading) => Reading::Value("fading"),
                Err(why) => Reading::Absent(why.clone()),
            },
            grids,
        }
    }
}

/// Lines on one spacing, more of them than chance puts there.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GridView {
    /// The spacing, Hz, and its standard uncertainty.
    pub spacing_hz: f64,
    pub spacing_sigma_hz: f64,
    /// Where the teeth sit: every `offset_hz + k · spacing_hz`.
    pub offset_hz: f64,
    /// Its members' frequencies, lowest first.
    pub members_hz: Vec<f64>,
    /// Positions inside its span with no line.
    pub gaps: usize,
    pub span: Span,
    /// The chance of so many members by accident, every grid tried counted.
    pub chance: f64,
}

/// The band at full resolution over the chosen span.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ZoomView {
    /// Its first bin's centre, Hz, absolute.
    pub from_hz: f64,
    pub bin_hz: f64,
    /// Each bin's excess over the floor, dB.
    pub excess_db: Vec<f32>,
    /// Each bin's spread.
    pub spread: Vec<f32>,
}

/// The floor lab's readings, for their route.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Floor {
    /// What every level here is relative to.
    pub relative_to: &'static str,
    /// The frames summed, as seconds of signal.
    pub exposure_s: f64,
    pub frames: u64,
    /// Frames a gap cut short and threw away, this exposure.
    pub dropped: u64,
    /// Exposures started again, and why the last one was.
    pub restarts: u64,
    pub last_restart: Option<String>,
    /// How far down the exposure sees, dB: `10·log10(1 + 1/√frames)`.
    pub depth_db: f64,
    /// The bin width delivered, Hz.
    pub bin_hz: f64,
    /// The band examined.
    pub span: Span,
    /// The band in at most 4096 cells.
    pub overview: Vec<CellView>,
    /// Strongest first.
    pub lines: Vec<LineView>,
    /// Lines past the cap, weaker than every one listed.
    pub lines_left_out: usize,
    pub grids: Vec<GridView>,
    pub zoom: Reading<ZoomView>,
    pub feed: Feed,
}

/// The floor lab's readings, with their testimony.
pub fn floor(m: &SdrMetrics, clock: Clock) -> Group<Floor> {
    let f = &m.floor;
    let mut memberships: Vec<Vec<usize>> = vec![Vec::new(); f.lines.len()];
    let grids = f
        .grids
        .iter()
        .enumerate()
        .map(|(g, grid)| {
            let mut members_hz: Vec<f64> = grid
                .members
                .iter()
                .filter_map(|&i| {
                    memberships.get_mut(i)?.push(g);
                    Some(f.lines[i].line.hz)
                })
                .collect();
            members_hz.sort_by(f64::total_cmp);
            GridView {
                spacing_hz: grid.spacing_hz,
                spacing_sigma_hz: grid.spacing_sigma_hz,
                offset_hz: grid.offset_hz,
                members_hz,
                gaps: grid.gaps,
                span: Span::of(grid.span_hz),
                chance: grid.chance,
            }
        })
        .collect();
    let lines = f
        .lines
        .iter()
        .zip(memberships)
        .map(|(l, g)| LineView::of(l, g))
        .collect();
    let zoom = match &f.zoom {
        Ok(z) => Reading::Value(ZoomView {
            from_hz: z.from_hz,
            bin_hz: z.bin_hz,
            excess_db: z.excess_db.clone(),
            spread: z.spread.clone(),
        }),
        Err(why) => Reading::Absent(why.clone()),
    };

    let mut testimony = Testimony::resolve(
        m,
        Staleness::NotStreaming,
        None,
        f.updated.map(|t| clock.unix_ms_of(t)),
    );
    testimony.absent = if !m.mode.feeds.floor {
        Some(format!(
            "not computing: the mode is {}; this exposure is as it was when it left lab_floor",
            if m.mode.view.is_empty() {
                "spectrum"
            } else {
                &m.mode.view
            }
        ))
    } else if let Some(why) = &f.refused {
        Some(why.clone())
    } else if f.frames == 0 {
        Some(match &f.last_restart {
            Some(why) => format!("exposure restarted: {why}"),
            None => "exposing: the first frame is not in yet".to_string(),
        })
    } else {
        None
    };
    Group {
        testimony,
        readings: Floor {
            relative_to: RELATIVE_TO,
            exposure_s: f.exposure_s,
            frames: f.frames,
            dropped: f.dropped,
            restarts: f.restarts,
            last_restart: f.last_restart.clone(),
            depth_db: f.depth_db,
            bin_hz: f.bin_hz,
            span: Span::of(f.span_hz),
            overview: f.overview.iter().map(CellView::of).collect(),
            lines,
            lines_left_out: f.lines_left_out,
            grids,
            zoom,
            feed: Feed::of(&f.feed),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::floor::grids::Grid;
    use crate::signal::floor::lines::Line;
    use std::time::Instant;

    fn clock() -> Clock {
        Clock {
            now: Instant::now(),
            unix_ms: 1_795_000_000_000,
        }
    }

    fn line(hz: f64, steadiness: Result<Steadiness, String>) -> FloorLine {
        FloorLine {
            line: Line {
                hz,
                hz_sigma: 0.05,
                excess_db: 6.0,
                excess_sigma_db: 0.17,
                width_bins: 3,
                first_seen_s: 40.0,
                peak_bin: 0,
            },
            spread: 0.6,
            spread_sigma: 0.04,
            steadiness,
        }
    }

    /// An exposure of 600 frames on 31 m, three lines and one grid.
    fn exposing() -> SdrMetrics {
        let mut m = SdrMetrics::fixture().streaming();
        m.mode = crate::engine::mode::Mode::named("lab_floor").unwrap();
        let f = &mut m.floor;
        f.frames = 600;
        f.exposure_s = 600.0;
        f.depth_db = crate::signal::floor::chance::depth_db(600);
        f.bin_hz = 1.0;
        f.span_hz = (9_903_200.0, 10_056_800.0);
        f.overview = vec![Cell {
            from_hz: 9_903_200.0,
            to_hz: 9_903_238.0,
            min_db: -0.3,
            median_db: 0.0,
            max_db: 0.4,
            chance: 37,
            impulsive: 1,
            steady: 0,
        }];
        f.lines = vec![
            line(9_982_000.0, Ok(Steadiness::Steady)),
            line(9_980_000.0, Ok(Steadiness::Fading)),
            line(
                9_981_000.0,
                Err("cannot tell at this strength and exposure: needs about 2 400 s".into()),
            ),
        ];
        f.grids = vec![Grid {
            spacing_hz: 1000.0,
            spacing_sigma_hz: 0.01,
            offset_hz: 0.0,
            members: vec![0, 1, 2],
            gaps: 0,
            span_hz: (9_980_000.0, 9_982_000.0),
            chance: 1e-6,
        }];
        f.updated = Some(Instant::now());
        m
    }

    #[test]
    fn the_answer_says_what_its_levels_are_relative_to() {
        let g = floor(&exposing(), clock());
        assert!(g.readings.relative_to.contains("not absolute"));
        assert!(g.readings.relative_to.contains("quieter 80 %"));
        assert_eq!(g.testimony.absent, None);
        assert_eq!(g.readings.frames, 600);
        assert_eq!(g.readings.overview.len(), 1);
        assert!(g.testimony.at_unix_ms.is_some());
    }

    #[test]
    fn off_its_mode_it_says_it_is_not_computing() {
        let mut m = exposing();
        m.mode = crate::engine::mode::Mode::named("lab_doppler").unwrap();
        let g = floor(&m, clock());
        let why = g.testimony.absent.unwrap();
        assert!(
            why.contains("not computing") && why.contains("lab_doppler"),
            "{why}"
        );
        assert_eq!(g.readings.lines.len(), 3, "the products are kept");
    }

    #[test]
    fn just_after_a_restart_it_says_why() {
        let mut m = exposing();
        m.floor.clear_products();
        m.floor.restarts = 1;
        m.floor.last_restart = Some("retuned".into());
        let g = floor(&m, clock());
        assert_eq!(
            g.testimony.absent.as_deref(),
            Some("exposure restarted: retuned")
        );
        // Once the first frame is in, the answer stands again.
        m.floor.frames = 1;
        assert_eq!(floor(&m, clock()).testimony.absent, None);
    }

    #[test]
    fn a_line_says_steady_fading_or_why_not() {
        let g = floor(&exposing(), clock());
        let s: Vec<&Reading<&str>> = g.readings.lines.iter().map(|l| &l.steadiness).collect();
        assert_eq!(s[0], &Reading::Value("steady"));
        assert_eq!(s[1], &Reading::Value("fading"));
        let Reading::Absent(why) = s[2] else {
            panic!("{:?}", s[2])
        };
        assert!(why.contains("needs about"), "{why}");
        let json = serde_json::to_value(&g.readings.lines[0]).unwrap();
        assert_eq!(json["steadiness"]["value"], "steady", "{json}");
    }

    #[test]
    fn a_grid_lists_its_members_by_frequency() {
        let g = floor(&exposing(), clock());
        assert_eq!(
            g.readings.grids[0].members_hz,
            vec![9_980_000.0, 9_981_000.0, 9_982_000.0]
        );
        assert!(g.readings.lines.iter().all(|l| l.grids == vec![0]));
    }
}
