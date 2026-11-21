// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::ui::panel::{Panel, PanelChrome, Staleness};
// `snr_color` shared rather than re-declared: one measurement, one scale.
// See `state::SAT_WARN_PCT` for what a second copy costs.
use crate::ui::widgets::micro_common::snr_color;

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

    fn chrome(&self, _state: &crate::ui::View<'_>) -> PanelChrome {
        PanelChrome::new("Sig_nal Metrics").stale_when(Staleness::FftAge)
    }

    fn render(
        &self,
        f: &mut Frame,
        inner: ratatui::layout::Rect,
        state: &crate::ui::View<'_>,
        theme: &crate::Theme,
        _focused: bool,
    ) {
        let lbl = Style::default().fg(theme.label);
        let val = Style::default().fg(theme.value);

        // The figures as the engine reads them, which is what the API serves
        // too: the frame taken at the last quarter-second tick, channel power
        // as the S-meter reads it, the carrier's occupied bandwidth. Whether
        // each can be shown is the engine's decision, not this panel's.
        let v = crate::engine::view::signal_readings(state);
        let snr = v.peak_to_nf_db.value().copied();
        let shown = |r: Option<String>| r.unwrap_or_else(|| "---".into());

        let rows: &[Line] = &[
            Line::from(vec![
                Span::styled(format!("{:<15}", "Peak/NF"), lbl),
                Span::styled(
                    shown(snr.map(|s| format!("{s:.1} dB"))),
                    Style::default().fg(snr.map_or(theme.label, |s| snr_color(s, theme))),
                ),
            ]),
            Line::from(vec![
                Span::styled(format!("{:<15}", "Channel power"), lbl),
                Span::styled(
                    shown(
                        v.channel_power_dbfs
                            .value()
                            .map(|p| format!("~{p:.1} dBFS")),
                    ),
                    val,
                ),
            ]),
            Line::from(vec![
                Span::styled(format!("{:<15}", "Occupied BW"), lbl),
                Span::styled(
                    shown(
                        v.occupied_bw_hz
                            .value()
                            .map(|&bw| crate::ui::widgets::micro_common::fmt_bw(bw)),
                    ),
                    val,
                ),
            ]),
            Line::from(vec![
                Span::styled(format!("{:<15}", "Noise floor"), lbl),
                Span::styled(
                    shown(v.noise_floor_dbfs.value().map(|n| format!("{n:.1} dBFS"))),
                    val,
                ),
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
    use crate::state::SdrMetrics;

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
        let text = crate::ui::fixture::draw(SignalMetricsPanel, 40, 10, &m).join("\n");
        assert!(text.contains("40.0 dB"), "{text}");
        assert!(text.contains("~-61.2 dBFS"), "{text}");
        assert!(text.contains("-100.0 dBFS"), "{text}");
    }

    /// The panel dashes a figure exactly where the engine's view has none:
    /// what the screen refuses, the API refuses, in every state the panel
    /// can be drawn in.
    #[test]
    fn the_panel_dashes_exactly_what_the_view_leaves_absent() {
        let states = [
            ("open, not streaming", SdrMetrics::fixture()),
            ("streaming, no carrier", SdrMetrics::fixture().streaming()),
            (
                "a carrier",
                SdrMetrics::fixture().streaming().with_carrier(0.0, 40.0),
            ),
            (
                "a carrier, the spectrum stale",
                SdrMetrics::fixture()
                    .streaming()
                    .with_carrier(0.0, 40.0)
                    .with_stale_fft(),
            ),
        ];
        for (what, m) in states {
            let v = crate::engine::view::signal_readings(&m);
            let lines = crate::ui::fixture::draw(SignalMetricsPanel, 40, 10, &m);
            let rows = [
                ("Peak/NF", v.peak_to_nf_db.absent().is_some()),
                ("Channel power", v.channel_power_dbfs.absent().is_some()),
                ("Occupied BW", v.occupied_bw_hz.absent().is_some()),
                ("Noise floor", v.noise_floor_dbfs.absent().is_some()),
            ];
            for (label, absent) in rows {
                let line = lines
                    .iter()
                    .find(|l| l.contains(label))
                    .unwrap_or_else(|| panic!("{what}: no {label} row"));
                assert_eq!(
                    line.contains("---"),
                    absent,
                    "{what}: {label} reads {line:?}, the view says absent={absent}"
                );
            }
        }
    }
}
