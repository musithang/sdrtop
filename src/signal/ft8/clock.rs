// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! When a sample was received, in UTC, from the stream's own positions and
//! the wall clock at each block's arrival.
//!
//! FT8's cycles start on the UTC quarter minute, so the receiver must know
//! which sample of its stream was taken when. The stream gives a position
//! for every block (`StreamBlock::first_pair`), exact and gapless; the wall
//! clock at the block's arrival gives the time, but late by whatever the USB
//! transfer, the driver and the queue held it for, and late by a different
//! amount each time.
//!
//! **The lower envelope, not the mean.** A block can arrive late, never
//! early: its last sample cannot reach us before it was taken. So over a
//! window the smallest `arrival - position / rate` is the best estimate of
//! when position zero was taken, and the one least disturbed by a slow
//! delivery. What it cannot remove is the part of the delay every block
//! shares (the transfer's own length, the driver's buffer): that is why the
//! clock check says it judges this computer's clock *and* the receive path,
//! never the clock alone.

use std::collections::VecDeque;

/// How far back the envelope looks, in stream seconds: long enough to catch
/// a quick delivery among slow ones, short against the sample clock's drift
/// (at 20 ppm, 30 s moves the estimate by 0.6 ms).
const WINDOW_SECONDS: f64 = 30.0;

/// How much later than the fastest delivery in the window the deliveries
/// arrived: what the receive path adds on top of the delay every block
/// shares, which this clock cannot see.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lateness {
    /// Half the deliveries came later than the fastest by more, ms.
    pub median_ms: f64,
    /// One in ten came later than the fastest by more, ms.
    pub p90_ms: f64,
}

#[derive(Clone, Debug, Default)]
pub struct StreamClock {
    rate: f64,
    /// Per block: its end in stream seconds, and `arrival - end`.
    seen: VecDeque<(f64, f64)>,
}

impl StreamClock {
    pub fn new(rate: f64) -> Self {
        Self {
            rate,
            seen: VecDeque::new(),
        }
    }

    /// A block of `pairs` starting at `first_pair` arrived at `arrived_unix`.
    pub fn push(&mut self, first_pair: u64, pairs: u64, arrived_unix: f64) {
        if self.rate <= 0.0 {
            return;
        }
        let end = (first_pair + pairs) as f64 / self.rate;
        self.seen.push_back((end, arrived_unix - end));
        while self
            .seen
            .front()
            .is_some_and(|&(t, _)| end - t > WINDOW_SECONDS)
        {
            self.seen.pop_front();
        }
    }

    /// UTC, as Unix seconds, of stream position `pair` (fractional allowed).
    /// `None` before the first block.
    pub fn unix_of(&self, pair: f64) -> Option<f64> {
        let zero = self
            .seen
            .iter()
            .map(|&(_, offset)| offset)
            .min_by(f64::total_cmp)?;
        Some(zero + pair / self.rate)
    }

    /// The window's deliveries against its fastest. `None` with fewer than
    /// two.
    pub fn lateness(&self) -> Option<Lateness> {
        if self.seen.len() < 2 {
            return None;
        }
        let min = self
            .seen
            .iter()
            .map(|&(_, offset)| offset)
            .min_by(f64::total_cmp)?;
        let mut late: Vec<f64> = self
            .seen
            .iter()
            .map(|&(_, offset)| (offset - min) * 1000.0)
            .collect();
        late.sort_by(f64::total_cmp);
        let n = late.len();
        let median = if n.is_multiple_of(2) {
            (late[n / 2 - 1] + late[n / 2]) / 2.0
        } else {
            late[n / 2]
        };
        let p90 = late[((0.9 * n as f64).ceil() as usize).clamp(1, n) - 1];
        Some(Lateness {
            median_ms: median,
            p90_ms: p90,
        })
    }

    /// Forget everything: a break, a new stream, a retune.
    pub fn reset(&mut self, rate: f64) {
        *self = Self::new(rate);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::dsp::testkit::Rng;

    #[test]
    fn the_fastest_delivery_sets_the_time() {
        // Position zero taken at Unix 1000.0; blocks of 0.1 s delivered
        // 5 to 60 ms after their last sample, at random.
        let rate = 192_000.0;
        let mut clock = StreamClock::new(rate);
        let mut rng = Rng::new(1);
        let block = 19_200u64;
        for k in 0..100u64 {
            let end_s = ((k + 1) * block) as f64 / rate;
            let late = 0.005 + 0.055 * rng.unit();
            clock.push(k * block, block, 1000.0 + end_s + late);
        }
        let t = clock.unix_of(0.0).unwrap();
        assert!((t - 1000.005).abs() < 0.001, "{t}");
        // A later sample by its position.
        let t = clock.unix_of(rate * 7.5).unwrap();
        assert!((t - 1007.505).abs() < 0.001, "{t}");
    }

    #[test]
    fn one_slow_block_moves_nothing() {
        let rate = 12_000.0;
        let mut clock = StreamClock::new(rate);
        for k in 0..50u64 {
            let late = if k == 20 { 2.0 } else { 0.010 };
            clock.push(k * 1200, 1200, 500.0 + (k + 1) as f64 * 0.1 + late);
        }
        assert!((clock.unix_of(0.0).unwrap() - 500.010).abs() < 1e-9);
    }

    #[test]
    fn the_window_lets_old_deliveries_go() {
        // A delivery 40 s ago that was impossibly early (a clock step) stops
        // counting once the window has passed it.
        let rate = 1000.0;
        let mut clock = StreamClock::new(rate);
        clock.push(0, 100, 100.1 - 0.5);
        for k in 1..400u64 {
            clock.push(k * 100, 100, 100.0 + (k + 1) as f64 * 0.1 + 0.02);
        }
        assert!((clock.unix_of(0.0).unwrap() - 100.02).abs() < 1e-9);
    }

    #[test]
    fn nothing_known_before_the_first_block() {
        assert_eq!(StreamClock::new(12_000.0).unix_of(0.0), None);
    }

    #[test]
    fn lateness_is_measured_from_the_fastest_delivery() {
        let rate = 12_000.0;
        let mut clock = StreamClock::new(rate);
        assert_eq!(clock.lateness(), None);
        for (k, late_ms) in [10.0, 13.0, 11.0, 12.0].into_iter().enumerate() {
            let k = k as u64;
            clock.push(
                k * 1200,
                1200,
                500.0 + (k + 1) as f64 * 0.1 + late_ms / 1000.0,
            );
        }
        let l = clock.lateness().unwrap();
        assert!((l.median_ms - 1.5).abs() < 1e-6, "{l:?}");
        assert!((l.p90_ms - 3.0).abs() < 1e-6, "{l:?}");
    }
}
