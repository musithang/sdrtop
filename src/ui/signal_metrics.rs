use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};

use crate::state::SdrMetrics;
use crate::ui::panel::Panel;

pub struct SignalMetricsPanel;

fn snr_color(snr: f32, theme: &crate::Theme) -> Color {
    if snr >= 20.0      { theme.status_ok   }
    else if snr >= 10.0 { theme.status_warn }
    else                { theme.status_crit }
}

impl Panel for SignalMetricsPanel {
    fn name(&self) -> &'static str { "signal_metrics" }
    fn min_size(&self) -> (u16, u16) { (32, 6) }
    fn focus_key(&self) -> Option<char> { Some('n') }
    fn focus_bindings(&self) -> &'static [(&'static str, &'static str)] {
        &[("C", "Snapshot to log")]
    }

    fn render(&self, f: &mut Frame, area: ratatui::layout::Rect, state: &SdrMetrics, theme: &crate::Theme, focused: bool) {
        let stale = state.waterfall.last_fft.as_ref()
            .map(|fr| fr.timestamp.elapsed().as_millis() > 500)
            .unwrap_or(true);

        let title = if stale { " Signal Metrics [STALE] " } else { " Signal Metrics " };
        let border_color = if focused { theme.border_focused }
            else if stale { theme.stale }
            else { theme.border_default };
        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(border_color));
        let inner = block.inner(area);
        f.render_widget(block, area);

        let lbl = Style::default().fg(theme.label);
        let val = Style::default().fg(theme.value);

        let noise_str = state.waterfall.last_fft.as_ref()
            .map(|fr| format!("{:.1} dBFS", fr.noise_floor))
            .unwrap_or_else(|| "---".into());

        let rows: &[Line] = &[
            Line::from(vec![
                Span::styled(format!("{:<15}", "Peak/NF"), lbl),
                Span::styled(
                    if stale { "---".into() } else { format!("{:.1} dB", state.signal.peak_to_nf_db) },
                    Style::default().fg(if stale { theme.label } else { snr_color(state.signal.peak_to_nf_db, theme) }),
                ),
            ]),
            Line::from(vec![
                Span::styled(format!("{:<15}", "Channel power"), lbl),
                Span::styled(
                    if state.signal.channel_power_dbfs.is_finite() {
                        format!("{:.1} dBFS", state.signal.channel_power_dbfs)
                    } else {
                        "---".into()
                    },
                    val,
                ),
            ]),
            Line::from(vec![
                Span::styled(format!("{:<15}", "Occupied BW"), lbl),
                Span::styled(
                    if state.signal.occupied_bw_hz > 0 { crate::ui::micro_common::fmt_bw(state.signal.occupied_bw_hz) } else { "---".into() },
                    val,
                ),
            ]),
            Line::from(vec![
                Span::styled(format!("{:<15}", "Noise floor"), lbl),
                Span::styled(noise_str, val),
            ]),
        ];

        let n = rows.len().min(inner.height as usize);
        if n == 0 { return; }
        let constraints: Vec<Constraint> = (0..n).map(|_| Constraint::Length(1)).collect();
        let row_areas = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(inner);

        for (i, line) in rows.iter().take(n).enumerate() {
            f.render_widget(Paragraph::new(line.clone()), row_areas[i]);
        }
    }
}
