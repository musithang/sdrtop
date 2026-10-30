// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! One synchronised candidate to soft bits, and a decoded transmission to
//! its physics.
//!
//! **Soft bits by block detection** (the QEX paper, section 6). Every symbol
//! is correlated with each of the eight tones. For blocks of one, two or
//! three consecutive data symbols, every tone sequence's correlations are
//! summed coherently and the magnitude taken; a bit's soft decision is the
//! best magnitude among the sequences where it is one, less the best where
//! it is zero. One symbol alone is robust to fading; longer blocks gain
//! sensitivity while the signal keeps its phase. The decoder tries one, then
//! two, then three.
//!
//! **Coherence across symbols.** FT8 is continuous-phase FSK with h = 1: a
//! symbol of tone `t` advances the phase by exactly `2 pi t`, so each symbol
//! starts with the carrier's own phase. The correlations here keep that: the
//! offset from tone 0 is referred to absolute time, the tone itself to the
//! symbol's start, so two symbols of one transmission correlate with one
//! common phase.
//!
//! **What a decode measured** ([`measure`]). Once the 79 tones are known, the
//! whole transmission is rebuilt and the received slice correlated against it
//! symbol by symbol. The frequency and the drift are the ones that make those
//! 79 correlations add up coherently; the amplitude is their sum; the noise
//! comes from the tones not sent (their median, so a neighbour's signal in one
//! bin does not raise it). The frequency carries its Cramer-Rao bound, the
//! least uncertainty any estimator could have at the measured SNR.

use std::f64::consts::TAU;

use num_complex::Complex;
use rustfft::FftPlanner;

use super::ldpc;
use super::modulate::{
    self, data_positions, Waveform, COSTAS, COSTAS_AT, GRAY, SYMBOLS, SYMBOL_SECONDS, TONE_SPACING,
};
use super::search::{DOWN_RATE, DOWN_SYMBOL};

/// The inverse of [`GRAY`]: the bit triad a tone carries.
const TONE_BITS: [u8; 8] = {
    let mut inv = [0u8; 8];
    let mut v = 0;
    while v < 8 {
        inv[GRAY[v] as usize] = v as u8;
        v += 1;
    }
    inv
};

/// Each symbol's correlation with each of the eight tones.
pub type Correlations = [[Complex<f32>; 8]; SYMBOLS];

/// Correlate a baseband slice `y` (at [`DOWN_RATE`], with tone 0 at `hz`)
/// for a transmission starting at sample `n0`.
pub fn correlate(y: &[Complex<f32>], n0: i64, hz: f64) -> Correlations {
    let mut z = [[Complex::new(0.0f32, 0.0); 8]; SYMBOLS];
    for (s, row) in z.iter_mut().enumerate() {
        let start = n0 + (s * DOWN_SYMBOL) as i64;
        for (t, cell) in row.iter_mut().enumerate() {
            let mut acc = Complex::new(0.0f64, 0.0);
            for n in 0..DOWN_SYMBOL as i64 {
                let i = start + n;
                if i < 0 || i as usize >= y.len() {
                    continue;
                }
                // The offset against absolute time, the tone against the
                // symbol's start: one phase for the whole transmission.
                let ph = -TAU * (hz * i as f64 + t as f64 * TONE_SPACING * n as f64) / DOWN_RATE;
                let v = y[i as usize];
                acc +=
                    Complex::new(f64::from(v.re), f64::from(v.im)) * Complex::from_polar(1.0, ph);
            }
            *cell = Complex::new(acc.re as f32, acc.im as f32);
        }
    }
    z
}

/// How many of the 21 Costas symbols are strongest on the tone they should be.
pub fn costas_matched(z: &Correlations) -> u8 {
    let mut n = 0;
    for &at in &COSTAS_AT {
        for (i, &tone) in COSTAS.iter().enumerate() {
            let row = &z[at + i];
            let best = (0..8)
                .max_by(|&a, &b| row[a].norm_sqr().total_cmp(&row[b].norm_sqr()))
                .unwrap_or(0);
            n += u8::from(best == usize::from(tone));
        }
    }
    n
}

/// The soft decisions are scaled to this standard deviation before belief
/// propagation, which reads them as log-likelihood ratios. Swept in the
/// conformance harness at -20 to -22 dB (108 transmissions): 1.5 and 2.0 gave
/// 31 decodes, 2.8 gave 34, 3.5 and 4.5 gave 36, none wrong.
pub const LLR_SCALE: f32 = 3.5;

/// The 174 soft decisions from blocks of `block` symbols (1, 2 or 3),
/// `ln P(1)/P(0)`-signed, scaled to a standard deviation of `scale`
/// ([`LLR_SCALE`] unless a measurement asks otherwise).
pub fn soft_bits(z: &Correlations, block: usize, scale: f32) -> [f32; ldpc::N] {
    let mut llr = [0f32; ldpc::N];
    let positions: Vec<usize> = data_positions().collect();
    // The two runs of 29 data symbols, each cut into blocks.
    for run in positions.chunks(29) {
        for group in run.chunks(block.clamp(1, 3)) {
            let g = group.len();
            let mut best1 = [f32::MIN; 9];
            let mut best0 = [f32::MIN; 9];
            for seq in 0..8usize.pow(g as u32) {
                let mut sum = Complex::new(0.0f32, 0.0);
                let mut bits = 0u32;
                for (k, &pos) in group.iter().enumerate() {
                    let tone = seq >> (3 * (g - 1 - k)) & 7;
                    sum += z[pos][tone];
                    bits = bits << 3 | u32::from(TONE_BITS[tone]);
                }
                let m = sum.norm();
                for j in 0..3 * g {
                    let bit = bits >> (3 * g - 1 - j) & 1;
                    let slot = if bit == 1 {
                        &mut best1[j]
                    } else {
                        &mut best0[j]
                    };
                    *slot = slot.max(m);
                }
            }
            let first = data_index(group[0]);
            for j in 0..3 * g {
                llr[3 * first + j] = best1[j] - best0[j];
            }
        }
    }
    let var = llr.iter().map(|l| l * l).sum::<f32>() / ldpc::N as f32;
    let scale = if var > 0.0 { scale / var.sqrt() } else { 0.0 };
    llr.map(|l| l * scale)
}

/// A data symbol's index among the 58, from its position among the 79.
fn data_index(pos: usize) -> usize {
    if pos < 36 {
        pos - 7
    } else {
        pos - 14
    }
}

/// What a decoded transmission measured.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Physics {
    /// Signal power over noise power in 2500 Hz, dB.
    pub snr_db: f64,
    /// The first symbol's start, seconds into the cycle, to a fraction of a
    /// millisecond.
    pub start: f64,
    /// Tone 0 in the middle of the transmission, Hz above the dial.
    pub f0: f64,
    /// Its Cramer-Rao bound, one standard deviation, Hz.
    pub f0_sigma: f64,
    /// Frequency change from the first symbol to the last, Hz.
    pub drift: f64,
    /// Amplitude of the received signal, in the slice's units.
    pub amplitude: f64,
    /// Carrier phase at the start, for the subtraction.
    pub phase: f64,
}

/// The tones a 91-bit word is sent as.
pub fn tones_of(word91: u128) -> [u8; SYMBOLS] {
    modulate::channel_symbols(&ldpc::encode(word91))
}

/// Frequency and drift grids for [`measure`].
const MEASURE_HZ: f64 = 1.0;
const DRIFT_HZ: f64 = 4.0;
const DRIFT_STEP_HZ: f64 = 0.25;

/// The start is searched this far either side of the synchronised one, in
/// steps of [`START_STEP`] seconds: the search's grid is a slice sample (5 ms),
/// and a reference a few milliseconds off its signal smears every tone
/// change of the smoothed waveform, which reads as lost amplitude.
const START_SPAN: f64 = 0.008;
const START_STEP: f64 = 0.0005;

/// A frequency offset's floor of uncertainty, Hz, added in quadrature to the
/// Cramer-Rao bound: the waveform model's own error, measured in the decode
/// rows (the bound alone was three to ten times too small).
const F0_FLOOR: f64 = 0.01;

/// The transmission at `start` (seconds into the slice) correlated, symbol by
/// symbol, against its rebuilt waveform; and that waveform's energy.
fn per_symbol(
    y: &[Complex<f32>],
    start: f64,
    hz: f64,
    tones: &[u8; SYMBOLS],
) -> (Vec<Complex<f64>>, f64) {
    let mut reference = vec![Complex::new(0.0f32, 0.0); y.len()];
    let w = Waveform {
        f0: hz,
        start,
        amplitude: 1.0,
        drift: 0.0,
        phase: 0.0,
    };
    modulate::add(tones, &w, DOWN_RATE, &mut reference);
    let first = (start * DOWN_RATE).ceil() as i64;
    let per = (0..SYMBOLS)
        .map(|s| {
            let a = first + (s * DOWN_SYMBOL) as i64;
            (a..a + DOWN_SYMBOL as i64)
                .filter(|&i| i >= 0 && (i as usize) < y.len())
                .map(|i| {
                    let (v, r) = (y[i as usize], reference[i as usize]);
                    Complex::new(f64::from(v.re), f64::from(v.im))
                        * Complex::new(f64::from(r.re), -f64::from(r.im))
                })
                .sum()
        })
        .collect();
    let energy = reference.iter().map(|r| f64::from(r.norm_sqr())).sum();
    (per, energy)
}

/// The 79 correlations summed with a frequency offset and a drift taken out:
/// the phase model `2 pi (df t + drift t^2 / 2L)`, `t` from the middle.
fn coherent(per: &[Complex<f64>], df: f64, drift: f64) -> Complex<f64> {
    let middle = SYMBOLS as f64 / 2.0;
    let length = SYMBOLS as f64 * SYMBOL_SECONDS;
    per.iter()
        .enumerate()
        .map(|(s, z)| {
            let t = (s as f64 + 0.5 - middle) * SYMBOL_SECONDS;
            z * Complex::from_polar(1.0, -TAU * (df * t + drift * t * t / (2.0 * length)))
        })
        .sum()
}

/// Transform length for the frequency search: 79 symbols zero-padded, a bin
/// of 1 / (512 T) = 0.0122 Hz.
const FIT_FFT: usize = 512;

/// The frequency offset (within [`MEASURE_HZ`]) and drift that make the
/// correlations add up best, and that sum's magnitude. For each drift on its
/// grid, the drift's phase is taken out and the frequency search is a
/// zero-padded transform over the symbols (the same sum as [`coherent`],
/// evaluated at every bin at once); the best bin is refined by a parabola.
fn fit(per: &[Complex<f64>]) -> (f64, f64, f64) {
    let fft = FftPlanner::<f64>::new().plan_fft_forward(FIT_FFT);
    let bin_hz = 1.0 / (FIT_FFT as f64 * SYMBOL_SECONDS);
    let reach = (MEASURE_HZ / bin_hz).floor() as i64;
    let nd = (DRIFT_HZ / DRIFT_STEP_HZ).round() as i64;
    let middle = SYMBOLS as f64 / 2.0;
    let length = SYMBOLS as f64 * SYMBOL_SECONDS;
    let mut best = (0.0, 0.0, f64::MIN);
    let mut buf = vec![Complex::new(0.0, 0.0); FIT_FFT];
    for i in -nd..=nd {
        let drift = i as f64 * DRIFT_STEP_HZ;
        buf.fill(Complex::new(0.0, 0.0));
        for (s, z) in per.iter().enumerate() {
            let t = (s as f64 + 0.5 - middle) * SYMBOL_SECONDS;
            buf[s] = z * Complex::from_polar(1.0, -TAU * drift * t * t / (2.0 * length));
        }
        fft.process(&mut buf);
        let mag = |k: i64| buf[k.rem_euclid(FIT_FFT as i64) as usize].norm();
        for k in -reach..=reach {
            let m = mag(k);
            if m > best.2 {
                let df = (k as f64 + vertex(mag(k - 1), m, mag(k + 1))) * bin_hz;
                best = (df, drift, m);
            }
        }
    }
    let (df, drift, _) = best;
    (df, drift, coherent(per, df, drift).norm())
}

/// The vertex of the parabola through three equally spaced values, in steps
/// from the middle one.
fn vertex(a: f64, b: f64, c: f64) -> f64 {
    let den = a - 2.0 * b + c;
    if den >= 0.0 {
        return 0.0;
    }
    (0.5 * (a - c) / den).clamp(-0.5, 0.5)
}

/// Measure a decoded transmission in its slice `y` (tone 0 near `hz`,
/// starting near `start` seconds), given the tones it was sent as.
pub fn measure(
    y: &[Complex<f32>],
    start: f64,
    hz: f64,
    tones: &[u8; SYMBOLS],
    z: &Correlations,
) -> Physics {
    // Frequency and drift at the synchronised start, then the start on a
    // finer grid with them, then both again at the start found.
    let (df, drift, _) = fit(&per_symbol(y, start, hz, tones).0);
    let k = (START_SPAN / START_STEP).round() as i64;
    let scan: Vec<f64> = (-k..=k)
        .map(|i| {
            let (per, _) = per_symbol(y, start + i as f64 * START_STEP, hz, tones);
            coherent(&per, df, drift).norm()
        })
        .collect();
    let best = (0..scan.len())
        .max_by(|&a, &b| scan[a].total_cmp(&scan[b]))
        .unwrap_or(k as usize);
    let off = if best > 0 && best + 1 < scan.len() {
        vertex(scan[best - 1], scan[best], scan[best + 1])
    } else {
        0.0
    };
    let start = start + ((best as i64 - k) as f64 + off) * START_STEP;
    let (per, energy) = per_symbol(y, start, hz, tones);
    let (df, drift, _) = fit(&per);
    let sum = coherent(&per, df, drift);
    let amplitude = sum.norm() / energy.max(f64::MIN_POSITIVE);

    // Noise per sample from the tones not sent: |z|^2 of noise alone is
    // exponential with mean 32 sigma^2, so its median is ln 2 of that.
    let mut off: Vec<f32> = z
        .iter()
        .zip(tones)
        .flat_map(|(row, &t)| {
            row.iter()
                .enumerate()
                .filter(move |&(k, _)| k != usize::from(t))
                .map(|(_, c)| c.norm_sqr())
        })
        .collect();
    off.sort_by(f32::total_cmp);
    let median = f64::from(off[off.len() / 2]);
    let sigma2 = median / std::f64::consts::LN_2 / DOWN_SYMBOL as f64;
    // Noise density per Hz is sigma^2 over the slice's rate; FT8's SNR is the
    // signal's power over the noise in 2500 Hz.
    let n0_density = sigma2 / DOWN_RATE;
    let snr = amplitude * amplitude / (2500.0 * n0_density).max(f64::MIN_POSITIVE);

    // Cramer-Rao bound for a tone's frequency over M samples at rate fs:
    // var = 6 fs^2 / ((2 pi)^2 rho M (M^2 - 1)), rho the per-sample SNR.
    let m = (SYMBOLS * DOWN_SYMBOL) as f64;
    let rho = amplitude * amplitude / sigma2.max(f64::MIN_POSITIVE);
    let var = 6.0 * DOWN_RATE * DOWN_RATE / (TAU * TAU * rho * m * (m * m - 1.0));

    Physics {
        snr_db: 10.0 * snr.max(1e-6).log10(),
        start,
        f0: hz + df,
        f0_sigma: (var + F0_FLOOR * F0_FLOOR).sqrt(),
        drift,
        amplitude,
        phase: sum.arg(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tone_bits_invert_the_gray_map() {
        for v in 0..8 {
            assert_eq!(usize::from(TONE_BITS[usize::from(GRAY[v])]), v);
        }
    }

    #[test]
    fn data_index_counts_the_58() {
        let idx: Vec<usize> = data_positions().map(data_index).collect();
        assert_eq!(idx, (0..58).collect::<Vec<_>>());
    }
}
