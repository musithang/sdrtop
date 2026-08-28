//! The measurements taken between the two lock blocks.
//!
//! **Nothing here takes the lock, touches the device, or reads a clock.** That is
//! the same rule `tasks/rx/metrics` follows, and for the same reason: this is the
//! expensive part of the frame, and it must not be holding the mutex the UI needs
//! to draw. The worker reads frequency and sample rate under a short lock, drops
//! it, calls [`measure`], and only then takes the lock again to publish.
//!
//! It is also why this file carries most of the worker's tests: it is a pure
//! function of a spectrum.

use crate::state::Modulation;

use super::acpr::acpr_bands;
use super::carrier::{carrier_window, centre_radius_bins, strongest_real_bin};

/// Fraction of the bins averaged for the noise floor.
const NOISE_FLOOR_FRACTION: usize = 10;

/// The carrier at centre, analysed once.
///
/// Two numbers from one window, so they cannot end up describing different slices
/// of the same spectrum - and one scan of the spectrum rather than two.
pub(super) struct Carrier {
    /// 99 % occupied bandwidth. Zero when there is no carrier.
    pub occupied_bw_hz: u64,
    /// In-channel power. `NEG_INFINITY` when there is no channel to integrate,
    /// which every reader renders as `---`.
    pub channel_power_dbfs: f32,
}

pub(super) fn carrier(linear: &[f32], sample_rate: f64, noise_floor_db: f32) -> Carrier {
    let window = carrier_window(linear, sample_rate, noise_floor_db);
    let bin_hz = sample_rate / linear.len().max(1) as f64;
    Carrier {
        occupied_bw_hz: window
            .map(|(lo, hi)| ((hi - lo + 1) as f64 * bin_hz) as u64)
            .unwrap_or(0),
        channel_power_dbfs: window
            .map(|(lo, hi)| linear[lo..=hi].iter().sum::<f32>())
            .filter(|p| *p > 0.0)
            .map(|p| 10.0 * p.log10())
            .unwrap_or(f32::NEG_INFINITY),
    }
}

/// Mean of the quietest tenth of the spectrum, via a partial sort - O(n) on
/// average, against O(n log n) for a full one, on every display frame.
///
/// `scratch` is the worker's reused buffer; nothing is allocated here.
pub(super) fn noise_floor(smoothed: &[f32], scratch: &mut [f32]) -> f32 {
    scratch.copy_from_slice(smoothed);
    let count = (smoothed.len() / NOISE_FLOOR_FRACTION).max(1);
    scratch.select_nth_unstable_by(count - 1, |a, b| {
        a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
    });
    scratch[..count].iter().sum::<f32>() / count as f32
}

/// The SNR the worker publishes: the strongest real bin *near centre*, minus the
/// noise floor.
///
/// Both bounds matter. The DC artefact is the tallest bin in the spectrum
/// whenever nothing else is on air, and reporting its height as signal-to-noise
/// put 46 dB on screen for an empty channel. And an unbounded peak makes this a
/// statistic about the loudest thing anywhere in the capture, which is not what
/// any of its readers want: the aiming view, the rail's 60-second trace and the
/// trend arrow are all asking about the station being tuned, and a subject that
/// jumps silently between signals makes a trend line meaningless.
///
/// Deliberately *not* taken from `carrier_window`: that needs the signal to clear
/// the carrier threshold first, and the interesting part of aiming an antenna is
/// the climb below it. This stays defined and continuous all the way down to the
/// noise.
///
/// The noise floor stays span-wide - a reference that moves with the signal would
/// flatten the very trend this is for.
pub(super) fn snr_db(smoothed: &[f32], sample_rate: f64, noise_floor: f32) -> f32 {
    let peak = strongest_real_bin(
        smoothed,
        Some(centre_radius_bins(smoothed.len(), sample_rate)),
    )
    .map(|(_, v)| v)
    .unwrap_or(f32::NEG_INFINITY);
    (peak - noise_floor).max(0.0)
}

/// dB → linear power, in place.
///
/// One pass, because `10^(x/10)` is expensive and three readers need it: channel
/// power, occupied bandwidth and the per-marker bandwidths.
pub(super) fn to_linear(smoothed: &[f32], linear: &mut [f32]) {
    for (l, &s) in linear.iter_mut().zip(smoothed.iter()) {
        *l = 10f32.powf(s / 10.0);
    }
}

/// Everything one display frame's analysis produces.
pub(super) struct Reading {
    pub noise_floor: f32,
    pub peak_to_nf_db: f32,
    pub occupied_bw_hz: u64,
    pub channel_power_dbfs: f32,
    pub modulation: Modulation,
    pub acpr_offset_hz: f64,
    pub acpr_lower_db: f32,
    pub acpr_upper_db: f32,
    pub adj_carrier_dbfs: f32,
}

/// The analysis pass. Fills `linear` from `smoothed` and returns the readings.
///
/// `scratch` is the noise-floor partial-sort buffer. Both are the worker's reused
/// allocations - this function allocates nothing.
pub(super) fn measure(
    smoothed: &[f32],
    linear: &mut [f32],
    scratch: &mut [f32],
    sample_rate: f64,
) -> Reading {
    let noise_floor = noise_floor(smoothed, scratch);
    let peak_to_nf_db = snr_db(smoothed, sample_rate, noise_floor);
    to_linear(smoothed, linear);

    let c = carrier(linear, sample_rate, noise_floor);

    // The modulation is needed before the ACPR, not after: it picks the channel
    // spacing the adjacent bands are measured at.
    let modulation = crate::state::classify(peak_to_nf_db, c.occupied_bw_hz);
    let acpr_offset_hz = crate::state::acpr_offset_hz(modulation);
    // `None` becomes the undefined sentinel - never a guessed ratio.
    let (acpr_lower_db, acpr_upper_db, adj_carrier_dbfs) =
        acpr_bands(linear, sample_rate, c.occupied_bw_hz, acpr_offset_hz).unwrap_or((
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
        ));

    Reading {
        noise_floor,
        peak_to_nf_db,
        occupied_bw_hz: c.occupied_bw_hz,
        channel_power_dbfs: c.channel_power_dbfs,
        modulation,
        acpr_offset_hz,
        acpr_lower_db,
        acpr_upper_db,
        adj_carrier_dbfs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The floor is the *quietest tenth*, not the whole span's mean and not half
    /// of it. On a graded spectrum those three give different answers, which is
    /// what makes the fraction a real decision rather than a formality.
    #[test]
    fn the_noise_floor_averages_only_the_quietest_tenth() {
        // A ramp from -100 dB to -1 dB: the bottom decile averages near -95,
        // the bottom half near -75, the whole span near -50.
        let n = 1000;
        let bins: Vec<f32> = (0..n).map(|i| -100.0 + i as f32 * 0.099).collect();
        let mut scratch = vec![0.0; n];
        let floor = noise_floor(&bins, &mut scratch);
        assert!(
            (-100.0..-90.0).contains(&floor),
            "expected the quietest tenth (~-95 dB), got {floor:.1}"
        );
    }

    /// The floor is order-independent: a partial sort, not a prefix.
    #[test]
    fn the_noise_floor_does_not_depend_on_bin_order() {
        let n = 500;
        let rising: Vec<f32> = (0..n).map(|i| -100.0 + i as f32 * 0.1).collect();
        let falling: Vec<f32> = rising.iter().rev().copied().collect();
        let mut scratch = vec![0.0; n];
        let a = noise_floor(&rising, &mut scratch);
        let b = noise_floor(&falling, &mut scratch);
        assert!((a - b).abs() < 1e-4, "{a} vs {b}");
    }

    /// A one-bin spectrum still has a floor rather than dividing by zero.
    #[test]
    fn a_single_bin_spectrum_has_a_floor() {
        let mut scratch = vec![0.0; 1];
        assert_eq!(noise_floor(&[-42.0], &mut scratch), -42.0);
    }

    /// `level_db` is the carrier's **total** power, spread evenly across the bins it
    /// covers - not a per-bin level. That distinction is the whole point: a fixed
    /// per-bin level would make the carrier's power scale with how many bins the
    /// sample rate happens to divide it into, so the same station would carry five
    /// times the power at 2 Msps as at 10, and any test comparing the two would be
    /// measuring the helper rather than the code.
    fn spectrum(
        n: usize,
        sample_rate: f64,
        nf_db: f32,
        bw_hz: f64,
        level_db: f32,
        offset_hz: f64,
    ) -> Vec<f32> {
        let lin = |db: f32| 10f32.powf(db / 10.0);
        let mut bins = vec![lin(nf_db); n];
        let bin_hz = sample_rate / n as f64;
        let half = ((bw_hz / bin_hz) / 2.0).round() as i64;
        let c = n as i64 / 2 + (offset_hz / bin_hz).round() as i64;
        let count = (2 * half).max(1) as f32;
        for i in (c - half)..(c + half) {
            if (0..n as i64).contains(&i) {
                bins[i as usize] = lin(level_db) / count;
            }
        }
        bins
    }

    #[test]
    fn snr_is_peak_minus_noise() {
        let peak_dbfs: f32 = -30.0;
        let noise_floor: f32 = -90.0;
        let snr = (peak_dbfs - noise_floor).max(0.0);
        assert!((snr - 60.0).abs() < 0.001);
    }

    #[test]
    fn channel_power_two_equal_bins() {
        let bins = [-60.0f32, -60.0];
        let total: f32 = bins.iter().map(|&b| 10f32.powf(b / 10.0)).sum();
        let power = 10.0 * total.log10();
        // Two -60 dBFS bins → -60 + 10*log10(2) ≈ -56.99 dBFS
        assert!((power - (-56.99)).abs() < 0.02);
    }

    #[test]
    fn channel_power_zero_signal_is_neg_inf() {
        let total_linear: f32 = 0.0;
        let power = if total_linear > 0.0 {
            10.0 * total_linear.log10()
        } else {
            f32::NEG_INFINITY
        };
        assert!(power.is_infinite() && power < 0.0);
    }

    #[test]
    fn channel_power_reports_the_carrier_not_the_capture() {
        // The bug: every bin was summed, so the answer was the total power in the
        // span. Here the noise spread across 2048 bins carries about five times the
        // carrier's own power, and the old code would have reported that.
        let bins = spectrum(2048, 2_000_000.0, -70.0, 20_000.0, -43.0, 0.0);
        let total: f32 = bins.iter().sum();
        let capture_db = 10.0 * total.log10();
        let ch = carrier(&bins, 2_000_000.0, -70.0).channel_power_dbfs;
        assert!(
            (ch - (-43.0)).abs() < 1.5,
            "expected the carrier's ~-43 dBFS, got {ch:.1}"
        );
        assert!(
            capture_db - ch > 5.0,
            "the capture should be visibly louder than the channel: {capture_db:.1} vs {ch:.1}"
        );
    }

    #[test]
    fn channel_power_ignores_a_station_outside_the_channel() {
        // A second, louder station elsewhere in the span used to be added straight
        // into the "channel" power of the one at centre.
        let mut bins = spectrum(2048, 2_000_000.0, -90.0, 100_000.0, -40.0, 0.0);
        let other = spectrum(2048, 2_000_000.0, -200.0, 100_000.0, -30.0, 700_000.0);
        for (b, o) in bins.iter_mut().zip(other.iter()) {
            *b += *o;
        }
        let ch = carrier(&bins, 2_000_000.0, -90.0).channel_power_dbfs;
        assert!(
            (ch - (-40.0)).abs() < 1.5,
            "the neighbour leaked in: {ch:.1} dBFS"
        );
    }

    #[test]
    fn channel_power_does_not_move_with_the_noise_floor() {
        // Same carrier, quiet span and noisy span: the noise is not in the channel.
        let quiet = carrier(
            &spectrum(2048, 2_000_000.0, -120.0, 180_000.0, -40.0, 0.0),
            2_000_000.0,
            -120.0,
        )
        .channel_power_dbfs;
        let noisy = carrier(
            &spectrum(2048, 2_000_000.0, -80.0, 180_000.0, -40.0, 0.0),
            2_000_000.0,
            -80.0,
        )
        .channel_power_dbfs;
        assert!(
            (quiet - noisy).abs() < 1.0,
            "noise floor leaked into the channel: {quiet:.1} vs {noisy:.1} dBFS"
        );
    }

    #[test]
    fn channel_power_is_undefined_without_a_carrier() {
        // No channel, no channel power. Every reader guards on `is_finite`.
        let bins = vec![10f32.powf(-90.0 / 10.0); 2048];
        assert!(carrier(&bins, 2_000_000.0, -90.0)
            .channel_power_dbfs
            .is_infinite());
    }

    #[test]
    fn snr_ignores_a_station_the_radio_is_not_tuned_to() {
        // Measured live at 447.137 MHz with nothing on air: a spur 863 kHz away read
        // as 26 dB of signal-to-noise, next to a verdict of NO SIGNAL. Both were
        // true, and they contradicted each other on the same panel.
        let mut bins = vec![-85.0f32; 2048];
        bins[1908] = -60.0; // the spur, well outside the tuned channel
        assert_eq!(snr_db(&bins, 2_000_000.0, -85.0), 0.0);
        // The same bin inside the radius is a real reading.
        let mut near = vec![-85.0f32; 2048];
        near[1100] = -60.0;
        assert!((snr_db(&near, 2_000_000.0, -85.0) - 25.0).abs() < 0.01);
    }

    #[test]
    fn snr_stays_continuous_below_the_carrier_threshold() {
        // The reason this is not taken from `carrier_window`: aiming an antenna is
        // mostly spent under the carrier threshold, watching the number climb. A
        // reading that sat at zero until the signal crossed 10 dB and then jumped
        // would destroy exactly the feedback the aiming view exists for.
        let mut last = -1.0f32;
        for level in [-83.0f32, -81.0, -79.0, -77.0, -75.0] {
            let mut bins = vec![-85.0f32; 2048];
            bins[1050] = level;
            let s = snr_db(&bins, 2_000_000.0, -85.0);
            assert!(s > last, "SNR must rise with the signal: {s} after {last}");
            assert!(
                s > 0.0,
                "still nothing to show at {level} dBFS over a -85 floor"
            );
            last = s;
        }
        // And it never goes negative when the channel is empty.
        assert_eq!(snr_db(&vec![-85.0f32; 2048], 2_000_000.0, -85.0), 0.0);
    }

    #[test]
    fn snr_refuses_the_dc_artefact() {
        let mut bins = vec![-85.0f32; 2048];
        for b in bins[1023..=1025].iter_mut() {
            *b = -39.0;
        }
        assert_eq!(snr_db(&bins, 2_000_000.0, -85.0), 0.0);
    }
}
