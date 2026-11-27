// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Lines on one spacing: a grid, found only when chance could not have put
//! that many lines there.
//!
//! **Candidates from the differences.** A comb's teeth differ by its spacing
//! and its multiples, so the spacings that recur among the pairwise
//! differences between lines are the candidates. The estimate is only as
//! good as one difference, and a spacing off by a hertz walks a hundred
//! teeth a hundred hertz off the grid, so a grid is **grown** from one of
//! its lines: a few teeth either side first, the spacing refitted, then
//! twice as far, until no tooth further out could join. A grid is one
//! stretch, its gaps no longer than [`MAX_GAP`] positions or half its
//! members, so a line far away that happens to sit on the spacing is not
//! counted.
//!
//! **Chance, counted honestly.** Given the line that anchors it and the one
//! that set its spacing, each other line inside the grid's span would sit
//! within the tolerance of a tooth by chance with probability
//! `2 · tolerance / spacing`. The chance of as many members as were found
//! is that binomial tail, times every grid tried; a grid is kept when that
//! is under [`MAX_CHANCE`].

use super::lines::Line;

/// The fewest members a grid has.
pub const MIN_MEMBERS: usize = 5;

/// The most chance a kept grid may be.
pub const MAX_CHANCE: f64 = 1e-3;

/// The smallest spacing looked for, Hz.
pub const MIN_SPACING_HZ: f64 = 5.0;

/// The largest spacing looked for, Hz.
pub const MAX_SPACING_HZ: f64 = 20_000.0;

/// A grid's longest gap, in empty positions, before the stretch ends (or
/// half its members, if more).
pub const MAX_GAP: usize = 8;

const MAX_CANDIDATES: usize = 40;
const MAX_ANCHORS: usize = 12;

/// Lines on one spacing.
#[derive(Clone, Debug, PartialEq)]
pub struct Grid {
    /// The spacing, Hz, fitted over the members.
    pub spacing_hz: f64,
    /// Its standard uncertainty.
    pub spacing_sigma_hz: f64,
    /// Where the teeth sit: every `offset_hz + k · spacing_hz`, the offset
    /// taken in `0 .. spacing_hz`.
    pub offset_hz: f64,
    /// The members, as indices into the lines given.
    pub members: Vec<usize>,
    /// Positions inside the span with no line.
    pub gaps: usize,
    /// From the lowest member to the highest, Hz.
    pub span_hz: (f64, f64),
    /// The chance of so many members by accident, every grid tried counted.
    pub chance: f64,
}

/// What growing a grid from one line gives, before chance is counted.
struct Grown {
    spacing: f64,
    sigma: f64,
    offset: f64,
    members: Vec<(i64, usize)>,
}

/// The grids among `lines`, most certain first; `tol_floor_hz` the least
/// tolerance a tooth is matched with.
pub fn find_grids(lines: &[Line], tol_floor_hz: f64) -> Vec<Grid> {
    if lines.len() < MIN_MEMBERS {
        return Vec::new();
    }
    let mut order: Vec<usize> = (0..lines.len()).collect();
    order.sort_by(|&a, &b| lines[a].hz.total_cmp(&lines[b].hz));
    let hz: Vec<f64> = order.iter().map(|&i| lines[i].hz).collect();
    let mut sigmas: Vec<f64> = lines.iter().map(|l| l.hz_sigma).collect();
    sigmas.sort_by(f64::total_cmp);
    let tol = tol_floor_hz.max(3.0 * sigmas[sigmas.len() / 2]);

    // Recurring differences, in bins of the tolerance.
    let nb = ((MAX_SPACING_HZ - MIN_SPACING_HZ) / tol) as usize + 2;
    let (mut count, mut sum) = (vec![0u32; nb], vec![0f64; nb]);
    for i in 0..hz.len() {
        for j in i + 1..hz.len() {
            let d = hz[j] - hz[i];
            if d > MAX_SPACING_HZ {
                break;
            }
            if d >= MIN_SPACING_HZ {
                let b = ((d - MIN_SPACING_HZ) / tol) as usize;
                count[b] += 1;
                sum[b] += d;
            }
        }
    }
    let pair = |b: usize| count[b] + count[b + 1];
    let mut candidates: Vec<(u32, f64)> = (0..nb - 1)
        .filter(|&b| {
            let c = pair(b);
            c as usize >= MIN_MEMBERS - 1
                && (b == 0 || pair(b - 1) < c)
                && (b + 2 >= nb || pair(b + 1) <= c)
        })
        .map(|b| (pair(b), (sum[b] + sum[b + 1]) / pair(b) as f64))
        .collect();
    candidates.sort_by_key(|c| std::cmp::Reverse(c.0));
    candidates.truncate(MAX_CANDIDATES);

    let mut tried = 0usize;
    let mut grown: Vec<Grown> = Vec::new();
    for &(_, s0) in &candidates {
        let near = |x: f64| {
            hz.binary_search_by(|h| h.total_cmp(&(x - tol)))
                .unwrap_or_else(|i| i)
        };
        let has = |x: f64| {
            let i = near(x);
            i < hz.len() && hz[i] <= x + tol
        };
        let anchors: Vec<usize> = (0..hz.len())
            .filter(|&i| has(hz[i] + s0))
            .take(MAX_ANCHORS)
            .collect();
        let mut best: Option<Grown> = None;
        for a in anchors {
            tried += 1;
            if let Some(g) = grow(&hz, a, s0, tol) {
                if best
                    .as_ref()
                    .is_none_or(|b| g.members.len() > b.members.len())
                {
                    best = Some(g);
                }
            }
        }
        grown.extend(best);
    }

    let mut grids: Vec<Grid> = grown
        .into_iter()
        .filter(|g| g.members.len() >= MIN_MEMBERS)
        .map(|g| {
            let lo = g.members.first().unwrap();
            let hi = g.members.last().unwrap();
            let span = (hz[lo.1], hz[hi.1]);
            let inside = hz
                .iter()
                .filter(|&&h| h >= span.0 - tol && h <= span.1 + tol)
                .count();
            let p = (2.0 * tol / g.spacing).min(1.0);
            let chance = (binomial_tail(inside.saturating_sub(2), p, g.members.len() - 2)
                * tried as f64)
                .min(1.0);
            Grid {
                spacing_hz: g.spacing,
                spacing_sigma_hz: g.sigma,
                offset_hz: g.offset.rem_euclid(g.spacing),
                members: g.members.iter().map(|&(_, i)| order[i]).collect(),
                gaps: (hi.0 - lo.0 + 1) as usize - g.members.len(),
                span_hz: span,
                chance,
            }
        })
        .filter(|g| g.chance < MAX_CHANCE)
        .collect();
    grids.sort_by(|a, b| a.chance.total_cmp(&b.chance));
    // A grid most of whose members a more certain one already holds (its
    // half or double spacing, or the same grid from another anchor) is that
    // grid again.
    let mut kept: Vec<Grid> = Vec::new();
    for g in grids {
        let again = kept.iter().any(|k| {
            let shared = g.members.iter().filter(|m| k.members.contains(m)).count();
            shared * 5 >= g.members.len() * 4
        });
        if !again {
            kept.push(g);
        }
    }
    kept
}

/// A grid grown from line `a` (sorted index) at about spacing `s0`.
fn grow(hz: &[f64], a: usize, s0: f64, tol: f64) -> Option<Grown> {
    let (mut s, mut off) = (s0, hz[a]);
    let mut reach: i64 = 3;
    let widest = ((hz[hz.len() - 1] - hz[0]) / s0).ceil() as i64 + 1;
    let mut members: Vec<(i64, usize)>;
    let mut sigma;
    loop {
        // Every line within the tolerance of a tooth inside the reach, one
        // per tooth, the closest.
        let lo = off - reach as f64 * s - tol;
        let hi = off + reach as f64 * s + tol;
        let start = hz.partition_point(|&h| h < lo);
        let mut by_tooth: Vec<(i64, usize, f64)> = Vec::new();
        for (i, &h) in hz.iter().enumerate().skip(start) {
            if h > hi {
                break;
            }
            let n = ((h - off) / s).round() as i64;
            let miss = (h - off - n as f64 * s).abs();
            if miss <= tol {
                match by_tooth.last_mut() {
                    Some(last) if last.0 == n => {
                        if miss < last.2 {
                            *last = (n, i, miss);
                        }
                    }
                    _ => by_tooth.push((n, i, miss)),
                }
            }
        }
        // The stretch around the anchor's tooth with no gap too long.
        let anchor = by_tooth.iter().position(|t| t.1 == a)?;
        let max_gap = |m: usize| MAX_GAP.max(m / 2) as i64;
        let (mut l, mut r) = (anchor, anchor);
        loop {
            let m = r - l + 1;
            let mut grew = false;
            if l > 0 && by_tooth[l].0 - by_tooth[l - 1].0 - 1 <= max_gap(m) {
                l -= 1;
                grew = true;
            }
            if r + 1 < by_tooth.len() && by_tooth[r + 1].0 - by_tooth[r].0 - 1 <= max_gap(m) {
                r += 1;
                grew = true;
            }
            if !grew {
                break;
            }
        }
        members = by_tooth[l..=r].iter().map(|t| (t.0, t.1)).collect();
        if members.len() < 2 {
            return None;
        }
        let fit = fit(hz, &members, tol);
        s = fit.0;
        off = fit.1;
        sigma = fit.2;
        // Renumber from the refitted grid, the anchor's tooth staying zero.
        let shift = members.iter().find(|m| m.1 == a).map_or(0, |m| m.0);
        members.iter_mut().for_each(|m| m.0 -= shift);
        off += shift as f64 * s;
        let room = max_gap(members.len());
        let edge = members[0].0.abs().max(members[members.len() - 1].0.abs());
        if reach >= widest || reach > edge + room {
            break;
        }
        reach *= 2;
    }
    Some(Grown {
        spacing: s,
        sigma,
        offset: off,
        members,
    })
}

/// Least squares of frequency on tooth number: spacing, the frequency at
/// tooth zero, and the spacing's standard uncertainty.
fn fit(hz: &[f64], members: &[(i64, usize)], tol: f64) -> (f64, f64, f64) {
    let m = members.len() as f64;
    let nbar = members.iter().map(|t| t.0 as f64).sum::<f64>() / m;
    let hbar = members.iter().map(|t| hz[t.1]).sum::<f64>() / m;
    let sxx: f64 = members.iter().map(|t| (t.0 as f64 - nbar).powi(2)).sum();
    if sxx == 0.0 {
        return (f64::NAN, hbar, f64::INFINITY);
    }
    let sxy: f64 = members
        .iter()
        .map(|t| (t.0 as f64 - nbar) * (hz[t.1] - hbar))
        .sum();
    let s = sxy / sxx;
    let off = hbar - s * nbar;
    let resid = if members.len() > 2 {
        (members
            .iter()
            .map(|t| (hz[t.1] - off - s * t.0 as f64).powi(2))
            .sum::<f64>()
            / (m - 2.0))
            .sqrt()
    } else {
        tol
    };
    (s, off, resid / sxx.sqrt())
}

/// P(Binomial(n, p) ≥ k), summed in logarithms.
fn binomial_tail(n: usize, p: f64, k: usize) -> f64 {
    if k == 0 {
        return 1.0;
    }
    if k > n || p <= 0.0 {
        return 0.0;
    }
    if p >= 1.0 {
        return 1.0;
    }
    let mut ln_fact = vec![0.0f64; n + 1];
    for i in 1..=n {
        ln_fact[i] = ln_fact[i - 1] + (i as f64).ln();
    }
    let (lp, lq) = (p.ln(), (1.0 - p).ln());
    let terms: Vec<f64> = (k..=n)
        .map(|j| ln_fact[n] - ln_fact[j] - ln_fact[n - j] + j as f64 * lp + (n - j) as f64 * lq)
        .collect();
    let top = terms.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    (top + terms.iter().map(|t| (t - top).exp()).sum::<f64>().ln())
        .exp()
        .min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(hz: f64) -> Line {
        Line {
            hz,
            hz_sigma: 0.2,
            excess_db: 1.0,
            excess_sigma_db: 0.17,
            width_bins: 2,
            first_seen_s: 100.0,
            peak_bin: 0,
        }
    }

    fn uniform(seed: &mut u64) -> f64 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        (*seed >> 11) as f64 / (1u64 << 53) as f64
    }

    fn comb(
        spacing: f64,
        from: f64,
        teeth: usize,
        jitter: f64,
        keep: impl Fn(usize) -> bool,
        seed: &mut u64,
    ) -> Vec<Line> {
        (0..teeth)
            .filter(|&k| keep(k))
            .map(|k| line(from + k as f64 * spacing + jitter * (2.0 * uniform(seed) - 1.0)))
            .collect()
    }

    fn scatter(n: usize, from: f64, width: f64, seed: &mut u64) -> Vec<Line> {
        (0..n).map(|_| line(from + width * uniform(seed))).collect()
    }

    fn sorted(mut l: Vec<Line>) -> Vec<Line> {
        l.sort_by(|a, b| a.hz.total_cmp(&b.hz));
        l
    }

    #[test]
    fn a_comb_is_one_grid_with_its_exact_spacing() {
        let mut seed = 31;
        let mut lines = comb(999.957, 9_898_612.0, 60, 0.3, |_| true, &mut seed);
        lines.extend(scatter(100, 9_898_000.0, 61_000.0, &mut seed));
        let grids = find_grids(&sorted(lines), 2.0);
        assert_eq!(
            grids.len(),
            1,
            "{:?}",
            grids.iter().map(|g| g.spacing_hz).collect::<Vec<_>>()
        );
        let g = &grids[0];
        assert!(
            (g.spacing_hz - 999.957).abs() < 3.0 * g.spacing_sigma_hz,
            "{} ± {}",
            g.spacing_hz,
            g.spacing_sigma_hz
        );
        assert!(g.members.len() >= 58, "{}", g.members.len());
        assert!(g.chance < 1e-3);
    }

    #[test]
    fn a_random_scatter_is_not_a_grid() {
        for s in 0..50u64 {
            let mut seed = 1000 + s;
            let lines = sorted(scatter(184, 9_898_000.0, 164_000.0, &mut seed));
            let grids = find_grids(&lines, 2.0);
            assert!(
                grids.is_empty(),
                "seed {s}: {:?}",
                grids
                    .iter()
                    .map(|g| (g.spacing_hz, g.members.len()))
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn a_comb_with_gaps_is_still_one_grid_and_counts_them() {
        let mut seed = 5;
        let lines = sorted(comb(
            999.957,
            9_900_000.0,
            60,
            0.3,
            |k| k % 3 != 1,
            &mut seed,
        ));
        let grids = find_grids(&lines, 2.0);
        assert_eq!(grids.len(), 1);
        assert_eq!(grids[0].members.len(), 40);
        assert_eq!(grids[0].gaps, 20);
    }

    /// Where chance decides: seven lines on one spacing among many are a
    /// grid; five are not, once every grid tried is counted against them
    /// and the two lines that defined the spacing are not counted for it.
    #[test]
    fn five_evenly_spaced_lines_among_many_are_not_enough() {
        let found = |teeth: usize| {
            let mut seed = 2024;
            let mut lines = scatter(184, 9_898_000.0, 164_000.0, &mut seed);
            lines.extend(comb(200.0, 9_950_250.0, teeth, 0.2, |_| true, &mut seed));
            find_grids(&sorted(lines), 2.0)
                .iter()
                .any(|g| (g.spacing_hz - 200.0).abs() < 1.0)
        };
        assert!(found(7), "seven teeth are a grid");
        assert!(!found(5), "five teeth among 184 lines are chance's");
    }

    /// Two combs on one band: each its own grid, and the line they share
    /// belongs to both.
    #[test]
    fn two_interleaved_combs_are_two_grids() {
        let mut seed = 77;
        let mut lines = comb(1000.0, 10_000_000.0, 30, 0.3, |_| true, &mut seed);
        // Forty-one hertz apart from the first comb's tenth tooth on, the
        // shared line being that tooth.
        lines.extend(comb(41.0, 10_009_000.0, 40, 0.3, |k| k > 0, &mut seed));
        let lines = sorted(lines);
        let grids = find_grids(&lines, 2.0);
        let mut spacings: Vec<f64> = grids.iter().map(|g| g.spacing_hz.round()).collect();
        spacings.sort_by(f64::total_cmp);
        assert_eq!(spacings, vec![41.0, 1000.0]);
        let shared = lines
            .iter()
            .position(|l| (l.hz - 10_009_000.0).abs() < 1.0)
            .unwrap();
        assert!(grids.iter().all(|g| g.members.contains(&shared)));
    }
}
