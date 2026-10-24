// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The bit clock: where each bit begins, followed as the transmitter's clock
//! drifts against ours.
//!
//! POCSAG is rectangular two-level FSK, so the discriminator's output steps
//! at every bit boundary where the data changes, and between steps it holds.
//! The clock is told every such step and where in the bit it fell, found to a
//! fraction of a sample by interpolating the zero crossing, and pulls its
//! boundary toward it: a proportional nudge to the phase, and a much smaller
//! one to the period, so a transmitter a few hundred ppm off is followed
//! rather than slipped.
//!
//! Why not `dsp::timing`'s Gardner loop: that module's own notes measured
//! it holding a static phase with a bias on shaped pulses, and it does not
//! track a period. Here the pulses are rectangular, the samples per bit many
//! (16 to 75), and the period is itself a reading (the clock's ppm), so the
//! loop that follows transitions is both simpler and the measurement.
//!
//! **Each bit's value is the mean over the whole bit**, integrate and dump:
//! for rectangular bits in white noise that is the matched filter, the
//! decision with the least error for the noise there is. **Its level is the
//! mean over the middle half**: the channel filter smears every transition
//! over a few samples, and at 2400 baud, 16 samples a bit, a whole-bit mean
//! pulled the measured shift 3 % short of what was sent.
//!
//! **Transitions are found on a smoothed trace.** In noise the raw
//! discriminator output crosses zero many times inside a bit, and every false
//! crossing pulled the boundary: the conformance table read 512 baud losing
//! pages at 9 dB where 2400 still read them, backwards for a rate with four
//! times the energy a bit, because at 75 samples a bit there are that many
//! more chances to cross. A moving average over a quarter bit finds the
//! crossings; the raw samples are delayed by the same half window, so the
//! bit is still integrated where it is.
//!
//! **The loop's period is not the clock's measurement.** It is the loop's
//! state, and lags a steady error (it read 437 ppm for 500). Counting bits
//! against the samples they took was better and still the loop's: it read
//! -25 ppm on a clean single batch, falling with length, as the loop settled
//! inside the span. What is measured is each transition's own time against
//! the number of the boundary it belongs to ([`Crossing`]): the slope of
//! those points is the transmitter's bit length, whatever the loop did.

/// How far one transition moves the boundary, as a share of its error.
const PHASE_GAIN: f64 = 0.06;

/// How far it moves the period, as a share of its error. Small: the period
/// is the slow part, and a noisy transition must not swing it.
const PERIOD_GAIN: f64 = 0.0008;

/// How far the period may stray from the nominal: 2 %, far past any crystal
/// a pager network runs on, and short of half a bit over a batch.
const PERIOD_RANGE: f64 = 0.02;

/// A transition, found to a fraction of a sample: when, in samples since
/// the clock started, and which bit boundary it was, counted from the first.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Crossing {
    pub boundary: u64,
    pub at: f64,
}

/// A crossing further than this share of a bit from the nearest boundary is
/// noise, not a boundary, and is not offered as one.
const BOUNDARY_SHARE: f64 = 0.25;

/// One bit, as the clock closes it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bit {
    /// The mean over the whole bit: what the decision is made on.
    pub mean: f32,
    /// The mean over its middle half: what its level is measured from.
    pub centre: f32,
}

#[derive(Clone, Debug)]
pub struct BitClock {
    nominal: f64,
    period: f64,
    /// Samples since the current bit began.
    phase: f64,
    acc: f64,
    n: u32,
    mid: f64,
    mid_n: u32,
    prev: f32,
    samples: u64,
    /// The last `window` raw samples, for the moving average and the delay.
    ring: std::collections::VecDeque<f32>,
    window: usize,
    sum: f64,
    bits_out: u64,
    crossings: Vec<Crossing>,
}

impl BitClock {
    /// A clock for `samples_per_bit` (fractional: the channel rate over the
    /// baud rate).
    pub fn new(samples_per_bit: f64) -> Self {
        // A quarter bit, odd, so the delay to its middle is whole samples.
        let window = ((samples_per_bit / 4.0).round() as usize).max(1) | 1;
        Self {
            nominal: samples_per_bit,
            period: samples_per_bit,
            phase: 0.0,
            acc: 0.0,
            n: 0,
            mid: 0.0,
            mid_n: 0,
            prev: 0.0,
            samples: 0,
            ring: std::collections::VecDeque::with_capacity(window),
            window,
            sum: 0.0,
            bits_out: 0,
            crossings: Vec::new(),
        }
    }

    /// The period it is running at now, in samples.
    pub fn period(&self) -> f64 {
        self.period
    }

    pub fn nominal(&self) -> f64 {
        self.nominal
    }

    /// One sample of the discriminator's output, less the decision threshold.
    /// Returns the bit when one has ended.
    pub fn push(&mut self, raw: f32) -> Option<Bit> {
        // The smoothed trace, and the raw sample from the middle of its
        // window: the two line up in time.
        self.ring.push_back(raw);
        self.sum += f64::from(raw);
        if self.ring.len() > self.window {
            self.sum -= f64::from(self.ring.pop_front().unwrap_or(0.0));
        }
        if self.ring.len() < self.window {
            return None;
        }
        let smooth = (self.sum / self.window as f64) as f32;
        let x = self.ring[self.window / 2];
        if (smooth > 0.0) != (self.prev > 0.0) && self.prev != 0.0 {
            // Where between the last sample and this one it crossed zero.
            let frac = f64::from(self.prev / (self.prev - smooth));
            let at = self.phase - 1.0 + frac;
            let (err, boundary) = if at < self.period / 2.0 {
                (at, self.bits_out)
            } else {
                (at - self.period, self.bits_out + 1)
            };
            if err.abs() < BOUNDARY_SHARE * self.period {
                self.crossings.push(Crossing {
                    boundary,
                    at: self.samples as f64 + frac - 1.0,
                });
            }
            self.phase -= PHASE_GAIN * err;
            let lo = self.nominal * (1.0 - PERIOD_RANGE);
            let hi = self.nominal * (1.0 + PERIOD_RANGE);
            self.period = (self.period + PERIOD_GAIN * err).clamp(lo, hi);
        }
        self.prev = smooth;
        self.samples += 1;
        self.acc += f64::from(x);
        self.n += 1;
        if (0.25..0.75).contains(&(self.phase / self.period)) {
            self.mid += f64::from(x);
            self.mid_n += 1;
        }
        self.phase += 1.0;
        if self.phase < self.period {
            return None;
        }
        self.phase -= self.period;
        let mean = (self.acc / f64::from(self.n.max(1))) as f32;
        let centre = if self.mid_n > 0 {
            (self.mid / f64::from(self.mid_n)) as f32
        } else {
            mean
        };
        self.acc = 0.0;
        self.n = 0;
        self.mid = 0.0;
        self.mid_n = 0;
        self.bits_out += 1;
        Some(Bit { mean, centre })
    }

    /// The crossings found since the last call, oldest first.
    pub fn take_crossings(&mut self) -> std::vec::Drain<'_, Crossing> {
        self.crossings.drain(..)
    }

    /// Start again at the nominal period.
    pub fn reset(&mut self) {
        *self = Self::new(self.nominal);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rectangular bits at a stated period, starting part way into a bit.
    fn nrz(bits: &[bool], period: f64, start: f64) -> Vec<f32> {
        let total = (bits.len() as f64 * period) as usize;
        (0..total)
            .map(|i| {
                let t = i as f64 + start;
                let k = ((t / period) as usize).min(bits.len() - 1);
                if bits[k] {
                    1.0
                } else {
                    -1.0
                }
            })
            .collect()
    }

    fn pattern(n: usize) -> Vec<bool> {
        // A preamble, then a fixed pseudo-random run.
        let mut s = 0x1234_5678u32;
        (0..n)
            .map(|i| {
                if i < 64 {
                    i % 2 == 0
                } else {
                    s ^= s << 13;
                    s ^= s >> 17;
                    s ^= s << 5;
                    s & 1 == 1
                }
            })
            .collect()
    }

    fn recover(x: &[f32], sps: f64) -> (Vec<bool>, Vec<Crossing>, f64) {
        let mut c = BitClock::new(sps);
        let mut crossings = Vec::new();
        let mut bits = Vec::new();
        for &v in x {
            if let Some(b) = c.push(v) {
                bits.push(b.mean > 0.0);
            }
            crossings.extend(c.take_crossings());
        }
        (bits, crossings, c.period())
    }

    /// Started half a bit out and running 500 ppm slow, it finds the
    /// boundary in the preamble and reads every bit after it; the bits
    /// counted against their samples give the transmitter's clock, and the
    /// loop's own period has followed it most of the way.
    #[test]
    fn it_locks_and_follows_a_clock_off_by_500_ppm() {
        let sps = 32.0;
        let true_period = sps * (1.0 + 500e-6);
        let bits = pattern(4_000);
        let (got, crossings, period) = recover(&nrz(&bits, true_period, sps / 2.0), sps);
        // Past the preamble every bit is right, at a fixed offset of a bit
        // or two either way (the first, part-sent bit and the smoothing
        // window's fill), which the sync word, not the clock, resolves.
        let tail = &bits[200..3_800];
        let found = (-2i64..=2).any(|shift| {
            let from = (200 + shift) as usize;
            got.get(from..from + tail.len()) == Some(tail)
        });
        assert!(found, "bits lost");
        let late: Vec<&Crossing> = crossings.iter().filter(|c| c.boundary >= 200).collect();
        let n = late.len() as f64;
        let (sx, sy) = late
            .iter()
            .fold((0.0, 0.0), |(x, y), c| (x + c.boundary as f64, y + c.at));
        let (mx, my) = (sx / n, sy / n);
        let (sxy, sxx) = late.iter().fold((0.0, 0.0), |(a, b), c| {
            let dx = c.boundary as f64 - mx;
            (a + dx * (c.at - my), b + dx * dx)
        });
        let ppm = (sxy / sxx / sps - 1.0) * 1e6;
        assert!((ppm - 500.0).abs() < 1.0, "measured {ppm} ppm");
        let lag = (period / sps - 1.0) * 1e6;
        assert!((lag - 500.0).abs() < 150.0, "the loop at {lag} ppm");
    }

    /// The middle of a bit is its level, where the whole bit's mean is
    /// pulled toward the other level by a smeared edge.
    #[test]
    fn the_centre_is_the_level() {
        let sps = 16.0;
        // Edges smeared over four samples either side, as a filter does.
        let x: Vec<f32> = (0..(sps as usize * 40))
            .map(|i| {
                let t = (i % 16) as f32;
                let sign = if (i / 16) % 2 == 0 { 1.0 } else { -1.0 };
                let edge = (t.min(16.0 - t) / 4.0).min(1.0);
                sign * edge
            })
            .collect();
        let mut c = BitClock::new(sps);
        let got: Vec<Bit> = x.iter().filter_map(|&v| c.push(v)).collect();
        let late = &got[20..];
        assert!(late.iter().all(|b| b.centre.abs() > 0.97), "{late:?}");
        assert!(late.iter().all(|b| b.mean.abs() < 0.9), "{late:?}");
    }

    /// Never strays past its range, whatever it is fed.
    #[test]
    fn the_period_stays_in_range() {
        let mut c = BitClock::new(16.0);
        for i in 0..100_000 {
            c.push(if i % 3 == 0 { 1.0 } else { -1.0 });
        }
        assert!((c.period() / 16.0 - 1.0).abs() <= PERIOD_RANGE + 1e-9);
    }
}
