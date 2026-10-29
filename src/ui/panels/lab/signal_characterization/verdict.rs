// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The verdict's words: a plain-language read of the carrier the panel has
//! just printed as numbers.
//!
//! The rule that picks the kind (`state::judge`) runs where the frames arrive,
//! so the kind can be held a second before it changes (`fft::readout`); what
//! is here is the copy for the kind held, filled with the carrier's figures as
//! the panel shows them. No ML, no demod; it mirrors
//! `timing_diagnostics::verdict_copy`'s honest-narrative approach.
//!
//! Deliberately free of any drawing: nothing here imports `ratatui`, and nothing
//! here knows how wide the panel is. [`card`](super::card) turns what this
//! returns into rows. That separation is what lets the `lab_signal` marker bar
//! quote the same severity the card shows without pulling a renderer in with it.

use crate::state::{Carrier, Verdict};
use crate::ui::widgets::micro_common::fmt_bw;

/// The verdict card's severity, driving its colour and mark glyph. `NoSignal`
/// reads dim/neutral, not critical - an empty channel isn't a fault; nor is
/// `Judging`, a carrier heard for less than the second a verdict waits for.
/// `pub(crate)` so the lab_signal marker bar's QUALITY field
/// (`lab_chrome::signal_marker_lines`) can read the exact same severity the
/// card shows - one source of truth, same precedent as
/// `image_scope::CarrierImage`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum VerdictLevel {
    Clean,
    Caution,
    Judging,
    NoSignal,
}

impl VerdictLevel {
    /// Short word for a tight space (the marker bar), distinct from the verdict
    /// card's fuller headline (e.g. "WEAK WFM SIGNAL" / "CLEAN WFM SIGNAL").
    pub(crate) fn short_label(self) -> &'static str {
        match self {
            VerdictLevel::Clean => "Clean",
            VerdictLevel::Caution => "Caution",
            VerdictLevel::Judging => "Judging",
            VerdictLevel::NoSignal => "No signal",
        }
    }
}

/// The verdict on `carrier`, as `(level, headline, detail)`; no carrier heard
/// for a second is no signal. `pub(crate)` for the same reason as
/// [`VerdictLevel`].
pub(crate) fn verdict(carrier: Option<&Carrier>) -> (VerdictLevel, String, String) {
    let Some(c) = carrier else {
        return (
            VerdictLevel::NoSignal,
            "NO SIGNAL".to_string(),
            "Nothing clearly above the noise floor at centre.".to_string(),
        );
    };
    // Wider than any one broadcast, the window has walked through station
    // after station: there is no carrier here to call clean or weak.
    if c.occupied_bw_hz > crate::state::WFM_MAX_BW_HZ {
        let bw = fmt_bw(c.occupied_bw_hz);
        return (
            VerdictLevel::Caution,
            "MANY SIGNALS".to_string(),
            format!(
                "{bw} wide at centre, more than any one broadcast: several signals, none judged."
            ),
        );
    }
    let Some(kind) = c.verdict else {
        return (
            VerdictLevel::Judging,
            "CARRIER HEARD".to_string(),
            "Judged once it has held for a second.".to_string(),
        );
    };
    let snr_db = c.snr_db;
    let obw_str = fmt_bw(c.occupied_bw_hz);
    // The pair as the ACPR rows print it, when both sides are measured: on one
    // alone the clause would read "ACPR -inf/-38 dB".
    let acpr = c
        .acpr_lower
        .zip(c.acpr_upper)
        .map(|(l, u)| (l.readout, u.readout));
    // The class held for the badge; none named while none has held.
    let m = c.modulation;
    let (class, class_sp) = if m.is_known() {
        (m.label(), format!("{} ", m.label()))
    } else {
        ("", String::new())
    };
    match kind {
        Verdict::Weak => (
            VerdictLevel::Caution,
            format!("WEAK {class_sp}SIGNAL"),
            format!(
                "{} detected, but only {snr_db:.0} dB above the noise floor \u{2014} marginal.",
                if class.is_empty() {
                    "Carrier".to_string()
                } else {
                    format!("{class} carrier")
                }
            ),
        ),
        Verdict::Splatter => {
            let detail = match acpr {
                Some((l, u)) => format!(
                    "Strong carrier ({snr_db:.0} dB), {obw_str} occupied, but only {:.0} dB adjacent-channel suppression.",
                    l.max(u)
                ),
                None => format!(
                    "Strong carrier ({snr_db:.0} dB), {obw_str} occupied, but poor adjacent-channel suppression."
                ),
            };
            (
                VerdictLevel::Caution,
                format!("{class_sp}CARRIER \u{2014} ADJACENT SPLATTER"),
                detail,
            )
        }
        Verdict::Clean => {
            let acpr_note = match acpr {
                Some((l, u)) => format!(", ACPR {l:.0}/{u:.0} dB"),
                None => String::new(),
            };
            (
                VerdictLevel::Clean,
                format!("CLEAN {class_sp}SIGNAL"),
                format!("Strong, well-separated {class_sp}carrier \u{2014} {snr_db:.0} dB above noise, {obw_str} occupied{acpr_note}."),
            )
        }
    }
}

/// A carrier as the readout would publish it from a steady frame, judged.
#[cfg(test)]
pub(crate) fn steady(
    modulation: crate::state::Modulation,
    snr_db: f32,
    acpr_lower_db: f32,
    acpr_upper_db: f32,
    occupied_bw_hz: u64,
) -> Carrier {
    let shown = |db: f32| {
        db.is_finite().then_some(crate::signal::ballistics::Shown {
            level: db,
            peak: db,
            readout: db,
        })
    };
    Carrier {
        snr_db,
        occupied_bw_hz,
        adj_carrier_dbfs: -80.0,
        acpr_lower: shown(acpr_lower_db),
        acpr_upper: shown(acpr_upper_db),
        verdict: crate::state::judge(modulation, snr_db, acpr_lower_db, acpr_upper_db),
        modulation,
        heard: std::time::Instant::now(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Modulation;

    fn on(m: Modulation, snr: f32, lo: f32, hi: f32, obw: u64) -> (VerdictLevel, String, String) {
        verdict(Some(&steady(m, snr, lo, hi, obw)))
    }

    /// **A band of stations is many signals**, never one clean carrier: the
    /// 40 m slice that once read `CLEAN WFM SIGNAL` at 657 kHz says so.
    #[test]
    fn a_band_of_stations_is_many_signals() {
        let c = steady(
            crate::state::Modulation::Unknown,
            53.0,
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
            657_400,
        );
        let (level, headline, detail) = verdict(Some(&c));
        assert_eq!(level, VerdictLevel::Caution);
        assert_eq!(headline, "MANY SIGNALS");
        assert!(
            detail.starts_with("657.4 kHz wide") && !detail.contains("carrier"),
            "{detail}"
        );
    }

    #[test]
    fn verdict_level_short_labels() {
        assert_eq!(VerdictLevel::Clean.short_label(), "Clean");
        assert_eq!(VerdictLevel::Caution.short_label(), "Caution");
        assert_eq!(VerdictLevel::Judging.short_label(), "Judging");
        assert_eq!(VerdictLevel::NoSignal.short_label(), "No signal");
    }

    #[test]
    fn verdict_no_signal_without_a_carrier() {
        let (level, headline, _) = verdict(None);
        assert_eq!(level, VerdictLevel::NoSignal);
        assert_eq!(headline, "NO SIGNAL");
    }

    #[test]
    fn a_carrier_not_yet_held_is_being_judged() {
        let mut c = steady(Modulation::Wfm, 40.0, -40.0, -40.0, 180_000);
        c.verdict = None;
        let (level, headline, _) = verdict(Some(&c));
        assert_eq!(level, VerdictLevel::Judging);
        assert_eq!(headline, "CARRIER HEARD");
    }

    /// A verdict settled before the class has: the words name no class
    /// rather than print the badge's dash.
    #[test]
    fn a_verdict_without_a_class_names_none() {
        let mut c = steady(Modulation::Wfm, 43.0, -38.0, -41.0, 180_000);
        c.modulation = Modulation::Unknown;
        let (_, headline, detail) = verdict(Some(&c));
        assert_eq!(headline, "CLEAN SIGNAL");
        assert!(
            detail.starts_with("Strong, well-separated carrier"),
            "{detail}"
        );
    }

    #[test]
    fn verdict_weak_signal_below_clean_threshold() {
        let (level, headline, detail) = on(Modulation::Wfm, 15.0, -40.0, -40.0, 180_000);
        assert_eq!(level, VerdictLevel::Caution);
        assert!(headline.contains("WEAK"));
        assert!(detail.contains("15 dB"));
    }

    #[test]
    fn verdict_flags_adjacent_splatter_despite_clean_snr() {
        let (level, headline, detail) = on(Modulation::Wfm, 40.0, -10.0, -40.0, 180_000);
        assert_eq!(level, VerdictLevel::Caution);
        assert!(headline.contains("SPLATTER"));
        assert!(detail.contains("-10 dB"));
    }

    #[test]
    fn verdict_clean_when_strong_and_well_suppressed() {
        let (level, headline, detail) = on(Modulation::Wfm, 43.7, -38.0, -41.0, 180_000);
        assert_eq!(level, VerdictLevel::Clean);
        assert!(headline.contains("CLEAN"));
        assert!(detail.contains("180.0 kHz"));
        assert!(detail.contains("ACPR -38/-41 dB"));
    }

    #[test]
    fn verdict_clean_without_acpr_data_omits_the_clause() {
        // No adjacent-channel measurement yet (e.g. band edge) → no fabricated ACPR note.
        let (level, _, detail) = on(
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
            let (level, _, detail) = on(Modulation::Wfm, 30.0, lo, hi, 120_000);
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
