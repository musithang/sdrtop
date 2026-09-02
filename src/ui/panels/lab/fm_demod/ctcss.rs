// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `CTCSS` - the subaudible tone a narrow-band FM repeater is squelched on.
//!
//! The only section with three empty states rather than one: a tone, a search
//! still filling its window, and a channel that carries no tone at all. They
//! look identical unless they are said differently, and only one is a finding.

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::state::SdrMetrics;
use crate::ui::chrome;

use super::fmt::{fmt_hz, lbl, val};
use super::stack::Stack;

pub(super) fn lines(
    stack: &mut Stack<'static>,
    state: &SdrMetrics,
    stale: bool,
    iw: usize,
    theme: &crate::Theme,
) {
    stack.heading(chrome::section("CTCSS", "subaudible", iw, theme));
    match state.demod.live_ctcss() {
        Some(t) => {
            stack.push(Line::from(vec![
                Span::raw(" "),
                Span::styled(
                    format!("\u{25cf} {:.1} Hz", t.tone_hz),
                    Style::default()
                        .fg(theme.status_ok)
                        .add_modifier(Modifier::BOLD),
                ),
            ]));
            stack.detail(Line::from(vec![
                chrome::field("Dev", 8, theme),
                Span::styled(fmt_hz(t.deviation_hz), val(theme)),
            ]));
            stack.minor(Line::from(vec![
                chrome::field("Margin", 8, theme),
                Span::styled(format!("{:.0} dB", t.margin_db), val(theme)),
            ]));
        }
        // "Still filling the window" and "there is no tone" look the
        // same on screen unless they are said differently - and only
        // one of them is a finding.
        None if state.demod.ctcss_searching() => {
            stack.push(Line::from(vec![
                Span::raw(" "),
                Span::styled(
                    format!("\u{25cc} SEARCHING {:.0}%", state.demod.ctcss_fill * 100.0),
                    lbl(theme),
                ),
            ]));
        }
        None if !stale => {
            stack.push(Line::from(vec![
                Span::raw(" "),
                Span::styled("\u{25cb} NO TONE", lbl(theme)),
            ]));
        }
        None => stack.gap(),
    }
}
