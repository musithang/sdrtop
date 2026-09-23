// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Where a GFSK transmitter's deviation can be read from ordinary traffic:
//! the ends of settled runs and of alternating runs of on-air symbols.
//!
//! Lifted out of `signal::ble::measure` when classic Bluetooth needed the
//! same reading (net-ux-polish-plan 6.4): the two protocol modules do not
//! know each other (foundation design section 10, rule 2), and what they
//! share is GFSK, which is this module's subject, not either protocol's.
//! `signal::ble::measure`'s module doc has why these two run shapes stand in
//! for the specification's `00001111` and `10101010` test patterns.

use super::uncertainty::Uncertain;

/// How many like, or alternating, symbols in a row counts as settled:
/// `00001111`'s four-symbol runs and `10101010`'s alternation both
/// generalise to this one number.
pub const SETTLED_RUN: usize = 4;

/// Whether the `SETTLED_RUN` bits ending at `i` (inclusive) are all one
/// value.
fn ends_settled_run(bits: &[bool], i: usize) -> bool {
    i + 1 >= SETTLED_RUN
        && bits[i + 1 - SETTLED_RUN..=i]
            .windows(2)
            .all(|w| w[0] == w[1])
}

/// Whether the `SETTLED_RUN` bits ending at `i` (inclusive) strictly
/// alternate.
fn ends_alternating_run(bits: &[bool], i: usize) -> bool {
    i + 1 >= SETTLED_RUN
        && bits[i + 1 - SETTLED_RUN..=i]
            .windows(2)
            .all(|w| w[0] != w[1])
}

/// The deviation magnitudes, in the unit `samples` carries, at the end of
/// every settled run and of every alternating run in `bits`: the delta-f1
/// and delta-f2 readings one burst supplies. `samples[i]` is the
/// discriminator reading at symbol `i`, measured from the carrier.
pub fn run_ends(bits: &[bool], samples: &[f32]) -> (Vec<f32>, Vec<f32>) {
    let n = bits.len().min(samples.len());
    let settled = (0..n)
        .filter(|&i| ends_settled_run(bits, i))
        .map(|i| samples[i].abs())
        .collect();
    let alternating = (0..n)
        .filter(|&i| ends_alternating_run(bits, i))
        .map(|i| samples[i].abs())
        .collect();
    (settled, alternating)
}

/// The carrier, from the burst itself: the midpoint of the mean reading at
/// the ends of settled runs of ones and of zeros, or `None` unless both
/// occur. A reference that owes nothing to a tracker running ahead of the
/// burst: a receiver's fast DC tracker follows a long run a little and
/// shrinks the deviation it is subtracted from (measured on classic
/// Bluetooth: 149 kHz read for 160 sent), which a centre taken from both
/// polarities at once does not.
pub fn settled_centre(bits: &[bool], samples: &[f32]) -> Option<f32> {
    let n = bits.len().min(samples.len());
    let (mut one, mut zero) = ((0.0f64, 0u32), (0.0f64, 0u32));
    for i in (0..n).filter(|&i| ends_settled_run(bits, i)) {
        let slot = if bits[i] { &mut one } else { &mut zero };
        slot.0 += samples[i] as f64;
        slot.1 += 1;
    }
    (one.1 > 0 && zero.1 > 0)
        .then(|| ((one.0 / one.1 as f64 + zero.0 / zero.1 as f64) / 2.0) as f32)
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

    #[test]
    fn run_ends_find_both_shapes_and_their_readings() {
        //            0      1      2      3      4      5      6      7      8
        let bits = [true, true, true, true, false, true, false, true, true];
        let samples = [1.0, 2.0, 3.0, 4.0, -5.0, 6.0, -7.0, 8.0, 9.0];
        let (settled, alternating) = run_ends(&bits, &samples);
        assert_eq!(settled, vec![4.0]);
        // 3..=6 is 1,0,1,0; 4..=7 is 0,1,0,1.
        assert_eq!(alternating, vec![7.0, 8.0]);
    }

    /// The centre is the midpoint of the two polarities' settled readings,
    /// whatever the carrier offset, and needs both.
    #[test]
    fn the_centre_is_read_from_both_polarities() {
        let bits = [true, true, true, true, false, false, false, false];
        let samples = [30.0, 40.0, 50.0, 60.0, -20.0, -30.0, -35.0, -40.0];
        assert_eq!(settled_centre(&bits, &samples), Some(10.0));
        assert_eq!(settled_centre(&bits[..4], &samples[..4]), None);
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
