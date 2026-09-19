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

use std::sync::Arc;

use num_complex::Complex;
use rustfft::{Fft, FftPlanner};

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
    /// The reference, conjugated, in its own order: applying it is a dot
    /// product with the window oldest sample first.
    taps: Vec<Complex<f64>>,
    reference_energy: f64,
    /// The last `n` samples, **written twice**: sample `i` goes to `i % n`
    /// and to `i % n + n`. Whatever the write position, the whole window in
    /// time order is then one contiguous slice, `hist[pos + 1..pos + 1 + n]`.
    ///
    /// **This is the receiver's hot loop, and the layout is the speed.** The
    /// first version kept an ordinary ring and indexed it with a modulo per
    /// tap: for BLE's 142-tap reference at 4 Msps that is 570 million
    /// divisions a second, and it measured at 1.4 us a sample - the matched
    /// filter alone ran at a fifth of real time and took the whole receiver
    /// with it. A contiguous window costs one extra write a sample and lets
    /// the dot product run straight through memory.
    hist: Vec<Complex<f64>>,
    /// Where the newest sample was written, in `0..n`.
    pos: usize,
    count: usize,
    energy: f64,
    /// The FFT plan [`Self::process_block`] uses, built the first time it is
    /// called - a caller that only ever calls [`Self::push`] never pays for it.
    block: Option<BlockPlan>,
}

/// Overlap-save correlation: the plan and the reference, transformed once.
struct BlockPlan {
    size: usize,
    forward: Arc<dyn Fft<f64>>,
    inverse: Arc<dyn Fft<f64>>,
    /// The reference as a filter kernel, transformed, with the inverse
    /// transform's `1 / size` folded in.
    kernel: Vec<Complex<f64>>,
    buffer: Vec<Complex<f64>>,
    scratch: Vec<Complex<f64>>,
}

impl BlockPlan {
    fn new(taps: &[Complex<f64>]) -> Self {
        let n = taps.len().max(1);
        // Four taps' worth or more per transform keeps most of each one's
        // output valid: `size - n + 1` of every `size` samples.
        let size = (4 * n).next_power_of_two().max(256);
        let mut planner = FftPlanner::<f64>::new();
        let forward = planner.plan_fft_forward(size);
        let inverse = planner.plan_fft_inverse(size);
        // Correlating with `taps` (applied oldest sample first) is convolving
        // with `taps` reversed.
        let scale = 1.0 / size as f64;
        let mut kernel = vec![Complex::new(0.0, 0.0); size];
        for (j, slot) in kernel.iter_mut().take(n).enumerate() {
            *slot = taps[n - 1 - j] * scale;
        }
        let scratch_len = forward
            .get_inplace_scratch_len()
            .max(inverse.get_inplace_scratch_len());
        let mut scratch = vec![Complex::new(0.0, 0.0); scratch_len];
        forward.process_with_scratch(&mut kernel, &mut scratch);
        Self {
            size,
            forward,
            inverse,
            kernel,
            buffer: vec![Complex::new(0.0, 0.0); size],
            scratch,
        }
    }
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
            taps: wide.iter().map(|s| s.conj()).collect(),
            reference_energy,
            hist: vec![Complex::new(0.0, 0.0); 2 * n],
            // One behind the first slot, so the first write lands on 0.
            pos: n - 1,
            count: 0,
            energy: 0.0,
            block: None,
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
        self.pos = self.taps.len().max(1) - 1;
        self.count = 0;
        self.energy = 0.0;
    }

    /// Feed a block of samples at once: one reading per sample, exactly what
    /// [`Self::push`] would have returned for each of them in turn.
    ///
    /// **The same answer by a cheaper road.** `push` recomputes the whole
    /// correlation for every sample, `n` multiply-adds each, because a
    /// correlation against an arbitrary sequence has no running form. Over a
    /// block it has a fast one: overlap-save, a transform per few hundred
    /// samples instead of `n` operations per sample. For BLE's 142-tap
    /// reference that is most of an order of magnitude, and it is the step
    /// that put the receiver inside real time. The window energy is kept by
    /// the same running sum, refreshed at the same points, as `push` keeps
    /// it; `the_block_path_is_the_sample_path` holds the two to agreeing
    /// sample by sample, and `push` may carry on after a block as if every
    /// sample had gone through it.
    pub fn process_block(&mut self, x: &[Complex<f32>], out: &mut Vec<Option<Match>>) {
        out.clear();
        if x.is_empty() {
            return;
        }
        let n = self.taps.len().max(1);
        if self.block.is_none() {
            self.block = Some(BlockPlan::new(&self.taps));
        }

        // The stream this block's windows reach into: the whole window before
        // it, oldest first, then the block. The first of those `n` samples is
        // read only as the one leaving the first new window, for the running
        // energy. Before `n` samples have been seen the missing ones are
        // zeros, and every window that would read one is a warm-up reading
        // `push` returns `None` for.
        let mut stream: Vec<Complex<f64>> = Vec::with_capacity(n + x.len());
        stream.extend_from_slice(&self.hist[self.pos + 1..self.pos + 1 + n]);
        stream.extend(x.iter().map(|s| Complex::new(s.re as f64, s.im as f64)));

        // Correlations, one per new sample: window `i` is
        // `stream[1 + i..1 + i + n]`, ending on new sample `i`.
        let mut values = vec![Complex::new(0.0, 0.0); x.len()];
        let plan = self.block.as_mut().expect("built above");
        let step = plan.size - n + 1;
        let mut start = 0;
        while start < x.len() {
            for (j, slot) in plan.buffer.iter_mut().enumerate() {
                *slot = stream
                    .get(1 + start + j)
                    .copied()
                    .unwrap_or(Complex::new(0.0, 0.0));
            }
            plan.forward
                .process_with_scratch(&mut plan.buffer, &mut plan.scratch);
            for (b, k) in plan.buffer.iter_mut().zip(&plan.kernel) {
                *b *= k;
            }
            plan.inverse
                .process_with_scratch(&mut plan.buffer, &mut plan.scratch);
            // Circular outputs `n - 1..size` are the linear ones: window
            // `start + m - (n - 1)` for each.
            for m in (n - 1)..plan.size {
                let i = start + m - (n - 1);
                if i >= x.len() {
                    break;
                }
                values[i] = plan.buffer[m];
            }
            start += step;
        }

        out.reserve(x.len());
        for (i, value) in values.into_iter().enumerate() {
            let t = self.count;
            self.count += 1;
            self.energy += stream[n + i].norm_sqr();
            if t >= n {
                self.energy -= stream[i].norm_sqr();
            }
            if t + 1 < n {
                out.push(None);
                continue;
            }
            if self.count.is_multiple_of(REFRESH) {
                self.energy = stream[1 + i..1 + i + n].iter().map(|s| s.norm_sqr()).sum();
            }
            out.push(Some(Match {
                value,
                window_energy: self.energy,
                reference_energy: self.reference_energy,
            }));
        }

        // Leave the history as `push` would have: the last `n` samples, in
        // order, so the next `push` or block sees the right window.
        for &s in &stream[stream.len().saturating_sub(n)..] {
            self.pos = if self.pos + 1 == n { 0 } else { self.pos + 1 };
            self.hist[self.pos] = s;
            self.hist[self.pos + n] = s;
        }
    }

    /// Feed one sample. Returns a reading once `reference.len()` samples have
    /// been seen; the reading is for the window ending at the sample just given.
    ///
    /// The correlation itself is recomputed each sample rather than carried,
    /// because a correlation against an arbitrary sequence has no running form.
    /// Only the window energy is a running sum, and it is the only part of this
    /// structure that can drift.
    pub fn push(&mut self, x: Complex<f32>) -> Option<Match> {
        let n = self.taps.len().max(1);
        self.pos = if self.pos + 1 == n { 0 } else { self.pos + 1 };
        let sample = Complex::new(x.re as f64, x.im as f64);
        // The slot being overwritten holds the sample leaving the window.
        let leaving = self.hist[self.pos];
        self.hist[self.pos] = sample;
        self.hist[self.pos + n] = sample;
        let t = self.count;
        self.count += 1;

        self.energy += sample.norm_sqr();
        if t >= n {
            self.energy -= leaving.norm_sqr();
        }

        if t + 1 < n {
            return None;
        }
        let window = &self.hist[self.pos + 1..self.pos + 1 + n];
        if self.count.is_multiple_of(REFRESH) {
            self.energy = window.iter().map(|s| s.norm_sqr()).sum();
        }
        // Real and imaginary parts accumulated separately, so the loop is
        // four independent multiply-adds per tap with nothing to stop the
        // compiler vectorising it.
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (tap, s) in self.taps.iter().zip(window) {
            re += tap.re * s.re - tap.im * s.im;
            im += tap.re * s.im + tap.im * s.re;
        }
        let value = Complex::new(re, im);
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

    /// The matched filter is exactly a correlation, checked against the
    /// definition computed the slowest way there is: every window, summed
    /// out in full, for every sample - across the energy refresh too, where
    /// a running sum and a recomputed one have to agree.
    #[test]
    fn the_matched_filter_is_the_correlation_it_claims_to_be() {
        const N: usize = 16;
        let mut rng = Rng::new(21);
        let reference = rng.qpsk(N);
        let x = rng.qpsk(REFRESH + 3 * N);
        let mut mf = MatchedFilter::new(&reference);
        for (t, s) in x.iter().enumerate() {
            let got = mf.push(*s);
            if t + 1 < N {
                assert!(got.is_none(), "no reading before a full window, t={t}");
                continue;
            }
            let got = got.expect("a reading once the window is full");
            let window = &x[t + 1 - N..=t];
            let mut value = Complex::new(0.0f64, 0.0);
            let mut energy = 0.0f64;
            for (r, w) in reference.iter().zip(window) {
                let r = Complex::new(r.re as f64, r.im as f64);
                let w = Complex::new(w.re as f64, w.im as f64);
                value += r.conj() * w;
                energy += w.norm_sqr();
            }
            assert!(
                (got.value - value).norm() < 1e-9,
                "t={t}: {} vs {value}",
                got.value
            );
            assert!((got.window_energy - energy).abs() < 1e-9, "t={t}");
        }
    }

    /// The block path is the sample path by another road: the same reading
    /// for every sample, whatever the blocks happen to be cut into - blocks
    /// shorter than the reference, a first block that ends mid warm-up, and
    /// a run long enough to cross the energy refresh - and `push` carries on
    /// after a block exactly as if every sample had gone through it.
    #[test]
    fn the_block_path_is_the_sample_path() {
        const N: usize = 142;
        let mut rng = Rng::new(31);
        let reference = rng.qpsk(N);
        let x = rng.qpsk(REFRESH + 20_000);

        let mut by_sample = MatchedFilter::new(&reference);
        let expected: Vec<Option<Match>> = x.iter().map(|s| by_sample.push(*s)).collect();

        let mut by_block = MatchedFilter::new(&reference);
        let mut got = Vec::new();
        let mut out = Vec::new();
        let sizes = [1usize, 7, 100, 1000, 5000, 3];
        let mut at = 0;
        let mut k = 0;
        // Stop short of the end, so the tail goes through `push`.
        while at < x.len() - 500 {
            let len = sizes[k % sizes.len()].min(x.len() - 500 - at);
            by_block.process_block(&x[at..at + len], &mut out);
            got.extend(out.iter().copied());
            at += len;
            k += 1;
        }
        got.extend(x[at..].iter().map(|s| by_block.push(*s)));

        assert_eq!(got.len(), expected.len());
        for (t, (g, e)) in got.iter().zip(&expected).enumerate() {
            match (g, e) {
                (None, None) => {}
                (Some(g), Some(e)) => {
                    assert!(
                        (g.value - e.value).norm() < 1e-9,
                        "t={t}: {} vs {}",
                        g.value,
                        e.value
                    );
                    assert!((g.window_energy - e.window_energy).abs() < 1e-9, "t={t}");
                }
                _ => panic!("t={t}: one path has a reading and the other does not"),
            }
        }
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
