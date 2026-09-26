// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The yardstick the receive chain is held to: a GFSK transmitter and a
//! tester, both written from the Bluetooth SIG's own test definitions, in
//! `f64`, and sharing no code with the chain they judge.
//!
//! **Why a second transmitter.** `signal::ble::gfsk::modulate` builds the
//! BLE detector's reference and every synthetic packet in the receive tests.
//! A figure checked against a signal made by the code that also shaped the
//! receiver's expectations is partly checked against itself. [`Gfsk`] is
//! written from the closed form instead: the frequency pulse of one
//! rectangular symbol through a Gaussian filter is a difference of error
//! functions, and its integral, the phase, has a closed form too. The IQ is
//! exact to `f64` rounding at any sample rate, with no filter span to
//! truncate and no numerical integration to drift.
//!
//! **Why a second tester.** The figures a Bluetooth tester reports are
//! defined by the test suites, not by the Core specification: RF.TS.p35 for
//! BR (RF/TRM/CA/BV-07-C modulation characteristics, -08-C initial carrier
//! frequency tolerance, -09-C carrier frequency drift) and RF-PHY.TS.4.2.1
//! for LE (TP/TRM-LE/CA/BV-05-C modulation characteristics, -06-C carrier
//! frequency offset and drift). Both were read for this module, and the
//! clause each function follows is named on it. The suites measure test
//! packets a device is commanded to send; this module measures whatever bits
//! it is told were sent, which is all a synthetic packet needs.
//!
//! **Two readings of one signal.** [`Trace::analytic`] reads the
//! transmitter's exact instantaneous frequency: what an ideal tester, with no
//! filter and no noise, would report. [`Trace::from_iq`] reads IQ through an
//! FM discriminator, behind the measurement filter the suites recommend when
//! [`mask_filter`] is applied first: what a compliant tester would report.
//! Where the two differ, the definition itself depends on the tester, and a
//! receiver cannot be asked to agree with either more closely than that.
//!
//! Test-only. It says what the right answer is; how production reaches it is
//! decided by the steps that use it.

use num_complex::Complex;
use std::f64::consts::{PI, TAU};

/// The error function, to `f64` rounding.
///
/// Summed as `2/sqrt(pi) * exp(-x^2) * sum_n 2^n x^(2n+1) / (1*3*...*(2n+1))`,
/// a series whose terms are all positive, so nothing cancels, and which stops
/// when a term no longer moves the sum. Past 6 the true value is within
/// `3e-17` of one, below `f64`'s resolution there. Held by a test to its own
/// defining derivative and limits, not to remembered values.
pub fn erf(x: f64) -> f64 {
    if x.is_nan() {
        return f64::NAN;
    }
    if x < 0.0 {
        return -erf(-x);
    }
    if x > 6.0 {
        return 1.0;
    }
    let x2 = x * x;
    let (mut term, mut sum, mut n) = (x, x, 0.0f64);
    loop {
        n += 1.0;
        term *= 2.0 * x2 / (2.0 * n + 1.0);
        sum += term;
        if term <= sum * f64::EPSILON * 0.25 {
            break;
        }
    }
    2.0 / PI.sqrt() * (-x2).exp() * sum
}

/// A GFSK transmitter, described by what the specifications state about one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gfsk {
    pub symbol_rate: f64,
    /// Where a long run of like symbols settles, from the carrier, in Hz:
    /// the modulation index times half the symbol rate.
    pub deviation_hz: f64,
    /// The Gaussian filter's bandwidth-time product: 0.5 for BR and for LE.
    pub bt: f64,
    /// The carrier's offset from the nominal channel, Hz.
    pub cfo_hz: f64,
    /// A linear carrier drift from the start of bit 0, Hz per second.
    pub drift_hz_per_s: f64,
}

impl Gfsk {
    pub fn new(symbol_rate: f64, deviation_hz: f64, bt: f64) -> Self {
        Self {
            symbol_rate,
            deviation_hz,
            bt,
            cfo_hz: 0.0,
            drift_hz_per_s: 0.0,
        }
    }

    pub fn with_cfo(self, cfo_hz: f64) -> Self {
        Self { cfo_hz, ..self }
    }

    pub fn with_drift(self, drift_hz_per_s: f64) -> Self {
        Self {
            drift_hz_per_s,
            ..self
        }
    }

    fn period(&self) -> f64 {
        1.0 / self.symbol_rate
    }

    /// The Gaussian's `sqrt(2) * sigma`, in seconds: the filter's -3 dB
    /// bandwidth is `bt / T`, and a Gaussian `exp(-t^2 / (2 sigma^2))` is
    /// 3 dB down at `sqrt(ln 2) / (2 pi sigma)`.
    fn spread(&self) -> f64 {
        2f64.sqrt() * 2f64.ln().sqrt() / (TAU * self.bt) * self.period()
    }

    /// One symbol's frequency pulse, centred on zero: a rectangle one period
    /// wide through the Gaussian. One when settled, so a run of like symbols
    /// sits at exactly [`Self::deviation_hz`].
    fn pulse(&self, t: f64) -> f64 {
        let (h, s) = (self.period() / 2.0, self.spread());
        0.5 * (erf((t + h) / s) - erf((t - h) / s))
    }

    /// The pulse's integral from minus infinity, in seconds: zero before the
    /// symbol, one period after it. `integral of erf(u / s) du` is
    /// `u erf(u / s) + s / sqrt(pi) exp(-(u / s)^2)`.
    fn pulse_area(&self, t: f64) -> f64 {
        let (h, s) = (self.period() / 2.0, self.spread());
        let from_zero = |x: f64| {
            let u = x / s;
            s * (u * erf(u) + ((-u * u).exp() - 1.0) / PI.sqrt())
        };
        0.5 * (from_zero(t + h) - from_zero(t - h)) + h
    }

    /// How far from its centre a symbol still moves the frequency: past half
    /// a period plus nine spreads, `erf` is one to `f64` rounding.
    fn reach(&self) -> f64 {
        self.period() / 2.0 + 9.0 * self.spread()
    }
}

/// One burst of `bits` from a [`Gfsk`] transmitter, bit 0 starting at
/// `t = 0`. Before the first bit and after the last the carrier is
/// unmodulated, so a caller pads with settling bits it does not measure.
pub struct Burst<'a> {
    tx: Gfsk,
    bits: &'a [bool],
    /// Sum of the levels of every symbol before each index: how many periods
    /// of settled deviation the symbols wholly in the past have added.
    settled: Vec<f64>,
}

fn level(bit: bool) -> f64 {
    if bit {
        1.0
    } else {
        -1.0
    }
}

impl<'a> Burst<'a> {
    pub fn new(tx: Gfsk, bits: &'a [bool]) -> Self {
        let mut settled = Vec::with_capacity(bits.len() + 1);
        let mut sum = 0.0;
        settled.push(sum);
        for &b in bits {
            sum += level(b);
            settled.push(sum);
        }
        Self { tx, bits, settled }
    }

    /// The symbols near enough to `t` to still be moving: `lo..hi`.
    fn near(&self, t: f64) -> (usize, usize) {
        let (period, reach) = (self.tx.period(), self.tx.reach());
        let lo = ((t - reach) / period - 0.5).floor().max(0.0) as usize;
        let hi = (((t + reach) / period - 0.5).ceil() + 1.0).max(0.0) as usize;
        (lo.min(self.bits.len()), hi.min(self.bits.len()))
    }

    fn centre(&self, k: usize) -> f64 {
        (k as f64 + 0.5) * self.tx.period()
    }

    /// The exact instantaneous frequency at `t`, Hz from the nominal channel.
    pub fn frequency(&self, t: f64) -> f64 {
        let (lo, hi) = self.near(t);
        let modulation: f64 = (lo..hi)
            .map(|k| level(self.bits[k]) * self.tx.pulse(t - self.centre(k)))
            .sum();
        self.tx.cfo_hz + self.tx.drift_hz_per_s * t + self.tx.deviation_hz * modulation
    }

    /// The exact phase at `t`, radians: `2 pi` times the frequency's integral
    /// from `t = 0`.
    pub fn phase(&self, t: f64) -> f64 {
        let (lo, hi) = self.near(t);
        let past = self.settled[lo] * self.tx.period();
        let moving: f64 = (lo..hi)
            .map(|k| level(self.bits[k]) * self.tx.pulse_area(t - self.centre(k)))
            .sum();
        // The pulses' areas count from minus infinity; before `t = 0` no
        // symbol has begun, so they add nothing there and the origin holds.
        let carrier = self.tx.cfo_hz * t + 0.5 * self.tx.drift_hz_per_s * t * t;
        TAU * (carrier + self.tx.deviation_hz * (past + moving))
    }

    /// `n` IQ samples at `rate`, sample `i` at `t = i / rate`, unit amplitude.
    pub fn iq(&self, rate: f64, n: usize) -> Vec<Complex<f64>> {
        (0..n)
            .map(|i| Complex::from_polar(1.0, self.phase(i as f64 / rate)))
            .collect()
    }

    /// Samples at `rate` to the end of the last bit.
    pub fn len_at(&self, rate: f64) -> usize {
        (self.bits.len() as f64 * self.tx.period() * rate).ceil() as usize
    }
}

/// Frequency readings, each placed in bit periods from the start of bit 0,
/// in time order: what every test definition below reads.
#[derive(Clone, Debug, Default)]
pub struct Trace {
    at: Vec<f64>,
    hz: Vec<f64>,
}

impl Trace {
    /// The exact frequency, `per_bit` readings in every bit at the centres of
    /// equal slices of it. The suites ask for at least 4 (BR) and 32 (LE).
    pub fn analytic(burst: &Burst, per_bit: usize) -> Self {
        let period = burst.tx.period();
        let mut trace = Self::default();
        for k in 0..burst.bits.len() {
            for j in 0..per_bit {
                let at = k as f64 + (j as f64 + 0.5) / per_bit as f64;
                trace.at.push(at);
                trace.hz.push(burst.frequency(at * period));
            }
        }
        trace
    }

    /// An FM discriminator's readings of `iq`, sample `i` taken at
    /// `i / rate` seconds from the start of bit 0: each reading is the phase
    /// step between two samples over their spacing, the mean frequency
    /// between them, placed at their midpoint.
    pub fn from_iq(iq: &[Complex<f64>], rate: f64, symbol_rate: f64) -> Self {
        let mut trace = Self::default();
        for (i, pair) in iq.windows(2).enumerate() {
            let step = (pair[1] * pair[0].conj()).arg();
            trace.at.push((i as f64 + 0.5) / rate * symbol_rate);
            trace.hz.push(step * rate / TAU);
        }
        trace
    }

    /// The reading at `at` bit periods, on the straight line between the
    /// two readings either side; `None` outside them.
    pub fn at(&self, at: f64) -> Option<f64> {
        let i = self.at.partition_point(|&a| a <= at);
        if i == 0 || i == self.at.len() {
            return None;
        }
        let (a0, a1) = (self.at[i - 1], self.at[i]);
        let w = (at - a0) / (a1 - a0);
        Some(self.hz[i - 1] + (self.hz[i] - self.hz[i - 1]) * w)
    }

    /// The readings placed in `[from, to)` bit periods.
    fn span(&self, from: f64, to: f64) -> &[f64] {
        let lo = self.at.partition_point(|&a| a < from);
        let hi = self.at.partition_point(|&a| a < to);
        &self.hz[lo..hi.max(lo)]
    }

    /// The mean reading in `[from, to)` bit periods, `None` over no readings.
    pub fn mean(&self, from: f64, to: f64) -> Option<f64> {
        let s = self.span(from, to);
        (!s.is_empty()).then(|| s.iter().sum::<f64>() / s.len() as f64)
    }

    /// The reading furthest from `centre` in `[from, to)`, as a distance.
    fn widest(&self, from: f64, to: f64, centre: f64) -> Option<f64> {
        self.span(from, to)
            .iter()
            .map(|f| (f - centre).abs())
            .reduce(f64::max)
    }
}

/// Delta-f1: RF.TS.p35 RF/TRM/CA/BV-07-C d), RF-PHY.TS.4.2.1
/// TP/TRM-LE/CA/BV-05-C steps 5 and 6. In every `00001111` in `bits[from..to]`
/// the mean over all eight bits is the sequence's centre, and each of the
/// second, third, sixth and seventh bits gives the distance of its own mean
/// reading from that centre. Sequences are taken where they occur, one after
/// another without overlap, as a test payload of the pattern lays them out.
pub fn delta_f1(trace: &Trace, bits: &[bool], from: usize, to: usize) -> Vec<f64> {
    const PATTERN: [bool; 8] = [false, false, false, false, true, true, true, true];
    let mut out = Vec::new();
    let mut k = from;
    while k + 8 <= to.min(bits.len()) {
        if bits[k..k + 8] != PATTERN {
            k += 1;
            continue;
        }
        if let Some(centre) = trace.mean(k as f64, (k + 8) as f64) {
            for j in [1, 2, 5, 6] {
                let at = (k + j) as f64;
                if let Some(bit) = trace.mean(at, at + 1.0) {
                    out.push((bit - centre).abs());
                }
            }
        }
        k += 8;
    }
    out
}

/// Delta-f2: RF.TS.p35 RF/TRM/CA/BV-07-C h), RF-PHY.TS.4.2.1
/// TP/TRM-LE/CA/BV-05-C steps 10 and 11. In every eight alternating bits in
/// `bits[from..to]` the mean over all eight is the sequence's centre, and
/// each bit gives the largest distance of any reading within it from that
/// centre. Sequences without overlap, as for [`delta_f1`].
pub fn delta_f2(trace: &Trace, bits: &[bool], from: usize, to: usize) -> Vec<f64> {
    let mut out = Vec::new();
    let mut k = from;
    while k + 8 <= to.min(bits.len()) {
        if !bits[k..k + 8].windows(2).all(|w| w[0] != w[1]) {
            k += 1;
            continue;
        }
        if let Some(centre) = trace.mean(k as f64, (k + 8) as f64) {
            for j in 0..8 {
                let at = (k + j) as f64;
                if let Some(widest) = trace.widest(at, at + 1.0, centre) {
                    out.push(widest);
                }
            }
        }
        k += 8;
    }
    out
}

/// The initial carrier frequency f0: RF.TS.p35 RF/TRM/CA/BV-08-C d) with
/// `preamble_bits` 4 (BR), RF-PHY.TS.4.2.1 TP/TRM-LE/CA/BV-06-C step 4 with
/// 8 (LE). The mean from the centre of the first preamble bit, at `first`, to
/// the centre of the first bit after the preamble.
pub fn initial_carrier(trace: &Trace, first: usize, preamble_bits: usize) -> Option<f64> {
    let start = first as f64 + 0.5;
    trace.mean(start, start + preamble_bits as f64)
}

/// The drift readings fk: RF.TS.p35 RF/TRM/CA/BV-09-C e), RF-PHY.TS.4.2.1
/// TP/TRM-LE/CA/BV-06-C step 6. The mean over every ten bits from the second
/// payload bit (`payload` is the first), whole blocks only, ending at or
/// before `end` (on LE, the start of the CRC).
pub fn ten_bit_means(trace: &Trace, payload: usize, end: usize) -> Vec<f64> {
    let mut out = Vec::new();
    let mut k = payload + 1;
    while k + 10 <= end {
        if let Some(m) = trace.mean(k as f64, (k + 10) as f64) {
            out.push(m);
        }
        k += 10;
    }
    out
}

/// The mean of `values`, `None` when there are none.
pub fn mean(values: &[f64]) -> Option<f64> {
    (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
}

/// The share of `values` above `limit`: the suites' "at least 99.9 % of all
/// f2max" is this against 115 kHz (BR) or 185 kHz (LE 1M).
pub fn share_above(values: &[f64], limit: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.iter().filter(|&&v| v > limit).count() as f64 / values.len() as f64
}

/// The measurement filter both suites recommend (RF.TS.p35 RF/TRM/CA/BV-07-C
/// to -09-C; RF-PHY.TS.4.2.1 TP/TRM-LE/CA/BV-05-C, -06-C): passband ripple
/// at most 0.5 dB peak to peak to 550 kHz, and at least 3 dB down at
/// 650 kHz, 14 dB at 1 MHz, 44 dB at 2 MHz, either side of the carrier.
///
/// A Kaiser-windowed sinc, passband edge 550 kHz, 52 dB reached by 740 kHz:
/// a ripple of a few hundredths of a dB and every recommended attenuation
/// beaten. The suites give minimums, so a steeper filter is as compliant as
/// the loosest; `the_mask_filter_meets_the_recommendation` measures this one
/// against the four points rather than trusting the design formulas.
pub fn mask_filter(rate: f64) -> Vec<f64> {
    const PASS_HZ: f64 = 550_000.0;
    const STOP_HZ: f64 = 740_000.0;
    const ATTEN_DB: f64 = 52.0;
    let fc = (PASS_HZ + STOP_HZ) / 2.0 / rate;
    let width = (STOP_HZ - PASS_HZ) / rate;
    let beta = 0.1102 * (ATTEN_DB - 8.7);
    let taps = (((ATTEN_DB - 7.95) / (2.285 * TAU * width)).ceil() as usize + 1) | 1;
    let m = (taps - 1) as f64 / 2.0;
    let mut h: Vec<f64> = (0..taps)
        .map(|i| {
            let x = i as f64 - m;
            let sinc = if x == 0.0 {
                2.0 * fc
            } else {
                (TAU * fc * x).sin() / (PI * x)
            };
            let r = x / m;
            sinc * bessel_i0(beta * (1.0 - r * r).max(0.0).sqrt()) / bessel_i0(beta)
        })
        .collect();
    let sum: f64 = h.iter().sum();
    h.iter_mut().for_each(|v| *v /= sum);
    h
}

/// The modified Bessel function of the first kind, order zero, by its power
/// series `sum ((x/2)^k / k!)^2`, all terms positive.
fn bessel_i0(x: f64) -> f64 {
    let (mut term, mut sum, mut k) = (1.0f64, 1.0f64, 0.0f64);
    loop {
        k += 1.0;
        term *= (x / (2.0 * k)).powi(2);
        sum += term;
        if term <= sum * f64::EPSILON * 0.25 {
            break;
        }
    }
    sum
}

/// `iq` through the real, odd-length, linear-phase filter `taps`, delayed
/// back into place, so sample `i` out lines up with sample `i` in. The ends,
/// within half the filter of the edges, see zeros beyond the burst.
pub fn filter(iq: &[Complex<f64>], taps: &[f64]) -> Vec<Complex<f64>> {
    let half = (taps.len() - 1) / 2;
    (0..iq.len())
        .map(|i| {
            taps.iter()
                .enumerate()
                .filter_map(|(k, &h)| {
                    let j = (i + half).checked_sub(k)?;
                    iq.get(j).map(|&x| x * h)
                })
                .sum()
        })
        .collect()
}

/// The filter's gain at `hz`, in dB.
pub fn response_db(taps: &[f64], hz: f64, rate: f64) -> f64 {
    let w = TAU * hz / rate;
    let h: Complex<f64> = taps
        .iter()
        .enumerate()
        .map(|(n, &t)| Complex::from_polar(t, -w * n as f64))
        .sum();
    20.0 * h.norm().log10()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::dsp::testkit::Rng;

    const BR_DEVIATION_HZ: f64 = 160_000.0;
    const LE_DEVIATION_HZ: f64 = 250_000.0;

    fn alternating(n: usize, first: bool) -> Vec<bool> {
        (0..n).map(|i| (i % 2 == 0) == first).collect()
    }

    /// Settling bits, then `body`, then settling bits: `body` starts at the
    /// returned index.
    fn padded(body: &[bool]) -> (Vec<bool>, usize) {
        let mut bits = alternating(24, true);
        let at = bits.len();
        bits.extend_from_slice(body);
        bits.extend(alternating(24, true));
        (bits, at)
    }

    fn repeat(pattern: [bool; 8], times: usize) -> Vec<bool> {
        pattern.iter().copied().cycle().take(8 * times).collect()
    }

    const ON_F0: [bool; 8] = [true, true, true, true, false, false, false, false];

    /// `erf` is odd, zero at zero, one at infinity, and its slope is
    /// `2/sqrt(pi) exp(-x^2)` everywhere: the definition, which pins it down
    /// without a single remembered value.
    #[test]
    fn erf_is_the_integral_of_the_gaussian() {
        assert_eq!(erf(0.0), 0.0);
        assert_eq!(erf(7.0), 1.0);
        assert!(1.0 - erf(5.99) < 1e-15);
        let d = 1e-5;
        let mut x = -5.0;
        while x < 5.0 {
            assert_eq!(erf(-x), -erf(x));
            let slope = (erf(x + d) - erf(x - d)) / (2.0 * d);
            let exact = 2.0 / PI.sqrt() * (-x * x).exp();
            assert!((slope - exact).abs() < 1e-9, "{x}: {slope} vs {exact}");
            x += 0.01;
        }
    }

    /// Shifted pulses add to one everywhere (the rectangles tile time, and
    /// the Gaussian keeps their sum), and the pulse's area is its integral.
    #[test]
    fn the_pulse_tiles_time_and_its_area_is_its_integral() {
        for bt in [0.3, 0.5, 1.0] {
            let tx = Gfsk::new(1e6, BR_DEVIATION_HZ, bt);
            let period = tx.period();
            for step in 0..40 {
                let t = step as f64 * period / 40.0;
                let sum: f64 = (-20..20).map(|k| tx.pulse(t - k as f64 * period)).sum();
                assert!((sum - 1.0).abs() < 1e-14, "bt {bt}, t {t}: {sum}");

                let d = period * 1e-5;
                let slope = (tx.pulse_area(t + d) - tx.pulse_area(t - d)) / (2.0 * d);
                assert!((slope - tx.pulse(t)).abs() < 1e-8, "bt {bt}, t {t}");
            }
            assert!(tx.pulse_area(-20.0 * period).abs() < 1e-20);
            assert!((tx.pulse_area(20.0 * period) - period).abs() < 1e-18);
        }
    }

    /// The phase is the frequency's integral, carrier offset and drift
    /// included, over random bits: the two closed forms agree.
    #[test]
    fn the_phase_is_the_integral_of_the_frequency() {
        let mut rng = Rng::new(3);
        let bits: Vec<bool> = (0..200).map(|_| rng.next_u64() & 1 == 1).collect();
        let tx = Gfsk::new(1e6, LE_DEVIATION_HZ, 0.5)
            .with_cfo(-41_000.0)
            .with_drift(3e6);
        let burst = Burst::new(tx, &bits);
        let d = 1e-10;
        for i in 0..2000 {
            let t = i as f64 * 1e-7 + 3e-8;
            let slope = (burst.phase(t + d) - burst.phase(t - d)) / (2.0 * d) / TAU;
            let f = burst.frequency(t);
            assert!((slope - f).abs() < 0.05, "t {t}: {slope} vs {f}");
        }
    }

    /// With a nearly rectangular pulse every bit is settled: delta-f1 and
    /// delta-f2 are the deviation itself, and the carrier and its drift are
    /// read back exactly. The test definitions reduce to the obvious answer
    /// where there is one.
    #[test]
    fn a_rectangular_pulse_reads_back_exactly() {
        let cfo = 37_000.0;
        let drift = 2e6; // 2 kHz per millisecond
        let tx = Gfsk::new(1e6, BR_DEVIATION_HZ, 1000.0)
            .with_cfo(cfo)
            .with_drift(drift);
        let mut body = alternating(8, true);
        body.extend(repeat(ON_F0, 12));
        body.extend(alternating(96, true));
        let (bits, at) = padded(&body);
        let burst = Burst::new(tx, &bits);
        let trace = Trace::analytic(&burst, 32);

        let end = at + body.len();
        // Carrier and drift move the centre, not the distance from it, so
        // only the drift within one sequence is left: 8 us at 2 Hz/us.
        for f1 in delta_f1(&trace, &bits, at, end) {
            assert!((f1 - BR_DEVIATION_HZ).abs() < 10.0, "{f1}");
        }
        let f2 = delta_f2(&trace, &bits, at + 8 + 96, end);
        assert!(f2.len() >= 80, "{}", f2.len());
        for f2 in f2 {
            assert!((f2 - BR_DEVIATION_HZ).abs() < 10.0, "{f2}");
        }

        // Over whole periods of an alternation the modulation averages to
        // nothing, and a linear drift averages to its value at the middle.
        let f0 = initial_carrier(&trace, at, 4).unwrap();
        let middle = (at as f64 + 0.5 + 2.0) * 1e-6;
        assert!((f0 - (cfo + drift * middle)).abs() < 1e-6, "{f0}");
        let payload = at + 8 + 96;
        for (n, fk) in ten_bit_means(&trace, payload, end).iter().enumerate() {
            let middle = (payload + 1 + 10 * n) as f64 * 1e-6 + 5e-6;
            let expected = cfo + drift * middle;
            assert!((fk - expected).abs() < 0.2 * BR_DEVIATION_HZ, "{fk}");
        }
    }

    /// Over an alternation throughout, ten-bit blocks are five whole periods:
    /// each reads the carrier and its drift at the block's middle exactly.
    #[test]
    fn drift_blocks_read_the_carrier_at_their_middle() {
        let (cfo, drift) = (-12_500.0, -4e6);
        let tx = Gfsk::new(1e6, LE_DEVIATION_HZ, 0.5)
            .with_cfo(cfo)
            .with_drift(drift);
        let bits = alternating(400, true);
        let burst = Burst::new(tx, &bits);
        let trace = Trace::analytic(&burst, 32);
        let blocks = ten_bit_means(&trace, 40, 360);
        assert_eq!(blocks.len(), 31);
        for (n, fk) in blocks.iter().enumerate() {
            let middle = (41 + 10 * n) as f64 * 1e-6 + 5e-6;
            assert!((fk - (cfo + drift * middle)).abs() < 1e-6, "{n}: {fk}");
        }
    }

    /// The discriminator on exact IQ at 32 samples a bit reads what the exact
    /// frequency reads, to within what averaging over one sample spacing
    /// does to a curved trace.
    #[test]
    fn the_discriminator_reads_the_exact_frequency() {
        let tx = Gfsk::new(1e6, BR_DEVIATION_HZ, 0.5).with_cfo(20_000.0);
        let mut body = repeat(ON_F0, 16);
        body.extend(alternating(128, false));
        let (bits, at) = padded(&body);
        let burst = Burst::new(tx, &bits);
        let rate = 32e6;
        let iq = burst.iq(rate, burst.len_at(rate));
        let from_iq = Trace::from_iq(&iq, rate, 1e6);
        let exact = Trace::analytic(&burst, 32);
        let end = at + body.len();
        let a = mean(&delta_f1(&from_iq, &bits, at, end)).unwrap();
        let b = mean(&delta_f1(&exact, &bits, at, end)).unwrap();
        assert!((a - b).abs() < 5.0, "df1 {a} vs {b}");
        let a = mean(&delta_f2(&from_iq, &bits, at + 128, end)).unwrap();
        let b = mean(&delta_f2(&exact, &bits, at + 128, end)).unwrap();
        assert!((a - b).abs() < 100.0, "df2 {a} vs {b}");
    }

    /// The measurement filter against the suites' four points, measured.
    #[test]
    fn the_mask_filter_meets_the_recommendation() {
        let rate = 32e6;
        let taps = mask_filter(rate);
        let pass: Vec<f64> = (0..=110)
            .map(|i| response_db(&taps, i as f64 * 5_000.0, rate))
            .collect();
        let ripple = pass.iter().copied().fold(f64::MIN, f64::max)
            - pass.iter().copied().fold(f64::MAX, f64::min);
        assert!(ripple <= 0.5, "ripple {ripple} dB");
        assert!(response_db(&taps, 650e3, rate) <= -3.0);
        assert!(response_db(&taps, 1e6, rate) <= -14.0);
        for i in 0..=140 {
            let hz = 2e6 + i as f64 * 100e3;
            assert!(response_db(&taps, hz, rate) <= -44.0, "{hz} Hz");
        }
    }

    /// Nominal transmitters pass the suites' limits as an ideal tester and
    /// as a compliant one reads them, and the two readings are printed side
    /// by side: the reference figures the receive chain is held to.
    #[test]
    fn nominal_transmitters_pass_as_the_suites_measure_them() {
        // BR: h = 0.32; LE 1M: h = 0.5. Limits: RF.TS.p35 RF/TRM/CA/BV-07-C,
        // RF-PHY.TS.4.2.1 TP/TRM-LE/CA/BV-05-C.
        let cases = [
            ("BR", BR_DEVIATION_HZ, (140e3, 175e3), 115e3),
            ("LE 1M", LE_DEVIATION_HZ, (225e3, 275e3), 185e3),
        ];
        let rate = 32e6;
        let taps = mask_filter(rate);
        for (name, deviation, (lo, hi), f2_floor) in cases {
            let tx = Gfsk::new(1e6, deviation, 0.5);
            let mut body = repeat(ON_F0, 20);
            let f2_at = body.len();
            body.extend(alternating(160, true));
            let (bits, at) = padded(&body);
            let end = at + body.len();
            let burst = Burst::new(tx, &bits);
            let iq = burst.iq(rate, burst.len_at(rate));
            let readings = [
                ("ideal", Trace::analytic(&burst, 32)),
                ("mask", Trace::from_iq(&filter(&iq, &taps), rate, 1e6)),
            ];
            for (tester, trace) in readings {
                let f1 = delta_f1(&trace, &bits, at, at + f2_at);
                let f2 = delta_f2(&trace, &bits, at + f2_at, end);
                let (f1_avg, f2_avg) = (mean(&f1).unwrap(), mean(&f2).unwrap());
                let ratio = f2_avg / f1_avg;
                eprintln!(
                    "{name:<5} {tester:<5} df1avg {:.3} kHz  df2avg {:.3} kHz  \
                     df2/df1 {ratio:.4}  f2max>{:.0}k {:.2} %",
                    f1_avg / 1e3,
                    f2_avg / 1e3,
                    f2_floor / 1e3,
                    100.0 * share_above(&f2, f2_floor)
                );
                assert!((lo..=hi).contains(&f1_avg), "{name} {tester}: {f1_avg}");
                assert!(share_above(&f2, f2_floor) >= 0.999, "{name} {tester}");
                assert!(ratio >= 0.8, "{name} {tester}: {ratio}");
            }
        }
    }
}

/// The real receive chain, `NetWorker` and all, held to the reference: the
/// report the accuracy work is judged by. Fed synthetic packets from
/// [`Gfsk`] as 8-bit bytes, as a radio would deliver them, and read back
/// from the state the panels show.
///
/// Three columns per figure, so an error can be placed:
/// - **chain**: what sdrtop reports, through its filters, mixer, slicer and
///   noise;
/// - **ideal**: sdrtop's own definition of the figure applied to the exact
///   frequency, no filter and no noise. Chain against ideal is the chain's
///   error; ideal against the suites is the definition's;
/// - **suites**: the test definitions of RF.TS.p35 / RF-PHY.TS.4.2.1 on the
///   exact frequency (the [`Trace::analytic`] figures).
///
/// SNR is stated in 1 MHz around the carrier, so it means the same thing at
/// every sample rate.
#[cfg(test)]
mod chain {
    use super::*;
    use crate::hardware::{SampleFormat, SampleGeometry, StreamBlock};
    use crate::signal::dsp::testkit::Rng;
    use crate::signal::net::worker::{NetWorker, SAFE_BT_CHANNELS};
    use crate::state::SdrMetrics;
    use std::sync::{Arc, Mutex};

    /// The signal's amplitude in the converter's full scale: room for the
    /// noise above it before the 8 bits clip.
    const AMPLITUDE: f64 = 0.5;
    /// The unmodulated carrier ramped up before the first bit and down after
    /// the last, as a transmitter does, so no filter sees a step.
    const RAMP_S: f64 = 4e-6;
    const PAIRS_PER_BLOCK: usize = 65_536;

    fn geometry() -> SampleGeometry {
        SampleGeometry {
            format: SampleFormat::Int8,
            full_scale: 128.0,
        }
    }

    /// Bursts in a stream of noise, one starting every `spacing_s` after
    /// `lead_s`, as bytes. Each burst is ramped carrier, its bits, ramped
    /// carrier.
    fn stream(
        bursts: &[Burst],
        rate: f64,
        lead_s: f64,
        spacing_s: f64,
        snr_db: f64,
        seed: u64,
    ) -> Vec<u8> {
        let total_s = lead_s + spacing_s * bursts.len() as f64;
        let n = (total_s * rate).ceil() as usize;
        // Noise power per complex sample for `snr_db` in 1 MHz.
        let noise = AMPLITUDE * AMPLITUDE / 10f64.powf(snr_db / 10.0) * rate / 1e6;
        let mut rng = Rng::new(seed);
        let mut iq: Vec<Complex<f64>> = rng
            .noise(n, noise)
            .iter()
            .map(|z| Complex::new(z.re as f64, z.im as f64))
            .collect();
        for (b, burst) in bursts.iter().enumerate() {
            let start = lead_s + spacing_s * b as f64 + RAMP_S;
            let bits_s = burst.bits.len() as f64 / burst.tx.symbol_rate;
            let first = ((start - RAMP_S) * rate).ceil() as usize;
            let last = (((start + bits_s + RAMP_S) * rate).floor() as usize).min(n - 1);
            for (i, slot) in iq.iter_mut().enumerate().take(last + 1).skip(first) {
                let t = i as f64 / rate - start;
                let edge = (t + RAMP_S).min(bits_s + RAMP_S - t) / RAMP_S;
                let envelope = if edge >= 1.0 {
                    1.0
                } else {
                    0.5 - 0.5 * (PI * edge.max(0.0)).cos()
                };
                *slot += Complex::from_polar(AMPLITUDE * envelope, burst.phase(t));
            }
        }
        iq.iter()
            .flat_map(|z| {
                let q = |v: f64| (v * 128.0).round().clamp(-127.0, 127.0) as i8 as u8;
                [q(z.re), q(z.im)]
            })
            .collect()
    }

    /// `bytes` through a fresh `NetWorker` on `preset`, tuned to `tuned`.
    fn run(preset: &str, tuned: u64, rate: f64, bytes: &[u8]) -> SdrMetrics {
        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = crate::signal::net::SECTION.to_string();
        m.ui.active_preset = preset.to_string();
        m.radio.frequency = tuned;
        m.radio.config_sample_rate = rate;
        m.radio.bb_filter_hz = 0;
        let state = Arc::new(Mutex::new(m));
        let (tx, rx) = crossbeam_channel::unbounded();
        for (k, chunk) in bytes.chunks(2 * PAIRS_PER_BLOCK).enumerate() {
            tx.send(StreamBlock {
                seq: k as u64 + 1,
                gap_before: false,
                first_pair: (k * PAIRS_PER_BLOCK) as u64,
                centre_hz: tuned,
                rate_hz: rate,
                bytes: chunk.to_vec(),
            })
            .unwrap();
        }
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), geometry(), SAFE_BT_CHANNELS).run();
        let m = state.lock().unwrap_or_else(|e| e.into_inner()).clone();
        m
    }

    /// The exact frequency at the centre of each of `bits` (`from..to`), the
    /// reading a slicer at the ideal phase would take.
    fn centres(burst: &Burst, from: usize, to: usize) -> Vec<f32> {
        let period = burst.tx.period();
        (from..to)
            .map(|k| burst.frequency((k as f64 + 0.5) * period) as f32)
            .collect()
    }

    /// Mean and sample standard deviation.
    fn spread(values: &[f64]) -> (f64, f64) {
        let n = values.len() as f64;
        let m = values.iter().sum::<f64>() / n;
        let v = values.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (n - 1.0).max(1.0);
        (m, v.sqrt())
    }

    fn khz(v: f64) -> String {
        format!("{:8.2}", v / 1e3)
    }

    fn khz_pm((m, s): (f64, f64)) -> String {
        format!("{:8.2} +-{:5.2}", m / 1e3, s / 1e3)
    }

    const PATTERN_F1: [bool; 8] = [true, true, true, true, false, false, false, false];

    /// One LE 1M ADV_IND on channel 37 whose payload, after the six address
    /// octets, is on the air as the suites' patterns: 15 octets of
    /// `11110000`, then 16 of `10101010`, with the whitening undone into the
    /// data so the CRC is right. Returns the bits from the preamble on, and
    /// where the two pattern stretches and the PDU start.
    fn le_packet(address: [u8; 6]) -> (Vec<bool>, usize, (usize, usize), (usize, usize)) {
        use crate::signal::ble::detect::{
            access_address_bits, preamble_bits, ADVERTISING_ACCESS_ADDRESS,
        };
        use crate::signal::ble::Phy;
        use crate::signal::dsp::code::lfsr::whiten;
        const CHANNEL: u8 = 37;
        const LENGTH: usize = 37;

        let mut on_air = Vec::new();
        for _ in 0..15 {
            on_air.extend(PATTERN_F1);
        }
        for _ in 0..16 {
            on_air.extend([true, false, true, false, true, false, true, false]);
        }
        let mut sequence = vec![false; (2 + LENGTH + 3) * 8];
        whiten(&mut sequence, CHANNEL);
        let mut payload = address.to_vec();
        for (octet, chunk) in on_air.chunks(8).enumerate() {
            let at = 16 + 8 * (6 + octet);
            let byte = chunk.iter().enumerate().fold(0u8, |acc, (i, &b)| {
                acc | (((b ^ sequence[at + i]) as u8) << i)
            });
            payload.push(byte);
        }
        let pdu = crate::signal::ble::pdu::encode(CHANNEL, 0x00, &payload);
        let pattern_at = 16 + 48;
        assert_eq!(&pdu[pattern_at..pattern_at + on_air.len()], &on_air[..]);

        let mut bits = preamble_bits(ADVERTISING_ACCESS_ADDRESS, Phy::OneM);
        bits.extend(access_address_bits(ADVERTISING_ACCESS_ADDRESS));
        let pdu_at = bits.len();
        bits.extend(pdu);
        let f1 = (pdu_at + pattern_at, pdu_at + pattern_at + 120);
        (bits, pdu_at, f1, (f1.1, f1.1 + 128))
    }

    /// One classic packet: preamble, `lap`'s sync word, trailer, a random
    /// whitened header in FEC(1/3), and random payload bits. The preamble and
    /// trailer alternate on from the sync word's ends. Returns the bits and
    /// where the trailer starts.
    fn br_packet(lap: u32, rng: &mut Rng) -> (Vec<bool>, usize) {
        let sync = crate::signal::bt::access_code::access_code_bits(lap);
        let (s0, s63) = (sync[0], sync[63]);
        let mut bits = vec![s0, !s0, s0, !s0];
        bits.extend(sync);
        let trailer = bits.len();
        bits.extend([!s63, s63, !s63, s63]);
        for _ in 0..18 {
            let b = rng.next_u64() & 1 == 1;
            bits.extend([b, b, b]);
        }
        bits.extend((0..240).map(|_| rng.next_u64() & 1 == 1));
        (bits, trailer)
    }

    /// The suites' delta-f1, delta-f2 and their ratio for a transmitter, from
    /// a burst of their own test patterns.
    fn suites_figures(tx: Gfsk) -> (f64, f64) {
        let mut bits: Vec<bool> = (0..24).map(|i| i % 2 == 0).collect();
        let f1_at = bits.len();
        for _ in 0..20 {
            bits.extend(PATTERN_F1);
        }
        let f2_at = bits.len();
        bits.extend((0..160).map(|i| i % 2 == 0));
        let end = bits.len();
        bits.extend((0..24).map(|i| i % 2 == 0));
        let burst = Burst::new(tx, &bits);
        let trace = Trace::analytic(&burst, 32);
        (
            mean(&delta_f1(&trace, &bits, f1_at, f2_at)).unwrap(),
            mean(&delta_f2(&trace, &bits, f2_at, end)).unwrap(),
        )
    }

    const RATES: [f64; 3] = [4e6, 8e6, 20e6];
    const SNRS_DB: [f64; 2] = [40.0, 20.0];
    const CFOS_HZ: [f64; 2] = [0.0, 40_000.0];
    const PACKETS: usize = 12;

    /// **The report.** Run by hand, in release:
    /// `cargo test --release --locked the_chain_against_the_reference --
    /// --ignored --nocapture`.
    #[test]
    #[ignore]
    fn the_chain_against_the_reference() {
        le_report();
        br_report();
    }

    fn le_report() {
        const TUNED: u64 = 2_402_000_000;
        let deviation = 250_000.0;
        let (ts_f1, ts_f2) = suites_figures(Gfsk::new(1e6, deviation, 0.5));
        eprintln!(
            "\nLE 1M, ADV_IND on channel 37, deviation {} kHz, BT 0.5, {PACKETS} packets a case",
            deviation / 1e3
        );
        eprintln!(
            "suites (exact frequency): df1avg {} kHz  df2avg {} kHz  df2/df1 {:.4}",
            khz(ts_f1),
            khz(ts_f2),
            ts_f2 / ts_f1
        );
        eprintln!(
            "{:>5} {:>4} {:>6} {:>5} | {:>17} {:>8} | {:>17} {:>8} | {:>15} {:>6} | {:>17} {:>8}",
            "Msps",
            "SNR",
            "CFO",
            "found",
            "df1 chain",
            "ideal",
            "df2 chain",
            "ideal",
            "ratio chain",
            "ideal",
            "CFO chain",
            "suites f0"
        );
        for rate in RATES {
            for snr in SNRS_DB {
                for cfo in CFOS_HZ {
                    let tx = Gfsk::new(1e6, deviation, 0.5).with_cfo(cfo);
                    let packets: Vec<_> = (0..PACKETS)
                        .map(|p| le_packet([0x10 + p as u8, 0x22, 0x33, 0x44, 0x55, 0xC6]))
                        .collect();
                    let bursts: Vec<Burst> = packets
                        .iter()
                        .map(|(bits, ..)| Burst::new(tx, bits))
                        .collect();
                    let bytes = stream(&bursts, rate, 300e-6, 700e-6, snr, 17 + snr as u64);
                    let m = run("net_ble", TUNED, rate, &bytes);

                    // sdrtop's definition on the exact frequency, and the
                    // suites' f0, per packet.
                    let (mut i1, mut i2, mut ir, mut f0) = (vec![], vec![], vec![], vec![]);
                    for (burst, (bits, pdu_at, ..)) in bursts.iter().zip(&packets) {
                        let used = crate::signal::ble::pdu::used_bits(37);
                        let air = &bits[*pdu_at..*pdu_at + used];
                        let hz = centres(burst, *pdu_at, *pdu_at + used);
                        let q = crate::signal::ble::measure::modulation_quality(
                            air,
                            &hz,
                            crate::signal::ble::Phy::OneM,
                        )
                        .expect("patterns give both runs");
                        i1.push(q.delta_f1_avg_hz.value());
                        i2.push(q.delta_f2_avg_hz.value());
                        ir.push(q.ratio.value());
                        let trace = Trace::analytic(burst, 32);
                        f0.push(initial_carrier(&trace, 0, 8).unwrap());
                    }

                    let got: Vec<_> = m.net.ble_packets.iter().filter(|p| p.crc_ok).collect();
                    let c1: Vec<f64> = got
                        .iter()
                        .filter_map(|p| p.modulation.map(|q| q.delta_f1_avg_hz.value()))
                        .collect();
                    let c2: Vec<f64> = got
                        .iter()
                        .filter_map(|p| p.modulation.map(|q| q.delta_f2_avg_hz.value()))
                        .collect();
                    let cr: Vec<f64> = got
                        .iter()
                        .filter_map(|p| p.modulation.map(|q| q.ratio.value()))
                        .collect();
                    let co: Vec<f64> = got
                        .iter()
                        .filter_map(|p| p.freq_offset_hz.map(|o| o.value()))
                        .collect();
                    let or_dash = |v: &[f64], f: &dyn Fn((f64, f64)) -> String| {
                        if v.is_empty() {
                            format!("{:>17}", "-")
                        } else {
                            f(spread(v))
                        }
                    };
                    eprintln!(
                        "{:>5} {:>4} {:>6} {:>2}/{:<2} | {} {} | {} {} | {} {:6.4} | {} {}",
                        rate / 1e6,
                        snr,
                        cfo / 1e3,
                        got.len(),
                        PACKETS,
                        or_dash(&c1, &khz_pm),
                        khz(spread(&i1).0),
                        or_dash(&c2, &khz_pm),
                        khz(spread(&i2).0),
                        or_dash(&cr, &|(m, s)| format!("{m:7.4} +-{s:6.4}")),
                        spread(&ir).0,
                        or_dash(&co, &khz_pm),
                        khz(spread(&f0).0),
                    );
                }
            }
        }
    }

    fn br_report() {
        const CHANNEL: u8 = 39;
        let tuned = crate::signal::bt::channel::centre_hz(CHANNEL).unwrap();
        let deviation = 160_000.0;
        // Not the general inquiry code, so it is counted as a piconet.
        let lap = 0x0044_5566;
        let (ts_f1, ts_f2) = suites_figures(Gfsk::new(1e6, deviation, 0.5));
        eprintln!(
            "\nBR, trailer and header of {PACKETS} packets a case on channel {CHANNEL}, \
             deviation {} kHz, BT 0.5",
            deviation / 1e3
        );
        eprintln!(
            "suites (exact frequency): df1avg {} kHz  df2avg {} kHz  df2/df1 {:.4}",
            khz(ts_f1),
            khz(ts_f2),
            ts_f2 / ts_f1
        );
        eprintln!(
            "{:>5} {:>4} {:>6} {:>5} | {:>17} {:>8} {:>8} | {:>17} {:>8} {:>8} | {:>7} {:>7} {:>7}",
            "Msps",
            "SNR",
            "CFO",
            "heads",
            "df1 chain",
            "mask",
            "ideal",
            "df2 chain",
            "mask",
            "ideal",
            "ratio",
            "mask",
            "ideal"
        );
        for rate in RATES {
            for snr in SNRS_DB {
                for cfo in CFOS_HZ {
                    let tx = Gfsk::new(1e6, deviation, 0.5).with_cfo(cfo);
                    let mut rng = Rng::new(5);
                    let packets: Vec<_> = (0..PACKETS).map(|_| br_packet(lap, &mut rng)).collect();
                    let bursts: Vec<Burst> = packets
                        .iter()
                        .map(|(bits, _)| Burst::new(tx, bits))
                        .collect();
                    // Each header hit waits for the receiver's full payload
                    // capture, 2744 bits, before it is emitted.
                    let bytes = stream(&bursts, rate, 300e-6, 3.2e-3, snr, 29 + snr as u64);
                    let m = run("net_bt", tuned, rate, &bytes);

                    // sdrtop's definition on the exact frequency, and on the
                    // reference's compliant tester, pooled over the same
                    // headers the chain pools.
                    let mut ideal = crate::signal::bt::piconet::Deviation::default();
                    let mut mask = crate::signal::bt::piconet::Deviation::default();
                    let fine = 32e6;
                    let taps = mask_filter(fine);
                    for (burst, (bits, trailer)) in bursts.iter().zip(&packets) {
                        let end = trailer + 4 + 54;
                        let air = &bits[*trailer..end];
                        let one = crate::signal::bt::piconet::Deviation::of(
                            air,
                            &centres(burst, *trailer, end),
                        );
                        ideal.settled.add(one.settled);
                        ideal.alternating.add(one.alternating);
                        let trace = Trace::from_iq(
                            &filter(&burst.iq(fine, burst.len_at(fine)), &taps),
                            fine,
                            burst.tx.symbol_rate,
                        );
                        let hz: Vec<f32> = (*trailer..end)
                            .map(|k| trace.at(k as f64 + 0.5).unwrap_or(f64::NAN) as f32)
                            .collect();
                        let one = crate::signal::bt::piconet::Deviation::of(air, &hz);
                        mask.settled.add(one.settled);
                        mask.alternating.add(one.alternating);
                    }
                    let mean_of = |s: &crate::signal::dsp::deviation::Sums| {
                        s.mean().map(|u| u.value()).unwrap_or(f64::NAN)
                    };
                    let (m1, m2) = (mean_of(&mask.settled), mean_of(&mask.alternating));
                    let (i1, i2) = (
                        ideal.settled.mean().map(|u| u.value()).unwrap_or(f64::NAN),
                        ideal
                            .alternating
                            .mean()
                            .map(|u| u.value())
                            .unwrap_or(f64::NAN),
                    );

                    let row = m.net.bt_piconets.iter().find(|p| p.lap == lap);
                    let captured = row.map(|p| p.headers.captured).unwrap_or(0);
                    let d = row.map(|p| p.headers.deviation).unwrap_or_default();
                    let show = |s: &crate::signal::dsp::deviation::Sums| match s.mean() {
                        Some(u) => format!("{:8.2} +-{:5.2}", u.value() / 1e3, u.sigma() / 1e3),
                        None => format!("{:>17}", "-"),
                    };
                    let ratio = match (d.settled.mean(), d.alternating.mean()) {
                        (Some(a), Some(b)) => format!("{:7.4}", b.value() / a.value()),
                        _ => format!("{:>7}", "-"),
                    };
                    eprintln!(
                        "{:>5} {:>4} {:>6} {:>2}/{:<2} | {} {} {} | {} {} {} | {} {:7.4} {:7.4}",
                        rate / 1e6,
                        snr,
                        cfo / 1e3,
                        captured,
                        PACKETS,
                        show(&d.settled),
                        khz(m1),
                        khz(i1),
                        show(&d.alternating),
                        khz(m2),
                        khz(i2),
                        ratio,
                        m2 / m1,
                        i2 / i1,
                    );
                }
            }
        }
    }
}
