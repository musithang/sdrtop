// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `PILOT / STEREO` - the 19 kHz tone that says a broadcast is in stereo, and
//! how hard it is being injected.

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::state::{PilotState, SdrMetrics};
use crate::ui::chrome;

use super::fmt::{fmt_hz, missing, no_reading, val};
use super::stack::Stack;

pub(super) fn lines(
    stack: &mut Stack<'static>,
    state: &SdrMetrics,
    iw: usize,
    theme: &crate::Theme,
) {
    stack.heading(chrome::section("PILOT / STEREO", "19 kHz", iw, theme));
    // One shape in every state: a pilot that comes and goes on a burst
    // changes the words, not the rows. Mono has no pilot to measure, so its
    // two figures are dashes rather than the noise at 19 kHz.
    let p = state.demod.live_pilot();
    match p {
        Some(p) => {
            let (mark, word, color) = match p.state {
                PilotState::Locked => ("\u{25cf}", "STEREO", theme.status_ok),
                PilotState::Marginal => ("\u{25d0}", "MARGINAL", theme.status_warn),
                PilotState::Absent => ("\u{25cb}", "MONO", theme.label),
            };
            stack.push(Line::from(vec![
                Span::raw(" "),
                Span::styled(
                    format!("{mark} {word}"),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
            ]));
        }
        None => stack.push(no_reading(theme)),
    }
    let p = p.filter(|p| p.state != PilotState::Absent);
    stack.detail(Line::from(vec![
        chrome::field("Pilot", 8, theme),
        p.map_or_else(
            || missing(theme),
            |p| Span::styled(fmt_hz(p.deviation_hz), val(theme)),
        ),
    ]));
    // Injection is the deviation restated against 75 kHz, so it is the one of
    // the pair that can go.
    stack.minor(Line::from(vec![
        chrome::field("Inject", 8, theme),
        p.map_or_else(
            || missing(theme),
            |p| Span::styled(format!("{:.1}%", p.injection_pct), val(theme)),
        ),
    ]));
}
