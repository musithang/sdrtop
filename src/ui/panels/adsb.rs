// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::state::{AdsbFrameEntry, SdrMetrics};
use crate::ui::chrome::section;
use crate::ui::panel::{Panel, PanelChrome, Staleness};

pub struct AdsbFramesPanel;

/// One frame as a line of columns. Every field is optional and an absent one
/// is a blank, never a zero: a position frame has no callsign and a callsign
/// frame has no altitude, and printing `0` for either would be a claim the
/// frame never made.
fn frame_line(frame: &AdsbFrameEntry, theme: &crate::Theme) -> Line<'static> {
    let address = frame
        .icao_address
        .map(|address| format!("{address:06X}"))
        .unwrap_or_else(|| "------".to_string());
    let details = &frame.details;

    let callsign = details
        .callsign
        .clone()
        .unwrap_or_else(|| "--------".to_string());
    let altitude = details
        .altitude_ft
        .map(|feet| format!("{feet:>6} ft"))
        .unwrap_or_else(|| "        ".to_string());
    let speed = details
        .groundspeed_kt
        .map(|knots| format!("{knots:>4.0} kt"))
        .unwrap_or_else(|| "       ".to_string());
    let track = details
        .track_deg
        .map(|degrees| format!("{degrees:>3.0}°"))
        .unwrap_or_else(|| "    ".to_string());
    let vertical = details
        .vertical_rate_fpm
        .map(|rate| format!("{rate:>+6} fpm"))
        .unwrap_or_else(|| "         ".to_string());
    let position = match (details.latitude, details.longitude) {
        (Some(latitude), Some(longitude)) => format!("{latitude:>8.4} {longitude:>9.4}"),
        _ => "                 ".to_string(),
    };

    Line::from(vec![
        Span::styled(
            address,
            Style::default()
                .fg(theme.value_hi)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" DF{:<2} ", frame.downlink_format),
            Style::default().fg(theme.label),
        ),
        Span::styled(
            format!("{:5.1} dBFS ", frame.signal_dbfs),
            Style::default().fg(theme.value),
        ),
        Span::styled(callsign, Style::default().fg(theme.value_hi)),
        Span::styled(altitude, Style::default().fg(theme.value)),
        Span::styled(speed, Style::default().fg(theme.value)),
        Span::styled(track, Style::default().fg(theme.label)),
        Span::styled(vertical, Style::default().fg(theme.value)),
        Span::styled(position, Style::default().fg(theme.label)),
    ])
}

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
            lines.push(Line::from(Span::styled(
                "ICAO   DF   level      callsign    alt      spd   trk    vrate    position",
                Style::default().fg(theme.label),
            )));
            for frame in adsb
                .frames
                .iter()
                .rev()
                .take(inner.height.saturating_sub(4) as usize)
            {
                lines.push(frame_line(frame, theme));
            }
        }

        f.render_widget(Paragraph::new(lines), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::adsb::AdsbDetails;
    use crate::state::fixture::draw;
    use crate::state::{AdsbFrameEntry, SdrMetrics};
    use std::time::Instant;

    fn entry(details: AdsbDetails) -> AdsbFrameEntry {
        AdsbFrameEntry {
            received_at: Instant::now(),
            icao_address: Some(0x4b_b463),
            downlink_format: 17,
            signal_dbfs: -42.0,
            details,
        }
    }

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
        state.adsb.push_frame(entry(AdsbDetails::default()));

        let out = draw(AdsbFramesPanel, 72, 12, &state).join("\n");
        assert!(out.contains("4BB463"), "{out}");
        assert!(out.contains("DF17"), "{out}");
    }

    #[test]
    fn the_panel_shows_the_decoded_fields_a_frame_carries() {
        let mut state = SdrMetrics::fixture().streaming();
        state.adsb.rate_supported = true;
        state.adsb.tuned_hz = crate::signal::adsb::CENTER_FREQUENCY_HZ;
        state.adsb.sample_rate_hz = crate::signal::adsb::RTL_SAMPLE_RATE_HZ;
        state.adsb.push_frame(entry(AdsbDetails {
            callsign: Some("DLH123".into()),
            altitude_ft: Some(37000),
            groundspeed_kt: Some(452.0),
            track_deg: Some(271.0),
            vertical_rate_fpm: Some(-640),
            latitude: Some(50.1234),
            longitude: Some(8.5678),
            ..AdsbDetails::default()
        }));

        let out = draw(AdsbFramesPanel, 100, 12, &state).join("\n");
        assert!(out.contains("DLH123"), "{out}");
        assert!(out.contains("37000 ft"), "{out}");
        assert!(out.contains("452 kt"), "{out}");
        assert!(out.contains("271°"), "{out}");
        assert!(out.contains("-640 fpm"), "{out}");
        assert!(out.contains("50.1234"), "{out}");
        assert!(out.contains("8.5678"), "{out}");
    }

    #[test]
    fn a_frame_without_a_field_leaves_it_blank_rather_than_zero() {
        let mut state = SdrMetrics::fixture().streaming();
        state.adsb.rate_supported = true;
        state.adsb.tuned_hz = crate::signal::adsb::CENTER_FREQUENCY_HZ;
        state.adsb.sample_rate_hz = crate::signal::adsb::RTL_SAMPLE_RATE_HZ;
        state.adsb.push_frame(entry(AdsbDetails {
            callsign: Some("DLH123".into()),
            ..AdsbDetails::default()
        }));

        let out = draw(AdsbFramesPanel, 100, 12, &state).join("\n");
        assert!(out.contains("DLH123"), "{out}");
        assert!(!out.contains("0 ft"), "{out}");
        assert!(!out.contains("0 kt"), "{out}");
    }
}
