// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The floating-point half of the poll: raw accumulator sums in, IQ and timing
//! measurements out.
//!
//! **This module is why the split exists.** Everything here is a pure function of
//! numbers the poll already drained, with no lock, no device and no clock in it,
//! which is exactly the property the hot path needs: `sqrt`, `log10` and `asin`
//! must not run while the mutex is held, because every frame the UI draws is
//! waiting on that same mutex.
//!
//! It is also the half that was untestable while it lived in the middle of a
//! 400-line `async move` block. The radio maths below - I/Q amplitude and phase
//! imbalance, ADC loading in dBFS, callback jitter - had no tests at all before
//! R8a.

use crate::state::IqCalState;

/// The floor every dBFS reading is clamped to, so a silent stream reports a
/// number rather than `-inf`.
pub(super) const DBFS_FLOOR: f32 = -120.0;

/// The raw integer moments one poll window drained from the accumulators.
///
/// Sums rather than averages on purpose: they are accumulated as integers in the
/// hot path and divided exactly once, here.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Moments {
    pub i_sum: i64,
    pub q_sum: i64,
    pub i_sq_sum: u64,
    pub q_sq_sum: u64,
    pub cross_sum: i64,
    pub samples: u64,
    /// Largest single-component magnitude seen in the window, in counts.
    pub peak_amp: u32,
}

/// What one window says about the front end.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct IqMetrics {
    /// DC offset as a fraction of full scale, **after** whatever correction is
    /// active, so it agrees with the corrected constellation.
    pub dc_i: f32,
    pub dc_q: f32,
    /// Mean I/Q in raw sample units: what DC-block subtracts, measured before
    /// correction because that is what a correction has to be built from.
    pub dc_i_raw: f32,
    pub dc_q_raw: f32,
    /// Candidate auto-cal Q-row coefficients, from the raw moments.
    pub iq_corr: (f32, f32),
    /// Residual amplitude imbalance after the active correction. `None` when Q
    /// carries no power to compare against.
    pub iq_imbalance_db: Option<f32>,
    /// Residual quadrature error in degrees, same caveat.
    pub phase_imbalance: Option<f32>,
    /// Loudest sample of the window, dBFS.
    pub adc_peak_dbfs: f32,
    /// Full-bandwidth RMS, dBFS.
    pub adc_rms_dbfs: f32,
}

impl IqMetrics {
    /// What an empty window reports: no impairment measured, and the dBFS floor
    /// rather than `-inf`. `iq_corr` is the identity Q-row.
    pub(super) fn idle() -> Self {
        Self {
            dc_i: 0.0,
            dc_q: 0.0,
            dc_i_raw: 0.0,
            dc_q_raw: 0.0,
            iq_corr: (0.0, 1.0),
            iq_imbalance_db: None,
            phase_imbalance: None,
            adc_peak_dbfs: DBFS_FLOOR,
            adc_rms_dbfs: DBFS_FLOOR,
        }
    }
}

/// Derive the window's IQ metrics.
///
/// `cal` is the correction that was live while the window was captured. It
/// splits the answer in two, deliberately:
///
/// - the **candidate** coefficients and the DC to subtract come from the raw
///   moments, because that is what `[C]` / `[D]` capture and apply;
/// - the **displayed** impairment is the residual left after the active
///   correction, so the number agrees with the scope beside it instead of
///   reporting a fault the app is already compensating.
///
/// `full_scale` is the device's own count for 0 dBFS, from
/// [`crate::hardware::SampleGeometry`]. It used to be a module constant of 128,
/// which was true of both radios sdrtop could open and of nothing else.
pub(super) fn iq_metrics(m: Moments, cal: IqCalState, full_scale: f64) -> IqMetrics {
    if m.samples == 0 {
        return IqMetrics::idle();
    }

    let n = m.samples as f64;
    let mean_i = m.i_sum as f64 / n;
    let mean_q = m.q_sum as f64 / n;
    let var_i = (m.i_sq_sum as f64 / n - mean_i * mean_i).max(0.0);
    let var_q = (m.q_sq_sum as f64 / n - mean_q * mean_q).max(0.0);
    let cov_iq = m.cross_sum as f64 / n - mean_i * mean_q;

    // ADC loading: peak from the loudest sample, RMS from the total I/Q power,
    // both referenced to full scale.
    let adc_peak_dbfs = if m.peak_amp > 0 {
        20.0 * (m.peak_amp as f32 / full_scale as f32).log10()
    } else {
        DBFS_FLOOR
    };
    let adc_rms_dbfs = {
        let p = (var_i + var_q) / (full_scale * full_scale);
        if p > 0.0 {
            (10.0 * p.log10()) as f32
        } else {
            DBFS_FLOOR
        }
    };

    let iq_corr = crate::signal::iq_correction_coeffs(var_i, var_q, cov_iq);

    let (ev_i, ev_q, ecov) = if cal.cal_applied {
        crate::signal::corrected_moments(var_i, var_q, cov_iq, cal.c_qi as f64, cal.c_qq as f64)
    } else {
        (var_i, var_q, cov_iq)
    };
    let (emean_i, emean_q) = if cal.dc_block_on || cal.cal_applied {
        (mean_i - cal.dc_i_raw as f64, mean_q - cal.dc_q_raw as f64)
    } else {
        (mean_i, mean_q)
    };

    let i_ac = ev_i.sqrt();
    let q_ac = ev_q.sqrt();
    let denom = ev_i + ev_q;
    IqMetrics {
        dc_i: (emean_i / full_scale) as f32,
        dc_q: (emean_q / full_scale) as f32,
        dc_i_raw: mean_i as f32,
        dc_q_raw: mean_q as f32,
        iq_corr,
        iq_imbalance_db: (q_ac > 0.0).then(|| (20.0 * (i_ac / q_ac).log10()) as f32),
        phase_imbalance: (denom > 0.0).then(|| {
            let sin_theta = (2.0 * ecov / denom).clamp(-1.0, 1.0);
            (sin_theta.asin() * 180.0 / std::f64::consts::PI) as f32
        }),
        adc_peak_dbfs,
        adc_rms_dbfs,
    }
}

/// Mean callback period and its standard deviation, in microseconds. `None` when
/// no callback landed in the window.
pub(super) fn callback_timing(sum_us: u64, sq_sum: u64, count: u64) -> Option<(u64, u64)> {
    if count == 0 {
        return None;
    }
    let mean = sum_us / count;
    let sq_mean = sq_sum / count;
    // Saturating: the two sums are accumulated independently, so a window that
    // straddles a reset can leave `sq_mean` below `mean²` and underflow.
    let variance = sq_mean.saturating_sub(mean.saturating_mul(mean));
    Some((mean, (variance as f64).sqrt() as u64))
}

/// What fraction of a pull backend's read loop went on work rather than waiting.
///
/// **The pull-mode answer to "is the host keeping up".** A push backend is
/// graded on whether callbacks arrive on time, because the driver paces them. A
/// pull loop sets its own rhythm, so arrival times say nothing; what says
/// everything is whether the loop ever gets to block. Below one there is
/// headroom. At one it never waits, which means it is behind and the driver's
/// buffer is filling behind it.
///
/// `None` before the first complete pass, when there is no window to divide by.
pub(super) fn occupancy(wait_us: u64, work_us: u64) -> Option<f32> {
    let total = wait_us.checked_add(work_us)?;
    if total == 0 {
        return None;
    }
    Some((work_us as f64 / total as f64) as f32)
}

/// How much history the sample-rate estimate averages over, in microseconds.
///
/// **This is a resolution requirement, not a taste.** Bytes arrive in whole
/// driver blocks, so a poll window holds a whole number of them and nothing in
/// between: measured on a HackRF through SoapyHackRF at 10 Msps, a 200 ms window
/// held anywhere from 120 to 127 blocks of 32768 bytes, and not one partial
/// block in 126 windows. One block is 8192 ppm of a 200 ms window, and the
/// observed spread of the per-window estimate was -17 000 to +40 000 ppm.
///
/// `TimingCause::Clock` fires above 500 ppm. An estimator sixteen times coarser
/// than its own threshold does not measure a clock, it measures how many blocks
/// happened to fit.
///
/// Summing consecutive windows fixes it by arithmetic rather than by smoothing:
/// the whole-block error survives only at the two ends of the baseline, so it
/// divides by the total elapsed time. One block is 8192 ppm over 200 ms, 164 ppm
/// over 10 s, and 55 ppm over 30 s. Thirty seconds leaves a comfortable margin
/// under the threshold.
const RATE_BASELINE_US: u64 = 30_000_000;

/// Below this the baseline is too short to be worth a number, and the reading is
/// `None` rather than a plausible-looking one.
///
/// Ten seconds, because that is where the block quantisation drops to 164 ppm at
/// 10 Msps, comfortably under the 500 ppm the grade fires at. A shorter floor
/// was tried at two seconds and measured on hardware: the estimate read -1379,
/// -797 and -670 ppm over its first twenty seconds, which would have traded a
/// permanent false clock fault for a temporary one.
const RATE_BASELINE_MIN_US: u64 = 10_000_000;

/// Pairs each window's bytes with the interval its blocks actually arrived in.
///
/// **This is what removes the quantisation rather than averaging it away.** A
/// window's byte count is exact: it is a whole number of driver blocks. What
/// was not exact was the time it was divided by, measured between poll instants
/// that fall wherever they fall between two block arrivals. The mismatch is up
/// to one block, and one block is 437 ppm over a 30 s baseline on a HackRF,
/// whose transfer is 262144 bytes. Measured on hardware, that made the native
/// bench swing between -159 and +499 ppm on a stream whose real offset is
/// about +130, and grade itself `GOOD` or `MARGINAL` at random.
///
/// Timing from the last block of one window to the last block of the next makes
/// the numerator and the denominator refer to the same two boundaries, and the
/// error cancels exactly. What is left is the callback's own scheduling jitter,
/// about 148 µs on a HackRF, which over 30 s is 5 ppm.
///
/// It holds an `Instant` but never reads one: the timestamp is recorded by
/// `process_block` in the hot path and handed here, so this module keeps its
/// rule of having no clock of its own.
#[derive(Debug, Default)]
pub(super) struct RateTracker {
    baseline: RateBaseline,
    prev_block_at: Option<std::time::Instant>,
}

impl RateTracker {
    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }

    /// Fold in one poll window: the arrival of its last block, and the bytes
    /// that arrived since the previous window's last block.
    ///
    /// A window with no block in it carries no timestamp and is skipped, which
    /// is also what makes the first window after a reset cost nothing: there is
    /// no earlier boundary to measure from, so there is nothing to fold.
    pub(super) fn push(&mut self, last_block_at: Option<std::time::Instant>, bytes: u64) {
        let Some(at) = last_block_at else { return };
        let Some(prev) = self.prev_block_at.replace(at) else {
            return;
        };
        self.baseline
            .push(at.duration_since(prev).as_micros() as u64, bytes);
    }

    pub(super) fn rate(&self, bytes_per_pair: usize) -> Option<u32> {
        self.baseline.rate(bytes_per_pair)
    }
}

/// Sliding long-baseline estimate of the delivered sample rate.
///
/// Task local, deliberately: it is neither drawn nor shared, so it has no
/// business being cloned into every frame with the rest of `SdrMetrics`.
///
/// No clock in here either, in keeping with the rest of this module. It is
/// handed each window's length rather than reading one.
#[derive(Debug, Default)]
pub(super) struct RateBaseline {
    /// `(window length in microseconds, bytes in that window)`, oldest first.
    windows: std::collections::VecDeque<(u64, u64)>,
    elapsed_us: u64,
    bytes: u64,
}

impl RateBaseline {
    /// Fold in one poll window, dropping whatever no longer fits the baseline.
    pub(super) fn push(&mut self, elapsed_us: u64, bytes: u64) {
        if elapsed_us == 0 {
            return;
        }
        self.windows.push_back((elapsed_us, bytes));
        self.elapsed_us += elapsed_us;
        self.bytes += bytes;
        // Keep at least one window beyond the target so the baseline never
        // shrinks below it after a trim.
        while self.windows.len() > 1 {
            let (oldest_us, oldest_bytes) = self.windows[0];
            if self.elapsed_us - oldest_us < RATE_BASELINE_US {
                break;
            }
            self.windows.pop_front();
            self.elapsed_us -= oldest_us;
            self.bytes -= oldest_bytes;
        }
    }

    /// Complex samples per second over the baseline, or `None` while it is still
    /// too short to mean anything.
    ///
    /// `bytes_per_pair` comes from the device's `SampleGeometry`. It used to be
    /// a hardcoded 2, which is right for `Int8` and `Uint8` and reports double
    /// the true rate for `Int16`.
    pub(super) fn rate(&self, bytes_per_pair: usize) -> Option<u32> {
        if self.elapsed_us < RATE_BASELINE_MIN_US || bytes_per_pair == 0 {
            return None;
        }
        let pairs = self.bytes / bytes_per_pair as u64;
        u32::try_from(pairs.checked_mul(1_000_000)? / self.elapsed_us).ok()
    }
}

#[cfg(test)]
mod tests {
    /// Both shipped radios. Named rather than repeated so a future 16-bit case
    /// reads as a different device and not as a typo.
    const EIGHT_BIT: f64 = 128.0;

    use super::*;

    /// Moments for `n` samples with the given per-component variance and no DC,
    /// as an ideal balanced front end would produce.
    fn balanced(n: u64, var: u64) -> Moments {
        Moments {
            i_sum: 0,
            q_sum: 0,
            i_sq_sum: var * n,
            q_sq_sum: var * n,
            cross_sum: 0,
            samples: n,
            peak_amp: 0,
        }
    }

    fn no_cal() -> IqCalState {
        IqCalState::default()
    }

    #[test]
    fn an_empty_window_measures_nothing_rather_than_zero() {
        // The distinction matters: 0.0 dB imbalance is a *perfect* front end, and
        // a stopped radio must not claim that.
        let m = iq_metrics(Moments::default(), no_cal(), EIGHT_BIT);
        assert_eq!(m, IqMetrics::idle());
        assert!(m.iq_imbalance_db.is_none() && m.phase_imbalance.is_none());
        assert_eq!(
            m.adc_peak_dbfs, DBFS_FLOOR,
            "a silent stream reports the floor, not -inf"
        );
        assert_eq!(
            m.iq_corr,
            (0.0, 1.0),
            "the identity Q-row leaves the samples alone"
        );
    }

    #[test]
    fn a_balanced_front_end_reports_no_impairment() {
        let m = iq_metrics(balanced(1000, 400), no_cal(), EIGHT_BIT);
        assert!(
            m.iq_imbalance_db.unwrap().abs() < 1e-4,
            "{:?}",
            m.iq_imbalance_db
        );
        assert!(
            m.phase_imbalance.unwrap().abs() < 1e-4,
            "{:?}",
            m.phase_imbalance
        );
        assert!(m.dc_i.abs() < 1e-6 && m.dc_q.abs() < 1e-6);
    }

    #[test]
    fn amplitude_imbalance_is_the_i_over_q_ratio_in_db() {
        // I carries 4x the power of Q, so twice the amplitude: +6.02 dB.
        let mut mo = balanced(1000, 100);
        mo.i_sq_sum *= 4;
        let db = iq_metrics(mo, no_cal(), EIGHT_BIT).iq_imbalance_db.unwrap();
        assert!((db - 6.0206).abs() < 0.01, "got {db}");
        // And the sign follows which arm is louder.
        let mut swapped = balanced(1000, 100);
        swapped.q_sq_sum *= 4;
        assert!(
            (iq_metrics(swapped, no_cal(), EIGHT_BIT)
                .iq_imbalance_db
                .unwrap()
                + 6.0206)
                .abs()
                < 0.01
        );
    }

    #[test]
    fn phase_imbalance_comes_out_of_the_i_q_covariance() {
        // Equal power in both arms with covariance c gives sin(theta) = 2c/(vi+vq).
        // Here c = vi/2, so sin(theta) = 0.5 and theta = 30 degrees.
        let mut mo = balanced(1000, 200);
        mo.cross_sum = 100 * 1000;
        let deg = iq_metrics(mo, no_cal(), EIGHT_BIT).phase_imbalance.unwrap();
        assert!((deg - 30.0).abs() < 0.01, "got {deg}");
    }

    #[test]
    fn a_perfectly_correlated_pair_does_not_blow_past_ninety_degrees() {
        // 2c/(vi+vq) can exceed 1 on a degenerate window, and `asin` of that is
        // NaN. The clamp is what keeps a number on screen.
        let mut mo = balanced(100, 50);
        mo.cross_sum = 10_000 * 100;
        let deg = iq_metrics(mo, no_cal(), EIGHT_BIT).phase_imbalance.unwrap();
        assert!(deg.is_finite() && (deg - 90.0).abs() < 1e-3, "got {deg}");
    }

    #[test]
    fn dc_offset_is_reported_as_a_fraction_of_full_scale() {
        let mo = Moments {
            i_sum: 64 * 1000,
            q_sum: -32 * 1000,
            samples: 1000,
            i_sq_sum: 100 * 1000,
            q_sq_sum: 100 * 1000,
            ..Moments::default()
        };
        let m = iq_metrics(mo, no_cal(), EIGHT_BIT);
        assert!(
            (m.dc_i - 0.5).abs() < 1e-6,
            "64 of 128 counts is half scale: {}",
            m.dc_i
        );
        assert!((m.dc_q + 0.25).abs() < 1e-6);
        // The raw figures stay in counts, because that is what DC-block subtracts.
        assert!((m.dc_i_raw - 64.0).abs() < 1e-4);
        assert!((m.dc_q_raw + 32.0).abs() < 1e-4);
    }

    #[test]
    fn dc_block_removes_the_offset_from_the_displayed_figure_only() {
        // With DC-block on, the panel should read the residual (zero), while the
        // raw estimate keeps tracking so the block follows slow drift.
        let mo = Moments {
            i_sum: 64 * 1000,
            q_sum: 0,
            samples: 1000,
            i_sq_sum: 100 * 1000,
            q_sq_sum: 100 * 1000,
            ..Moments::default()
        };
        let cal = IqCalState {
            dc_block_on: true,
            dc_i_raw: 64.0,
            ..IqCalState::default()
        };
        let m = iq_metrics(mo, cal, EIGHT_BIT);
        assert!(m.dc_i.abs() < 1e-6, "residual should be ~0, got {}", m.dc_i);
        assert!(
            (m.dc_i_raw - 64.0).abs() < 1e-4,
            "the raw estimate must keep tracking"
        );
    }

    #[test]
    fn adc_loading_is_referenced_to_full_scale() {
        let mut mo = balanced(1000, 0);
        mo.peak_amp = 128;
        assert!(
            iq_metrics(mo, no_cal(), EIGHT_BIT).adc_peak_dbfs.abs() < 1e-4,
            "a rail-hitting sample is 0 dBFS"
        );
        mo.peak_amp = 64;
        let half = iq_metrics(mo, no_cal(), EIGHT_BIT).adc_peak_dbfs;
        assert!(
            (half + 6.0206).abs() < 0.01,
            "half scale is -6 dBFS, got {half}"
        );
        // No sample recorded at all: the floor, never -inf.
        mo.peak_amp = 0;
        assert_eq!(
            iq_metrics(mo, no_cal(), EIGHT_BIT).adc_peak_dbfs,
            DBFS_FLOOR
        );
    }

    #[test]
    fn adc_rms_sums_the_power_of_both_arms() {
        // var_i = var_q = 128²/2 gives total power 128², i.e. 0 dBFS.
        let var = (128 * 128) / 2;
        let db = iq_metrics(balanced(1000, var), no_cal(), EIGHT_BIT).adc_rms_dbfs;
        assert!(db.abs() < 0.01, "got {db}");
        // A dead-silent stream floors rather than reporting -inf.
        assert_eq!(
            iq_metrics(balanced(1000, 0), no_cal(), EIGHT_BIT).adc_rms_dbfs,
            DBFS_FLOOR
        );
    }

    #[test]
    fn callback_timing_is_mean_and_standard_deviation() {
        // Three gaps of 1000 us: mean 1000, no spread.
        assert_eq!(callback_timing(3000, 3 * 1000 * 1000, 3), Some((1000, 0)));
        // 900 / 1100 us: mean 1000, sd 100.
        let sq = 900u64 * 900 + 1100 * 1100;
        assert_eq!(callback_timing(2000, sq, 2), Some((1000, 100)));
    }

    #[test]
    fn callback_timing_declines_an_empty_window() {
        assert_eq!(
            callback_timing(0, 0, 0),
            None,
            "no callbacks means no measurement"
        );
    }

    #[test]
    fn callback_timing_survives_sums_that_disagree() {
        // The two sums are accumulated separately, so a window straddling a reset
        // can leave the mean of squares below the square of the mean. That is a
        // subtraction away from underflowing a u64 into a vast bogus jitter.
        assert_eq!(callback_timing(1_000_000, 0, 1), Some((1_000_000, 0)));
    }
    /// The dBFS reading follows the device's own full scale. A 16-bit radio
    /// whose peak sample is at its rail is at 0 dBFS, exactly like an 8-bit one
    /// at its own. Before this was a parameter it would have read as -48 dBFS,
    /// which is a receiver that looks broken and is not.
    #[test]
    fn full_scale_is_the_devices_own_and_not_a_constant() {
        let mut mo = Moments {
            samples: 1000,
            ..Moments::default()
        };
        mo.peak_amp = 32768;
        assert!(
            iq_metrics(mo, no_cal(), 32768.0).adc_peak_dbfs.abs() < 1e-4,
            "a 16-bit rail is 0 dBFS on a 16-bit device"
        );
        assert!(
            iq_metrics(mo, no_cal(), EIGHT_BIT).adc_peak_dbfs > 40.0,
            "and the same counts on an 8-bit device are far above its rail"
        );
    }

    // ── Read-loop occupancy ─────────────────────────────────────────────────

    #[test]
    fn occupancy_is_the_share_of_the_loop_spent_working() {
        assert_eq!(occupancy(900, 100), Some(0.1), "mostly waiting is healthy");
        assert_eq!(occupancy(500, 500), Some(0.5));
        assert_eq!(
            occupancy(0, 1_000),
            Some(1.0),
            "never blocking means the loop is behind"
        );
    }

    /// Before the first pass there is no window, and dividing by one that does
    /// not exist would report a confident 0 or 100 per cent.
    #[test]
    fn no_window_yet_is_not_an_answer() {
        assert_eq!(occupancy(0, 0), None);
    }

    /// Two counters read at different instants can in principle be nonsense.
    /// Saturating rather than panicking is the right response in a poll loop.
    #[test]
    fn absurd_counters_do_not_panic() {
        assert_eq!(occupancy(u64::MAX, u64::MAX), None, "the sum overflows");
        assert_eq!(occupancy(u64::MAX, 0), Some(0.0));
    }

    // ── The sample-rate baseline ────────────────────────────────────────────
    //
    // The fixture reproduces the measurement that motivated it: a driver that
    // hands over whole 32768-byte blocks, so each 200 ms window carries a whole
    // number of them and the count wobbles by a few either way.

    const BLOCK_BYTES: u64 = 32_768;
    const WINDOW_US: u64 = 200_000;
    /// 10 Msps of CS8 is 20 MB/s, which is 4 000 000 bytes per 200 ms window,
    /// i.e. 122.07 blocks. A real window therefore holds 122 or 123, and the
    /// hardware capture ranged over 120 to 127.
    const TRUE_RATE: u64 = 10_000_000;

    /// Feed `b` with windows from a device running at `rate` samples per second
    /// that delivers **whole blocks only**.
    ///
    /// The quantisation is produced rather than copied from the capture: bytes
    /// accumulate at the true rate, a window carries away as many whole blocks
    /// as have accumulated, and the remainder carries into the next one. That
    /// is the physical situation, and it is why the per-window count wobbles
    /// while the long-run total does not.
    ///
    /// Writing this out was worth it: the first version of these tests hardcoded
    /// eight block counts read off the capture, whose mean was 123.125 against a
    /// true 122.07. The fixture itself ran 8600 ppm fast and failed the test it
    /// was written to support.
    fn feed_quantised(b: &mut RateBaseline, rate: u64, bytes_per_pair: u64, windows: usize) {
        let mut pending = 0u64;
        for _ in 0..windows {
            pending += rate * bytes_per_pair * WINDOW_US / 1_000_000;
            let delivered = (pending / BLOCK_BYTES) * BLOCK_BYTES;
            pending -= delivered;
            b.push(WINDOW_US, delivered);
        }
    }

    fn ppm_off(rate: u32) -> i64 {
        (rate as i64 - TRUE_RATE as i64) * 1_000_000 / TRUE_RATE as i64
    }

    /// A single window cannot measure a clock, and the estimate is not asked to.
    #[test]
    fn a_short_baseline_is_refused_rather_than_answered_badly() {
        let mut b = RateBaseline::default();
        b.push(WINDOW_US, 127 * BLOCK_BYTES);
        assert_eq!(b.rate(2), None, "200 ms is not a baseline");
        feed_quantised(&mut b, TRUE_RATE, 2, 20); // 4 s
        assert_eq!(b.rate(2), None, "nor is 4 s, which measured -1379 ppm");
    }

    /// The whole reason the tracker exists: with the window timed between block
    /// arrivals, a whole-block byte count divides by the interval those exact
    /// blocks spanned, and the quantisation is gone rather than averaged down.
    ///
    /// Sized on the HackRF's transfer, which is the case that exposed it: 131072
    /// pairs is 262144 bytes, eight times the SoapySDR read, and one of those
    /// over a 30 s baseline is 437 ppm against a 500 ppm threshold. Measured on
    /// hardware, the native bench swung between -159 and +499 ppm on a stream
    /// whose real offset is about +130.
    #[test]
    fn timing_between_block_arrivals_removes_the_quantisation() {
        const HACKRF_BLOCK: u64 = 262_144;
        let base = std::time::Instant::now();
        let mut t = RateTracker::default();

        // A device delivering whole HackRF transfers at exactly 10 Msps. Poll
        // windows fall wherever they fall, so each carries a varying number of
        // blocks, but every block boundary lands on the exact rate.
        let block_us = HACKRF_BLOCK * 1_000_000 / (TRUE_RATE * 2); // 13107 us
        let mut blocks_sent = 0u64;
        for window in 1..=200u64 {
            // 200 ms of polling holds 15.26 blocks, so the count alternates.
            let want = window * 200_000 / block_us;
            let n = want - blocks_sent;
            blocks_sent = want;
            let at = base + std::time::Duration::from_micros(blocks_sent * block_us);
            t.push(Some(at), n * HACKRF_BLOCK);
        }
        let ppm = ppm_off(t.rate(2).expect("plenty of baseline"));
        assert!(
            ppm.abs() < 50,
            "block-boundary timing should be all but exact: {ppm} ppm"
        );
    }

    /// A window with no block in it carries no boundary, and the first window
    /// after a reset has nothing earlier to measure from. Neither may fold.
    #[test]
    fn a_window_without_a_block_boundary_is_skipped() {
        let base = std::time::Instant::now();
        let mut t = RateTracker::default();
        t.push(None, 4_000_000);
        assert_eq!(t.rate(2), None, "no boundary, nothing to measure");
        t.push(Some(base), 4_000_000);
        assert_eq!(t.rate(2), None, "the first boundary is only a start point");

        // And a reset puts it back to needing a fresh start point.
        for i in 1..=200u64 {
            t.push(
                Some(base + std::time::Duration::from_micros(i * 200_000)),
                4_000_000,
            );
        }
        assert!(t.rate(2).is_some());
        t.reset();
        assert_eq!(
            t.rate(2),
            None,
            "a reset forgets the baseline and its origin"
        );
    }

    /// The stride, at all three widths. `Int16` is the one that was wrong: the
    /// hardcoded 2 reported double the true rate for a four-byte pair.
    #[test]
    fn the_rate_divides_by_the_geometrys_stride() {
        let mut narrow = RateBaseline::default();
        let mut wide = RateBaseline::default();
        for _ in 0..30 {
            // The same sample count at each width is twice the bytes at Int16.
            narrow.push(WINDOW_US, 2_000_000);
            wide.push(WINDOW_US, 4_000_000);
        }
        assert_eq!(
            narrow.rate(2),
            wide.rate(4),
            "the same stream of pairs reads as the same rate at either width"
        );
        assert_eq!(
            wide.rate(2),
            narrow.rate(2).map(|r| r * 2),
            "and reading a four-byte pair as two bytes doubles it, which is the bug"
        );
    }

    /// The baseline slides rather than growing forever, so a rate change is not
    /// averaged against the old rate for the rest of the session.
    #[test]
    fn the_baseline_slides_and_forgets() {
        let mut b = RateBaseline::default();
        for _ in 0..400 {
            b.push(WINDOW_US, 4_000_000);
        }
        assert!(
            b.elapsed_us <= RATE_BASELINE_US + WINDOW_US,
            "80 s of windows must not accumulate into an 80 s baseline: {}",
            b.elapsed_us
        );
    }

    /// A window with no elapsed time is not a window, and must not divide.
    #[test]
    fn a_zero_length_window_is_ignored() {
        let mut b = RateBaseline::default();
        b.push(0, 4_000_000);
        assert_eq!(b.rate(2), None);
        assert_eq!(b.elapsed_us, 0);
    }
}
