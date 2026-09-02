// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Adjacent-channel power ratio: how much of this transmission is landing in the
//! channels either side of it.
//!
//! Pure, and deliberately willing to answer `None`: a band that falls off the end
//! of the capture, or one that would overlap the channel being measured, has no
//! ratio - and a guessed one would be worse than none.

/// Ratio floor for the ACPR measurement itself: an adjacent band this far below
/// the in-channel power (or lower) reports as "clean" rather than chasing
/// floating-point underflow toward -inf. Distinct from the panel's own display
/// scale - this is a measurement clamp, not a UI concern.
const ACPR_MEASURE_FLOOR_DB: f32 = -100.0;

/// Adjacent-channel power ratio: the lower/upper adjacent-band power relative to
/// the in-channel power, plus the absolute level (dBFS) of the louder - worse -
/// adjacent band. Each band is the same width as `occupied_bw_hz` (the standard
/// ACPR convention: compare like-sized channels), centred at ±`offset_hz` from
/// the spectrum's centre bin (`n/2`, already fftshifted). `None` when there is no
/// measured channel to compare against, a band would fall outside the captured
/// span (too close to the edge, or the offset exceeds the span), or the bands
/// would overlap the channel they are being compared against.
///
/// That last guard is what the measurement lived without until the occupied
/// bandwidth was scoped to the carrier. A band as wide as the channel, offset by
/// less than that width, is mostly the same bins as the channel - so the ratio
/// comes out at 0 dB and the panel drew a full red bar and called a clean carrier
/// splattered. `None` is the honest answer: at that width, at this spacing, there
/// is nothing to compare.
pub(super) fn acpr_bands(
    linear: &[f32],
    sample_rate: f64,
    occupied_bw_hz: u64,
    offset_hz: f64,
) -> Option<(f32, f32, f32)> {
    let n = linear.len();
    if occupied_bw_hz == 0 || sample_rate <= 0.0 || n == 0 {
        return None;
    }
    if occupied_bw_hz as f64 >= offset_hz {
        return None;
    }
    let bin_hz = sample_rate / n as f64;
    let half_bw_bins = ((occupied_bw_hz as f64 / bin_hz) / 2.0).round() as i64;
    let offset_bins = (offset_hz / bin_hz).round() as i64;
    let center = n as i64 / 2;

    let band_power = |c: i64| -> Option<f32> {
        let lo = c - half_bw_bins;
        let hi = c + half_bw_bins;
        if lo < 0 || hi >= n as i64 || lo > hi {
            return None;
        }
        Some(linear[lo as usize..=hi as usize].iter().sum())
    };

    let ic = band_power(center)?;
    if ic <= 0.0 {
        return None;
    }
    let lower = band_power(center - offset_bins)?;
    let upper = band_power(center + offset_bins)?;

    let ratio_db = |band: f32| {
        if band > 0.0 {
            (10.0 * (band / ic).log10()).max(ACPR_MEASURE_FLOOR_DB)
        } else {
            ACPR_MEASURE_FLOOR_DB
        }
    };
    let lower_db = ratio_db(lower);
    let upper_db = ratio_db(upper);
    let worse_lin = if lower_db >= upper_db { lower } else { upper };
    // A genuinely silent adjacent band has no level to report. The undefined
    // sentinel says so; a floor constant would reach the screen as "-160.0 dBFS".
    let adj_dbfs = if worse_lin > 0.0 {
        10.0 * worse_lin.log10()
    } else {
        f32::NEG_INFINITY
    };
    Some((lower_db, upper_db, adj_dbfs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acpr_bands_computes_relative_and_absolute_adjacent_levels() {
        // 100 bins over 1 MHz (10 kHz/bin). A strong 100 kHz in-channel signal at
        // centre, with both adjacent bands (±200 kHz) 40 dB weaker.
        let n = 100;
        let sample_rate = 1_000_000.0;
        let mut linear = vec![1e-8f32; n];
        let center = n / 2;
        linear[center - 5..center + 5].fill(1.0); // in-channel: sum = 10.0
        linear[center - 25..center - 15].fill(1e-4); // lower adjacent: sum = 1e-3
        linear[center + 15..center + 25].fill(1e-4); // upper adjacent: sum = 1e-3

        let (lo_db, up_db, adj_dbfs) =
            acpr_bands(&linear, sample_rate, 100_000, 200_000.0).unwrap();
        // ratio = 1e-3 / 10.0 = 1e-4 → -40 dB
        assert!((lo_db - (-40.0)).abs() < 0.5, "lo_db={lo_db}");
        assert!((up_db - (-40.0)).abs() < 0.5, "up_db={up_db}");
        // absolute adjacent level: 10*log10(1e-3) = -30 dBFS
        assert!((adj_dbfs - (-30.0)).abs() < 0.5, "adj_dbfs={adj_dbfs}");
    }

    #[test]
    fn acpr_bands_none_when_band_falls_outside_span() {
        let linear = vec![1.0f32; 20];
        // A 900 kHz offset on a 1 MHz / 20-bin span pushes the adjacent band
        // clean off the array - must report "can't measure", not a wrong number.
        assert!(acpr_bands(&linear, 1_000_000.0, 100_000, 900_000.0).is_none());
    }

    #[test]
    fn acpr_bands_none_when_the_bands_would_overlap_the_channel() {
        // The bug that made a clean broadcast read as splattered: each band is as
        // wide as the channel, so an occupancy at or beyond the spacing makes all
        // three bands nearly the same bins. The ratio then comes out at 0 dB and the
        // panel draws a full red bar.
        let linear = vec![1.0f32; 2048];
        assert!(
            acpr_bands(&linear, 2_000_000.0, 300_000, 200_000.0).is_none(),
            "300 kHz occupancy at 200 kHz spacing: the bands overlap"
        );
        assert!(
            acpr_bands(&linear, 2_000_000.0, 200_000, 200_000.0).is_none(),
            "exactly touching is still not a comparison"
        );
        // Clear of the spacing, the measurement stands.
        assert!(acpr_bands(&linear, 2_000_000.0, 120_000, 200_000.0).is_some());
    }

    #[test]
    fn acpr_bands_reports_no_level_for_a_silent_adjacent_band() {
        // `-160.0 dBFS` is a floor constant, not a measurement, and it used to reach
        // the screen as one.
        let n = 2048;
        let mut linear = vec![0.0f32; n];
        for b in linear[n / 2 - 30..n / 2 + 30].iter_mut() {
            *b = 1.0;
        }
        let (_, _, adj) = acpr_bands(&linear, 2_000_000.0, 60_000, 200_000.0).unwrap();
        assert!(
            adj.is_infinite() && adj < 0.0,
            "expected the undefined sentinel, got {adj}"
        );
    }

    #[test]
    fn acpr_bands_none_without_occupied_bandwidth() {
        let linear = vec![1.0f32; 100];
        assert!(acpr_bands(&linear, 1_000_000.0, 0, 200_000.0).is_none());
    }

    #[test]
    fn acpr_bands_clean_adjacent_clamps_to_measure_floor() {
        // In-channel signal, but genuinely silent adjacent bands (linear ~ 0).
        let n = 100;
        let mut linear = vec![0.0f32; n];
        let center = n / 2;
        linear[center - 5..center + 5].fill(1.0);
        let (lo_db, up_db, _) = acpr_bands(&linear, 1_000_000.0, 100_000, 200_000.0).unwrap();
        assert_eq!(lo_db, ACPR_MEASURE_FLOOR_DB);
        assert_eq!(up_db, ACPR_MEASURE_FLOOR_DB);
    }
}
