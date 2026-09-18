// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `NetCoexistPanel` - what is stepping on your link, and when.
//!
//! The band down the side, time across the bottom, and each cell coloured by how
//! busy that megahertz was at that moment. Design section 9.1 calls it the
//! single most immediately legible thing either arc produces, and section 8's
//! acceptance criterion for it is not a test: **readable at two metres.**
//!
//! **What it is fed on, and what the plan said it would be fed on.** The plan
//! has this panel drawing bursts from N14, colour-coded by protocol. N14 landed
//! per-cell duty cycle and no burst detector - "no demodulation anywhere" was
//! its own instruction - so there are no bursts to draw and there will be none
//! until an arc lands one. What there is instead is real and is the same
//! picture at a coarser grain: the occupancy history, half a second to a column.
//! Colour carries the duty cycle where it will eventually carry the protocol.
//!
//! That is a deviation from the plan, and it is written here rather than left to
//! be discovered: this panel is finished when a burst overlay replaces the ramp,
//! not before.

use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::signal::net::{band, occupancy};
use crate::state::{SdrMetrics, COLUMN_INTERVAL};
use crate::ui::panel::{Panel, PanelChrome, Staleness};
use crate::ui::widgets::canvas::{fold, ink, row, Duty};

pub struct NetCoexistPanel;

/// Rows reserved under the canvas for the time axis.
const AXIS_ROWS: u16 = 1;
/// Columns reserved to the left for the frequency scale.
const SCALE_COLS: u16 = 5;

/// `2400` down the left edge, on the rows a label lands on.
///
/// Spaced by **row**, not by frequency band: a row carries two bands and only
/// its upper one has a line to be written on, so labelling every nth band put
/// labels on an irregular handful of rows and left the rest blank.
fn scale_label(row: usize, rows: usize, bands: usize) -> Option<String> {
    // Four labels down the side, which is as many as a forty-row panel carries
    // without them running together.
    //
    // **Counted from the bottom**, so the bottom row always carries one. Counted
    // from the top, the lowest label landed on whichever row happened to be a
    // multiple of the spacing, and the scale read 2407 at the foot of a band
    // that starts at 2400.
    let step = rows.div_ceil(4).max(1);
    if !(rows.saturating_sub(1 + row)).is_multiple_of(step) {
        return None;
    }
    // The row's *lower* band: a label marks the bottom edge of its row, the way
    // a ruler does, and it is what makes the bottom row read as the bottom of
    // the band rather than one cell above it.
    let band = bands.saturating_sub(2 + row * 2);
    let cell = band * occupancy::CELLS / bands.max(1);
    Some(format!(
        "{:>4}",
        (band::LOW_HZ + cell as u64 * occupancy::CELL_HZ) / 1_000_000
    ))
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
    }

    fn render(
        &self,
        f: &mut Frame,
        inner: Rect,
        state: &SdrMetrics,
        theme: &crate::Theme,
        _focused: bool,
    ) {
        if inner.width <= SCALE_COLS || inner.height <= AXIS_ROWS {
            return;
        }
        let width = (inner.width - SCALE_COLS) as usize;
        let rows = (inner.height - AXIS_ROWS) as usize;
        // Two frequency bands to a character row: that is what a half block buys
        // and it is why this panel can hold the whole band in twenty rows.
        let bands = rows * 2;
        let history = &state.net.band.history;

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

        // The newest column on the right, so time runs the way it is read. A
        // history shorter than the panel leaves the left dark rather than
        // stretching what there is across it.
        let shown: Vec<&Vec<f32>> = history.iter().rev().take(width).rev().collect();
        let pad = width.saturating_sub(shown.len());
        let folded: Vec<Vec<Duty>> = shown.iter().map(|c| fold(c, bands)).collect();

        let at = |band: usize, column: usize| -> Duty {
            column
                .checked_sub(pad)
                .and_then(|i| folded.get(i))
                .and_then(|f| f.get(band).copied())
                .flatten()
        };

        let mut lines = Vec::with_capacity(rows + 1);
        for r in 0..rows {
            // Low frequency at the bottom, the way a band is drawn everywhere
            // else in this app, so the top row is the top of the band.
            let upper: Vec<Duty> = (0..width).map(|x| at(bands - 1 - r * 2, x)).collect();
            let lower: Vec<Duty> = (0..width).map(|x| at(bands - 2 - r * 2, x)).collect();
            let mut spans = vec![Span::styled(
                format!(
                    "{:<width$}",
                    scale_label(r, rows, bands).unwrap_or_default(),
                    width = SCALE_COLS as usize
                ),
                Style::default().fg(theme.label),
            )];
            spans.extend(row(&upper, &lower, theme).spans);
            lines.push(Line::from(spans));
        }

        // The time axis: how far back the left edge is.
        let span_s = shown.len() as f64 * COLUMN_INTERVAL.as_secs_f64();
        lines.push(Line::from(Span::styled(
            format!(
                "{:<width$}-{span_s:.0} s{:>right$}now",
                "",
                "",
                width = SCALE_COLS as usize,
                right = width.saturating_sub(9)
            ),
            Style::default().fg(theme.label),
        )));
        let _ = ink(None, theme);
        f.render_widget(Paragraph::new(lines), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{fixture::draw, CellReading};
    use ratatui::{backend::TestBackend, Terminal};

    /// One column of the band with `busy` set on exactly `cell`.
    fn column(cell: usize) -> Vec<f32> {
        let mut c = vec![0.0f32; occupancy::CELLS];
        c[cell] = 1.0;
        c
    }

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

    /// **A burst at a known time and frequency lands in the known cell**, at
    /// three canvas sizes. The plan's test for this panel, and the one that
    /// catches an axis drawn upside down or off by a row.
    #[test]
    fn a_busy_cell_lands_where_the_band_says_it_should() {
        let theme = crate::Theme::sdr();
        let busy = ink(Some(1.0), &theme);

        for (w, h) in [(40u16, 12u16), (90, 24), (60, 45)] {
            let rows = (h - AXIS_ROWS) as usize;
            let bands = rows * 2;
            for cell in [0usize, occupancy::CELLS / 2, occupancy::CELLS - 1] {
                let buf = cells(&with(vec![column(cell)]), w, h);
                // Where it should be: the low end of the band at the bottom.
                let band = crate::ui::widgets::canvas::band_of(cell, occupancy::CELLS, bands);
                let row = (bands - 1 - band) / 2;
                let upper = (bands - 1 - row * 2) == band;
                let x = w - 1; // the newest column, on the right
                let style = buf.get(x, row as u16).style();
                let got = if upper { style.fg } else { style.bg };
                assert_eq!(
                    got,
                    Some(busy),
                    "{w}x{h}: cell {cell} should be at row {row} ({}), \
                     but that half is not busy",
                    if upper { "upper" } else { "lower" }
                );
            }
        }
    }

    /// The band edges are the band edges: the frequency scale starts and ends
    /// where `band` says the band does, and nothing is drawn past the axis.
    #[test]
    fn the_scale_covers_the_band_and_no_more() {
        let m = with(vec![column(0)]);
        let lines = draw(NetCoexistPanel, 60, 24, &m);
        let text = lines.join("\n");
        // The lowest label is the bottom of the band.
        assert!(
            text.contains(&format!("{}", band::LOW_HZ / 1_000_000)),
            "the bottom of the band is not on the scale:\n{text}"
        );
        // And no label is above the top of it.
        for line in &lines {
            for word in line.split_whitespace() {
                if let Ok(mhz) = word.parse::<u64>() {
                    assert!(
                        (band::LOW_HZ / 1_000_000..=band::HIGH_HZ / 1_000_000).contains(&mhz),
                        "{mhz} MHz is outside the band"
                    );
                }
            }
        }
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
        for y in 0..19u16 {
            for x in SCALE_COLS..60 {
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

    /// A history shorter than the panel leaves the old end dark rather than
    /// stretching what there is across it.
    #[test]
    fn a_short_history_does_not_pretend_to_fill_the_panel() {
        let theme = crate::Theme::sdr();
        let buf = cells(&with(vec![column(40); 4]), 60, 20);
        // The newest four columns carry the reading; the left edge does not.
        assert_ne!(buf.get(59, 9).style().fg, Some(ink(None, &theme)));
        assert_eq!(buf.get(SCALE_COLS, 9).style().fg, Some(ink(None, &theme)));
    }

    #[test]
    fn it_fits_every_size_the_layout_can_hand_it() {
        for w in 10..70u16 {
            for h in 3..30u16 {
                for line in draw(NetCoexistPanel, w, h, &with(vec![column(40); 10])) {
                    assert!(line.chars().count() <= w as usize, "{w}x{h}: {line:?}");
                }
            }
        }
    }
}
