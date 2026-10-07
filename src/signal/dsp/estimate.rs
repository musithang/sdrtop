// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Estimators, each with the variance that turns its answer into a measurement.
//!
//! **A number without a variance is not a measurement, it is a readout.** Every
//! displayed value carries an uncertainty, and an uncertainty has to come from
//! somewhere: the estimator's own variance, at the SNR that was actually
//! measured and the sample count that was actually used. So the variance
//! arrives in the same step as the estimator it belongs to, never as a later
//! refinement, because an estimator that has shipped without one has already
//! been displayed without one.
//!
//! **On where the formulas come from.** The estimators are named and cited: they
//! are Moose's and Schmidl and Cox's, and being named is what gives them a
//! literature and a known behaviour. The variance expressions here are *derived*
//! from those estimators' own data model rather than quoted, and they say so.
//! That is the more honest label and, in this project, the stronger one: a
//! quoted constant is testimony about a paper, while these are checked against
//! a Monte Carlo run over thousands of noise realisations, which is testimony
//! about the code that will actually run.
//!
//! The chain this module completes, and it closes on itself:
//!
//! 1. The correlator finds a burst and reports a coherence.
//! 2. The coherence gives the SNR, because two noisy copies of one signal agree
//!    exactly as well as their SNR allows and no better.
//! 3. The SNR gives the variance of every estimate taken from that burst.
//! 4. The variance becomes the uncertainty printed beside the value
//!    (`dsp::uncertainty`).
//!
//! Nothing in that chain needs a number the radio did not supply.

use num_complex::Complex;
use std::f64::consts::TAU;

// ---------------------------------------------------------------------------
// Frequency offset
// ---------------------------------------------------------------------------

/// Moose's frequency offset estimate, in cycles per sample.
///
/// Source: Moose, "A Technique for Orthogonal Frequency Division Multiplexing
/// Frequency Offset Correction", IEEE Trans. Communications 42(10), October
/// 1994, pp. 2908-2914.
///
/// `p` is the delayed autocorrelation from [`super::correlate`]: the signal
/// against itself `lag` samples ago. Where the signal repeats with period `lag`,
/// every product in that sum has the same phase, and that phase is exactly what
/// the offset accumulated in `lag` samples. Dividing it out is the whole
/// estimator. It is the maximum-likelihood estimate for this data model, which
/// is why nothing cleverer is called for.
///
/// **Its range is `+/- 1/(2 * lag)` and outside that it wraps silently**, giving
/// a wrong answer that looks exactly like a right one. See `moose_range`; the
/// caller is responsible for arranging that the offset it is looking for fits.
pub fn moose_offset(p: Complex<f64>, lag: usize) -> f64 {
    p.arg() / (TAU * lag.max(1) as f64)
}

/// The largest offset [`moose_offset`] can report without ambiguity, in cycles
/// per sample. The phase of one correlation cannot distinguish an angle from the
/// same angle plus a full turn, so a longer lag buys precision and pays for it
/// with range, one for one.
// **No consumer yet, and the one candidate reimplemented it instead.**
// `signal::reference::carrier_offset_hz` derives its own lag from a
// caller-supplied search range rather than composing this function's inverse,
// because it needed the range-to-lag direction and this is lag-to-range. A
// timing detector working the other way round - given a lag, what range does
// it buy - is what a Wi-Fi burst and symbol-timing detector would need.
#[cfg(test)]
pub fn moose_range(lag: usize) -> f64 {
    0.5 / lag.max(1) as f64
}

/// Variance of [`moose_offset`], in (cycles per sample) squared.
///
/// **Derived, not quoted.** For `pairs` products of a repeat at lag `lag`, in
/// noise of per-sample signal-to-noise ratio `snr`, the phase of the correlation
/// is perturbed by the component of the noise perpendicular to it. Both copies
/// carry independent noise and each contributes half, so the phase variance is
/// `1 / (pairs * snr)`, and dividing the phase by `2 * pi * lag` divides the
/// variance by its square:
///
/// ```text
/// var = 1 / (4 * pi^2 * lag^2 * pairs * snr)
/// ```
///
/// It is the high-SNR form: the noise-times-noise term is dropped, so it is
/// optimistic where the SNR is poor, and `the_frequency_variance_matches_the_
/// closed_form` states by how much at each SNR it tests.
///
/// Infinite for a signal-to-noise ratio of zero or less, which is the correct
/// answer rather than an evasion: with no signal there is no information, and an
/// infinite variance is exactly how an uncertainty says so. An infinite SNR is
/// allowed through and gives a variance of zero, because noiseless data is a
/// fixture rather than a radio and a fixture should read exactly.
pub fn moose_variance(snr: f64, pairs: usize, lag: usize) -> f64 {
    if snr.is_nan() || snr <= 0.0 || pairs == 0 || lag == 0 {
        return f64::INFINITY;
    }
    1.0 / (TAU * TAU * (lag * lag) as f64 * pairs as f64 * snr)
}

// ---------------------------------------------------------------------------
// Coherence, its bias, and the SNR it hides
// ---------------------------------------------------------------------------

/// Squared coherence with the noise floor taken out of it. **The naive metric
/// is biased upward and the bias is a closed form**, and a bias with a closed
/// form is removed rather than merely stated. Two noisy copies of one signal
/// correlate with coefficient `rho = snr / (1 + snr)`, so the metric should
/// read `rho^2`; but the correlation of the noise with itself adds a floor, and
/// to first order
///
/// ```text
/// E[M] = rho^2 + (1 - rho^2) / pairs
/// ```
///
/// which inverts exactly. At 64 pairs and 0 dB the raw metric reads 4.8 % high
/// and the SNR taken from it 0.2 dB high, which is the direction that flatters
/// the instrument, so removing it is not optional.
///
/// `the_coherence_bias_is_real_and_the_correction_removes_it` measures both at
/// 64 and 256 pairs: the raw bias is ten standard errors clear of the truth, and
/// what the correction leaves is under one. No claim is made here about the
/// order of that remainder. Resolving it would take some forty thousand trials
/// rather than four, and the test that tried to state it was reading its own
/// noise until the standard error was computed alongside it.
pub fn coherence_squared(metric: f64, pairs: usize) -> f64 {
    if pairs < 2 {
        return metric.clamp(0.0, 1.0);
    }
    let n = pairs as f64;
    ((n * metric - 1.0) / (n - 1.0)).clamp(0.0, 1.0)
}

/// Signal-to-noise ratio, as a power ratio, from a delayed-autocorrelation
/// metric.
///
/// **Derived from the model rather than quoted.** Write one copy as `s + n1` and
/// the other as `s + n2`. Their correlation is the signal power alone, while
/// each one's energy is signal plus noise, so the correlation coefficient is
/// `rho = snr / (1 + snr)` and inverting gives `snr = rho / (1 - rho)`. The
/// burst detector therefore measures the SNR as a by-product of detecting the
/// burst, and nothing extra has to be estimated to put an uncertainty on
/// everything else taken from that burst.
///
/// `None` at a coherence of one, where the expression divides by zero. That is
/// noiseless data, which happens in a test fixture and not on a radio, and
/// inventing a very large number for it would be inventing a measurement.
pub fn snr_from_metric(metric: f64, pairs: usize) -> Option<f64> {
    let rho = coherence_squared(metric, pairs).sqrt();
    if rho >= 1.0 {
        return None;
    }
    Some(rho / (1.0 - rho))
}

// ---------------------------------------------------------------------------
// Coarse timing
// ---------------------------------------------------------------------------

/// Where a repeat was found: the run of positions whose metric is within a
/// stated fraction of the peak.
///
/// No consumer yet: Wi-Fi symbol timing from the L-LTF cross-correlation
/// would be one.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg(test)]
pub struct Plateau {
    /// First index in the run.
    pub start: usize,
    /// Last index in the run, inclusive.
    pub end: usize,
    /// Index of the largest metric in it.
    pub peak: usize,
    pub peak_metric: f64,
}

/// Schmidl and Cox's coarse timing, as a plateau rather than as a point.
///
/// Source: Schmidl and Cox, "Robust Frequency and Timing Synchronization for
/// OFDM", IEEE Trans. Communications 45(12), December 1997. The timing metric of
/// a two-halves preamble is flat for the whole guard interval, because the
/// cyclic prefix repeats too, and the paper takes the timing from the region
/// within 90 % of the maximum for exactly that reason.
///
/// **This returns the geometry and not a decision.** Which end of the plateau is
/// the symbol boundary depends on the guard interval, and the guard interval is
/// protocol knowledge; `signal::dsp` knows no protocol. Handing back a centre
/// as though it were the answer would bake in a bias of half a guard interval
/// and hide it inside a module that cannot even name the unit it is wrong by.
///
/// `None` for an empty input. `fraction` is the 0.9 of the paper unless the
/// caller has a reason.
/// No consumer yet; see [`Plateau`].
#[cfg(test)]
pub fn schmidl_cox_plateau(metric: &[f64], fraction: f64) -> Option<Plateau> {
    let (peak, &peak_metric) = metric
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))?;
    let floor = peak_metric * fraction.clamp(0.0, 1.0);
    let mut start = peak;
    while start > 0 && metric[start - 1] >= floor {
        start -= 1;
    }
    let mut end = peak;
    while end + 1 < metric.len() && metric[end + 1] >= floor {
        end += 1;
    }
    Some(Plateau {
        start,
        end,
        peak,
        peak_metric,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::dsp::correlate::DelayedAutocorrelator;
    use crate::signal::dsp::nco::Nco;
    use crate::signal::dsp::testkit::Rng;

    const D: usize = 64;

    /// One trial: a repeated preamble of `d` samples per half, at a known offset
    /// and a known SNR, through the real correlator, giving the reading every
    /// estimator here reads from.
    fn trial(d: usize, offset: f64, snr_db: f64, rng: &mut Rng) -> (Complex<f64>, f64) {
        let half = rng.qpsk(d);
        let mut x: Vec<_> = half.iter().chain(half.iter()).copied().collect();
        // The offset is applied to the whole preamble, which is what a radio
        // tuned a little off does to it.
        Nco::new(offset, 1.0).mix(&mut x);
        let noise_power = 1.0 / 10f64.powf(snr_db / 10.0);
        let n = rng.noise(x.len(), noise_power);
        let mut c = DelayedAutocorrelator::new(d, d);
        let mut last = None;
        for (s, z) in x.iter().zip(n.iter()) {
            if let Some(r) = c.push(s + z) {
                last = Some(r);
            }
        }
        let r = last.expect("two halves must produce one reading");
        (r.p, r.metric().unwrap())
    }

    fn mean(v: &[f64]) -> f64 {
        v.iter().sum::<f64>() / v.len() as f64
    }

    fn variance(v: &[f64]) -> f64 {
        let m = mean(v);
        v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (v.len() - 1) as f64
    }

    /// The reason this module exists: the estimate is
    /// centred on the truth, and its spread is the spread the closed form says
    /// it should have, at three signal-to-noise ratios.
    #[test]
    fn the_frequency_estimate_is_unbiased_and_as_tight_as_the_formula_says() {
        const TRIALS: usize = 4000;
        let truth = 0.002; // well inside the range of 1/128
        for (snr_db, tolerance) in [(20.0, 1.10), (10.0, 1.15), (0.0, 1.60)] {
            let snr = 10f64.powf(snr_db / 10.0);
            let mut rng = Rng::new(0x5EED + snr_db as u64);
            let est: Vec<f64> = (0..TRIALS)
                .map(|_| moose_offset(trial(D, truth, snr_db, &mut rng).0, D))
                .collect();

            let want_var = moose_variance(snr, D, D);
            let got_var = variance(&est);
            let bias = mean(&est) - truth;
            // The mean of TRIALS draws has this standard error, so a bias
            // smaller than a few of them is not a bias, it is the test's own
            // resolution. Three sigma, stated rather than eyeballed.
            let sem = (got_var / TRIALS as f64).sqrt();
            assert!(
                bias.abs() < 3.0 * sem,
                "{snr_db} dB: bias {bias:e} against a standard error of {sem:e}"
            );
            let ratio = got_var / want_var;
            assert!(
                ratio > 1.0 / tolerance && ratio < tolerance,
                "{snr_db} dB: measured variance {got_var:e} is {ratio:.3} times the formula's {want_var:e}"
            );
        }
    }

    /// The failure mode, demonstrated rather than described. Outside the range
    /// the estimate does not degrade, it wraps: it comes back confident and
    /// wrong, and a caller that has not arranged for the offset to fit gets no
    /// warning from this function.
    #[test]
    fn outside_the_acquisition_range_the_estimate_wraps() {
        let range = moose_range(D);
        assert!((range - 1.0 / 128.0).abs() < 1e-15);

        let mut rng = Rng::new(99);
        let inside = moose_offset(trial(D, range * 0.9, 30.0, &mut rng).0, D);
        assert!((inside - range * 0.9).abs() < 1e-3, "inside: {inside}");

        // A little over the edge comes back near the other edge.
        let truth = range * 1.1;
        let outside = moose_offset(trial(D, truth, 30.0, &mut rng).0, D);
        assert!(
            (outside - truth).abs() > range,
            "an offset past the range should not come back right: {outside}"
        );
        assert!(
            (outside - (truth - 2.0 * range)).abs() < 1e-3,
            "it should wrap by exactly one turn, got {outside}"
        );
    }

    /// Is the corrected metric biased? The question only means anything against
    /// the resolution of the measurement asking it, so both are computed: the
    /// raw metric must sit many standard errors above the truth, and the
    /// corrected one must sit within a few of it. Measured at 4000 trials:
    /// 0.0121 raw against a standard error of 0.0012 at 64 pairs, and 0.0003
    /// left after the correction.
    ///
    /// Repeated at 256 pairs, where the first-order term is four times smaller,
    /// so a correction that only happened to fit at one window length would show
    /// itself.
    #[test]
    fn the_coherence_bias_is_real_and_the_correction_removes_it() {
        const TRIALS: usize = 4000;
        let snr = 1.0f64; // 0 dB, where the bias is largest relative to the value
        let rho2 = (snr / (1.0 + snr)).powi(2);
        for pairs in [64usize, 256] {
            let mut rng = Rng::new(0xC0DE + pairs as u64);
            let m: Vec<f64> = (0..TRIALS)
                .map(|_| trial(pairs, 0.0, 0.0, &mut rng).1)
                .collect();
            let sem = (variance(&m) / TRIALS as f64).sqrt();
            let raw = mean(&m) - rho2;
            let left = mean(
                &m.iter()
                    .map(|&v| coherence_squared(v, pairs))
                    .collect::<Vec<_>>(),
            ) - rho2;
            assert!(
                raw > 3.0 * sem,
                "{pairs}: the raw bias {raw:e} is not resolvable against {sem:e}"
            );
            assert!(
                left.abs() < 3.0 * sem,
                "{pairs}: the corrected metric is still {left:e} off, {:.1} standard errors",
                left.abs() / sem
            );
        }
    }

    /// The SNR falls out of the coherence with no extra estimation, and does so
    /// to a few tenths of a dB where it has the resolution.
    #[test]
    fn the_snr_falls_out_of_the_coherence() {
        const TRIALS: usize = 2000;
        for (pairs, snr_db) in [(64usize, 0.0f64), (64, 6.0), (64, 10.0), (1024, 20.0)] {
            let mut rng = Rng::new(0xBEEF + pairs as u64 + snr_db as u64);
            let s: Vec<f64> = (0..TRIALS)
                .map(|_| {
                    let m = trial(pairs, 0.0, snr_db, &mut rng).1;
                    snr_from_metric(m, pairs).map_or(f64::NAN, |v| 10.0 * v.log10())
                })
                .filter(|v| v.is_finite())
                .collect();
            assert!(
                s.len() > TRIALS * 9 / 10,
                "{pairs} pairs at {snr_db} dB: {} of {TRIALS} trials refused",
                TRIALS - s.len()
            );
            let got = mean(&s);
            assert!(
                (got - snr_db).abs() < 0.6,
                "{pairs} pairs: asked for {snr_db} dB, the coherence says {got:.2} dB"
            );
        }
    }

    /// **The ceiling, measured.** To read a 20 dB SNR off a correlation is to
    /// resolve a noise floor one percent of the signal, and with 64 pairs that
    /// floor's own standard error is larger than the floor. The estimator then
    /// returns a coherence of one and [`snr_from_metric`] refuses, which is the
    /// correct behaviour and not a defect: 22 % of trials refuse at 20 dB with
    /// 64 pairs, and under 1 % at 10 dB. A longer preamble raises the ceiling,
    /// which is why `the_snr_falls_out_of_the_coherence` reaches 20 dB with 1024.
    #[test]
    fn the_snr_estimator_refuses_above_its_ceiling_rather_than_guessing() {
        const TRIALS: usize = 2000;
        let mut rng = Rng::new(0xCE11);
        let refused = (0..TRIALS)
            .filter(|_| {
                let m = trial(D, 0.0, 20.0, &mut rng).1;
                snr_from_metric(m, D).is_none()
            })
            .count();
        assert!(
            refused > TRIALS / 20,
            "only {refused} of {TRIALS} refused at 20 dB with {D} pairs; the ceiling \
             this test documents has moved"
        );
    }

    /// The plateau is wider than the guard interval, and by a knowable amount.
    ///
    /// One sample past the flat run, one product in the sum is wrong and the
    /// metric reads `((L-1)/L)^2`; it takes `L * (1 - sqrt(0.9))`, about 5 % of
    /// the window, to fall through the paper's 90 % line. So the run this
    /// function returns overstates the guard interval by roughly a twentieth of
    /// the window at each end, plus whatever the unrelated data either side
    /// happens to correlate to. **A caller that read the guard interval off the
    /// plateau length would read it long**, which is the concrete reason this
    /// function returns geometry rather than a decision.
    #[test]
    fn the_plateau_covers_the_cyclic_prefix_and_a_little_more() {
        const L: usize = 64;
        const G: usize = 16;
        const PRE: usize = 200;
        let mut rng = Rng::new(21);
        let a = rng.qpsk(L);
        let mut x = rng.qpsk(PRE);
        x.extend_from_slice(&a[L - G..]);
        x.extend_from_slice(&a);
        x.extend_from_slice(&a);
        x.extend_from_slice(&rng.qpsk(400));

        let mut c = DelayedAutocorrelator::new(L, L);
        let m: Vec<f64> = x
            .iter()
            .filter_map(|s| c.push(*s))
            .map(|r| r.metric().unwrap())
            .collect();

        let p = schmidl_cox_plateau(&m, 0.9).unwrap();
        // The flat run itself: windows starting anywhere in the cyclic prefix.
        assert!(
            p.start <= PRE && p.end >= PRE + G,
            "the plateau {}..={} does not cover the flat run {PRE}..={}",
            p.start,
            p.end,
            PRE + G
        );
        let spill = (L as f64 * (1.0 - 0.9f64.sqrt())).ceil() as usize + 4;
        assert!(
            PRE - p.start <= spill && p.end - (PRE + G) <= spill,
            "the plateau {}..={} spills more than {spill} past {PRE}..={}",
            p.start,
            p.end,
            PRE + G
        );
        assert!(p.peak_metric > 0.99);
    }

    #[test]
    fn an_empty_metric_has_no_plateau() {
        assert!(schmidl_cox_plateau(&[], 0.9).is_none());
    }

    #[test]
    fn a_variance_with_no_signal_in_it_is_infinite() {
        assert!(moose_variance(0.0, D, D).is_infinite());
        assert!(moose_variance(-1.0, D, D).is_infinite());
        assert!(moose_variance(1.0, 0, D).is_infinite());
    }
}
