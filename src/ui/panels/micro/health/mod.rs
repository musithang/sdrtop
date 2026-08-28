//! `micro_health` — the field health view (`[0]` cycle, 4th step).
//!
//! For long unattended captures: what the stream is doing, what the host is
//! spending on it, whether the clock is honest, and one line saying whether to
//! worry. Everything on one screen, nothing to press.
//!
//! Split by whose problem each block is:
//!
//! - [`rows`]: the `LABEL value trend OK/⚠` row the stream block is built from.
//! - [`stream`]: drops, saturation, buffer fill.
//! - [`host`]: CPU, RAM, the USB link.
//! - [`clock`]: sample-rate accuracy.
//! - [`summary`]: the verdict and the session timer.

mod clock;
mod host;
mod rows;
mod stream;
mod summary;

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    widgets::Paragraph,
    Frame,
};

use crate::state::SdrMetrics;
use crate::ui::panel::{Panel, PanelChrome};

use super::field::{self, Field};

pub struct MicroHealthPanel;

impl Panel for MicroHealthPanel {
    fn name(&self) -> &'static str {
        "micro_health_panel"
    }
    fn min_size(&self) -> (u16, u16) {
        (44, 8)
    }

    fn chrome(&self, _state: &SdrMetrics) -> PanelChrome {
        PanelChrome::untitled()
    }

    fn render(
        &self,
        f: &mut Frame,
        inner: Rect,
        state: &SdrMetrics,
        theme: &crate::Theme,
        _focused: bool,
    ) {
        let fd = Field::new(state, theme);

        let rows: Vec<Rect> = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // 0 header
                Constraint::Length(1), // 1 blank
                Constraint::Length(1), // 2 DROP
                Constraint::Length(1), // 3 SAT
                Constraint::Length(1), // 4 BUF
                Constraint::Length(1), // 5 blank
                Constraint::Length(1), // 6 CPU
                Constraint::Length(1), // 7 RAM
                Constraint::Length(1), // 8 blank
                Constraint::Length(1), // 9 USB
                Constraint::Length(1), // 10 SR
                Constraint::Length(1), // 11 blank
                Constraint::Length(1), // 12 summary
                Constraint::Min(0),
            ])
            .split(inner)
            .to_vec();

        f.render_widget(Paragraph::new(field::header(state, theme)), rows[0]);
        for (line, row) in stream::lines(state, &fd).into_iter().zip(&rows[2..5]) {
            f.render_widget(Paragraph::new(line), *row);
        }
        f.render_widget(Paragraph::new(host::cpu_line(state, &fd)), rows[6]);
        f.render_widget(Paragraph::new(host::ram_line(state, &fd)), rows[7]);
        f.render_widget(Paragraph::new(host::usb_line(state, &fd)), rows[9]);
        f.render_widget(Paragraph::new(clock::line(state, &fd)), rows[10]);
        f.render_widget(Paragraph::new(summary::line(state, &fd)), rows[12]);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::fixture::draw;

    const W: u16 = 46;
    const H: u16 = 18;

    /// Idle is a state, not a failure: the panel says so plainly and does not
    /// dress up stale counters as live ones.
    #[test]
    fn an_idle_radio_reads_idle_rather_than_all_clear() {
        let out = draw(MicroHealthPanel, W, H, &SdrMetrics::fixture()).join("\n");
        assert!(out.contains("IDLE"), "idle verdict missing:\n{out}");
        assert!(!out.contains("System OK"), "idle must not read OK:\n{out}");
    }

    /// A healthy stream gets the all-clear plus a session timer, which is the
    /// reason this panel exists: a long unattended capture you can glance at.
    #[test]
    fn a_healthy_stream_reads_ok_with_a_session_timer() {
        let m = SdrMetrics::fixture().streaming().with_carrier(0.0, 40.0);
        let out = draw(MicroHealthPanel, W, H, &m).join("\n");
        assert!(out.contains("System OK"), "all-clear missing:\n{out}");
        assert!(out.contains("session"), "session timer missing:\n{out}");
    }

    /// Drops outrank CPU outrank all-clear, and the panel has one verdict row —
    /// so the worst thing true must be the thing shown.
    #[test]
    fn the_verdict_shows_the_worst_thing_that_is_true() {
        let mut m = SdrMetrics::fixture().streaming().with_carrier(0.0, 40.0);
        m.system.process_cpu_pct = 85.0;
        let cpu_only = draw(MicroHealthPanel, W, H, &m).join("\n");
        assert!(
            cpu_only.contains("CPU HIGH"),
            "CPU verdict missing:\n{cpu_only}"
        );
        assert!(!cpu_only.contains("System OK"));

        // Drops on top of high CPU: drops win, and the CPU line is still drawn.
        m.signal.drops_per_sec = 12;
        let both = draw(MicroHealthPanel, W, H, &m).join("\n");
        assert!(
            both.contains("DROP DETECTED"),
            "drop verdict missing:\n{both}"
        );
        assert!(!both.contains("CPU HIGH"), "two verdicts at once:\n{both}");
    }

    /// CPU and RAM come from the system task, not the radio, so they stay live
    /// with RX stopped. Everything measured from the stream must not.
    #[test]
    fn system_stats_survive_a_stopped_radio() {
        let mut m = SdrMetrics::fixture();
        m.system.process_cpu_pct = 12.5;
        m.system.process_rss_mb = 41;
        let out = draw(MicroHealthPanel, W, H, &m).join("\n");
        assert!(out.contains("41 MB"), "RSS missing while idle:\n{out}");
        assert!(
            out.contains("12.5") || out.contains("12"),
            "CPU missing:\n{out}"
        );
    }

    /// The panel has to survive the sizes a micro layout actually gives it.
    #[test]
    fn it_renders_at_every_size_the_layout_can_hand_it() {
        let m = SdrMetrics::fixture().streaming().with_carrier(0.0, 40.0);
        let (min_w, min_h) = MicroHealthPanel.min_size();
        for (w, h) in [(min_w, min_h), (W, H), (120, 40)] {
            let out = draw(MicroHealthPanel, w, h, &m);
            assert_eq!(
                out.len(),
                h as usize,
                "{w}x{h} produced the wrong row count"
            );
        }
    }
}
