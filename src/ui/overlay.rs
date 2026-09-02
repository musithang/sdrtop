// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    widgets::Clear,
    Frame,
};

use crate::state::SdrMetrics;

/// The Command Rail's full-log overlay (`L` in rail-focus): a centred, themed
/// panel showing the whole scrollback. Reuses the standard `log::render` so it
/// reads identically to the docked log, just larger and floating over the deck.
pub fn render_log(f: &mut Frame, m: &SdrMetrics, theme: &crate::Theme) {
    let full = f.size();
    let w = ((full.width as u32 * 7 / 10) as u16)
        .max(24)
        .min(full.width);
    let h = ((full.height as u32 * 7 / 10) as u16)
        .max(7)
        .min(full.height);
    let area = centered_rect(w, h, full);
    f.render_widget(Clear, area);
    crate::ui::panels::core::log::render(f, area, m, theme);
}

/// A `width` by `height` box in the middle of `r`. Shared with `ui::menu`,
/// which floats over the deck the same way the log overlay does.
pub(crate) fn centered_rect(width: u16, height: u16, r: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(r.height.saturating_sub(height) / 2),
            Constraint::Length(height),
            Constraint::Min(0),
        ])
        .split(r);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(r.width.saturating_sub(width) / 2),
            Constraint::Length(width),
            Constraint::Min(0),
        ])
        .split(vertical[1])[1]
}
