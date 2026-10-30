// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Finding transmissions in one cycle of samples: where in time each starts
//! and what tone 0's frequency is.
//!
//! **Two passes, coarse then fine.**
//!
//! 1. [`Spectrogram`]: the cycle in symbol-long frames, half a symbol apart,
//!    in bins half a tone wide. Every place in that grid is scored by how
//!    much of the power in its eight tones sits on the tones the three Costas
//!    arrays should light ([`candidates`]). The best local maxima become
//!    candidates: within 80 ms and 3.125 Hz of the truth.
//! 2. [`Baseband`] and [`refine`]: the whole cycle transformed once, and for
//!    each candidate a narrow slice of that spectrum brought back to the time
//!    domain at 200 samples a second with tone 0 at zero. There the Costas
//!    symbols are correlated against their tones on a fine grid of time (one
//!    sample, 5 ms) and frequency (0.25 Hz), each symbol on its own
//!    (noncoherently, so the carrier's phase does not matter), and the peak
//!    is interpolated.
//!
//! The candidate's sync score is a ratio, not a decision: noise scores
//! about 1, and which candidates are worth decoding is the decoder's
//! question, answered by measurement.

use std::f64::consts::{PI, TAU};
use std::sync::Arc;

use num_complex::Complex;
use rustfft::{Fft, FftPlanner};

use super::modulate::{COSTAS, COSTAS_AT, SYMBOL_SECONDS, TONE_SPACING};

/// The rate a cycle's samples arrive at: 1920 samples a symbol.
pub const RATE: f64 = 12_000.0;

/// Samples in a symbol at [`RATE`].
const SYMBOL: usize = 1920;

/// The spectrogram's step: half a symbol.
const STEP: usize = SYMBOL / 2;

/// The spectrogram's transform: a symbol zero-padded to twice its length, so
/// a bin is half a tone (3.125 Hz).
const NFFT: usize = 2 * SYMBOL;

/// Bin width, Hz.
const BIN_HZ: f64 = RATE / NFFT as f64;

/// The lowest and highest tone 0 searched, Hz above the dial. Below 100 Hz
/// is the receiver's own (what is left of its DC).
pub const F_MIN: f64 = 100.0;
pub const F_MAX: f64 = 3000.0;

/// Earliest and latest first-symbol start searched, seconds from the start
/// of the buffer: DT from -2.5 to +2.5 s about the nominal 0.5 s.
pub const START_MIN: f64 = -2.0;
pub const START_MAX: f64 = 3.0;

/// The most candidates one search returns, strongest first.
pub const MAX_CANDIDATES: usize = 300;

/// A sync score below this is not a candidate.
pub const SYNC_MIN: f32 = 1.5;

/// Power by frame (half a symbol apart) and bin (half a tone wide).
pub struct Spectrogram {
    frames: usize,
    bins: usize,
    power: Vec<f32>,
}

impl Spectrogram {
    pub fn new(samples: &[Complex<f32>]) -> Self {
        let bins = ((F_MAX + 8.0 * TONE_SPACING) / BIN_HZ).ceil() as usize + 1;
        let frames = if samples.len() >= SYMBOL {
            (samples.len() - SYMBOL) / STEP + 1
        } else {
            0
        };
        let fft = FftPlanner::new().plan_fft_forward(NFFT);
        let mut buf = vec![Complex::new(0.0f32, 0.0); NFFT];
        let mut power = vec![0f32; frames * bins];
        for f in 0..frames {
            buf[..SYMBOL].copy_from_slice(&samples[f * STEP..f * STEP + SYMBOL]);
            buf[SYMBOL..].fill(Complex::new(0.0, 0.0));
            fft.process(&mut buf);
            for (b, p) in power[f * bins..(f + 1) * bins].iter_mut().enumerate() {
                *p = buf[b].norm_sqr();
            }
        }
        Self {
            frames,
            bins,
            power,
        }
    }

    /// Mean power across the band in bins `width_hz` wide from 0 to `top_hz`,
    /// in dB over the median of those bins: where there was energy this cycle,
    /// decoded or not, measured against the cycle's own floor.
    pub fn band(&self, width_hz: f64, top_hz: f64) -> Vec<f32> {
        if self.frames == 0 {
            return Vec::new();
        }
        let per = (width_hz / BIN_HZ).round().max(1.0) as usize;
        let count = ((top_hz / width_hz).floor() as usize).min(self.bins / per);
        let mut mean: Vec<f64> = (0..count)
            .map(|k| {
                let mut sum = 0f64;
                for f in 0..self.frames {
                    let row = &self.power[f * self.bins..(f + 1) * self.bins];
                    sum += row[k * per..(k + 1) * per]
                        .iter()
                        .map(|&p| f64::from(p))
                        .sum::<f64>();
                }
                sum / (self.frames * per) as f64
            })
            .collect();
        let mut sorted = mean.clone();
        sorted.sort_by(f64::total_cmp);
        let floor = sorted
            .get(sorted.len() / 2)
            .copied()
            .unwrap_or(1.0)
            .max(f64::MIN_POSITIVE);
        for m in &mut mean {
            *m = 10.0 * (m.max(f64::MIN_POSITIVE) / floor).log10();
        }
        mean.into_iter().map(|m| m as f32).collect()
    }

    fn row(&self, frame: i64) -> Option<&[f32]> {
        (0..self.frames as i64)
            .contains(&frame)
            .then(|| &self.power[frame as usize * self.bins..(frame as usize + 1) * self.bins])
    }

    /// The Costas score of a transmission whose first symbol starts at frame
    /// `frame` with tone 0 in bin `bin`: the power on the Costas tones over
    /// the mean power on the other seven, over the arrays inside the cycle.
    fn sync(&self, frame: i64, bin: usize) -> Option<f32> {
        let (mut on, mut all, mut symbols) = (0f32, 0f32, 0);
        for &at in &COSTAS_AT {
            for (i, &tone) in COSTAS.iter().enumerate() {
                let Some(row) = self.row(frame + 2 * (at + i) as i64) else {
                    continue;
                };
                all += (0..8).map(|t| row[bin + 2 * t]).sum::<f32>();
                on += row[bin + 2 * usize::from(tone)];
                symbols += 1;
            }
        }
        (symbols >= COSTAS.len() && all > on).then(|| on / ((all - on) / 7.0))
    }
}

/// A place worth looking at closely.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Candidate {
    /// The first symbol's start, seconds from the start of the buffer.
    pub start: f64,
    /// Tone 0, Hz above the dial.
    pub f0: f64,
    /// The Costas score.
    pub sync: f32,
}

/// The cycle's candidates, strongest first.
pub fn candidates(spec: &Spectrogram) -> Vec<Candidate> {
    let step_s = STEP as f64 / RATE;
    let frames = (START_MIN / step_s).floor() as i64..=(START_MAX / step_s).ceil() as i64;
    let bins = (F_MIN / BIN_HZ).floor() as usize..=(F_MAX / BIN_HZ).ceil() as usize;
    let width = bins.clone().count();
    let height = frames.clone().count();
    let mut score = vec![0f32; width * height];
    for (fi, frame) in frames.clone().enumerate() {
        for (bi, bin) in bins.clone().enumerate() {
            score[fi * width + bi] = spec.sync(frame, bin).unwrap_or(0.0);
        }
    }
    // Local maxima over the eight neighbours, above the floor.
    let mut peaks: Vec<(f32, usize, usize)> = Vec::new();
    for fi in 0..height {
        for bi in 0..width {
            let s = score[fi * width + bi];
            if s < SYNC_MIN {
                continue;
            }
            let neighbours = (-1i64..=1)
                .flat_map(|df| (-1i64..=1).map(move |db| (df, db)))
                .filter(|&(df, db)| df != 0 || db != 0);
            let is_peak = neighbours.into_iter().all(|(df, db)| {
                let (f, b) = (fi as i64 + df, bi as i64 + db);
                if f < 0 || b < 0 || f >= height as i64 || b >= width as i64 {
                    return true;
                }
                score[f as usize * width + b as usize] <= s
            });
            if is_peak {
                peaks.push((s, fi, bi));
            }
        }
    }
    peaks.sort_by(|a, b| b.0.total_cmp(&a.0));
    // Of peaks closer than a tone and a symbol, the strongest stands for all.
    let mut out: Vec<Candidate> = Vec::new();
    for (s, fi, bi) in peaks {
        let start = (*frames.start() + fi as i64) as f64 * step_s;
        let f0 = (*bins.start() + bi) as f64 * BIN_HZ;
        let near = out.iter().any(|c| {
            (c.f0 - f0).abs() < TONE_SPACING * 0.99
                && (c.start - start).abs() < SYMBOL_SECONDS * 0.99
        });
        if !near {
            out.push(Candidate { start, f0, sync: s });
            if out.len() == MAX_CANDIDATES {
                break;
            }
        }
    }
    out
}

/// The baseband rate a candidate is examined at.
pub const DOWN_RATE: f64 = 200.0;

/// Samples in a symbol at [`DOWN_RATE`].
pub const DOWN_SYMBOL: usize = 32;

/// How much of the spectrum, each side of tone 0, a slice keeps whole; it
/// tapers to nothing at half [`DOWN_RATE`].
const PASS_HZ: f64 = 80.0;

/// The whole cycle's spectrum, from which each candidate's slice is taken.
pub struct Baseband {
    spectrum: Vec<Complex<f32>>,
    inverse: Arc<dyn Fft<f32>>,
}

impl Baseband {
    pub fn new(samples: &[Complex<f32>]) -> Self {
        let decimation = (RATE / DOWN_RATE) as usize;
        let n = samples.len().div_ceil(decimation) * decimation;
        let mut spectrum = samples.to_vec();
        spectrum.resize(n, Complex::new(0.0, 0.0));
        let mut planner = FftPlanner::new();
        planner.plan_fft_forward(n).process(&mut spectrum);
        let inverse = planner.plan_fft_inverse(n / decimation);
        Self { spectrum, inverse }
    }

    fn bin_hz(&self) -> f64 {
        RATE / self.spectrum.len() as f64
    }

    /// The samples at [`DOWN_RATE`], shifted so that `f0` sits at zero, and
    /// the frequency actually put at zero (the nearest bin to `f0`).
    pub fn around(&self, f0: f64) -> (Vec<Complex<f32>>, f64) {
        let n = self.spectrum.len();
        let m = self.inverse.len();
        let df = self.bin_hz();
        let centre = (f0 / df).round() as i64;
        let mut small = vec![Complex::new(0.0f32, 0.0); m];
        let half = m as i64 / 2;
        for j in -half..half {
            let hz = j as f64 * df;
            let taper = if hz.abs() <= PASS_HZ {
                1.0
            } else {
                let x = (hz.abs() - PASS_HZ) / (DOWN_RATE / 2.0 - PASS_HZ);
                0.5 * (1.0 + (PI * x.min(1.0)).cos())
            };
            let src = (centre + j).rem_euclid(n as i64) as usize;
            small[j.rem_euclid(m as i64) as usize] = self.spectrum[src] * (taper / n as f64) as f32;
        }
        self.inverse.process(&mut small);
        (small, centre as f64 * df)
    }
}

/// A candidate after fine synchronisation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Synced {
    pub start: f64,
    pub f0: f64,
    /// The coarse Costas score it was found with.
    pub sync: f32,
    /// Costas power at the fine peak, per symbol, relative to the slice's
    /// mean power per symbol.
    pub power: f32,
}

/// Fine search half-widths and steps.
const FINE_SAMPLES: i64 = 20;
const FINE_HZ: f64 = 2.5;
const FINE_STEP_HZ: f64 = 0.25;

/// Noncoherent Costas energy in `y` (tone 0 at zero) for a start of `n0`
/// samples and a frequency offset of `hz`.
fn costas_energy(y: &[Complex<f32>], n0: i64, refs: &[Vec<Complex<f32>>; 8]) -> f32 {
    let mut e = 0f32;
    for &at in &COSTAS_AT {
        for (i, &tone) in COSTAS.iter().enumerate() {
            let s = n0 + ((at + i) * DOWN_SYMBOL) as i64;
            if s < 0 || s as usize + DOWN_SYMBOL > y.len() {
                continue;
            }
            let r = &refs[usize::from(tone)];
            let z: Complex<f32> = y[s as usize..s as usize + DOWN_SYMBOL]
                .iter()
                .zip(r)
                .map(|(a, b)| a * b)
                .sum();
            e += z.norm_sqr();
        }
    }
    e
}

/// The eight tones' reference waveforms, conjugated, at an offset of `hz`.
fn references(hz: f64) -> [Vec<Complex<f32>>; 8] {
    std::array::from_fn(|t| {
        let f = t as f64 * TONE_SPACING + hz;
        (0..DOWN_SYMBOL)
            .map(|n| {
                let ph = -TAU * f * n as f64 / DOWN_RATE;
                Complex::new(ph.cos() as f32, ph.sin() as f32)
            })
            .collect()
    })
}

/// The vertex of the parabola through three equally spaced values, as an
/// offset from the middle one in steps (within half a step).
fn vertex(a: f32, b: f32, c: f32) -> f64 {
    let den = f64::from(a - 2.0 * b + c);
    if den >= 0.0 {
        return 0.0;
    }
    (0.5 * f64::from(a - c) / den).clamp(-0.5, 0.5)
}

/// Fine time and frequency for a candidate.
pub fn refine(bb: &Baseband, c: &Candidate) -> Synced {
    let (y, at_zero) = bb.around(c.f0);
    let n0 = (c.start * DOWN_RATE).round() as i64;
    let offsets: Vec<f64> = {
        let k = (FINE_HZ / FINE_STEP_HZ).round() as i64;
        (-k..=k)
            .map(|i| i as f64 * FINE_STEP_HZ + (c.f0 - at_zero))
            .collect()
    };
    let refs: Vec<[Vec<Complex<f32>>; 8]> = offsets.iter().map(|&hz| references(hz)).collect();
    let dts: Vec<i64> = (-FINE_SAMPLES..=FINE_SAMPLES).collect();
    let mut grid = vec![0f32; dts.len() * offsets.len()];
    let mut best = (0usize, 0usize, f32::MIN);
    for (i, &dt) in dts.iter().enumerate() {
        for (j, r) in refs.iter().enumerate() {
            let e = costas_energy(&y, n0 + dt, r);
            grid[i * offsets.len() + j] = e;
            if e > best.2 {
                best = (i, j, e);
            }
        }
    }
    let (bi, bj, e) = best;
    let g = |i: usize, j: usize| grid[i * offsets.len() + j];
    let di = if bi > 0 && bi + 1 < dts.len() {
        vertex(g(bi - 1, bj), e, g(bi + 1, bj))
    } else {
        0.0
    };
    let dj = if bj > 0 && bj + 1 < offsets.len() {
        vertex(g(bi, bj - 1), e, g(bi, bj + 1))
    } else {
        0.0
    };
    let start = (n0 + dts[bi]) as f64 / DOWN_RATE + di / DOWN_RATE;
    let f0 = at_zero + offsets[bj] + dj * FINE_STEP_HZ;
    let mean_symbol: f32 =
        y.iter().map(|z| z.norm_sqr()).sum::<f32>() / y.len().max(1) as f32 * DOWN_SYMBOL as f32;
    Synced {
        start,
        f0,
        sync: c.sync,
        power: e / (3.0 * COSTAS.len() as f32) / mean_symbol.max(f32::MIN_POSITIVE),
    }
}
