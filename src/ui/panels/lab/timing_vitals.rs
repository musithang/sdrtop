//! `timing_vitals` — the right column of the `lab_timing` preset's redesign.
//!
//! The host-pipeline health view, built in the shared lab side-panel language: an
//! airy Line stack collapsed to fit (`chrome::collapse_spacers`), inline trend
//! sparklines, and `gain_bar_colored` ⅛-block bars (same look as the RF-chain /
//! ADC-loading / IQ-diagnostics bars). Sample drops, ADC saturation and CPU as
//! 60 s trends, then the USB link and ring-buffer state as captioned bars, closed
//! by a one-line vitals verdict + uptime. Link utilisation is referenced to the
//! device's own USB ceiling (`caps.sample_rate_max_hz`), not a magic constant.

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::state::SdrMetrics;
use crate::ui::panel::{Panel, PanelChrome, Staleness};
use crate::ui::widgets::charts::gain_bar_colored;
use crate::ui::widgets::micro_common::{buf_color, drop_color, sat_color, sparkline};

pub struct TimingVitalsPanel;

/// Binary MB/s ceiling of the USB link for this device: the byte rate at the
/// device's maximum sample rate (8-bit I/Q ⇒ 2 bytes per complex sample). Honest
/// per-device headroom reference rather than a magic constant.
fn link_ceiling_mbps(sample_rate_max_hz: f64) -> f64 {
    (sample_rate_max_hz * 2.0) / (1024.0 * 1024.0)
}

/// Overrun margin: how much ring-buffer headroom remains below the ceiling, from
/// the session peak fill. Clamped to a sane 0..=100.
fn overrun_margin_pct(peak_fill_pct: f64) -> f64 {
    (100.0 - peak_fill_pct).clamp(0.0, 100.0)
}

/// `HH:MM:SS` uptime from a whole-second count.
fn fmt_uptime(secs: u64) -> String {
    format!(
        "{:02}:{:02}:{:02}",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}

fn threshold_color(value: f64, warn: f64, crit: f64, theme: &crate::Theme) -> Color {
    if value >= crit {
        theme.status_crit
    } else if value >= warn {
        theme.status_warn
    } else {
        theme.status_ok
    }
}

impl Panel for TimingVitalsPanel {
    fn name(&self) -> &'static str {
        "timing_vitals"
    }
    fn min_size(&self) -> (u16, u16) {
        (30, 18)
    }
    fn focus_key(&self) -> Option<char> {
        Some('v')
    }
    fn focus_bindings(&self) -> &'static [(&'static str, &'static str)] {
        &[("R", "Reset drop counter"), ("C", "Clear history")]
    }

    fn chrome(&self, _state: &SdrMetrics) -> PanelChrome {
        PanelChrome::new("Hardware _Vitals").stale_when(Staleness::NotStreaming)
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
        let iw = inner.width as usize;

        let lbl = Style::default().fg(theme.label);
        let val = Style::default().fg(theme.value);
        let dim = Style::default().fg(theme.stale);
        let dash = || Span::styled("---".to_string(), dim);
        let spark_w = iw.saturating_sub(2).max(4);

        // A label + value trend block: the heading row, then an inline sparkline.
        let trend = |heading: Vec<Span<'static>>, hist: Vec<f64>| -> [Line<'static>; 2] {
            let s = if stale {
                String::new()
            } else {
                sparkline(&hist, spark_w)
            };
            [
                Line::from(heading),
                Line::from(vec![Span::raw(" "), Span::styled(s, val)]),
            ]
        };

        // A captioned ⅛-block bar row (lab bar language) that never lets the value
        // collide with the bar: fixed label column, computed bar width, value tail.
        let bar_row = |label: &'static str,
                       ratio: f64,
                       value_str: String,
                       lo: Color,
                       hi: Color,
                       val_col: Color|
         -> Line<'static> {
            const LW: usize = 11;
            let vw = value_str.chars().count() + 1;
            let bar_w = iw.saturating_sub(1 + LW + 1 + vw).max(4);
            let mut spans = vec![
                Span::raw(" "),
                Span::styled(format!("{label:<LW$}"), lbl),
                Span::raw(" "),
            ];
            if stale {
                spans.extend(gain_bar_colored(0, 1000, bar_w, lo, hi, theme.border_dim));
                spans.push(Span::styled(" ---".to_string(), dim));
            } else {
                let v = (ratio.clamp(0.0, 1.0) * 1000.0) as u32;
                spans.extend(gain_bar_colored(v, 1000, bar_w, lo, hi, theme.border_dim));
                spans.push(Span::styled(
                    format!(" {value_str}"),
                    Style::default().fg(val_col),
                ));
            }
            Line::from(spans)
        };

        let mut lines: Vec<Line> = Vec::new();
        lines.push(Line::from(vec![
            Span::raw(" "),
            Span::styled("host pipeline health \u{00b7} 60 s rolling", lbl),
        ]));

        // ── Sample drops ────────────────────────────────────────────────────────
        let dcol = drop_color(state.signal.drops_per_sec, theme);
        let drops_head = if stale {
            vec![Span::raw(" "), Span::styled("Sample drops ", lbl), dash()]
        } else {
            vec![
                Span::raw(" "),
                Span::styled("Sample drops ", lbl),
                Span::styled(
                    format!("{}/s", state.signal.drops_per_sec),
                    Style::default().fg(dcol),
                ),
                Span::styled(
                    format!("   session {}", state.signal.total_drops_session),
                    lbl,
                ),
            ]
        };
        lines.extend(trend(
            drops_head,
            state
                .signal
                .drop_history
                .iter()
                .map(|&v| v as f64)
                .collect(),
        ));
        lines.push(Line::raw(""));

        // ── ADC saturation ──────────────────────────────────────────────────────
        let scol = sat_color(state.signal.adc_saturation_pct, theme);
        let sat_head = if stale {
            vec![Span::raw(" "), Span::styled("ADC saturation ", lbl), dash()]
        } else {
            vec![
                Span::raw(" "),
                Span::styled("ADC saturation ", lbl),
                Span::styled(
                    format!("{:.1} %", state.signal.adc_saturation_pct),
                    Style::default().fg(scol),
                ),
                Span::styled(
                    format!("   peak {:.1}%", state.signal.adc_saturation_peak),
                    lbl,
                ),
            ]
        };
        lines.extend(trend(
            sat_head,
            state
                .signal
                .saturation_history
                .iter()
                .map(|&v| v as f64)
                .collect(),
        ));
        lines.push(Line::raw(""));

        // ── CPU / RAM ───────────────────────────────────────────────────────────
        let cpu = state.system.process_cpu_pct as f64;
        let ccol = threshold_color(cpu, 50.0, 80.0, theme);
        let cpu_head = vec![
            Span::raw(" "),
            Span::styled("CPU load ", lbl),
            Span::styled(format!("{cpu:.1} %"), Style::default().fg(ccol)),
            Span::styled(format!("   RAM {} MB", state.system.process_rss_mb), lbl),
        ];
        lines.extend(trend(
            cpu_head,
            state.system.cpu_history.iter().map(|&v| v as f64).collect(),
        ));
        lines.push(Line::raw(""));

        // ── USB link ────────────────────────────────────────────────────────────
        lines.push(crate::ui::chrome::section(
            "USB LINK",
            "bulk transfer",
            iw,
            theme,
        ));
        let usb_recent: u64 = state.signal.usb_error_history.iter().sum();
        let ucol = if usb_recent > 0 {
            theme.status_crit
        } else if state.signal.usb_errors_session > 0 {
            theme.status_warn
        } else {
            theme.status_ok
        };
        lines.push(Line::from(vec![
            Span::raw(" "),
            Span::styled("USB errors ", lbl),
            Span::styled(
                format!("{}", state.signal.usb_errors_session),
                Style::default().fg(ucol),
            ),
            Span::styled(" (session)", lbl),
        ]));
        let mbps = state.timing.throughput_mean_mbps;
        let ceiling = link_ceiling_mbps(state.caps.sample_rate_max_hz);
        let util = if ceiling > 0.0 {
            (mbps / ceiling).clamp(0.0, 1.0)
        } else {
            0.0
        };
        lines.push(Line::from(if stale {
            vec![Span::raw(" "), Span::styled("Bus throughput ", lbl), dash()]
        } else {
            vec![
                Span::raw(" "),
                Span::styled("Bus throughput ", lbl),
                Span::styled(format!("{mbps:.1} MB/s"), val),
                Span::styled(format!(" of {ceiling:.1} max"), lbl),
            ]
        }));
        lines.push(bar_row(
            "link util",
            util,
            format!("{:.0}%", util * 100.0),
            theme.status_ok,
            theme.status_warn,
            theme.value,
        ));
        lines.push(Line::raw(""));

        // ── Ring buffer ─────────────────────────────────────────────────────────
        lines.push(crate::ui::chrome::section(
            "RING BUFFER",
            "overrun margin",
            iw,
            theme,
        ));
        let fill = state.iq.buf_fill_pct as f64;
        let fcol = buf_color(state.iq.buf_fill_pct, theme);
        lines.push(bar_row(
            "fill depth",
            fill / 100.0,
            format!("{fill:.0}%"),
            theme.status_ok,
            theme.status_crit,
            fcol,
        ));
        let peak_fill = state.iq.buf_fill_history.iter().copied().max().unwrap_or(0) as f64 / 10.0;
        let (peak_tag, peak_col) = if peak_fill >= 100.0 {
            ("hit ceiling", theme.status_crit)
        } else {
            ("headroom ok", theme.status_ok)
        };
        lines.push(Line::from(if stale {
            vec![Span::raw(" "), Span::styled("Peak fill ", lbl), dash()]
        } else {
            vec![
                Span::raw(" "),
                Span::styled("Peak fill ", lbl),
                Span::styled(
                    format!("{peak_fill:.0} %"),
                    Style::default().fg(buf_color(peak_fill as f32, theme)),
                ),
                Span::styled(format!("   {peak_tag}"), Style::default().fg(peak_col)),
            ]
        }));
        let margin = overrun_margin_pct(peak_fill);
        lines.push(Line::from(if stale {
            vec![Span::raw(" "), Span::styled("Overrun margin ", lbl), dash()]
        } else {
            vec![
                Span::raw(" "),
                Span::styled("Overrun margin ", lbl),
                Span::styled(
                    format!("{margin:.0}%"),
                    Style::default().fg(threshold_color(100.0 - margin, 50.0, 80.0, theme)),
                ),
            ]
        }));
        lines.push(Line::raw(""));

        // ── Verdict + uptime ────────────────────────────────────────────────────
        if stale {
            lines.push(Line::from(vec![
                Span::raw(" "),
                Span::styled("\u{25cb} idle \u{2014} RX stopped", dim),
            ]));
        } else {
            let (mark, text, col) = match state.timing.timing_quality.severity() {
                0 => ("\u{2713}", "all vitals nominal", theme.status_ok),
                1 | 2 => ("\u{26a0}", "pipeline under load", theme.status_warn),
                _ => ("\u{26a0}", "overrun logged", theme.status_crit),
            };
            let mut spans = vec![
                Span::raw(" "),
                Span::styled(
                    format!("{mark} {text}"),
                    Style::default().fg(col).add_modifier(Modifier::BOLD),
                ),
            ];
            if let Some(up) = state
                .radio
                .rx_start_time
                .map(|t| fmt_uptime(t.elapsed().as_secs()))
            {
                let used = 1 + mark.chars().count() + 1 + text.chars().count();
                let tail = format!("uptime {up}");
                let gap = iw.saturating_sub(used + tail.chars().count()).max(1);
                spans.push(Span::raw(" ".repeat(gap)));
                spans.push(Span::styled(tail, lbl));
            }
            lines.push(Line::from(spans));
        }

        crate::ui::chrome::fit_spacers(&mut lines, inner.height as usize);
        f.render_widget(Paragraph::new(lines), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_ceiling_is_byte_rate_at_max_sr() {
        // 20 Msps HackRF → 40 MB(byte)/s → ~38.1 binary MB/s.
        let c = link_ceiling_mbps(20_000_000.0);
        assert!((c - 38.147).abs() < 0.05, "got {c}");
        assert_eq!(link_ceiling_mbps(0.0), 0.0);
    }

    #[test]
    fn overrun_margin_clamps() {
        assert_eq!(overrun_margin_pct(0.0), 100.0);
        assert_eq!(overrun_margin_pct(62.0), 38.0);
        assert_eq!(overrun_margin_pct(100.0), 0.0);
        // A peak above the ceiling cannot push the margin negative.
        assert_eq!(overrun_margin_pct(140.0), 0.0);
    }

    #[test]
    fn uptime_formats_hms() {
        assert_eq!(fmt_uptime(0), "00:00:00");
        assert_eq!(fmt_uptime(15_127), "04:12:07");
        assert_eq!(fmt_uptime(59), "00:00:59");
    }
}
