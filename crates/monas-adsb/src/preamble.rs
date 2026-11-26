// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

use crate::ppm::{half_cell_samples, integrate, sample_power, PpmError, PREAMBLE_HALF_CELLS};

// ICAO Annex 10 Volume IV §3.1.2.2.5.1.1. Each half-cell is 0.5 µs.
const PULSE_CELLS: [usize; 4] = [0, 2, 7, 9];
const QUIET_CELLS: usize = PREAMBLE_HALF_CELLS - PULSE_CELLS.len();
const PHASES: usize = 8;

// Starting point for synthetic evaluation. Select the operating threshold
// from labeled captures before qualifying a hardware profile.
const MIN_CONTRAST: f64 = 0.2;

/// One timing hypothesis with detector-input evidence.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PreambleMatch {
    /// Slice-local preamble edge in the same coordinates as the PPM slicer.
    pub start_sample: f64,
    /// Mean power in each of the four nominal pulse windows.
    pub pulse_power: [f64; PULSE_CELLS.len()],
    /// Mean power in the other twelve half-cells.
    pub quiet_power: f64,
    /// (Mean pulse power − quiet power) / (mean pulse power + quiet power).
    pub contrast: f64,
}

/// A bounded set of alternative timings in one sample cell.
#[derive(Clone, Debug, Default)]
pub struct PreambleMatches {
    timings: [PreambleMatch; PHASES],
    len: usize,
}

impl PreambleMatches {
    /// Highest contrast first, with earliest edge first on an exact tie.
    pub fn as_slice(&self) -> &[PreambleMatch] {
        &self.timings[..self.len]
    }
}

/// A pulse pattern check for one whole-sample start cell at a time.
#[derive(Clone, Copy, Debug)]
pub struct PreambleDetector {
    half_cell_samples: f64,
    pulse_probes: [(usize, usize); PULSE_CELLS.len()],
    quiet_probes: [usize; 2],
    lookahead: usize,
    phases: [PhaseWindows; PHASES],
}

#[derive(Clone, Copy, Debug)]
struct PhaseWindows {
    start: f64,
    preamble: Window,
    pulses: [Window; PULSE_CELLS.len()],
}

#[derive(Clone, Copy, Debug)]
struct Window {
    start: Edge,
    end: Edge,
}

#[derive(Clone, Copy, Debug)]
struct Edge {
    cell: usize,
    fraction: f64,
}

impl Edge {
    fn new(sample: f64) -> Self {
        Self {
            cell: sample as usize,
            fraction: sample.fract(),
        }
    }

    fn cumulative_power(self, power: &[f32], prefix: &[f64]) -> f64 {
        let partial = power.get(self.cell).copied().unwrap_or(0.0);
        prefix[self.cell] + f64::from(partial) * self.fraction
    }

    fn sample(self) -> f64 {
        self.cell as f64 + self.fraction
    }
}

impl Window {
    fn energy(self, power: &[f32], prefix: &[f64]) -> Result<f64, PpmError> {
        let start = self.start.cumulative_power(power, prefix);
        let end = self.end.cumulative_power(power, prefix);
        let energy = end - start;
        // Preserve weak windows just after a much stronger sample.
        if start > 0.0 && energy <= end * (8.0 * f64::EPSILON) {
            integrate(power, self.start.sample(), self.end.sample())
        } else {
            Ok(energy)
        }
    }
}

impl PreambleDetector {
    /// Precompute the input-grid probes for the actual IQ pair rate.
    pub fn new(sample_rate_hz: f64) -> Result<Self, PpmError> {
        let half_cell_samples = half_cell_samples(sample_rate_hz)?;
        let pulse_probes = PULSE_CELLS.map(|cell| {
            let center = (cell as f64 + 0.5) * half_cell_samples;
            (center as usize, (center + 1.0) as usize)
        });
        // These taps lie inside the long gap after the final pulse at
        // all sub-sample offsets covered by the start cell.
        let quiet_probes = [11.0, 13.0].map(|cell| (cell * half_cell_samples) as usize);
        let last_phase = (PHASES - 1) as f64 / PHASES as f64;
        let lookahead = (PREAMBLE_HALF_CELLS as f64 * half_cell_samples + last_phase).ceil();
        let phases = std::array::from_fn(|index| {
            let start = index as f64 / PHASES as f64;
            let edge = |cell| Edge::new(start + cell as f64 * half_cell_samples);
            PhaseWindows {
                start,
                preamble: Window {
                    start: edge(0),
                    end: edge(PREAMBLE_HALF_CELLS),
                },
                pulses: PULSE_CELLS.map(|cell| Window {
                    start: edge(cell),
                    end: edge(cell + 1),
                }),
            }
        });
        Ok(Self {
            half_cell_samples,
            pulse_probes,
            quiet_probes,
            lookahead: lookahead as usize,
            phases,
        })
    }

    /// Number of samples required from the candidate's first cell onward.
    pub fn lookahead(&self) -> usize {
        self.lookahead
    }

    /// Search edges in [first_cell, first_cell+1) on an eighth-sample grid.
    ///
    /// A coarse four-pulse gate avoids integrating every phase at ordinary
    /// start positions. Each surviving phase must have a contrast above
    /// the provisional threshold and all four pulses above the quiet mean.
    pub fn candidates_at(
        &self,
        power: &[f32],
        first_cell: usize,
    ) -> Result<PreambleMatches, PpmError> {
        let available = power
            .get(first_cell..)
            .and_then(|tail| tail.get(..self.lookahead))
            .ok_or(PpmError::IncompletePreamble)?;
        let mut matches = self
            .candidates_at_start(available, &mut Vec::new())
            .map_err(|error| match error {
                PpmError::InvalidPower { index } => PpmError::InvalidPower {
                    index: first_cell + index,
                },
                other => other,
            })?
            .unwrap_or_default();
        for timing in &mut matches.timings[..matches.len] {
            timing.start_sample += first_cell as f64;
        }
        Ok(matches)
    }

    /// Fit an already bounded stream window and reuse its prefix scratch.
    pub(crate) fn candidates_at_start(
        &self,
        power: &[f32],
        prefix: &mut Vec<f64>,
    ) -> Result<Option<PreambleMatches>, PpmError> {
        let power = power
            .get(..self.lookahead)
            .ok_or(PpmError::IncompletePreamble)?;
        if !self.coarse_gate(power)? {
            return Ok(None);
        }
        prefix.clear();
        prefix.push(0.0);
        let mut sum = 0.0;
        for (index, &sample) in power.iter().enumerate() {
            sum += sample_power(sample, index)?;
            prefix.push(sum);
        }

        let mut matches = PreambleMatches::default();
        for windows in &self.phases {
            if let Some(timing) = self.score(power, prefix, windows)? {
                matches.timings[matches.len] = timing;
                matches.len += 1;
            }
        }
        if matches.len == 0 {
            return Ok(None);
        }
        matches.timings[..matches.len].sort_unstable_by(|a, b| {
            b.contrast
                .total_cmp(&a.contrast)
                .then_with(|| a.start_sample.total_cmp(&b.start_sample))
        });
        Ok(Some(matches))
    }

    fn coarse_gate(&self, power: &[f32]) -> Result<bool, PpmError> {
        let [first, second] = self.quiet_probes;
        let quiet =
            (sample_power(power[first], first)? + sample_power(power[second], second)?) * 0.5;
        for (first, last) in self.pulse_probes {
            let peak = sample_power(power[first], first)?.max(sample_power(power[last], last)?);
            if peak <= quiet {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn score(
        &self,
        power: &[f32],
        prefix: &[f64],
        windows: &PhaseWindows,
    ) -> Result<Option<PreambleMatch>, PpmError> {
        let all_energy = windows.preamble.energy(power, prefix)?;
        let mut pulse_energy = [0.0; PULSE_CELLS.len()];
        for (energy, window) in pulse_energy.iter_mut().zip(windows.pulses) {
            *energy = window.energy(power, prefix)?;
        }
        let on_energy: f64 = pulse_energy.iter().sum();
        let quiet_power =
            (all_energy - on_energy).max(0.0) / (QUIET_CELLS as f64 * self.half_cell_samples);
        let pulse_power = pulse_energy.map(|energy| energy / self.half_cell_samples);
        if pulse_power.iter().any(|&pulse| pulse <= quiet_power) {
            return Ok(None);
        }
        let mean_pulse = on_energy / (PULSE_CELLS.len() as f64 * self.half_cell_samples);
        let contrast = (mean_pulse - quiet_power) / (mean_pulse + quiet_power);
        Ok((contrast >= MIN_CONTRAST).then_some(PreambleMatch {
            start_sample: windows.start,
            pulse_power,
            quiet_power,
            contrast,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ppm::tests::{sampled_power, ARCHIVED_FRAME, PUBLISHED_FRAME};
    use crate::{Df17Frame, PpmSlicer};

    #[test]
    fn precomputed_windows_agree_with_direct_integration() {
        for rate in [2_400_000.0, 2_515_625.0, 6_000_000.0, 10_000_000.0] {
            let detector = PreambleDetector::new(rate).unwrap();
            let power: Vec<_> = (0..detector.lookahead())
                .map(|index| ((index * 7 % 13) as f32) / 13.0)
                .collect();
            let mut prefix = vec![0.0];
            for &sample in &power {
                prefix.push(prefix.last().unwrap() + f64::from(sample));
            }
            for phase in detector.phases {
                for window in std::iter::once(phase.preamble).chain(phase.pulses) {
                    let direct =
                        integrate(&power, window.start.sample(), window.end.sample()).unwrap();
                    let cached = window.energy(&power, &prefix).unwrap();
                    assert!((direct - cached).abs() < 1e-12, "{rate} Hz, {window:?}");
                }
            }
        }
    }

    #[test]
    fn weak_windows_survive_after_a_large_sample() {
        let power = [f32::MAX, 1.0, 3.0];
        let strong = f64::from(power[0]);
        let prefix = [0.0, strong, strong, strong];
        let window = Window {
            start: Edge::new(1.2),
            end: Edge::new(2.4),
        };
        assert!((window.energy(&power, &prefix).unwrap() - 2.0).abs() < 1e-12);
    }

    #[test]
    fn finds_a_crc_valid_timing_in_the_start_cell_across_rates_and_phases() {
        for rate in [2_400_000.0, 2_500_000.0, 6_000_000.0, 10_000_000.0] {
            let detector = PreambleDetector::new(rate).unwrap();
            let slicer = PpmSlicer::new(rate).unwrap();
            for expected in [ARCHIVED_FRAME, PUBLISHED_FRAME] {
                for phase in 0..64 {
                    let start = 5.0 + f64::from(phase) / 64.0;
                    for subsamples in [1, 16] {
                        let mut power = sampled_power(&expected, rate, start, subsamples);
                        power.push(0.0);
                        let timings = detector.candidates_at(&power, 5).unwrap();
                        let candidates = timings.as_slice();
                        assert!(candidates.windows(2).all(|pair| {
                            pair[0].contrast > pair[1].contrast
                                || (pair[0].contrast == pair[1].contrast
                                    && pair[0].start_sample < pair[1].start_sample)
                        }));
                        let mut valid = 0;
                        for timing in candidates {
                            let bytes = slicer
                                .slice_frame(&power, timing.start_sample, None)
                                .unwrap();
                            if Df17Frame::try_from(bytes.as_slice()).is_ok() {
                                assert_eq!(bytes, expected);
                                valid += 1;
                            }
                        }
                        assert!(valid > 0, "{rate} Hz, phase {phase}, {subsamples}x");
                    }
                }
            }
        }
    }

    #[test]
    fn measures_the_four_pulses_and_quiet_on_the_aligned_grid() {
        let rate = 10_000_000.0;
        let detector = PreambleDetector::new(rate).unwrap();
        let mut power = sampled_power(&ARCHIVED_FRAME, rate, 6.0, 1);
        for sample in &mut power {
            *sample = *sample * 0.75 + 0.25;
        }
        let matches = detector.candidates_at(&power, 6).unwrap();
        let strongest = matches.as_slice()[0];
        assert_eq!(strongest.start_sample, 6.0);
        assert_eq!(strongest.pulse_power, [1.0; 4]);
        assert_eq!(strongest.quiet_power, 0.25);
        assert!((strongest.contrast - 0.6).abs() < 1e-12);
    }

    #[test]
    fn rejects_flat_power_an_impulse_and_a_missing_pulse() {
        let rate = 10_000_000.0;
        let detector = PreambleDetector::new(rate).unwrap();
        let count = detector.lookahead();
        for level in [0.0, 1.0] {
            let flat = vec![level; count];
            assert!(detector
                .candidates_at(&flat, 0)
                .unwrap()
                .as_slice()
                .is_empty());
        }
        let mut impulse = vec![0.0; count];
        impulse[2] = 1.0;
        assert!(detector
            .candidates_at(&impulse, 0)
            .unwrap()
            .as_slice()
            .is_empty());

        let mut three_pulses = sampled_power(&ARCHIVED_FRAME, rate, 0.0, 1);
        three_pulses[45..50].fill(0.0);
        assert!(detector
            .candidates_at(&three_pulses, 0)
            .unwrap()
            .as_slice()
            .is_empty());
    }

    #[test]
    fn checks_bounds_and_returns_slice_relative_sample_errors() {
        assert!(matches!(
            PreambleDetector::new(f64::NAN),
            Err(PpmError::InvalidSampleRate)
        ));
        let detector = PreambleDetector::new(10_000_000.0).unwrap();
        assert_eq!(detector.lookahead(), 81);
        assert!(detector.candidates_at(&[0.0; 83], 2).is_ok());
        assert!(matches!(
            detector.candidates_at(&[0.0; 82], 2),
            Err(PpmError::IncompletePreamble)
        ));
        assert!(matches!(
            detector.candidates_at(&[], usize::MAX),
            Err(PpmError::IncompletePreamble)
        ));

        let mut power = [0.0; 83];
        power[2 + 55] = f32::NAN;
        assert!(matches!(
            detector.candidates_at(&power, 2),
            Err(PpmError::InvalidPower { index: 57 })
        ));
    }
}
