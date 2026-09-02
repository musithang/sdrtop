// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The text boxes drawn *over* the canvas: a vector-analyser read-out top-right,
//! the density legend bottom-left, the measured centroid bottom-right, and the
//! caption along the foot.
//!
//! Each box is its own `Paragraph` over its own `Rect`, so its cells overwrite the
//! cloud underneath and stay legible. Every one of them has a size gate: a box
//! that cannot hold its own longest line is dropped rather than clipped, because a
//! half-printed number reads as a measurement.

use ratatui::{
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use super::cloud::{HEAT, HEAT_LEVELS};
use super::fit::CloudStats;

/// Bottom caption - echoes the image scope's own framing, so the two Lab IQ
/// panels read as one bench.
const CAPTION: &str =
    "image mirrors the carrier about the LO \u{00b7} DC offset \u{2192} centre spike";

pub(super) fn draw(f: &mut Frame, inner: Rect, stats: &CloudStats, theme: &crate::Theme) {
    stats_box(f, inner, stats, theme);
    foot(f, inner, stats, theme);
}

/// EVM / MER / σ / n / fit, top-right.
fn stats_box(f: &mut Frame, inner: Rect, stats: &CloudStats, theme: &crate::Theme) {
    if inner.width < 26 || inner.height < 8 {
        return;
    }

    let val = theme.value_hi;
    let bold = Style::default().fg(val).add_modifier(Modifier::BOLD);
    let dimst = Style::default().fg(theme.border_dim);
    let labst = Style::default().fg(theme.label);

    let mut sl: Vec<Line> = Vec::new();
    if let (Some(r), Some(p)) = (stats.evm_rms, stats.evm_pk) {
        sl.push(Line::from(vec![
            Span::styled("EVM ", labst),
            Span::styled(format!("{:.1}% ", r * 100.0), bold),
            Span::styled("rms · ", dimst),
            Span::styled(format!("{:.0}% ", p * 100.0), Style::default().fg(val)),
            Span::styled("pk", dimst),
        ]));
    }
    if let Some(m) = stats.mer_db {
        sl.push(Line::from(vec![
            Span::styled("MER ", labst),
            Span::styled(format!("{m:.1} dB"), bold),
        ]));
    }
    sl.push(Line::from(vec![
        Span::styled("σ ", labst),
        Span::styled(format!("{:.2}", stats.sigma), Style::default().fg(val)),
        Span::styled(" · n ", dimst),
        Span::styled(format!("{}", stats.n), Style::default().fg(val)),
    ]));
    if let (Some(e), Some(t)) = (stats.ecc, stats.tilt_deg) {
        sl.push(Line::from(vec![
            Span::styled("fit ecc ", dimst),
            Span::styled(format!("{e:.3}"), Style::default().fg(val)),
            Span::styled(" · tilt ", dimst),
            Span::styled(format!("{t:+.1}\u{b0}"), Style::default().fg(val)),
        ]));
    }
    // Wide enough for the longest line ("fit ecc 1.002 · tilt +0.2°" = 26);
    // a narrower box right-clips the leading "fit" off that line.
    let w = 28u16.min(inner.width);
    let h = (sl.len() as u16).min(inner.height);
    let rect = Rect {
        x: inner.x + inner.width - w,
        y: inner.y,
        width: w,
        height: h,
    };
    f.render_widget(Paragraph::new(sl).alignment(Alignment::Right), rect);
}

/// The legend, the centroid and the caption - the bottom two or three rows.
fn foot(f: &mut Frame, inner: Rect, stats: &CloudStats, theme: &crate::Theme) {
    if inner.width < 24 || inner.height < 6 {
        return;
    }

    let dimst = Style::default().fg(theme.border_dim);
    let labst = Style::default().fg(theme.label);
    let dc_color = theme.status_warn;
    let ellipse_color = theme.border_focused;

    // Caption is decorative - only draw it when the whole line fits, otherwise
    // a centred truncation chops both ends into noise.
    let cap_h: u16 = if inner.height >= 7 && inner.width as usize >= CAPTION.chars().count() {
        1
    } else {
        0
    };
    let row_y = inner.y + inner.height - 2 - cap_h;

    // Centroid carries live numbers, so it gets the width it needs first; the
    // legend tiles into whatever is left and is dropped when too tight to read
    // (its widest line is "rms fit  ⊕ centroid" = 21 cells). This stops the
    // right-aligned centroid from left-clipping its own "I …" value.
    let cen = vec![
        Line::from(vec![
            Span::styled("\u{2295} ", Style::default().fg(dc_color)),
            Span::styled("centroid", dimst),
        ]),
        Line::from(Span::styled(
            format!("I {:+.4} \u{b7} Q {:+.4}", stats.cx, stats.cy),
            labst,
        )),
    ];
    let cw = 22u16.min(inner.width);
    let cen_rect = Rect {
        x: inner.x + inner.width - cw,
        y: row_y,
        width: cw,
        height: 2,
    };
    f.render_widget(Paragraph::new(cen).alignment(Alignment::Right), cen_rect);

    let leg_w = inner.width.saturating_sub(cw);
    if leg_w >= 21 {
        let leg = vec![
            Line::from(vec![
                Span::styled("\u{28ff} ", Style::default().fg(HEAT[HEAT_LEVELS - 1])),
                Span::styled("dense  ", dimst),
                Span::styled("\u{2802} ", Style::default().fg(HEAT[0])),
                Span::styled("sparse", dimst),
            ]),
            Line::from(vec![
                Span::styled("\u{25ef} ", Style::default().fg(ellipse_color)),
                Span::styled("rms fit  ", dimst),
                Span::styled("\u{2295} ", Style::default().fg(dc_color)),
                Span::styled("centroid", dimst),
            ]),
        ];
        let leg_rect = Rect {
            x: inner.x,
            y: row_y,
            width: leg_w,
            height: 2,
        };
        f.render_widget(Paragraph::new(leg), leg_rect);
    }

    // Caption, full-width bottom row - echoes the scope's framing.
    if cap_h == 1 {
        let cap = Line::from(Span::styled(CAPTION, dimst));
        let rect = Rect {
            x: inner.x,
            y: inner.y + inner.height - 1,
            width: inner.width,
            height: 1,
        };
        f.render_widget(Paragraph::new(cap).alignment(Alignment::Center), rect);
    }
}
