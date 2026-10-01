// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::state::SdrMetrics;
use crate::ui::chrome::section;
use crate::ui::panel::{Panel, PanelChrome, Staleness};

pub struct AdsbFramesPanel;

impl Panel for AdsbFramesPanel {
    fn name(&self) -> &'static str {
        "adsb_frames"
    }

    fn min_size(&self) -> (u16, u16) {
        (36, 8)
    }

    fn chrome(&self, _state: &SdrMetrics) -> PanelChrome {
        PanelChrome::new("Mode S / ADS-B").stale_when(Staleness::NotStreaming)
    }

    fn render(
        &self,
        f: &mut Frame,
        inner: Rect,
        state: &SdrMetrics,
        theme: &crate::Theme,
        _focused: bool,
    ) {
        let width = inner.width as usize;
        let mut lines = vec![section("valid frames", "CRC · LO/DC tracked", width, theme)];
        let adsb = &state.adsb;
        lines.push(Line::from(Span::styled(
            format!(
                "Tuned {:.3} MHz · {:.3} MS/s",
                adsb.tuned_hz as f64 / 1e6,
                adsb.sample_rate_hz / 1e6
            ),
            Style::default().fg(theme.label),
        )));

        if !adsb.rate_supported {
            lines.push(Line::from(Span::styled(
                format!(
                    "Set rate to 2.4 MS/s (RTL) or 6.0 MS/s (HackRF); now {:.3}",
                    adsb.sample_rate_hz / 1e6
                ),
                Style::default().fg(theme.status_warn),
            )));
        } else if adsb.frames.is_empty() {
            lines.push(Line::from(Span::styled(
                format!(
                    "Searching at 1090 MHz; no valid frame yet{}",
                    adsb.last_frame_at
                        .map(|at| format!(" · last {}s ago", at.elapsed().as_secs()))
                        .unwrap_or_default()
                ),
                Style::default().fg(theme.stale),
            )));
        } else {
            lines.push(Line::from(Span::styled(
                format!("{} valid frames this session", adsb.frames_session),
                Style::default().fg(theme.status_ok),
            )));
            for frame in adsb
                .frames
                .iter()
                .rev()
                .take(inner.height.saturating_sub(3) as usize)
            {
                let address = frame
                    .icao_address
                    .map(|address| format!("{address:06X}"))
                    .unwrap_or_else(|| "------".to_string());
                lines.push(Line::from(vec![
                    Span::styled(
                        address,
                        Style::default()
                            .fg(theme.value_hi)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!(" DF{} ", frame.downlink_format),
                        Style::default().fg(theme.label),
                    ),
                    Span::styled(
                        format!("{:5.1} dBFS ", frame.signal_dbfs),
                        Style::default().fg(theme.value),
                    ),
                    Span::raw(frame.summary.clone()),
                ]));
            }
        }

        f.render_widget(Paragraph::new(lines), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::fixture::draw;
    use crate::state::{AdsbFrameEntry, SdrMetrics};
    use std::time::Instant;

    #[test]
    fn the_panel_explains_the_required_rtl_sample_rate() {
        let state = SdrMetrics::fixture().streaming();
        let out = draw(AdsbFramesPanel, 54, 12, &state).join("\n");
        assert!(
            out.contains("Set rate to 2.4 MS/s (RTL) or 6.0 MS/s (HackRF)"),
            "{out}"
        );
    }

    #[test]
    fn the_panel_lists_only_recorded_crc_valid_frames() {
        let mut state = SdrMetrics::fixture().streaming();
        state.adsb.rate_supported = true;
        state.adsb.tuned_hz = crate::signal::adsb::CENTER_FREQUENCY_HZ;
        state.adsb.sample_rate_hz = crate::signal::adsb::RTL_SAMPLE_RATE_HZ;
        state.adsb.push_frame(AdsbFrameEntry {
            received_at: Instant::now(),
            icao_address: Some(0x4b_b463),
            downlink_format: 17,
            signal_dbfs: -42.0,
            summary: "Extended Squitter Airborne position".into(),
        });

        let out = draw(AdsbFramesPanel, 72, 12, &state).join("\n");
        assert!(out.contains("4BB463"), "{out}");
        assert!(out.contains("DF17"), "{out}");
        assert!(out.contains("Extended Squitter"), "{out}");
    }
}
