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
use crate::ui::panel::{FrameTone, Panel, PanelChrome};

pub struct ObserverPanel;

impl Panel for ObserverPanel {
    fn name(&self) -> &'static str {
        "observer"
    }
    fn min_size(&self) -> (u16, u16) {
        (40, 10)
    }

    fn chrome(&self, _state: &SdrMetrics) -> PanelChrome {
        // The border is the mode indicator: the radio belongs to another process,
        // and the whole panel exists to say so.
        PanelChrome::new("Observer Mode").tone(FrameTone::Observer)
    }

    fn render(
        &self,
        f: &mut Frame,
        inner: Rect,
        state: &SdrMetrics,
        theme: &crate::Theme,
        _focused: bool,
    ) {
        let area = inner;
        let dash = "—";
        let device = state.observer.device.as_deref().unwrap_or(dash);
        let serial = state.observer.serial.as_deref().unwrap_or(dash);
        let usb = state.observer.usb.as_deref().unwrap_or(dash);
        let connected = state.observer.connected.as_deref().unwrap_or(dash);

        let mut lines: Vec<Line> = vec![
            Line::from(vec![Span::styled(
                " Observer Mode",
                Style::default()
                    .fg(theme.observer)
                    .add_modifier(Modifier::BOLD),
            )]),
            Line::from(""),
            Line::from(Span::styled(
                format!("  {}", device),
                Style::default().fg(theme.value_hi),
            )),
            Line::from(Span::styled(
                format!("  Serial: {}", serial),
                Style::default().fg(theme.value),
            )),
            Line::from(Span::styled(
                format!("  USB {}", usb),
                Style::default().fg(theme.value),
            )),
            Line::from(Span::styled(
                format!("  Connected: {}", connected),
                Style::default().fg(theme.value),
            )),
            Line::from(""),
        ];

        if let Some(owner) = &state.observer.owner {
            lines.push(Line::from(Span::styled(
                format!("  In use by: {}", owner),
                Style::default().fg(theme.value),
            )));
            if let Some(cmdline) = &state.observer.cmdline {
                let truncated = if cmdline.len() > (area.width as usize).saturating_sub(4) {
                    format!(
                        "  {}…",
                        cmdline
                            .chars()
                            .take((area.width as usize).saturating_sub(5))
                            .collect::<String>()
                    )
                } else {
                    format!("  {}", cmdline)
                };
                lines.push(Line::from(Span::styled(
                    truncated,
                    Style::default().fg(theme.label),
                )));
            }
            let uptime = state.observer.owner_uptime.as_deref().unwrap_or(dash);
            let cpu = state
                .observer
                .owner_cpu_pct
                .map_or_else(|| dash.to_string(), |c| format!("{c:.1}%"));
            let ram = state
                .observer
                .owner_ram_mb
                .map_or_else(|| dash.to_string(), |r| format!("{r} MB"));
            lines.push(Line::from(Span::styled(
                format!("  CPU: {cpu}  ·  RAM: {ram}  ·  Running: {uptime}"),
                Style::default().fg(theme.value),
            )));
        } else {
            lines.push(Line::from(Span::styled(
                "  Owner: unknown (different user or process ended)",
                Style::default().fg(theme.label),
            )));
        }

        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  Hardware controls disabled.",
            Style::default().fg(theme.label),
        )));

        f.render_widget(Paragraph::new(lines), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::fixture::draw;

    fn held(cpu: Option<f32>, ram: Option<u64>) -> SdrMetrics {
        let mut m = SdrMetrics::fixture();
        m.observer.active = true;
        m.observer.owner = Some("hackrf_transfer (PID 4312)".into());
        m.observer.owner_cpu_pct = cpu;
        m.observer.owner_ram_mb = ram;
        m
    }

    /// **A holder whose readings the system kept to itself shows dashes**,
    /// not 0.0 % and 0 MB: another user's process, or the first second
    /// before a CPU share can be worked out.
    #[test]
    fn a_reading_the_system_kept_is_a_dash_not_a_zero() {
        let screen = draw(ObserverPanel, 80, 16, &held(None, None)).join("\n");
        assert!(screen.contains("CPU: —  ·  RAM: —"), "{screen}");
        assert!(!screen.contains("0.0%"), "{screen}");

        let screen = draw(ObserverPanel, 80, 16, &held(Some(2.5), Some(41))).join("\n");
        assert!(screen.contains("CPU: 2.5%  ·  RAM: 41 MB"), "{screen}");
    }
}
