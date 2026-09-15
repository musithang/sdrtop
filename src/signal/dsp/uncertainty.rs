// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! From a variance to a number a person can read, and the floor physics puts
//! under all of it.
//!
//! Design section 5.3 is the decision this module implements: **every measured
//! value is displayed with an uncertainty, computed and never assumed.** N6
//! produced the variances; this turns one into a `sigma` beside a value, decides
//! how many digits that value is entitled to, and says whether the measurement
//! can resolve the difference the caller cares about.
//!
//! **The convention, stated because a `+/-` that does not state its convention
//! is decoration.** What is carried and displayed is the *standard* uncertainty,
//! one sigma, a coverage of about 68 % for a Gaussian. Not two sigma. A bench
//! instrument's `+/-` is conventionally one sigma, and quietly doubling it would
//! make every reading here look worse than the same reading on an instrument
//! that did not. [`Uncertain::expanded`] is there for a caller that needs the
//! 95 % figure and will label it.
//!
//! **The Cramer-Rao bound is here for two jobs, and the first one is ours.**
//!
//! 1. *As a check on the implementation.* An unbiased estimator cannot have a
//!    variance below the bound. One that appears to has a bug - in the estimator,
//!    in the variance expression, or in the test that measured it - and
//!    `moose_does_not_beat_its_bound` turns a whole class of quiet DSP error
//!    into a failing test rather than a number nobody questioned.
//! 2. *As a statement to the user.* When the uncertainty sits at the bound, the
//!    measurement is as good as physics allows at that SNR and the panel can say
//!    so. When it sits well above, the limit is us, and that is worth knowing
//!    too.

use std::f64::consts::TAU;

/// A measured value and its standard uncertainty, in the same unit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Uncertain {
    value: f64,
    sigma: f64,
}

impl Uncertain {
    /// From an estimate and the variance of the estimator that produced it.
    ///
    /// A negative variance is not a variance and is treated as zero. A variance
    /// that is not a number is an *unknown* uncertainty, which becomes an
    /// infinite one: that is what it means to have measured something and have
    /// no idea how well, and it propagates honestly through everything below
    /// instead of poisoning it with a NaN.
    pub fn from_variance(value: f64, variance: f64) -> Self {
        let sigma = if variance.is_nan() {
            f64::INFINITY
        } else {
            variance.max(0.0).sqrt()
        };
        Self { value, sigma }
    }

    pub fn from_sigma(value: f64, sigma: f64) -> Self {
        Self::from_variance(value, sigma * sigma)
    }

    /// A number that carries no uncertainty of its own: a specification limit, a
    /// channel centre, a count.
    ///
    /// Reaches `main` since B8: `ui::panels::net::bt_rf` reads
    /// `ModulationQuality::delta_f2_max_hz` this way, a maximum of several
    /// noisy readings with no closed-form sigma the way a mean gets one -
    /// see that struct's own doc.
    pub fn exact(value: f64) -> Self {
        Self { value, sigma: 0.0 }
    }

    pub fn value(&self) -> f64 {
        self.value
    }

    /// The standard uncertainty, one sigma. Never negative.
    pub fn sigma(&self) -> f64 {
        self.sigma
    }

    /// The expanded uncertainty, `k` sigma. A caller using this owes the reader
    /// the `k`.
    pub fn expanded(&self, k: f64) -> f64 {
        self.sigma * k.abs()
    }

    /// The same measurement in another unit. Cycles per sample to ppm, volts to
    /// dB of a ratio, anything linear: the uncertainty scales with the value,
    /// which is the whole reason it travels attached to it.
    pub fn scale(&self, k: f64) -> Self {
        Self {
            value: self.value * k,
            sigma: self.sigma * k.abs(),
        }
    }

    /// Shifted by an exactly known offset. The uncertainty is untouched, because
    /// an exact offset adds none.
    pub fn shift(&self, delta: f64) -> Self {
        Self {
            value: self.value + delta,
            sigma: self.sigma,
        }
    }

    /// `self - other`, for two independent measurements.
    ///
    /// Exact, unlike [`Self::ratio`]: for independent `A` and `B`,
    /// `Var(A - B) = Var(A) + Var(B)` is not a linearisation of anything, it
    /// is the definition of variance under a linear combination. B9 is the
    /// reasoned first consumer - a packet's own frequency offset measured
    /// early versus late, the two ends of a drift `signal::ble::measure`
    /// reports as a single number and its own honest uncertainty rather
    /// than two numbers a reader has to subtract by eye.
    pub fn difference(&self, other: &Uncertain) -> Self {
        Self::from_variance(
            self.value - other.value,
            self.sigma * self.sigma + other.sigma * other.sigma,
        )
    }

    /// `self / other`, for two independent measurements, with the uncertainty
    /// propagated by first-order (delta-method) error propagation.
    ///
    /// `scale` and `shift` are exact because multiplying or shifting by a
    /// known constant is linear; a ratio of two *measured* quantities is not,
    /// so this is the first combinator here that is an approximation rather
    /// than an identity. To first order, for independent `A` and `B`:
    ///
    /// ```text
    /// Var(A/B) ~= Var(A) / B^2 + A^2 * Var(B) / B^4
    /// ```
    ///
    /// the same linearisation the Guide to the Expression of Uncertainty in
    /// Measurement uses for any combination of uncorrelated inputs. Exact in
    /// the limit of small relative uncertainty on `other`; B8 is the reasoned
    /// first consumer, a ratio of two deviation measurements whose own
    /// relative uncertainty is a few percent at a workable SNR, well inside
    /// where the linearisation holds.
    ///
    /// `other` at exactly zero has no meaningful ratio and gets an infinite
    /// uncertainty rather than a divide silently producing infinity or NaN -
    /// the same "unknown becomes infinite, not invented" rule
    /// [`Self::from_variance`] already follows for a NaN variance.
    ///
    /// **The value itself carries a small bias this does not correct.**
    /// `E[A/B]` is not exactly `E[A]/E[B]` for a ratio of two random
    /// quantities - Jensen's inequality gives it a second-order pull of
    /// about `(sigma_b / b)^2` relative, which this first-order expansion
    /// does not remove. Negligible next to the reported uncertainty itself
    /// at the relative uncertainties this arc's own measurements run at, and
    /// measured rather than assumed:
    /// `the_ratio_matches_a_monte_carlo_simulation_of_the_same_two_measurements`.
    pub fn ratio(&self, other: &Uncertain) -> Self {
        if other.value == 0.0 {
            return Self::from_variance(f64::INFINITY, f64::INFINITY);
        }
        let value = self.value / other.value;
        let variance = (self.sigma * self.sigma) / (other.value * other.value)
            + (self.value * self.value) * (other.sigma * other.sigma) / other.value.powi(4);
        Self::from_variance(value, variance)
    }

    /// Relative uncertainty, `sigma / |value|`. `None` at a value of zero, where
    /// it is not defined and where its absence is the point: a frequency offset
    /// of zero is a perfectly good measurement, and a relative uncertainty is
    /// simply the wrong question to ask about it.
    // N11 wired in the other three; this one had no honest use in a limit row,
    // where a margin as a fraction of an exact limit is not a quantity anybody
    // wants. **The claim below, that occupancy would need it, was written
    // before occupancy was built and did not hold up**: N14/N15 measure duty
    // cycle, which has no natural "relative to what" question either. Still no
    // consumer.
    #[allow(dead_code)]
    pub fn relative(&self) -> Option<f64> {
        if self.value == 0.0 {
            None
        } else {
            Some(self.sigma / self.value.abs())
        }
    }

    /// Can this measurement resolve a difference of `resolution`?
    ///
    /// **The caller supplies the scale, and that is deliberate.** There is no
    /// universal rule for when a value stops meaning anything: a carrier offset
    /// of zero is an excellent reading, so `value / sigma` says nothing, and the
    /// number that decides is always the difference the user cares about - a
    /// specification limit, a channel spacing, a ppm budget. That knowledge
    /// lives with the protocol, not here.
    pub fn is_resolved(&self, resolution: f64) -> bool {
        self.sigma.is_finite() && self.sigma <= resolution.abs()
    }

    /// How many decimal places the value is entitled to, given its uncertainty.
    ///
    /// **A value printed finer than its uncertainty is a lie told in digits**,
    /// and it is the most common one in instrument software. The rule is the
    /// Guide to the Expression of Uncertainty in Measurement's: the uncertainty
    /// is quoted to two significant figures when its leading digit is 1 or 2 and
    /// to one otherwise, and the value is rounded to that same decimal place.
    /// So `+/-0.43` becomes `+/-0.4` and the value gets one decimal, while
    /// `+/-0.15` keeps both digits and the value gets two.
    ///
    /// Negative results mean rounding to tens or hundreds, which is correct and
    /// which a formatter has to honour: `1234 +/- 30` is `1230 +/- 30`.
    ///
    /// `None` when there is no uncertainty to round against - an exact value, or
    /// one whose uncertainty is unknown or infinite. The caller then has no
    /// guidance from here and must choose for its own reasons.
    pub fn decimals(&self) -> Option<i32> {
        if !self.sigma.is_finite() || self.sigma <= 0.0 {
            return None;
        }
        let decade = self.sigma.log10().floor();
        let mantissa = self.sigma / 10f64.powf(decade);
        // Rounded to two figures *before* the decision, because 0.3 is held as
        // 0.29999999999999998 and dividing it by 0.1 gives 2.9999999999999996.
        // Reading the leading digit off that directly calls it a 2 and awards
        // the value a decimal place it has not earned. The same rounding also
        // absorbs the other direction, where `log10` at an exact power of ten
        // lands a hair low and hands back a mantissa of 10.
        let two_figures = (mantissa * 10.0).round() / 10.0;
        let decade = decade as i32;
        Some(if two_figures < 3.0 {
            1 - decade
        } else {
            -decade
        })
    }
}

/// Cramer-Rao lower bound on the variance of a frequency estimate, in
/// (cycles per sample) squared.
///
/// The classical result for a constant-modulus signal of `samples` samples at a
/// per-sample signal-to-noise power ratio `snr`, with the phase unknown and
/// estimated alongside the frequency:
///
/// ```text
/// var >= 6 / ((2 pi)^2 * snr * m * (m^2 - 1))
/// ```
///
/// Source: Rife and Boorstyn, "Single-Tone Parameter Estimation from
/// Discrete-Time Observations", IEEE Trans. Information Theory 20(5), September
/// 1974. `the_bound_is_the_closed_form_it_claims` pins the arithmetic and
/// `the_bound_falls_with_snr_and_with_the_cube_of_the_length` pins both powers.
///
/// **The constant-modulus assumption is real.** For a preamble whose envelope
/// varies, `m(m^2 - 1)/12` is replaced by the energy-weighted second moment of
/// the sample index, and the bound is looser. Both arcs' preambles are
/// constant-modulus at the point this is applied; a caller for which that stops
/// being true needs the general form, not this one.
///
/// Infinite below two samples, where there is no frequency to estimate, and at
/// or below zero SNR, where there is nothing to estimate it from.
/// No consumer yet. Design section 5.4 wants this beside a frequency
/// measurement - "the Cramer-Rao bound is the floor, and it is displayed" - and
/// N16's reference card does not yet show it: the card states the estimator's
/// own uncertainty but not how close that sits to the physical limit.
#[allow(dead_code)]
pub fn crlb_frequency(snr: f64, samples: usize) -> f64 {
    if snr.is_nan() || snr <= 0.0 || samples < 2 {
        return f64::INFINITY;
    }
    let m = samples as f64;
    6.0 / (TAU * TAU * snr * m * (m * m - 1.0))
}

/// How close an estimator gets to the floor: `bound / variance`, in `(0, 1]`.
///
/// One is an efficient estimator, which for these data models means an optimal
/// one. `10 * log10(1 / efficiency)` is the same statement in dB, and is the
/// form to display: "1.2 dB from the bound" is a sentence a person can act on,
/// where "efficiency 0.75" is a sentence they have to convert first.
///
/// Above one is impossible for an unbiased estimator, so a caller seeing it has
/// found a bug rather than a good day.
/// No consumer yet; see [`crlb_frequency`].
#[allow(dead_code)]
pub fn efficiency(variance: f64, bound: f64) -> f64 {
    if variance <= 0.0 || !variance.is_finite() || !bound.is_finite() {
        return 0.0;
    }
    bound / variance
}

/// The sample mean of `x`, with its own Cramer-Rao-achieving uncertainty.
///
/// **A different bound from [`crlb_frequency`], for a different problem, not
/// a substitute for it.** That bound is Moose's: a frequency read from the
/// phase ramp of a repeated complex sequence, and its `M(M^2-1)` denominator
/// is specific to a phase estimator's geometry. This is the elementary case
/// instead, a constant buried in additive noise and estimated by averaging
/// real-valued samples directly, and for i.i.d. Gaussian noise the sample
/// mean is itself the minimum-variance unbiased estimator, achieving its own
/// bound exactly: `Var(mean) = sigma^2 / N`, with `sigma^2` the sample
/// variance around that mean. B7 is the reasoned first consumer: a GFSK
/// discriminator's output over a run of whitened (so, on average, balanced)
/// data has this exact shape, a constant carrier-frequency offset sitting in
/// noise, and no repeated sequence a phase-ramp method could use instead.
///
/// `Uncertain::exact(0.0)` for fewer than two samples, where there is no
/// variance to estimate from - not zero uncertainty, which `is_resolved`
/// would then read as an unusually good measurement instead of a missing one.
pub fn mean_with_uncertainty(x: &[f32]) -> Uncertain {
    let n = x.len();
    if n < 2 {
        return Uncertain::from_variance(x.first().copied().unwrap_or(0.0) as f64, f64::INFINITY);
    }
    let mean = x.iter().map(|&v| v as f64).sum::<f64>() / n as f64;
    let sample_variance =
        x.iter().map(|&v| (v as f64 - mean).powi(2)).sum::<f64>() / (n - 1) as f64;
    Uncertain::from_variance(mean, sample_variance / n as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::dsp::correlate::DelayedAutocorrelator;
    use crate::signal::dsp::estimate::{moose_offset, moose_variance};
    use crate::signal::dsp::nco::Nco;
    use crate::signal::dsp::testkit::Rng;

    #[test]
    fn the_bound_is_the_closed_form_it_claims() {
        // Worked independently: m = 128, snr = 10, so m(m^2 - 1) = 128 * 16383
        // = 2_097_024, and 6 / (39.478417... * 10 * 2_097_024).
        let got = crlb_frequency(10.0, 128);
        let want = 6.0 / (39.47841760435743 * 10.0 * 2_097_024.0);
        assert!(
            (got / want - 1.0).abs() < 1e-12,
            "bound {got:e} against {want:e}"
        );
    }

    #[test]
    fn the_bound_falls_with_snr_and_with_the_cube_of_the_length() {
        // Exactly one over SNR.
        let a = crlb_frequency(1.0, 256);
        let b = crlb_frequency(2.0, 256);
        assert!((a / b - 2.0).abs() < 1e-12, "{a:e} against {b:e}");

        // Asymptotically one over the cube of the length. At 256 samples the
        // exact m(m^2 - 1) differs from m^3 by 15 parts per million, so the
        // measured ratio has to land on eight, not near it.
        let c = crlb_frequency(1.0, 512);
        let ratio = a / c;
        assert!(
            (ratio - 8.0).abs() < 1e-3,
            "doubling the length changed the bound by {ratio}, not 8"
        );
    }

    #[test]
    fn there_is_no_bound_without_a_signal_or_a_second_sample() {
        assert!(crlb_frequency(0.0, 128).is_infinite());
        assert!(crlb_frequency(-1.0, 128).is_infinite());
        assert!(crlb_frequency(f64::NAN, 128).is_infinite());
        assert!(crlb_frequency(10.0, 1).is_infinite());
        assert_eq!(efficiency(1e-9, f64::INFINITY), 0.0);
    }

    /// **N7's exit condition, and the reason this step is separate.** The
    /// estimator's measured spread must sit above the floor physics puts under
    /// it, and within a stated distance of it. Beating the bound is not a good
    /// result, it is a bug.
    ///
    /// A shorter preamble than N6 uses and fewer trials, deliberately: this
    /// needs the ratio, not the third decimal of the variance, and suite time is
    /// bought back by shortening the signal rather than by asking fewer
    /// questions of it.
    #[test]
    fn moose_does_not_beat_its_bound() {
        const D: usize = 32;
        const TRIALS: usize = 2000;
        let truth = 0.003;
        for snr_db in [10.0f64, 20.0] {
            let snr = 10f64.powf(snr_db / 10.0);
            let mut rng = Rng::new(0xB0_1D + snr_db as u64);
            let est: Vec<f64> = (0..TRIALS)
                .map(|_| {
                    let half = rng.qpsk(D);
                    let mut x: Vec<_> = half.iter().chain(half.iter()).copied().collect();
                    Nco::new(truth, 1.0).mix(&mut x);
                    let n = rng.noise(x.len(), 1.0 / snr);
                    let mut c = DelayedAutocorrelator::new(D, D);
                    let mut last = None;
                    for (s, z) in x.iter().zip(n.iter()) {
                        if let Some(r) = c.push(s + z) {
                            last = Some(r);
                        }
                    }
                    moose_offset(last.unwrap().p, D)
                })
                .collect();

            let mean = est.iter().sum::<f64>() / TRIALS as f64;
            let measured =
                est.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (TRIALS - 1) as f64;
            let bound = crlb_frequency(snr, 2 * D);

            assert!(
                measured > bound,
                "{snr_db} dB: measured variance {measured:e} is below the bound {bound:e}"
            );
            // Moose's own expression is 1 / (4 pi^2 D^3 snr) and the bound is
            // 6 / (4 pi^2 snr * 2D(4D^2 - 1)), so the ratio is 4/3 exactly in
            // the limit. Anything much above it means the estimator is leaving
            // accuracy on the table; anything below means it cannot be unbiased.
            let formula = moose_variance(snr, D, D);
            assert!(
                (formula / bound - 4.0 / 3.0).abs() < 0.01,
                "{snr_db} dB: the closed forms are {:.4} apart, not 4/3",
                formula / bound
            );
            let ratio = measured / bound;
            assert!(
                ratio < 1.7,
                "{snr_db} dB: measured variance is {ratio:.3} times the bound"
            );
        }
    }

    #[test]
    fn the_digits_a_value_is_entitled_to_come_from_its_uncertainty() {
        // The Guide's rule: two significant figures on the uncertainty when it
        // leads with 1 or 2, one otherwise, and the value rounded to match.
        for (sigma, want) in [
            (0.43, 1),
            (0.15, 2),
            (0.24, 2),
            (0.3, 1),
            (1.1, 1),
            (7.0, 0),
            (30.0, -1),
            (120.0, -1),
            (0.0004, 4),
            // The cases where the decimal the reader sees and the binary the
            // computer holds disagree.
            (0.29, 2),
            (0.3, 1),
            (100.0, -1),
            (1000.0, -2),
            // Leading digit 1, so two figures: +/-0.0010 needs four decimals.
            (0.001, 4),
        ] {
            let u = Uncertain::from_sigma(1.0, sigma);
            assert_eq!(u.decimals(), Some(want), "sigma {sigma}");
        }
        // The design's own worked example: -3.2 +/- 0.4 ppm is one decimal.
        assert_eq!(Uncertain::from_sigma(-3.2, 0.4).decimals(), Some(1));
    }

    #[test]
    fn a_value_with_nothing_to_round_against_gives_no_guidance() {
        assert_eq!(Uncertain::exact(5.0).decimals(), None);
        assert_eq!(Uncertain::from_variance(5.0, f64::NAN).decimals(), None);
        assert_eq!(
            Uncertain::from_variance(5.0, f64::INFINITY).decimals(),
            None
        );
    }

    #[test]
    fn an_unknown_variance_becomes_an_infinite_uncertainty() {
        let u = Uncertain::from_variance(1.0, f64::NAN);
        assert!(u.sigma().is_infinite());
        assert!(!u.is_resolved(1e9));
        // A negative variance is not a variance.
        assert_eq!(Uncertain::from_variance(1.0, -4.0).sigma(), 0.0);
    }

    #[test]
    fn changing_the_unit_carries_the_uncertainty_with_it() {
        // A frequency offset in cycles per sample, at 20 Msps, on a 2.4 GHz
        // carrier, expressed in ppm: two exact scalings in a row.
        let offset = Uncertain::from_variance(1e-5, 4e-12);
        let hz = offset.scale(20e6);
        assert!((hz.value() - 200.0).abs() < 1e-9);
        assert!((hz.sigma() - 2e-6 * 20e6).abs() < 1e-9);
        let ppm = hz.scale(1e6 / 2.4e9);
        assert!((ppm.value() - 200.0 / 2400.0).abs() < 1e-12);
        // The relative uncertainty is what survives a change of unit unchanged.
        assert!((ppm.relative().unwrap() - hz.relative().unwrap()).abs() < 1e-12);
    }

    #[test]
    fn resolution_is_the_callers_question_not_this_modules() {
        let u = Uncertain::from_sigma(0.0, 0.4);
        // A reading of exactly zero is a perfectly good measurement, so nothing
        // here may judge it by value over sigma.
        assert!(u.relative().is_none());
        assert!(u.is_resolved(0.5));
        assert!(!u.is_resolved(0.3));
        assert!(Uncertain::exact(0.0).is_resolved(0.0));
    }

    #[test]
    fn an_exact_offset_adds_no_uncertainty() {
        let u = Uncertain::from_sigma(10.0, 0.5).shift(-3.0);
        assert_eq!(u.value(), 7.0);
        assert_eq!(u.sigma(), 0.5);
        assert_eq!(Uncertain::from_sigma(1.0, 0.5).expanded(2.0), 1.0);
    }

    /// A known constant buried in noise is recovered close to its true value,
    /// within the uncertainty this function reports for itself - the basic
    /// claim any estimator's own error bar has to make good on.
    #[test]
    fn the_mean_recovers_a_known_constant_within_its_own_uncertainty() {
        let mut rng = Rng::new(1);
        let true_value = 37_000.0f64;
        let sigma = 5_000.0f64;
        let samples: Vec<f32> = (0..2000)
            .map(|_| (true_value + rng.normal_pair().0 * sigma) as f32)
            .collect();
        let u = mean_with_uncertainty(&samples);
        assert!(
            (u.value() - true_value).abs() < 4.0 * u.sigma(),
            "value {} sigma {} true {}",
            u.value(),
            u.sigma(),
            true_value
        );
    }

    /// This is what "the uncertainty tracks SNR" means for a mean estimator:
    /// twice as many samples of the same noisy signal, or half the noise on
    /// the same number of samples, both halve the variance the classical
    /// `sigma^2 / N` way - `1/sqrt(2)` on `sigma`, not on the variance itself.
    #[test]
    fn the_uncertainty_shrinks_with_more_samples_and_with_less_noise() {
        let mut rng = Rng::new(2);
        let noisy = |n: usize, sigma: f64, rng: &mut Rng| -> Vec<f32> {
            (0..n)
                .map(|_| (rng.normal_pair().0 * sigma) as f32)
                .collect()
        };

        let base = mean_with_uncertainty(&noisy(1000, 1.0, &mut rng));
        let more_samples = mean_with_uncertainty(&noisy(4000, 1.0, &mut rng));
        let less_noise = mean_with_uncertainty(&noisy(1000, 0.5, &mut rng));

        // Four times the samples: half the sigma.
        assert!(
            (more_samples.sigma() / base.sigma() - 0.5).abs() < 0.1,
            "base {} more_samples {}",
            base.sigma(),
            more_samples.sigma()
        );
        // Half the per-sample noise: half the sigma too.
        assert!(
            (less_noise.sigma() / base.sigma() - 0.5).abs() < 0.1,
            "base {} less_noise {}",
            base.sigma(),
            less_noise.sigma()
        );
    }

    /// `Var(A - B) = Var(A) + Var(B)`, checked against a Monte Carlo
    /// simulation of the same two independent measurements - a formula this
    /// simple is still worth measuring rather than trusting, the same
    /// discipline `moose_does_not_beat_its_bound` and the ratio's own test
    /// below hold their closed forms to.
    #[test]
    fn the_difference_matches_a_monte_carlo_simulation_of_the_same_two_measurements() {
        let a = Uncertain::from_sigma(10.0, 0.6);
        let b = Uncertain::from_sigma(4.0, 0.3);
        let z = a.difference(&b);
        assert!((z.value() - 6.0).abs() < 1e-12);
        assert!((z.sigma() - (0.6f64.powi(2) + 0.3f64.powi(2)).sqrt()).abs() < 1e-12);

        let mut rng = Rng::new(43);
        const TRIALS: usize = 200_000;
        let samples: Vec<f64> = (0..TRIALS)
            .map(|_| {
                let da = rng.normal_pair().0 * a.sigma();
                let db = rng.normal_pair().0 * b.sigma();
                (a.value() + da) - (b.value() + db)
            })
            .collect();
        let mean = samples.iter().sum::<f64>() / TRIALS as f64;
        let variance =
            samples.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (TRIALS - 1) as f64;
        assert!((mean - z.value()).abs() < 0.01, "simulated mean {mean}");
        assert!(
            (variance.sqrt() / z.sigma() - 1.0).abs() < 0.02,
            "simulated sigma {} against {}",
            variance.sqrt(),
            z.sigma()
        );
    }

    /// The delta-method formula, checked against a Monte Carlo simulation of
    /// the same two independent measurements - the same discipline
    /// `moose_does_not_beat_its_bound` holds a closed form to, rather than
    /// trusting the algebra on its own.
    #[test]
    fn the_ratio_matches_a_monte_carlo_simulation_of_the_same_two_measurements() {
        let a = Uncertain::from_sigma(10.0, 0.6);
        let b = Uncertain::from_sigma(4.0, 0.3);
        let z = a.ratio(&b);
        assert!((z.value() - 2.5).abs() < 1e-12);

        let mut rng = Rng::new(42);
        const TRIALS: usize = 200_000;
        let samples: Vec<f64> = (0..TRIALS)
            .map(|_| {
                let da = rng.normal_pair().0 * a.sigma();
                let db = rng.normal_pair().0 * b.sigma();
                (a.value() + da) / (b.value() + db)
            })
            .collect();
        let mean = samples.iter().sum::<f64>() / TRIALS as f64;
        let variance =
            samples.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (TRIALS - 1) as f64;
        // Not the tight match variance gets: E[A/B] carries its own small
        // second-order (Jensen's inequality) bias from B's own spread, which
        // a first-order expansion around the mean does not capture - real
        // and expected, at about `(sigma_b / b)^2` relative here, not a
        // defect in the variance formula this test actually exists to check.
        assert!(
            (mean - z.value()).abs() < 0.02,
            "simulated mean {mean} against the ratio's own {}",
            z.value()
        );
        assert!(
            (variance.sqrt() / z.sigma() - 1.0).abs() < 0.03,
            "simulated sigma {} against the delta-method {}",
            variance.sqrt(),
            z.sigma()
        );
    }

    /// A zero denominator has no ratio, and says so with an infinite
    /// uncertainty rather than the divide's own infinity or NaN.
    #[test]
    fn a_zero_denominator_gives_an_unknown_ratio_not_an_invented_one() {
        let a = Uncertain::from_sigma(10.0, 0.6);
        let zero = Uncertain::from_sigma(0.0, 0.3);
        assert!(a.ratio(&zero).sigma().is_infinite());
    }

    /// Fewer than two samples has no variance to estimate from, and says so
    /// with an infinite uncertainty rather than the zero a missing variance
    /// would otherwise default to - the same reasoning `is_resolved`'s own
    /// tests hold every other estimator in this module to.
    #[test]
    fn fewer_than_two_samples_is_infinitely_uncertain_not_perfectly_known() {
        assert_eq!(mean_with_uncertainty(&[]).sigma(), f64::INFINITY);
        assert_eq!(mean_with_uncertainty(&[3.0]).sigma(), f64::INFINITY);
    }
}
