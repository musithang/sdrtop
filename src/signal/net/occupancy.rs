// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Per-cell duty cycle and power across the 2.4 GHz band: the first thing this
//! section measures, and it demodulates nothing.
//!
//! The band is divided into one-megahertz cells and each cell is asked one
//! question over and over: **was anything there in this window?** The fraction
//! of windows that answer yes is the duty cycle. Nothing here knows what a
//! subcarrier is, which is the rule for everything in `net`: the band's
//! measurements belong to no protocol.
//!
//! # The threshold is derived, and here is the derivation
//!
//! A magic constant is not allowed here, and "signal is 10 dB above the
//! noise" is the magic constant this measurement is usually built on. Three
//! steps, each with a closed form:
//!
//! 1. **The noise floor comes from the quietest tenth of what was seen.** The
//!    power of complex Gaussian noise is exponentially distributed, and the
//!    `q`-th quantile of an exponential with mean `m` is `-m ln(1-q)`. So the
//!    lower decile of the observed powers, divided by `-ln(0.9)`, estimates the
//!    noise power - **if everything measured was noise**, which it is not, and
//!    the correction for that is step 2.
//!
//! 2. **The occupancy biases that estimate by a factor with a closed form, so
//!    it is removed rather than stated.** If a fraction `a` of the plane is
//!    noise and the rest sits above it, the `q`-th quantile of what was measured
//!    is the `q/a`-th quantile of the noise, so the naive estimate is too high
//!    by `ln(1-q/a) / ln(1-q)`. At half the plane busy that is three decibels,
//!    which is not a rounding error. One refinement pass measures `a` against
//!    the first threshold and re-derives the floor with it.
//!
//! 3. **The threshold comes from the false-alarm rate.** For that same
//!    distribution, `P(power > T) = exp(-T/m)`, so `T = -m ln(p)` puts the
//!    threshold exactly at the false-alarm probability `p`. And `p` itself is
//!    derived rather than chosen: a duty cycle is displayed to a tenth of a
//!    percent, so the detector is allowed to be wrong an order of magnitude
//!    less often than the last digit shown. See [`FALSE_ALARM`].
//!
//! 4. **The estimate checks its own preconditions, and neither check may use
//!    the threshold.** The obvious test - measure the occupancy and refuse when
//!    it is too high - is circular: a plane ninety percent busy produces a floor
//!    so far out that nothing crosses the threshold at all, and the measurement
//!    reports an empty band with complete confidence. Both checks are therefore
//!    made on the shape of the sample distribution alone.
//!
//!    *Is the bottom of it exponential?* For an exponential the ratio between
//!    the tenth and the twentieth quantile is `ln(0.9)/ln(0.95)`, which is
//!    2.054. As the plane fills, the decile climbs the noise's own tail and that
//!    ratio grows: at the eighty percent limit of step 2 it is exactly
//!    `ln(0.5)/ln(0.75)`, 2.409. This looks only where the estimate comes from,
//!    and it needs no threshold to do it.
//!
//!    *Is there a distribution at all?* The ratio between the upper and the
//!    lower decile of an exponential is `ln(0.1)/ln(0.9)`, 21.9, and any signal
//!    in the plane widens it. A plane whose top decile is barely above its
//!    bottom one is a rail - a saturated front end, or a band busy everywhere at
//!    once - where nothing below is noise and the tail-shape check above passes
//!    happily.
//!
//!    Either failure makes every number here wrong, and the reading says so
//!    rather than being confidently incorrect.
//!
//! The residual bias is then known and removed rather than stated: a cell of
//! pure noise reads `FALSE_ALARM` busy, so [`duty_cycle`] subtracts it. An empty
//! band reads zero, which is a measurement, and not "no data", which would be a
//! refusal to answer a question we can answer.

use super::band;

/// One megahertz, the cell the band is measured in.
///
/// Fine enough to separate a two-megahertz Bluetooth channel from its
/// neighbours, coarse enough that a cell holds enough transform bins for its
/// power to mean something at any sample rate this section runs at.
pub const CELL_HZ: u64 = 1_000_000;

/// Whole cells between [`band::LOW_HZ`] and [`band::HIGH_HZ`].
///
/// Eighty-three, not eighty-three and a half: the top half-megahertz of the band
/// is not a whole cell and is not measured. Wi-Fi channel 14 is centred above
/// the band entirely and so has no cell at all, which is the same answer
/// `band.rs` gives about it everywhere else.
pub const CELLS: usize = ((band::HIGH_HZ - band::LOW_HZ) / CELL_HZ) as usize;

/// The quantile the noise floor is read at.
///
/// A tenth. The median would break on a band half busy and the 2.4 GHz band is
/// routinely past half exactly when somebody is looking at it; a decile, with
/// the occupancy correction above it, holds to eighty percent. It costs
/// precision - the estimate rests on a lower order statistic, so its spread is
/// `3.16 / sqrt(n)` of the floor rather than the quartile's `2.01 / sqrt(n)` -
/// and at the tens of thousands of samples a dwell produces that is under a
/// tenth of a decibel.
pub const FLOOR_QUANTILE: f64 = 0.10;

/// How busy the plane may be before the floor stops being a measurement.
///
/// Twice the quantile, which is the conditioning limit rather than a taste: at
/// this much occupancy the decile of the plane is the *median* of the noise,
/// still in its bulk. Past it the decile walks out into the noise's upper tail,
/// where a quantile is set by a handful of samples and the correction runs away.
///
/// It is not tested for directly, because it cannot be: see [`TAIL_LIMIT`].
#[cfg(test)]
pub const BUSY_LIMIT: f64 = 1.0 - 2.0 * FLOOR_QUANTILE;

/// `ln(0.9) / ln(0.95)`: the ratio between the tenth and the twentieth quantile
/// of an exponential, which is what the bottom of a noise-only plane looks like.
#[cfg(test)]
pub const TAIL_SHAPE: f64 = 2.054_079_717_745_686;

/// The same ratio at `BUSY_LIMIT`, where the plane's decile is the noise's
/// median: `ln(0.5) / ln(0.75)`.
///
/// **This is the busy check, and it is the busy check because the direct one is
/// circular.** A plane too busy to measure produces a floor so high that nothing
/// crosses the threshold, so counting what crossed it reports an empty band. The
/// shape of the lower tail is set by how much of the plane is noise and by
/// nothing else, and reading it costs one more quantile.
pub const TAIL_LIMIT: f64 = 2.409_420_839_653_209;

/// The duty cycle is displayed to a whole percent.
///
/// **It was a tenth of a percent, and that was a claim the measurement could not
/// pay for.** A duty cycle over `n` windows carries a binomial standard error of
/// `sqrt(d(1-d)/n)`, which at the eight thousand windows a dwell produces is a
/// quarter of a percent - so a tenth-of-a-percent reading would have been two
/// and a half times finer than its own uncertainty. Reaching a tenth honestly
/// needs a quarter of a million windows, which is one and a half seconds of
/// observation per cell and ten seconds a pass while surveying: too slow to
/// watch. Nothing said so until [`crate::ui::widgets::reading::Reading`] was
/// asked to print the number and refused, which is what that widget is for.
pub const DUTY_RESOLUTION: f64 = 0.01;

/// So the detector is allowed to be wrong an order of magnitude less often than
/// the last digit it is shown to.
///
/// This is the whole justification, and it is a fact about this program rather
/// than a number from a table. It buys a threshold of `-ln(1e-3)`, which is 8.4
/// dB above the noise floor.
pub const FALSE_ALARM: f64 = DUTY_RESOLUTION / 10.0;

/// `ln(0.1) / ln(0.9)`: the ratio between the upper and the lower decile of an
/// exponential, and so of noise power.
#[cfg(test)]
pub const NOISE_SPREAD: f64 = 21.854_345_326_782_87;

/// Below this spread the plane is a rail rather than a distribution.
///
/// Noise alone spreads by twenty-two, and any signal in the plane spreads it
/// further. A plane whose top decile is less than twice its bottom one is not a
/// sampling fluctuation at these counts: it is a front end on its rails, or a
/// band busy everywhere at once, and either way nothing below is noise.
pub const SPREAD_FLOOR: f64 = 2.0;

/// The noise floor a dwell was measured against, and whether to believe it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Floor {
    /// Mean noise power, in the same units as the samples it came from.
    pub power: f64,
    /// The busy threshold derived from it.
    pub threshold: f64,
    /// The plane's own upper-to-lower decile ratio.
    pub spread: f64,
    /// The plane's own tenth-to-twentieth quantile ratio, which says how much of
    /// it was noise without needing a threshold to ask.
    pub tail: f64,
    /// The fraction of the whole plane found above the threshold: what the
    /// floor was corrected for. Not what decides `trusted`, since a plane too
    /// busy to measure reads empty ([`TAIL_LIMIT`] has why).
    pub busy: f64,
    /// Whether both preconditions held.
    pub trusted: bool,
}

/// The cell a frequency falls in, or `None` outside the band.
pub fn cell_of(hz: f64) -> Option<usize> {
    if !hz.is_finite() || hz < band::LOW_HZ as f64 {
        return None;
    }
    let cell = ((hz - band::LOW_HZ as f64) / CELL_HZ as f64) as usize;
    (cell < CELLS).then_some(cell)
}

/// The centre frequency of a cell.
pub fn cell_centre_hz(cell: usize) -> u64 {
    band::LOW_HZ + cell as u64 * CELL_HZ + CELL_HZ / 2
}

/// The cells lying wholly inside an observed span.
///
/// `span_hz` is the usable bandwidth, which is the baseband filter's where the
/// radio has one and the sample rate's where it does not. **Wholly**, because a
/// cell measured over two-thirds of its width is not a measurement of that cell
/// and reporting it as one is the sort of quiet dilution nobody ever finds.
pub fn cells_observed(centre_hz: f64, span_hz: f64) -> std::ops::Range<usize> {
    if !centre_hz.is_finite() || !span_hz.is_finite() || span_hz <= 0.0 {
        return 0..0;
    }
    let low = centre_hz - span_hz / 2.0;
    let high = centre_hz + span_hz / 2.0;
    let edge = |hz: f64| (hz - band::LOW_HZ as f64) / CELL_HZ as f64;
    // Ceiling at the bottom and floor at the top: a cell counts only when both
    // of its edges are inside the span.
    let first = edge(low).ceil().clamp(0.0, CELLS as f64) as usize;
    let last = edge(high).floor().clamp(0.0, CELLS as f64) as usize;
    first..last.max(first)
}

/// The cell each transform bin belongs to, for one tuning.
///
/// `None` where the bin falls outside the cells [`cells_observed`] admits, which
/// includes every bin out at the edges of the sample rate that the baseband
/// filter has already rolled off. Built once per dwell rather than per window.
///
/// Bin `k` of an `n`-point transform sits at `k * rate / n` for the first half
/// and `(k - n) * rate / n` for the second, which is the ordering `rustfft`
/// produces and the reason this is a lookup table rather than a formula at the
/// point of use.
pub fn bin_cells(centre_hz: f64, rate_hz: f64, span_hz: f64, n: usize) -> Vec<Option<usize>> {
    let observed = cells_observed(centre_hz, span_hz);
    (0..n)
        .map(|k| {
            // **The bins at DC are dropped**, for the reason
            // `signal::fft::carrier` drops them: both front ends park their own
            // DC offset and LO leakage on the centre bin, and on a quiet band
            // that artefact is the strongest thing in the spectrum - measured
            // live at 46 dB above the floor with nothing on air. Left in, it
            // reads as a fully busy megahertz at every hop centre, so the survey
            // paints one bright stripe per position at exactly the frequencies
            // it is tuned to.
            if k.min(n - k) <= DC_GUARD_BINS {
                return None;
            }
            let offset = if k * 2 < n {
                k as f64
            } else {
                k as f64 - n as f64
            };
            let hz = centre_hz + offset * rate_hz / n as f64;
            cell_of(hz).filter(|c| observed.contains(c))
        })
        .collect()
}

/// Bins either side of centre treated as the DC artefact rather than as signal.
///
/// The same two `signal::fft::carrier` uses, and for the same reason: a single
/// spectral line, which the Hann window spreads over about three bins.
pub const DC_GUARD_BINS: usize = 2;

/// The cells a hop centred here cannot measure, because its own DC sits in them.
///
/// **A hop plan has to arrange that no cell is in this set on every pass**, or
/// the survey carries a permanent blind stripe it never mentions. See
/// `survey::Plan`, which walks the positions one cell along on alternate passes
/// for exactly this reason.
pub fn dc_shadow(centre_hz: f64, rate_hz: f64, n: usize) -> std::ops::Range<usize> {
    let guard = DC_GUARD_BINS as f64 * rate_hz / n as f64;
    match (cell_of(centre_hz - guard), cell_of(centre_hz + guard)) {
        (Some(a), Some(b)) => a..b + 1,
        (Some(a), None) => a..a + 1,
        (None, Some(b)) => b..b + 1,
        (None, None) => 0..0,
    }
}

/// The noise floor and threshold for a plane of window-by-cell powers.
///
/// The plane is reordered in place; the caller owns a scratch buffer rather than
/// this allocating one per dwell. `None` when there is nothing to measure, or
/// when the quarter-quantile is zero and no ratio can be formed from it.
pub fn derive_floor(plane: &mut [f64]) -> Option<Floor> {
    let lo = quantile(plane, FLOOR_QUANTILE)?;
    let hi = quantile(plane, 1.0 - FLOOR_QUANTILE)?;
    let half = quantile(plane, FLOOR_QUANTILE / 2.0)?;
    if lo <= 0.0 || half <= 0.0 || !lo.is_finite() || !hi.is_finite() {
        return None;
    }
    let spread = hi / lo;
    let tail = lo / half;

    // First pass: the inversion that would be right if the whole plane were
    // noise. The quantile of an exponential with mean m is -m ln(1-q), so the
    // mean is the quantile over -ln(1-q). Nothing here is fitted or tuned.
    let naive = lo / -(1.0 - FLOOR_QUANTILE).ln();
    let busy = above(plane, -naive * FALSE_ALARM.ln());

    // Second pass: correct for the part of the plane that was not noise. With a
    // noise fraction `a`, the plane's q-th quantile is the noise's (q/a)-th, so
    // the honest inversion divides by -ln(1 - q/a) instead. At `a` of one this
    // is exactly the first pass, which is why there is one expression and not
    // two branches.
    let noise_fraction = 1.0 - busy;
    let ratio = FLOOR_QUANTILE / noise_fraction;
    let power = if ratio < 1.0 {
        lo / -(1.0 - ratio).ln()
    } else {
        naive
    };

    Some(Floor {
        power,
        threshold: -power * FALSE_ALARM.ln(),
        spread,
        tail,
        busy,
        trusted: spread >= SPREAD_FLOOR && tail <= TAIL_LIMIT,
    })
}

/// The fraction of a plane above a threshold.
fn above(plane: &[f64], threshold: f64) -> f64 {
    if plane.is_empty() {
        return 0.0;
    }
    plane.iter().filter(|p| **p > threshold).count() as f64 / plane.len() as f64
}

/// A duty cycle with the uncertainty its window count supports.
///
/// **This is what sampling actually costs, and it is not the magnitude.** A
/// channel busy all the time, watched a sixth of the time, is busy all the time;
/// scaling its reading down to a sixth would be a wrong number, not a sampled
/// one. What a shorter look costs is certainty, and for a fraction of `n`
/// independent windows that is the binomial standard error, `sqrt(d(1-d)/n)`.
///
/// It is the estimator's own spread and not the whole story - a burst pattern
/// correlated with the hop rhythm would beat it, and that survey and lock are
/// different claims is the part no error bar can carry. Which is why the mode
/// is a tag on the panel as well as a sigma on the number.
pub fn duty_uncertain(duty: f64, windows: u64) -> crate::signal::dsp::uncertainty::Uncertain {
    use crate::signal::dsp::uncertainty::Uncertain;
    if windows == 0 {
        return Uncertain::from_sigma(duty, f64::INFINITY);
    }
    let d = duty.clamp(0.0, 1.0);
    Uncertain::from_variance(duty, d * (1.0 - d) / windows as f64)
}

/// The `q`-th value of the plane, by selection rather than by a full sort.
///
/// Reorders in place, which is why the caller owns the buffer: a dwell is tens
/// of thousands of samples and sorting one per block would be the most expensive
/// thing this feature does.
fn quantile(plane: &mut [f64], q: f64) -> Option<f64> {
    if plane.is_empty() || !q.is_finite() {
        return None;
    }
    let k = ((plane.len() as f64 - 1.0) * q.clamp(0.0, 1.0)).round() as usize;
    let (_, at, _) = plane.select_nth_unstable_by(k, |a, b| a.total_cmp(b));
    Some(*at)
}

/// The busy fraction, with the false-alarm floor removed.
pub fn duty_cycle(busy: u64, windows: u64) -> f64 {
    if windows == 0 {
        return 0.0;
    }
    let raw = busy as f64 / windows as f64;
    // The false-alarm floor is known exactly, so it is removed rather than
    // stated: a cell of pure noise reads busy `FALSE_ALARM` of the time, and a
    // panel that showed 0.01 % on an empty band for ever would teach its reader
    // to ignore the column.
    ((raw - FALSE_ALARM) / (1.0 - FALSE_ALARM)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::dsp::testkit::Rng;

    /// Exponentially distributed power samples: what noise looks like to this
    /// measurement, drawn from the inverse CDF rather than by squaring
    /// Gaussians, so the test knows the distribution exactly rather than
    /// through the same arithmetic the code under test uses.
    fn noise(rng: &mut Rng, n: usize, mean: f64) -> Vec<f64> {
        (0..n).map(|_| -mean * rng.unit().ln()).collect()
    }

    /// Every constant above is a closed form typed out by hand, which is the
    /// one thing rule 7 says not to trust. Here is the arithmetic.
    #[test]
    fn the_constants_are_the_closed_forms() {
        let q = FLOOR_QUANTILE;
        // The upper-to-lower decile ratio of an exponential.
        assert!((NOISE_SPREAD - q.ln() / (1.0 - q).ln()).abs() < 1e-12);
        // The tenth-to-twentieth quantile ratio of the same.
        assert!((TAIL_SHAPE - (1.0 - q).ln() / (1.0 - q / 2.0).ln()).abs() < 1e-12);
        // And that ratio at the busy limit, where the plane's decile is the
        // noise's median.
        let noise = 1.0 - BUSY_LIMIT;
        assert!((TAIL_LIMIT - (1.0 - q / noise).ln() / (1.0 - q / 2.0 / noise).ln()).abs() < 1e-12);
        assert!((TAIL_LIMIT - 0.5f64.ln() / 0.75f64.ln()).abs() < 1e-12);
        // The threshold the false-alarm rate buys, in decibels above the floor.
        assert!(
            (-10.0 * FALSE_ALARM.log10() * std::f64::consts::LN_10 / 10.0 - 6.908).abs() < 0.01
        );
    }

    #[test]
    fn the_band_divides_into_whole_megahertz_cells() {
        assert_eq!(CELLS, 83);
        assert_eq!(cell_centre_hz(0), band::LOW_HZ + 500_000);
        assert_eq!(cell_of(band::LOW_HZ as f64), Some(0));
        assert_eq!(cell_of(band::LOW_HZ as f64 + 999_999.0), Some(0));
        assert_eq!(cell_of(band::LOW_HZ as f64 + 1_000_000.0), Some(1));
        assert_eq!(cell_of(2_437_000_000.0), Some(37));
        // Outside the band, and outside the whole cells inside it.
        assert_eq!(cell_of(band::LOW_HZ as f64 - 1.0), None);
        assert_eq!(cell_of(2_483_200_000.0), None, "the ragged half megahertz");
        assert_eq!(cell_of(2_484_000_000.0), None, "channel 14 has no cell");
    }

    /// A span covers a cell or it does not; there is no partial credit.
    #[test]
    fn only_the_cells_wholly_inside_the_span_are_observed() {
        // 20 MHz centred on channel 6: 2427 to 2447 exactly.
        let seen = cells_observed(2_437_000_000.0, 20_000_000.0);
        assert_eq!(seen, 27..47, "twenty whole cells");
        assert_eq!(cell_centre_hz(seen.start), 2_427_500_000);

        // Shifted by half a cell, the two ragged ends drop out.
        let seen = cells_observed(2_437_500_000.0, 20_000_000.0);
        assert_eq!(seen, 28..47, "nineteen, and neither ragged end");

        // A span reaching past the band is clipped to it.
        let wide = cells_observed(2_440_000_000.0, 200_000_000.0);
        assert_eq!(wide, 0..CELLS);
    }

    /// The floor is the closed form and not a guess, so a known noise power
    /// comes back.
    /// A plane of `cells` by `windows` powers, all noise of unit mean, with
    /// cell zero carrying bursts 20 dB up for `duty` of its windows.
    ///
    /// The shape matters: the floor is derived from the whole span-time plane,
    /// which is what lets one busy cell read busy instead of setting its own
    /// floor and reading empty.
    fn plane(rng: &mut Rng, cells: usize, windows: usize, duty: f64) -> Vec<Vec<f64>> {
        let burst = (windows as f64 * duty).round() as usize;
        (0..cells)
            .map(|c| {
                let mut col = noise(rng, windows, 1.0);
                if c == 0 {
                    for p in col.iter_mut().take(burst) {
                        *p += 100.0;
                    }
                }
                col
            })
            .collect()
    }

    fn flatten(p: &[Vec<f64>]) -> Vec<f64> {
        p.iter().flatten().copied().collect()
    }

    /// What `scan::Scan` does per block, in one line: count what crossed the
    /// plane's threshold and turn the count into a duty cycle.
    ///
    /// The floor is the *receiver's*, taken across the whole observed span,
    /// which is what lets a single saturated cell read as fully busy. A floor
    /// derived from that cell alone would be the signal's own power and the cell
    /// would read empty.
    fn duty_of(powers: &[f64], floor: &Floor) -> f64 {
        let busy = powers.iter().filter(|p| **p > floor.threshold).count() as u64;
        duty_cycle(busy, powers.len() as u64)
    }

    /// The floor is the closed form and not a guess, so a known noise power
    /// comes back and the threshold sits exactly where the false-alarm rate puts
    /// it.
    #[test]
    fn the_noise_floor_is_recovered_from_the_lower_decile() {
        let mut rng = Rng::new(0x0CC0_1234);
        let mut flat = noise(&mut rng, 40_000, 4.0);
        let f = derive_floor(&mut flat).unwrap();
        // The bound is the estimator's own: a decile-based estimate of an
        // exponential mean has a relative spread of 3.16/sqrt(n), so three
        // sigma at forty thousand samples is 4.7 per mille. A tighter assertion
        // than the measurement supports fails on a seed, not on a bug.
        let three_sigma = 4.0 * 3.0 * 3.164 / (40_000f64).sqrt();
        assert!(
            (f.power - 4.0).abs() < three_sigma,
            "estimated {} from a mean of 4, bound {three_sigma}",
            f.power
        );
        assert!((f.threshold / f.power + FALSE_ALARM.ln()).abs() < 1e-12);
        assert!(
            (f.threshold / f.power - 6.908).abs() < 0.01,
            "8.4 dB above the floor: {}",
            f.threshold / f.power
        );
        assert!(f.trusted);
        assert!(
            (f.tail - TAIL_SHAPE).abs() < 0.1,
            "noise-only tail is 2.054: {}",
            f.tail
        );
        assert!(
            (f.spread - NOISE_SPREAD).abs() < 1.0,
            "noise spreads by 21.9: {}",
            f.spread
        );
    }

    /// The occupancy bias is removed and not merely acknowledged.
    ///
    /// Without the second pass, a plane half busy reads its floor three decibels
    /// high, because the decile of what was measured is no longer the decile of
    /// the noise. The correction has a closed form, so it is applied.
    #[test]
    fn a_busy_plane_does_not_report_a_floor_that_is_not_there() {
        for busy in [0.0, 0.25, 0.5, 0.75] {
            let mut rng = Rng::new(0xB005 + (busy * 100.0) as u64);
            // Every cell equally busy, which is the worst case: there is no
            // quiet cell anywhere for the floor to rest on.
            let mut flat = noise(&mut rng, 60_000, 1.0);
            let n = (60_000.0 * busy) as usize;
            for p in flat.iter_mut().take(n) {
                *p += 100.0;
            }
            let f = derive_floor(&mut flat).unwrap();
            let error_db = 10.0 * f.power.log10();
            assert!(
                error_db.abs() < 0.4,
                "{busy} busy: floor off by {error_db:.2} dB (power {})",
                f.power
            );
            assert!(
                f.trusted,
                "{busy} busy is inside the limit, tail {}",
                f.tail
            );
        }
    }

    /// Past the limit, and on a rail, the reading refuses rather than lies.
    #[test]
    fn a_plane_that_is_not_noise_plus_signal_says_so() {
        // A rail: nothing to take a distribution from.
        let mut flat = vec![1.0f64; 10_000];
        let f = derive_floor(&mut flat).unwrap();
        assert!(!f.trusted, "spread {}", f.spread);
        assert!(f.spread < SPREAD_FLOOR);

        // Ninety percent busy: the decile of the plane is out in the noise's
        // upper tail, where the correction is set by a handful of samples.
        let mut rng = Rng::new(99);
        let mut flat = noise(&mut rng, 40_000, 1.0);
        for p in flat.iter_mut().take(36_000) {
            *p += 100.0;
        }
        let f = derive_floor(&mut flat).unwrap();
        assert!(!f.trusted, "tail {}", f.tail);
        // At this much occupancy the decile and the nine-decile are both in the
        // signal, so the spread has collapsed too. Both checks fire.
        assert!(f.tail > TAIL_LIMIT);
        assert!(f.spread < SPREAD_FLOOR);
    }

    /// The case that separates the two checks, and the reason the busy one is
    /// not the direct one.
    ///
    /// At eighty-eight percent the plane still spreads - its top decile is a
    /// hundred times its bottom one - so the rail check passes it. The floor is
    /// nonsense all the same, and it is nonsense in the direction that hides
    /// itself: the threshold lands above the signal, so counting what crossed it
    /// reports a **completely empty band** with total confidence. Only the shape
    /// of the lower tail knows, because only it never touches the threshold.
    #[test]
    fn the_busy_check_cannot_be_the_obvious_one() {
        let mut rng = Rng::new(0x88);
        let mut flat = noise(&mut rng, 40_000, 1.0);
        for p in flat.iter_mut().take(35_200) {
            *p += 100.0;
        }
        let f = derive_floor(&mut flat).unwrap();
        assert!(
            f.spread >= SPREAD_FLOOR,
            "the plane still spreads: {}",
            f.spread
        );
        assert!(
            f.busy <= BUSY_LIMIT,
            "and the direct check sees {} busy on a band 88 % occupied",
            f.busy
        );
        assert!(f.tail > TAIL_LIMIT, "tail {}", f.tail);
        assert!(!f.trusted);
    }

    /// Bins land in the cell their frequency is in, and nowhere else.
    #[test]
    fn every_transform_bin_finds_its_cell() {
        // 20 MHz on channel 6, 16 bins of 1.25 MHz, usable span 18 MHz.
        let cells = bin_cells(2_437_000_000.0, 20_000_000.0, 18_000_000.0, 16);
        assert_eq!(cells.len(), 16);
        // Bin 0 is DC, which is the tuned frequency: 2437 MHz, cell 37.
        assert_eq!(cells[4], Some(42), "5 MHz up");
        // Bin 3 is 3.75 MHz up, clear of the DC guard below.
        assert_eq!(cells[3], Some(40));
        // Bin 13 is 3.75 MHz *down*, not 16.25 MHz up.
        assert_eq!(cells[13], Some(33));
        // 8.75 MHz up is 2445.75, whose whole cell is inside the 18 MHz span.
        assert_eq!(cells[7], Some(45));
        // The Nyquist bin is 10 MHz *down*, past the span, and is dropped rather
        // than folded into an edge cell.
        assert_eq!(cells[8], None);

        // The usable span is the baseband filter's, not the sample rate's, and
        // narrowing it drops bins the rate alone would have admitted. Nothing
        // else in the pipeline knows the front end rolled them off.
        let narrow = bin_cells(2_437_000_000.0, 20_000_000.0, 14_000_000.0, 16);
        assert_eq!(narrow[7], None, "2445.75 is outside a 14 MHz span");
        assert_eq!(narrow[3], Some(40), "and the middle is untouched");

        // The DC bins are dropped whatever they would have mapped to: the
        // front end's own leakage sits there and reads as a full megahertz.
        assert_eq!(cells[0], None, "DC");
        assert_eq!(cells[1], None, "one bin up is still the artefact");
        assert_eq!(cells[15], None, "and one bin down");
        assert_eq!(cells[2], None, "two bins out is still the artefact");
        assert_eq!(cells[3], Some(40), "three bins out is signal again");

        // A tuning outside the band puts every bin nowhere.
        let none = bin_cells(100_000_000.0, 20_000_000.0, 18_000_000.0, 16);
        assert!(none.iter().all(Option::is_none));
    }

    /// The one that matters: a known duty cycle measures back to itself.
    #[test]
    fn a_known_duty_cycle_is_measured_back() {
        for want in [0.0, 0.05, 0.25, 0.5, 1.0] {
            let mut rng = Rng::new(0xD00D + (want * 1000.0) as u64);
            let p = plane(&mut rng, 20, 4_000, want);
            let f = derive_floor(&mut flatten(&p)).unwrap();
            assert!(f.trusted, "one busy cell in twenty is not a busy plane");
            let got = duty_of(&p[0], &f);
            assert!(
                (got - want).abs() < 0.005,
                "wanted {want}, measured {got} (threshold {})",
                f.threshold
            );
            // And a cell nobody transmitted in reads empty, not "no data".
            //
            // Empty to the resolution it is shown at, rather than exactly zero:
            // the correction removes the *expected* false-alarm floor and what
            // is left is the Poisson noise on it, four counts in four thousand
            // windows. That residual is an order of magnitude under
            // `DUTY_RESOLUTION`, which is the whole reason `FALSE_ALARM` is a
            // tenth of it.
            let quiet = duty_of(&p[1], &f);
            assert!(quiet < DUTY_RESOLUTION, "a quiet cell read {quiet}");
        }
    }

    /// Sampling costs certainty, not magnitude.
    #[test]
    fn a_shorter_look_widens_the_reading_rather_than_shrinking_it() {
        let long = duty_uncertain(0.5, 40_000);
        let short = duty_uncertain(0.5, 400);
        assert_eq!(long.value(), short.value(), "the value is the value");
        assert!(
            short.sigma() > long.sigma() * 9.0,
            "ten times fewer windows"
        );
        // The closed form, so the bound is arithmetic rather than a feeling.
        assert!((long.sigma() - (0.25f64 / 40_000.0).sqrt()).abs() < 1e-12);

        // A saturated channel has no spread at all: every window said yes and
        // there is nothing left for the sampling to have got wrong.
        assert_eq!(duty_uncertain(1.0, 400).sigma(), 0.0);
        assert_eq!(duty_uncertain(0.0, 400).sigma(), 0.0);
        // And a cell nobody measured has an uncertainty nobody can write down,
        // which `Reading` already knows how to refuse.
        assert!(!duty_uncertain(0.0, 0).sigma().is_finite());
    }

    /// An empty band reads zero, and a full one reads one, and neither runs off
    /// the end of the scale.
    #[test]
    fn an_empty_band_reads_zero_and_a_full_one_reads_one() {
        assert_eq!(duty_cycle(0, 10_000), 0.0);
        assert_eq!(duty_cycle(10_000, 10_000), 1.0);
        // The false-alarm floor is removed, so noise alone reads zero rather
        // than reading one hundredth of a percent busy for ever.
        let expected_false = (10_000.0 * FALSE_ALARM).round() as u64;
        assert_eq!(duty_cycle(expected_false, 10_000), 0.0);
        // And one window over that floor is not negative.
        assert!(duty_cycle(expected_false + 1, 10_000) > 0.0);
        // Nothing to measure is nothing, not a division by zero.
        assert_eq!(duty_cycle(0, 0), 0.0);
        assert_eq!(duty_cycle(5, 0), 0.0);
        // A cell more busy than its windows cannot happen, and does not
        // overflow the scale if it does.
        assert_eq!(duty_cycle(20_000, 10_000), 1.0);
    }
}
