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

/// How finely [`Oversampled`] reads between the samples it is given.
const OVERSAMPLE: usize = 8;
/// The share of the input's Nyquist band the interpolation filter spends on
/// its transition: flat to three quarters of it.
const OVERSAMPLE_ROLLOFF: f64 = 0.25;
/// How far down the interpolation filter puts the images it removes.
const OVERSAMPLE_STOPBAND_DB: f64 = 60.0;

/// Instantaneous frequency at any instant of an IQ block, not only between
/// two of its samples.
///
/// **Why a measurement cannot read between readings.** [`discriminate`]
/// gives one reading per pair of samples. A figure taken at an instant that
/// falls between two readings, such as the centre of a symbol, is not
/// their straight-line blend: a frequency trace is curved, most of all at
/// the peak a deviation is read from, and the chord of a curve runs below
/// it. At four samples a symbol the centre of a GFSK symbol sits exactly
/// between two readings, and that chord read an ideal BLE transmitter's
/// alternating peaks 8.7 % low and its settled ones 3.5 % low.
///
/// **The IQ is interpolated instead, which is exact.** A block that has
/// passed a channel filter is band-limited, so its samples fix the whole
/// waveform between them: [`super::resample::Resampler`] rebuilds it
/// [`OVERSAMPLE`] times more finely, and the discriminator reads that. The
/// readings are then an eighth of a sample apart, where a straight line
/// between two of them is as good as the arithmetic (measured against the
/// exact frequency in this module's tests). Exact for content within three
/// quarters of the input's Nyquist frequency; a caller's channel filter is
/// what keeps it there.
pub struct Oversampled {
    inst: Vec<f32>,
    /// The input-sample instant of `inst[0]`.
    origin: f64,
}

impl Oversampled {
    /// Build from `iq`, sampled at `rate`. Costs one polyphase interpolation
    /// and one discriminator pass over the block, so it is for a block worth
    /// measuring, not for every block that arrives.
    pub fn new(iq: &[Complex<f32>], rate: f64) -> Self {
        let mut up = super::resample::Resampler::new(
            OVERSAMPLE,
            1,
            OVERSAMPLE_ROLLOFF,
            OVERSAMPLE_STOPBAND_DB,
        );
        let delay = up.delay_input_samples();
        let mut fine = Vec::new();
        up.process(iq, &mut fine);
        let mut inst = Vec::new();
        discriminate(&fine, rate * OVERSAMPLE as f64, &mut inst);
        // `fine[k]` stands for input instant `delay + k / OVERSAMPLE`, and a
        // reading sits halfway between the two samples it compares.
        Self {
            inst,
            origin: delay + 0.5 / OVERSAMPLE as f64,
        }
    }

    /// The frequency, in Hz, at instant `t`, in input samples: `t = i` is the
    /// instant of `iq[i]`. Clamped to the readings there are, as
    /// [`super::timing::interpolate`] clamps.
    pub fn at(&self, t: f64) -> f32 {
        super::timing::interpolate(&self.inst, (t - self.origin) * OVERSAMPLE as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An FM tone read at the midpoints between samples, where a straight
    /// line between two plain readings runs under the curve: the oversampled
    /// reading is the exact frequency, the chord is not.
    #[test]
    fn the_oversampled_reading_is_the_exact_frequency() {
        use std::f64::consts::TAU;
        let rate = 4e6;
        // 250 kHz peak deviation, modulated at 500 kHz: a GFSK alternation's
        // fundamental, band-limited well inside the rebuilt band.
        let (peak, fm) = (250e3, 500e3);
        let iq: Vec<Complex<f32>> = (0..4000)
            .map(|i| {
                let t = i as f64 / rate;
                let ph = peak / fm * (TAU * fm * t).sin();
                Complex::new(ph.cos() as f32, ph.sin() as f32)
            })
            .collect();
        let fine = Oversampled::new(&iq, rate);
        let mut plain = Vec::new();
        discriminate(&iq, rate, &mut plain);
        let (mut worst_fine, mut worst_chord) = (0.0f64, 0.0f64);
        for i in 400..3600 {
            // The instant midway between plain readings i and i + 1.
            let t = i as f64 + 1.0;
            let exact = peak * (TAU * fm * t / rate).cos();
            worst_fine = worst_fine.max((fine.at(t) as f64 - exact).abs());
            let chord = super::super::timing::interpolate(&plain, i as f64 + 0.5);
            worst_chord = worst_chord.max((chord as f64 - exact).abs());
        }
        // 182 Hz measured, 0.07 % of the peak: what is left is the tone's own
        // sidebands past Nyquist, which a plain FM tone has and a signal
        // behind a channel filter does not. The chord: 24.9 kHz, 10 %.
        assert!(worst_fine < 250.0, "oversampled worst {worst_fine} Hz");
        assert!(worst_chord > 10_000.0, "the chord was {worst_chord} Hz off");
    }

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
