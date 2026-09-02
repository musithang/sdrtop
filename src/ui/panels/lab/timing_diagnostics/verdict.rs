// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The closing verdict, and the key hints under it.
//!
//! [`verdict_copy`] draws nothing: it takes a severity and two numbers and
//! returns two lines of plain English, so the wording is testable on its own.
//! This panel prints the full four-level [`TimingQuality`] label, unlike
//! `timing_vitals` which collapses the middle two - this is the column with the
//! numbers, so it is the one that can afford the distinction.

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::state::{SdrMetrics, TimingCause};
use crate::ui::widgets::timing_fmt::{fmt_us, quality_color};

use super::rows::Rows;

/// What the copy is allowed to talk about: the grade, the reason the grader
/// gave, and the numbers behind each reason.
///
/// Every field here is **read** before it is mentioned. That is the whole point
/// of the struct: the previous version took three numbers and, for its worst
/// band, printed two sentences about a fourth and a fifth it had never seen.
pub(super) struct Reading {
    pub severity: u8,
    pub cause: TimingCause,
    pub peak_us: u64,
    pub budget_us: u64,
    pub drops_per_sec: u64,
    pub sr_delta_ppm: i64,
}

/// Two-line plain-language verdict copy.
///
/// Keyed off the grade **and the reason for it**, because three independent
/// conditions reach the worst grade and only one of them means samples were
/// lost. Naming a drop that did not happen is worse than saying nothing: it
/// sends someone hunting a USB problem they do not have.
pub(super) fn verdict_copy(r: &Reading) -> [String; 2] {
    let pct = (r.peak_us * 100).checked_div(r.budget_us).unwrap_or(0);
    let worst = fmt_us(r.peak_us);
    let ppm = r.sr_delta_ppm.unsigned_abs();

    match (r.severity, r.cause) {
        (0, _) => [
            "Every callback met its deadline.".into(),
            format!("Worst {worst} ({pct}% of budget)."),
        ],
        // Samples were counted as lost. This is the only branch that may say so.
        (_, TimingCause::Drops) => [
            "Overrun \u{2014} samples lost.".into(),
            format!("{}/s dropped.", r.drops_per_sec),
        ],
        (_, TimingCause::Clock) => [
            "Sample clock is off the configured rate.".into(),
            format!("{ppm} ppm out, nothing lost."),
        ],
        (3, _) => [
            "Deadlines missed by a wide margin.".into(),
            format!("Worst {worst} ({pct}%), nothing lost."),
        ],
        _ => [
            "Real-time deadlines under pressure.".into(),
            format!("Worst {worst} ({pct}%), no drops yet."),
        ],
    }
}

pub(super) fn lines(state: &SdrMetrics, r: &Rows) -> Vec<Line<'static>> {
    let theme = r.theme;
    if r.stale {
        return vec![Line::from(vec![
            Span::raw(" "),
            Span::styled("\u{25cb} IDLE \u{2014} RX stopped", r.dim()),
        ])];
    }

    let t = &state.timing;
    let q = t.timing_quality;
    let mark = if q.severity() == 0 {
        "\u{2713}"
    } else {
        "\u{26a0}"
    };

    let mut out = vec![Line::from(vec![
        Span::raw(" "),
        Span::styled(
            format!("{mark} {}", q.label()),
            Style::default()
                .fg(quality_color(q, theme))
                .add_modifier(Modifier::BOLD),
        ),
    ])];
    let reading = Reading {
        severity: q.severity(),
        cause: t.timing_cause,
        peak_us: t.dev_peak_us,
        budget_us: t.deadline_budget_us,
        drops_per_sec: state.signal.drops_per_sec,
        sr_delta_ppm: t.sr_delta_ppm,
    };
    for copy in verdict_copy(&reading) {
        out.push(Line::from(vec![
            Span::raw(" "),
            Span::styled(copy, r.lbl()),
        ]));
    }
    out.push(Line::raw(""));
    out.push(Line::from(vec![
        Span::raw(" "),
        Span::styled("[R]", r.key()),
        Span::styled(" reset peak  ", r.lbl()),
        Span::styled("[C]", r.key()),
        Span::styled(" clear counters", r.lbl()),
    ]));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reading with nothing wrong, for tests to vary one field of.
    fn settled() -> Reading {
        Reading {
            severity: 0,
            cause: TimingCause::Settled,
            peak_us: 210,
            budget_us: 603,
            drops_per_sec: 0,
            sr_delta_ppm: 0,
        }
    }

    #[test]
    fn verdict_copy_folds_in_numbers_and_state() {
        let ok = verdict_copy(&settled());
        assert!(ok[0].contains("met its deadline"));
        assert!(
            ok[1].contains("210") && ok[1].contains("34%"),
            "{:?}",
            ok[1]
        );

        let bad = verdict_copy(&Reading {
            severity: 3,
            cause: TimingCause::Drops,
            peak_us: 6_300,
            drops_per_sec: 12,
            ..settled()
        });
        assert!(bad[0].contains("Overrun"));
        assert!(bad[1].contains("12"), "it names the count: {:?}", bad[1]);
    }

    /// The regression this checkpoint exists for.
    ///
    /// A SoapySDR read loop is a pull, so a burst of fast reads followed by a
    /// wait puts the p99 deviation far over the deadline budget on a link that
    /// is losing nothing. Measured on a HackRF through SoapyHackRF: 4378 % of
    /// budget, 136 late of 160, and `drops 0/s` printed two lines above. The
    /// old copy answered that with "block dropped, resynced" and "ring buffer
    /// hit its ceiling", neither of which it had looked at.
    #[test]
    fn the_worst_grade_without_drops_never_claims_a_drop() {
        let jittery = verdict_copy(&Reading {
            severity: 3,
            cause: TimingCause::Jitter,
            peak_us: 6_568,
            budget_us: 150,
            drops_per_sec: 0,
            sr_delta_ppm: -576,
        });
        let joined = jittery.join(" ").to_lowercase();
        // Claims of loss, not the word: "nothing lost" is exactly what this copy
        // is supposed to say, and it contains "lost".
        for claim in ["drop", "overrun", "samples lost", "ceiling", "resync"] {
            assert!(
                !joined.contains(claim),
                "nothing was lost, so the copy must not say {claim:?}: {joined:?}"
            );
        }
        assert!(joined.contains("nothing lost"), "{joined:?}");
        // The numbers are the ones actually measured on the SoapySDR path.
        assert!(joined.contains("4378%"), "{joined:?}");
    }

    /// The other cause that reaches the worst grade with no drops. A clock
    /// 600 ppm out is not a deadline problem and must not be worded as one.
    #[test]
    fn a_clock_fault_is_named_as_a_clock_fault() {
        let v = verdict_copy(&Reading {
            severity: 3,
            cause: TimingCause::Clock,
            sr_delta_ppm: -600,
            ..settled()
        });
        assert!(v[0].to_lowercase().contains("clock"), "{:?}", v[0]);
        assert!(v[1].contains("600"), "it names the offset: {:?}", v[1]);
        assert!(!v.join(" ").to_lowercase().contains("drop"), "{v:?}");
    }

    /// A zero budget must not divide by zero; the copy degrades to 0 %.
    #[test]
    fn a_zero_budget_reads_as_zero_percent() {
        let v = verdict_copy(&Reading {
            budget_us: 0,
            ..settled()
        });
        assert!(v[1].contains("0%"), "{:?}", v[1]);
    }

    /// The middle severities share their copy, and the ends do not.
    #[test]
    fn each_band_says_something_different() {
        let jitter = |severity| {
            verdict_copy(&Reading {
                severity,
                cause: if severity == 0 {
                    TimingCause::Settled
                } else {
                    TimingCause::Jitter
                },
                peak_us: 100,
                budget_us: 600,
                ..settled()
            })
        };
        assert_eq!(jitter(1), jitter(2));
        assert_ne!(jitter(0), jitter(1));
        assert_ne!(jitter(2), jitter(3));
    }
}
