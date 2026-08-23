//! The bar chart: an LO-centred slice of the spectrum drawn in block characters,
//! with a marker row above it and a frequency axis below.
//!
//! Not a ratatui `Canvas` — the whole point of this plot is that the carrier, its
//! mirror and the DC spike are *the same three columns* in the marker row, the
//! bars and the axis, and colouring a column consistently across all three is
//! what makes the mirror symmetry legible. Block cells give that column identity;
//! a braille canvas would put two peaks in one cell.

use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};

use super::tint::Tint;

/// Fixed dBFS window for the bar chart — a stable axis (0 at top, −120 at the
/// floor) reads better than an auto-ranging one when comparing two peaks.
const FLOOR_DBFS: f32 = -120.0;
const TOP_DBFS:   f32 = 0.0;

/// Partial-cell block ramp: index 0 = empty, 8 = full cell.
const BLOCKS: [char; 9] = [' ', '\u{2581}', '\u{2582}', '\u{2583}', '\u{2584}',
                           '\u{2585}', '\u{2586}', '\u{2587}', '\u{2588}'];

/// Gutter width holding the dBFS tick labels ("−120").
pub(super) const GUTTER: usize = 4;

/// The LO-centred frequency window the chart spans, and the three frequencies
/// that get their own colour in it.
pub(super) struct Window {
    pub left_hz:   f64,
    pub right_hz:  f64,
    pub center_hz: f64,
    pub carrier_hz: f64,
    pub image_hz:   f64,
}

impl Window {
    /// The window wide enough to hold both the carrier and its mirror, with a
    /// floor on the span so a carrier sitting on the LO still gets a readable
    /// slice rather than collapsing to nothing.
    pub(super) fn new(center_hz: f64, carrier_offset_hz: f64, rate: f64) -> Self {
        let off  = carrier_offset_hz.abs().max(rate * 0.04);
        let span = (off * 1.5).min(rate / 2.0).max(1.0);
        Self {
            left_hz:    center_hz - span,
            right_hz:   center_hz + span,
            center_hz,
            carrier_hz: center_hz + carrier_offset_hz,
            image_hz:   center_hz - carrier_offset_hz,
        }
    }

    fn width_hz(&self) -> f64 { (self.right_hz - self.left_hz).max(1.0) }

    /// Which display column a frequency lands in, or `None` outside the window.
    fn col_of(&self, fhz: f64, chart_w: usize) -> Option<usize> {
        if fhz < self.left_hz || fhz >= self.right_hz { return None; }
        Some((((fhz - self.left_hz) / self.width_hz()) * chart_w as f64) as usize)
            .map(|c| c.min(chart_w - 1))
    }
}

/// Render the marker row, the bar rows and the frequency-axis row into `out`.
/// Draws nothing when the area is too small to be read.
pub(super) fn draw(
    out: &mut Vec<Line<'static>>, bins: &[f32], rate: f64,
    win: &Window, chart_w: usize, chart_h: usize, tint: &Tint,
) {
    if chart_w < 8 || chart_h < 3 { return; }
    let n = bins.len();
    let bin_hz = rate / n as f64;

    // Aggregate bins into columns (peak per column), single O(n) pass.
    let mut col_level = vec![FLOOR_DBFS; chart_w];
    for (i, &v) in bins.iter().enumerate() {
        let bf = win.center_hz + (i as f64 - n as f64 / 2.0) * bin_hz;
        if bf < win.left_hz || bf >= win.right_hz { continue; }
        let c = (((bf - win.left_hz) / win.width_hz()) * chart_w as f64) as usize;
        let c = c.min(chart_w - 1);
        if v > col_level[c] { col_level[c] = v; }
    }

    let carrier_col = win.col_of(win.carrier_hz, chart_w);
    let image_col   = win.col_of(win.image_hz, chart_w);
    let dc_col      = win.col_of(win.center_hz, chart_w);
    let col_color = |c: usize| -> Color {
        if Some(c) == carrier_col      { tint.carrier }
        else if Some(c) == image_col   { tint.image }
        else if Some(c) == dc_col      { tint.dc }
        else                           { tint.base }
    };

    // Marker row: ▼ over carrier/image, ▮ over DC.
    let mut mk: Vec<Span> = vec![Span::raw(" ".repeat(GUTTER + 1))];
    let mut run = String::new();
    let mut run_col = tint.base;
    let flush = |run: &mut String, col: Color, out: &mut Vec<Span>| {
        if !run.is_empty() { out.push(Span::styled(std::mem::take(run), Style::default().fg(col))); }
    };
    for c in 0..chart_w {
        let (ch, col) =
            if Some(c) == carrier_col      { ('\u{25bc}', tint.carrier) }
            else if Some(c) == image_col   { ('\u{25bc}', tint.image) }
            else if Some(c) == dc_col      { ('\u{25ae}', tint.dc) }
            else                           { (' ', tint.base) };
        if col != run_col { flush(&mut run, run_col, &mut mk); run_col = col; }
        run.push(ch);
    }
    flush(&mut run, run_col, &mut mk);
    out.push(Line::from(mk));

    // dBFS gridline label rows.
    let tick_row = |db: f32| -> usize {
        (((TOP_DBFS - db) / (TOP_DBFS - FLOOR_DBFS)) * (chart_h - 1) as f32).round() as usize
    };
    let mut row_label = vec![String::new(); chart_h];
    for db in [0.0, -30.0, -60.0, -90.0, -120.0] {
        let row = tick_row(db).min(chart_h - 1);
        if row_label[row].is_empty() { row_label[row] = format!("{:>width$}", db as i32, width = GUTTER); }
    }

    // Bar rows, top → bottom.
    for row in 0..chart_h {
        let mut spans: Vec<Span> = Vec::new();
        let g = &row_label[row];
        if g.is_empty() {
            spans.push(Span::styled(format!("{:>gutter$}\u{2502}", "", gutter = GUTTER), Style::default().fg(tint.rule)));
        } else {
            spans.push(Span::styled(format!("{g}\u{2524}"), Style::default().fg(tint.rule)));
        }
        let from_bottom = (chart_h - 1 - row) as f32;
        let mut run = String::new();
        let mut run_col = tint.base;
        let mut started = false;
        for c in 0..chart_w {
            let frac = ((col_level[c] - FLOOR_DBFS) / (TOP_DBFS - FLOOR_DBFS)).clamp(0.0, 1.0);
            let cell = frac * chart_h as f32 - from_bottom;
            let ch = if cell >= 1.0 { '\u{2588}' }
                     else if cell <= 0.05 { ' ' }
                     else { BLOCKS[(cell * 8.0).round().clamp(1.0, 8.0) as usize] };
            let col = if ch == ' ' { tint.base } else { col_color(c) };
            if !started { run_col = col; started = true; }
            if col != run_col {
                spans.push(Span::styled(std::mem::take(&mut run), Style::default().fg(run_col)));
                run_col = col;
            }
            run.push(ch);
        }
        spans.push(Span::styled(run, Style::default().fg(run_col)));
        out.push(Line::from(spans));
    }

    // Frequency axis row.
    let mut axis: Vec<char> = vec![' '; chart_w];
    let write = |buf: &mut Vec<char>, at: usize, s: &str| {
        for (k, ch) in s.chars().enumerate() {
            if at + k < buf.len() { buf[at + k] = ch; }
        }
    };
    let lo_lbl = format!("{} LO", fmt_mhz2(win.center_hz));
    write(&mut axis, 0, &fmt_mhz2(win.left_hz));
    let cen = chart_w.saturating_sub(lo_lbl.chars().count()) / 2;
    write(&mut axis, cen, &lo_lbl);
    let r_lbl = fmt_mhz2(win.right_hz);
    write(&mut axis, chart_w.saturating_sub(r_lbl.chars().count()), &r_lbl);
    out.push(Line::from(vec![
        Span::raw(" ".repeat(GUTTER + 1)),
        Span::styled(axis.into_iter().collect::<String>(), Style::default().fg(tint.rule)),
    ]));

    out.push(Line::raw(""));
}

fn fmt_mhz2(hz: f64) -> String { format!("{:.2}M", hz / 1e6) }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_holds_both_carrier_and_its_mirror() {
        // A carrier 200 kHz above a 100 MHz LO: the window must contain both it and
        // the mirror 200 kHz below, or the plot's whole point is off screen.
        let w = Window::new(100e6, 200e3, 2e6);
        assert!(w.left_hz <= w.image_hz, "mirror is off the left edge");
        assert!(w.right_hz > w.carrier_hz, "carrier is off the right edge");
        assert_eq!(w.carrier_hz, 100.2e6);
        assert_eq!(w.image_hz, 99.8e6);
    }

    #[test]
    fn window_stays_readable_for_a_carrier_on_the_lo() {
        // Zero offset would collapse the span to nothing; the rate-derived floor
        // keeps a slice to look at.
        let w = Window::new(100e6, 0.0, 2e6);
        assert!(w.right_hz - w.left_hz > 0.0);
        assert_eq!(w.carrier_hz, w.image_hz, "on the LO the two coincide");
    }

    #[test]
    fn window_never_exceeds_the_captured_span() {
        // Half the sample rate either side is all there is; a huge offset must clamp
        // rather than plot frequencies that were never captured.
        let rate = 2e6;
        let w = Window::new(100e6, 5e6, rate);
        assert!(w.right_hz - w.center_hz <= rate / 2.0 + 1.0);
        assert!(w.center_hz - w.left_hz <= rate / 2.0 + 1.0);
    }

    #[test]
    fn columns_span_the_window_left_to_right() {
        let w = Window::new(100e6, 200e3, 2e6);
        assert_eq!(w.col_of(w.left_hz, 40), Some(0), "left edge is the first column");
        assert_eq!(w.col_of(w.right_hz, 40), None, "the right edge is exclusive");
        let last = w.col_of(w.right_hz - 1.0, 40).unwrap();
        assert_eq!(last, 39, "just inside the right edge is the last column");
        // The LO sits at the middle of an LO-centred window.
        assert_eq!(w.col_of(w.center_hz, 40), Some(20));
    }
}
