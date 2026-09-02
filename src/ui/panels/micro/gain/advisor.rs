// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The headline: one sentence saying what to do about the gain.
//!
//! The reason this view exists. Everything above it is evidence; this is the
//! answer, and it is what someone setting gain on arrival reads first.

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::state::SdrMetrics;
use crate::ui::rf_calc::gain_advice;

use super::super::field::Field;

pub(super) fn draw(f: &mut Frame, area: Rect, state: &SdrMetrics, fd: &Field) {
    let theme = fd.theme;
    // With no stream there is no histogram to advise from, and guessing would be
    // worse than saying so.
    let (text, color) = if fd.stale {
        ("--- (RX not streaming)", theme.stale)
    } else {
        let (text, severity) = gain_advice(&state.iq.iq_amplitude_hist);
        let color = match severity {
            2 => theme.status_crit,
            1 => theme.status_warn,
            _ => theme.status_ok,
        };
        (text, color)
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(" "),
            Span::styled(
                text,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
        ])),
        area,
    );
}
