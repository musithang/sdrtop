// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The tree's colour roles on the theme: the one place a [`Tone`] becomes a
//! colour, so the crumb, the columns and the card cannot paint one row two
//! ways.

use ratatui::style::Color;

use super::tree::Tone;

pub fn of(tone: Tone, theme: &crate::Theme) -> Color {
    match tone {
        Tone::Spectrum | Tone::Radio => theme.border_accent,
        Tone::Sweep => theme.peak_hold,
        Tone::Receiver => theme.status_ok,
        Tone::Ble => theme.net_ble,
        Tone::Classic => theme.net_bt,
        Tone::Coded => theme.series_color(4),
        Tone::Wifi => theme.net_wifi,
        Tone::Quiet => theme.label,
    }
}

/// A colour at reduced strength: pulled toward the quiet border colour, so
/// it reads as the same hue at rest without the menu assuming what the
/// terminal's background is.
pub fn rest(color: Color, theme: &crate::Theme) -> Color {
    crate::ui::widgets::charts::lerp_color(color, theme.border_dim, 0.5)
}
