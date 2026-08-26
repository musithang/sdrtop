//! `SweepState` — the frequency-scanner mode (`lab_sweep`, `[9]`).
//!
//! A sweep maps a band wider than one sample-rate window by retuning the radio
//! across a series of positions, harvesting the FFT at each, and stitching the
//! results into one curve (frequency on the x-axis, dBFS on the y-axis). The
//! `sweep_task` drives the radio; this module holds the data model and the pure
//! math (position layout, curve projection) so it stays testable without hardware.

use std::sync::Arc;
use std::time::Instant;

/// Step = sample_rate × this, leaving a 10 % overlap at the window edges where
/// the FFT window function attenuates.
pub const SWEEP_OVERLAP_FACTOR: f64 = 0.9;
/// Samples discarded after a retune while the PLL settles (ms).
pub const SWEEP_SETTLING_MS: u64 = 25;
pub const DEFAULT_SWEEP_DWELL_MS: u64 = 200;

#[derive(Clone, Debug, PartialEq)]
pub struct SweepConfig {
    pub start_hz: u64,
    pub stop_hz: u64,
    /// Explicit step in Hz, or 0 to derive it from the sample rate.
    pub step_hz: u64,
    pub dwell_ms: u64,
}

impl Default for SweepConfig {
    fn default() -> Self {
        Self {
            start_hz: 400_000_000,
            stop_hz: 500_000_000,
            step_hz: 0,
            dwell_ms: DEFAULT_SWEEP_DWELL_MS,
        }
    }
}

impl SweepConfig {
    /// Effective step: the configured value if set, otherwise sample_rate × overlap.
    pub fn effective_step_hz(&self, sample_rate: f64) -> u64 {
        if self.step_hz > 0 {
            self.step_hz
        } else {
            ((sample_rate * SWEEP_OVERLAP_FACTOR) as u64).max(1)
        }
    }

    /// Number of retune positions across `[start_hz, stop_hz]`.
    pub fn positions_total(&self, sample_rate: f64) -> usize {
        if self.stop_hz <= self.start_hz {
            return 0;
        }
        let span = self.stop_hz - self.start_hz;
        let step = self.effective_step_hz(sample_rate);
        (span / step) as usize + 1
    }

    /// Center frequency of position `i`.
    pub fn position_hz(&self, i: usize, sample_rate: f64) -> u64 {
        self.start_hz + self.effective_step_hz(sample_rate) * i as u64
    }
}

/// One completed sweep cycle: a stitched curve of bin center frequencies and the
/// peak / mean dBFS measured at each, ascending in frequency.
pub struct SweepFrame {
    pub start_hz: u64,
    pub stop_hz: u64,
    pub freq_hz: Vec<u64>,
    pub peak_dbfs: Vec<f32>,
    pub mean_dbfs: Vec<f32>,
    pub timestamp: Instant,
    pub cycle_count: u64,
    pub cycle_duration_ms: u64,
}

impl SweepFrame {
    /// Project the stitched curve onto `width` horizontal buckets: each bucket
    /// holds the maximum dBFS of the bins that fall in it (peak or mean per
    /// `peak`). Empty buckets read `f32::NEG_INFINITY`.
    pub fn project(&self, width: usize, peak: bool) -> Vec<f32> {
        let mut out = vec![f32::NEG_INFINITY; width];
        if width == 0 || self.freq_hz.is_empty() || self.stop_hz <= self.start_hz {
            return out;
        }
        let span = (self.stop_hz - self.start_hz) as f64;
        let vals = if peak {
            &self.peak_dbfs
        } else {
            &self.mean_dbfs
        };
        for (i, &f) in self.freq_hz.iter().enumerate() {
            if f < self.start_hz || f >= self.stop_hz {
                continue;
            }
            let frac = (f - self.start_hz) as f64 / span;
            let bucket = ((frac * width as f64) as usize).min(width - 1);
            let v = vals.get(i).copied().unwrap_or(f32::NEG_INFINITY);
            if v > out[bucket] {
                out[bucket] = v;
            }
        }
        out
    }

    /// The `n` strongest distinct peaks as `(freq_hz, dbfs)` from the peak curve,
    /// keeping selected peaks at least `min_spacing_hz` apart so one broad signal
    /// doesn't fill the list. Strongest first. Powers the micro_sweep field view.
    pub fn top_peaks(&self, n: usize, min_spacing_hz: u64) -> Vec<(u64, f32)> {
        let len = self.freq_hz.len().min(self.peak_dbfs.len());
        let mut idx: Vec<usize> = (0..len)
            .filter(|&i| self.peak_dbfs[i].is_finite())
            .collect();
        idx.sort_by(|&a, &b| {
            self.peak_dbfs[b]
                .partial_cmp(&self.peak_dbfs[a])
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut out: Vec<(u64, f32)> = Vec::new();
        for i in idx {
            let f = self.freq_hz[i];
            if out.iter().any(|(of, _)| f.abs_diff(*of) < min_spacing_hz) {
                continue;
            }
            out.push((f, self.peak_dbfs[i]));
            if out.len() >= n {
                break;
            }
        }
        out
    }

    /// Frequency at a horizontal fraction (0.0 = start, 1.0 = stop).
    pub fn freq_at_fraction(&self, frac: f64) -> u64 {
        let frac = frac.clamp(0.0, 1.0);
        self.start_hz + ((self.stop_hz - self.start_hz) as f64 * frac) as u64
    }
}

#[derive(Clone)]
pub struct SweepState {
    pub config: SweepConfig,
    /// True while the `lab_sweep` preset is active — drives the `sweep_task`.
    pub active: bool,
    /// Position the radio is currently parked on.
    pub current_hz: u64,
    /// Last completed sweep cycle, shared with the renderer.
    pub current_frame: Option<Arc<SweepFrame>>,
    pub positions_total: usize,
    pub positions_done: usize,
    pub cycle_count: u64,
    pub cycle_duration_ms: u64,
    /// Render the peak curve (`true`) or the mean curve (`false`); toggled by `[M]`.
    pub show_peak: bool,
    /// Cursor position as a 0..1 fraction across the band, set in the panel's
    /// focus mode (`None` = no cursor).
    pub cursor_frac: Option<f64>,
    /// Set by the `[Enter]` jump: the frequency to return to when sweep stops, so
    /// the radio lands on the cursor instead of the pre-sweep frequency.
    pub pending_tune: Option<u64>,
}

impl Default for SweepState {
    fn default() -> Self {
        Self {
            config: SweepConfig::default(),
            active: false,
            current_hz: 0,
            current_frame: None,
            positions_total: 0,
            positions_done: 0,
            cycle_count: 0,
            cycle_duration_ms: 0,
            show_peak: true,
            cursor_frac: None,
            pending_tune: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_step_uses_sample_rate_when_unset() {
        let c = SweepConfig {
            step_hz: 0,
            ..Default::default()
        };
        // 10 Msps × 0.9 = 9 MHz.
        assert_eq!(c.effective_step_hz(10_000_000.0), 9_000_000);
        // Explicit step wins.
        let c2 = SweepConfig {
            step_hz: 5_000_000,
            ..Default::default()
        };
        assert_eq!(c2.effective_step_hz(10_000_000.0), 5_000_000);
    }

    #[test]
    fn positions_total_matches_design_table() {
        // 400–500 MHz at 9 MHz step → 12 positions (matches the design doc).
        let c = SweepConfig {
            start_hz: 400_000_000,
            stop_hz: 500_000_000,
            step_hz: 0,
            dwell_ms: 200,
        };
        assert_eq!(c.positions_total(10_000_000.0), 12);
        // 20 Msps → 18 MHz step → 6 positions.
        assert_eq!(c.positions_total(20_000_000.0), 6);
        // Degenerate range → no positions.
        let bad = SweepConfig {
            start_hz: 500_000_000,
            stop_hz: 400_000_000,
            ..Default::default()
        };
        assert_eq!(bad.positions_total(10_000_000.0), 0);
    }

    #[test]
    fn position_hz_steps_from_start() {
        let c = SweepConfig {
            start_hz: 400_000_000,
            stop_hz: 500_000_000,
            step_hz: 0,
            dwell_ms: 200,
        };
        assert_eq!(c.position_hz(0, 10_000_000.0), 400_000_000);
        assert_eq!(c.position_hz(2, 10_000_000.0), 418_000_000);
    }

    fn frame() -> SweepFrame {
        SweepFrame {
            start_hz: 400_000_000,
            stop_hz: 500_000_000,
            freq_hz: vec![400_000_000, 450_000_000, 499_000_000],
            peak_dbfs: vec![-80.0, -20.0, -60.0],
            mean_dbfs: vec![-90.0, -40.0, -70.0],
            timestamp: Instant::now(),
            cycle_count: 1,
            cycle_duration_ms: 2400,
        }
    }

    #[test]
    fn project_buckets_take_the_max() {
        let f = frame();
        let curve = f.project(10, true);
        assert_eq!(curve.len(), 10);
        // 400 MHz → bucket 0; 450 MHz → bucket 5; 499 MHz → bucket 9.
        assert_eq!(curve[0], -80.0);
        assert_eq!(curve[5], -20.0);
        assert_eq!(curve[9], -60.0);
        // An untouched bucket stays at -inf.
        assert_eq!(curve[3], f32::NEG_INFINITY);
        // Mean curve reads the other array.
        assert_eq!(f.project(10, false)[5], -40.0);
    }

    #[test]
    fn top_peaks_picks_strongest_and_spaces_them() {
        let f = SweepFrame {
            start_hz: 400_000_000,
            stop_hz: 500_000_000,
            freq_hz: vec![410_000_000, 410_500_000, 450_000_000, 480_000_000],
            peak_dbfs: vec![-30.0, -28.0, -50.0, -70.0],
            mean_dbfs: vec![-40.0, -38.0, -60.0, -80.0],
            timestamp: Instant::now(),
            cycle_count: 1,
            cycle_duration_ms: 2400,
        };
        // 1 MHz spacing collapses the two ~410 MHz bins into the stronger one.
        let peaks = f.top_peaks(3, 1_000_000);
        assert_eq!(peaks.len(), 3);
        assert_eq!(peaks[0].0, 410_500_000); // -28 is strongest
        assert!((peaks[0].1 - (-28.0)).abs() < 1e-6);
        assert_eq!(peaks[1].0, 450_000_000);
        assert_eq!(peaks[2].0, 480_000_000);
    }

    #[test]
    fn freq_at_fraction_spans_the_range() {
        let f = frame();
        assert_eq!(f.freq_at_fraction(0.0), 400_000_000);
        assert_eq!(f.freq_at_fraction(0.5), 450_000_000);
        assert_eq!(f.freq_at_fraction(1.0), 500_000_000);
        // Clamps out-of-range fractions.
        assert_eq!(f.freq_at_fraction(-1.0), 400_000_000);
    }
}
