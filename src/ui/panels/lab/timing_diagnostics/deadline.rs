//! DEADLINE BUDGET: how close the worst callbacks came to the drop threshold.
//!
//! Three bars — p95, p99, peak — against the same budget, with the budget marker
//! at mid-bar. Reading up the three tells you whether lateness is the usual case
//! or one outlier, which is the difference between a machine that needs tuning
//! and one that hiccuped.

use ratatui::{
    style::Style,
    text::{Line, Span},
};

use crate::state::SdrMetrics;

use super::rows::{budget_bar, Rows};

/// Share of the window that may be over budget before it reads as a fault rather
/// than as noise: one in twenty.
const LATE_CRIT_RATIO: u32 = 20;

pub(super) fn lines(state: &SdrMetrics, r: &Rows) -> Vec<Line<'static>> {
    let t = &state.timing;
    let theme = r.theme;
    let budget = t.deadline_budget_us;

    let mut out = vec![crate::ui::chrome::section(
        "DEADLINE BUDGET",
        &format!("\u{250A} = \u{00b1}{} \u{00b5}s", budget),
        r.iw,
        theme,
    )];

    let bars = [
        ("p95", t.dev_p95_us),
        ("p99", t.dev_p99_us),
        ("peak", t.dev_peak_us),
    ];
    for (i, (name, v)) in bars.iter().enumerate() {
        let value_str = format!("{} \u{00b5}s", v);
        // lead(1) + label(4) + gap(1) + bar + gap(1) + value
        let bar_w =
            r.iw.saturating_sub(1 + 4 + 1 + 1 + value_str.chars().count())
                .max(6);
        let mut spans = vec![Span::styled(format!(" {name:<4} "), r.lbl())];
        if r.stale {
            spans.push(r.dash());
        } else {
            spans.extend(budget_bar(*v, budget, bar_w, theme));
            spans.push(Span::styled(format!(" {value_str}"), r.val()));
        }
        out.push(Line::from(spans));
        // Breathing row between the bars so they never read as one block.
        if i < bars.len() - 1 {
            out.push(Line::raw(""));
        }
    }

    out.push(Line::from(if r.stale {
        vec![r.field("late"), r.dash()]
    } else if t.late_callbacks == 0 {
        vec![
            r.field("late"),
            Span::styled(
                "\u{2713} none over budget".to_string(),
                Style::default().fg(theme.status_ok),
            ),
        ]
    } else {
        let color = if t.late_callbacks * LATE_CRIT_RATIO > t.late_window {
            theme.status_crit
        } else {
            theme.status_warn
        };
        vec![
            r.field("late"),
            Span::styled(
                format!("{} / {} over budget", t.late_callbacks, t.late_window),
                Style::default().fg(color),
            ),
        ]
    }));
    out
}
