// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Rational resampling by L/M, polyphase.
//!
//! A radio that can be set to exactly the rate a mode needs does not come here.
//! This is for the second case: the device's rate is a rational multiple of the
//! one the mode wants, so the stream has to be moved onto the right grid before
//! anything measures a symbol on it. The third case, a device that cannot reach
//! the rate at all, is refused rather than fudged, and that decision is made
//! well above this module.
//!
//! **The idea in one line:** insert `L-1` zeros between input samples, low-pass,
//! keep every `M`th result. Doing that literally would multiply the sample count
//! by `L` and then throw most of the work away, so the filter is split into `L`
//! branches and each output touches only the branch that lands on it: the cost
//! is the same as one filter at the input rate, whatever `L` is.
//!
//! **What the filter has to do decides its specification, and both halves
//! matter:** the zero-stuffing puts images of the signal at every multiple of
//! the input rate, and the decimation folds everything above the output Nyquist
//! back into the band. One low-pass handles both, and the band edge is the
//! lower of the two Nyquist frequencies, `0.5 / max(L, M)` on the upsampled
//! grid. That is why the filter comes from `fir::design_lowpass_to_spec`: the
//! caller states the rejection it needs, in dB, and gets it. A fixed window
//! would have handed out one number and left every caller to find out whether
//! it was enough.
//!
//! **Gain.** The design has unit DC gain, but only one branch in `L` contributes
//! to each output, so the kernels carry a factor of `L`. A resampler that changes
//! a level is a resampler that quietly rewrites every dBm on screen.
//!
//! **Delay is a measurement, not an inconvenience.** A linear-phase kernel delays
//! everything by half its length, and a burst timestamped without accounting for
//! that is wrong by a known, constant amount. [`Resampler::delay_input_samples`]
//! states it, in input samples, so the caller can subtract it rather than
//! discover it.

use super::fir::{design_lowpass_kaiser, kaiser_beta, kaiser_taps};
use num_complex::Complex;

/// No consumer outside this file; see [`Resampler`].
fn gcd(a: usize, b: usize) -> usize {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

/// A streaming polyphase resampler at a fixed rational ratio.
///
/// **No production consumer yet.** It is for a radio whose rate is a rational
/// multiple of what a mode needs, and nothing yet asks a radio for a rate it
/// cannot produce directly.
pub struct Resampler {
    l: usize,
    m: usize,
    /// `l` branch kernels of `bt` taps each, reversed so applying one is a
    /// forward dot product against the sample window, and scaled by `l`.
    branches: Vec<Vec<f32>>,
    bt: usize,
    /// Carried input: the history the next output needs, plus anything not yet
    /// consumed.
    buf: Vec<Complex<f32>>,
    /// Index into `buf` of the newest input sample the next output needs.
    q: usize,
    /// Which polyphase branch the next output falls on.
    p: usize,
    /// Group delay of the designed kernel, in upsampled samples.
    delay_up: f64,
}

impl Resampler {
    /// A resampler by `l/m`, with the anti-image and anti-alias filter designed
    /// to `stopband_db` of rejection.
    ///
    /// `rolloff` is the fraction of the usable band spent on the filter's
    /// transition: 0.1 keeps nine tenths of it flat and pays for the sharpness
    /// with taps. It is a specification and not a taste, so it is clamped into
    /// `(0.001, 0.999)` rather than allowed to be a number that describes no
    /// filter at all.
    ///
    /// The ratio is reduced first. Asking for 4/2 builds the same object as
    /// asking for 2/1, rather than doing twice the work to reach the same grid.
    pub fn new(l: usize, m: usize, rolloff: f64, stopband_db: f64) -> Self {
        let (l, m) = (l.max(1), m.max(1));
        let g = gcd(l, m);
        let (l, m) = (l / g, m / g);

        // The band edge is the lower of the two Nyquist frequencies, expressed on
        // the upsampled grid, because that is the grid the kernel lives on.
        let edge = 0.5 / l.max(m) as f64;
        let rolloff = rolloff.clamp(0.001, 0.999);
        let transition = edge * rolloff;
        let fc = edge - transition / 2.0;

        let nt = kaiser_taps(transition, stopband_db);
        let h = design_lowpass_kaiser(nt, fc, kaiser_beta(stopband_db));

        // Every branch must have the same length, so the kernel is padded up to a
        // multiple of `l`. The padding is zeros and goes on both ends: zeros do
        // not change the response, and splitting them keeps the centre of the
        // kernel as close to the centre of the padded array as an odd tap count
        // and an arbitrary `l` allow. Where it cannot be exact, the group delay
        // below is computed from where the centre actually is, not from where a
        // symmetric array would put it.
        let bt = h.len().div_ceil(l);
        let pad = bt * l - h.len();
        let front = pad / 2;
        let mut padded = vec![0.0f32; bt * l];
        padded[front..front + h.len()].copy_from_slice(&h);
        let delay_up = front as f64 + (h.len() - 1) as f64 / 2.0;

        let gain = l as f32;
        let branches = (0..l)
            .map(|p| {
                (0..bt)
                    .map(|u| padded[p + (bt - 1 - u) * l] * gain)
                    .collect()
            })
            .collect();

        let mut r = Self {
            l,
            m,
            branches,
            bt,
            buf: Vec::new(),
            q: 0,
            p: 0,
            delay_up,
        };
        r.reset();
        r
    }

    /// Forget the carried state. The next output block starts a fresh run, and
    /// its first sample sits at [`Self::delay_input_samples`] again.
    pub fn reset(&mut self) {
        self.buf.clear();
        // The first output is the first one whose window is entirely inside the
        // data. Nothing is emitted from a half-filled filter, which is the same
        // rule `StreamingDecimator` follows.
        self.q = self.bt - 1;
        self.p = 0;
    }

    /// The input-sample time the first output after a reset stands for.
    ///
    /// Output `k` stands for input time `delay_input_samples() + k * m / l`. This
    /// is the number to subtract from a burst's position before it is called a
    /// time, and it is exact: the kernel is linear phase, so the delay is the
    /// same at every frequency in the passband.
    pub fn delay_input_samples(&self) -> f64 {
        ((self.bt - 1) * self.l) as f64 / self.l as f64 - self.delay_up / self.l as f64
    }

    pub fn process(&mut self, input: &[Complex<f32>], out: &mut Vec<Complex<f32>>) {
        out.clear();
        self.buf.extend_from_slice(input);

        while self.q < self.buf.len() {
            let start = self.q + 1 - self.bt;
            let acc = {
                let w = &self.buf[start..=self.q];
                let k = &self.branches[self.p];
                let mut re = 0.0f32;
                let mut im = 0.0f32;
                for (s, &c) in w.iter().zip(k.iter()) {
                    re += s.re * c;
                    im += s.im * c;
                }
                Complex::new(re, im)
            };
            out.push(acc);

            // Step one output along the upsampled grid: `m` upsampled samples,
            // which is `(p + m) / l` input samples and a new branch.
            let n = self.p + self.m;
            self.p = n % self.l;
            self.q += n / self.l;
        }

        // Keep only what the next output still needs. When the stride overshoots
        // the buffer, everything goes and `q` carries the deficit into the next
        // block, which is the same bookkeeping the decimator does.
        let keep_from = self.q + 1 - self.bt;
        let consumed = keep_from.min(self.buf.len());
        self.buf.drain(..consumed);
        self.q -= consumed;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::dsp::nco::Nco;
    use std::f64::consts::TAU;

    const ROLLOFF: f64 = 0.1;
    const STOPBAND: f64 = 60.0;

    fn tone(f: f64, n: usize) -> Vec<Complex<f32>> {
        let mut v = vec![Complex::new(0.0f32, 0.0); n];
        Nco::new(f, 1.0).fill(&mut v);
        v
    }

    fn db(x: f64) -> f64 {
        20.0 * x.log10()
    }

    /// Mean magnitude, and the frequency recovered from argument differences in
    /// cycles per sample.
    ///
    /// Two pieces of arithmetic here are deliberate, and both are the same
    /// lesson the oscillator taught: the measurement's own precision runs out
    /// before the signal's does. The arguments are taken in `f64` because a
    /// hundred and sixty thousand `f32` arguments carry a rounding bias that
    /// reaches a milliradian in total, which is a nanocycle per sample of
    /// imaginary frequency error. The sum is compensated because a long sum of
    /// small angles is otherwise limited by the size of its own running total.
    fn measure(x: &[Complex<f32>]) -> (f64, f64) {
        let wide = |s: &Complex<f32>| Complex::new(s.re as f64, s.im as f64);
        let mag = x.iter().map(|s| wide(s).norm()).sum::<f64>() / x.len() as f64;
        let (mut sum, mut carry) = (0.0f64, 0.0f64);
        for w in x.windows(2) {
            let d = (wide(&w[1]) / wide(&w[0])).arg() - carry;
            let t = sum + d;
            carry = (t - sum) - d;
            sum = t;
        }
        (mag, sum / (x.len() - 1) as f64 / TAU)
    }

    fn run_ragged(r: &mut Resampler, input: &[Complex<f32>], sizes: &[usize]) -> Vec<Complex<f32>> {
        let mut all = Vec::new();
        let mut out = Vec::new();
        let mut i = 0;
        let mut k = 0;
        while i < input.len() {
            let n = sizes[k % sizes.len()].min(input.len() - i);
            r.process(&input[i..i + n], &mut out);
            all.extend_from_slice(&out);
            i += n;
            k += 1;
        }
        all
    }

    /// The rate, measured over an interval rather than from the beginning of
    /// time.
    ///
    /// The first block is short by the filter's history: nothing is emitted from
    /// a half-filled filter, which is the convention `StreamingDecimator` set. A
    /// test that counts from a reset therefore measures the startup as well as
    /// the rate, and the two are only separable by making the run long enough to
    /// drown the startup - which is how the first version of this test passed
    /// while being 90 outputs short. Warming up first and counting afterwards
    /// leaves nothing to hide in, so the claim can be exact: over any interval,
    /// `l` outputs for every `m` inputs, to within the one sample the grid can
    /// be caught mid-step.
    #[test]
    fn the_output_rate_is_the_ratio_it_was_given() {
        for (l, m) in [(5usize, 4usize), (4, 5), (1, 4), (3, 2), (1, 1), (8, 3)] {
            const N: usize = 60_000;
            let mut r = Resampler::new(l, m, ROLLOFF, STOPBAND);
            let x = tone(0.01, 2 * N);
            let pieces = [1000, 333, 4096, 7];
            let _warmup = run_ragged(&mut r, &x[..N], &pieces);
            let steady = run_ragged(&mut r, &x[N..], &pieces).len();
            let want = N as f64 * l as f64 / m as f64;
            assert!(
                (steady as f64 - want).abs() <= 1.0,
                "{l}/{m}: {steady} outputs for {N} inputs, expected {want}"
            );
        }
    }

    #[test]
    fn a_tone_keeps_its_frequency_and_its_amplitude() {
        let delta = 10f64.powf(-STOPBAND / 20.0);
        for (l, m) in [(5usize, 4usize), (4, 5), (1, 4), (3, 2), (1, 1), (8, 3)] {
            let f_in = 0.02;
            let mut r = Resampler::new(l, m, ROLLOFF, STOPBAND);
            let mut out = Vec::new();
            r.process(&tone(f_in, 20_000), &mut out);
            let (mag, f_out) = measure(&out);
            let want = f_in * m as f64 / l as f64;
            assert!(
                (f_out - want).abs() < 1e-9,
                "{l}/{m}: tone came out at {f_out} cycles/sample, expected {want}"
            );
            assert!(
                (mag - 1.0).abs() < 2.0 * delta,
                "{l}/{m}: amplitude {mag} is outside the designed ripple"
            );
        }
    }

    /// A tone above the lower of the two Nyquist frequencies has nowhere honest
    /// to go: decimation folds it into the band. The filter is the only thing
    /// stopping it, and the point of a specified design is that the caller says
    /// how hard.
    ///
    /// **Swept across the whole stopband, and asserted from both sides.** A
    /// single tone deep in the stopband passes this with twenty-odd dB to
    /// spare, because the worst rejection is at the stopband edge and nowhere
    /// else, so such a test would sail past a filter designed twenty dB too
    /// weak. The upper bound matters for the same reason it does for the filter
    /// design: a resampler that rejects far more than it was asked to is
    /// spending taps nobody authorised.
    #[test]
    fn a_tone_that_would_alias_is_suppressed_by_the_designed_stopband() {
        for stopband in [40.0, 60.0, 80.0] {
            let (l, m) = (1usize, 4usize);
            let edge = 0.5 / m as f64;
            let mut worst: f64 = 0.0;
            let mut at = 0.0;
            // One resampler, reset between tones: designing the filter forty-one
            // times over would measure nothing the first design does not already
            // say, and it exercises `reset` for free.
            let mut r = Resampler::new(l, m, ROLLOFF, stopband);
            let mut out = Vec::new();
            for k in 0..=40 {
                let f = edge + (0.5 - edge) * k as f64 / 40.0;
                r.reset();
                r.process(&tone(f, 4000), &mut out);
                let (mag, _) = measure(&out);
                if mag > worst {
                    worst = mag;
                    at = f;
                }
            }
            let got = -db(worst);
            assert!(
                got >= stopband,
                "asked for {stopband} dB, the worst alias came through at {got:.2} dB, at {at}"
            );
            assert!(
                got <= stopband + 3.0,
                "asked for {stopband} dB and got {got:.2} dB: the design is paying for taps it was not asked for"
            );
        }
    }

    /// A ratio of one still filters, so this is not an identity. What it must be
    /// is the input, delayed by exactly the amount the resampler states.
    #[test]
    fn a_ratio_of_one_is_the_input_delayed_by_the_stated_amount() {
        let mut r = Resampler::new(1, 1, ROLLOFF, STOPBAND);
        let d = r.delay_input_samples();
        assert_eq!(d, d.round(), "at 1/1 the delay must be a whole sample");
        let x = tone(0.1, 10_000);
        let mut y = Vec::new();
        r.process(&x, &mut y);
        let delta = 10f64.powf(-STOPBAND / 20.0);
        for (k, s) in y.iter().enumerate().take(5_000) {
            let want = x[k + d as usize];
            assert!(
                (s - want).norm() as f64 <= 3.0 * delta,
                "sample {k}: {s} against {want}, delay {d}"
            );
        }
    }

    #[test]
    fn feeding_in_ragged_pieces_matches_one_long_block() {
        for (l, m) in [(5usize, 4usize), (1, 4), (8, 3)] {
            let x = tone(0.03, 20_000);
            let mut whole = Resampler::new(l, m, ROLLOFF, STOPBAND);
            let mut a = Vec::new();
            whole.process(&x, &mut a);

            let mut split = Resampler::new(l, m, ROLLOFF, STOPBAND);
            let b = run_ragged(&mut split, &x, &[1, 2, 3, 5, 8, 1013, 4096]);

            assert_eq!(a, b, "{l}/{m}: the seam between blocks is visible");
        }
    }

    /// Up by 5/4 and back down by 4/5 is a round trip
    /// through both directions of the same machinery, and what comes out has to
    /// be the tone that went in.
    #[test]
    fn a_tone_resampled_up_and_back_down_is_the_same_tone() {
        let f = 0.02;
        let x = tone(f, 60_000);
        let mut up = Resampler::new(5, 4, ROLLOFF, STOPBAND);
        let mut down = Resampler::new(4, 5, ROLLOFF, STOPBAND);
        let (mut a, mut b) = (Vec::new(), Vec::new());
        up.process(&x, &mut a);
        down.process(&a, &mut b);

        let (mag, f_out) = measure(&b);
        assert!(
            (f_out - f).abs() < 1e-9,
            "the round trip returned {f_out} cycles/sample, not {f}"
        );
        let delta = 10f64.powf(-STOPBAND / 20.0);
        assert!(
            (mag - 1.0).abs() < 4.0 * delta,
            "the round trip returned amplitude {mag}"
        );

        // Residual against the ideal tone, once the constant phase the two
        // delays add is taken out. This is the whole claim, in one number.
        let mut ref_nco = Nco::new(f, 1.0);
        let mut refs = vec![Complex::new(0.0f32, 0.0); b.len()];
        ref_nco.fill(&mut refs);
        let rot = (b[0] / refs[0]).unscale(b[0].norm());
        let err = b
            .iter()
            .zip(refs.iter())
            .map(|(y, r)| (y - r * rot).norm() as f64)
            .fold(0.0f64, f64::max);
        // Measured: 1.68e-4, which is -75 dB against a unit tone, on two cascaded
        // filters whose passband ripple is 1e-3 each. The bound is set from that
        // rather than from the ripple, because a round trip that came back at
        // the full ripple would mean the two stages were not cancelling and
        // something in the polyphase decomposition had gone wrong.
        assert!(err < 4e-4, "worst residual {err} against the ideal tone");
    }
}
