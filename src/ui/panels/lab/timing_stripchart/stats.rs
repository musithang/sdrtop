// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The inset stats line: the numbers the chart cannot show.
//!
//! The plot says *when* callbacks were late; this row says how often and how
//! badly, and closes with the same `TimingQuality` verdict `timing_diagnostics`
//! prints - from `quality_color`, so the two cannot grade the stream differently.

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::state::TimingState;
use crate::ui::widgets::timing_fmt::{fmt_us, quality_color};

pub(super) fn line(t: &TimingState, stale: bool, theme: &crate::Theme) -> Line<'static> {
    if stale {
        return Line::from(vec![
            Span::raw(" "),
            Span::styled("RX stopped", Style::default().fg(theme.stale)),
        ]);
    }

    let lbl = Style::default().fg(theme.label);
    let q = t.timing_quality;
    let mark = if q.severity() == 0 {
        "\u{2713}"
    } else {
        "\u{26a0}"
    };
    Line::from(vec![
        Span::raw(" "),
        Span::styled(
            format!("jitter \u{00b1}{} \u{00b5}s", t.cb_jitter_us),
            Style::default().fg(theme.value),
        ),
        Span::styled("   worst ", lbl),
        Span::styled(fmt_us(t.dev_peak_us), Style::default().fg(theme.value)),
        Span::styled("   over budget ", lbl),
        Span::styled(
            format!("{} / {}", t.late_callbacks, t.late_window),
            Style::default().fg(if t.late_callbacks == 0 {
                theme.status_ok
            } else {
                theme.status_warn
            }),
        ),
        Span::styled("   ", lbl),
        Span::styled(
            format!("{mark} {}", q.label()),
            Style::default()
                .fg(quality_color(q, theme))
                .add_modifier(Modifier::BOLD),
        ),
    ])
}
