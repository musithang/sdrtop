//! The closing verdict, and the key hints under it.
//!
//! [`verdict_copy`] draws nothing: it takes a severity and two numbers and
//! returns two lines of plain English, so the wording is testable on its own.
//! This panel prints the full four-level [`TimingQuality`] label, unlike
//! `timing_vitals` which collapses the middle two — this is the column with the
//! numbers, so it is the one that can afford the distinction.

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::state::SdrMetrics;
use crate::ui::widgets::timing_fmt::{fmt_us, quality_color};

use super::rows::Rows;

/// Two-line plain-language verdict copy, keyed off the 4-level severity, with live
/// numbers folded in (worst deviation and its share of the budget).
pub(super) fn verdict_copy(severity: u8, peak_us: u64, budget_us: u64) -> [String; 2] {
    let pct = (peak_us * 100).checked_div(budget_us).unwrap_or(0);
    match severity {
        0 => [
            "Every callback met its deadline.".into(),
            format!("Worst {} ({pct}% of budget).", fmt_us(peak_us)),
        ],
        1 | 2 => [
            "Real-time deadlines under pressure.".into(),
            format!("Worst {} ({pct}%), no drops yet.", fmt_us(peak_us)),
        ],
        _ => [
            "Overrun \u{2014} block dropped, resynced.".into(),
            "Ring buffer hit its ceiling.".into(),
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
    for copy in verdict_copy(q.severity(), t.dev_peak_us, t.deadline_budget_us) {
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

    #[test]
    fn verdict_copy_folds_in_numbers_and_state() {
        let ok = verdict_copy(0, 210, 603);
        assert!(ok[0].contains("met its deadline"));
        assert!(
            ok[1].contains("210") && ok[1].contains("34%"),
            "{:?}",
            ok[1]
        );

        let bad = verdict_copy(3, 6_300, 603);
        assert!(bad[0].contains("Overrun"));
        assert!(
            !bad[1].contains('%'),
            "the overrun copy names no percentage: {:?}",
            bad[1]
        );
    }

    /// A zero budget must not divide by zero; the copy degrades to 0 %.
    #[test]
    fn a_zero_budget_reads_as_zero_percent() {
        let v = verdict_copy(0, 210, 0);
        assert!(v[1].contains("0%"), "{:?}", v[1]);
    }

    /// The middle severities share their copy, and the ends do not.
    #[test]
    fn each_band_says_something_different() {
        assert_eq!(verdict_copy(1, 100, 600), verdict_copy(2, 100, 600));
        assert_ne!(verdict_copy(0, 100, 600), verdict_copy(1, 100, 600));
        assert_ne!(verdict_copy(2, 100, 600), verdict_copy(3, 100, 600));
    }
}
