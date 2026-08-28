//! The strip itself: braille columns, coloured where they cross the budget, with
//! the deadline guide behind them and spike tags at the edges.
//!
//! Two braille columns fit in one character cell, so every text column carries
//! **two** callbacks. That is why the per-column severity is the worse of a pair
//! rather than one sample's - a cell that contains one late callback is late.

use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::state::TimingState;
use crate::ui::widgets::charts::bipolar_braille_strip;

use super::scale::{self, BLANK, GUTTER_W};
use super::severity::{dev_severity, over_tag_sign, severity_color};

/// Below this many plot columns there is nothing to read, so the panel says why
/// instead of drawing four cells of noise.
const MIN_COLS: usize = 4;

/// Callbacks per character cell - braille packs two columns of dots per cell.
const SAMPLES_PER_COL: usize = 2;

/// Per-column readings, resolved once so the row loop is a lookup.
struct Columns {
    severity: Vec<Option<u8>>,
    /// `+1` late, `−1` early, `0` in range.
    over_sign: Vec<i8>,
}

impl Columns {
    fn of(window: &[i32], cols: usize, over: &[usize], budget_us: u64) -> Self {
        let pair = |c: usize| {
            (
                window.get(SAMPLES_PER_COL * c).copied(),
                window.get(SAMPLES_PER_COL * c + 1).copied(),
            )
        };
        let severity = (0..cols)
            .map(|c| match pair(c) {
                (None, None) => None,
                (a, b) => Some(dev_severity(
                    a.map(|v| v.unsigned_abs() as u64)
                        .unwrap_or(0)
                        .max(b.map(|v| v.unsigned_abs() as u64).unwrap_or(0)),
                    budget_us,
                )),
            })
            .collect();
        let mut over_sign = vec![0i8; cols];
        for &c in over {
            let (a, b) = pair(c);
            if let Some(slot) = over_sign.get_mut(c) {
                *slot = over_tag_sign(a.unwrap_or(0), b.unwrap_or(0));
            }
        }
        Self {
            severity,
            over_sign,
        }
    }

    fn over_sign(&self, c: usize) -> i8 {
        self.over_sign.get(c).copied().unwrap_or(0)
    }
}

/// How many callbacks a chart this wide shows. Used by the window legend, so the
/// two cannot disagree about what is on screen.
pub(super) fn shown_samples(chart_w: u16, available: usize) -> usize {
    let cols = (chart_w as usize).saturating_sub(GUTTER_W);
    (SAMPLES_PER_COL * cols).min(available)
}

pub(super) fn draw(f: &mut Frame, area: Rect, t: &TimingState, stale: bool, theme: &crate::Theme) {
    let chart_h = area.height as usize;
    let cols = (area.width as usize).saturating_sub(GUTTER_W);
    if stale || cols < MIN_COLS || chart_h == 0 || t.cb_deviations_us.is_empty() {
        return placeholder(f, area, stale, theme);
    }

    let full_scale = scale::full_scale_us(t.deadline_budget_us);
    let dev = &t.cb_deviations_us;
    let (strip, over) = bipolar_braille_strip(dev, cols, chart_h, full_scale);

    // The samples actually shown (the last two per column), for per-column colour.
    let window = &dev[dev.len().saturating_sub(SAMPLES_PER_COL * cols)..];
    let columns = Columns::of(window, cols, &over, t.deadline_budget_us);

    let (band_top, band_bot) = scale::band_rows(chart_h, t.deadline_budget_us, full_scale);
    let label_rows = scale::label_rows(chart_h);
    let last_row = chart_h - 1;
    let lbl = Style::default().fg(theme.label);
    let dim = Style::default().fg(theme.border_dim);

    let mut out: Vec<Line> = Vec::with_capacity(chart_h);
    for (r, row_str) in strip.iter().enumerate() {
        let label = label_rows
            .contains(&r)
            .then(|| scale::fmt_axis(scale::axis_value_us(r, chart_h, full_scale)));
        let mut spans = vec![Span::styled(scale::gutter_label(label), lbl)];
        for (c, ch) in row_str.chars().enumerate() {
            let sign = columns.over_sign(c);
            if r == 0 && sign > 0 {
                // A spike that ran off the top: tag it where it left.
                spans.push(Span::styled(
                    "\u{25B2}".to_string(),
                    Style::default().fg(theme.status_crit),
                ));
            } else if r == last_row && sign < 0 {
                spans.push(Span::styled(
                    "\u{25BC}".to_string(),
                    Style::default().fg(theme.status_crit),
                ));
            } else if ch == BLANK {
                // Empty cell: the deadline guide shows through on the band rows.
                if r == band_top || r == band_bot {
                    spans.push(Span::styled("\u{2504}".to_string(), dim));
                } else {
                    spans.push(Span::styled(BLANK.to_string(), dim));
                }
            } else {
                let color = columns
                    .severity
                    .get(c)
                    .copied()
                    .flatten()
                    .map(|s| severity_color(s, theme))
                    .unwrap_or(theme.value);
                spans.push(Span::styled(ch.to_string(), Style::default().fg(color)));
            }
        }
        out.push(Line::from(spans));
    }
    f.render_widget(Paragraph::new(out), area);
}

/// Nothing to plot yet. Centred vertically so it does not read as a data row.
fn placeholder(f: &mut Frame, area: Rect, stale: bool, theme: &crate::Theme) {
    let msg = if stale {
        "\u{25cb} IDLE \u{2014} RX stopped"
    } else {
        "waiting for callbacks\u{2026}"
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(" "),
            Span::styled(msg, Style::default().fg(theme.stale)),
        ])),
        Rect {
            y: area.y + area.height / 2,
            height: 1,
            ..area
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The window legend and the plot must agree about how much is on screen.
    #[test]
    fn shown_samples_is_two_per_plot_column() {
        assert_eq!(shown_samples(GUTTER_W as u16 + 10, 1_000), 20);
        // Never more than there is.
        assert_eq!(shown_samples(GUTTER_W as u16 + 10, 7), 7);
        // A chart narrower than its own gutter shows nothing.
        assert_eq!(shown_samples(3, 1_000), 0);
    }

    /// A column carries two callbacks, and the worse of the pair decides its
    /// colour - a cell holding one late callback is late.
    #[test]
    fn a_column_takes_the_worse_of_its_two_samples() {
        let window = [10i32, 5_000, 20, 30];
        let c = Columns::of(&window, 2, &[], 600);
        assert_eq!(c.severity[0], Some(2), "the 5000 µs sample must win");
        assert_eq!(c.severity[1], Some(0));
    }

    /// Columns past the end of a short series have no reading at all, rather than
    /// a fabricated in-budget one.
    #[test]
    fn columns_past_the_data_have_no_severity() {
        let c = Columns::of(&[10i32, 20], 4, &[], 600);
        assert_eq!(c.severity[0], Some(0));
        assert_eq!(c.severity[1], None);
        assert_eq!(c.severity[3], None);
    }

    #[test]
    fn only_over_range_columns_get_a_tag() {
        let window = [10i32, 20, -9_000, 5];
        let c = Columns::of(&window, 2, &[1], 600);
        assert_eq!(c.over_sign(0), 0, "an in-range column is untagged");
        assert_eq!(c.over_sign(1), -1, "an early spike tags downward");
        assert_eq!(c.over_sign(99), 0, "out of range is untagged, not a panic");
    }
}
