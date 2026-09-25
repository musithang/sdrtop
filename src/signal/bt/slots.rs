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

/// Half a slot, µs: the tick of the Bluetooth clock ("the LSB shall tick in
/// units of 312.5 μs (i.e. half a time slot)", Core 5.4 Vol 2 Part B 1.1)
/// and the pace of inquiry and paging, which hop at up to 3200 times a
/// second (2.1, both read 2026-09-25). A device inquiring or paging sends
/// its ID packets on this grid, two to a slot, where a piconet's packets
/// start on whole slots.
pub const HALF_SLOT_US: f64 = 312.5;

/// How finely a hit is dated, µs: a quarter symbol. Rounding to it alone
/// scatters a time by `QUANTUM_US / sqrt(12)`, the floor under any spread
/// the fit states, however clean the hits line up.
const QUANTUM_US: f64 = 0.25;

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
    /// Its standard error, ppm: the slope's, from the residuals' scatter
    /// over the span the hits cover.
    pub rate_sigma_ppm: f64,
    /// The residuals' root mean square, µs, with its standard error.
    pub rms_us: Uncertain,
    /// The largest residual, µs.
    pub max_us: f64,
    /// Every residual, µs, in time order.
    pub residuals_us: Vec<f32>,
    /// The fitted model, so a hit's residual can be read again
    /// ([`Self::residual_at`]): the first hit's time, the trial period, the
    /// phase it gave, and the least-squares line through the unwrapped
    /// residuals.
    pub model: Model,
}

/// The grid as fitted: see [`SlotFit::residual_at`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Model {
    pub t0_us: f64,
    pub period_us: f64,
    pub offset_us: f64,
    pub mean_x: f64,
    pub mean_y: f64,
    pub slope: f64,
}

impl SlotFit {
    /// The residual of a hit at `t_us` on the same clock, or `None` outside
    /// the span the grid was fitted over: inside it the grid is a
    /// measurement, beyond it an extrapolation (rule 2).
    pub fn residual_at(&self, t_us: f64) -> Option<f64> {
        let m = &self.model;
        let x = t_us - m.t0_us;
        if !(-1.0..=self.span_us + 1.0).contains(&x) {
            return None;
        }
        let d = x - m.offset_us;
        let raw = d - (d / m.period_us).round() * m.period_us;
        Some(raw - m.mean_y - m.slope * (x - m.mean_x))
    }
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
    let period_us = SLOT_US;
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
    let max_span = STEP_CYCLES * period_us / (step_floor * 1e-6);
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
    let span = rel.last().copied().unwrap_or(0.0).max(period_us);
    let n = rel.len() as f64;

    // The rate search: the concentration of the hits' phases on each trial
    // period, the best kept.
    let step = (STEP_CYCLES * period_us / (span * 1e-6)).clamp(step_floor, 1.0);
    let steps = (2.0 * SEARCH_PPM / step).ceil() as usize + 1;
    let phases = |ppm: f64| {
        let period = period_us * (1.0 + ppm * 1e-6);
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
    let period = period_us * (1.0 + best_ppm * 1e-6);
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
        rate_sigma_ppm: if sxx > 0.0 {
            rms.max(QUANTUM_US / 12f64.sqrt()) / sxx.sqrt() * 1e6
        } else {
            f64::INFINITY
        },
        rms_us: Uncertain::from_sigma(rms, rms / (2.0 * dof).sqrt()),
        max_us: max,
        residuals_us: residuals.iter().map(|&e| e as f32).collect(),
        model: Model {
            t0_us: t0,
            period_us: period,
            offset_us: offset,
            mean_x: mx,
            mean_y: my,
            slope,
        },
    })
}

/// Consecutive hits closer than this are one burst's: a page or inquiry
/// train of 16 frequencies lasts 16 slots, 10 ms (Vol 2 Part B 8.3.2).
const CLOSE_US: f64 = 10_000.0;

/// How near a grid line a spacing must fall to count as on it: twice the
/// 1 µs a transmitter's instantaneous timing is held to (2.2.5), plus our
/// quarter-symbol resolution.
const ON_GRID_US: f64 = 2.25;

/// Odd half-slot spacings needed before a pace is called inquiry's or
/// paging's at all, whatever the odds say about fewer.
const MIN_ODD: usize = 4;

/// How a LAP's hits are spaced, one burst at a time.
///
/// **The spacing, not a grid.** A grid fitted to minutes of hits mixes
/// every source that sent the code and every burst they sent it in: the
/// GIAC, sent by every device searching, fitted a half-slot grid at 80 µs
/// rms. The spacing between one hit and the next in the same burst is
/// clean whatever else is on the air (0.5 µs rms live, 2026-09-25), and it
/// separates the two kinds of traffic outright: a piconet's packets start
/// on whole slots (2.2.3), so its spacings are whole slots; inquiry and
/// paging send ID packets at up to 3200 a second (2.1), two to a slot, so
/// theirs include odd half slots, 312.5, 937.5 µs and so on, which no
/// piconet ever produces.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pace {
    /// Spacings between consecutive hits of one burst.
    pub close: usize,
    /// Of those, on a whole number of slots.
    pub whole: usize,
    /// Of those, on an odd number of half slots.
    pub odd_half: usize,
}

impl Pace {
    /// Whether the hits keep inquiry's and paging's pace: more odd
    /// half-slot spacings than chance gives, at the same odds the slot fit
    /// refuses a grid at ([`FALSE_GRID`]).
    ///
    /// **Odds, not a share.** A random spacing lands within [`ON_GRID_US`]
    /// of an odd half slot about once in 140; a piconet's never does. How
    /// many of a LAP's spacings are odd depends on which of a train's
    /// frequencies the view holds, so a share threshold missed the GIAC live
    /// (14 of 156, 9 %, against about one expected by chance). The tail of
    /// the count expected by chance (Poisson, `n p` small) is what decides.
    pub fn is_half_slot(&self) -> bool {
        if self.odd_half < MIN_ODD {
            return false;
        }
        let p = 2.0 * ON_GRID_US / SLOT_US;
        let lambda = self.close as f64 * p;
        // P(X >= k) = 1 - sum_{i<k} e^-l l^i / i!
        let mut term = (-lambda).exp();
        let mut below = 0.0;
        for i in 0..self.odd_half {
            below += term;
            term *= lambda / (i + 1) as f64;
        }
        1.0 - below < FALSE_GRID
    }
}

/// The spacings of `times_us` (µs on one clock), burst by burst.
pub fn pace(times_us: &[f64]) -> Pace {
    let mut t: Vec<f64> = times_us.iter().copied().filter(|x| x.is_finite()).collect();
    t.sort_by(f64::total_cmp);
    let mut p = Pace::default();
    for d in t.windows(2).map(|w| w[1] - w[0]).filter(|d| *d < CLOSE_US) {
        p.close += 1;
        let halves = (d / HALF_SLOT_US).round();
        if (d - halves * HALF_SLOT_US).abs() > ON_GRID_US {
            continue;
        }
        if halves as i64 % 2 == 0 {
            p.whole += 1;
        } else {
            p.odd_half += 1;
        }
    }
    p
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

    /// **A page train's spacings include odd half slots, a piconet's never
    /// do.** Bursts of ID packets on random half slots of a pager's clock,
    /// against a piconet's hits on whole slots; and random times, which
    /// fall on odd half slots about once in 140.
    #[test]
    fn a_page_train_is_told_from_a_piconet_by_its_spacings() {
        let mut rng = Rng::new(9);
        let mut page = Vec::new();
        for burst in 0..6 {
            let start = burst as f64 * 2e6;
            let mut k = 0.0;
            for _ in 0..20 {
                k += 1.0 + (rng.unit() * 6.0).floor();
                page.push(start + k * HALF_SLOT_US + rng.normal_pair().0 * 0.4);
            }
        }
        let p = pace(&page);
        assert!(p.is_half_slot(), "{p:?}");
        assert!(p.odd_half >= 40, "{p:?}");

        let whole = pace(&piconet(80, 12.0, 0.5, 0.3, 4));
        assert_eq!(whole.odd_half, 0, "{whole:?}");
        assert!(whole.whole > 20, "{whole:?}");
        assert!(!whole.is_half_slot());

        let random: Vec<f64> = (0..400).map(|_| rng.unit() * 2e6).collect();
        assert!(!pace(&random).is_half_slot(), "{:?}", pace(&random));

        // The GIAC as it was live: few odd spacings in many, and still far
        // past chance.
        let giac = Pace {
            close: 156,
            whole: 139,
            odd_half: 14,
        };
        assert!(giac.is_half_slot());
        let chance = Pace {
            close: 156,
            whole: 100,
            odd_half: 3,
        };
        assert!(!chance.is_half_slot());
    }

    /// **The grid its own hits imply**: sixty hits over a minute from a
    /// piconet 12 ppm slow on our clock, jittering 0.3 µs, give back the
    /// rate and the jitter.
    #[test]
    fn a_piconet_gives_back_its_rate_and_its_jitter() {
        let f = fit(&piconet(60, 12.0, 60.0, 0.3, 1)).expect("a grid");
        assert_eq!(f.hits, 60);
        assert!((f.rate_ppm - 12.0).abs() < 0.05, "{f:?}");
        // The stated uncertainty covers the error it has.
        assert!(f.rate_sigma_ppm > 0.0 && f.rate_sigma_ppm < 0.05, "{f:?}");
        assert!(
            (f.rate_ppm - 12.0).abs() < 4.0 * f.rate_sigma_ppm + 0.005,
            "{f:?}"
        );
        let rms = f.rms_us.value();
        assert!((rms - 0.3).abs() < 3.0 * f.rms_us.sigma() + 0.02, "{f:?}");
        assert!(f.max_us < 1.2, "{f:?}");
    }

    /// The rate's stated uncertainty is the one it has: over many piconets,
    /// each with its own jitter, the rate error sits within two stated
    /// sigma about as often as a normal error does, neither far more (a
    /// sigma too wide) nor far less (too narrow).
    #[test]
    fn the_rates_uncertainty_matches_its_scatter() {
        let trials = 200;
        let inside = (0..trials)
            .filter(|&k| {
                let f = fit(&piconet(40, -7.0, 10.0, 0.5, 100 + k)).expect("a grid");
                (f.rate_ppm + 7.0).abs() <= 2.0 * f.rate_sigma_ppm
            })
            .count();
        let share = inside as f64 / trials as f64;
        assert!((0.88..=0.99).contains(&share), "{share}");
    }

    /// A hit's residual read again from the model is the one the fit
    /// computed, and a time outside the fitted span has none.
    #[test]
    fn the_model_gives_back_each_hits_residual() {
        let mut t = piconet(30, -8.0, 20.0, 0.4, 5);
        t.sort_by(f64::total_cmp);
        let f = fit(&t).expect("a grid");
        for (x, r) in t.iter().zip(&f.residuals_us) {
            let again = f.residual_at(*x).unwrap();
            assert!((again - *r as f64).abs() < 1e-3, "{again} vs {r}");
        }
        assert_eq!(f.residual_at(t[0] - 1_000.0), None);
        assert_eq!(f.residual_at(t[29] + 1_000.0), None);
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
