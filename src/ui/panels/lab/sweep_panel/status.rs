//! The bottom row: what the cursor is on, or how the scan is going.
//!
//! One row, two mutually exclusive readouts. With a cursor placed the row belongs
//! to the cursor, because that is what the operator just asked about; without
//! one it reports the cycle.

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::state::{SdrMetrics, SweepFrame};
use crate::ui::widgets::band_plan::band_at;

use super::envelope::Envelope;
use super::scale::cursor_bucket;

pub(super) fn line(
    state: &SdrMetrics,
    frame: &SweepFrame,
    env: &Envelope,
    theme: &crate::Theme,
) -> Line<'static> {
    match state.sweep.cursor_frac {
        Some(frac) => cursor(frac, frame, env, theme),
        None => cycle(state, frame, theme),
    }
}

fn cursor(frac: f64, frame: &SweepFrame, env: &Envelope, theme: &crate::Theme) -> Line<'static> {
    let hz = frame.freq_at_fraction(frac);
    // A bucket the sweep never reached reads as a dash, not as the window floor:
    // "no measurement here" and "−100 dBFS here" are different answers.
    let level = match env.level_at(cursor_bucket(frac, env.len())) {
        Some(v) => format!("{v:.1} dBFS"),
        None => "\u{2014}".to_string(),
    };
    let band = band_at(hz).map(|b| format!("  [{b}]")).unwrap_or_default();
    Line::from(vec![
        Span::styled(" Cursor ", Style::default().fg(theme.label)),
        Span::styled(
            format!("{:.3} MHz", hz as f64 / 1e6),
            Style::default()
                .fg(theme.value_hi)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(level, Style::default().fg(theme.value)),
        Span::styled(band, Style::default().fg(theme.status_ok)),
    ])
}

fn cycle(state: &SdrMetrics, frame: &SweepFrame, theme: &crate::Theme) -> Line<'static> {
    let sw = &state.sweep;
    Line::from(vec![
        Span::styled(" pos ", Style::default().fg(theme.label)),
        Span::styled(
            format!("{}/{}", sw.positions_done, sw.positions_total),
            Style::default().fg(theme.value),
        ),
        Span::styled("  \u{00b7}  cycle ", Style::default().fg(theme.label)),
        Span::styled(
            format!(
                "#{} ({:.1}s)",
                frame.cycle_count,
                frame.cycle_duration_ms as f64 / 1000.0
            ),
            Style::default().fg(theme.value),
        ),
        Span::styled("  \u{00b7}  ", Style::default().fg(theme.label)),
        Span::styled(
            if sw.show_peak { "PEAK" } else { "MEAN" },
            Style::default().fg(theme.value_hi),
        ),
        Span::styled(
            format!(
                "  \u{00b7}  {:.0}s ago",
                frame.timestamp.elapsed().as_secs_f64()
            ),
            Style::default().fg(theme.stale),
        ),
        Span::styled(
            "  \u{00b7}  focus [G] for cursor",
            Style::default().fg(theme.stale),
        ),
    ])
}
