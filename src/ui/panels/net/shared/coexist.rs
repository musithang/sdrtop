// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `NetCoexistPanel` - what is stepping on your link, and when.
//!
//! The band across, time running down from now at the top, and each cell
//! coloured by how busy that megahertz was at that moment: the waterfall's
//! orientation, so the occupancy profile above it and this history below it
//! read as one instrument with one frequency ruler (net-ux-polish-plan Stop 3).
//! Design section 9.1 calls it the single most immediately legible thing either
//! arc produces, and section 8's acceptance criterion for it is not a test:
//! **readable at two metres.**
//!
//! **Frequency through the band axis, never its own.** A column covers the
//! cells `band_axis::cells_of` says, the same cells the occupancy profile's
//! column above it covers, because a shared ruler is only honest if a column
//! means the same megahertz in both. It was frequency down the side until
//! 2026-09-19, which gave the two panels no axis in common.
//!
//! **What it is fed on, and what the plan said it would be fed on.** The plan
//! has this panel drawing bursts from N14, colour-coded by protocol. N14 landed
//! per-cell duty cycle and no burst detector - "no demodulation anywhere" was
//! its own instruction - so there are no bursts to draw and there will be none
//! until an arc lands one. What there is instead is real and is the same
//! picture at a coarser grain: the occupancy history, half a second to a row
//! half. Colour carries the duty cycle; decoded packets are marked over it in
//! Stop 3.3.b.

use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use super::band_axis;
use crate::state::{SdrMetrics, COLUMN_INTERVAL};
use crate::ui::panel::{FeedSpan, Panel, PanelChrome, Staleness};
use crate::ui::widgets::canvas::{fold, row, Duty};

pub struct NetCoexistPanel;

/// Rows reserved under the canvas: the channel ruler and the band's edges with
/// the time the canvas spans.
const AXIS_ROWS: u16 = 2;

/// The canvas: `rows` character rows, two moments each (a half block's upper
/// and lower halves), newest at the top, each moment folded into `width`
/// columns by the band axis. A moment older than the history holds is `None`
/// throughout, drawn as unlooked-at rather than as a quiet band.
fn canvas(history: &[Vec<f32>], rows: usize, width: usize) -> Vec<Vec<Duty>> {
    (0..rows * 2)
        .map(|step| {
            history
                .len()
                .checked_sub(1 + step)
                .map(|i| fold(&history[i], width))
                .unwrap_or_else(|| vec![None; width])
        })
        .collect()
}

/// `2400 MHz   now at the top, 12 s down   2483 MHz`: the band's edges, and
/// how far back the bottom of the canvas reaches, when it fits between them.
fn edges_and_time(width: usize, span_s: f64) -> String {
    let edges = band_axis::edges(width);
    let note = format!("now at the top, {span_s:.0} s down");
    let (left, right) = ("2400 MHz".len(), "2483 MHz".len());
    if left + right + note.len() + 4 > width {
        return edges;
    }
    let start = left + (width - left - right - note.len()) / 2;
    let mut chars: Vec<char> = edges.chars().collect();
    for (i, c) in note.chars().enumerate() {
        chars[start + i] = c;
    }
    chars.into_iter().collect()
}

impl Panel for NetCoexistPanel {
    fn name(&self) -> &'static str {
        "net_coexist"
    }

    fn min_size(&self) -> (u16, u16) {
        (30, 8)
    }

    fn chrome(&self, state: &SdrMetrics) -> PanelChrome {
        PanelChrome::new("Coexistence")
            .stale_when(Staleness::NotStreaming)
            .tag_if(true, state.net.mode.tag())
            // The canvas holds the band's last HISTORY_COLUMNS moments, one
            // every COLUMN_INTERVAL: a drop inside that stretch thins a moment.
            .counts_from_feed(FeedSpan::Window(
                crate::state::COLUMN_INTERVAL * crate::state::HISTORY_COLUMNS as u32,
            ))
    }

    fn render(
        &self,
        f: &mut Frame,
        inner: Rect,
        state: &SdrMetrics,
        theme: &crate::Theme,
        _focused: bool,
    ) {
        if inner.width == 0 || inner.height <= AXIS_ROWS {
            return;
        }
        let width = inner.width as usize;
        let rows = (inner.height - AXIS_ROWS) as usize;
        let history: Vec<Vec<f32>> = state.net.band.history.iter().cloned().collect();

        if history.is_empty() {
            // A pass that will never come is not a pass to wait for. The same
            // distinction the occupancy profile makes, for the same reason.
            let said = match &state.net.survey_refused {
                Some(why) => format!("no pass is possible: {why}"),
                None => "waiting for the first pass".to_string(),
            };
            f.render_widget(
                Paragraph::new(vec![Line::from(Span::styled(
                    said,
                    Style::default().fg(theme.stale),
                ))]),
                inner,
            );
            return;
        }

        let moments = canvas(&history, rows, width);
        let mut lines: Vec<Line<'static>> = moments
            .chunks(2)
            .map(|pair| row(&pair[0], &pair[1], theme))
            .collect();

        // How far back the bottom of the canvas reaches: the moments it holds,
        // not the ones it has room for, so a short history says it is short.
        let shown = history.len().min(rows * 2);
        let span_s = shown as f64 * COLUMN_INTERVAL.as_secs_f64();
        let dim = Style::default().fg(theme.label);
        lines.push(Line::from(Span::styled(band_axis::ruler(width), dim)));
        lines.push(Line::from(Span::styled(edges_and_time(width, span_s), dim)));
        f.render_widget(Paragraph::new(lines), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::net::occupancy;
    use crate::state::{fixture::draw, CellReading};
    use crate::ui::widgets::canvas::ink;
    use ratatui::{backend::TestBackend, Terminal};

    /// One moment of the band with `busy` set on exactly `cell`.
    fn column(cell: usize) -> Vec<f32> {
        let mut c = vec![0.0f32; occupancy::CELLS];
        c[cell] = 1.0;
        c
    }

    /// `columns` oldest first, the way the history holds them.
    fn with(columns: Vec<Vec<f32>>) -> SdrMetrics {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.band.cells = vec![CellReading::default(); occupancy::CELLS];
        m.net.band.history = columns.into();
        m
    }

    /// Render through a real buffer and return the styled cells, so a test can
    /// ask about **colour** - which is the whole reading on this panel and is
    /// invisible in the text `state::fixture::draw` returns.
    fn cells(m: &SdrMetrics, w: u16, h: u16) -> ratatui::buffer::Buffer {
        let theme = crate::Theme::sdr();
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal
            .draw(|f| NetCoexistPanel.render(f, f.size(), m, &theme, false))
            .unwrap();
        terminal.backend().buffer().clone()
    }

    /// **A burst at a known frequency and moment lands in the known cell**, at
    /// three sizes: the column the band axis gives its megahertz, the upper
    /// half of the top row for the newest moment and the lower half for the
    /// one before. Catches an axis drawn upside down, off by a row, or mapped
    /// differently from the occupancy profile above it.
    #[test]
    fn a_busy_cell_lands_where_the_band_axis_says_it_should() {
        let theme = crate::Theme::sdr();
        let busy = ink(Some(1.0), &theme);
        for (w, h) in [(40u16, 12u16), (90, 24), (120, 45)] {
            for cell in [0usize, occupancy::CELLS / 2, occupancy::CELLS - 1] {
                let other = (cell + 30) % occupancy::CELLS;
                // Oldest first: `other` a moment ago, `cell` now.
                let buf = cells(&with(vec![column(other), column(cell)]), w, h);
                let x = band_axis::column_of(cell, w as usize) as u16;
                assert_eq!(buf.get(x, 0).style().fg, Some(busy), "{w}x{h}: now, {cell}");
                let x = band_axis::column_of(other, w as usize) as u16;
                assert_eq!(
                    buf.get(x, 0).style().bg,
                    Some(busy),
                    "{w}x{h}: before, {other}"
                );
            }
        }
    }

    /// The ruler and the band's edges are the band axis's, and the time the
    /// canvas reaches back is said in seconds.
    #[test]
    fn the_axis_is_the_band_axis_and_the_time_is_said() {
        let m = with(vec![column(0); 20]);
        let lines = draw(NetCoexistPanel, 90, 24, &m);
        let text = lines.join("\n");
        assert!(text.contains(&band_axis::ruler(88)), "{text}");
        assert!(
            text.contains("2400 MHz") && text.contains("2483 MHz"),
            "{text}"
        );
        // Twenty moments at half a second each.
        assert!(text.contains("now at the top, 10 s down"), "{text}");
    }

    /// Every colour is the theme's.
    #[test]
    fn the_colours_come_from_the_theme() {
        let theme = crate::Theme::sdr();
        let allowed: Vec<_> = (0..=20)
            .map(|i| theme.palette_color(i as f32 / 20.0))
            .chain([theme.border_dim, theme.label, theme.stale])
            .collect();
        let buf = cells(&with(vec![column(10), column(40), column(70)]), 60, 20);
        for y in 0..20u16 {
            for x in 0..60 {
                let style = buf.get(x, y).style();
                for c in [style.fg, style.bg] {
                    if let Some(c) = c.filter(|c| *c != ratatui::style::Color::Reset) {
                        assert!(allowed.contains(&c), "{c:?} at {x},{y} is not the theme's");
                    }
                }
            }
        }
    }

    /// Before the first pass there is nothing to draw, and the panel says so
    /// rather than showing an empty grid that reads as a silent band.
    #[test]
    fn an_empty_history_says_it_is_waiting() {
        let out = draw(NetCoexistPanel, 60, 20, &SdrMetrics::fixture().streaming()).join("\n");
        assert!(out.contains("waiting for the first pass"), "{out}");
    }

    /// A pass that will never come is not a pass to wait for.
    #[test]
    fn a_survey_that_cannot_run_is_not_a_pass_to_wait_for() {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.survey_refused = Some("1.8 MHz of view is too narrow".to_string());
        let out = draw(NetCoexistPanel, 60, 20, &m).join("\n");
        assert!(out.contains("no pass is possible"), "{out}");
        assert!(out.contains("1.8 MHz"), "{out}");
        assert!(!out.contains("waiting"), "{out}");
    }

    /// A history shorter than the canvas leaves the old end, the bottom, dark
    /// rather than stretching what there is down it.
    #[test]
    fn a_short_history_does_not_pretend_to_fill_the_panel() {
        let theme = crate::Theme::sdr();
        let buf = cells(&with(vec![column(40); 4]), 60, 20);
        let x = band_axis::column_of(40, 60) as u16;
        // Four moments fill the top two rows; the bottom of the canvas is dark.
        assert_ne!(buf.get(x, 0).style().fg, Some(ink(None, &theme)));
        assert_eq!(buf.get(x, 17).style().fg, Some(ink(None, &theme)));
    }

    #[test]
    fn it_fits_every_size_the_layout_can_hand_it() {
        for w in 10..130u16 {
            for h in 3..30u16 {
                for line in draw(NetCoexistPanel, w, h, &with(vec![column(40); 10])) {
                    assert!(line.chars().count() <= w as usize, "{w}x{h}: {line:?}");
                }
            }
        }
    }
}
