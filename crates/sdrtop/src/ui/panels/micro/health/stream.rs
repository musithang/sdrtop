// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! What the stream is doing: drops, saturation, buffer fill - each with its 60 s
//! trend and a one-word verdict.
//!
//! Three readings of one pipeline, read top to bottom: drops are the symptom,
//! buffer fill is the leading indicator of them, and saturation is the unrelated
//! way the same stream can be wrong.

use ratatui::text::Line;

use crate::state::SdrMetrics;
use crate::ui::widgets::micro_common::{buf_color, drop_color, sat_color};

use super::super::field::Field;
use super::rows::{row, Row};

/// Buffer fill at which the queue is close enough to overflowing to warn.
const BUF_WARN_PCT: f32 = 80.0;
/// Saturation at which the reading stops being noise. The shared scale's warn
/// point; see `state::SAT_WARN_PCT`.
const SAT_OK_PCT: f32 = crate::state::SAT_WARN_PCT;

pub(super) fn lines(state: &SdrMetrics, fd: &Field) -> [Line<'static>; 3] {
    let theme = fd.theme;

    let drops = state.signal.drops_per_sec;
    let drop_hist: Vec<f64> = state
        .signal
        .drop_history
        .iter()
        .map(|&v| v as f64)
        .collect();

    let sat = state.signal.adc_saturation_pct;
    let sat_hist: Vec<f64> = state
        .signal
        .saturation_history
        .iter()
        .map(|&v| v as f64)
        .collect();

    let buf = state.iq.buf_fill_pct;
    let buf_hist: Vec<f64> = state
        .iq
        .buf_fill_history
        .iter()
        .map(|&v| v as f64)
        .collect();

    [
        row(
            Row {
                label: "DROP",
                value: Some(format!("{drops}/s")),
                hist: &drop_hist,
                color: drop_color(drops, theme),
                ok: drops == 0,
            },
            fd,
        ),
        row(
            Row {
                label: "SAT",
                value: Some(format!("{sat:.1}%")),
                hist: &sat_hist,
                color: sat_color(sat, theme),
                ok: sat < SAT_OK_PCT,
            },
            fd,
        ),
        row(
            Row {
                label: "BUF",
                value: Some(format!("{buf:.0}%")),
                hist: &buf_hist,
                color: buf_color(buf, theme),
                ok: buf < BUF_WARN_PCT,
            },
            fd,
        ),
    ]
}
