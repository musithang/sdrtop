// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The waterfall grid itself: history rows painted as coloured half-blocks.
//!
//! Each character cell carries **two** time rows - `▀` with the older row as the
//! background and the newer as the foreground - so the visible history is twice
//! the panel's height. That doubling is why the scroll and stride arithmetic
//! keeps two units in play at once, `skip_chars` and `skip_data`, and mixing
//! them up scrolls at half or double speed.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::palette::{magnitude_to_color_palette, ColorDepth, WaterfallPalette};
use crate::state::{BinAxis, BinWindow};

/// Max dB over the bin range `[start, end)` of one waterfall row, clamped to the
/// row's own length. Rows are normally all the (fixed) FFT bin count, but reading
/// each row against its own length means a row that ever differs - e.g. if the FFT
/// size changed at runtime - degrades to a partial read instead of slicing out of
/// bounds and panicking. An empty or fully out-of-range span reads −∞ (the floor).
pub(super) fn band_max(row: &[f32], start: usize, end: usize) -> f32 {
    let s = start.min(row.len());
    let e = end.min(row.len()).max(s);
    if s == e {
        f32::NEG_INFINITY
    } else {
        row[s..e].iter().copied().fold(f32::NEG_INFINITY, f32::max)
    }
}

/// Which bins each plot column covers, under the current frequency zoom.
///
/// Zoom keeps the centre `1/zoom` of the row's bins - the same slice the bonded
/// spectrum above narrows to, so `+`/`-` zoom both plots as one instrument.
pub(super) struct Columns {
    pub window: BinWindow,
    cols: usize,
    bin_axis: BinAxis,
}

impl Columns {
    /// Use a window computed from the row being drawn
    pub fn new(window: BinWindow, cols: usize, bin_axis: BinAxis) -> Self {
        Self {
            window,
            cols: cols.max(1),
            bin_axis,
        }
    }

    /// The `[start, end)` bin span column `col` reads. Always non-empty, so a
    /// wide panel over few bins still gets one bin per column rather than none.
    pub fn range(&self, col: usize) -> (usize, usize) {
        let visible_n = self.window.bin_count;
        let (start, end) = match self.bin_axis {
            BinAxis::FftBins => (
                col * visible_n / self.cols,
                (col + 1) * visible_n / self.cols,
            ),
            BinAxis::MeasuredPoints => {
                let intervals = visible_n.saturating_sub(1);
                let start = (col * intervals).div_ceil(self.cols);
                let end = if col + 1 >= self.cols {
                    visible_n
                } else {
                    ((col + 1) * intervals).div_ceil(self.cols)
                };
                (start, end)
            }
        };
        let start = start.min(visible_n - 1);
        let end = end.max(start + 1).min(visible_n);
        (self.window.first_bin + start, self.window.first_bin + end)
    }
}

/// Paint the history grid. `skip_data` is in *data rows*, not character rows.
#[allow(clippy::too_many_arguments)]
pub(super) fn draw(
    f: &mut Frame,
    area: Rect,
    rows: &VecDeque<(Instant, Arc<Vec<f32>>)>,
    columns_for_row: impl Fn(usize) -> Option<Columns>,
    cursor_col: Option<usize>,
    skip_data: usize,
    db_min: f32,
    db_max: f32,
    palette: WaterfallPalette,
    theme: &crate::Theme,
) {
    let cols = area.width as usize;
    let depth = ColorDepth::detect();
    let color = |db: f32| magnitude_to_color_palette(db, db_min, db_max, depth, theme, palette);
    let floor = color(f32::NEG_INFINITY);

    let mut lines: Vec<Line> = Vec::with_capacity(area.height as usize);
    let mut data = rows.iter().skip(skip_data).take(area.height as usize * 2);
    while let Some((_ts, top_row)) = data.next() {
        let bot_row = data.next().map(|(_ts, r)| r.as_ref());
        let top_columns = columns_for_row(top_row.len());
        let bot_columns = bot_row.and_then(|row| columns_for_row(row.len()));
        let row_color = |row: &[f32], columns: Option<&Columns>, col| {
            columns.map_or(floor, |columns| {
                let (lo, hi) = columns.range(col);
                color(band_max(row, lo, hi))
            })
        };
        let spans: Vec<Span> = (0..cols)
            .map(|col| {
                let bot_color = bot_row
                    .map(|r| row_color(r, bot_columns.as_ref(), col))
                    .unwrap_or(floor);
                // The cursor column keeps the background so the history still reads
                // through it, but takes a bright foreground as its marker.
                let top_color = if Some(col) == cursor_col {
                    theme.value_hi
                } else {
                    row_color(top_row, top_columns.as_ref(), col)
                };
                Span::styled("\u{2580}", Style::default().fg(top_color).bg(bot_color))
            })
            .collect();
        lines.push(Line::from(spans));
    }

    f.render_widget(Paragraph::new(lines), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn columns(row_bins: usize, zoom: usize, cols: usize) -> Columns {
        let axis = BinAxis::FftBins;
        let window = axis
            .window(100_000_000, 32_000_000.0, row_bins, zoom)
            .unwrap();
        Columns::new(window, cols, axis)
    }
    #[test]
    fn cell_colors_use_the_configured_ceiling() {
        let theme = crate::Theme::sdr();
        let palette = WaterfallPalette::default();
        let rows = VecDeque::from([(Instant::now(), Arc::new(vec![-10.0]))]);
        for max in [0.0, -10.0] {
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(1, 1)).unwrap();
            terminal
                .draw(|f| {
                    draw(
                        f,
                        f.area(),
                        &rows,
                        |row_bins| Some(columns(row_bins, 1, 1)),
                        None,
                        0,
                        -110.0,
                        max,
                        palette,
                        &theme,
                    )
                })
                .unwrap();
            assert_eq!(
                terminal.backend().buffer()[(0, 0)].fg,
                magnitude_to_color_palette(
                    -10.0,
                    -110.0,
                    max,
                    ColorDepth::detect(),
                    &theme,
                    palette
                )
            );
        }
    }

    #[test]
    fn band_max_reads_in_range() {
        let row = [-90.0, -50.0, -70.0, -60.0];
        assert_eq!(band_max(&row, 0, 4), -50.0, "max over the whole row");
        assert_eq!(band_max(&row, 2, 4), -60.0, "max over a sub-range");
    }

    #[test]
    fn band_max_clamps_out_of_range_indices() {
        let row = [-90.0, -50.0, -70.0];
        // End past the row length must clamp, not panic.
        assert_eq!(band_max(&row, 1, 99), -50.0);
        // A start at/after the row length yields the floor (empty span).
        assert_eq!(band_max(&row, 3, 9), f32::NEG_INFINITY);
        assert_eq!(band_max(&row, 5, 2), f32::NEG_INFINITY);
    }

    #[test]
    fn band_max_empty_row_is_floor() {
        assert_eq!(band_max(&[], 0, 4), f32::NEG_INFINITY);
    }

    #[test]
    fn unzoomed_columns_cover_every_bin_exactly_once() {
        let c = columns(1024, 1, 128);
        let (first, _) = c.range(0);
        let (_, last) = c.range(127);
        assert_eq!(first, 0, "the first column starts at the first bin");
        assert_eq!(last, 1024, "the last column ends at the last bin");
        // Contiguous: each column picks up where the previous one stopped.
        for col in 1..128 {
            assert_eq!(
                c.range(col).0,
                c.range(col - 1).1,
                "gap or overlap at column {col}"
            );
        }
    }

    #[test]
    fn zoom_keeps_the_centre_slice() {
        let c = columns(1024, 4, 128);
        let (first, _) = c.range(0);
        let (_, last) = c.range(127);
        assert_eq!(first, 384, "a quarter of the way in");
        assert_eq!(last, 640, "and out again: 256 bins around the centre");
    }

    #[test]
    fn columns_follow_the_bin_window_at_uneven_zoom() {
        let axis = BinAxis::FftBins;
        let window = axis.window(100, 10.0, 10, 3).unwrap();
        let columns = Columns::new(window, 3, axis);
        assert_eq!(columns.range(0), (4, 5));
        assert_eq!(columns.range(1), (5, 6));
        assert_eq!(columns.range(2), (6, 7));
        for col in 0..3 {
            assert_eq!(
                axis.frequency_of_bin(window.left_hz, window.span_hz, window.bin_count, col),
                Some(99.0 + col as f64)
            );
        }
    }

    #[test]
    fn a_column_is_never_empty_however_odd_the_geometry() {
        // More columns than bins: every column still reads at least one bin,
        // rather than an empty span that would paint the whole plot at the floor.
        let c = columns(8, 1, 200);
        for col in 0..200 {
            let (lo, hi) = c.range(col);
            assert!(hi > lo, "column {col} is empty");
            assert!(hi <= 8, "column {col} runs past the row");
        }
    }

    #[test]
    fn zoomed_columns_are_nothing_like_the_naive_full_range_mapping() {
        // The bug this pins. The focus readout used to map its column onto the
        // *whole* row - `col * row_bins / cols` - while the grid mapped it
        // through the zoom window. At zoom 1 the two agree, which is why it
        // went unnoticed; zoomed in, the readout quoted the level of a
        // completely different frequency from the column it pointed at.
        let (row_bins, cols) = (1024usize, 128usize);
        let naive = |col: usize| col * row_bins / cols;

        let unzoomed = columns(row_bins, 1, cols);
        for col in 0..cols {
            assert_eq!(unzoomed.range(col).0, naive(col), "zoom 1 hides the bug");
        }

        let zoomed = columns(row_bins, 4, cols);
        assert_eq!(zoomed.range(0).0, 384);
        assert_eq!(
            naive(0),
            0,
            "the naive mapping reads a bin that is off screen"
        );
        // They coincide only at the centre, the one place the old readout was right.
        assert_eq!(zoomed.range(cols / 2).0, naive(cols / 2));
    }

    #[test]
    fn singleton_and_zero_columns_do_not_underflow() {
        // A zero-bin row and a zero zoom are both nonsense, and both used to be
        // one subtraction away from panicking.
        let c = columns(1, 32, 40);
        let (lo, hi) = c.range(0);
        assert!(hi > lo);
        let zero_cols = columns(1024, 4, 0);
        let _ = zero_cols.range(0);
    }

    #[test]
    fn high_zoom_sub_bin_frequencies_use_the_same_interval() {
        let axis = BinAxis::FftBins;
        let columns = columns(256, 32, 160);
        let window = columns.window;
        for col in 0..160 {
            let frequency = window.left_hz + col as f64 * window.span_hz / 160.0;
            let index = axis
                .nearest_bin(window.left_hz, window.span_hz, window.bin_count, frequency)
                .unwrap();
            assert_eq!(
                columns.range(col).0,
                window.first_bin + index,
                "column {col}"
            );
        }
    }
    #[test]
    fn measured_point_columns_follow_endpoint_coordinates() {
        let window = BinAxis::MeasuredPoints
            .window(131_500_000, 63_000_000.0, 64, 1)
            .unwrap();
        let columns = Columns::new(window, 40, BinAxis::MeasuredPoints);
        assert_eq!(columns.range(0), (0, 2));
        assert_eq!(columns.range(39), (62, 64));
    }
}
