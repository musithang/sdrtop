// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The bins chance cannot explain, grouped into lines.
//!
//! A bin is over when its mean, over the floor under it, passes the level
//! that chance passes about once in ten exposures over the whole band.
//! Neighbouring bins over it (a gap of one bin allowed, since a window
//! spreads one tone over two or three) are one line: its frequency the
//! centroid of their excess, its level the peak's.
//!
//! **A strong line's skirt is not a crowd of lines.** A window leaks every
//! tone into its neighbours through sidelobes, and at a long exposure the
//! leakage around a strong carrier stands clear of chance. A weaker group
//! inside the Hann sidelobe envelope of a stronger line nearby is that
//! line's skirt, and is folded into it.

use super::chance::threshold;
use super::settings::Derived;

/// The most lines reported, strongest first; the rest are counted.
pub const MAX_LINES: usize = 1000;

/// How far a strong line's skirt is looked for, in bins.
pub const SKIRT_BINS: usize = 32;

/// A Hann window's sidelobe envelope at `bins` from its peak, dB: its first
/// sidelobe at two and a half bins, 31.5 dB down, and 18 dB an octave
/// after; inside the main lobe, nothing is told apart.
fn hann_envelope_db(bins: f64) -> f64 {
    if bins <= 2.5 {
        0.0
    } else {
        -31.5 - 18.0 * (bins / 2.5).log2()
    }
}

/// One line.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    /// Its frequency, Hz, absolute: the centroid of its bins' excess.
    pub hz: f64,
    /// That frequency's standard uncertainty, Hz.
    pub hz_sigma: f64,
    /// Its peak bin over the local floor, dB.
    pub excess_db: f64,
    /// That level's standard uncertainty, dB.
    pub excess_sigma_db: f64,
    /// Bins it spans.
    pub width_bins: usize,
    /// The exposure at which it first stood out, seconds.
    pub first_seen_s: f64,
    /// Its peak bin.
    pub peak_bin: usize,
}

/// The lines in `x` (each bin's mean over its floor), strongest first, and
/// how many were left out past [`MAX_LINES`]. `first_over` keeps, per bin,
/// the frame it first crossed (0: never), across calls.
pub fn find_lines(
    x: &[f64],
    d: &Derived,
    frames: u64,
    frame_s: f64,
    first_over: &mut [u32],
    low_edge_hz: f64,
    bin_hz: f64,
) -> (Vec<Line>, usize) {
    let t = frames.max(1) as f64;
    let thr = threshold(frames.max(1), d.usable.len());
    // Groups of over-threshold bins, a one-bin gap allowed.
    let mut groups: Vec<(usize, usize)> = Vec::new();
    for k in d.usable.clone() {
        if x[k] > thr {
            if first_over[k] == 0 {
                first_over[k] = frames.min(u32::MAX as u64) as u32;
            }
            match groups.last_mut() {
                Some((_, end)) if k <= *end + 2 => *end = k,
                _ => groups.push((k, k)),
            }
        }
    }
    let mut lines: Vec<Line> = groups
        .iter()
        .map(|&(a, b)| {
            let peak = (a..=b).max_by(|&i, &j| x[i].total_cmp(&x[j])).unwrap();
            let w: Vec<f64> = (a..=b).map(|k| (x[k] - 1.0).max(0.0)).collect();
            let total: f64 = w.iter().sum();
            let c = (a..=b).zip(&w).map(|(k, w)| k as f64 * w).sum::<f64>() / total;
            // Each bin's mean is uncertain by its own level over the root of
            // the exposure; carried into the centroid.
            let var: f64 = (a..=b)
                .map(|k| ((k as f64 - c) / total).powi(2) * (x[k] / t.sqrt()).powi(2))
                .sum();
            let width = b - a + 1;
            let floor_sigma = if width == 1 { 1.0 / 12f64.sqrt() } else { 0.0 };
            Line {
                hz: low_edge_hz + c * bin_hz,
                hz_sigma: var.sqrt().max(floor_sigma) * bin_hz,
                excess_db: 10.0 * x[peak].log10(),
                excess_sigma_db: 10.0 * (1.0 + 1.0 / t.sqrt()).log10(),
                width_bins: width,
                first_seen_s: (a..=b)
                    .filter(|&k| first_over[k] > 0)
                    .map(|k| first_over[k])
                    .min()
                    .unwrap_or(0) as f64
                    * frame_s,
                peak_bin: peak,
            }
        })
        .collect();
    lines.sort_by(|p, q| q.excess_db.total_cmp(&p.excess_db));
    // Fold the skirts: walking from the strongest, a weaker line inside a
    // kept line's sidelobe envelope (with 3 dB to spare for chance) goes.
    let mut kept: Vec<Line> = Vec::with_capacity(lines.len());
    for l in lines {
        let skirt = kept.iter().any(|k| {
            let dist = (l.peak_bin as f64 - k.peak_bin as f64).abs();
            if dist > SKIRT_BINS as f64 {
                return false;
            }
            let leak = 10f64.powf((k.excess_db + hann_envelope_db(dist) + 3.0) / 10.0);
            10f64.powf(l.excess_db / 10.0) - 1.0 <= leak
        });
        if !skirt {
            kept.push(l);
        }
    }
    let left_out = kept.len().saturating_sub(MAX_LINES);
    kept.truncate(MAX_LINES);
    (kept, left_out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::floor::chance::{local_floor, threshold};
    use crate::signal::floor::frames::{Frames, Sums};
    use crate::signal::floor::settings::{derive, FloorSetting};
    use crate::signal::floor::testing::{add, noise, tone};
    use rustfft::num_complex::Complex;

    /// A small band, so long exposures run quickly; a bin's statistics do
    /// not depend on how many bins there are.
    const RATE: f64 = 12_000.0;
    const SIGMA: f32 = 0.01;

    struct Run {
        d: Derived,
        frames: Frames,
        sums: Sums,
        first_over: Vec<u32>,
    }

    impl Run {
        fn new() -> Run {
            let d = derive(&FloorSetting::default(), RATE).unwrap();
            Run {
                frames: Frames::new(&d),
                sums: Sums::new(&d),
                first_over: vec![0; d.bins],
                d,
            }
        }

        fn push(&mut self, iq: &[Complex<f32>]) {
            self.frames.push(iq, false, &mut self.sums);
        }

        fn lines(&mut self) -> (Vec<Line>, usize) {
            let t = self.sums.frames;
            let mean: Vec<f64> = self.sums.s1.iter().map(|s| s / t as f64).collect();
            let floor = local_floor(&mean, &self.d, t);
            let x: Vec<f64> = mean
                .iter()
                .zip(&floor)
                .map(|(m, f)| if *f > 0.0 { m / f } else { 0.0 })
                .collect();
            find_lines(
                &x,
                &self.d,
                t,
                1.0,
                &mut self.first_over,
                -RATE / 2.0,
                self.d.bin_hz,
            )
        }
    }

    /// A tone's amplitude for a given excess over one bin's mean noise, on a
    /// bin's centre: the Hann window's coherent gain is a half, its noise
    /// gain three eighths.
    fn amp_for(excess: f64, n: usize) -> f32 {
        (SIGMA as f64 * (1.5 * excess / n as f64).sqrt()) as f32
    }

    #[test]
    fn pure_noise_gives_almost_no_lines() {
        let mut seed = 21;
        let mut total = 0;
        for _ in 0..30 {
            let mut r = Run::new();
            for _ in 0..60 {
                r.push(&noise(RATE as usize, SIGMA, &mut seed));
            }
            total += r.lines().0.len();
        }
        assert!(total <= 9, "{total} lines on noise alone over 30 exposures");
    }

    /// A tone 6 dB under one bin's noise: nothing in one frame, a line once
    /// the exposure is deep enough, and first seen about when the threshold
    /// says it should be.
    #[test]
    fn a_tone_too_weak_for_one_second_rises_by_the_predicted_exposure() {
        let mut r = Run::new();
        let (mut seed, mut phase) = (4, 0.0);
        let a = amp_for(0.25, r.d.frame_len);
        let n = RATE as usize;
        let mut seen_at_one = None;
        for f in 0..600u64 {
            let iq = add(
                tone(RATE, 1234.0, a, f * n as u64, n, &mut phase),
                &noise(n, SIGMA, &mut seed),
            );
            r.push(&iq);
            let (lines, _) = r.lines();
            if f == 0 {
                seen_at_one = Some(lines.iter().any(|l| (l.hz - 1234.0).abs() < 1.5));
            }
        }
        assert_eq!(seen_at_one, Some(false));
        let (lines, _) = r.lines();
        let l = lines
            .iter()
            .find(|l| (l.hz - 1234.0).abs() < 1.0)
            .expect("the tone is a line by 600 s");
        let usable = r.d.usable.len();
        let predicted = (1..2000u64).find(|&t| threshold(t, usable) < 1.25).unwrap() as f64;
        assert!(
            (l.first_seen_s / predicted - 1.0).abs() < 0.6,
            "first seen at {} s, predicted {predicted} s",
            l.first_seen_s
        );
    }

    /// A carrier a million times the floor leaks through the window's
    /// sidelobes into its neighbours; those are its skirt, not lines.
    #[test]
    fn a_strong_carrier_brings_no_crowd_of_skirt_lines() {
        let mut r = Run::new();
        let (mut seed, mut phase) = (8, 0.0);
        let a = amp_for(1e6, r.d.frame_len);
        let n = RATE as usize;
        for f in 0..300u64 {
            r.push(&add(
                tone(RATE, 2000.3, a, f * n as u64, n, &mut phase),
                &noise(n, SIGMA, &mut seed),
            ));
        }
        let (lines, _) = r.lines();
        let near: Vec<f64> = lines
            .iter()
            .filter(|l| (l.hz - 2000.3).abs() <= 32.0)
            .map(|l| l.hz)
            .collect();
        assert_eq!(near.len(), 1, "{near:?}");
    }

    /// A tone drifting two hertz over the exposure, as a radio warming moves
    /// one, smears over its bins and stays one line.
    #[test]
    fn a_drifting_line_stays_one_line() {
        let mut r = Run::new();
        let (mut seed, mut phase) = (12, 0.0);
        let a = amp_for(10.0, r.d.frame_len);
        let n = RATE as usize;
        for f in 0..300u64 {
            let hz = 3000.0 + 2.0 * f as f64 / 300.0;
            r.push(&add(
                tone(RATE, hz, a, f * n as u64, n, &mut phase),
                &noise(n, SIGMA, &mut seed),
            ));
        }
        let (lines, _) = r.lines();
        let near: Vec<&Line> = lines
            .iter()
            .filter(|l| (l.hz - 3001.0).abs() <= 6.0)
            .collect();
        assert_eq!(
            near.len(),
            1,
            "{:?}",
            near.iter().map(|l| l.hz).collect::<Vec<_>>()
        );
        assert!(near[0].width_bins >= 2, "{}", near[0].width_bins);
    }

    /// The folding rule itself: bumps inside a strong line's sidelobe
    /// envelope, separated from it by quiet bins, are its skirt; a line
    /// above the envelope is kept.
    #[test]
    fn bumps_inside_a_strong_lines_envelope_are_its_skirt() {
        let d = derive(&FloorSetting::default(), RATE).unwrap();
        let mut x = vec![1.0; d.bins];
        let p = d.usable.start + 500;
        x[p] = 1e6;
        // Half the envelope's leak at four and seven bins: both stand over
        // the threshold, and the quiet bins between keep them apart.
        for off in [4usize, 7] {
            let leak = 1e6 * 10f64.powf(hann_envelope_db(off as f64) / 10.0);
            assert!(1.0 + 0.5 * leak > threshold(100, d.usable.len()));
            x[p + off] = 1.0 + 0.5 * leak;
            x[p - off] = 1.0 + 0.5 * leak;
        }
        x[p + 20] = 50.0;
        let mut first = vec![0; d.bins];
        let (lines, _) = find_lines(&x, &d, 100, 1.0, &mut first, 0.0, 1.0);
        let bins: Vec<usize> = lines.iter().map(|l| l.peak_bin).collect();
        assert_eq!(bins, vec![p, p + 20]);
    }

    #[test]
    fn a_one_bin_gap_does_not_split_a_line() {
        let d = derive(&FloorSetting::default(), RATE).unwrap();
        let mut x = vec![1.0; d.bins];
        let k = d.usable.start + 900;
        x[k] = 5.0;
        x[k + 2] = 4.0;
        let mut first = vec![0; d.bins];
        let (lines, _) = find_lines(&x, &d, 100, 1.0, &mut first, 0.0, 1.0);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].width_bins, 3);
    }

    #[test]
    fn the_cap_keeps_the_strongest_and_counts_the_rest() {
        let d = derive(&FloorSetting::default(), RATE).unwrap();
        let mut x = vec![1.0; d.bins];
        for i in 0..1200usize {
            x[d.usable.start + 4 + 7 * i] = 2.0 + i as f64 * 0.01;
        }
        let mut first = vec![0; d.bins];
        let (lines, left_out) = find_lines(&x, &d, 100, 1.0, &mut first, -RATE / 2.0, d.bin_hz);
        assert_eq!((lines.len(), left_out), (MAX_LINES, 200));
        assert!(lines.windows(2).all(|w| w[0].excess_db >= w[1].excess_db));
        assert!(lines[0].excess_db > 10.0 * (2.0 + 11.9f64).log10() - 0.01);
    }
}
