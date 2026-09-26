// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Where a GFSK transmitter's deviation can be read from ordinary traffic:
//! every on-air bit whose neighbours make it one of the test suites' own
//! readings ([`suite_readings`]).
//!
//! Lifted out of `signal::ble::measure` when classic Bluetooth needed the
//! same reading (net-ux-polish-plan 6.4): the two protocol modules do not
//! know each other (foundation design section 10, rule 2), and what they
//! share is GFSK, which is this module's subject, not either protocol's.

use super::uncertainty::Uncertain;

/// How many readings a bit is read at, evenly spaced across it: the LE test
/// suite's minimum (RF-PHY.TS.4.2.1 TP/TRM-LE/CA/BV-05-C step 5), which
/// covers BR's four (RF.TS.p35 RF/TRM/CA/BV-07-C d).
pub const READINGS_PER_BIT: usize = 32;

/// The test suites' delta-f1 and delta-f2 readings, from whatever bits were
/// sent: `at(x)` is the frequency at `x` bit periods from the start of
/// `bits[0]`. `None` unless settled bits of both polarities occur, since the
/// carrier they are measured from is read from them.
///
/// **What the suites measure, and why any traffic has it.** They command a
/// device to send `00001111` and `10101010` and read bits 2, 3, 6, 7 of the
/// first and every bit of the second. What makes those bits the right ones
/// is their neighbours: the first kind has the same bit either side, the
/// second the opposite. A GFSK symbol with BT 0.5 moves the frequency at a
/// bit two away by about 1e-8 of the deviation, so a bit's shape is decided
/// by its two neighbours and nothing further. Every bit whose neighbours
/// both equal it is therefore read as the suites read bits 2, 3, 6, 7, and
/// every bit whose neighbours both differ as they read an alternation, in
/// ordinary whitened traffic, with no test mode. `conformance` holds this
/// to the suites' own figures on their own patterns.
///
/// **Read as the suites read.** A settled bit gives the mean of its
/// [`READINGS_PER_BIT`] readings (delta-f1, "the average of the samples
/// within the bit period"), as a distance from the carrier.
///
/// **An alternating bit gives its reading at the centre,** where the suites
/// ask for "the maximum deviation ... within the bit period". For a GFSK
/// alternation the two are one number: the pulse is symmetric, so the peak
/// sits at the centre (`conformance` measures them within 0.1 %, which is
/// the suites' own 32-reading grid missing the peak by 1/64 of a bit). They part
/// where a tester on a cable never looks: the largest of 32 noisy readings is
/// biased upward by the noise, and an adjacent channel's ripple through the
/// discriminator lifts it further, where the centre only scatters. Measured
/// through the measurement filter with a neighbour 25 dB down, the maximum
/// read an ideal BR transmitter 18 % high and the centre 0.5 % low.
///
/// **Signed, towards the bit's own side, not a distance.** The suites say
/// "deviation", and on a cable a distance and a signed deviation are one
/// number. Over the air they are not: the distance of a noisy reading from
/// the carrier is lifted by the noise whichever way the noise pushed, and
/// the signed reading is not. That is only part of what noise does to a
/// single reading at the centre, and the measurement says so: at 20 dB in
/// 1 MHz the signed df2 still reads BR 3 to 11 % high at 20 Msps and LE 2
/// to 7 % low, each with a sigma of 6 to 18 kHz, where at 40 dB both are
/// within 1 %. What else is in it is open, and measured by the harness. The suites take the carrier as the mean over
/// each eight-bit test sequence; here it is the midpoint of the settled
/// ones' and zeros' means over the whole span, which a symmetric pulse
/// puts on the carrier just the same. A drift over the span moves the
/// midpoint to the middle of it, and one polarity's readings up and the
/// other's down by the same amount, which the averages of both then cancel.
pub fn suite_readings(bits: &[bool], at: impl Fn(f64) -> f32) -> Option<(Vec<f32>, Vec<f32>)> {
    let n = READINGS_PER_BIT;
    let within = |k: usize| -> Vec<f32> {
        (0..n)
            .map(|j| at(k as f64 + (j as f64 + 0.5) / n as f64))
            .collect()
    };
    let neighbours = |k: usize| (bits[k - 1], bits[k], bits[k + 1]);
    let mut settled = Vec::new();
    let mut alternating = Vec::new();
    for k in 1..bits.len().saturating_sub(1) {
        match neighbours(k) {
            (a, b, c) if a == b && b == c => {
                let r = within(k);
                settled.push((b, r.iter().map(|&f| f as f64).sum::<f64>() / n as f64));
            }
            (a, b, c) if a != b && b != c => alternating.push((b, at(k as f64 + 0.5) as f64)),
            _ => {}
        }
    }
    let side = |one: bool| {
        let (sum, count) = settled
            .iter()
            .filter(|(b, _)| *b == one)
            .fold((0.0, 0usize), |(s, c), (_, m)| (s + m, c + 1));
        (count > 0).then(|| sum / count as f64)
    };
    let centre = (side(true)? + side(false)?) / 2.0;
    // Towards the bit's own side of the carrier, signed.
    let toward = |one: bool, f: f64| if one { f - centre } else { centre - f } as f32;
    Some((
        settled.iter().map(|&(b, m)| toward(b, m)).collect(),
        alternating.iter().map(|&(b, f)| toward(b, f)).collect(),
    ))
}

/// Readings gathered over many bursts as sums, so they are refined with an
/// increment inside a lock and turned into a mean only when read (the
/// census's own reason for keeping sums).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sums {
    pub n: u64,
    pub sum: f64,
    pub sum_sq: f64,
}

impl Sums {
    pub fn of(values: &[f32]) -> Self {
        let mut s = Self::default();
        for &v in values {
            s.n += 1;
            s.sum += v as f64;
            s.sum_sq += (v as f64) * (v as f64);
        }
        s
    }

    pub fn add(&mut self, other: Sums) {
        self.n += other.n;
        self.sum += other.sum;
        self.sum_sq += other.sum_sq;
    }

    /// The mean and its standard error; `None` below two readings, where no
    /// spread can be stated.
    pub fn mean(&self) -> Option<Uncertain> {
        if self.n < 2 {
            return None;
        }
        let n = self.n as f64;
        let mean = self.sum / n;
        let var = ((self.sum_sq - n * mean * mean) / (n - 1.0)).max(0.0);
        Some(Uncertain::from_variance(mean, var / n))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A trace that is exactly the carrier plus the deviation in each bit's
    /// direction, held flat: every settled and every alternating bit reads
    /// the deviation, and the carrier cancels; the ends and bits with one
    /// like and one unlike neighbour give nothing.
    #[test]
    fn suite_readings_take_the_bits_their_neighbours_qualify() {
        let bits = [true, true, true, false, true, false, false, false, true];
        let at = |x: f64| {
            let k = (x.floor() as usize).min(bits.len() - 1);
            7_000.0 + if bits[k] { 100.0 } else { -100.0 }
        };
        let (settled, alternating) = suite_readings(&bits, at).unwrap();
        // Settled: 1 (1,1,1) and 6 (0,0,0). Alternating: 3 (1,0,1), 4 (0,1,0).
        assert_eq!(settled, vec![100.0, 100.0]);
        assert_eq!(alternating, vec![100.0, 100.0]);
        // No settled zero: no carrier, no reading.
        assert!(suite_readings(&[true, true, true, false, true], at).is_none());
    }

    #[test]
    fn sums_give_the_mean_and_refuse_one_reading() {
        let mut s = Sums::of(&[2.0]);
        assert_eq!(s.mean(), None);
        s.add(Sums::of(&[4.0, 6.0]));
        let m = s.mean().unwrap();
        assert!((m.value() - 4.0).abs() < 1e-12);
        // Sample variance 4, over three readings.
        assert!((m.sigma() - (4.0f64 / 3.0).sqrt()).abs() < 1e-9, "{m:?}");
    }
}
