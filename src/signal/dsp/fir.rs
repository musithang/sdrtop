// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! FIR design and streaming decimation.
//!
//! Two things, and they are separate for a reason: [`design_lowpass`] answers
//! "what kernel", [`StreamingDecimator`] answers "apply it across block
//! boundaries without a seam". A caller that needs a filter but not a decimator
//! should not have to build one.
//!
//! **The window here shapes a filter kernel, not a transform input.** That is
//! the whole difference between this module and [`super::window`], which does
//! the other job under the same word.

use num_complex::Complex;

/// A windowed sinc low-pass, given the window as a function of tap index.
///
/// Both designs in this module go through here, which is what makes "the same
/// contract" a structural fact rather than a promise: unit DC gain, the -6 dB
/// point at `fc`, an odd tap count so the delay is a whole number of samples.
/// Only the window differs.
fn windowed_sinc(taps: usize, fc: f64, window: impl Fn(usize, usize) -> f64) -> Vec<f32> {
    use std::f64::consts::PI;
    let taps = taps.max(1) | 1;
    let m = (taps - 1) as f64 / 2.0;
    let mut h = Vec::with_capacity(taps);
    let mut sum = 0.0f64;
    for i in 0..taps {
        let x = i as f64 - m;
        // sinc, with the removable singularity at the centre tap handled exactly.
        let sinc = if x.abs() < 1e-9 {
            2.0 * fc
        } else {
            (2.0 * PI * fc * x).sin() / (PI * x)
        };
        let v = sinc * window(i, taps);
        sum += v;
        h.push(v);
    }
    // Normalise to unit DC gain so decimation does not change the level, and the
    // deviation figures stay in real Hz.
    if sum.abs() > 1e-12 {
        for v in h.iter_mut() {
            *v /= sum;
        }
    }
    h.into_iter().map(|v| v as f32).collect()
}

/// Hamming-windowed sinc low-pass. `fc` is the cutoff in cycles/sample (< 0.5).
///
/// **The cutoff convention is the one thing to get right here**, and it is the
/// classic source of a silent factor of two: `fc` is in cycles per sample, so
/// `fc = 0.25` is a quarter of the sample rate and half of Nyquist. The design
/// places its -6 dB point there, which is what
/// `the_cutoff_sits_where_the_argument_says_it_does` pins.
///
/// The stopband is whatever Hamming gives, about 53 dB, and no argument can
/// change that. A caller that needs to *specify* a stopband wants
/// [`design_lowpass_to_spec`].
pub fn design_lowpass(taps: usize, fc: f64) -> Vec<f32> {
    use std::f64::consts::PI;
    windowed_sinc(taps, fc, |i, taps| {
        0.54 - 0.46 * (2.0 * PI * i as f64 / (taps - 1).max(1) as f64).cos()
    })
}

/// The modified Bessel function of the first kind, order zero.
///
/// The series is the definition: `I0(x) = sum over k of ((x/2)^k / k!)^2`. Each
/// term is the one before it times `(x / 2k)^2`, so nothing here evaluates a
/// factorial or a power, and the loop stops when a term no longer moves the sum.
/// Sixty-four terms cover every `beta` a filter design will ask for; `beta = 20`
/// is a 190 dB stopband and converges in about thirty.
/// No consumer yet outside this file; see [`design_lowpass_to_spec`].
#[allow(dead_code)]
fn bessel_i0(x: f64) -> f64 {
    let mut term = 1.0f64;
    let mut sum = 1.0f64;
    for k in 1..=64 {
        term *= (x / (2.0 * k as f64)).powi(2);
        sum += term;
        if term < sum * 1e-17 {
            break;
        }
    }
    sum
}

/// Kaiser's shape parameter for a required stopband attenuation.
///
/// `stopband_db` is Kaiser's `A = -20 log10(delta)`, where `delta` is the peak
/// approximation error. Kaiser's design makes the passband ripple and the
/// stopband ripple the same `delta`, so asking for 60 dB of stopband also asks
/// for a passband flat to about 0.0087 dB.
///
/// Source: Kaiser, "Nonrecursive Digital Filter Design Using the I0-Sinh Window
/// Function", Proc. 1974 IEEE Int. Symp. Circuits and Systems, pp. 20-23. The
/// same piecewise form appears in Oppenheim and Schafer, Discrete-Time Signal
/// Processing, pp. 475-476.
///
/// Below 21 dB the window is rectangular. That is not a special case bolted on:
/// truncating the sinc alone already gives about 21 dB, so there is nothing left
/// for a window to do.
/// No production consumer yet; see [`design_lowpass_to_spec`].
#[allow(dead_code)]
pub fn kaiser_beta(stopband_db: f64) -> f64 {
    if stopband_db > 50.0 {
        0.1102 * (stopband_db - 8.7)
    } else if stopband_db >= 21.0 {
        let x = stopband_db - 21.0;
        0.5842 * x.powf(0.4) + 0.07886 * x
    } else {
        0.0
    }
}

/// Tap count for a required stopband attenuation and transition width.
///
/// `transition` is the width of the transition band in cycles per sample, the
/// unit [`design_lowpass`] takes its cutoff in. The band is centred on the
/// cutoff: it runs from `fc - transition/2` to `fc + transition/2`.
///
/// Source: Kaiser (1974), `M = (A - 7.95) / (2.285 * dw)` for the order, with
/// `dw` the transition width in radians per sample, so `dw = 2*pi*transition`.
/// Oppenheim and Schafer round the 7.95 to 8; this uses Kaiser's own figure. The
/// tap count is the order plus one, rounded up and forced odd.
///
/// The two constants are not distinguishable by measurement here: substituting 8
/// for 7.95 changes the estimate by a fifth of a tap at a transition of 0.02
/// cycles per sample, and every test in this module still passes. The primary
/// source is therefore the only reason to prefer one, which is reason enough.
///
/// **The estimate is an estimate**, and Kaiser never claimed otherwise, which is
/// why `the_requested_stopband_is_delivered` measures the filter that comes out
/// rather than trusting the count that went in.
///
/// Below 21 dB the count is evaluated at 21 dB: that is where beta bottoms out
/// at a rectangular window, and below it the formula has nothing to say. There
/// is no upper clamp. A transition of a millionth of the sample rate really does
/// need millions of taps, and whether that is affordable is the caller's
/// question, not this function's to answer with a number nobody asked for.
/// No production consumer yet; see [`design_lowpass_to_spec`].
#[allow(dead_code)]
pub fn kaiser_taps(transition: f64, stopband_db: f64) -> usize {
    use std::f64::consts::TAU;
    if !transition.is_finite() || transition <= 0.0 || !stopband_db.is_finite() {
        return 1;
    }
    let order = (stopband_db.max(21.0) - 7.95) / (2.285 * TAU * transition);
    ((order.ceil() as usize).saturating_add(1)).max(1) | 1
}

/// Kaiser-windowed sinc low-pass, with the window shape given directly.
///
/// Same contract as [`design_lowpass`]: `fc` in cycles per sample, unit DC gain,
/// -6 dB at `fc`. `beta` comes from [`kaiser_beta`], and pairing it with a tap
/// count from [`kaiser_taps`] for the *same* attenuation is the caller's job.
/// [`design_lowpass_to_spec`] exists so that job can be skipped.
/// No production consumer yet; see [`design_lowpass_to_spec`], which calls
/// this, and [`super::resample::Resampler::new`], which calls the same three
/// primitives directly rather than through it.
#[allow(dead_code)]
pub fn design_lowpass_kaiser(taps: usize, fc: f64, beta: f64) -> Vec<f32> {
    let denom = bessel_i0(beta);
    windowed_sinc(taps, fc, |i, taps| {
        let m = (taps - 1) as f64 / 2.0;
        if m <= 0.0 {
            return 1.0;
        }
        let r = (i as f64 - m) / m;
        // Clamped because the endpoints can land a few ulps outside the unit
        // interval, and a negative square root here would be a NaN in the kernel
        // rather than an error anyone could see.
        bessel_i0(beta * (1.0 - r * r).max(0.0).sqrt()) / denom
    })
}

/// The design rule, as one call: ask for a stopband and a transition width, get
/// a filter that meets them.
///
/// This is the reason N3 exists as a step. Hamming hands out 53 dB and no
/// argument changes it; a channel filter and a decimator do not want the same
/// rejection, and neither should have to take the one number a fixed window
/// happens to give. Pairing the shape with the length is done here because a
/// mismatched pair meets neither specification and looks perfectly reasonable
/// while doing it.
///
/// Measured, at `fc = 0.1` and a transition of 0.02 cycles per sample: a request
/// for 40 dB comes back as 39.92 dB in 113 taps, 60 dB as 60.08 dB in 183, and
/// 80 dB as 79.96 dB in 253. The design rule is that close to calibrated, which
/// is why `the_requested_stopband_is_delivered` holds it to half a dB either
/// way rather than only checking that the filter is good enough.
/// **No production consumer yet.** Design section 12.3's resample case - "the
/// device's rate is a rational multiple of what a mode needs" - is the
/// identified future need; no arc has reached the point of building a feed at a
/// rate the radio cannot produce directly.
#[allow(dead_code)]
pub fn design_lowpass_to_spec(fc: f64, transition: f64, stopband_db: f64) -> Vec<f32> {
    design_lowpass_kaiser(
        kaiser_taps(transition, stopband_db),
        fc,
        kaiser_beta(stopband_db),
    )
}

/// A Gaussian low-pass kernel, for pulse-shaping a GFSK/GMSK modulator.
///
/// `bt` is the filter's bandwidth-time product - its own -3 dB bandwidth times
/// the symbol period - which is the number a specification actually states
/// (0.5 for Bluetooth BR and for BLE; see the arc documents for the clause).
/// `sps` is samples per symbol and `span_symbols` is how many symbol periods
/// the kernel spans, centred on zero. `sps * span_symbols` is forced odd, the
/// same convention [`windowed_sinc`] uses, so the group delay is a whole
/// number of samples.
///
/// Closed form: `alpha = sqrt(ln 2 / 2) / bt`, `h(t) = (sqrt(pi) / alpha) *
/// exp(-(pi t / alpha)^2)` for `t` in symbol periods, normalised to unit sum
/// so filtering a constant NRZ run leaves its level unchanged - the same
/// DC-gain convention [`design_lowpass`] uses, for the same reason: a
/// modulator's deviation is set by that level, not by the filter's own gain.
/// This is the standard Gaussian pulse for GFSK/GMSK (the closed form MATLAB's
/// Communications Toolbox `gaussdesign` documents); the mathematics is not
/// itself a specification clause, only `bt` is, and
/// `the_dash_3db_point_sits_at_bt_over_sps` measures the filter this produces
/// rather than trusting the formula on faith.
///
/// No production consumer yet: `signal::ble::testkit`'s synthetic transmitter
/// is a test-only fixture, so this is presently only reached from `cfg(test)`
/// code, the same shape as [`design_lowpass_to_spec`] above.
#[allow(dead_code)]
pub fn gaussian_taps(bt: f64, sps: usize, span_symbols: usize) -> Vec<f32> {
    use std::f64::consts::PI;
    let n = (sps * span_symbols).max(1) | 1;
    let m = (n - 1) as f64 / 2.0;
    let alpha = (2f64.ln() / 2.0).sqrt() / bt;
    let mut h = Vec::with_capacity(n);
    let mut sum = 0.0f64;
    for i in 0..n {
        let t = (i as f64 - m) / sps as f64;
        let v = (PI.sqrt() / alpha) * (-(PI * t / alpha).powi(2)).exp();
        sum += v;
        h.push(v);
    }
    if sum.abs() > 1e-12 {
        for v in h.iter_mut() {
            *v /= sum;
        }
    }
    h.into_iter().map(|v| v as f32).collect()
}

/// A decimating FIR that keeps its state between calls, so successive blocks
/// produce one seamless output stream.
///
/// A filter restarted at each block discards its first `taps` samples and resets
/// the decimation grid, which puts a small timing step at every block boundary.
/// Deviation statistics never notice; a narrowband tone detector does, because
/// its window spans several blocks and a phase step inside it destroys the
/// coherence the detection depends on. Anything that measures across a block
/// boundary needs this rather than a stateless filter.
pub struct StreamingDecimator {
    taps: Vec<f32>,
    d: usize,
    /// Input samples carried over so the next block's first output can see the
    /// full filter history.
    tail: Vec<Complex<f32>>,
    /// Where the decimation grid resumes inside the next block.
    phase: usize,
}

impl StreamingDecimator {
    pub fn new(taps: Vec<f32>, d: usize) -> Self {
        Self {
            taps,
            d: d.max(1),
            tail: Vec::new(),
            phase: 0,
        }
    }

    /// Forget the carried state - after a dropped block, or a parameter change.
    /// The next output block starts a fresh contiguous run.
    pub fn reset(&mut self) {
        self.tail.clear();
        self.phase = 0;
    }

    pub fn process(&mut self, input: &[Complex<f32>], out: &mut Vec<Complex<f32>>) {
        out.clear();
        let n = self.taps.len();
        if n == 0 || input.is_empty() {
            return;
        }

        // Splice the carried history in front of the new samples.
        let mut buf = std::mem::take(&mut self.tail);
        buf.extend_from_slice(input);
        if buf.len() < n {
            self.tail = buf;
            return;
        }

        let mut start = self.phase;
        while start + n <= buf.len() {
            let w = &buf[start..start + n];
            let mut acc = Complex {
                re: 0.0f32,
                im: 0.0f32,
            };
            for (s, &h) in w.iter().zip(self.taps.iter()) {
                acc.re += s.re * h;
                acc.im += s.im * h;
            }
            out.push(acc);
            start += self.d;
        }

        // Keep the samples the next output still needs, and remember where the
        // grid stands relative to them. When the stride overshoots the buffer
        // entirely, the leftover stride carries into the next block as phase.
        let consumed = start.min(buf.len());
        buf.drain(..consumed);
        self.phase = start - consumed;
        self.tail = buf;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    /// Magnitude response at `f` cycles/sample. The kernel is real, so this is
    /// the plain DTFT sum and needs nothing from the FFT.
    fn response(h: &[f32], f: f64) -> f64 {
        let (mut re, mut im) = (0.0, 0.0);
        for (n, &c) in h.iter().enumerate() {
            let ph = -2.0 * PI * f * n as f64;
            re += c as f64 * ph.cos();
            im += c as f64 * ph.sin();
        }
        (re * re + im * im).sqrt()
    }

    fn db(x: f64) -> f64 {
        20.0 * x.log10()
    }

    /// A complex tone at `f` cycles/sample, unit amplitude.
    fn tone(f: f64, n: usize) -> Vec<Complex<f32>> {
        (0..n)
            .map(|i| {
                let ph = 2.0 * PI * f * i as f64;
                Complex {
                    re: ph.cos() as f32,
                    im: ph.sin() as f32,
                }
            })
            .collect()
    }

    /// The window method's own textbook figures, which is what makes this a
    /// design rather than a kernel that happened to work.
    ///
    /// A Hamming-windowed sinc has three published properties, none of which
    /// depends on the tap count: a transition width of `3.3 / N` normalised to
    /// the sample rate, at least 53 dB of stopband attenuation beyond it, and a
    /// passband ripple of about 0.02 dB. Asserting those rather than "the
    /// response is small somewhere out there" is what would catch a wrong window
    /// coefficient, a missing normalisation, or a sinc that is off by a factor
    /// of two.
    #[test]
    fn the_window_method_meets_its_own_textbook_figures() {
        for (taps, fc) in [(129usize, 0.05f64), (65, 0.1), (511, 0.0125)] {
            let h = design_lowpass(taps, fc);

            // Passband, over the flat half of it. The shoulder near the cutoff
            // is the transition and is measured by the width rule below, not
            // here.
            let (mut lo, mut hi) = (f64::INFINITY, 0.0f64);
            for k in 0..=200 {
                let m = response(&h, 0.5 * fc * k as f64 / 200.0);
                lo = lo.min(m);
                hi = hi.max(m);
            }
            assert!(
                db(hi / lo) < 0.0194,
                "taps={taps} fc={fc}: passband ripple {:.4} dB exceeds Hamming's 0.0194",
                db(hi / lo)
            );

            // Stopband, from one transition width above the cutoff to Nyquist.
            let edge = fc + 3.3 / taps as f64;
            let mut worst = 0.0f64;
            let mut f = edge;
            while f <= 0.5 {
                worst = worst.max(response(&h, f));
                f += 1e-4;
            }
            assert!(
                db(worst) <= -53.0,
                "taps={taps} fc={fc}: stopband {:.2} dB is worse than Hamming's -53",
                db(worst)
            );
        }
    }

    /// The cutoff convention, which is the one thing in a filter API that goes
    /// wrong silently.
    ///
    /// `fc` is cycles per sample, so a windowed sinc puts its **-6 dB** point
    /// exactly there. A design that took `fc` as a fraction of Nyquist instead
    /// would put it at `fc / 2` and every filter in the app would be twice as
    /// narrow as its caller believed, with nothing to show for it but a slightly
    /// quiet signal.
    #[test]
    fn the_cutoff_sits_where_the_argument_says_it_does() {
        for (taps, fc) in [(129usize, 0.05f64), (65, 0.1), (511, 0.0125)] {
            let h = design_lowpass(taps, fc);
            let at_fc = response(&h, fc);
            assert!(
                (at_fc - 0.5).abs() < 0.01,
                "taps={taps} fc={fc}: |H(fc)| = {at_fc:.4}, the -6 dB point is elsewhere"
            );
        }
    }

    /// Unit DC gain, so decimating never changes a level and the figures
    /// downstream stay in real units.
    #[test]
    fn a_lowpass_has_unit_dc_gain() {
        for (taps, fc) in [(65usize, 0.05f64), (31, 0.2), (511, 0.01)] {
            let dc: f32 = design_lowpass(taps, fc).iter().sum();
            assert!((dc - 1.0).abs() < 1e-4, "taps={taps}: DC gain = {dc}");
        }
    }

    /// An impulse in gives the kernel back, which is the definition of the
    /// thing and pins two mistakes at once.
    ///
    /// The inner loop pairs `taps[k]` with `window[k]` rather than with
    /// `window[n-1-k]`, so it is a correlation and not a convolution. For a
    /// symmetric kernel those are the same, and this asserts both halves of
    /// that: the response is the kernel, **and** the kernel is symmetric. Break
    /// the symmetry and the two stop agreeing, which is exactly when a
    /// correlation dressed as a convolution starts mattering.
    #[test]
    fn an_impulse_comes_back_out_as_the_kernel() {
        let h = design_lowpass(65, 0.1);
        let n = h.len();
        for (i, &t) in h.iter().enumerate() {
            assert!(
                (t - h[n - 1 - i]).abs() < 1e-7,
                "tap {i} breaks symmetry: {t} vs {}",
                h[n - 1 - i]
            );
        }

        let mut input = vec![
            Complex {
                re: 0.0f32,
                im: 0.0
            };
            2 * n
        ];
        input[n - 1] = Complex { re: 1.0, im: 0.0 };
        let mut out = Vec::new();
        StreamingDecimator::new(h.clone(), 1).process(&input, &mut out);

        assert!(out.len() > n, "not enough output to see the whole kernel");
        for (i, tap) in h.iter().enumerate() {
            assert!(
                (out[i].re - h[n - 1 - i]).abs() < 1e-6 && out[i].im.abs() < 1e-6,
                "sample {i} is {} and should be {tap}",
                out[i]
            );
        }
    }

    /// A constant in is the same constant out, at any decimation. This is unit
    /// DC gain observed through the decimator rather than asserted on the taps,
    /// which is where a caller would actually notice it going wrong.
    #[test]
    fn a_constant_survives_decimation_unchanged() {
        let h = design_lowpass(63, 0.05);
        for d in [1usize, 2, 8, 40] {
            let input = vec![
                Complex {
                    re: 0.25f32,
                    im: -0.75
                };
                4096
            ];
            let mut out = Vec::new();
            StreamingDecimator::new(h.clone(), d).process(&input, &mut out);
            assert!(!out.is_empty(), "d={d} produced nothing");
            for (i, v) in out.iter().enumerate() {
                assert!(
                    (v.re - 0.25).abs() < 1e-4 && (v.im + 0.75).abs() < 1e-4,
                    "d={d} sample {i} is {v}"
                );
            }
        }
    }

    /// A block shorter than the filter cannot produce an output, and must carry
    /// forward rather than emit a half-warmed sample.
    #[test]
    fn decimating_is_a_noop_when_the_input_is_shorter_than_the_filter() {
        let mut sd = StreamingDecimator::new(design_lowpass(63, 0.1), 4);
        let mut out = Vec::new();
        sd.process(&tone(0.01, 10), &mut out);
        assert!(out.is_empty());
    }

    /// **The property every measurement spanning a block boundary rests on.**
    /// The same samples, delivered whole or in ragged pieces, must give the same
    /// output: same values, same count, no timing step at the seams. None of the
    /// piece sizes is a multiple of the decimation factor, which is the case
    /// that exercises the carried phase.
    #[test]
    fn feeding_in_ragged_pieces_matches_one_long_block() {
        let d = 8;
        let taps = design_lowpass(133, 0.4 / d as f64);
        let iq = tone(0.0025, 1 << 15);

        let mut whole = Vec::new();
        StreamingDecimator::new(taps.clone(), d).process(&iq, &mut whole);

        for pieces in [3_001usize, 101, 7, 999, 4_097, 1, 65_536] {
            let mut sd = StreamingDecimator::new(taps.clone(), d);
            let (mut pieced, mut part) = (Vec::new(), Vec::new());
            for chunk in iq.chunks(pieces) {
                sd.process(chunk, &mut part);
                pieced.extend_from_slice(&part);
            }
            assert_eq!(
                pieced.len(),
                whole.len(),
                "chunk size {pieces}: sample count diverged"
            );
            for (i, (a, b)) in pieced.iter().zip(whole.iter()).enumerate() {
                assert!(
                    (a - b).norm() < 1e-4,
                    "chunk size {pieces}, sample {i}: {a} vs {b}"
                );
            }
        }
    }

    /// After a reset the filter has no history, so it re-warms exactly as it did
    /// the first time rather than splicing onto stale samples.
    #[test]
    fn a_reset_starts_a_fresh_run() {
        let mut sd = StreamingDecimator::new(design_lowpass(31, 0.1), 4);
        let iq = tone(0.005, 4096);
        let (mut a, mut b) = (Vec::new(), Vec::new());
        sd.process(&iq, &mut a);
        sd.reset();
        sd.process(&iq, &mut b);
        assert_eq!(a.len(), b.len());
        for (i, (x, y)) in a.iter().zip(b.iter()).enumerate() {
            assert!((x - y).norm() < 1e-6, "sample {i}: {x} vs {y}");
        }
    }

    /// Standard tabulated values of I0. The series is the function's own
    /// definition, so what is at risk here is not the mathematics but the loop:
    /// an off-by-one in the term recurrence, or a break that fires before the
    /// sum has settled.
    #[test]
    fn the_bessel_function_matches_its_table() {
        for (x, want) in [
            (0.0, 1.0),
            (1.0, 1.2660658777520084),
            (2.0, 2.2795853023360673),
            (3.75, 9.118945860844565),
            (5.0, 27.23987182360445),
            (10.0, 2815.716628466255),
        ] {
            let got = bessel_i0(x);
            assert!(
                (got - want).abs() <= want * 1e-12,
                "I0({x}) came out {got}, the table says {want}"
            );
        }
    }

    /// Peak response anywhere in the stopband, in dB. The transition band is
    /// centred on the cutoff, so the stopband starts half a transition above it.
    /// Sampled finely, because the worst ripple sits right at that edge.
    fn stopband_peak_db(h: &[f32], fc: f64, transition: f64) -> f64 {
        let start = fc + transition / 2.0;
        let mut peak = 0.0f64;
        for k in 0..=4000 {
            let f = start + (0.5 - start) * k as f64 / 4000.0;
            peak = peak.max(response(h, f));
        }
        db(peak)
    }

    /// N3's exit condition: ask for a stopband, get one.
    #[test]
    fn the_requested_stopband_is_delivered() {
        for a in [40.0, 60.0, 80.0] {
            let (fc, t) = (0.1, 0.02);
            let h = design_lowpass_to_spec(fc, t, a);
            let got = -stopband_peak_db(&h, fc, t);
            // Two-sided on purpose. Falling short means the filter does not do
            // what the caller asked; overshooting means the tap estimate is
            // paying for attenuation nobody wanted, and at these widths that is
            // just as much a defect in a design rule.
            assert!(
                (got - a).abs() <= 0.5,
                "asked for {a} dB, measured {got:.2} dB with {} taps",
                h.len()
            );
        }
    }

    /// The transition really is the width that was requested, measured at both
    /// of its edges. Kaiser's design makes the passband ripple equal the
    /// stopband ripple, so the lower edge answers to the same delta as the
    /// upper one, and asserting both is what pins the width.
    #[test]
    fn the_transition_lands_between_the_edges_it_was_given() {
        for a in [40.0, 60.0, 80.0] {
            let (fc, t) = (0.1, 0.02);
            let h = design_lowpass_to_spec(fc, t, a);
            let delta = 10f64.powf(-a / 20.0);
            let pass = response(&h, fc - t / 2.0);
            assert!(
                (pass - 1.0).abs() <= 2.0 * delta,
                "a={a}: passband edge is {pass}, off by more than twice delta {delta:.2e}"
            );
            let stop = db(response(&h, fc + t / 2.0));
            assert!(
                stop <= -a + 2.0,
                "a={a}: stopband edge is only {stop:.2} dB down"
            );
        }
    }

    /// Both designs go through `windowed_sinc`, so this is a check that the
    /// shared path was not broken for one window while working for the other:
    /// unit DC gain and the -6 dB point at the cutoff, the same two properties
    /// `a_lowpass_has_unit_dc_gain` and `the_cutoff_sits_where_the_argument_
    /// says_it_does` hold for Hamming.
    #[test]
    fn kaiser_answers_to_the_same_contract_as_hamming() {
        for (taps, fc, a) in [
            (129usize, 0.05f64, 60.0f64),
            (257, 0.1, 80.0),
            (65, 0.2, 40.0),
        ] {
            let h = design_lowpass_kaiser(taps, fc, kaiser_beta(a));
            let dc = response(&h, 0.0);
            assert!((dc - 1.0).abs() < 1e-6, "taps={taps} a={a}: DC gain {dc}");
            let at_fc = response(&h, fc);
            assert!(
                (at_fc - 0.5).abs() < 0.01,
                "taps={taps} a={a}: |H(fc)| = {at_fc:.4}, the -6 dB point is elsewhere"
            );
        }
    }

    /// Asked for what Hamming gives, Kaiser gives the same kind of object. This
    /// is the sanity check that the two designs are comparable at all, not a
    /// claim that they are identical: they are different windows and their
    /// stopbands have different shapes.
    #[test]
    fn kaiser_at_hammings_attenuation_is_hammings_kind_of_filter() {
        let (taps, fc) = (161usize, 0.1f64);
        let t = 3.3 / taps as f64;
        let hamming = -stopband_peak_db(&design_lowpass(taps, fc), fc, t);
        let kaiser = -stopband_peak_db(&design_lowpass_kaiser(taps, fc, kaiser_beta(53.0)), fc, t);
        // Measured: 51.46 dB for Hamming, 52.72 dB for Kaiser asked for 53.
        assert!(
            (hamming - kaiser).abs() < 3.0,
            "hamming gives {hamming:.2} dB and kaiser asked for 53 gives {kaiser:.2} dB"
        );
    }

    /// A Gaussian kernel has unit sum (DC gain), the way every filter in this
    /// module normalises, so a constant NRZ run through it survives at level.
    #[test]
    fn a_gaussian_kernel_has_unit_dc_gain() {
        for (bt, sps, span) in [(0.5f64, 4usize, 4usize), (0.3, 8, 6), (1.0, 4, 2)] {
            let h = gaussian_taps(bt, sps, span);
            let dc: f32 = h.iter().sum();
            assert!((dc - 1.0).abs() < 1e-4, "bt={bt} sps={sps}: DC gain = {dc}");
        }
    }

    /// Symmetric, the way an odd-length linear-phase kernel must be, so
    /// filtering introduces a whole-sample delay and nothing else.
    #[test]
    fn a_gaussian_kernel_is_symmetric() {
        let h = gaussian_taps(0.5, 4, 4);
        let n = h.len();
        assert_eq!(n % 2, 1, "kernel length must be odd");
        for (i, &t) in h.iter().enumerate() {
            assert!((t - h[n - 1 - i]).abs() < 1e-7, "tap {i} breaks symmetry");
        }
    }

    /// The bandwidth-time product's own definition, measured rather than
    /// assumed: `bt`'s -3 dB point sits at `bt / sps` cycles per sample, since
    /// a symbol period is `sps` samples and `bt` is bandwidth times symbol
    /// period.
    #[test]
    fn the_dash_3db_point_sits_at_bt_over_sps() {
        for (bt, sps) in [(0.5f64, 8usize), (0.3, 8), (0.7, 4)] {
            let h = gaussian_taps(bt, sps, 8);
            let at = response(&h, bt / sps as f64);
            assert!(
                (at - std::f64::consts::FRAC_1_SQRT_2).abs() < 0.1,
                "bt={bt} sps={sps}: |H(bt/sps)| = {at:.4}, expected about -3 dB (0.707)"
            );
        }
    }

    /// A wider `bt` is a wider filter: at a frequency fixed in cycles per
    /// sample, the response only grows as `bt` grows. This is the ordering
    /// invariant a wrong sign or an inverted formula would break even if the
    /// -3 dB point happened to land right by coincidence.
    #[test]
    fn a_wider_bt_passes_more_at_a_fixed_frequency() {
        let sps = 8;
        let f = 0.05;
        let mut last = 0.0;
        for bt in [0.2f64, 0.4, 0.6, 0.8, 1.0] {
            let h = gaussian_taps(bt, sps, 8);
            let at = response(&h, f);
            assert!(at > last, "bt={bt}: response {at} did not grow from {last}");
            last = at;
        }
    }

    /// The tap count follows the two things it is a function of, and refuses a
    /// transition width that is not a width.
    #[test]
    fn the_tap_estimate_grows_with_the_specification() {
        assert!(kaiser_taps(0.02, 80.0) > kaiser_taps(0.02, 40.0));
        assert!(kaiser_taps(0.01, 60.0) > kaiser_taps(0.02, 60.0));
        assert_eq!(
            kaiser_taps(0.02, 60.0) % 2,
            1,
            "the tap count must stay odd"
        );
        for bad in [0.0, -0.01, f64::NAN] {
            assert_eq!(kaiser_taps(bad, 60.0), 1);
        }
    }
}
