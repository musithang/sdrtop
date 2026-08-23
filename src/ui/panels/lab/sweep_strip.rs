//! `sweep_strip` — a one-line status bar for the `lab_sweep` preset, mirroring
//! `signal_strip`: sweep badge, band, progress, cycle info, and the cursor
//! readout with band-plan identification.

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::state::SdrMetrics;
use crate::ui::widgets::band_plan::band_at;
use crate::ui::panel::{Panel, PanelChrome};

pub struct SweepStripPanel;

impl Panel for SweepStripPanel {
    fn name(&self) -> &'static str { "sweep_strip" }
    fn min_size(&self) -> (u16, u16) { (40, 3) }
    fn preferred_height(&self, _w: u16, _s: &SdrMetrics) -> u16 { 3 }

    fn chrome(&self, _state: &SdrMetrics) -> PanelChrome { PanelChrome::untitled() }

    fn render(&self, f: &mut Frame, inner: Rect, state: &SdrMetrics, theme: &crate::Theme, _focused: bool) {
        let sw = &state.sweep;
        if inner.width == 0 || inner.height == 0 { return; }

        let sep = || Span::styled("  ·  ", Style::default().fg(theme.border_dim));
        let mut spans = vec![
            Span::raw(" "),
            Span::styled("SWEEP", Style::default().fg(theme.status_ok).add_modifier(Modifier::BOLD)),
            Span::raw("  "),
            Span::styled(
                format!("{:.0}–{:.0} MHz", sw.config.start_hz as f64 / 1e6, sw.config.stop_hz as f64 / 1e6),
                Style::default().fg(theme.value),
            ),
            sep(),
            Span::styled("pos ", Style::default().fg(theme.label)),
            Span::styled(format!("{}/{}", sw.positions_done, sw.positions_total), Style::default().fg(theme.value)),
            sep(),
            Span::styled(format!("cycle #{}", sw.cycle_count), Style::default().fg(theme.value)),
            sep(),
            Span::styled(format!("{:.1}s/cycle", sw.cycle_duration_ms as f64 / 1000.0), Style::default().fg(theme.value)),
        ];

        // Cursor readout, when set in the panel's focus mode.
        if let (Some(frac), Some(frame)) = (sw.cursor_frac, sw.current_frame.as_ref()) {
            let hz = frame.freq_at_fraction(frac);
            spans.push(sep());
            spans.push(Span::styled("cursor: ", Style::default().fg(theme.label)));
            spans.push(Span::styled(format!("{:.3} MHz", hz as f64 / 1e6), Style::default().fg(theme.value_hi).add_modifier(Modifier::BOLD)));
            if let Some(band) = band_at(hz) {
                spans.push(Span::styled(format!("  [{}]", band), Style::default().fg(theme.status_ok)));
            }
        }

        f.render_widget(Paragraph::new(Line::from(spans)), inner);
    }
}
