use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::Style,
    text::Span,
    widgets::{Block, BorderType, Borders, Gauge, Paragraph, Sparkline},
    Frame,
};

use crate::state::SdrMetrics;
use crate::ui::panel::Panel;

pub struct SystemResourcesPanel;

impl Panel for SystemResourcesPanel {
    fn name(&self) -> &'static str { "system_resources" }
    fn min_size(&self) -> (u16, u16) { (30, 10) }

    fn render(&self, f: &mut Frame, area: Rect, state: &SdrMetrics, theme: &crate::Theme, _focused: bool) {
        let block = Block::default()
            .title(" System Resources ")
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(theme.border_dim));
        let inner = block.inner(area);
        f.render_widget(block, area);

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Length(2),
                Constraint::Length(1),
                Constraint::Min(0),
            ])
            .split(inner);

        let cpu = state.system.process_cpu_pct.clamp(0.0, 100.0);
        let cpu_color = if cpu > 80.0 { theme.status_crit }
            else if cpu > 50.0       { theme.status_warn  }
            else                     { theme.status_ok    };
        f.render_widget(
            Gauge::default()
                .label(format!("CPU  {:.1}%", cpu))
                .ratio(cpu as f64 / 100.0)
                .style(Style::default().fg(cpu_color)),
            rows[0],
        );

        let rss = state.system.process_rss_mb;
        let rss_ratio = (rss as f64 / 512.0).min(1.0);
        // 3-tier colour, consistent with the CPU gauge.
        let rss_color = if rss_ratio > 0.8 { theme.status_crit }
            else if rss_ratio > 0.5        { theme.status_warn }
            else                           { theme.status_ok   };
        f.render_widget(
            Gauge::default()
                .label(format!("RAM  {} MB", rss))
                .ratio(rss_ratio)
                .style(Style::default().fg(rss_color)),
            rows[1],
        );

        // In observer mode the device is owned by another process and our own
        // rx task does not run, so our throughput is always 0 — show N/A rather
        // than a misleading "0.00 MB/s" with an empty graph.
        if state.observer.active {
            f.render_widget(
                Paragraph::new(Span::styled(
                    "USB  — (device owned externally)",
                    Style::default().fg(theme.label),
                )),
                rows[2],
            );
        } else {
            let throughput_mb = state.radio.current_throughput_bps as f64 / 1_000_000.0;
            f.render_widget(
                Paragraph::new(Span::styled(
                    format!("USB  {:.2} MB/s", throughput_mb),
                    Style::default().fg(theme.value),
                )),
                rows[2],
            );
            let sparkline_data: Vec<u64> = state.radio.throughput_history.iter().cloned().collect();
            f.render_widget(
                Sparkline::default()
                    .data(&sparkline_data)
                    .style(Style::default().fg(theme.status_ok)),
                rows[3],
            );
        }
    }
}
