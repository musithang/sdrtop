use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::state::SdrMetrics;
use crate::ui::panel::{Panel, PanelChrome, Staleness};
// `snr_color` shared rather than re-declared: one measurement, one scale.
// See `state::SAT_WARN_PCT` for what a second copy costs.
use crate::ui::widgets::micro_common::{fft_stale, snr_color};

pub struct SignalMetricsPanel;

impl Panel for SignalMetricsPanel {
    fn name(&self) -> &'static str {
        "signal_metrics"
    }
    fn min_size(&self) -> (u16, u16) {
        (32, 6)
    }
    fn focus_key(&self) -> Option<char> {
        Some('n')
    }
    fn focus_bindings(&self) -> &'static [(&'static str, &'static str)] {
        &[("C", "Snapshot to log")]
    }

    fn chrome(&self, _state: &SdrMetrics) -> PanelChrome {
        PanelChrome::new("Sig_nal Metrics").stale_when(Staleness::FftAge)
    }

    fn render(
        &self,
        f: &mut Frame,
        inner: ratatui::layout::Rect,
        state: &SdrMetrics,
        theme: &crate::Theme,
        _focused: bool,
    ) {
        let stale = fft_stale(state);

        let lbl = Style::default().fg(theme.label);
        let val = Style::default().fg(theme.value);

        let noise_str = state
            .waterfall
            .last_fft
            .as_ref()
            .map(|fr| format!("{:.1} dBFS", fr.noise_floor))
            .unwrap_or_else(|| "---".into());

        let rows: &[Line] = &[
            Line::from(vec![
                Span::styled(format!("{:<15}", "Peak/NF"), lbl),
                Span::styled(
                    if stale {
                        "---".into()
                    } else {
                        format!("{:.1} dB", state.signal.peak_to_nf_db)
                    },
                    Style::default().fg(if stale {
                        theme.label
                    } else {
                        snr_color(state.signal.peak_to_nf_db, theme)
                    }),
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
                    if state.signal.occupied_bw_hz > 0 {
                        crate::ui::widgets::micro_common::fmt_bw(state.signal.occupied_bw_hz)
                    } else {
                        "---".into()
                    },
                    val,
                ),
            ]),
            Line::from(vec![
                Span::styled(format!("{:<15}", "Noise floor"), lbl),
                Span::styled(noise_str, val),
            ]),
        ];

        let n = rows.len().min(inner.height as usize);
        if n == 0 {
            return;
        }
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
