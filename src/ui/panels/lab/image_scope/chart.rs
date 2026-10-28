// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The bar chart: an LO-centred slice of the spectrum drawn in block characters,
//! with a marker row above it and a frequency axis below.
//!
//! Not a ratatui `Canvas` - the whole point of this plot is that the carrier, its
//! mirror and the DC spike are *the same three columns* in the marker row, the
//! bars and the axis, and colouring a column consistently across all three is
//! what makes the mirror symmetry legible. Block cells give that column identity;
//! a braille canvas would put two peaks in one cell.

use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};

use super::tint::Tint;

/// The dBFS window: the floor 10 dB under its bottom, the strongest bin 5 dB
/// under its top, both on 10 dB lines, never above full scale. A fixed
/// 0 to -120 dBFS left the top half of the chart empty on every real capture.
fn y_range(bins: &[f32], floor: f32) -> (f32, f32) {
    let max = bins.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let top = (((max + 5.0) / 10.0).ceil() * 10.0).min(0.0);
    let bottom = ((floor - 10.0) / 10.0).floor() * 10.0;
    (bottom.min(top - 20.0), top)
}

/// Partial-cell block ramp: index 0 = empty, 8 = full cell.
const BLOCKS: [char; 9] = [
    ' ', '\u{2581}', '\u{2582}', '\u{2583}', '\u{2584}', '\u{2585}', '\u{2586}', '\u{2587}',
    '\u{2588}',
];

/// Gutter width holding the dBFS tick labels ("−120").
pub(super) const GUTTER: usize = 4;

/// The LO-centred frequency window the chart spans, and the three frequencies
/// that get their own colour in it.
pub(super) struct Window {
    pub left_hz: f64,
    pub right_hz: f64,
    /// The frame's centre: where its bins are counted from.
    pub center_hz: f64,
    /// The LO: what the mirror reflects about, and what the axis names.
    pub lo_hz: f64,
    pub carrier_hz: f64,
    pub image_hz: f64,
}

impl Window {
    /// The window wide enough to hold both the carrier and its mirror, with a
    /// floor on the span so a carrier sitting on the LO still gets a readable
    /// slice rather than collapsing to nothing.
    /// Centred on the LO, `lo_offset_hz` from the frame's centre; the carrier
    /// is `carrier_offset_hz` from the centre and its mirror as far the other
    /// side of the LO.
    pub(super) fn new(
        center_hz: f64,
        lo_offset_hz: f64,
        carrier_offset_hz: f64,
        rate: f64,
    ) -> Self {
        let lo_hz = center_hz + lo_offset_hz;
        let carrier_hz = center_hz + carrier_offset_hz;
        let off = (carrier_hz - lo_hz).abs().max(rate * 0.04);
        let span = (off * 1.5).min(rate / 2.0).max(1.0);
        Self {
            left_hz: lo_hz - span,
            right_hz: lo_hz + span,
            center_hz,
            lo_hz,
            carrier_hz,
            image_hz: 2.0 * lo_hz - carrier_hz,
        }
    }

    fn width_hz(&self) -> f64 {
        (self.right_hz - self.left_hz).max(1.0)
    }

    /// Which display column a frequency lands in, or `None` outside the window.
    fn col_of(&self, fhz: f64, chart_w: usize) -> Option<usize> {
        if fhz < self.left_hz || fhz >= self.right_hz {
            return None;
        }
        Some((((fhz - self.left_hz) / self.width_hz()) * chart_w as f64) as usize)
            .map(|c| c.min(chart_w - 1))
    }
}

/// What the chart marks beyond the bars: where the carrier's level is
/// carried across to its mirror, and what the gap is called. `None` draws the
/// spectrum and the LO alone, for a carrier that has no image to show.
pub(super) struct Gap {
    pub carrier_dbfs: f32,
    pub image_dbfs: f32,
    /// `30 dB`, `≥17 dB` or `5 dB?`.
    pub label: String,
}

/// Render the marker row, the bar rows and the frequency-axis row into `out`.
/// Draws nothing when the area is too small to be read.
#[allow(clippy::too_many_arguments)]
pub(super) fn draw(
    out: &mut Vec<Line<'static>>,
    bins: &[f32],
    rate: f64,
    floor: f32,
    win: &Window,
    gap: Option<&Gap>,
    chart_w: usize,
    chart_h: usize,
    tint: &Tint,
) {
    if chart_w < 8 || chart_h < 3 {
        return;
    }
    let n = bins.len();
    let bin_hz = rate / n as f64;
    let (bottom, top) = y_range(bins, floor);
    // Aggregate bins into columns (peak per column), single O(n) pass.
    let mut col_level = vec![bottom; chart_w];
    for (i, &v) in bins.iter().enumerate() {
        let bf = win.center_hz + (i as f64 - n as f64 / 2.0) * bin_hz;
        if let Some(c) = win.col_of(bf, chart_w) {
            col_level[c] = col_level[c].max(v);
        }
    }
    let carrier_col = gap.and(win.col_of(win.carrier_hz, chart_w));
    let image_col = gap.and(win.col_of(win.image_hz, chart_w));
    let lo_col = win.col_of(win.lo_hz, chart_w);
    let rows = chart_h as f32;
    // Rows down from the top, fractional, for a level.
    let depth = |db: f32| ((top - db) / (top - bottom)).clamp(0.0, 1.0) * rows;

    // The marker row: the LO always, the carrier and its mirror when there is
    // an image to show.
    let mut cells = vec![(' ', tint.rule); chart_w];
    if let Some(c) = lo_col {
        cells[c] = ('\u{250a}', tint.rule);
    }
    if let Some(c) = carrier_col {
        cells[c] = ('\u{25bc}', tint.carrier);
    }
    if let Some(c) = image_col {
        cells[c] = ('\u{25bd}', tint.image);
    }
    let mut marks = with_gutter(String::new(), &cells, tint);
    // Above the chart the gutter carries no rule.
    marks.spans[0] = Span::raw(" ".repeat(GUTTER + 1));
    out.push(marks);

    // The bars.
    let mut grid = vec![vec![(' ', tint.base); chart_w]; chart_h];
    for (c, &level) in col_level.iter().enumerate() {
        let h = rows - depth(level);
        let col = if Some(c) == carrier_col {
            tint.carrier
        } else if Some(c) == image_col {
            tint.image
        } else {
            tint.base
        };
        for (r, row) in grid.iter_mut().enumerate() {
            let cell = h - (chart_h - 1 - r) as f32;
            if cell > 0.05 {
                row[c] = (BLOCKS[(cell * 8.0).round().clamp(1.0, 8.0) as usize], col);
            }
        }
    }
    // The carrier's level carried across to its mirror, and the gap between
    // the two drawn on the mirror's column with its size beside it: the image
    // rejection where it is, not two numbers to subtract.
    if let (Some(g), Some(m)) = (gap, image_col) {
        let from = (depth(g.carrier_dbfs) as usize).min(chart_h - 1);
        let to = (depth(g.image_dbfs) as usize).min(chart_h - 1);
        let reach = m.saturating_sub(1)..=(m + 1).min(chart_w - 1);
        grid[from][reach].fill(('\u{254c}', tint.carrier));
        for row in grid.iter_mut().take(to).skip(from + 1) {
            row[m] = ('\u{2506}', tint.label);
        }
        let len = g.label.chars().count();
        let at = if m + 2 + len <= chart_w {
            m + 2
        } else {
            m.saturating_sub(len + 1)
        };
        let mid = (from + to.max(from)) / 2;
        for (k, ch) in g.label.chars().enumerate() {
            if let Some(cell) = grid[mid].get_mut(at + k) {
                *cell = (ch, tint.label);
            }
        }
    }
    // dBFS every 10 dB where the rows allow, else every 20.
    let step = if rows / ((top - bottom) / 10.0) >= 2.0 {
        10.0
    } else {
        20.0
    };
    let mut ticks = vec![String::new(); chart_h];
    let mut db = top;
    while db >= bottom {
        let r = (depth(db).round() as usize).min(chart_h - 1);
        if ticks[r].is_empty() {
            ticks[r] = format!("{:>width$}", db as i32, width = GUTTER);
        }
        db -= step;
    }
    for (row, tick) in grid.iter().zip(ticks) {
        out.push(with_gutter(tick, row, tint));
    }

    // Frequency axis row.
    let mut axis: Vec<char> = vec![' '; chart_w];
    let write = |buf: &mut Vec<char>, at: usize, s: &str| {
        for (k, ch) in s.chars().enumerate() {
            if at + k < buf.len() {
                buf[at + k] = ch;
            }
        }
    };
    let lo_lbl = format!("LO {}", fmt_mhz2(win.lo_hz));
    write(&mut axis, 0, &fmt_mhz2(win.left_hz));
    let cen = chart_w.saturating_sub(lo_lbl.chars().count()) / 2;
    write(&mut axis, cen, &lo_lbl);
    let r_lbl = fmt_mhz2(win.right_hz);
    write(
        &mut axis,
        chart_w.saturating_sub(r_lbl.chars().count()),
        &r_lbl,
    );
    out.push(Line::from(vec![
        Span::raw(" ".repeat(GUTTER + 1)),
        Span::styled(
            axis.into_iter().collect::<String>(),
            Style::default().fg(tint.rule),
        ),
    ]));
    out.push(Line::raw(""));
}

/// One chart row: its tick in the gutter (or the bare rule) and its cells as
/// styled runs.
fn with_gutter(tick: String, cells: &[(char, Color)], tint: &Tint) -> Line<'static> {
    let gutter = if tick.is_empty() {
        format!("{:>GUTTER$}\u{2502}", "")
    } else {
        format!("{tick}\u{2524}")
    };
    let mut spans = vec![Span::styled(gutter, Style::default().fg(tint.rule))];
    let mut run = String::new();
    let mut cur: Option<Color> = None;
    for &(ch, col) in cells {
        if cur != Some(col) {
            if let Some(c) = cur {
                spans.push(Span::styled(
                    std::mem::take(&mut run),
                    Style::default().fg(c),
                ));
            }
            cur = Some(col);
        }
        run.push(ch);
    }
    if let Some(c) = cur {
        spans.push(Span::styled(run, Style::default().fg(c)));
    }
    Line::from(spans)
}

fn fmt_mhz2(hz: f64) -> String {
    format!("{:.2}M", hz / 1e6)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The chart spans the floor to the strongest bin, on 10 dB lines.
    #[test]
    fn the_y_range_follows_the_capture() {
        let mut b = vec![-84.0; 64];
        b[32] = -39.0;
        assert_eq!(y_range(&b, -84.2), (-100.0, -30.0));
        // Never above full scale.
        b[32] = 0.0;
        assert_eq!(y_range(&b, -84.2).1, 0.0);
    }

    #[test]
    fn window_holds_both_carrier_and_its_mirror() {
        // A carrier 200 kHz above a 100 MHz LO: the window must contain both it and
        // the mirror 200 kHz below, or the plot's whole point is off screen.
        let w = Window::new(100e6, 0.0, 200e3, 2e6);
        assert!(w.left_hz <= w.image_hz, "mirror is off the left edge");
        assert!(w.right_hz > w.carrier_hz, "carrier is off the right edge");
        assert_eq!(w.carrier_hz, 100.2e6);
        assert_eq!(w.image_hz, 99.8e6);
    }

    #[test]
    fn window_stays_readable_for_a_carrier_on_the_lo() {
        // Zero offset would collapse the span to nothing; the rate-derived floor
        // keeps a slice to look at.
        let w = Window::new(100e6, 0.0, 0.0, 2e6);
        assert!(w.right_hz - w.left_hz > 0.0);
        assert_eq!(w.carrier_hz, w.image_hz, "on the LO the two coincide");
    }

    #[test]
    fn window_never_exceeds_the_captured_span() {
        // Half the sample rate either side is all there is; a huge offset must clamp
        // rather than plot frequencies that were never captured.
        let rate = 2e6;
        let w = Window::new(100e6, 0.0, 5e6, rate);
        assert!(w.right_hz - w.center_hz <= rate / 2.0 + 1.0);
        assert!(w.center_hz - w.left_hz <= rate / 2.0 + 1.0);
    }

    #[test]
    fn columns_span_the_window_left_to_right() {
        let w = Window::new(100e6, 0.0, 200e3, 2e6);
        assert_eq!(
            w.col_of(w.left_hz, 40),
            Some(0),
            "left edge is the first column"
        );
        assert_eq!(
            w.col_of(w.right_hz, 40),
            None,
            "the right edge is exclusive"
        );
        let last = w.col_of(w.right_hz - 1.0, 40).unwrap();
        assert_eq!(last, 39, "just inside the right edge is the last column");
        // The LO sits at the middle of an LO-centred window.
        assert_eq!(w.col_of(w.center_hz, 40), Some(20));
    }

    /// The window is centred on the LO, and the mirror sits as far the other
    /// side of it as the carrier: with the LO 5 kHz up, a carrier 20 kHz up is
    /// 15 kHz above the LO and mirrors 15 kHz below it, 10 kHz under the
    /// centre.
    #[test]
    fn the_window_turns_about_the_lo() {
        let w = Window::new(100e6, 5e3, 20e3, 768e3);
        assert_eq!(w.lo_hz, 100.005e6);
        assert_eq!(w.carrier_hz, 100.020e6);
        assert_eq!(w.image_hz, 99.990e6);
        assert_eq!(
            (w.right_hz - w.lo_hz, w.lo_hz - w.left_hz),
            (w.lo_hz - w.left_hz, w.right_hz - w.lo_hz),
            "symmetric about the LO"
        );
    }
}
