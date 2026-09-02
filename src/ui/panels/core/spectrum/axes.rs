// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The three rows and one column around the canvas: the frequency ruler under
//! it, the tuning handle below that, and the dBFS gutter down its left side.

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use super::scale::{fmt_spectrum_step, freq_scale_spans};
use super::view::SpectrumView;

/// The frequency ruler: `┬` ticks with MHz labels at each quarter of the view.
///
/// Omitted when bonded below the waterfall, whose top border carries the same
/// scale as the shared ruler and reclaims this row for the plot.
pub(super) fn frequency(
    f: &mut Frame,
    area: Rect,
    canvas_width: u16,
    view: &SpectrumView,
    border: Color,
    theme: &crate::Theme,
) {
    let spans = freq_scale_spans(
        view.left_hz,
        view.bw,
        canvas_width as usize,
        border,
        theme.value,
        ' ',
    );
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// The tuning handle, shown in focus mode: `───◀ 92.800 MHz ▶───  step 25 kHz`.
///
/// The handle is centred in the panel, not in the dashes. The trailing readout
/// sits to the right of it, so the left arm has to balance the right arm *plus*
/// that readout, or the handle visibly drifts left of centre.
pub(super) fn tuning(
    f: &mut Frame,
    area: Rect,
    freq_hz: u64,
    step_hz: u64,
    cursor: Option<(f64, f32)>,
    theme: &crate::Theme,
) {
    let step_str = fmt_spectrum_step(step_hz);
    let freq_str = format!("  {:.3} MHz  ", freq_hz as f64 / 1_000_000.0);
    let readout = match cursor {
        Some((mhz, pwr)) => format!("  cur: {mhz:.3} MHz  {pwr:.1} dBFS  step {step_str}  J/K"),
        None => format!("  step {step_str}  [/]"),
    };

    let handle_w = 2 + freq_str.chars().count();
    let readout_w = readout.chars().count();
    let dashes = (area.width as usize).saturating_sub(handle_w + readout_w);
    let left_arm = ((area.width as usize).saturating_sub(handle_w) / 2).min(dashes);
    let right_arm = dashes - left_arm;

    let arrow = Style::default()
        .fg(theme.border_accent)
        .add_modifier(Modifier::BOLD);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "\u{2500}".repeat(left_arm),
                Style::default().fg(theme.border_dim),
            ),
            Span::styled("\u{25C0}", arrow),
            Span::styled(
                freq_str,
                Style::default()
                    .fg(theme.value_hi)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("\u{25B6}", arrow),
            Span::styled(
                "\u{2500}".repeat(right_arm),
                Style::default().fg(theme.border_dim),
            ),
            Span::styled(readout, Style::default().fg(theme.label)),
        ])),
        area,
    );
}

/// The dBFS gutter down the left edge, tracking the current y-zoom.
///
/// The right edge of the gutter is a vertical rule `│` that becomes a tick `┤`
/// at each labelled value, so the scale reads like a ruled instrument axis
/// rather than a plain border.
pub(super) fn db_gutter(
    f: &mut Frame,
    area: Rect,
    y_min: f32,
    y_max: f32,
    border: Color,
    theme: &crate::Theme,
) {
    let h = area.height as usize;
    if h == 0 {
        return;
    }
    let mut row_label: Vec<Option<String>> = vec![None; h];
    for i in 0..=4 {
        let frac = i as f32 / 4.0;
        let db = y_max - (y_max - y_min) * frac;
        let row = (frac * h.saturating_sub(1) as f32).round() as usize;
        row_label[row.min(h - 1)] = Some(format!("{db:>5.0}"));
    }
    let lines: Vec<Line> = row_label
        .iter()
        .map(|label| {
            let (lbl, edge) = match label {
                Some(s) => (s.clone(), "\u{2524}"),  // ┤
                None => (" ".repeat(5), "\u{2502}"), // │
            };
            Line::from(vec![
                Span::styled(lbl, Style::default().fg(theme.value)),
                Span::styled(edge, Style::default().fg(border)),
            ])
        })
        .collect();
    f.render_widget(Paragraph::new(lines), area);
}
