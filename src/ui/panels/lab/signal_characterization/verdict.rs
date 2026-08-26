//! The verdict engine: a plain-language read of the same four zones the panel
//! has just printed as numbers.
//!
//! Rule-based and pure — a function of what Tier A already measures (modulation,
//! SNR, ACPR, occupied BW). No ML, no demod; it mirrors
//! `timing_diagnostics::verdict_copy`'s honest-narrative approach.
//!
//! Deliberately free of any drawing: nothing here imports `ratatui`, and nothing
//! here knows how wide the panel is. [`card`](super::card) turns what this
//! returns into rows. That separation is what lets the `lab_signal` marker bar
//! quote the same severity the card shows without pulling a renderer in with it.

use crate::state::Modulation;
use crate::ui::widgets::micro_common::fmt_bw;

/// SNR floor below which the panel won't even hazard a modulation guess (mirrors
/// [`crate::state::CLASSIFY_MIN_SNR_DB`], the classifier's own gate) — below this
/// there's nothing to characterize.
const VERDICT_NO_SIGNAL_SNR_DB: f32 = crate::state::CLASSIFY_MIN_SNR_DB;
/// SNR at/above which the carrier reads as genuinely clean — the same "clean"
/// threshold `snr_color` already uses everywhere else in this panel.
const VERDICT_CLEAN_SNR_DB: f32 = 20.0;
/// ACPR worse (less negative) than this is flagged as adjacent-channel splatter
/// worth a note. sdrtop's own instrument reading, not an asserted regulatory mask
/// — same honesty stance as [`ACPR_BAR_FLOOR_DB`](super::acpr::ACPR_BAR_FLOOR_DB).
const VERDICT_ACPR_CONCERN_DB: f32 = -20.0;

/// The verdict card's severity, driving its colour and mark glyph. `NoSignal`
/// reads dim/neutral, not critical — an empty channel isn't a fault. `pub(crate)`
/// so the lab_signal marker bar's QUALITY field (`lab_chrome::signal_marker_lines`)
/// can read the exact same severity the card shows — one source of truth, same
/// precedent as `image_scope::CarrierImage`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum VerdictLevel {
    Clean,
    Caution,
    NoSignal,
}

impl VerdictLevel {
    /// Short word for a tight space (the marker bar), distinct from the verdict
    /// card's fuller headline (e.g. "WEAK WFM SIGNAL" / "CLEAN WFM SIGNAL").
    pub(crate) fn short_label(self) -> &'static str {
        match self {
            VerdictLevel::Clean => "Clean",
            VerdictLevel::Caution => "Caution",
            VerdictLevel::NoSignal => "No signal",
        }
    }
}

/// The rule itself, returning `(level, headline, detail)`. `pub(crate)` for the
/// same reason as [`VerdictLevel`].
pub(crate) fn verdict(
    modulation: Modulation,
    snr_db: f32,
    acpr_lower_db: f32,
    acpr_upper_db: f32,
    obw_hz: u64,
) -> (VerdictLevel, String, String) {
    if !modulation.is_known() || snr_db < VERDICT_NO_SIGNAL_SNR_DB {
        return (
            VerdictLevel::NoSignal,
            "NO SIGNAL".to_string(),
            "Nothing clearly above the noise floor at centre.".to_string(),
        );
    }

    let obw_str = fmt_bw(obw_hz);
    let mod_label = modulation.label();
    // Worst (least negative — closest to the carrier) adjacent-channel ratio, when
    // the pair was measurable. Both sides, because the clause below prints both: on
    // one alone it would read "ACPR -inf/-38 dB".
    let worst_acpr = (acpr_lower_db.is_finite() && acpr_upper_db.is_finite())
        .then(|| acpr_lower_db.max(acpr_upper_db));

    if snr_db < VERDICT_CLEAN_SNR_DB {
        return (
            VerdictLevel::Caution,
            format!("WEAK {mod_label} SIGNAL"),
            format!("{mod_label} carrier detected, but only {snr_db:.0} dB above the noise floor \u{2014} marginal."),
        );
    }

    if let Some(w) = worst_acpr {
        if w > VERDICT_ACPR_CONCERN_DB {
            return (
                VerdictLevel::Caution,
                format!("{mod_label} CARRIER \u{2014} ADJACENT SPLATTER"),
                format!("Strong carrier ({snr_db:.0} dB), {obw_str} occupied, but only {w:.0} dB adjacent-channel suppression."),
            );
        }
    }

    let acpr_note = match worst_acpr {
        Some(_) => format!(", ACPR {acpr_lower_db:.0}/{acpr_upper_db:.0} dB"),
        None => String::new(),
    };
    (
        VerdictLevel::Clean,
        format!("CLEAN {mod_label} SIGNAL"),
        format!("Strong, well-separated {mod_label} carrier \u{2014} {snr_db:.0} dB above noise, {obw_str} occupied{acpr_note}."),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdict_level_short_labels() {
        assert_eq!(VerdictLevel::Clean.short_label(), "Clean");
        assert_eq!(VerdictLevel::Caution.short_label(), "Caution");
        assert_eq!(VerdictLevel::NoSignal.short_label(), "No signal");
    }

    #[test]
    fn verdict_no_signal_below_classify_gate() {
        // Unknown modulation (e.g. weak/no carrier) never gets a guessed verdict.
        let (level, headline, _) = verdict(Modulation::Unknown, 40.0, -40.0, -40.0, 180_000);
        assert_eq!(level, VerdictLevel::NoSignal);
        assert_eq!(headline, "NO SIGNAL");
        // A known modulation with SNR under the gate is still "no signal".
        let (level, ..) = verdict(Modulation::Wfm, 5.0, -40.0, -40.0, 180_000);
        assert_eq!(level, VerdictLevel::NoSignal);
    }

    #[test]
    fn verdict_weak_signal_below_clean_threshold() {
        let (level, headline, detail) = verdict(Modulation::Wfm, 15.0, -40.0, -40.0, 180_000);
        assert_eq!(level, VerdictLevel::Caution);
        assert!(headline.contains("WEAK"));
        assert!(detail.contains("15 dB"));
    }

    #[test]
    fn verdict_flags_adjacent_splatter_despite_clean_snr() {
        let (level, headline, detail) = verdict(Modulation::Wfm, 40.0, -10.0, -40.0, 180_000);
        assert_eq!(level, VerdictLevel::Caution);
        assert!(headline.contains("SPLATTER"));
        assert!(detail.contains("-10 dB"));
    }

    #[test]
    fn verdict_clean_when_strong_and_well_suppressed() {
        let (level, headline, detail) = verdict(Modulation::Wfm, 43.7, -38.0, -41.0, 180_000);
        assert_eq!(level, VerdictLevel::Clean);
        assert!(headline.contains("CLEAN"));
        assert!(detail.contains("180.0 kHz"));
        assert!(detail.contains("ACPR -38/-41 dB"));
    }

    #[test]
    fn verdict_clean_without_acpr_data_omits_the_clause() {
        // No adjacent-channel measurement yet (e.g. band edge) → no fabricated ACPR note.
        let (level, _, detail) = verdict(
            Modulation::Nfm,
            30.0,
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
            15_000,
        );
        assert_eq!(level, VerdictLevel::Clean);
        assert!(!detail.contains("ACPR"));
    }

    #[test]
    fn verdict_needs_both_sides_before_it_quotes_a_ratio() {
        // The clause prints the pair, so half a measurement must produce none of it
        // rather than "ACPR -inf/-38 dB".
        for (lo, hi) in [(f32::NEG_INFINITY, -38.0), (-38.0, f32::NEG_INFINITY)] {
            let (level, _, detail) = verdict(Modulation::Wfm, 30.0, lo, hi, 120_000);
            assert_eq!(level, VerdictLevel::Clean);
            assert!(
                !detail.contains("ACPR"),
                "half a pair reached the copy: {detail}"
            );
            assert!(
                !detail.contains("inf"),
                "sentinel reached the copy: {detail}"
            );
        }
    }
}
