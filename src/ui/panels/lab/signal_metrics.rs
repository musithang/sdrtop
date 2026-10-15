// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

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

        // The figures as the Lab Signal bench prints them: the frame taken at
        // the last quarter-second tick, and channel power as the S-meter reads
        // it, so a number changes slowly enough to be read and the two
        // benches never disagree.
        let frame = state.signal.readout.as_ref().filter(|_| !stale);
        let noise_str = frame
            .map(|fr| format!("{:.1} dBFS", fr.noise_floor))
            .unwrap_or_else(|| "---".into());
        let snr = frame.map(|fr| fr.peak_to_nf_db);

        let rows: &[Line] = &[
            Line::from(vec![
                Span::styled(format!("{:<15}", "Peak/NF"), lbl),
                Span::styled(
                    snr.map_or_else(|| "---".into(), |s| format!("{s:.1} dB")),
                    Style::default().fg(snr.map_or(theme.label, |s| snr_color(s, theme))),
                ),
            ]),
            Line::from(vec![
                Span::styled(format!("{:<15}", "Channel power"), lbl),
                Span::styled(
                    match state
                        .signal
                        .s_meter
                        .filter(|s| !stale && s.readout.is_finite())
                    {
                        Some(s) => format!("~{:.1} dBFS", s.readout),
                        None => "---".into(),
                    },
                    val,
                ),
            ]),
            Line::from(vec![
                Span::styled(format!("{:<15}", "Occupied BW"), lbl),
                Span::styled(
                    // The carrier's: a gap between bursts occupies the span.
                    if let Some(c) = state.signal.carrier.filter(|c| c.occupied_bw_hz > 0) {
                        crate::ui::widgets::micro_common::fmt_bw(c.occupied_bw_hz)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The channel figures are the quarter-second frame's and the S-meter's
    /// readout, as on the Lab Signal bench: one account for one quantity.
    #[test]
    fn the_figures_are_the_paced_ones() {
        let mut m = SdrMetrics::fixture().streaming().with_carrier(0.0, 40.0);
        m.signal.peak_to_nf_db = 4.0;
        if let Some(fr) = m.waterfall.last_fft.as_mut() {
            fr.noise_floor = -95.0;
        }
        if let Some(s) = m.signal.s_meter.as_mut() {
            s.readout = -61.2;
        }
        let text = crate::state::fixture::draw(SignalMetricsPanel, 40, 10, &m).join("\n");
        assert!(text.contains("40.0 dB"), "{text}");
        assert!(text.contains("~-61.2 dBFS"), "{text}");
        assert!(text.contains("-100.0 dBFS"), "{text}");
    }
}
