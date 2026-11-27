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
//!
//! **Steady or fading.** A line's own spread tells a steady carrier from
//! one that fades as a sky wave does. With `a` the line's power over the
//! noise's in its bin, a steady line's frame power is noncentral (mean
//! `1 + a`, variance `1 + 2a`), so its spread is `√(1 + 2a)/(1 + a)`; a
//! Rayleigh-fading line plus noise is again exponential, spread one. Each
//! has a standard error by the same delta method on its own moments; the
//! two are told apart only when they stand three combined errors apart,
//! and otherwise the answer is how long it would take.

use super::frames::Sums;
use super::settings::{grouped, Derived};

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

/// What a line's power does from frame to frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Steadiness {
    /// Holds still: a carrier reaching the antenna by one way.
    Steady,
    /// Wanders as Rayleigh fading does.
    Fading,
}

/// `T` times the variance of the spread over `T` frames, by the delta
/// method, from the frame power's mean, variance and third and fourth
/// central moments.
fn spread_variance(mu: f64, var: f64, mu3: f64, mu4: f64) -> f64 {
    (mu4 - var * var) / (4.0 * var * mu * mu) + var * var / mu.powi(4) - mu3 / mu.powi(3)
}

/// `T` times the variance of a steady line's spread at `a` times the noise:
/// the frame power is half a noncentral chi-square of two degrees with
/// noncentrality `2a`.
fn steady_variance(a: f64) -> f64 {
    let var = 1.0 + 2.0 * a;
    let mu4 = 3.0 * var * var + 6.0 * (1.0 + 4.0 * a);
    spread_variance(1.0 + a, var, 2.0 + 6.0 * a, mu4)
}

/// A steady line's spread at `a` times the noise in its bin, and its
/// standard error over `frames`.
pub fn steady_spread(a: f64, frames: u64) -> (f64, f64) {
    let t = frames.max(1) as f64;
    (
        (1.0 + 2.0 * a).sqrt() / (1.0 + a),
        (steady_variance(a) / t).sqrt(),
    )
}

/// The frames that set the steady and the fading spread three combined
/// standard errors apart at `a`.
fn frames_to_separate(a: f64) -> f64 {
    let gap = 1.0 - (1.0 + 2.0 * a).sqrt() / (1.0 + a);
    (MARGIN * MARGIN * (steady_variance(a) + 1.0) / (gap * gap)).ceil()
}

/// The weakest line, as a multiple of the noise in its bin, whose steady
/// and fading spreads stand three combined standard errors apart over
/// `frames`.
pub fn boundary(frames: u64) -> f64 {
    let t = frames.max(1) as f64;
    let (mut lo, mut hi) = (-6.0f64, 8.0f64);
    for _ in 0..100 {
        let mid = 0.5 * (lo + hi);
        if frames_to_separate(10f64.powf(mid)) > t {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    10f64.powf(hi)
}

/// "needs about N s", N to two significant figures.
fn needs(frames: f64, frame_s: f64) -> String {
    let s = (frames * frame_s).max(1.0);
    let unit = 10f64.powf((s.log10().floor() - 1.0).max(0.0));
    let n = ((s / unit).ceil() * unit) as usize;
    format!(
        "cannot tell at this strength and exposure: needs about {} s",
        grouped(n)
    )
}

/// Whether a line of `excess` (its mean over the floor, a ratio) and spread
/// `r` over `frames` of `frame_s` seconds is steady or fading, or why that
/// cannot be told yet.
pub fn steadiness(excess: f64, r: f64, frames: u64, frame_s: f64) -> Result<Steadiness, String> {
    let a = excess - 1.0;
    if a.is_nan() || a <= 0.0 || r.is_nan() {
        return Err("cannot tell: the line stands no higher than the floor".into());
    }
    let (rs, ses) = steady_spread(a, frames);
    let sef = spread_sigma_chance(frames);
    if 1.0 - rs <= MARGIN * (ses * ses + sef * sef).sqrt() {
        return Err(needs(frames_to_separate(a), frame_s));
    }
    let near_steady = (r - rs).abs() <= MARGIN * ses;
    let near_fading = (r - 1.0).abs() <= MARGIN * sef;
    match (near_steady, near_fading) {
        (true, false) => Ok(Steadiness::Steady),
        (false, true) => Ok(Steadiness::Fading),
        (true, true) => {
            // Both bands reach r: wait until they part.
            let vs = steady_variance(a).sqrt();
            let gap = 1.0 - rs;
            Err(needs((MARGIN * (vs + 1.0) / gap).powi(2).ceil(), frame_s))
        }
        (false, false) => Err(format!(
            "cannot tell: a spread of {r:.3} fits neither a steady line ({rs:.3}) nor a fading one (1)"
        )),
    }
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

    /// The peak bin's excess over the floor and its spread, after `t`
    /// one-second frames of noise and a tone at `a` times one bin's noise,
    /// steady or fading.
    fn exposed(a: f64, t: u64, fading: bool) -> (f64, f64) {
        use crate::signal::floor::chance::local_floor;
        use crate::signal::floor::testing::{rayleigh_tone, Fading};
        let rate = 12_000.0;
        let d = derive(&FloorSetting::default(), rate).unwrap();
        let n = rate as usize;
        let (mut f, mut sums) = (Frames::new(&d), Sums::new(&d));
        let (mut seed, mut phase, mut fade) = (21, 0.0, Fading::new(5));
        let amp = (0.01f64 * (1.5 * a / n as f64).sqrt()) as f32;
        for k in 0..t {
            let iq = noise(n, 0.01, &mut seed);
            let line = if fading {
                rayleigh_tone(rate, 1500.0, amp, 3.0, n, &mut fade)
            } else {
                tone(rate, 1500.0, amp, k * n as u64, n, &mut phase)
            };
            f.push(&add(iq, &line), false, &mut sums);
        }
        let frames = sums.frames;
        let mean: Vec<f64> = sums.s1.iter().map(|v| v / frames as f64).collect();
        let floor = local_floor(&mean, &d, frames);
        let bin = (1500.0 + rate / 2.0) as usize;
        (
            mean[bin] / floor[bin],
            spread(sums.s1[bin], sums.s2[bin], frames),
        )
    }

    #[test]
    fn a_steady_tone_at_ten_db_is_steady() {
        let (x, r) = exposed(10.0, 300, false);
        assert_eq!(
            steadiness(x, r, 300, 1.0),
            Ok(Steadiness::Steady),
            "x {x} r {r}"
        );
    }

    #[test]
    fn a_fading_tone_at_ten_db_is_fading() {
        let (x, r) = exposed(10.0, 300, true);
        assert_eq!(
            steadiness(x, r, 300, 1.0),
            Ok(Steadiness::Fading),
            "x {x} r {r}"
        );
    }

    #[test]
    fn a_tone_at_one_db_cannot_be_told_in_ten_minutes() {
        // One decibel of excess over the floor, as a line is reported.
        let a = 10f64.powf(0.1) - 1.0;
        let (x, r) = exposed(a, 620, false);
        let told = steadiness(x, r, 620, 1.0);
        let why = told.expect_err("decided at +1 dB in 620 s");
        assert!(
            why.starts_with("cannot tell at this strength and exposure: needs about "),
            "{why}"
        );
        let secs: f64 = why
            .trim_start_matches("cannot tell at this strength and exposure: needs about ")
            .trim_end_matches(" s")
            .replace(' ', "")
            .parse()
            .unwrap();
        assert!(secs > 620.0, "{why}");
    }

    /// Under the boundary, a spread that happens to sit on the steady value
    /// and outside the fading band is still not taken for steady: the two
    /// do not stand apart at this strength, whatever one exposure shows.
    #[test]
    fn under_the_boundary_a_lucky_spread_is_not_taken_for_steady() {
        let a = 0.7 * boundary(600);
        let (rs, ses) = steady_spread(a, 600);
        let r = 1.0 - 3.2 * spread_sigma_chance(600);
        assert!((r - rs).abs() < MARGIN * ses, "precondition: {r} near {rs}");
        let why = steadiness(1.0 + a, r, 600, 1.0).expect_err("decided under the boundary");
        assert!(why.starts_with("cannot tell at this strength"), "{why}");
    }

    /// One frame's power of a steady line at `a` times the noise, the noise
    /// a unit complex Gaussian.
    fn steady_frames(a: f64, t: u64, seed: &mut u64) -> (f64, f64) {
        let n = noise(t as usize, 1.0, seed);
        let amp = a.sqrt();
        n.iter().fold((0.0, 0.0), |(s1, s2), z| {
            let p = (amp + z.re as f64).powi(2) + (z.im as f64).powi(2);
            (s1 + p, s2 + p * p)
        })
    }

    /// The steady spread's delta-method standard error, against 4000
    /// exposures of 600 frames at 3 dB and 10 dB.
    #[test]
    fn the_steady_standard_error_holds_by_simulation() {
        let mut seed = 31;
        for a in [2.0, 10.0] {
            let r: Vec<f64> = (0..4000)
                .map(|_| {
                    let (s1, s2) = steady_frames(a, 600, &mut seed);
                    spread(s1, s2, 600)
                })
                .collect();
            let mean = r.iter().sum::<f64>() / r.len() as f64;
            let sd = (r.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / r.len() as f64).sqrt();
            let (want, sigma) = steady_spread(a, 600);
            assert!(
                (mean - want).abs() < 0.2 * sigma,
                "a {a}: {mean} against {want}"
            );
            assert!(
                (sd / sigma - 1.0).abs() < 0.1,
                "a {a}: sd {sd} against {sigma}"
            );
        }
    }

    /// For T = 600, the weakest excess decided steady in at least half of
    /// 200 exposures sits within 1 dB of where the formulas put the
    /// boundary.
    #[test]
    fn the_boundary_is_where_the_formulas_put_it() {
        let formula = boundary(600);
        let mut seed = 41;
        let mut weakest = None;
        for step in 0..120 {
            let a = 10f64.powf((10.0 * formula.log10() - 3.0 + 0.1 * step as f64) / 10.0);
            let steady = (0..200)
                .filter(|_| {
                    let (s1, s2) = steady_frames(a, 600, &mut seed);
                    let x = s1 / 600.0;
                    steadiness(x, spread(s1, s2, 600), 600, 1.0) == Ok(Steadiness::Steady)
                })
                .count();
            if steady >= 100 {
                weakest = Some(a);
                break;
            }
        }
        let weakest = weakest.expect("never decided steady within 9 dB above");
        let off = 10.0 * (weakest / formula).log10();
        assert!(
            off.abs() < 1.0,
            "weakest {weakest}, formula {formula}: {off} dB"
        );
    }
}
