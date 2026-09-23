// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Slot timing, measured against itself (Bluetooth design measurement 12,
//! net-ux-polish-plan 6.5): how far each access code a piconet sent lands
//! from its own fitted 625 µs grid.
//!
//! **Read from the Core Specification 5.4, Vol 2, Part B** on the SIG's own
//! site this session: the physical channel "is divided into time slots,
//! each 625 μs in length" and "the packet start shall be aligned with the
//! slot start" (2.2.3); "the average timing of packet transmission shall
//! not drift faster than 20 ppm relative to the ideal slot timing of
//! 625 μs. The instantaneous timing shall not deviate more than 1 μs from
//! the average timing" (2.2.5). The last sentence is the limit the jitter
//! is shown against.
//!
//! **Against itself, not against us.** Our sample clock and theirs differ
//! by tens of ppm, which over a minute is thousands of microseconds, so the
//! grid is fitted: its period (a rate within [`SEARCH_PPM`] of 625 µs on our
//! clock) and its phase. What is left over is the piconet's own jitter,
//! plus our own time resolution (a quarter symbol, 0.25 µs).
//!
//! **A grid is found, or refused, not assumed.** Hits a few a minute apart
//! can line up with *some* period by chance when enough periods are tried,
//! so the best alignment is tested (Rayleigh's test on the hits' phases,
//! with the number of periods tried counted against it) and a grid that
//! could be chance is refused as one (rule 2).

use crate::signal::dsp::uncertainty::Uncertain;

/// The nominal slot, µs (2.2.3).
pub const SLOT_US: f64 = 625.0;

/// Hits needed before a fit is tried: two numbers are fitted and a spread
/// is stated, and fewer than this cannot pass the chance test anyway.
pub const MIN_HITS: usize = 8;

/// Arrivals kept per piconet: enough for a spread to be read, bounded so the
/// fit stays cheap.
pub const KEPT: usize = 256;

/// How far the grid's rate is searched either side of 625 µs on our clock:
/// the specification allows each side ±20 ppm (2.2.5), our own oscillator
/// is a comparable part, and the rest is margin.
pub const SEARCH_PPM: f64 = 60.0;

/// The most rates tried; a longer span than this resolves is fitted on its
/// most recent hits.
const MAX_STEPS: usize = 4_000;

/// Phase error allowed between rate steps, as a fraction of a slot over the
/// span: fine enough that the best step lands within reach of the linear
/// refinement.
const STEP_CYCLES: f64 = 0.05;

/// The chance a grid this good would appear among random times, with the
/// rates tried counted, above which it is refused.
const FALSE_GRID: f64 = 1e-3;

/// A fitted grid and what is left over.
#[derive(Clone, Debug, PartialEq)]
pub struct SlotFit {
    /// Hits the grid was fitted to, and the span they cover, µs.
    pub hits: usize,
    pub span_us: f64,
    /// Their slot period against our sample clock, ppm from 625 µs:
    /// positive when their slots are longer on our clock.
    pub rate_ppm: f64,
    /// The residuals' root mean square, µs, with its standard error.
    pub rms_us: Uncertain,
    /// The largest residual, µs.
    pub max_us: f64,
    /// Every residual, µs, in time order.
    pub residuals_us: Vec<f32>,
}

/// Why there is no fit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotRefusal {
    /// Fewer than [`MIN_HITS`].
    Collecting { have: usize, need: usize },
    /// Enough hits, and no grid they line up on beyond chance.
    NoGrid { hits: usize },
}

/// Fit a slot grid to access-code times, µs on one continuous clock.
pub fn fit(times_us: &[f64]) -> Result<SlotFit, SlotRefusal> {
    let mut t: Vec<f64> = times_us.iter().copied().filter(|x| x.is_finite()).collect();
    t.sort_by(f64::total_cmp);
    if t.len() < MIN_HITS {
        return Err(SlotRefusal::Collecting {
            have: t.len(),
            need: MIN_HITS,
        });
    }
    // The longest span the step budget resolves; older hits are left out.
    let step_floor = 2.0 * SEARCH_PPM / MAX_STEPS as f64;
    let max_span = STEP_CYCLES * SLOT_US / (step_floor * 1e-6);
    let last = *t.last().expect("non-empty");
    t.retain(|&x| last - x <= max_span);
    if t.len() < MIN_HITS {
        return Err(SlotRefusal::Collecting {
            have: t.len(),
            need: MIN_HITS,
        });
    }
    let t0 = t[0];
    let rel: Vec<f64> = t.iter().map(|x| x - t0).collect();
    let span = rel.last().copied().unwrap_or(0.0).max(SLOT_US);
    let n = rel.len() as f64;

    // The rate search: the concentration of the hits' phases on each trial
    // period, the best kept.
    let step = (STEP_CYCLES * SLOT_US / (span * 1e-6)).clamp(step_floor, 1.0);
    let steps = (2.0 * SEARCH_PPM / step).ceil() as usize + 1;
    let phases = |ppm: f64| {
        let period = SLOT_US * (1.0 + ppm * 1e-6);
        let (mut c, mut s) = (0.0, 0.0);
        for &x in &rel {
            let a = std::f64::consts::TAU * (x / period);
            c += a.cos();
            s += a.sin();
        }
        (c, s)
    };
    let (mut best_ppm, mut best): (f64, (f64, f64)) = (0.0, (0.0, 0.0));
    for k in 0..steps {
        let ppm = -SEARCH_PPM + k as f64 * step;
        let cs = phases(ppm);
        if cs.0.hypot(cs.1) > best.0.hypot(best.1) {
            (best_ppm, best) = (ppm, cs);
        }
    }

    // Rayleigh: the chance of this concentration among random phases is
    // about exp(-n R^2); the trials are the independent rates tried, one per
    // slot of phase drift across the span.
    let r = best.0.hypot(best.1) / n;
    let trials = steps as f64 * STEP_CYCLES + 1.0;
    if trials * (-n * r * r).exp() > FALSE_GRID {
        return Err(SlotRefusal::NoGrid { hits: rel.len() });
    }

    // Unwrap each hit onto its slot, then refine rate and phase together by
    // least squares on the residuals.
    let period = SLOT_US * (1.0 + best_ppm * 1e-6);
    let offset = best.1.atan2(best.0) / std::f64::consts::TAU * period;
    let raw: Vec<f64> = rel
        .iter()
        .map(|&x| {
            let d = x - offset;
            d - (d / period).round() * period
        })
        .collect();
    let mx = rel.iter().sum::<f64>() / n;
    let my = raw.iter().sum::<f64>() / n;
    let sxx: f64 = rel.iter().map(|x| (x - mx).powi(2)).sum();
    let sxy: f64 = rel.iter().zip(&raw).map(|(x, y)| (x - mx) * (y - my)).sum();
    let slope = if sxx > 0.0 { sxy / sxx } else { 0.0 };
    let residuals: Vec<f64> = rel
        .iter()
        .zip(&raw)
        .map(|(x, y)| y - my - slope * (x - mx))
        .collect();
    let dof = n - 2.0;
    let rms = (residuals.iter().map(|e| e * e).sum::<f64>() / dof).sqrt();
    let max = residuals.iter().fold(0.0f64, |m, e| m.max(e.abs()));
    Ok(SlotFit {
        hits: rel.len(),
        span_us: span,
        // A residual growing by `slope` µs per µs is a period that long.
        rate_ppm: best_ppm + slope * 1e6,
        rms_us: Uncertain::from_sigma(rms, rms / (2.0 * dof).sqrt()),
        max_us: max,
        residuals_us: residuals.iter().map(|&e| e as f32).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::dsp::testkit::Rng;

    /// `n` access codes on random slots of a piconet whose slots run `ppm`
    /// long on our clock, over `span_s`, each `jitter_us` (Gaussian) off.
    fn piconet(n: usize, ppm: f64, span_s: f64, jitter_us: f64, seed: u64) -> Vec<f64> {
        let mut rng = Rng::new(seed);
        let slots = span_s * 1e6 / SLOT_US;
        let mut picks: Vec<f64> = (0..n).map(|_| (rng.unit() * slots).floor()).collect();
        picks.sort_by(f64::total_cmp);
        picks
            .iter()
            .map(|k| 1_234.5 + k * SLOT_US * (1.0 + ppm * 1e-6) + rng.normal_pair().0 * jitter_us)
            .collect()
    }

    /// **The grid its own hits imply**: sixty hits over a minute from a
    /// piconet 12 ppm slow on our clock, jittering 0.3 µs, give back the
    /// rate and the jitter.
    #[test]
    fn a_piconet_gives_back_its_rate_and_its_jitter() {
        let f = fit(&piconet(60, 12.0, 60.0, 0.3, 1)).expect("a grid");
        assert_eq!(f.hits, 60);
        assert!((f.rate_ppm - 12.0).abs() < 0.05, "{f:?}");
        let rms = f.rms_us.value();
        assert!((rms - 0.3).abs() < 3.0 * f.rms_us.sigma() + 0.02, "{f:?}");
        assert!(f.max_us < 1.2, "{f:?}");
    }

    /// Sparse and long, as the air gave it: twenty hits over five minutes
    /// still find their grid.
    #[test]
    fn a_few_hits_a_minute_still_find_the_grid() {
        let f = fit(&piconet(20, -25.0, 300.0, 0.5, 2)).expect("a grid");
        assert!((f.rate_ppm + 25.0).abs() < 0.05, "{f:?}");
        assert!(f.rms_us.value() < 1.0, "{f:?}");
    }

    /// **Chance is refused.** Times on no grid at all give no fit, whatever
    /// rate would line a few of them up best.
    #[test]
    fn times_on_no_grid_are_refused_as_one() {
        let mut rng = Rng::new(3);
        let t: Vec<f64> = (0..40).map(|_| rng.unit() * 60e6).collect();
        assert_eq!(fit(&t), Err(SlotRefusal::NoGrid { hits: 40 }));
    }

    #[test]
    fn too_few_hits_are_collected_not_fitted() {
        assert_eq!(
            fit(&piconet(5, 0.0, 1.0, 0.1, 4)),
            Err(SlotRefusal::Collecting { have: 5, need: 8 })
        );
    }
}
