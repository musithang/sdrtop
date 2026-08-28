//! Where the carrier is, where its mirror is, and how far apart they sit.
//!
//! Pure and deterministic: [`detect_image`] takes a slice of bins and returns the
//! reading, with no theme, no widths and no frame in it. That is what lets the
//! Lab IQ marker bar and `input.rs`'s `[M]` pin call [`carrier_image`] and get the
//! same answer the scope is drawing.

use crate::state::{FftFrame, SdrMetrics};

/// How far above the noise floor (dB) the strongest bin must sit before the auto
/// path treats it as a carrier. Below this there is no signal to measure - only
/// noise - so the scope reports "no carrier" rather than a random noise peak.
/// A placed marker / pin bypasses this gate.
const CARRIER_MIN_SNR_DB: f32 = 10.0;

use super::CarrierImage;

/// Resolve the carrier and its LO-mirror image into absolute frequencies + levels,
/// honouring a placed marker / pin as the carrier (see [`carrier_hint_bin`]).
/// Shared by the scope panel and the marker bar so both tell the same story.
/// `None` when there is no frame yet or it is too small / silent.
pub(crate) fn carrier_image(state: &SdrMetrics) -> Option<CarrierImage> {
    let frame = state.waterfall.last_fft.as_ref()?;
    let hint = carrier_hint_bin(state, frame);
    let r = detect_image(&frame.bins_dbfs, frame.sample_rate, frame.noise_floor, hint)?;
    let center = frame.center_freq_hz as f64;
    Some(CarrierImage {
        carrier_hz: (center + r.carrier_offset_hz).round() as u64,
        image_hz: (center - r.carrier_offset_hz).round() as u64,
        carrier_dbfs: r.carrier_dbfs,
        image_dbfs: r.image_dbfs,
        suppression_db: r.suppression_db,
    })
}

/// Carrier / image / DC read-out derived from one fftshifted FFT frame.
pub(super) struct ImageReadout {
    /// Which bin the carrier was found in. Nothing draws it - the panel works in
    /// frequencies - but it is the single most important thing `detect_image`
    /// decides, and every test in this module asserts on it. Kept as the
    /// detection's assertion surface, and as where a future cursor would read
    /// from. (This is what the old `let _ = r.carrier_idx;` in `render` was for;
    /// the reason belongs here, not in a no-op statement three screens away.)
    #[cfg_attr(not(test), allow(dead_code))]
    pub carrier_idx: usize,
    pub carrier_dbfs: f32,
    pub image_dbfs: f32,
    pub dc_dbfs: f32,
    /// carrier − image, in dB (positive = image is below the carrier).
    pub suppression_db: f32,
    /// Carrier offset from the LO, signed (Hz).
    pub carrier_offset_hz: f64,
}

/// Locate the carrier, its mirror about the centre (LO) bin, and the DC-spike
/// level. The carrier is `carrier_hint` when supplied and valid (a placed marker
/// or pin); otherwise the strongest bin outside a small DC guard, when it clears
/// the noise floor by [`CARRIER_MIN_SNR_DB`]. `None` when the
/// frame is too small / silent. Pure + deterministic for unit testing.
pub(super) fn detect_image(
    bins: &[f32],
    sample_rate: f64,
    noise_floor: f32,
    carrier_hint: Option<usize>,
) -> Option<ImageReadout> {
    let n = bins.len();
    if n < 8 {
        return None;
    }
    let center = n / 2;
    let guard = (n / 64).max(2);
    let in_band = |i: usize| i < n && (i as isize - center as isize).unsigned_abs() > guard;

    let carrier_idx = match carrier_hint {
        // Honour a hinted carrier (marker / pin) when it lands in a usable band,
        // regardless of strength - the operator may be probing a deliberately
        // weak signal, so explicit intent overrides the noise gate below.
        Some(h) if in_band(h) => h,
        // Auto path: the strongest bin outside the DC guard, but only when it
        // stands clear of the noise floor. With no real carrier present (just
        // noise), the loudest bin is a random noise peak - reporting it as a
        // "carrier" would show a meaningless, alarming image-suppression figure
        // and make the scope's frequency axis jitter frame to frame. Treat that
        // as "no signal" instead.
        _ => {
            let mut idx = center;
            let mut best = f32::NEG_INFINITY;
            for (i, &v) in bins.iter().enumerate() {
                if !in_band(i) {
                    continue;
                }
                if v > best {
                    best = v;
                    idx = i;
                }
            }
            if best < noise_floor + CARRIER_MIN_SNR_DB {
                return None;
            }
            idx
        }
    };

    let image_idx = (2 * center).saturating_sub(carrier_idx).min(n - 1);
    let bin_hz = sample_rate / n as f64;
    Some(ImageReadout {
        carrier_idx,
        carrier_dbfs: bins[carrier_idx],
        image_dbfs: bins[image_idx],
        dc_dbfs: bins[center],
        suppression_db: bins[carrier_idx] - bins[image_idx],
        carrier_offset_hz: (carrier_idx as f64 - center as f64) * bin_hz,
    })
}

/// Resolve the carrier bin from operator intent, in priority order:
/// 1. an explicit `[M]` pin, 2. the strongest **placed spectrum marker**, else
///    `None` so [`detect_image`] auto-picks the strongest bin. This is what makes
///    a marker you set on the spectrum actually drive the image calculation.
pub(super) fn carrier_hint_bin(state: &SdrMetrics, frame: &FftFrame) -> Option<usize> {
    let n = frame.bins_dbfs.len();
    let to_bin = |f: u64| freq_to_bin(f, frame.center_freq_hz, frame.sample_rate, n);

    if let Some((carrier_hz, _)) = state.lab.iq_marker_pin {
        if let Some(b) = to_bin(carrier_hz) {
            return Some(b);
        }
    }
    state
        .spectrum
        .markers
        .iter()
        .filter_map(|m| to_bin(m.freq_hz).map(|b| (b, frame.bins_dbfs[b])))
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(b, _)| b)
}

/// Map an absolute frequency to a bin index in the fftshifted frame, or `None` if
/// it falls outside the captured span. Uses the canonical fftshift convention
/// (`bin = n/2 + (f − f_c)·n/rate`, bin `n/2` = DC), the exact inverse of the
/// per-bin frequency the scope and `carrier_offset_hz` use - so a frequency and
/// its bin round-trip without the off-by-one an `(n−1)` mapping introduces.
fn freq_to_bin(freq_hz: u64, center_freq_hz: u64, sample_rate: f64, n: usize) -> Option<usize> {
    if n == 0 || sample_rate <= 0.0 {
        return None;
    }
    let bin_hz = sample_rate / n as f64;
    let b = (n as f64 / 2.0 + (freq_hz as f64 - center_freq_hz as f64) / bin_hz).round();
    if b < 0.0 || b >= n as f64 {
        return None;
    }
    Some(b as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(n: usize) -> Vec<f32> {
        vec![-110.0; n]
    }

    #[test]
    fn detect_image_finds_carrier_and_mirror() {
        let mut b = frame(64);
        b[40] = -8.0; // carrier, +8 bins from centre (32)
        b[24] = -64.0; // its mirror, −8 bins
        b[32] = -20.0; // DC spike
        let r = detect_image(&b, 64.0, -110.0, None).unwrap();
        assert_eq!(r.carrier_idx, 40);
        assert!((r.carrier_dbfs - (-8.0)).abs() < 1e-6);
        assert!((r.image_dbfs - (-64.0)).abs() < 1e-6);
        assert!((r.dc_dbfs - (-20.0)).abs() < 1e-6);
        assert!((r.suppression_db - 56.0).abs() < 1e-6);
        assert!(
            (r.carrier_offset_hz - 8.0).abs() < 1e-6,
            "off {}",
            r.carrier_offset_hz
        );
    }

    #[test]
    fn detect_image_ignores_dc_spike_as_carrier() {
        // A tall DC spike inside the guard band must not be picked as the carrier.
        let mut b = frame(64);
        b[32] = 0.0; // huge DC, at centre
        b[44] = -12.0; // the real carrier
        let r = detect_image(&b, 64.0, -110.0, None).unwrap();
        assert_eq!(r.carrier_idx, 44, "carrier should skip the guarded DC bin");
    }

    #[test]
    fn detect_image_honours_carrier_hint() {
        // A weaker bin chosen by a marker must override the strongest-bin auto-pick.
        let mut b = frame(64);
        b[50] = -4.0; // strongest peak (would auto-win)
        b[40] = -18.0; // the bin the operator marked
        let r = detect_image(&b, 64.0, -110.0, Some(40)).unwrap();
        assert_eq!(r.carrier_idx, 40, "hint should drive the carrier");
        assert_eq!((2 * 32usize) - 40, 24);
        assert!((r.carrier_dbfs - (-18.0)).abs() < 1e-6);
    }

    #[test]
    fn detect_image_invalid_hint_falls_back_to_auto() {
        let mut b = frame(64);
        b[50] = -4.0;
        // A hint inside the DC guard is rejected → auto-pick the strongest bin.
        let r = detect_image(&b, 64.0, -110.0, Some(33)).unwrap();
        assert_eq!(r.carrier_idx, 50);
        // An out-of-range hint is likewise ignored.
        let r2 = detect_image(&b, 64.0, -110.0, Some(999)).unwrap();
        assert_eq!(r2.carrier_idx, 50);
    }

    #[test]
    fn freq_to_bin_maps_endpoints_and_centre() {
        // 64-bin frame, 64 Hz span centred on 1000 Hz → 1 Hz/bin, left edge = 968.
        assert_eq!(freq_to_bin(968, 1000, 64.0, 64), Some(0));
        assert_eq!(freq_to_bin(1000, 1000, 64.0, 64), Some(32)); // centre = n/2
        assert_eq!(freq_to_bin(2000, 1000, 64.0, 64), None); // out of span
    }

    #[test]
    fn detect_image_too_small_is_none() {
        assert!(detect_image(&frame(4), 64.0, -110.0, None).is_none());
    }

    #[test]
    fn detect_image_gates_noise_when_no_carrier() {
        // Only noise: the loudest in-band bin sits a few dB over the floor, below
        // the SNR gate → auto-detection reports no carrier (not a noise peak).
        let mut b = frame(64); // floor -110
        b[40] = -104.0; // a 6 dB noise bump, under the 10 dB gate
        assert!(
            detect_image(&b, 64.0, -110.0, None).is_none(),
            "a sub-gate noise peak must not be reported as a carrier"
        );
        // The same weak bin is still measured when the operator marks it explicitly.
        let r = detect_image(&b, 64.0, -110.0, Some(40)).unwrap();
        assert_eq!(
            r.carrier_idx, 40,
            "an explicit hint bypasses the noise gate"
        );
        // A real carrier well above the floor passes the auto gate.
        b[44] = -70.0;
        let r2 = detect_image(&b, 64.0, -110.0, None).unwrap();
        assert_eq!(r2.carrier_idx, 44);
    }
}
