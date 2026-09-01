//! Peak-to-Average Power Ratio, estimated from the amplitude histogram.
//!
//! Pure arithmetic over the bin counts, with no access to the panel - which is
//! why it already had tests before the split, and why they moved here unchanged.

use super::zones::BINS;

/// Estimated Peak-to-Average Power Ratio from the IQ amplitude histogram.
///
/// Bin mapping (from the rx callback): the 0..full-scale amplitude range is cut
/// into [`BINS`] equal buckets, so bin `i` normalised runs `[i/32, (i+1)/32)`.
/// Peak amplitude = top of the highest occupied bin.
/// RMS amplitude  = sqrt( Σ hist[i]·((i+0.5)/32)² / total ).
/// Returns None when no samples or when all samples are in bin 0 (zero RMS).
///
/// **The device's full scale cancels out.** It used to appear here as a literal
/// 128 alongside a literal bin width of 4, which was two device facts hiding in
/// a ratio that does not depend on either: a peak-to-average *ratio* is the same
/// number on an 8-bit radio and a 16-bit one. So this needs no device at all.
pub(super) fn estimate_papr_db(hist: &[u64; BINS], total: u64) -> Option<f64> {
    if total == 0 {
        return None;
    }

    let peak_bin = hist
        .iter()
        .enumerate()
        .rev()
        .find(|(_, &c)| c > 0)
        .map(|(i, _)| i)?;
    let peak_amp = (peak_bin + 1) as f64 / BINS as f64;

    let mean_sq: f64 = hist
        .iter()
        .enumerate()
        .map(|(i, &c)| {
            let amp = (i as f64 + 0.5) / BINS as f64; // bin centre, normalised
            c as f64 * amp * amp
        })
        .sum::<f64>()
        / total as f64;

    let rms = mean_sq.sqrt();
    if rms <= 0.0 {
        return None;
    }

    Some(20.0 * (peak_amp / rms).log10())
}

/// What a crest factor of this size usually means, as a one-word hint.
pub(super) fn papr_hint(db: f64) -> &'static str {
    if db < 3.0 {
        "CW / FM"
    } else if db < 8.0 {
        "AM / mixed"
    } else if db < 15.0 {
        "wideband"
    } else {
        "impulsive"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn papr_none_when_empty() {
        assert!(estimate_papr_db(&[0u64; 32], 0).is_none());
    }

    #[test]
    fn papr_single_bin_is_near_zero_db() {
        // All samples in one mid/high bin → peak_top/bin_centre ≈ 1 → PAPR near 0 dB.
        // (Bin 0 is a special case: ratio = 4/2 = 2 → 6 dB, a quantisation artefact
        //  for noise-floor signals - the status label already flags those as "weak".)
        let mut hist = [0u64; 32];
        hist[20] = 1000;
        let papr = estimate_papr_db(&hist, 1000).unwrap();
        assert!(
            papr.abs() < 1.0,
            "single-bin PAPR (bin 20) should be ~0.2 dB, got {:.2}",
            papr
        );
    }

    #[test]
    fn papr_uniform_distribution_near_5db() {
        // Uniform distribution: peak is max bin, rms is ~sqrt(1/3) of peak
        // → PAPR ≈ 20*log10(sqrt(3)) ≈ 4.8 dB
        let hist = [100u64; 32];
        let papr = estimate_papr_db(&hist, 3200).unwrap();
        assert!(
            papr > 3.0 && papr < 7.0,
            "uniform PAPR should be ~4.8 dB, got {:.2}",
            papr
        );
    }

    #[test]
    fn papr_low_amplitude_signal_computable() {
        // All samples in lowest bins → PAPR should still compute without panic
        let mut hist = [0u64; 32];
        hist[0] = 500;
        hist[1] = 300;
        hist[2] = 100;
        let papr = estimate_papr_db(&hist, 900);
        assert!(
            papr.is_some(),
            "should return Some for low-amplitude signal"
        );
    }

    #[test]
    fn papr_hint_coverage() {
        assert_eq!(papr_hint(1.0), "CW / FM");
        assert_eq!(papr_hint(5.0), "AM / mixed");
        assert_eq!(papr_hint(10.0), "wideband");
        assert_eq!(papr_hint(20.0), "impulsive");
    }
}
