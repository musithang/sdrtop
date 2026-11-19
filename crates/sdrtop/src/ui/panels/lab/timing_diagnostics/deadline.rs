// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! LATE BY: how late the worst callbacks came, against the radio's slack.
//!
//! Three bars - p95, p99, peak - on one log scale with the budget marked, each
//! with how late it was and how many budgets that is. Reading up the three
//! tells you whether lateness is the usual case or one outlier, which is the
//! difference between a machine that needs tuning and one that hiccuped.
//!
//! **The budget is the radio's slack**, named on the nameplate: a slice of one
//! block where the callback holds the USB transfer (libhackrf, librtlsdr),
//! the queue's depth less one block where the driver queues blocks before the
//! callback (libairspy's eight). Against a slice of one block, an R2 whose
//! callbacks came 2 ms late read 109 of 160 late with nothing lost.

use ratatui::{
    style::Style,
    text::{Line, Span},
};

use crate::state::SdrMetrics;

use super::rows::{late_bar, late_col, Rows};

/// Beyond this share of the window over budget, the count reads critical.
const LATE_CRIT_RATIO: u32 = 20;

/// Columns the value after a bar takes: `" 2.330 ms 7.7×"`.
const VALUE_W: usize = 14;

fn short(us: u64) -> String {
    if us >= 1_000 {
        format!("{:.1} ms", us as f64 / 1_000.0)
    } else {
        format!("{us} \u{00b5}s")
    }
}

pub(super) fn lines(state: &SdrMetrics, r: &Rows) -> Vec<Line<'static>> {
    let t = &state.timing;
    let theme = r.theme;
    let budget = t.deadline_budget_us;
    let why = match state.caps.driver_queue_blocks {
        Some(q) => format!("{q}-block queue"),
        None => "a slice of a block".to_string(),
    };
    let mut out = vec![crate::ui::chrome::section(
        "Late by",
        &format!("\u{250A} = {}, {why}", short(budget)),
        r.iw,
        theme,
    )];
    // lead(1) + label(5) + gap(1) + bar + value
    let bar_w = r.iw.saturating_sub(1 + 5 + 1 + VALUE_W).max(10);
    for (name, v) in [
        ("p95", t.dev_p95_us),
        ("p99", t.dev_p99_us),
        ("peak", t.dev_peak_us),
    ] {
        let mut spans = vec![Span::styled(format!(" {name:<5} "), r.lbl())];
        if r.stale {
            spans.push(r.dash());
        } else {
            let (bar, col) = late_bar(v, budget, bar_w, theme);
            spans.extend(bar);
            let ratio = v as f64 / budget.max(1) as f64;
            spans.push(Span::styled(
                if ratio >= 1.0 {
                    format!(" {} {ratio:.1}\u{00d7}", short(v))
                } else {
                    format!(" {} {:.0} %", short(v), ratio * 100.0)
                },
                Style::default().fg(col),
            ));
        }
        out.push(Line::from(spans));
    }
    // The scale under the bars: 1 %, 10 %, the budget, 10x.
    let mut axis = vec![' '; bar_w];
    for (ratio, label) in [
        (0.01, "1%"),
        (0.1, "10%"),
        (1.0, "1\u{00d7}"),
        (10.0, "10\u{00d7}"),
    ] {
        let at = late_col(ratio, bar_w)
            .saturating_sub(label.chars().count() / 2)
            .min(bar_w.saturating_sub(label.chars().count()));
        for (k, c) in label.chars().enumerate() {
            axis[at + k] = c;
        }
    }
    out.push(Line::from(vec![
        Span::raw(" ".repeat(7)),
        Span::styled(axis.into_iter().collect::<String>(), r.dim()),
    ]));
    out.push(Line::from(if r.stale {
        vec![r.field("Late"), r.dash()]
    } else if t.late_callbacks == 0 {
        vec![
            r.field("Late"),
            Span::styled(
                "\u{2713} none past the budget".to_string(),
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
            r.field("Late"),
            Span::styled(
                format!("{} of {} past the budget", t.late_callbacks, t.late_window),
                Style::default().fg(color),
            ),
        ]
    }));
    out
}
