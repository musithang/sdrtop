//! The dBFS colour-scale gutter down the left edge and the focus-mode readout
//! row along the bottom.
//!
//! The gutter is six columns wide to match the spectrum's dB labels exactly, so
//! the two panels share an x-axis and the bonded ruler lines up with both.

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
use crate::state::SdrMetrics;
use crate::ui::panels::core::spectrum::fmt_spectrum_step;

use super::cells::{band_max, Columns, DB_MAX};

/// Width of the dB gutter. Matches the spectrum's label column so the two plots
/// start at the same x.
pub(super) const DB_COL: u16 = 6;

/// The colour scale, painted as half-blocks like the grid it explains, with the
/// top, middle and bottom values labelled. Tracks the live `db_min` floor.
pub(super) fn db_legend(
    f: &mut Frame, area: Rect, db_min: f32, palette: WaterfallPalette, theme: &crate::Theme,
) {
    let h = area.height as usize;
    if h == 0 { return; }
    let depth = ColorDepth::detect();
    let steps = (h * 2).max(2);
    let at = |t: f32| magnitude_to_color_palette(
        DB_MAX + (db_min - DB_MAX) * t, db_min, DB_MAX, depth, theme, palette);

    let legend: Vec<Line> = (0..h).map(|row| {
        let top = at((row * 2) as f32 / (steps - 1) as f32);
        let bot = at((row * 2 + 1) as f32 / (steps - 1) as f32);
        let label = match row {
            0 => format!("{:>+4} ", DB_MAX as i32),
            r if r == h.saturating_sub(1) => format!("{:>4} ", db_min as i32),
            r if r == h / 2 => format!("{:>4} ", ((DB_MAX + db_min) / 2.0) as i32),
            _ => "     ".to_string(),
        };
        Line::from(vec![
            Span::styled("\u{2580}", Style::default().fg(top).bg(bot)),
            Span::styled(label, Style::default().fg(theme.value)),
        ])
    }).collect();
    f.render_widget(Paragraph::new(legend), area);
}

/// The focus-mode readout: what the cursor is sitting on, or the keys available
/// when there is no cursor.
#[allow(clippy::too_many_arguments)]
pub(super) fn indicator(
    f: &mut Frame, area: Rect, state: &SdrMetrics,
    rows: &VecDeque<(Instant, Arc<Vec<f32>>)>, columns: &Columns,
    skip_data: usize, cursor_col: Option<usize>, stride: usize, theme: &crate::Theme,
) {
    let text = match (state.waterfall.cursor_freq, cursor_col) {
        (Some(cf), Some(col)) => cursor_readout(cf, col, rows, columns, skip_data),
        // A cursor set outside the current zoom window: name it, but there is
        // nothing on screen to read a level from.
        (Some(cf), None) => format!("  cur: {:.3} MHz  \u{2190} \u{2192}  M", cf as f64 / 1e6),
        (None, _) => format!(
            "  \u{00D7}{}  frames/row  [ ]  M cursor  step {}  \u{2191}\u{2193} zoom  J/K scroll",
            stride, fmt_spectrum_step(state.spectrum.step_hz),
        ),
    };

    let dashes = (area.width as usize).saturating_sub(text.chars().count());
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("\u{2500}".repeat(dashes), Style::default().fg(theme.border_dim)),
            Span::styled(text, Style::default().fg(theme.label)),
        ])),
        area,
    );
}

/// Frequency, level and age at the cursor, read from the newest visible row.
fn cursor_readout(
    freq_hz: u64, col: usize,
    rows: &VecDeque<(Instant, Arc<Vec<f32>>)>, columns: &Columns, skip_data: usize,
) -> String {
    let mhz = freq_hz as f64 / 1e6;
    let Some((ts, row)) = rows.get(skip_data) else {
        return format!("  cur: {mhz:.3} MHz  \u{2190} \u{2192}  M");
    };
    let (lo, hi) = columns.range(col);
    let db = band_max(row, lo, hi);
    if db.is_finite() {
        format!("  cur: {mhz:.3} MHz  {db:.1} dBFS  {}s ago  \u{2190} \u{2192}  M", ts.elapsed().as_secs())
    } else {
        format!("  cur: {mhz:.3} MHz  \u{2190} \u{2192}  M")
    }
}
