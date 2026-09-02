// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Projecting a completed sweep onto the plot's horizontal resolution.
//!
//! Two vectors come out of this, and the difference matters: `raw` keeps
//! `-inf` for a bucket the sweep never reached, which is what lets the cursor
//! readout print a dash instead of a fabricated level; `body` has those floored
//! into the window, because a canvas cannot draw negative infinity.

use crate::state::SweepFrame;

use super::scale::{Y_MAX, Y_MIN};

/// Horizontal buckets per character cell.
///
/// Two, because the canvas draws in braille and a single-bin peak projected at
/// one bucket per cell fills the whole cell - a narrow spike and a broad signal
/// would look the same. At two, a spike reads as a spike.
const BUCKETS_PER_CELL: usize = 2;

pub(super) struct Envelope {
    /// As projected: `-inf` where the sweep has no data.
    pub raw: Vec<f32>,
    /// Clamped into the dBFS window, ready to draw.
    pub body: Vec<f32>,
}

impl Envelope {
    pub(super) fn project(frame: &SweepFrame, plot_w: usize, show_peak: bool) -> Self {
        let n = (plot_w * BUCKETS_PER_CELL).max(2);
        let raw = frame.project(n, show_peak);
        let body = raw
            .iter()
            .map(|&v| {
                if v.is_finite() {
                    v.clamp(Y_MIN, Y_MAX)
                } else {
                    Y_MIN
                }
            })
            .collect();
        Self { raw, body }
    }

    pub(super) fn len(&self) -> usize {
        self.body.len()
    }

    /// The level in a bucket, or `None` where the sweep never went.
    pub(super) fn level_at(&self, bucket: usize) -> Option<f32> {
        self.raw.get(bucket).copied().filter(|v| v.is_finite())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn frame(peaks: &[f32]) -> SweepFrame {
        let n = peaks.len() as u64;
        SweepFrame {
            start_hz: 100_000_000,
            stop_hz: 100_000_000 + n * 1_000_000,
            freq_hz: (0..n).map(|i| 100_000_000 + i * 1_000_000).collect(),
            peak_dbfs: peaks.to_vec(),
            mean_dbfs: peaks.iter().map(|v| v - 10.0).collect(),
            timestamp: Instant::now(),
            cycle_count: 1,
            cycle_duration_ms: 100,
        }
    }

    #[test]
    fn the_plot_is_projected_at_two_buckets_per_cell() {
        let e = Envelope::project(&frame(&[-50.0; 8]), 20, true);
        assert_eq!(e.len(), 40);
    }

    /// A zero-width plot still yields a projection the canvas can index, rather
    /// than an empty one that panics on `len() - 1`.
    #[test]
    fn a_zero_width_plot_still_has_two_buckets() {
        assert_eq!(Envelope::project(&frame(&[-50.0]), 0, true).len(), 2);
    }

    /// An unvisited bucket must stay unknown in `raw` and be floored in `body`.
    /// Conflating the two is how a gap in a sweep would read as a real −100 dBFS
    /// measurement.
    #[test]
    fn a_gap_is_unknown_in_the_readout_and_floored_in_the_plot() {
        // Only two positions across a band wide enough for many buckets.
        let mut f = frame(&[-30.0, -40.0]);
        f.stop_hz = f.start_hz + 100_000_000;
        let e = Envelope::project(&f, 20, true);
        let empty = e
            .raw
            .iter()
            .position(|v| !v.is_finite())
            .expect("expected an unvisited bucket");
        assert_eq!(e.level_at(empty), None, "a gap must not report a level");
        assert_eq!(e.body[empty], Y_MIN, "but it must be drawable");
        assert!(e.body.iter().all(|v| v.is_finite()));
    }

    /// Levels above the window are clamped, not drawn off the top of the canvas.
    #[test]
    fn levels_outside_the_window_are_clamped_for_drawing() {
        let e = Envelope::project(&frame(&[20.0, -250.0]), 4, true);
        assert!(e.body.iter().all(|&v| (Y_MIN..=Y_MAX).contains(&v)));
        // …but `raw` keeps what was measured, so the readout is honest.
        assert!(e.raw.iter().any(|&v| v > Y_MAX));
    }

    #[test]
    fn peak_and_mean_select_different_curves() {
        let f = frame(&[-30.0, -30.0, -30.0, -30.0]);
        let peak = Envelope::project(&f, 4, true);
        let mean = Envelope::project(&f, 4, false);
        let hi = peak.raw.iter().cloned().fold(f32::MIN, f32::max);
        let lo = mean.raw.iter().cloned().fold(f32::MIN, f32::max);
        assert!(hi > lo, "peak {hi} should sit above mean {lo}");
    }
}
