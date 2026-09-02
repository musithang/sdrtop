// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! CALLBACK TIMING: how regularly the driver hands blocks over.
//!
//! Two different scatter measures live here and they are not the same thing:
//! `Jitter` is the per-window rms of the callback period, while p95/p99/peak are
//! percentiles of the *absolute per-callback deviation*. The rms says how noisy
//! the stream is on average; the percentiles say how late a callback actually
//! got, which is what the deadline budget below is measured against.

use ratatui::text::{Line, Span};

use crate::state::SdrMetrics;
use crate::ui::widgets::timing_fmt::{fmt_us, ppm_span};

use super::rows::Rows;

pub(super) fn lines(state: &SdrMetrics, r: &Rows) -> Vec<Line<'static>> {
    let t = &state.timing;
    let theme = r.theme;
    // No measured period yet reads the same as stale: an "exp" comparison against
    // a period of zero is not a reading.
    let no_period = r.stale || t.cb_period_us == 0;

    let period = if no_period {
        Line::from(vec![r.field("Period"), r.dash()])
    } else {
        Line::from(vec![
            r.field("Period"),
            Span::styled(fmt_us(t.cb_period_us), r.val()),
            Span::styled(format!("   exp {}", fmt_us(t.cb_period_expected)), r.dim()),
        ])
    };
    let drift = if no_period {
        Line::from(vec![r.field("Host drift"), r.dash()])
    } else {
        Line::from(vec![
            r.field("Host drift"),
            ppm_span(t.cb_period_delta_ppm, theme),
        ])
    };

    vec![
        crate::ui::chrome::section("CALLBACK TIMING", "RX stream", r.iw, theme),
        period,
        drift,
        r.row(
            "Jitter",
            vec![Span::styled(
                format!("\u{00b1}{} \u{00b5}s rms", t.cb_jitter_us),
                r.val(),
            )],
        ),
        r.row(
            "",
            vec![Span::styled(
                format!(
                    "p95 {}  p99 {}  peak {} \u{00b5}s",
                    t.dev_p95_us, t.dev_p99_us, t.dev_peak_us
                ),
                r.val(),
            )],
        ),
        r.trend(
            "trend",
            &state
                .iq
                .jitter_history
                .iter()
                .map(|&v| v as f64)
                .collect::<Vec<f64>>(),
            "  60 s",
        ),
    ]
}
