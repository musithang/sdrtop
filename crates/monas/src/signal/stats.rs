// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! These statistics operate on finite spectrum measurements in a common dB unit.
//! Non-finite inputs represent unavailable measurements.
//! `NEG_INFINITY` represents an unavailable output.
//! A finite floor such as the FFT's `DB_FLOOR` remains a measurement.

/// Compute an EMA from the available values.
///
/// A finite sample seeds unavailable history regardless of `alpha`.
/// An unavailable sample preserves finite history.
/// Two unavailable values produce `NEG_INFINITY`.
/// The caller must supply a finite `alpha` in `0..=1`.
/// With two finite values, zero retains history and one selects the sample.
pub(crate) fn finite_ema(previous: f32, sample: f32, alpha: f32) -> f32 {
    match (previous.is_finite(), sample.is_finite()) {
        (true, true) => alpha * sample + (1.0 - alpha) * previous,
        (false, true) => sample,
        (true, false) => previous,
        (false, false) => f32::NEG_INFINITY,
    }
}

/// Smooth each bin and decay its peak toward the smoothed value.
///
/// `has_history = false` discards both traces before seeding from the samples.
/// With history, each bin follows [`finite_ema`]'s availability rules.
/// Non-finite previous peaks are unavailable.
/// The caller must supply a finite `alpha` in `0..=1`.
/// `decay_db` must be finite and nonnegative.
/// It sets the peak reduction in dB per call, including calls with unavailable samples.
/// Zero holds the peak.
///
/// # Panics
///
/// All three slices must have equal lengths.
/// A mismatch panics before either output is mutated.
pub(crate) fn average_and_peak(
    samples: &[f32],
    smoothed: &mut [f32],
    peak: &mut [f32],
    alpha: f32,
    decay_db: f32,
    has_history: bool,
) {
    assert_eq!(
        samples.len(),
        smoothed.len(),
        "samples/smoothed length mismatch"
    );
    assert_eq!(samples.len(), peak.len(), "samples/peak length mismatch");
    for ((smoothed, peak), sample) in smoothed
        .iter_mut()
        .zip(peak.iter_mut())
        .zip(samples.iter().copied())
    {
        if !has_history {
            *smoothed = f32::NEG_INFINITY;
            *peak = f32::NEG_INFINITY;
        }
        *smoothed = finite_ema(*smoothed, sample, alpha);
        let decayed_peak = if peak.is_finite() {
            *peak - decay_db
        } else {
            f32::NEG_INFINITY
        };
        *peak = if smoothed.is_finite() {
            decayed_peak.max(*smoothed)
        } else {
            decayed_peak
        };
    }
}

/// Average the quietest fraction of the finite dB measurements.
///
/// The selected count is the finite count divided by `fraction`, rounded down.
/// At least one measurement is selected from nonempty finite input.
/// A zero fraction selects all finite measurements.
/// Empty or all-non-finite input produces `NEG_INFINITY`.
/// `scratch` is reusable workspace with unspecified contents after the call.
///
/// # Panics
///
/// `scratch` must hold at least `values.len()` entries, including unavailable values.
/// Insufficient capacity panics before `scratch` is mutated.
pub(crate) fn quietest_mean(values: &[f32], scratch: &mut [f32], fraction: usize) -> f32 {
    assert!(
        scratch.len() >= values.len(),
        "scratch is shorter than values"
    );
    let mut finite_count = 0;
    for value in values.iter().copied().filter(|value| value.is_finite()) {
        scratch[finite_count] = value;
        finite_count += 1;
    }
    if finite_count == 0 {
        return f32::NEG_INFINITY;
    }

    let count = (finite_count / fraction.max(1)).max(1);
    scratch[..finite_count].select_nth_unstable_by(count - 1, f32::total_cmp);
    scratch[..count].iter().sum::<f32>() / count as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic::{catch_unwind, AssertUnwindSafe};

    #[test]
    fn finite_ema_keeps_the_available_value() {
        assert_eq!(finite_ema(-90.0, -80.0, 0.2), -88.0);
        assert_eq!(finite_ema(f32::NAN, -80.0, 0.2), -80.0);
        assert_eq!(finite_ema(-90.0, f32::NAN, 0.2), -90.0);
        assert_eq!(finite_ema(f32::NAN, f32::INFINITY, 0.2), f32::NEG_INFINITY);
    }

    #[test]
    fn average_and_peak_handles_missing_samples() {
        let mut smoothed = [0.0, 0.0];
        let mut peak = [0.0, 0.0];
        average_and_peak(
            &[f32::NAN, -90.0],
            &mut smoothed,
            &mut peak,
            0.2,
            0.5,
            false,
        );
        average_and_peak(&[-70.0, f32::NAN], &mut smoothed, &mut peak, 0.2, 0.5, true);
        assert_eq!(smoothed, [-70.0, -90.0]);
        assert_eq!(peak, [-70.0, -90.0]);
    }

    #[test]
    fn quietest_mean_ignores_non_finite_values() {
        let values = [-100.0, f32::NAN, -90.0, f32::INFINITY, -80.0];
        let mut scratch = vec![0.0; values.len()];
        assert_eq!(quietest_mean(&values, &mut scratch, 10), -100.0);
    }

    #[test]
    fn finite_ema_handles_all_unavailable_values_and_recovers() {
        for unavailable in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(finite_ema(unavailable, -80.0, 0.2), -80.0);
            assert_eq!(finite_ema(-90.0, unavailable, 0.2), -90.0);
            for other in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
                let absent = finite_ema(unavailable, other, 0.2);
                assert_eq!(absent, f32::NEG_INFINITY);
                assert_eq!(finite_ema(absent, -80.0, 0.2), -80.0);
            }
        }
    }

    #[test]
    fn finite_ema_alpha_boundaries_preserve_seeding() {
        assert_eq!(finite_ema(-90.0, -80.0, 0.0), -90.0);
        assert_eq!(finite_ema(-90.0, -80.0, 1.0), -80.0);
        for alpha in [0.0, 1.0] {
            assert_eq!(finite_ema(f32::NEG_INFINITY, -80.0, alpha), -80.0);
            assert_eq!(finite_ema(-90.0, f32::NAN, alpha), -90.0);
        }
    }

    #[test]
    fn average_and_peak_accepts_empty_traces() {
        for has_history in [false, true] {
            average_and_peak(&[], &mut [], &mut [], 0.2, 0.5, has_history);
        }
    }

    #[test]
    fn average_and_peak_recovers_from_all_unavailable_samples() {
        let unavailable = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY];
        let mut smoothed = [-90.0; 3];
        let mut peak = [-80.0; 3];
        average_and_peak(&unavailable, &mut smoothed, &mut peak, 0.2, 0.5, false);
        assert_eq!(smoothed, [f32::NEG_INFINITY; 3]);
        assert_eq!(peak, [f32::NEG_INFINITY; 3]);
        average_and_peak(&unavailable, &mut smoothed, &mut peak, 0.2, 0.5, true);
        assert_eq!(smoothed, [f32::NEG_INFINITY; 3]);
        assert_eq!(peak, [f32::NEG_INFINITY; 3]);
        average_and_peak(&[-70.0; 3], &mut smoothed, &mut peak, 0.2, 0.5, true);
        assert_eq!(smoothed, [-70.0; 3]);
        assert_eq!(peak, [-70.0; 3]);
    }

    #[test]
    fn average_and_peak_discards_non_finite_previous_peaks() {
        for previous_peak in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            for previous in [-90.0, f32::NEG_INFINITY] {
                let mut smoothed = [previous];
                let mut peak = [previous_peak];
                average_and_peak(&[f32::NAN], &mut smoothed, &mut peak, 0.2, 0.5, true);
                assert_eq!(smoothed, [previous]);
                assert_eq!(peak, [previous]);
                average_and_peak(&[-70.0], &mut smoothed, &mut peak, 1.0, 0.5, true);
                assert_eq!(smoothed, [-70.0]);
                assert_eq!(peak, [-70.0]);
            }
        }
    }

    #[test]
    fn average_and_peak_alpha_and_decay_boundaries() {
        for (alpha, expected) in [(0.0, -90.0), (1.0, -80.0)] {
            for decay in [0.0, 1.0] {
                let mut smoothed = [-90.0];
                let mut peak = [-70.0];
                average_and_peak(&[-80.0], &mut smoothed, &mut peak, alpha, decay, true);
                assert_eq!(smoothed, [expected]);
                assert_eq!(peak, [-70.0 - decay]);
                average_and_peak(&[f32::NAN], &mut smoothed, &mut peak, alpha, decay, true);
                assert_eq!(smoothed, [expected]);
                assert_eq!(peak, [-70.0 - 2.0 * decay]);
            }
        }
        let mut smoothed = [-90.0];
        let mut peak = [-89.5];
        average_and_peak(&[-90.0], &mut smoothed, &mut peak, 1.0, 1.0, true);
        assert_eq!(peak, smoothed);
    }

    #[test]
    fn average_and_peak_rejects_mismatches_before_mutating() {
        for (samples_len, smoothed_len, peak_len) in
            [(1, 2, 2), (2, 1, 2), (2, 2, 1), (2, 2, 3), (0, 1, 1)]
        {
            for sample in [-80.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
                for has_history in [false, true] {
                    let samples = vec![sample; samples_len];
                    let mut smoothed = vec![-90.0; smoothed_len];
                    let mut peak = vec![-70.0; peak_len];
                    let result = catch_unwind(AssertUnwindSafe(|| {
                        average_and_peak(&samples, &mut smoothed, &mut peak, 0.2, 0.5, has_history);
                    }));
                    assert!(result.is_err());
                    assert_eq!(smoothed, vec![-90.0; smoothed_len]);
                    assert_eq!(peak, vec![-70.0; peak_len]);
                }
            }
        }
    }

    #[test]
    fn quietest_mean_empty_and_unavailable_inputs_have_no_measurement() {
        assert_eq!(quietest_mean(&[], &mut [], 10), f32::NEG_INFINITY);
        let mut scratch = [42.0; 3];
        assert_eq!(
            quietest_mean(
                &[f32::NAN, f32::INFINITY, f32::NEG_INFINITY],
                &mut scratch,
                10,
            ),
            f32::NEG_INFINITY
        );
        assert_eq!(
            quietest_mean(&[-90.0, -80.0, -70.0], &mut scratch, 10),
            -90.0
        );
    }

    #[test]
    fn quietest_mean_zero_and_one_select_all_finite_values() {
        let values = [-90.0, f32::NAN, -70.0, f32::INFINITY, f32::NEG_INFINITY];
        let mut scratch = [0.0; 8];
        for fraction in [0, 1] {
            assert_eq!(quietest_mean(&values, &mut scratch, fraction), -80.0);
        }
        assert_eq!(quietest_mean(&values, &mut scratch, usize::MAX), -90.0);
    }

    #[test]
    fn quietest_mean_rejects_short_scratch_before_mutating() {
        for values in [
            [-90.0, -80.0],
            [-90.0, f32::NAN],
            [f32::NAN, f32::INFINITY],
            [f32::NEG_INFINITY, f32::NEG_INFINITY],
        ] {
            for fraction in [0, 1, 10] {
                let mut scratch = [42.0];
                let result = catch_unwind(AssertUnwindSafe(|| {
                    quietest_mean(&values, &mut scratch, fraction);
                }));
                assert!(result.is_err());
                assert_eq!(scratch, [42.0]);
            }
        }
    }
}
