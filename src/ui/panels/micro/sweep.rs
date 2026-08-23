//! `micro_sweep` — the field scanner view (`[0]` cycle, sweep step).
//!
//! A compact glance at the running sweep: band range and cycle progress, plus the
//! strongest signals found, each tagged with its band-plan name. Entering this
//! micro view starts the sweep (the draw loop flags `sweep.active`), so it is
//! self-contained for field use.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::state::SdrMetrics;
use crate::ui::widgets::band_plan::band_at;
use crate::ui::widgets::micro_common::bar_spans;
use crate::ui::panel::{Panel, PanelChrome};

pub struct MicroSweepPanel;

/// Minimum spacing between listed peaks (Hz) so one wide signal doesn't dominate.
const PEAK_SPACING_HZ: u64 = 1_000_000;

impl Panel for MicroSweepPanel {
    fn name(&self) -> &'static str { "micro_sweep_panel" }
    fn min_size(&self) -> (u16, u16) { (40, 8) }

    fn chrome(&self, _state: &SdrMetrics) -> PanelChrome { PanelChrome::untitled() }

    fn render(&self, f: &mut Frame, inner: Rect, state: &SdrMetrics, theme: &crate::Theme, _focused: bool) {
        let sw = &state.sweep;

        let lbl = |s: String| Span::styled(s, Style::default().fg(theme.label));

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // 0 header
                Constraint::Length(1), // 1 progress
                Constraint::Length(1), // 2 blank
                Constraint::Length(1), // 3 TOP SIGNALS label
                Constraint::Min(0),    // 4.. peaks
            ])
            .split(inner);

        // Header: SWEEP badge + band range + cycle.
        f.render_widget(Paragraph::new(Line::from(vec![
            Span::raw(" "),
            Span::styled("◉ SWEEP", Style::default().fg(theme.status_ok).add_modifier(Modifier::BOLD)),
            Span::raw("  "),
            Span::styled(
                format!("{:.0}–{:.0} MHz", sw.config.start_hz as f64 / 1e6, sw.config.stop_hz as f64 / 1e6),
                Style::default().fg(theme.value_hi),
            ),
            Span::raw("   "),
            lbl(format!("cycle #{}", sw.cycle_count)),
        ])), rows[0]);

        // Progress bar across the current cycle's positions.
        let ratio = if sw.positions_total > 0 {
            sw.positions_done as f64 / sw.positions_total as f64
        } else { 0.0 };
        let bar_w = (inner.width.saturating_sub(14)).min(20) as usize;
        let [filled, empty] = bar_spans(ratio, bar_w, theme.value, theme);
        f.render_widget(Paragraph::new(Line::from(vec![
            Span::raw(" "), lbl(format!("{:<5}", "pos")),
            filled, empty,
            Span::styled(format!("  {}/{}", sw.positions_done, sw.positions_total), Style::default().fg(theme.value)),
        ])), rows[1]);

        // Top signals list.
        f.render_widget(Paragraph::new(Line::from(vec![
            Span::raw(" "),
            Span::styled("TOP SIGNALS", Style::default().fg(theme.value_hi).add_modifier(Modifier::BOLD)),
        ])), rows[3]);

        let list_area = rows[4];
        let Some(frame) = sw.current_frame.as_ref() else {
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    "  scanning…", Style::default().fg(theme.stale),
                ))),
                list_area,
            );
            return;
        };

        let max_rows = list_area.height as usize;
        let peaks = frame.top_peaks(max_rows.min(5), PEAK_SPACING_HZ);
        let lines: Vec<Line> = peaks.iter().enumerate().map(|(i, &(hz, db))| {
            let band = band_at(hz).map(|b| format!("  [{}]", b)).unwrap_or_default();
            Line::from(vec![
                Span::styled(format!(" {}  ", i + 1), Style::default().fg(theme.label)),
                Span::styled(format!("{:>11.3} MHz", hz as f64 / 1e6), Style::default().fg(theme.value_hi).add_modifier(Modifier::BOLD)),
                Span::styled(format!("  {:>6.1} dBFS", db), Style::default().fg(theme.value)),
                Span::styled(band, Style::default().fg(theme.status_ok)),
            ])
        }).collect();
        let lines = if lines.is_empty() {
            vec![Line::from(Span::styled("  no signals above noise", Style::default().fg(theme.stale)))]
        } else { lines };
        f.render_widget(Paragraph::new(lines), list_area);
    }
}
