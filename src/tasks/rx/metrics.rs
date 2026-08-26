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
//! 400-line `async move` block. The radio maths below — I/Q amplitude and phase
//! imbalance, ADC loading in dBFS, callback jitter — had no tests at all before
//! R8a.

use crate::state::IqCalState;

/// Full scale for an 8-bit signed sample, in counts.
const FULL_SCALE: f64 = 128.0;

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
pub(super) fn iq_metrics(m: Moments, cal: IqCalState) -> IqMetrics {
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
        20.0 * (m.peak_amp as f32 / FULL_SCALE as f32).log10()
    } else {
        DBFS_FLOOR
    };
    let adc_rms_dbfs = {
        let p = (var_i + var_q) / (FULL_SCALE * FULL_SCALE);
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
        dc_i: (emean_i / FULL_SCALE) as f32,
        dc_q: (emean_q / FULL_SCALE) as f32,
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

#[cfg(test)]
mod tests {
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
        let m = iq_metrics(Moments::default(), no_cal());
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
        let m = iq_metrics(balanced(1000, 400), no_cal());
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
        let db = iq_metrics(mo, no_cal()).iq_imbalance_db.unwrap();
        assert!((db - 6.0206).abs() < 0.01, "got {db}");
        // And the sign follows which arm is louder.
        let mut swapped = balanced(1000, 100);
        swapped.q_sq_sum *= 4;
        assert!((iq_metrics(swapped, no_cal()).iq_imbalance_db.unwrap() + 6.0206).abs() < 0.01);
    }

    #[test]
    fn phase_imbalance_comes_out_of_the_i_q_covariance() {
        // Equal power in both arms with covariance c gives sin(theta) = 2c/(vi+vq).
        // Here c = vi/2, so sin(theta) = 0.5 and theta = 30 degrees.
        let mut mo = balanced(1000, 200);
        mo.cross_sum = 100 * 1000;
        let deg = iq_metrics(mo, no_cal()).phase_imbalance.unwrap();
        assert!((deg - 30.0).abs() < 0.01, "got {deg}");
    }

    #[test]
    fn a_perfectly_correlated_pair_does_not_blow_past_ninety_degrees() {
        // 2c/(vi+vq) can exceed 1 on a degenerate window, and `asin` of that is
        // NaN. The clamp is what keeps a number on screen.
        let mut mo = balanced(100, 50);
        mo.cross_sum = 10_000 * 100;
        let deg = iq_metrics(mo, no_cal()).phase_imbalance.unwrap();
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
        let m = iq_metrics(mo, no_cal());
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
        let m = iq_metrics(mo, cal);
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
            iq_metrics(mo, no_cal()).adc_peak_dbfs.abs() < 1e-4,
            "a rail-hitting sample is 0 dBFS"
        );
        mo.peak_amp = 64;
        let half = iq_metrics(mo, no_cal()).adc_peak_dbfs;
        assert!(
            (half + 6.0206).abs() < 0.01,
            "half scale is -6 dBFS, got {half}"
        );
        // No sample recorded at all: the floor, never -inf.
        mo.peak_amp = 0;
        assert_eq!(iq_metrics(mo, no_cal()).adc_peak_dbfs, DBFS_FLOOR);
    }

    #[test]
    fn adc_rms_sums_the_power_of_both_arms() {
        // var_i = var_q = 128²/2 gives total power 128², i.e. 0 dBFS.
        let var = (128 * 128) / 2;
        let db = iq_metrics(balanced(1000, var), no_cal()).adc_rms_dbfs;
        assert!(db.abs() < 0.01, "got {db}");
        // A dead-silent stream floors rather than reporting -inf.
        assert_eq!(
            iq_metrics(balanced(1000, 0), no_cal()).adc_rms_dbfs,
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
}
