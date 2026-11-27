// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The local floor under each bin, the depth an exposure has reached, and
//! the level chance does not pass.
//!
//! **What chance looks like.** A bin's power in one frame of noise is
//! exponential; its mean over T frames is Gamma distributed with shape T and
//! the same mean, so its spread about the mean is `1/√T`. Everything here is
//! that distribution, used exactly rather than through a normal
//! approximation, so a one-second exposure is judged as fairly as an hour.
//!
//! **The floor, robust to its lines.** Under each bin the floor is taken
//! from about a kilohertz around it: the mean of the quieter 80 % of a
//! block's bins, which no handful of lines moves, divided by what that
//! trimming takes off a Gamma mean (a closed form, below), and interpolated
//! between block centres.

use monas_dsp::special::erf;

use super::settings::Derived;

/// Chance's allowance: lines that pure noise may put over the threshold in
/// one exposure, over the whole usable band. One in ten exposures.
pub const ALPHA: f64 = 0.1;

/// The share of a block the floor is taken from: its quieter part.
pub const KEPT: f64 = 0.8;

/// Above this many frames, the survival function is the Wilson-Hilferty
/// normal approximation; at or below, the exact sum.
const EXACT_UP_TO: u64 = 200;

/// P(Gamma(t, 1) > y), the chance that a sum of `t` exponentials of mean
/// one exceeds `y`.
pub fn gamma_sf(t: u64, y: f64) -> f64 {
    if t <= EXACT_UP_TO {
        gamma_sf_exact(t, y)
    } else {
        gamma_sf_wh(t, y)
    }
}

/// `e^(−y) · Σ_{k<t} y^k / k!`, summed in logarithms so neither a large `y`
/// nor a large `t` underflows.
fn gamma_sf_exact(t: u64, y: f64) -> f64 {
    if y <= 0.0 {
        return 1.0;
    }
    let ln_y = y.ln();
    let mut ln_term = -y; // k = 0
    let mut terms = Vec::with_capacity(t as usize);
    for k in 0..t {
        if k > 0 {
            ln_term += ln_y - (k as f64).ln();
        }
        terms.push(ln_term);
    }
    let top = terms.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    (top + terms.iter().map(|l| (l - top).exp()).sum::<f64>().ln())
        .exp()
        .min(1.0)
}

/// Wilson and Hilferty: the cube root of a Gamma variate is nearly normal.
fn gamma_sf_wh(t: u64, y: f64) -> f64 {
    let t = t as f64;
    let v = 1.0 / (9.0 * t);
    let z = ((y / t).cbrt() - (1.0 - v)) / v.sqrt();
    0.5 * (1.0 - erf(z / std::f64::consts::SQRT_2))
}

/// The value of `x` (a T-frame mean over its true mean) that `p` of chance
/// exceeds, by bisection.
fn quantile_above(frames: u64, p: f64) -> f64 {
    let t = frames.max(1) as f64;
    let (mut lo, mut hi) = (0.0, 1.0);
    while gamma_sf(frames.max(1), hi * t) > p {
        hi *= 2.0;
    }
    for _ in 0..100 {
        let mid = 0.5 * (lo + hi);
        if gamma_sf(frames.max(1), mid * t) > p {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}

/// The level over the floor that chance passes, on noise alone, about
/// [`ALPHA`] times an exposure over `usable_bins` bins.
pub fn threshold(frames: u64, usable_bins: usize) -> f64 {
    quantile_above(frames, ALPHA / usable_bins.max(1) as f64)
}

/// What keeping the quieter [`KEPT`] of a block takes off a T-frame mean:
/// the mean of a Gamma(T, 1/T) variate below its `KEPT` quantile, over
/// `KEPT`. Since `x · f(x; T)` is `f(x; T + 1)` at this scale, that mean is
/// a survival function of shape `T + 1`, no integral needed.
pub fn trimmed_bias(frames: u64) -> f64 {
    let t = frames.max(1);
    let q = quantile_above(t, 1.0 - KEPT);
    (1.0 - gamma_sf(t + 1, q * t as f64)) / KEPT
}

/// The floor under every bin from the per-bin means; zero outside the
/// usable span.
pub fn local_floor(mean: &[f64], d: &Derived, frames: u64) -> Vec<f64> {
    let bias = trimmed_bias(frames);
    let mut centres = Vec::new();
    let mut levels = Vec::new();
    let mut scratch = Vec::with_capacity(d.block * 2);
    let mut start = d.usable.start;
    while start < d.usable.end {
        let mut end = (start + d.block).min(d.usable.end);
        // A last block shorter than half a block joins the one before.
        if d.usable.end - end < d.block / 2 {
            end = d.usable.end;
        }
        scratch.clear();
        scratch.extend_from_slice(&mean[start..end]);
        let keep = ((scratch.len() as f64 * KEPT).round() as usize).max(1);
        scratch.select_nth_unstable_by(keep - 1, f64::total_cmp);
        let m = scratch[..keep].iter().sum::<f64>() / keep as f64;
        centres.push(0.5 * (start + end) as f64);
        levels.push(m / bias);
        start = end;
    }
    let mut out = vec![0.0; mean.len()];
    let mut j = 0;
    for (k, slot) in out
        .iter_mut()
        .enumerate()
        .take(d.usable.end)
        .skip(d.usable.start)
    {
        let x = k as f64 + 0.5;
        while j + 1 < centres.len() && centres[j + 1] < x {
            j += 1;
        }
        *slot = if centres.len() == 1 || x <= centres[0] {
            levels[0]
        } else if j + 1 >= centres.len() {
            levels[centres.len() - 1]
        } else {
            let w = (x - centres[j]) / (centres[j + 1] - centres[j]);
            levels[j] * (1.0 - w) + levels[j + 1] * w
        };
    }
    out
}

/// The depth an exposure of `frames` has reached: one standard deviation
/// of chance about the floor, in dB.
pub fn depth_db(frames: u64) -> f64 {
    10.0 * (1.0 + 1.0 / (frames.max(1) as f64).sqrt()).log10()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::floor::settings::{derive, FloorSetting};
    use crate::signal::floor::testing::gamma_means;

    fn d(rate: f64) -> Derived {
        derive(&FloorSetting::default(), rate).unwrap()
    }

    /// Synthetic exposures: each bin a mean of `t` exponentials of mean one.
    #[test]
    fn the_floor_is_unbiased_at_any_exposure() {
        let d = d(125_000.0);
        for t in [1u64, 10, 100, 1000] {
            let mut seed = 5 + t;
            let mean = gamma_means(d.bins, t, &mut seed);
            let f = local_floor(&mean, &d, t);
            let got: f64 = d.usable.clone().map(|k| f[k]).sum::<f64>() / d.usable.len() as f64;
            assert!((got - 1.0).abs() < 0.005, "T = {t}: {got}");
        }
    }

    #[test]
    fn the_floor_ignores_its_lines() {
        let d = d(125_000.0);
        let mut seed = 9;
        let mut mean = gamma_means(d.bins, 100, &mut seed);
        let clean = local_floor(&mean, &d, 100);
        let k0 = d.usable.start + 3 * d.block;
        mean[k0..k0 + 20].iter_mut().for_each(|m| *m = 100.0);
        let lined = local_floor(&mean, &d, 100);
        let c = k0 + 10;
        assert!(
            (lined[c] / clean[c] - 1.0).abs() < 0.01,
            "{} {}",
            lined[c],
            clean[c]
        );
    }

    #[test]
    fn gamma_sf_matches_the_exponential_at_one_frame() {
        for y in [0.1, 1.0, 5.0, 20.0] {
            assert!((gamma_sf(1, y) - (-y).exp()).abs() < 1e-12 * (-y).exp().max(1e-300) + 1e-15);
        }
    }

    /// Where the exact sum hands over to the approximation, the two agree
    /// down to the deepest tail a threshold is ever set in: `ALPHA` over the
    /// most bins, about 3e-8.
    #[test]
    fn the_approximation_agrees_with_the_sum_where_they_meet() {
        let deepest = ALPHA / crate::signal::floor::settings::MAX_BINS as f64;
        let mut x = 1.02;
        while gamma_sf_exact(200, x * 200.0) >= deepest {
            let (a, b) = (gamma_sf_exact(200, x * 200.0), gamma_sf_wh(200, x * 200.0));
            assert!((a / b - 1.0).abs() < 0.05, "{x}: {a} {b}");
            x += 0.02;
        }
        assert!(x > 1.3, "the range checked reached {x}");
    }

    #[test]
    fn the_trim_bias_has_the_exponentials_closed_form_at_one_frame() {
        let closed = (1.0 - 0.2 * (1.0 + 5f64.ln())) / 0.8;
        assert!(
            (trimmed_bias(1) - closed).abs() < 1e-9,
            "{}",
            trimmed_bias(1)
        );
        assert!(trimmed_bias(1000) > 0.98 && trimmed_bias(1000) < 1.0);
    }

    /// Over the usable band, chance crosses the threshold about `ALPHA`
    /// times an exposure.
    #[test]
    fn chance_crosses_the_threshold_about_alpha_times() {
        let (bins, t, runs) = (100_000usize, 50u64, 400);
        let thr = threshold(t, bins);
        let mut seed = 77;
        let mut crossings = 0usize;
        for _ in 0..runs {
            crossings += gamma_means(bins, t, &mut seed)
                .iter()
                .filter(|&&x| x > thr)
                .count();
        }
        let per = crossings as f64 / runs as f64;
        assert!((0.05..=0.2).contains(&per), "{per} per exposure");
    }

    #[test]
    fn depth_shrinks_as_the_root_of_the_exposure() {
        assert!((depth_db(620) - 0.1710).abs() < 0.0005, "{}", depth_db(620));
        assert!((depth_db(1) - 10.0 * 2f64.log10()).abs() < 1e-12);
    }
}
