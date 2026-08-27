//! `micro_health` — the field hardware-monitoring view (`[0]` cycle, 4th step).
//!
//! For long unattended captures on a Pi: drop / saturation / buffer sparklines,
//! CPU / RAM, USB throughput and sample-rate accuracy, plus a one-glance summary
//! verdict and the running session timer.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::state::SdrMetrics;
use crate::ui::panel::{Panel, PanelChrome};
use crate::ui::widgets::micro_common::{
    buf_color, drop_color, fmt_freq_mhz, sat_color, sparkline, status_badge,
};

pub struct MicroHealthPanel;

/// Inline sparkline width in cells.
const SPARK_W: usize = 14;

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
        let stale = !state.radio.hw_streaming;
        let lbl = |s: String| Span::styled(s, Style::default().fg(theme.label));
        let dash = || Span::styled("---".to_string(), Style::default().fg(theme.stale));

        let rows = Layout::default()
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
            .split(inner);

        // Header.
        let [dot, word] = status_badge(state, theme);
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::raw(" "),
                dot,
                word,
                Span::raw("   "),
                Span::styled(
                    fmt_freq_mhz(state.radio.frequency),
                    Style::default()
                        .fg(theme.value_hi)
                        .add_modifier(Modifier::BOLD),
                ),
            ])),
            rows[0],
        );

        // DROP / SAT / BUF rows with sparklines + a status word.
        let drops = state.signal.drops_per_sec;
        let drop_hist: Vec<f64> = state
            .signal
            .drop_history
            .iter()
            .map(|&v| v as f64)
            .collect();
        f.render_widget(
            Paragraph::new(health_row(
                "DROP",
                if stale {
                    None
                } else {
                    Some(format!("{}/s", drops))
                },
                &drop_hist,
                if stale {
                    theme.stale
                } else {
                    drop_color(drops, theme)
                },
                stale,
                drops == 0,
                theme,
            )),
            rows[2],
        );

        let sat = state.signal.adc_saturation_pct;
        let sat_hist: Vec<f64> = state
            .signal
            .saturation_history
            .iter()
            .map(|&v| v as f64)
            .collect();
        f.render_widget(
            Paragraph::new(health_row(
                "SAT",
                if stale {
                    None
                } else {
                    Some(format!("{:.1}%", sat))
                },
                &sat_hist,
                if stale {
                    theme.stale
                } else {
                    sat_color(sat, theme)
                },
                stale,
                sat < 1.0,
                theme,
            )),
            rows[3],
        );

        let buf = state.iq.buf_fill_pct;
        let buf_hist: Vec<f64> = state
            .iq
            .buf_fill_history
            .iter()
            .map(|&v| v as f64)
            .collect();
        f.render_widget(
            Paragraph::new(health_row(
                "BUF",
                if stale {
                    None
                } else {
                    Some(format!("{:.0}%", buf))
                },
                &buf_hist,
                if stale {
                    theme.stale
                } else {
                    buf_color(buf, theme)
                },
                stale,
                buf < 80.0,
                theme,
            )),
            rows[4],
        );

        // CPU (sparkline, history is %×10) + RAM. These come from the system task,
        // so they stay live even when RX is stopped.
        let cpu = state.system.process_cpu_pct;
        let cpu_hist: Vec<f64> = state.system.cpu_history.iter().map(|&v| v as f64).collect();
        let spark = sparkline(&cpu_hist, SPARK_W);
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::raw(" "),
                lbl(format!("{:<6}", "CPU")),
                Span::styled(
                    format!("{:<7}", format!("{:.1}%", cpu)),
                    Style::default().fg(theme.value),
                ),
                Span::styled(spark, Style::default().fg(theme.value)),
            ])),
            rows[6],
        );
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::raw(" "),
                lbl(format!("{:<6}", "RAM")),
                Span::styled(
                    format!("{} MB", state.system.process_rss_mb),
                    Style::default().fg(theme.value),
                ),
            ])),
            rows[7],
        );

        // USB throughput (binary MB/s) + error count.
        let usb_span = if stale {
            dash()
        } else {
            let mb_s = state.radio.current_throughput_bps as f64 / 1024.0 / 1024.0;
            Span::styled(
                format!("{:.1} MB/s", mb_s),
                Style::default().fg(theme.value),
            )
        };
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::raw(" "),
                lbl(format!("{:<6}", "USB")),
                usb_span,
                Span::raw("   "),
                lbl("err: ".to_string()),
                Span::styled(
                    format!("{}", state.signal.usb_errors_session),
                    Style::default().fg(if state.signal.usb_errors_session == 0 {
                        theme.value
                    } else {
                        theme.status_warn
                    }),
                ),
            ])),
            rows[9],
        );

        // Sample-rate accuracy: configured → actual, with ppm offset.
        let cfg_msps = state.radio.config_sample_rate / 1_000_000.0;
        let sr_span = if stale || state.radio.actual_sample_rate == 0 {
            vec![
                Span::styled(
                    format!("{:.3} MHz", cfg_msps),
                    Style::default().fg(theme.value),
                ),
                Span::raw("  "),
                dash(),
            ]
        } else {
            let actual = state.radio.actual_sample_rate as f64;
            let act_msps = actual / 1_000_000.0;
            let ppm = ((actual - state.radio.config_sample_rate) / state.radio.config_sample_rate
                * 1_000_000.0)
                .round() as i64;
            vec![
                Span::styled(
                    format!("{:.3} → {:.3} MHz", cfg_msps, act_msps),
                    Style::default().fg(theme.value),
                ),
                Span::raw("  "),
                Span::styled(
                    format!("{:+}ppm", ppm),
                    Style::default().fg(if ppm.abs() < 100 {
                        theme.status_ok
                    } else {
                        theme.status_warn
                    }),
                ),
            ]
        };
        let mut sr_line = vec![Span::raw(" "), lbl(format!("{:<6}", "SR"))];
        sr_line.extend(sr_span);
        f.render_widget(Paragraph::new(Line::from(sr_line)), rows[10]);

        // Summary verdict + session timer.
        f.render_widget(Paragraph::new(summary_line(state, theme)), rows[12]);
    }
}

/// One DROP/SAT/BUF row: `LABEL  value   sparkline   OK/⚠`.
fn health_row(
    label: &str,
    value: Option<String>,
    hist: &[f64],
    color: Color,
    stale: bool,
    ok: bool,
    theme: &crate::Theme,
) -> Line<'static> {
    let mut spans = vec![
        Span::raw(" "),
        Span::styled(format!("{:<6}", label), Style::default().fg(theme.label)),
    ];
    match value {
        Some(v) => spans.push(Span::styled(
            format!("{:<7}", v),
            Style::default().fg(color),
        )),
        None => spans.push(Span::styled(
            format!("{:<7}", "---"),
            Style::default().fg(theme.stale),
        )),
    }
    spans.push(Span::styled(
        sparkline(hist, SPARK_W),
        Style::default().fg(color),
    ));
    if !stale {
        spans.push(Span::raw("  "));
        let (mark, mc) = if ok {
            ("OK", theme.status_ok)
        } else {
            ("⚠", theme.status_warn)
        };
        spans.push(Span::styled(mark, Style::default().fg(mc)));
    }
    Line::from(spans)
}

/// One-glance verdict: drops beat CPU beat all-clear; idle when not streaming.
fn summary_line(state: &SdrMetrics, theme: &crate::Theme) -> Line<'static> {
    if !state.radio.hw_streaming {
        return Line::from(vec![
            Span::raw(" "),
            Span::styled("○ IDLE — RX stopped", Style::default().fg(theme.stale)),
        ]);
    }
    let session = state
        .radio
        .rx_start_time
        .map(|t| crate::tasks::fmt_duration(t.elapsed().as_secs()))
        .unwrap_or_else(|| "—".to_string());

    let (text, color) = if state.signal.drops_per_sec > 0 {
        ("⚠ DROP DETECTED".to_string(), theme.status_crit)
    } else if state.system.process_cpu_pct > 70.0 {
        ("⚠ CPU HIGH".to_string(), theme.status_warn)
    } else {
        (
            format!("✓ System OK — session {}", session),
            theme.status_ok,
        )
    };
    Line::from(vec![
        Span::raw(" "),
        Span::styled(
            text,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
    ])
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
