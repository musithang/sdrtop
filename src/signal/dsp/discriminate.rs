// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The quadrature discriminator: instantaneous frequency between two IQ
//! samples, with no opinion about what to do when one of them is
//! untrustworthy.
//!
//! Generalised the way N1 generalised `signal::demod`'s streaming decimator
//! into [`super::fir`]: the arithmetic with a closed-form answer moved here,
//! and `signal::demod::fm_discriminate` keeps the envelope-gate-and-hold
//! policy it has always had, because that policy exists to protect a WFM
//! pilot on a channel assumed to carry a continuous carrier - a decision that
//! belongs with the demodulator that has a reason to want it, not here. BLE
//! has neither a pilot nor a continuous carrier to protect, so B2 calls
//! [`discriminate`] directly with no gate.

use num_complex::Complex;

/// Instantaneous frequency in Hz between two consecutive samples.
///
/// `f = arg(b * conj(a)) * rate / 2π`, unambiguous to ±`rate`/2.
pub fn instantaneous_freq_hz(a: Complex<f32>, b: Complex<f32>, rate: f64) -> f32 {
    use std::f64::consts::PI;
    let prod = b * a.conj();
    let scale = (rate / (2.0 * PI)) as f32;
    prod.im.atan2(prod.re) * scale
}

/// The whole block, consecutive pairs. `len - 1` outputs: the first sample of
/// the block has no predecessor to discriminate against.
///
/// No production consumer yet: `signal::demod::fm_discriminate` calls
/// [`instantaneous_freq_hz`] directly rather than this, because it needs the
/// envelope gate applied between the two calls, not around the whole block.
/// B2's own tests are the only caller until a step in this arc needs the
/// gate-free block form.
#[allow(dead_code)]
pub fn discriminate(iq: &[Complex<f32>], rate: f64, out: &mut Vec<f32>) {
    out.clear();
    if iq.len() < 2 {
        return;
    }
    out.reserve(iq.len() - 1);
    for w in iq.windows(2) {
        out.push(instantaneous_freq_hz(w[0], w[1], rate));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tone offset from DC by a constant frequency reads back as exactly
    /// that offset, everywhere except the ambiguity limit.
    #[test]
    fn a_constant_offset_reads_back_as_that_offset() {
        use std::f64::consts::TAU;
        let rate = 1_000_000.0;
        let offset = 50_000.0;
        let n = 200;
        let step = TAU * offset / rate;
        let mut phase = 0.0f64;
        let iq: Vec<_> = (0..n)
            .map(|_| {
                let z = Complex::new(phase.cos() as f32, phase.sin() as f32);
                phase += step;
                z
            })
            .collect();
        let mut out = Vec::new();
        discriminate(&iq, rate, &mut out);
        assert_eq!(out.len(), n - 1);
        for &f in &out {
            assert!((f as f64 - offset).abs() < 1.0, "got {f}, want {offset}");
        }
    }

    /// Two samples in, one frequency out - the definition, pinned so nobody
    /// "fixes" the off-by-one later.
    #[test]
    fn two_samples_yield_one_frequency() {
        let a = Complex::new(1.0, 0.0);
        let b = Complex::new(0.0, 1.0);
        let mut out = Vec::new();
        discriminate(&[a, b], 1_000_000.0, &mut out);
        assert_eq!(out.len(), 1);
    }

    /// Fewer than two samples has no pair to discriminate, and must say so by
    /// producing nothing rather than by panicking on an empty window.
    #[test]
    fn fewer_than_two_samples_yields_nothing() {
        let mut out = Vec::new();
        discriminate(&[], 1_000_000.0, &mut out);
        assert!(out.is_empty());
        discriminate(&[Complex::new(1.0, 0.0)], 1_000_000.0, &mut out);
        assert!(out.is_empty());
    }

    /// A quarter turn per sample is exactly a quarter of the sample rate - the
    /// single hand-checkable point on the curve, independent of the rate
    /// scaling `a_constant_offset_reads_back_as_that_offset` exercises.
    #[test]
    fn a_quarter_turn_per_sample_is_a_quarter_of_the_rate() {
        let rate = 8_000.0;
        let f = instantaneous_freq_hz(Complex::new(1.0, 0.0), Complex::new(0.0, 1.0), rate);
        assert!((f - 2_000.0).abs() < 1e-3, "got {f}");
    }
}
