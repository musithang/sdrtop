// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Synthetic carriers for the doppler tests: a tone whose frequency follows
//! any curve, keying, and Gaussian noise from a fixed seed, generated block by
//! block on the stream's own pair count so a test can push them as a radio
//! would.

use rustfft::num_complex::Complex;

/// A carrier at `offset_hz(t)` from the tuned centre, amplitude `amp`, for
/// `n` pairs starting at pair `start` of a stream at `rate`. The phase is the
/// integral of the frequency, carried in `phase` across calls so blocks join.
pub fn carrier(
    rate: f64,
    offset_hz: &dyn Fn(f64) -> f64,
    amp: f32,
    start: u64,
    n: usize,
    phase: &mut f64,
) -> Vec<Complex<f32>> {
    let tau = std::f64::consts::TAU;
    (0..n)
        .map(|i| {
            let t = (start + i as u64) as f64 / rate;
            *phase = (*phase + tau * offset_hz(t) / rate) % tau;
            Complex::from_polar(amp, *phase as f32)
        })
        .collect()
}

/// The same, keyed off for the first 100 ms of every second, as a time
/// signal marks its seconds.
pub fn keyed(
    rate: f64,
    offset_hz: &dyn Fn(f64) -> f64,
    amp: f32,
    start: u64,
    n: usize,
    phase: &mut f64,
) -> Vec<Complex<f32>> {
    let mut out = carrier(rate, offset_hz, amp, start, n, phase);
    for (i, x) in out.iter_mut().enumerate() {
        let t = (start + i as u64) as f64 / rate;
        if t.fract() < 0.1 {
            *x = Complex::new(0.0, 0.0);
        }
    }
    out
}

/// Complex Gaussian noise of total power `sigma²` per sample, from a
/// xorshift seeded by `seed` and advanced in place.
pub fn noise(n: usize, sigma: f32, seed: &mut u64) -> Vec<Complex<f32>> {
    let mut uniform = || {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        ((*seed >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    };
    let s = sigma as f64 / std::f64::consts::SQRT_2;
    (0..n)
        .map(|_| {
            let (u1, u2) = (uniform(), uniform());
            let r = (-2.0 * u1.ln()).sqrt() * s;
            let a = std::f64::consts::TAU * u2;
            Complex::new((r * a.cos()) as f32, (r * a.sin()) as f32)
        })
        .collect()
}

/// `a + b`, sample by sample.
pub fn add(mut a: Vec<Complex<f32>>, b: &[Complex<f32>]) -> Vec<Complex<f32>> {
    for (x, y) in a.iter_mut().zip(b) {
        *x += *y;
    }
    a
}

/// The HF+'s wire format, for feeding a worker: interleaved little-endian
/// `i32`, `full_scale` counts to one.
pub fn int32_bytes(samples: &[Complex<f32>], full_scale: f32) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * 8);
    for s in samples {
        out.extend_from_slice(&((s.re * full_scale) as i32).to_le_bytes());
        out.extend_from_slice(&((s.im * full_scale) as i32).to_le_bytes());
    }
    out
}

/// The geometry [`int32_bytes`] writes.
pub fn int32_geometry() -> crate::hardware::SampleGeometry {
    crate::hardware::SampleGeometry {
        format: crate::hardware::SampleFormat::Int32,
        full_scale: 8_388_608.0,
        converter_bits: Some(16),
    }
}
