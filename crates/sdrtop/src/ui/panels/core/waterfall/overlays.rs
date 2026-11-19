// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Text drawn over the coloured grid: band names along the top, elapsed-time
//! ticks down the right edge.
//!
//! Both reset the background explicitly. A default `Style` leaves `bg` as
//! `None`, which lets the waterfall's colours show through the glyphs and makes
//! the text unreadable; resetting to the panel's black turns each label into an
//! engraved tab instead.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::Span,
    widgets::Paragraph,
    Frame,
};

use crate::ui::widgets::band_plan::BAND_PLAN;

/// Band names along the top row, one per band overlapping the view, skipping any
/// that would collide with the one before it.
///
/// Not drawn when bonded: the spectrum above already carries the band plan, and
/// showing it twice is noise.
pub(super) fn band_plan(f: &mut Frame, area: Rect, left_hz: f64, bw: f64, theme: &crate::Theme) {
    if area.height < 2 || area.width <= 4 {
        return;
    }
    let cw = area.width as f64;
    let right_hz = left_hz + bw;
    let mut next_free_col: i32 = -1;

    for &(band_s, band_e, label) in BAND_PLAN {
        let (bs, be) = (band_s as f64, band_e as f64);
        if bs >= right_hz || be <= left_hz {
            continue;
        }
        let center = (bs.max(left_hz) + be.min(right_hz)) / 2.0;
        let lw = label.len() as u16;
        let col = ((((center - left_hz) / bw) * cw) as u16).min(area.width.saturating_sub(lw));
        if (col as i32) < next_free_col {
            continue;
        }
        next_free_col = col as i32 + lw as i32 + 1;
        f.render_widget(
            Paragraph::new(Span::styled(
                label,
                Style::default().fg(theme.label).bg(Color::Reset),
            )),
            Rect {
                x: area.x + col,
                y: area.y,
                width: lw,
                height: 1,
            },
        );
    }
}

/// Elapsed-time ticks down the right edge: `╴12s`.
///
/// The newest row is at the top (≈ now), and each tick reads the *real*
/// timestamp of the row on that screen line, so the labels stay honest when the
/// stride changes or the buffer runs short. Kept on the right so the dB legend
/// on the left and the frequency span shared with the spectrum are untouched.
pub(super) fn time_axis(
    f: &mut Frame,
    area: Rect,
    rows: &VecDeque<(Instant, Arc<Vec<f32>>)>,
    skip_data: usize,
    theme: &crate::Theme,
) {
    if area.height < 6 || area.width <= 12 {
        return;
    }
    let h = area.height as usize;
    for k in 1..=4 {
        let r = (h - 1) * k / 4; // screen row, skipping the top "now" row
        let data_idx = skip_data + r * 2; // the top sub-row of that character cell
        let Some((ts, _)) = rows.get(data_idx) else {
            continue;
        };
        let label = format!("\u{2574}{}s", ts.elapsed().as_secs());
        let lw = label.chars().count() as u16;
        f.render_widget(
            Paragraph::new(Span::styled(
                label,
                Style::default()
                    .fg(theme.value_hi)
                    .bg(Color::Reset)
                    .add_modifier(Modifier::BOLD),
            )),
            Rect {
                x: area.x + area.width - lw,
                y: area.y + r as u16,
                width: lw,
                height: 1,
            },
        );
    }
}
