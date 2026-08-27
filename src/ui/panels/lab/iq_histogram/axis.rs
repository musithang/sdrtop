//! The zone label row under the chart: `0───`, a centred `── OK ──`, and `clip`.
//!
//! The three widths are the three zones' share of the chart, so the labels sit
//! under the columns they describe.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use super::zones::{CLIP_BINS, LOW_BINS};

const MID_LABEL: &str = "\u{2500}\u{2500} OK \u{2500}\u{2500}";

pub(super) fn draw(f: &mut Frame, area: Rect, chart_w: u16, n_bins: usize, theme: &crate::Theme) {
    let n_bins = n_bins.max(1);
    let low_cols = (chart_w as usize * LOW_BINS / n_bins).max(1);
    let clip_cols = (chart_w as usize * CLIP_BINS / n_bins).max(1);
    let mid_cols = (chart_w as usize)
        .saturating_sub(low_cols + clip_cols)
        .max(1);

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(low_cols as u16),
            Constraint::Length(mid_cols as u16),
            Constraint::Min(0),
        ])
        .split(area);

    let dim = Style::default().fg(theme.label);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("0", dim),
            Span::styled("\u{2500}".repeat(low_cols.saturating_sub(1)), dim),
        ])),
        cols[0],
    );
    f.render_widget(
        Paragraph::new(Span::styled(
            // Pad by `chars().count()`, not `len()`: '─' is three bytes in UTF-8,
            // so a byte-width format would over-pad by eight and push the label
            // off the right edge of a narrow panel.
            format!(
                "{:>width$}",
                MID_LABEL,
                width = cols[1].width as usize / 2 + MID_LABEL.chars().count()
            ),
            Style::default().fg(theme.status_ok),
        )),
        cols[1],
    );
    f.render_widget(
        Paragraph::new(Span::styled("clip", Style::default().fg(theme.status_crit))),
        cols[2],
    );
}
