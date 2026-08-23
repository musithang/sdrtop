use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};

use crate::state::SdrMetrics;
use crate::ui::panel::Panel;

pub struct ObserverPanel;

impl Panel for ObserverPanel {
    fn name(&self) -> &'static str { "observer" }
    fn min_size(&self) -> (u16, u16) { (40, 10) }

    fn render(&self, f: &mut Frame, area: Rect, state: &SdrMetrics, theme: &crate::Theme, _focused: bool) {
        let dash = "—";
        let device    = state.observer.device.as_deref().unwrap_or(dash);
        let serial    = state.observer.serial.as_deref().unwrap_or(dash);
        let usb       = state.observer.usb.as_deref().unwrap_or(dash);
        let connected = state.observer.connected.as_deref().unwrap_or(dash);

        let mut lines: Vec<Line> = vec![
            Line::from(vec![
                Span::styled(" Observer Mode", Style::default().fg(theme.observer).add_modifier(Modifier::BOLD)),
            ]),
            Line::from(""),
            Line::from(Span::styled(format!("  {}", device),    Style::default().fg(theme.value_hi))),
            Line::from(Span::styled(format!("  Serial: {}", serial), Style::default().fg(theme.value))),
            Line::from(Span::styled(format!("  USB {}", usb),   Style::default().fg(theme.value))),
            Line::from(Span::styled(format!("  Connected: {}", connected), Style::default().fg(theme.value))),
            Line::from(""),
        ];

        if let Some(owner) = &state.observer.owner {
            lines.push(Line::from(Span::styled(
                format!("  In use by: {}", owner),
                Style::default().fg(theme.value),
            )));
            if let Some(cmdline) = &state.observer.cmdline {
                let truncated = if cmdline.len() > (area.width as usize).saturating_sub(4) {
                    format!("  {}…", &cmdline.chars().take((area.width as usize).saturating_sub(5)).collect::<String>())
                } else {
                    format!("  {}", cmdline)
                };
                lines.push(Line::from(Span::styled(truncated, Style::default().fg(theme.label))));
            }
            let uptime = state.observer.owner_uptime.as_deref().unwrap_or(dash);
            lines.push(Line::from(Span::styled(
                format!(
                    "  CPU: {:.1}%  ·  RAM: {} MB  ·  Running: {}",
                    state.observer.owner_cpu_pct,
                    state.observer.owner_ram_mb,
                    uptime,
                ),
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

        let para = Paragraph::new(lines)
            .block(
                Block::default()
                    .title(" Observer Mode ")
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(theme.observer)),
            );
        f.render_widget(para, area);
    }
}
