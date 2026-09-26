// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Numerically controlled oscillator, and the complex mixer built on it.
//!
//! **The phase is a 64-bit integer, not a float, and that is the entire point.**
//! An accumulator that adds a floating-point increment to a floating-point phase
//! loses a little of the increment at every step, and loses more of it as the
//! phase grows: the error is not noise, it is a slow frequency error that only
//! shows itself after a few million samples, by which time whatever it was
//! measuring has been quietly wrong for a while. An integer accumulator wraps
//! exactly at one turn, so the only error in the generated frequency is the one
//! introduced when the increment was computed, once, at the start. It does not
//! accumulate. That is why [`Nco::frequency_hz`] can state what is being
//! generated rather than what was asked for.
//!
//! The difference was measured rather than assumed: replacing the accumulator
//! with a growing `f64` phase in radians, and changing nothing else, wanders
//! 4.2 microradians from the closed form over a million samples at 1.23 MHz on
//! a 20 Msps grid. `the_tone_does_not_drift_over_a_million_samples` holds a
//! bound four thousand times tighter than that.
//!
//! Conventions, all three of which are the classic places to lose a sign:
//!
//! * Frequency is signed. Positive is counter-clockwise, `exp(+j2*pi*f*t)`.
//!   **Mixing a signal at `+f` down to baseband therefore takes an oscillator at
//!   `-f`**, and that is the caller's decision to spell out, not this module's to
//!   guess.
//! * Phase runs in turns, not radians, everywhere inside. One turn is the full
//!   circle and is exactly `2^64` accumulator units. Radians appear only at the
//!   last step, where the sample is generated.
//! * [`Nco::next_sample`] returns the sample *at* the current phase and then
//!   advances, so the first sample out of a fresh oscillator is exactly `1 + 0j`.
//!
//! [`Nco::sample`] costs one `sin_cos`, in `f64`: the accurate choice. **The
//! block methods, [`Nco::mix`] and [`Nco::fill`], do not pay it per sample.**
//! They became a measured hot path: every classic Bluetooth channel mixes its
//! whole raw block, and `sin_cos` per raw sample was a fifth of the Classic
//! view's time on the i3. So a block is walked [`RESYNC`] samples at a time:
//! each run starts from the exact sample at the accumulator's phase, and
//! within it the oscillator is rotated by one fixed `f64` step. The rounding
//! of that recurrence grows by about one ulp a step, so a run ends within
//! about `RESYNC * 1e-16` of the exact value, and the next run starts exact
//! again: the integer accumulator still decides the phase, and nothing
//! accumulates across runs. `the_block_methods_match_the_exact_oscillator`
//! holds them to `sample` at frequencies that are no neat fraction of the
//! rate, across calls of awkward lengths.

use num_complex::Complex;
use std::f64::consts::TAU;

/// Samples between exact restarts of the block methods' rotation.
const RESYNC: usize = 1024;

/// One full turn, in accumulator units. The accumulator is `u64`, so a turn is
/// its whole range and wrapping is exact.
const TURN: f64 = 18_446_744_073_709_551_616.0; // 2^64

/// Phase increment for a given advance in turns per sample.
///
/// The split into two 32-bit halves is not decoration: `turns * 2^64` is not
/// representable as an `f64` near a full turn, and the saturating `as` cast
/// would silently hand back `u64::MAX` for a frequency just under Nyquist. Doing
/// the top half and the remainder separately keeps every bit the `f64` actually
/// carries, and the carry out of the low half lands where it belongs.
fn step_from_turns(turns_per_sample: f64) -> u64 {
    if !turns_per_sample.is_finite() {
        return 0;
    }
    const HALF: f64 = 4_294_967_296.0; // 2^32
    let turns = turns_per_sample.rem_euclid(1.0);
    let scaled = turns * HALF;
    let hi = scaled.floor();
    let lo = (scaled - hi) * HALF;
    ((hi as u64) << 32).wrapping_add(lo.round() as u64)
}

/// A phase-accumulator oscillator at a fixed sample rate.
///
/// **No production consumer yet.** Every DSP test in this crate that needs a
/// synthetic tone or a known frequency offset builds one, but nothing in the
/// running app has needed to *generate or mix out* a carrier: N16's reference
/// measurement reads an offset with `dsp::correlate`/`dsp::estimate` and
/// reports it, it does not correct for it. Mixing a measured offset out before
/// decode is an arc's job, once one exists.
#[allow(dead_code)]
pub struct Nco {
    /// Current phase in accumulator units, `2^64` to the turn.
    phase: u64,
    /// Phase advance per sample, two's complement, so a negative frequency is a
    /// wrapping subtraction and needs no branch.
    step: u64,
    sample_rate_hz: f64,
}

#[allow(dead_code)]
impl Nco {
    /// An oscillator at `freq_hz`, starting at zero phase.
    ///
    /// A sample rate that is not finite and positive cannot be honoured, so the
    /// oscillator stops: `step` is zero and [`Self::frequency_hz`] reports the
    /// 0 Hz it is actually generating rather than the frequency it was asked
    /// for. Nothing here invents a rate to carry on with.
    pub fn new(freq_hz: f64, sample_rate_hz: f64) -> Self {
        let mut nco = Self {
            phase: 0,
            step: 0,
            sample_rate_hz: if sample_rate_hz.is_finite() && sample_rate_hz > 0.0 {
                sample_rate_hz
            } else {
                0.0
            },
        };
        nco.set_frequency_hz(freq_hz);
        nco
    }

    /// Retune without disturbing the phase, which is what keeps a retune from
    /// putting a step into a stream that is being measured across the change.
    pub fn set_frequency_hz(&mut self, freq_hz: f64) {
        self.step = if self.sample_rate_hz > 0.0 {
            step_from_turns(freq_hz / self.sample_rate_hz)
        } else {
            0
        };
    }

    /// The frequency actually generated, signed, in `(-fs/2, fs/2]`.
    ///
    /// This is the number to display, and it is fixed at the moment of tuning:
    /// the accumulator does not add to it over time. Reading the increment back
    /// as a signed integer rather than subtracting a turn from a fraction is
    /// deliberate: `step / 2^64` for a negative frequency is a value just under
    /// 1, whose `f64` ulp is sixteen times coarser than the small number the
    /// subtraction is about to produce.
    pub fn frequency_hz(&self) -> f64 {
        (self.step as i64 as f64) / TURN * self.sample_rate_hz
    }

    /// The tuning step: one accumulator unit expressed in Hz.
    ///
    /// **This is finer than the `f64` the caller tuned with.** At 20 Msps a unit
    /// is about a picohertz, while an `f64` holding 2.4 GHz cannot resolve
    /// better than half a microhertz, so a tuning round trip answers to the
    /// float, not to this number. Sixty-four bits are not here to make one tune
    /// exact; they are here so that a million samples later the phase is still
    /// where the closed form says it is, which is what
    /// `the_tone_does_not_drift_over_a_million_samples` measures.
    pub fn resolution_hz(&self) -> f64 {
        self.sample_rate_hz / TURN
    }

    /// Current phase in turns, `[0, 1)`.
    pub fn phase_turns(&self) -> f64 {
        (self.phase as f64) / TURN
    }

    pub fn set_phase_turns(&mut self, turns: f64) {
        self.phase = step_from_turns(turns);
    }

    /// Back to zero phase. The frequency is untouched.
    pub fn reset(&mut self) {
        self.phase = 0;
    }

    /// The sample at the current phase, without advancing.
    pub fn sample(&self) -> Complex<f64> {
        let (sin, cos) = (self.phase_turns() * TAU).sin_cos();
        Complex::new(cos, sin)
    }

    /// Advance one sample without generating anything.
    pub fn advance(&mut self) {
        self.phase = self.phase.wrapping_add(self.step);
    }

    pub fn next_sample(&mut self) -> Complex<f64> {
        let s = self.sample();
        self.advance();
        s
    }

    /// Walk `len` samples from the current phase, handing each oscillator
    /// value to `each`, and advance the phase past them: exact at every
    /// [`RESYNC`]th sample, a rotation by one fixed step in between (see the
    /// module's own note).
    fn walk(&mut self, len: usize, mut each: impl FnMut(usize, Complex<f64>)) {
        let step = (self.step as f64) / TURN * TAU;
        let rotation = Complex::new(step.cos(), step.sin());
        let mut done = 0;
        while done < len {
            let run = RESYNC.min(len - done);
            let mut o = self.sample();
            for k in done..done + run {
                each(k, o);
                o *= rotation;
            }
            self.phase = self.phase.wrapping_add(self.step.wrapping_mul(run as u64));
            done += run;
        }
    }

    /// Fill a block with the oscillator, continuing from the current phase.
    pub fn fill(&mut self, out: &mut [Complex<f32>]) {
        self.walk(out.len(), |k, o| {
            out[k] = Complex::new(o.re as f32, o.im as f32)
        });
    }

    /// Multiply a block by the oscillator, in place.
    ///
    /// The product is formed in `f32`, which is the width the sample stream
    /// already has; the oscillator itself stays in `f64` so the phase reference
    /// the product is measured against is better than the samples being mixed.
    pub fn mix(&mut self, block: &mut [Complex<f32>]) {
        self.walk(block.len(), |k, o| {
            let (c, q) = (o.re as f32, o.im as f32);
            let s = block[k];
            block[k] = Complex::new(s.re * c - s.im * q, s.re * q + s.im * c);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The block methods are the oscillator**, to within what a run of
    /// [`RESYNC`] rotations can round: at frequencies that are no neat
    /// fraction of the rate, positive and negative, over calls whose lengths
    /// straddle a resync, and the phase they leave is the one `advance`
    /// would have.
    #[test]
    fn the_block_methods_match_the_exact_oscillator() {
        for f in [1_234_567.0, -3_000_000.0, 7_999_999.5, -123.25, 0.0] {
            let mut exact = Nco::new(f, FS);
            let mut fast = Nco::new(f, FS);
            let mut worst = 0.0f64;
            for len in [1usize, 1023, 1024, 1025, 5000, 7] {
                let mut out = vec![Complex::new(0.0f32, 0.0); len];
                fast.fill(&mut out);
                for o in &out {
                    let e = exact.next_sample();
                    worst = worst.max((f64::from(o.re) - e.re).hypot(f64::from(o.im) - e.im));
                }
            }
            // The f32 output's own rounding dominates; the recurrence adds
            // far less.
            assert!(worst < 2e-7, "{f} Hz: {worst:e}");
            assert_eq!(fast.phase, exact.phase, "{f} Hz");

            // Mixing is multiplying by the same values.
            let mut a = Nco::new(f, FS);
            let mut b = Nco::new(f, FS);
            let mut block: Vec<Complex<f32>> = (0..3000)
                .map(|k| Complex::new((k as f32).sin(), 0.5))
                .collect();
            let input = block.clone();
            a.mix(&mut block);
            for (x, y) in input.iter().zip(&block) {
                let o = b.next_sample();
                let want = Complex::new(f64::from(x.re), f64::from(x.im)) * o;
                assert!(
                    (f64::from(y.re) - want.re).hypot(f64::from(y.im) - want.im) < 2e-6,
                    "{f} Hz"
                );
            }
        }
    }

    const FS: f64 = 20_000_000.0;
    /// Deliberately not a neat fraction of the sample rate: a frequency that
    /// divides evenly would hide a rounding error in the phase increment.
    const F: f64 = 1_234_567.0;

    /// Wrap a phase difference into `(-pi, pi]`.
    fn wrap(x: f64) -> f64 {
        let mut x = x % TAU;
        if x > std::f64::consts::PI {
            x -= TAU;
        }
        if x <= -std::f64::consts::PI {
            x += TAU;
        }
        x
    }

    #[test]
    fn the_requested_frequency_survives_quantisation() {
        for f in [0.0, 1.0, F, -F, FS / 4.0, -FS / 3.0, 9_999_999.5] {
            let nco = Nco::new(f, FS);
            let err = (nco.frequency_hz() - f).abs();
            // The accumulator is finer than the float that carried the request,
            // so the round trip answers to one ulp of the frequency itself. See
            // `Nco::resolution_hz`.
            let bound = nco.resolution_hz() + f.abs() * f64::EPSILON;
            assert!(
                err <= bound,
                "{f} Hz came back as {} Hz, error {err} against a bound of {bound}",
                nco.frequency_hz()
            );
        }
    }

    #[test]
    fn the_tuning_resolution_is_the_accumulators_own() {
        let nco = Nco::new(F, FS);
        assert!((nco.resolution_hz() - FS / TURN).abs() < f64::EPSILON);
        // Under a picohertz at 20 Msps, which is what 64 bits buys.
        assert!(nco.resolution_hz() < 1e-11);
    }

    #[test]
    fn a_rate_that_cannot_be_honoured_stops_the_oscillator() {
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let mut nco = Nco::new(F, bad);
            assert_eq!(nco.frequency_hz(), 0.0);
            let first = nco.next_sample();
            let later = nco.next_sample();
            assert_eq!(first, later, "a stopped oscillator must not turn");
        }
    }

    #[test]
    fn the_generated_tone_advances_by_the_frequency_each_sample() {
        let mut nco = Nco::new(F, FS);
        let n = 100_000;
        let mut prev = nco.next_sample();
        // Compensated summation, because a naive one is not good enough to make
        // this measurement: a hundred thousand steps of 0.39 rad reach a total
        // near 39000, whose ulp is large enough that the rounding of the sum
        // alone lands a few microhertz away from the answer. The oscillator was
        // never the limit here; the arithmetic doing the measuring was.
        let (mut sum, mut carry) = (0.0f64, 0.0f64);
        for _ in 1..n {
            let s = nco.next_sample();
            let d = wrap((s / prev).arg()) - carry;
            let t = sum + d;
            carry = (t - sum) - d;
            sum = t;
            prev = s;
        }
        let recovered_hz = sum / (n - 1) as f64 / TAU * FS;
        assert!(
            (recovered_hz - F).abs() < 1e-6,
            "measured {recovered_hz} Hz for a requested {F} Hz"
        );
    }

    #[test]
    fn the_oscillator_stays_on_the_unit_circle() {
        let mut nco = Nco::new(F, FS);
        for _ in 0..10_000 {
            let s = nco.next_sample();
            assert!((s.norm() - 1.0).abs() < 1e-12);
        }
    }

    #[test]
    fn a_block_boundary_leaves_no_seam() {
        let mut whole = Nco::new(F, FS);
        let mut one = vec![Complex::new(0.0f32, 0.0); 256];
        whole.fill(&mut one);

        let mut split = Nco::new(F, FS);
        let mut two = vec![Complex::new(0.0f32, 0.0); 256];
        let (head, tail) = two.split_at_mut(100);
        split.fill(head);
        split.fill(tail);

        assert_eq!(
            one, two,
            "the phase did not carry across the block boundary"
        );
    }

    #[test]
    fn a_negative_frequency_is_the_conjugate_of_a_positive_one() {
        let mut up = Nco::new(F, FS);
        let mut down = Nco::new(-F, FS);
        for _ in 0..1000 {
            let a = up.next_sample();
            let b = down.next_sample();
            assert!((a.conj() - b).norm() < 1e-12);
        }
    }

    #[test]
    fn mixing_a_tone_to_dc_leaves_a_constant() {
        let mut tone = Nco::new(F, FS);
        let mut block = vec![Complex::new(0.0f32, 0.0); 4096];
        tone.fill(&mut block);

        let mut lo = Nco::new(-F, FS);
        lo.mix(&mut block);

        let first = block[0];
        for (n, s) in block.iter().enumerate() {
            assert!(
                (s - first).norm() < 1e-6,
                "sample {n} moved off DC: {s} against {first}"
            );
        }
    }

    #[test]
    fn mixing_by_zero_is_the_identity() {
        let original: Vec<Complex<f32>> = (0..64)
            .map(|n| Complex::new(n as f32 * 0.01, 1.0 - n as f32 * 0.02))
            .collect();
        let mut block = original.clone();
        Nco::new(0.0, FS).mix(&mut block);
        assert_eq!(block, original);
    }

    #[test]
    fn retuning_does_not_step_the_phase() {
        let mut nco = Nco::new(F, FS);
        for _ in 0..100 {
            nco.advance();
        }
        let before = nco.phase_turns();
        nco.set_frequency_hz(F * 2.0);
        assert_eq!(before, nco.phase_turns());
    }

    /// N2's exit condition. A drifting oscillator is one whose phase error grows
    /// with the sample index, so the test is not "is it accurate" but "is it as
    /// accurate at the end as at the beginning".
    #[test]
    fn the_tone_does_not_drift_over_a_million_samples() {
        let mut nco = Nco::new(F, FS);
        let turns_per_sample = F / FS;
        let mut worst = 0.0f64;
        for n in 0..1_000_000u64 {
            if n % 50_000 == 0 {
                let ideal = (turns_per_sample * n as f64).rem_euclid(1.0) * TAU;
                let err = wrap(nco.sample().arg() - ideal).abs();
                worst = worst.max(err);
            }
            nco.advance();
        }
        assert!(
            worst < 1e-9,
            "phase wandered {worst} rad from the closed form over a million samples"
        );
    }
}
