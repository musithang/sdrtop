// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The floor lab's long exposure, as the worker last published it.
//!
//! **Products, not sums.** The two sums per bin (12 MB at 768 kS/s and 1 Hz)
//! stay in the worker. What is kept here is bounded whatever the band: at
//! most [`MAX_CELLS`] overview cells, the lines the finder caps, their grids,
//! and a zoom of at most [`MAX_ZOOM_BINS`] bins, so a clone of the state
//! stays cheap.

use std::time::Instant;

use crate::signal::floor::grids::Grid;
use crate::signal::floor::lines::Line;
use crate::signal::floor::spread::Steadiness;
use crate::signal::floor::FloorSetting;
use crate::state::PagerFeed;

/// Cells the band is summarised in, at most.
pub const MAX_CELLS: usize = 4096;

pub use crate::signal::floor::settings::MAX_ZOOM_BINS;

/// What no zoom says until one is chosen.
pub const NO_ZOOM: &str = "no zoom chosen: send the floor command with a span";

/// A stretch of the band in the overview.
#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    /// Its edges, Hz, absolute.
    pub from_hz: f64,
    pub to_hz: f64,
    /// The least, the median and the most of its bins' excess over the
    /// floor, dB.
    pub min_db: f32,
    pub median_db: f32,
    pub max_db: f32,
    /// Its bins that behave like chance, like clicks, and like a steady line.
    pub chance: u32,
    pub impulsive: u32,
    pub steady: u32,
}

/// A line, with what its own spread says of it.
#[derive(Clone, Debug, PartialEq)]
pub struct FloorLine {
    pub line: Line,
    /// Its peak bin's spread.
    pub spread: f64,
    /// That spread's standard error, the larger of a steady line's and a
    /// fading one's at its strength.
    pub spread_sigma: f64,
    pub steadiness: Result<Steadiness, String>,
}

/// The band at full resolution over the span the client chose.
#[derive(Clone, Debug, PartialEq)]
pub struct Zoom {
    /// Its first bin's centre, Hz, absolute.
    pub from_hz: f64,
    pub bin_hz: f64,
    /// Each bin's excess over the floor, dB.
    pub excess_db: Vec<f32>,
    /// Each bin's spread.
    pub spread: Vec<f32>,
}

/// The floor lab's state.
#[derive(Clone, Debug)]
pub struct FloorState {
    /// What was asked: the bin width and the zoom.
    pub setting: FloorSetting,
    /// The frames summed, as seconds of signal.
    pub exposure_s: f64,
    pub frames: u64,
    /// Frames a gap cut short and threw away, this exposure.
    pub dropped: u64,
    /// Exposures started again, and why the last one was.
    pub restarts: u64,
    pub last_restart: Option<String>,
    /// How far down the exposure sees: `10·log10(1 + 1/√frames)`.
    pub depth_db: f64,
    /// The bin width delivered.
    pub bin_hz: f64,
    /// The band examined, Hz, absolute.
    pub span_hz: (f64, f64),
    pub overview: Vec<Cell>,
    /// Strongest first.
    pub lines: Vec<FloorLine>,
    pub lines_left_out: usize,
    /// Their members index `lines`.
    pub grids: Vec<Grid>,
    pub zoom: Result<Zoom, String>,
    /// Why nothing is exposed at this tuning, when nothing is.
    pub refused: Option<String>,
    /// Raised by the `floor_reset` command; the worker starts again when it
    /// differs from the one it last saw.
    pub reset_generation: u64,
    pub feed: PagerFeed,
    /// When the worker last published.
    pub updated: Option<Instant>,
}

impl Default for FloorState {
    fn default() -> Self {
        FloorState {
            setting: FloorSetting::default(),
            exposure_s: 0.0,
            frames: 0,
            dropped: 0,
            restarts: 0,
            last_restart: None,
            depth_db: 0.0,
            bin_hz: 0.0,
            span_hz: (0.0, 0.0),
            overview: Vec::new(),
            lines: Vec::new(),
            lines_left_out: 0,
            grids: Vec::new(),
            zoom: Err(NO_ZOOM.to_string()),
            refused: None,
            reset_generation: 0,
            feed: PagerFeed::default(),
            updated: None,
        }
    }
}

impl FloorState {
    /// Forget the products, as at a restart or a refusal; the setting, the
    /// counts of restarts and the feed stay.
    pub fn clear_products(&mut self) {
        self.exposure_s = 0.0;
        self.frames = 0;
        self.dropped = 0;
        self.depth_db = 0.0;
        self.overview.clear();
        self.lines.clear();
        self.lines_left_out = 0;
        self.grids.clear();
        self.zoom = Err(NO_ZOOM.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::size_of;

    /// The heap a state holds, counted by hand.
    fn heap(s: &FloorState) -> usize {
        let lines: usize = s
            .lines
            .iter()
            .map(|l| l.steadiness.as_ref().err().map_or(0, |e| e.capacity()))
            .sum();
        let grids: usize = s
            .grids
            .iter()
            .map(|g| g.members.capacity() * size_of::<usize>())
            .sum();
        let zoom = s.zoom.as_ref().map_or(0, |z| {
            (z.excess_db.capacity() + z.spread.capacity()) * size_of::<f32>()
        });
        s.overview.capacity() * size_of::<Cell>()
            + s.lines.capacity() * size_of::<FloorLine>()
            + lines
            + s.grids.capacity() * size_of::<Grid>()
            + grids
            + zoom
    }

    /// Full to every bound, a state is still well under a megabyte.
    #[test]
    fn a_clone_of_the_state_stays_small() {
        let line = Line {
            hz: 9_980_000.0,
            hz_sigma: 0.1,
            excess_db: 3.0,
            excess_sigma_db: 0.17,
            width_bins: 3,
            first_seen_s: 12.0,
            peak_bin: 100,
        };
        let cell = Cell {
            from_hz: 0.0,
            to_hz: 37.0,
            min_db: -0.2,
            median_db: 0.0,
            max_db: 0.3,
            chance: 30,
            impulsive: 3,
            steady: 4,
        };
        let s = FloorState {
            overview: vec![cell; MAX_CELLS],
            lines: vec![
                FloorLine {
                    line,
                    spread: 0.9,
                    spread_sigma: 0.04,
                    steadiness: Err(
                        "cannot tell at this strength and exposure: needs about 12 000 s".into()
                    ),
                };
                crate::signal::floor::lines::MAX_LINES
            ],
            grids: vec![
                Grid {
                    spacing_hz: 1000.0,
                    spacing_sigma_hz: 0.01,
                    offset_hz: 3.0,
                    members: (0..150).collect(),
                    gaps: 2,
                    span_hz: (0.0, 1.0),
                    chance: 1e-9,
                };
                20
            ],
            zoom: Ok(Zoom {
                from_hz: 0.0,
                bin_hz: 1.0,
                excess_db: vec![0.0; MAX_ZOOM_BINS],
                spread: vec![1.0; MAX_ZOOM_BINS],
            }),
            ..FloorState::default()
        };
        let bytes = heap(&s.clone());
        assert!(bytes < 1 << 20, "{bytes} bytes");
    }
}
