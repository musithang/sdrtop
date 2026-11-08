// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The noise under the FT8 band in one cycle, as power per hertz on the
//! samples' own scale (for the radio's stream: dBFS of its word per Hz).
//!
//! **Its own transform, not the search's.** The search's spectrogram is
//! unwindowed, which is right for finding tones and wrong for this: a strong
//! signal's sidelobes spread over hundreds of hertz, and on a cycle with two
//! dozen signals at -5 to +20 dB its cells' median read the floor 10.8 dB
//! high. Here every frame is weighted by a four-term Blackman-Harris window,
//! whose sidelobes sit some 92 dB down.
//!
//! **Where nobody is sending.** Even windowed, a busy band hides its floor
//! while stations transmit: their own skirts and their starts and stops fill
//! the gaps between them (two dozen signals still read 1.8 dB high, forty
//! 3.3, at any quantile). But FT8 has a pause in every cycle: a transmission
//! ends 13.14 s after the boundary plus its DT, and the next starts half a
//! second after the next boundary. So the floor is read in the quietest
//! [`QUIET_FRAMES`] frames of the cycle, found from the band's own power,
//! which puts them in that pause wherever this computer's clock puts the
//! boundary.
//!
//! **Chosen in one half, read in the other.** Picking the quietest of some
//! hundred and eighty stretches also picks the noise's own lucky low: on a
//! band of noise alone that read 0.4 dB under the truth, and 0 dB on a busy
//! band, an error that would show as the floor falling when the band goes
//! quiet. The pause is the whole band's, so the stretch is chosen from the
//! lower half's power and read in the upper half's cells, and the other way
//! round; the two halves' noise is independent, so the choosing tells the
//! reading nothing.
//!
//! **Read, and checked against itself.** A cell of noise alone is
//! exponential, so every quantile names the same mean: read at [`Q_READ`]
//! and at [`Q_CHECK`], noise alone gives the same figure from both, and the
//! more of the cells something fills all cycle long (another mode that never
//! pauses, wideband interference) the further the two part. Past
//! [`AGREE_DB`] there is no floor to see, and none is given (POLICY.md,
//! rule 2).
//!
//! Measured (`measure_the_floor_under_signals`, 6 cycles a cell, 0 to 40
//! transmissions at -5 to +20 dB): the median reads the floor within 0.12 dB
//! on average at every crowding, one cycle within about 0.5 dB.

use std::f64::consts::PI;

use num_complex::Complex;
use rustfft::FftPlanner;

use super::search::{F_MAX, F_MIN, RATE};

/// Samples a frame: one symbol, so a bin is one tone (6.25 Hz). Frames
/// step by half that, which the window's taper leaves nearly independent.
const FRAME: usize = 1920;
const HOP: usize = FRAME / 2;

/// Frames the floor is read in: 0.64 s, inside the shortest pause a cycle
/// has (from DT +1.0 s, the latest the search reaches with a whole
/// transmission in the cycle, to the boundary: 0.86 s).
pub const QUIET_FRAMES: usize = 7;

/// The quantile the floor is read at (the least biased of 0.1 to 0.5 in
/// the measurement), and the one it is checked against.
pub const Q_READ: f64 = 0.5;
pub const Q_CHECK: f64 = 0.2;

/// The most the two readings may differ by, dB, for the floor to be given.
pub const AGREE_DB: f64 = 1.0;

/// The cycle's floor, dB per Hz on the samples' scale, or `None` when no
/// stretch of the band is quiet enough to show it, or the cycle is too short.
pub fn read(samples: &[Complex<f32>]) -> Option<f32> {
    let cells = quiet_cells(samples)?;
    let (read, check) = (at_quantile(&cells, Q_READ)?, at_quantile(&cells, Q_CHECK)?);
    ((read - check).abs() <= AGREE_DB).then_some(read as f32)
}

/// [`read`] at one quantile, unchecked: for measuring.
pub fn read_at(samples: &[Complex<f32>], q: f64) -> Option<f64> {
    at_quantile(&quiet_cells(samples)?, q)
}

/// The window's weights, and their energy.
fn window() -> (Vec<f32>, f64) {
    let w: Vec<f32> = (0..FRAME)
        .map(|n| {
            let x = 2.0 * PI * n as f64 / FRAME as f64;
            (0.35875 - 0.48829 * x.cos() + 0.14128 * (2.0 * x).cos() - 0.01168 * (3.0 * x).cos())
                as f32
        })
        .collect();
    let energy = w.iter().map(|&v| f64::from(v * v)).sum();
    (w, energy)
}

/// Every frame's cells from [`F_MIN`] to [`F_MAX`], already divided by the
/// window's energy; then, for each half of the band, the cells of the
/// [`QUIET_FRAMES`] consecutive frames the other half found quietest.
fn quiet_cells(samples: &[Complex<f32>]) -> Option<Vec<f32>> {
    if samples.len() < FRAME {
        return None;
    }
    let frames = (samples.len() - FRAME) / HOP + 1;
    if frames < QUIET_FRAMES {
        return None;
    }
    let lo = (F_MIN * FRAME as f64 / RATE).ceil() as usize;
    let hi = (F_MAX * FRAME as f64 / RATE).floor() as usize;
    let (window, energy) = window();
    let fft = FftPlanner::new().plan_fft_forward(FRAME);
    let mut buf = vec![Complex::new(0.0f32, 0.0); FRAME];
    let width = hi - lo + 1;
    let mut cells = Vec::with_capacity(frames * width);
    for f in 0..frames {
        for ((b, &x), &w) in buf
            .iter_mut()
            .zip(&samples[f * HOP..f * HOP + FRAME])
            .zip(&window)
        {
            *b = x * w;
        }
        fft.process(&mut buf);
        cells.extend(
            buf[lo..=hi]
                .iter()
                .map(|c| (f64::from(c.norm_sqr()) / energy) as f32),
        );
    }
    let half = width / 2;
    let halves = [0..half, half..width];
    // The first frame of the quietest run, by the power in `bins`.
    let quietest = |bins: &std::ops::Range<usize>| {
        let power: Vec<f64> = cells
            .chunks(width)
            .map(|row| row[bins.clone()].iter().map(|&c| f64::from(c)).sum())
            .collect();
        (0..=frames - QUIET_FRAMES)
            .min_by(|&a, &b| {
                let sa: f64 = power[a..a + QUIET_FRAMES].iter().sum();
                let sb: f64 = power[b..b + QUIET_FRAMES].iter().sum();
                sa.total_cmp(&sb)
            })
            .unwrap_or(0)
    };
    let mut out = Vec::with_capacity(QUIET_FRAMES * width);
    for (read, choose) in [(&halves[1], &halves[0]), (&halves[0], &halves[1])] {
        let first = quietest(choose);
        for row in cells.chunks(width).skip(first).take(QUIET_FRAMES) {
            out.extend_from_slice(&row[read.clone()]);
        }
    }
    Some(out)
}

/// The floor from the cells' `q` quantile, dB per Hz: for noise alone a
/// cell (already over the window's energy) is the noise's variance on
/// average, which is its `q` quantile over `-ln(1 - q)`; the variance over
/// [`RATE`] is the power per hertz.
fn at_quantile(cells: &[f32], q: f64) -> Option<f64> {
    if cells.is_empty() {
        return None;
    }
    let mut cells = cells.to_vec();
    let at = ((cells.len() as f64 * q) as usize).min(cells.len() - 1);
    let (_, &mut cell, _) = cells.select_nth_unstable_by(at, f32::total_cmp);
    let per_hz = f64::from(cell) / -(1.0 - q).ln() / RATE;
    (per_hz > 0.0).then(|| 10.0 * per_hz.log10())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::dsp::testkit::Rng;

    const CYCLE: usize = 15 * RATE as usize;

    #[test]
    fn noise_alone_reads_its_floor() {
        let mut rng = Rng::new(3);
        let x = rng.noise(CYCLE, 1e-4);
        let floor = f64::from(read(&x).expect("a floor"));
        let want = 10.0 * (1e-4 / RATE).log10();
        assert!((floor - want).abs() < 0.5, "read {floor}, sent {want}");
    }

    /// Something 20 dB over the noise across 100 to 1500 Hz all cycle long:
    /// half the cells are taken and never pause, so no floor is given.
    #[test]
    fn a_band_filled_all_cycle_gives_no_floor() {
        let mut rng = Rng::new(5);
        let mut x = rng.noise(CYCLE, 1.0);
        let fill = rng.noise(CYCLE, 1.0);
        let mut fft = FftPlanner::<f32>::new();
        let mut spec = fill.clone();
        fft.plan_fft_forward(CYCLE).process(&mut spec);
        let bin = RATE / CYCLE as f64;
        for (k, c) in spec.iter_mut().enumerate() {
            let hz = k as f64 * bin;
            *c *= if (100.0..1500.0).contains(&hz) {
                10.0
            } else {
                0.0
            };
        }
        fft.plan_fft_inverse(CYCLE).process(&mut spec);
        for (v, f) in x.iter_mut().zip(&spec) {
            *v += f / CYCLE as f32;
        }
        assert_eq!(read(&x), None);
    }

    #[test]
    fn a_cycle_too_short_has_no_floor() {
        let mut rng = Rng::new(7);
        assert_eq!(read(&rng.noise(FRAME, 1.0)), None);
    }
}
