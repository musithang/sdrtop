// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The two correlators every burst detector in this project is built from.
//!
//! * [`DelayedAutocorrelator`] compares a signal with itself a fixed number of
//!   samples ago. It finds anything built out of a repeat, which is what an OFDM
//!   short training field is, and it does so without knowing the preamble: the
//!   repeat is the whole signature. Schmidl and Cox's timing metric is read
//!   straight off it, and so, in N6, is Moose's frequency offset.
//! * [`MatchedFilter`] compares a signal with a sequence known in advance. It
//!   finds a Bluetooth access address, and it is the optimal detector for a
//!   known waveform in white Gaussian noise, which is a statement with a proof
//!   rather than a preference.
//!
//! **Both are running-sum structures, and that is the point.** Detection is
//! always on, at up to 20 Msps, so it has to cost a few operations per sample
//! rather than a window's worth. A running sum buys that by adding one term and
//! subtracting another instead of re-adding the window, and pays for it by
//! carrying the rounding error of every sample it has ever seen. Everything here
//! accumulates in `f64` for that reason, and
//! `the_running_sums_match_the_direct_ones_over_a_long_run` is the assertion
//! that the cheap structure has not quietly drifted away from the honest one.
//!
//! **Both report a normalised figure, not a level**, in `[0, 1]`. A detector
//! whose threshold is an amplitude has to be retuned every time the gain moves,
//! and on a radio with AGC that is always. A normalised coherence means the same
//! thing at any gain, and, better, it means something a specification can be
//! written against: [`false_alarm_rate`] turns a threshold into the probability
//! that noise alone will trip it.

use num_complex::Complex;

/// How often a running sum is rebuilt from the history it still holds.
///
/// **A running sum carries the rounding error of every sample it has ever seen,
/// and design section 5.6 asks for a form whose error does not grow with
/// length.** Measured, an `f64` pair of sums drifts about 3e-13 relative over
/// four hundred thousand samples and would keep going; the objection is not the
/// size, which is harmless, but the shape, because a number that grows without a
/// bound has no bound to state. Rebuilding both sums from the ring every 65536 samples caps it at
/// whatever that many terms can accumulate, forever. The cost is one window's
/// worth of arithmetic every 65536 samples: about a tenth of a percent, which at
/// 20 Msps is every 3.3 ms.
const REFRESH: usize = 1 << 16;

/// One reading from [`DelayedAutocorrelator`].
#[derive(Clone, Copy, Debug)]
pub struct Coherence {
    /// The correlation itself. Its magnitude says how alike the two windows are;
    /// its argument is the phase the repeat accumulated, which is a frequency
    /// offset waiting to be read (N6).
    pub p: Complex<f64>,
    /// Energy of the later window, `sum |x|^2`.
    pub energy: f64,
}

impl Coherence {
    /// Schmidl and Cox's timing metric, `|P|^2 / R^2`, in `[0, 1]`.
    ///
    /// Source: Schmidl and Cox, "Robust Frequency and Timing Synchronization for
    /// OFDM", IEEE Trans. Communications 45(12), December 1997, equation 5.
    ///
    /// `None` when the window holds no energy at all. That is not a detection of
    /// nothing, it is the absence of anything to decide about, and returning a
    /// zero would put an invented number where a missing one belongs.
    pub fn metric(&self) -> Option<f64> {
        if self.energy > 0.0 {
            Some((self.p.norm_sqr() / (self.energy * self.energy)).min(1.0))
        } else {
            None
        }
    }
}

/// Correlation of a signal with itself, `lag` samples ago, over a sliding window.
pub struct DelayedAutocorrelator {
    lag: usize,
    window: usize,
    hist: Vec<Complex<f64>>,
    pos: usize,
    count: usize,
    p: Complex<f64>,
    energy: f64,
}

impl DelayedAutocorrelator {
    /// `window` terms at a lag of `lag` samples. For an OFDM short training
    /// field both are the length of one repeat.
    pub fn new(window: usize, lag: usize) -> Self {
        let (window, lag) = (window.max(1), lag.max(1));
        Self {
            lag,
            window,
            hist: vec![Complex::new(0.0, 0.0); window + lag + 1],
            pos: 0,
            count: 0,
            p: Complex::new(0.0, 0.0),
            energy: 0.0,
        }
    }

    /// **No consumer yet.** `signal::reference::capture` builds a fresh
    /// correlator per call rather than reusing one across captures, so nothing
    /// has needed this. It is here for whichever detector runs continuously
    /// over a live stream and needs to clear its state between windows -
    /// design section 10's `net::detect` split, or an OFDM burst detector's
    /// own preamble search.
    #[allow(dead_code)]
    pub fn reset(&mut self) {
        self.hist
            .iter_mut()
            .for_each(|s| *s = Complex::new(0.0, 0.0));
        self.pos = 0;
        self.count = 0;
        self.p = Complex::new(0.0, 0.0);
        self.energy = 0.0;
    }

    /// The sample `k` steps back from the newest one.
    fn at(&self, k: usize) -> Complex<f64> {
        let cap = self.hist.len();
        self.hist[(self.pos + cap - k) % cap]
    }

    /// Both sums, recomputed from the history the ring still holds. See
    /// [`REFRESH`].
    fn rebuild(&mut self) {
        let mut p = Complex::new(0.0, 0.0);
        let mut energy = 0.0;
        for k in 0..self.window {
            p += self.at(self.window + self.lag - 1 - k).conj() * self.at(self.window - 1 - k);
            energy += self.at(k).norm_sqr();
        }
        self.p = p;
        self.energy = energy;
    }

    /// Feed one sample. Returns a reading once the window is full, which is
    /// after `window + lag` samples and not before: nothing is reported from a
    /// half-filled correlator, the same rule the filters in this module follow.
    pub fn push(&mut self, x: Complex<f32>) -> Option<Coherence> {
        let cap = self.hist.len();
        self.pos = (self.pos + 1) % cap;
        self.hist[self.pos] = Complex::new(x.re as f64, x.im as f64);
        let t = self.count;
        self.count += 1;

        // The pair term that just became available, and the one that just left.
        if t >= self.lag {
            self.p += self.at(self.lag).conj() * self.at(0);
        }
        if t >= self.window + self.lag {
            let gone = self.at(self.window + self.lag).conj() * self.at(self.window);
            self.p -= gone;
        }

        // The energy window is simply the last `window` samples.
        self.energy += self.at(0).norm_sqr();
        if t >= self.window {
            self.energy -= self.at(self.window).norm_sqr();
        }

        if t + 1 >= self.window + self.lag {
            if self.count.is_multiple_of(REFRESH) {
                self.rebuild();
            }
            Some(Coherence {
                p: self.p,
                energy: self.energy,
            })
        } else {
            None
        }
    }
}

/// One reading from [`MatchedFilter`].
///
/// Reaches `main` since B6: `signal::ble::detect::Detector` builds a
/// combined preamble-and-access-address reference and reads this back on
/// every sample of a live capture, since the advertising access address is
/// known in advance and a known sequence correlated against the live stream
/// is exactly what a matched filter is for. Design section 10's F4 (symbol
/// timing from the L-LTF cross-correlation) is a second identified consumer,
/// not yet built.
#[derive(Clone, Copy, Debug)]
pub struct Match {
    /// The correlation with the reference sequence.
    pub value: Complex<f64>,
    /// Energy of the window the correlation was taken over.
    pub window_energy: f64,
    reference_energy: f64,
}

impl Match {
    /// `|y|^2 / (E_reference * E_window)`, in `[0, 1]` by Cauchy-Schwarz.
    ///
    /// One at a perfect match of any amplitude, and distributed by a law with a
    /// closed form under noise alone, which is what makes [`false_alarm_rate`]
    /// possible. `None` for an empty window, for [`Coherence::metric`]'s reason.
    pub fn coherence(&self) -> Option<f64> {
        let denom = self.reference_energy * self.window_energy;
        if denom > 0.0 {
            Some((self.value.norm_sqr() / denom).min(1.0))
        } else {
            None
        }
    }
}

/// Correlation of a signal with a sequence known in advance.
///
/// See [`Match`] for who uses it.
pub struct MatchedFilter {
    /// The reference, conjugated and reversed, so applying it is a forward walk
    /// back through the history.
    taps: Vec<Complex<f64>>,
    reference_energy: f64,
    hist: Vec<Complex<f64>>,
    pos: usize,
    count: usize,
    energy: f64,
}

impl MatchedFilter {
    pub fn new(reference: &[Complex<f32>]) -> Self {
        let wide: Vec<Complex<f64>> = reference
            .iter()
            .map(|s| Complex::new(s.re as f64, s.im as f64))
            .collect();
        let reference_energy = wide.iter().map(|s| s.norm_sqr()).sum();
        let n = wide.len().max(1);
        Self {
            taps: wide.iter().rev().map(|s| s.conj()).collect(),
            reference_energy,
            hist: vec![Complex::new(0.0, 0.0); n + 1],
            pos: 0,
            count: 0,
            energy: 0.0,
        }
    }

    pub fn len(&self) -> usize {
        self.taps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.taps.is_empty()
    }

    /// **No consumer yet.** `signal::reference::capture` builds a fresh
    /// correlator per call rather than reusing one across captures, so nothing
    /// has needed this. It is here for whichever detector runs continuously
    /// over a live stream and needs to clear its state between windows -
    /// design section 10's `net::detect` split, or an OFDM burst detector's
    /// own preamble search.
    #[allow(dead_code)]
    pub fn reset(&mut self) {
        self.hist
            .iter_mut()
            .for_each(|s| *s = Complex::new(0.0, 0.0));
        self.pos = 0;
        self.count = 0;
        self.energy = 0.0;
    }

    fn at(&self, k: usize) -> Complex<f64> {
        let cap = self.hist.len();
        self.hist[(self.pos + cap - k) % cap]
    }

    /// Feed one sample. Returns a reading once `reference.len()` samples have
    /// been seen; the reading is for the window ending at the sample just given.
    ///
    /// The correlation itself is recomputed each sample rather than carried,
    /// because a correlation against an arbitrary sequence has no running form.
    /// Only the window energy is a running sum, and it is the only part of this
    /// structure that can drift.
    pub fn push(&mut self, x: Complex<f32>) -> Option<Match> {
        let cap = self.hist.len();
        let n = self.taps.len();
        self.pos = (self.pos + 1) % cap;
        self.hist[self.pos] = Complex::new(x.re as f64, x.im as f64);
        let t = self.count;
        self.count += 1;

        self.energy += self.at(0).norm_sqr();
        if t >= n {
            self.energy -= self.at(n).norm_sqr();
        }

        if t + 1 < n {
            return None;
        }
        if self.count.is_multiple_of(REFRESH) {
            self.energy = (0..n).map(|k| self.at(k).norm_sqr()).sum();
        }
        let mut value = Complex::new(0.0, 0.0);
        for (u, tap) in self.taps.iter().enumerate() {
            value += tap * self.at(u);
        }
        Some(Match {
            value,
            window_energy: self.energy,
            reference_energy: self.reference_energy,
        })
    }
}

/// The probability that noise alone drives [`Match::coherence`] above
/// `threshold`, for a reference of `taps` samples.
///
/// For circularly symmetric noise the sample vector points in a direction
/// uniformly distributed on the complex unit sphere, so the squared cosine of
/// its angle to the reference is `Beta(1, N-1)` distributed and the exceedance
/// is `(1 - t)^(N-1)`. It depends on the length of the reference and on nothing
/// else: not on the noise power, not on the gain, not on the reference itself.
///
/// This is what a normalised detector buys. A threshold can be chosen from the
/// false-alarm rate it is required to have, which is a specification, rather
/// than from a level that happened to work on one recording.
/// `noise_alone_obeys_the_false_alarm_law` measures it rather than trusting it.
///
/// **Still no production consumer.** `signal::ble::detect`'s live path (B6
/// onward) reaches for [`threshold_for_false_alarm`] instead, the direction
/// a caller actually thinks in - "I want this false-alarm rate, what
/// threshold gives it" - not this function's own direction. Only this
/// module's own tests call it, checking the law it states rather than
/// living by it. F2 (Wi-Fi burst detection) is a second identified future
/// consumer of the same kind.
#[allow(dead_code)]
pub fn false_alarm_rate(taps: usize, threshold: f64) -> f64 {
    if taps < 2 {
        return 1.0;
    }
    (1.0 - threshold.clamp(0.0, 1.0)).powi(taps as i32 - 1)
}

/// The coherence threshold whose false-alarm probability is `rate`. The inverse
/// of [`false_alarm_rate`], and the direction a caller actually thinks in.
pub fn threshold_for_false_alarm(taps: usize, rate: f64) -> f64 {
    if taps < 2 {
        return 1.0;
    }
    1.0 - rate.clamp(0.0, 1.0).powf(1.0 / (taps - 1) as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::dsp::testkit::{at_snr, Rng};

    /// The honest form: recompute both sums over the window, from scratch.
    fn direct(x: &[Complex<f32>], n: usize, window: usize, lag: usize) -> Coherence {
        let wide = |i: usize| Complex::new(x[i].re as f64, x[i].im as f64);
        let mut p = Complex::new(0.0, 0.0);
        let mut energy = 0.0;
        for k in 0..window {
            p += wide(n + k).conj() * wide(n + k + lag);
            energy += wide(n + k + lag).norm_sqr();
        }
        Coherence { p, energy }
    }

    /// The running structure is the only reason 20 Msps is affordable, and it is
    /// also the only structure here that can be wrong in a way that grows.
    #[test]
    fn the_running_sums_match_the_direct_ones_over_a_long_run() {
        let (window, lag) = (64usize, 64usize);
        let mut rng = Rng::new(11);
        // Comfortably more than six refresh intervals, so what is measured is
        // the steady state and not one lucky stretch before the first rebuild.
        let x = rng.noise(400_000, 1.0);
        let mut c = DelayedAutocorrelator::new(window, lag);
        let mut worst_p = 0.0f64;
        let mut worst_e = 0.0f64;
        for (t, s) in x.iter().enumerate() {
            let Some(got) = c.push(*s) else { continue };
            if t % 4096 != 0 {
                continue;
            }
            let want = direct(&x, t + 1 - window - lag, window, lag);
            worst_p = worst_p.max((got.p - want.p).norm() / want.p.norm().max(1.0));
            worst_e = worst_e.max((got.energy - want.energy).abs() / want.energy);
        }
        // Measured: 1.4e-13 on the correlation and 8.4e-14 on the energy, against
        // 2.6e-13 and 3.1e-13 for the same run with the rebuild removed. The
        // improvement in size is the small half of the point. The large half is
        // that this run covers six refresh intervals and the error is reset at
        // each one, so the bound is a property of a single interval and holds for
        // a run of any length - which is what design section 5.6 asks for, and
        // the only form of the claim that is a number rather than a trend.
        assert!(
            worst_p < 1e-12 && worst_e < 1e-12,
            "running sums drifted: p by {worst_p:e}, energy by {worst_e:e}"
        );
    }

    /// The plateau is the reason this detector is used rather than a matched
    /// filter: a repeat preceded by a cyclic prefix correlates perfectly for the
    /// whole length of the prefix, so the metric is flat and the timing decision
    /// can be made calmly rather than off a single sample.
    #[test]
    fn a_repeated_preamble_makes_the_plateau_the_detector_relies_on() {
        const L: usize = 64;
        const G: usize = 16;
        let mut rng = Rng::new(12);
        let a = rng.qpsk(L);
        // Schmidl and Cox's preamble: a cyclic prefix, then the half twice.
        let mut x = Vec::new();
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

        // Output j is the metric for a window starting at input index j.
        let plateau = (0..=G).all(|j| m[j] > 0.99);
        assert!(plateau, "no plateau: {:?}", &m[..=G]);
        assert!(
            m[G + 1] < 0.99,
            "the plateau outlasts the cyclic prefix, which it cannot"
        );
        // Well past the preamble the metric must collapse. Random data has no
        // repeat at this lag, so what is left is the correlation of noise.
        let far = m[G + 2 * L..].iter().cloned().fold(0.0f64, f64::max);
        assert!(far < 0.5, "the metric stays at {far} on unrelated data");
    }

    #[test]
    fn the_matched_filter_peaks_where_the_sequence_is() {
        const N: usize = 64;
        const OFFSET: usize = 300;
        let mut rng = Rng::new(13);
        let seq = rng.qpsk(N);
        let mut x = rng.qpsk(OFFSET);
        x.extend_from_slice(&seq);
        x.extend_from_slice(&rng.qpsk(300));

        let mut mf = MatchedFilter::new(&seq);
        let c: Vec<f64> = x
            .iter()
            .filter_map(|s| mf.push(*s))
            .map(|r| r.coherence().unwrap())
            .collect();

        // A reading is for the window ending at the sample just fed, and the
        // first reading is at input index N-1, so output j ends at input j+N-1
        // and starts at input j.
        let peak = c
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap();
        assert_eq!(
            peak.0, OFFSET,
            "peak at {}, sequence is at {OFFSET}",
            peak.0
        );
        assert!(
            *peak.1 > 0.999,
            "a noiseless match should be 1, not {}",
            peak.1
        );
    }

    /// The closed form the whole normalisation exists to make possible, checked
    /// against a measurement rather than believed.
    ///
    /// **The short reference at a high threshold is the case that pins the
    /// exponent.** At 64 taps, confusing `N - 1` with `N` changes the predicted
    /// rate by a factor of `1 - t`, which at these thresholds is within a few
    /// percent of one and no measurement of this length could see it. At 4 taps
    /// and a threshold of 0.9 the same confusion is a factor of ten.
    #[test]
    fn noise_alone_obeys_the_false_alarm_law() {
        for (n, thresholds) in [(64usize, &[0.05, 0.1, 0.15][..]), (4, &[0.5, 0.9][..])] {
            let mut rng = Rng::new(14);
            let seq = rng.qpsk(n);
            let x = rng.noise(400_000, 1.0);
            let mut mf = MatchedFilter::new(&seq);
            let c: Vec<f64> = x
                .iter()
                .filter_map(|s| mf.push(*s))
                .map(|r| r.coherence().unwrap())
                .collect();

            for &t in thresholds {
                let want = false_alarm_rate(n, t);
                let got = c.iter().filter(|&&v| v > t).count() as f64 / c.len() as f64;
                // Overlapping windows make consecutive readings correlated, so
                // the count has a wider spread than a binomial would, but its
                // mean is untouched. A factor of two either way is a real test
                // at these rates and does not depend on the seed.
                assert!(
                    got > want / 2.0 && got < want * 2.0,
                    "n={n} threshold {t}: measured {got:e}, the law says {want:e}"
                );
            }
        }
    }

    #[test]
    fn a_threshold_and_its_false_alarm_rate_are_inverses() {
        for n in [8usize, 64, 256] {
            for rate in [1e-2, 1e-4, 1e-8] {
                let t = threshold_for_false_alarm(n, rate);
                let back = false_alarm_rate(n, t);
                assert!(
                    (back / rate - 1.0).abs() < 1e-9,
                    "n={n} rate={rate}: threshold {t} gives {back}"
                );
            }
        }
    }

    /// N5's exit condition: a known preamble, buried in noise at a stated SNR,
    /// found at the right offset, with the false-alarm rate the threshold was
    /// chosen for.
    #[test]
    fn a_preamble_in_noise_is_found_at_the_stated_false_alarm_rate() {
        const N: usize = 64;
        const OFFSET: usize = 2000;
        const SNR_DB: f64 = 0.0;
        const RATE: f64 = 1e-6;

        let mut rng = Rng::new(15);
        let seq = rng.qpsk(N);
        let mut clean = rng.qpsk(OFFSET);
        clean.extend_from_slice(&seq);
        clean.extend_from_slice(&rng.qpsk(60_000));
        let x = at_snr(&clean, SNR_DB, &mut Rng::new(16));

        let threshold = threshold_for_false_alarm(N, RATE);
        let mut mf = MatchedFilter::new(&seq);
        let c: Vec<f64> = x
            .iter()
            .filter_map(|s| mf.push(*s))
            .map(|r| r.coherence().unwrap())
            .collect();

        assert!(
            c[OFFSET] > threshold,
            "the preamble scored {} against a threshold of {threshold}",
            c[OFFSET]
        );
        let elsewhere = c
            .iter()
            .enumerate()
            .filter(|(j, _)| j.abs_diff(OFFSET) > N)
            .filter(|(_, &v)| v > threshold)
            .count();
        assert_eq!(
            elsewhere,
            0,
            "{elsewhere} false alarms in {} samples at a rate of {RATE}",
            c.len()
        );
    }
}
