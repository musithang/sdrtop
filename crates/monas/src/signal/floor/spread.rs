// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Whether a bin behaves like chance, like a steady line, or like clicks.
//!
//! A bin's **spread** is its power's standard deviation over its mean,
//! across the frames. For chance (noise whose power per frame is
//! exponential) it is exactly one, whatever the level. A steady line adds
//! power that does not change from frame to frame, so the spread falls
//! below one; a source that clicks puts power in some frames and not
//! others, so it rises above.
//!
//! **Its uncertainty, for chance.** By the delta method on the exponential's
//! moments (the variance of the sample variance `8μ⁴/T`, its covariance with
//! the mean `2μ³/T`), the spread's standard error over T frames is exactly
//! `1/√T`; held by a simulation test.

use super::frames::Sums;
use super::settings::Derived;

/// How many standard errors a bin's spread must stand from one to be other
/// than chance.
pub const MARGIN: f64 = 3.0;

/// A bin's spread from its sums over `frames`.
pub fn spread(s1: f64, s2: f64, frames: u64) -> f64 {
    let t = frames.max(1) as f64;
    let m = s1 / t;
    if m <= 0.0 {
        return f64::NAN;
    }
    (s2 / t - m * m).max(0.0).sqrt() / m
}

/// The spread's standard error for chance over `frames`.
pub fn spread_sigma_chance(frames: u64) -> f64 {
    1.0 / (frames.max(1) as f64).sqrt()
}

/// What a bin behaves like.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinKind {
    /// Within [`MARGIN`] standard errors of one.
    Chance,
    /// Above: power in some frames and not in others.
    Impulsive,
    /// Below: power that holds still.
    Steady,
}

/// What each usable bin behaves like, in order.
pub fn kinds<'a>(sums: &'a Sums, d: &Derived) -> impl Iterator<Item = BinKind> + 'a {
    let band = MARGIN * spread_sigma_chance(sums.frames);
    let frames = sums.frames;
    d.usable.clone().map(move |k| {
        let r = spread(sums.s1[k], sums.s2[k], frames);
        if r > 1.0 + band {
            BinKind::Impulsive
        } else if r < 1.0 - band {
            BinKind::Steady
        } else {
            BinKind::Chance
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::floor::frames::{Frames, Sums};
    use crate::signal::floor::settings::{derive, FloorSetting};
    use crate::signal::floor::testing::{add, impulses, noise, tone};

    fn exponential_sums(bins: usize, t: u64, seed: &mut u64) -> (Vec<f64>, Vec<f64>) {
        let (mut s1, mut s2) = (vec![0.0; bins], vec![0.0; bins]);
        for _ in 0..t {
            for k in 0..bins {
                *seed ^= *seed << 13;
                *seed ^= *seed >> 7;
                *seed ^= *seed << 17;
                let u = ((*seed >> 11) as f64 + 0.5) / (1u64 << 53) as f64;
                let p = -u.ln();
                s1[k] += p;
                s2[k] += p * p;
            }
        }
        (s1, s2)
    }

    #[test]
    fn chance_has_a_spread_of_one() {
        let mut seed = 3;
        let (s1, s2) = exponential_sums(100_000, 400, &mut seed);
        let r: Vec<f64> = s1
            .iter()
            .zip(&s2)
            .map(|(a, b)| spread(*a, *b, 400))
            .collect();
        let mean = r.iter().sum::<f64>() / r.len() as f64;
        assert!((mean - 1.0).abs() < 0.005, "{mean}");
    }

    #[test]
    fn its_standard_error_is_one_over_root_t() {
        let mut seed = 4;
        let (s1, s2) = exponential_sums(100_000, 400, &mut seed);
        let r: Vec<f64> = s1
            .iter()
            .zip(&s2)
            .map(|(a, b)| spread(*a, *b, 400))
            .collect();
        let mean = r.iter().sum::<f64>() / r.len() as f64;
        let sd = (r.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / r.len() as f64).sqrt();
        assert!((sd / spread_sigma_chance(400) - 1.0).abs() < 0.1, "{sd}");
    }

    /// Clicks make a bin's power jump from frame to frame more than chance
    /// does; a steady tone makes it jump less. Two streams, since clicks
    /// loud enough to see also swamp the tone's bin.
    #[test]
    fn impulses_raise_the_spread_and_a_steady_tone_lowers_it() {
        let rate = 12_000.0;
        let d = derive(&FloorSetting::default(), rate).unwrap();
        let n = rate as usize;
        let run = |clicks: bool| {
            let (mut f, mut sums) = (Frames::new(&d), Sums::new(&d));
            let (mut seed, mut phase) = (9, 0.0);
            let a = (0.01f64 * (1.5 * 10.0 / n as f64).sqrt()) as f32;
            for k in 0..300u64 {
                let mut iq = noise(n, 0.01, &mut seed);
                if clicks {
                    // A burst of clicks in every third second.
                    if k % 3 == 0 {
                        iq = add(iq, &impulses(n, 200, 0.5, &mut seed));
                    }
                } else {
                    iq = add(iq, &tone(rate, 1500.0, a, k * n as u64, n, &mut phase));
                }
                f.push(&iq, false, &mut sums);
            }
            kinds(&sums, &d).collect::<Vec<BinKind>>()
        };
        let steady = run(false);
        assert_eq!(
            steady[(1500.0 + rate / 2.0) as usize - d.usable.start],
            BinKind::Steady
        );
        let clicked = run(true);
        let impulsive = clicked.iter().filter(|k| **k == BinKind::Impulsive).count();
        assert!(
            impulsive > clicked.len() / 2,
            "{impulsive} of {}",
            clicked.len()
        );
    }
}
