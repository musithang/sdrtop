// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Synthetic floors for the floor tests: noise, a tone, a comb of tones, a
//! tone that fades as a sky wave does, and impulses, block by block on the
//! stream's own pair count.

use rustfft::num_complex::Complex;

pub use crate::signal::doppler::testing::{add, noise};

/// A tone at `hz` from the tuned centre, amplitude `amp`, for `n` pairs from
/// pair `start` of a stream at `rate`; `phase` carries across calls.
pub fn tone(
    rate: f64,
    hz: f64,
    amp: f32,
    start: u64,
    n: usize,
    phase: &mut f64,
) -> Vec<Complex<f32>> {
    crate::signal::doppler::testing::carrier(rate, &|_| hz, amp, start, n, phase)
}

/// `teeth` tones every `spacing_hz` from `offset_hz`, each of amplitude
/// `amp`; `phases` holds one phase per tooth.
pub fn comb(
    rate: f64,
    spacing_hz: f64,
    offset_hz: f64,
    amp: f32,
    start: u64,
    n: usize,
    phases: &mut [f64],
) -> Vec<Complex<f32>> {
    let mut out = vec![Complex::new(0.0, 0.0); n];
    for (k, phase) in phases.iter_mut().enumerate() {
        let t = tone(
            rate,
            offset_hz + k as f64 * spacing_hz,
            amp,
            start,
            n,
            phase,
        );
        out = add(out, &t);
    }
    out
}

/// A tone whose complex gain wanders as a Rayleigh-fading sky wave's does:
/// a Gaussian gain, correlated over `coherence_s`, updated every
/// millisecond.
pub struct Fading {
    gain: Complex<f64>,
    phase: f64,
    seed: u64,
    held: usize,
}

impl Fading {
    pub fn new(seed: u64) -> Fading {
        let mut f = Fading {
            gain: Complex::new(0.0, 0.0),
            phase: 0.0,
            seed,
            held: 0,
        };
        f.gain = f.gaussian();
        f
    }

    fn gaussian(&mut self) -> Complex<f64> {
        let g = noise(1, 1.0, &mut self.seed)[0];
        Complex::new(g.re as f64, g.im as f64)
    }
}

pub fn rayleigh_tone(
    rate: f64,
    hz: f64,
    amp: f32,
    coherence_s: f64,
    n: usize,
    state: &mut Fading,
) -> Vec<Complex<f32>> {
    let step = (rate / 1000.0).round() as usize;
    let rho = (-0.001 / coherence_s).exp();
    let tau = std::f64::consts::TAU;
    (0..n)
        .map(|_| {
            if state.held == 0 {
                let w = state.gaussian();
                state.gain = state.gain * rho + w * (1.0 - rho * rho).sqrt();
                state.held = step;
            }
            state.held -= 1;
            state.phase = (state.phase + tau * hz / rate) % tau;
            let x = Complex::from_polar(amp as f64, state.phase) * state.gain;
            Complex::new(x.re as f32, x.im as f32)
        })
        .collect()
}

/// Clicks: about one every `every` pairs, each a single pair of amplitude
/// `height` at a random phase.
pub fn impulses(n: usize, every: usize, height: f32, seed: &mut u64) -> Vec<Complex<f32>> {
    let mut uniform = || {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        (*seed >> 11) as f64 / (1u64 << 53) as f64
    };
    (0..n)
        .map(|_| {
            if uniform() < 1.0 / every as f64 {
                Complex::from_polar(height, (std::f64::consts::TAU * uniform()) as f32)
            } else {
                Complex::new(0.0, 0.0)
            }
        })
        .collect()
}

/// `n` means of `t` exponentials of mean one, each a Gamma(t, 1/t) draw by
/// Marsaglia and Tsang's method: an exposure of `t` frames on pure chance,
/// without generating its samples.
pub fn gamma_means(n: usize, t: u64, seed: &mut u64) -> Vec<f64> {
    let mut uniform = || {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        ((*seed >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    };
    let d = t as f64 - 1.0 / 3.0;
    let c = 1.0 / (9.0 * d).sqrt();
    (0..n)
        .map(|_| loop {
            let (u1, u2) = (uniform(), uniform());
            let z = (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos();
            let v = (1.0 + c * z).powi(3);
            if v <= 0.0 {
                continue;
            }
            if uniform().ln() < 0.5 * z * z + d - d * v + d * v.ln() {
                break d * v / t as f64;
            }
        })
        .collect()
}
