// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! ARRIVAL: how regularly the driver hands blocks over.
//!
//! The block period against the one the rate implies, and its jitter with a
//! trend. No ppm here: the period's mean over one poll window is the same
//! question the sample clock answers over a long baseline, and the two used
//! to sit one zone apart as `Host drift +2136 ppm` and `SR drift +321 ppm`.
//! The clock is [`super::sample_rate`]'s to state.

use ratatui::text::{Line, Span};

use crate::state::SdrMetrics;
use crate::ui::widgets::timing_fmt::fmt_us;

use super::rows::Rows;

pub(super) fn lines(state: &SdrMetrics, r: &Rows) -> Vec<Line<'static>> {
    let t = &state.timing;
    let theme = r.theme;
    // No measured period yet reads the same as stale: a comparison against a
    // period of zero is not a reading.
    let no_period = r.stale || t.cb_period_us == 0;
    let block = if no_period {
        Line::from(vec![r.field("Block"), r.dash()])
    } else {
        Line::from(vec![
            r.field("Block"),
            Span::styled(fmt_us(t.cb_period_us), r.val()),
            Span::styled(
                format!("   expected {}", fmt_us(t.cb_period_expected)),
                r.dim(),
            ),
        ])
    };
    vec![
        crate::ui::chrome::section("Arrival", "a block every", r.iw, theme),
        block,
        r.row(
            "Jitter",
            vec![Span::styled(
                format!("\u{00b1}{} rms", fmt_us(t.cb_jitter_us)),
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
