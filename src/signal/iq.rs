// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Shared I/Q quality math used by both the RX task (trend history) and the
//! Lab IQ panels, so the displayed IRR and the logged/trended IRR can never drift
//! apart.

/// Image Rejection Ratio from I/Q amplitude and phase imbalance.
///
/// Exact formula for a direct-conversion quadrature receiver:
///   IRR = 10·log₁₀( (1 + α² + 2α·cosθ) / (1 + α² − 2α·cosθ) )
/// where α = linear amplitude ratio, θ = phase error in radians.
/// Returns 99.9 dB when imbalances are negligible (den ≈ 0 → IRR → ∞).
pub fn image_rejection_db(amp_imbalance_db: f32, phase_imbalance_deg: f32) -> f64 {
    let alpha = 10f64.powf(amp_imbalance_db as f64 / 20.0);
    let theta = phase_imbalance_deg as f64 * std::f64::consts::PI / 180.0;
    let num = 1.0 + alpha * alpha + 2.0 * alpha * theta.cos();
    let den = 1.0 + alpha * alpha - 2.0 * alpha * theta.cos();
    if den <= 1e-12 {
        return 99.9;
    }
    10.0 * (num / den).log10()
}

/// Blind I/Q correction coefficients from DC-removed second moments. Produces a
/// Q-row `(c_qi, c_qq)` such that `q_out = c_qi·i + c_qq·q` is decorrelated from I
/// and power-matched to it (Gram-Schmidt orthonormalisation), which is what
/// cancels the mirror image. Returns identity `(0.0, 1.0)` for already-balanced or
/// degenerate input, so applying it is always safe.
pub fn iq_correction_coeffs(var_i: f64, var_q: f64, cov_iq: f64) -> (f32, f32) {
    if var_i <= 1e-9 {
        return (0.0, 1.0);
    }
    let q_perp_var = (var_q - cov_iq * cov_iq / var_i).max(1e-9);
    let b = (var_i / q_perp_var).sqrt();
    ((-b * cov_iq / var_i) as f32, b as f32)
}

/// Second moments after the Q-row correction `q' = c_qi·i + c_qq·q` is applied to
/// DC-removed samples (I passes through). Lets the RX task report the **residual**
/// imbalance the corrected stream actually has - without a second per-sample
/// accumulator - so the diagnostics agree with the corrected scope/constellation.
/// Returns `(var_i', var_q', cov_iq')`.
pub fn corrected_moments(
    var_i: f64,
    var_q: f64,
    cov_iq: f64,
    c_qi: f64,
    c_qq: f64,
) -> (f64, f64, f64) {
    let var_q2 = c_qi * c_qi * var_i + c_qq * c_qq * var_q + 2.0 * c_qi * c_qq * cov_iq;
    let cov2 = c_qi * var_i + c_qq * cov_iq;
    (var_i, var_q2.max(0.0), cov2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corrected_moments_zeroes_residual_for_its_own_coeffs() {
        // Imbalanced + correlated input → derive its coeffs → applying them must
        // leave Q decorrelated from I (cov'≈0) and power-matched (var_i'≈var_q').
        let (var_i, var_q, cov) = (1.0, 0.36, 0.18);
        let (c_qi, c_qq) = iq_correction_coeffs(var_i, var_q, cov);
        let (vi, vq, cv) = corrected_moments(var_i, var_q, cov, c_qi as f64, c_qq as f64);
        assert!(cv.abs() < 1e-5, "cov should cancel, got {cv}");
        assert!(
            (vi - vq).abs() < 1e-5,
            "power should match, got {vi} vs {vq}"
        );
        // → corrected IRR is effectively perfect.
        let amp = 10.0 * (vi / vq).log10();
        let phase = (2.0 * cv / (vi + vq)).asin().to_degrees();
        assert!(image_rejection_db(amp as f32, phase as f32) > 80.0);
    }

    #[test]
    fn corrected_moments_identity_is_passthrough() {
        let (vi, vq, cv) = corrected_moments(1.0, 0.5, 0.1, 0.0, 1.0);
        assert!((vi - 1.0).abs() < 1e-12 && (vq - 0.5).abs() < 1e-12 && (cv - 0.1).abs() < 1e-12);
    }

    #[test]
    fn coeffs_balanced_is_identity() {
        let (c_qi, c_qq) = iq_correction_coeffs(1.0, 1.0, 0.0);
        assert!(c_qi.abs() < 1e-6 && (c_qq - 1.0).abs() < 1e-6);
    }

    #[test]
    fn coeffs_amplitude_only_scales_q() {
        // I has 4× the power of Q, no correlation → scale Q by 2, no cross term.
        let (c_qi, c_qq) = iq_correction_coeffs(4.0, 1.0, 0.0);
        assert!(c_qi.abs() < 1e-6, "no cross term, got {c_qi}");
        assert!((c_qq - 2.0).abs() < 1e-4, "scale ≈2, got {c_qq}");
    }

    #[test]
    fn coeffs_phase_correlation_decorrelates() {
        // Equal power but correlated → a non-zero cross term that removes the I part.
        let (c_qi, c_qq) = iq_correction_coeffs(1.0, 1.0, 0.3);
        assert!(
            c_qi < 0.0,
            "cross term should subtract the I component, got {c_qi}"
        );
        assert!(c_qq > 1.0, "rescale to restore power, got {c_qq}");
    }

    #[test]
    fn coeffs_degenerate_is_identity() {
        let (c_qi, c_qq) = iq_correction_coeffs(0.0, 1.0, 0.0);
        assert!(c_qi.abs() < 1e-6 && (c_qq - 1.0).abs() < 1e-6);
    }

    #[test]
    fn irr_perfect_balance_is_high() {
        // α=1, θ=0 → den=0 → clamped to 99.9 dB
        let irr = image_rejection_db(0.0, 0.0);
        assert!((irr - 99.9).abs() < 0.01, "got {irr:.1}");
    }

    #[test]
    fn irr_amp_only_0_5db() {
        // 0.5 dB amplitude imbalance, 0° phase → IRR ≈ 32 dB
        let irr = image_rejection_db(0.5, 0.0);
        assert!(irr > 30.0 && irr < 35.0, "expected ~32 dB, got {irr:.1}");
    }

    #[test]
    fn irr_phase_only_2deg() {
        // 0 dB amplitude, 2° phase imbalance ≈ 35.2 dB
        let irr = image_rejection_db(0.0, 2.0);
        assert!(irr > 34.0 && irr < 37.0, "expected ~35.2 dB, got {irr:.1}");
    }

    #[test]
    fn irr_worsens_with_more_imbalance() {
        let irr_low = image_rejection_db(0.5, 1.0);
        let irr_high = image_rejection_db(3.0, 5.0);
        assert!(irr_low > irr_high, "more imbalance should give worse IRR");
    }
}
