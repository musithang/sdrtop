// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The card's map of a view: its panels as boxes, where the deck would put
//! them, named as their frames name them.
//!
//! **Drawn from the placement, never from a picture.** The rects come from
//! `LayoutEngine::placement_of`, the arithmetic the deck itself draws by, on
//! the real screen size; this only scales them down. A map typed by hand
//! would be right the day it was typed (rule 7).

use ratatui::{buffer::Buffer, layout::Rect, style::Style};

/// Scale `placed` from a screen of `source` into `area` and draw each as a
/// box with its name. Edges are scaled, not sizes, so neighbours still meet.
pub fn render(
    buf: &mut Buffer,
    area: Rect,
    source: Rect,
    placed: &[(String, Rect)],
    theme: &crate::Theme,
) {
    if source.width == 0 || source.height == 0 || area.width < 8 || area.height < 3 {
        return;
    }
    let sx = |x: u16| {
        area.x + (u32::from(x - source.x) * u32::from(area.width) / u32::from(source.width)) as u16
    };
    let sy = |y: u16| {
        area.y
            + (u32::from(y - source.y) * u32::from(area.height) / u32::from(source.height)) as u16
    };
    let edge = Style::default().fg(theme.border_dim);
    let name_ink = Style::default().fg(theme.label);
    for (name, r) in placed {
        let r = r.intersection(source);
        if r.width == 0 || r.height == 0 {
            continue;
        }
        let (x0, x1) = (sx(r.x), sx(r.x + r.width));
        let (y0, y1) = (sy(r.y), sy(r.y + r.height));
        let (w, h) = (x1.saturating_sub(x0), y1.saturating_sub(y0));
        if w < 2 {
            continue;
        }
        let label: String = name.chars().take(w.saturating_sub(2) as usize).collect();
        let len = label.chars().count() as u16;
        let mid = x0 + (w - len) / 2;
        if h < 2 {
            // Too short for a box: a rule with the name on it.
            buf.set_string(x0, y0, "─".repeat(w as usize), edge);
            buf.set_string(mid, y0, &label, name_ink);
            continue;
        }
        let inner = w.saturating_sub(2) as usize;
        buf.set_string(x0, y0, format!("┌{}┐", "─".repeat(inner)), edge);
        for y in y0 + 1..y1 - 1 {
            buf.set_string(x0, y, "│", edge);
            buf.set_string(x1 - 1, y, "│", edge);
        }
        buf.set_string(x0, y1 - 1, format!("└{}┘", "─".repeat(inner)), edge);
        let row = if h > 2 { y0 + (h - 1) / 2 } else { y0 };
        buf.set_string(mid, row, &label, name_ink);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draw(placed: &[(String, Rect)], w: u16, h: u16) -> Vec<String> {
        let area = Rect::new(0, 0, w, h);
        let mut buf = Buffer::empty(area);
        render(
            &mut buf,
            area,
            Rect::new(0, 0, 190, 40),
            placed,
            &crate::Theme::sdr(),
        );
        (0..h)
            .map(|y| (0..w).map(|x| buf.get(x, y).symbol().to_string()).collect())
            .collect()
    }

    /// Two panels side by side on the deck are two boxes side by side on
    /// the map, each named, meeting without overlapping.
    #[test]
    fn neighbours_meet_and_are_named() {
        let placed = vec![
            ("constellation".to_string(), Rect::new(0, 0, 95, 40)),
            ("image scope".to_string(), Rect::new(95, 0, 95, 40)),
        ];
        let rows = draw(&placed, 40, 10);
        assert!(
            rows[0].starts_with("┌") && rows[0].contains("┐┌"),
            "{}",
            rows[0]
        );
        let all = rows.join("\n");
        assert!(
            all.contains("constellatio") && all.contains("image scope"),
            "{all}"
        );
    }

    /// A band too thin for a box at this scale becomes a named rule.
    #[test]
    fn a_thin_band_is_a_named_rule() {
        let placed = vec![("header".to_string(), Rect::new(0, 0, 190, 2))];
        let rows = draw(&placed, 40, 10);
        assert!(
            rows[0].contains("header") && rows[0].starts_with('─'),
            "{}",
            rows[0]
        );
    }

    /// Nothing to scale from, nothing drawn.
    #[test]
    fn an_empty_source_draws_nothing() {
        let area = Rect::new(0, 0, 20, 5);
        let mut buf = Buffer::empty(area);
        let placed = vec![("x".to_string(), Rect::new(0, 0, 5, 5))];
        render(
            &mut buf,
            area,
            Rect::default(),
            &placed,
            &crate::Theme::sdr(),
        );
        assert_eq!(buf, Buffer::empty(area));
    }
}
