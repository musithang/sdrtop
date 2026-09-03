// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `micro_panel` - the field-operator entry view (`micro_main`).
//!
//! A self-contained panel (not a composition of others) that answers the four
//! field questions in stacked zones: where am I (freq), what's the signal, is it
//! running healthy, and the gain I'm most likely adjusting. It adapts to width
//! in three modes - compact (≥60), narrow (40–59), minimum (<40) - so it stays
//! readable from an 80×24 SSH session down to a 40-col framebuffer.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::state::SdrMetrics;
use crate::ui::panel::{Panel, PanelChrome};
use crate::ui::widgets::charts::draw_hbar;
use crate::ui::widgets::micro_common::{
    buf_color, drop_color, fft_stale, fmt_rbw, sat_color, snr_color,
};

/// Width threshold (inner columns) for each adaptive mode.
const COMPACT_MIN: u16 = 60;
const NARROW_MIN: u16 = 40;

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Compact,
    Narrow,
    Minimum,
}

impl Mode {
    fn from_width(w: u16) -> Self {
        if w >= COMPACT_MIN {
            Mode::Compact
        } else if w >= NARROW_MIN {
            Mode::Narrow
        } else {
            Mode::Minimum
        }
    }
}

pub struct MicroPanel;

impl Panel for MicroPanel {
    fn name(&self) -> &'static str {
        "micro_panel"
    }
    fn min_size(&self) -> (u16, u16) {
        (40, 4)
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
        let mode = Mode::from_width(inner.width);

        // Four stacked zones; trailing Min(0) absorbs any extra height so the
        // rows pack at the top. Rows that fall past the bottom get 0 height and
        // render as no-ops.
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(0),
            ])
            .split(inner);

        f.render_widget(Paragraph::new(status_line(state, theme, mode)), rows[0]);
        render_gain(f, rows[1], state, theme, mode);
        f.render_widget(Paragraph::new(signal_line(state, theme, mode)), rows[2]);
        f.render_widget(Paragraph::new(health_line(state, theme, mode)), rows[3]);
    }
}

// ── Zone builders ───────────────────────────────────────────────────────────

/// FREQ zone, row 1: status badge, frequency, sample rate, AMP.
fn status_line(state: &SdrMetrics, theme: &crate::Theme, mode: Mode) -> Line<'static> {
    let r = &state.radio;
    let (dot, dot_col, word) = if r.rx_enabled {
        ("●", theme.status_ok, "RX")
    } else {
        ("○", theme.status_warn, "IDLE")
    };
    let freq_mhz = r.frequency as f64 / 1_000_000.0;
    let sr_msps = r.config_sample_rate / 1_000_000.0;
    let amp = if r.amp_enabled { "ON" } else { "OFF" };

    let badge = Span::styled(dot, Style::default().fg(dot_col));
    let freq_style = Style::default()
        .fg(theme.value_hi)
        .add_modifier(Modifier::BOLD);
    let dim = |s: String| Span::styled(s, Style::default().fg(theme.value));

    match mode {
        Mode::Compact => Line::from(vec![
            Span::raw(" "),
            badge,
            Span::styled(format!(" {}", word), Style::default().fg(dot_col)),
            Span::raw("   "),
            Span::styled(format!("{:.3} MHz", freq_mhz), freq_style),
            Span::raw("   "),
            dim(format!("{:.1} Msps", sr_msps)),
            Span::raw("   "),
            dim(format!("AMP {}", amp)),
        ]),
        Mode::Narrow => Line::from(vec![
            Span::raw(" "),
            badge,
            Span::raw(" "),
            Span::styled(format!("{:.3} MHz", freq_mhz), freq_style),
            Span::raw("  "),
            dim(format!("{:.1}M", sr_msps)),
            Span::raw("  "),
            dim(format!("AMP:{}", amp)),
        ]),
        Mode::Minimum => Line::from(vec![
            Span::raw(" "),
            badge,
            Span::raw(" "),
            Span::styled(format!("{:.3}MHz", freq_mhz), freq_style),
        ]),
    }
}

/// FREQ zone, row 2: LNA / VGA gain. Compact draws side-by-side bars; the
/// narrower modes fall back to text so nothing is lost on tiny terminals.
fn render_gain(f: &mut Frame, area: Rect, state: &SdrMetrics, theme: &crate::Theme, mode: Mode) {
    if area.height == 0 {
        return;
    }
    let r = &state.radio;
    match mode {
        Mode::Compact => {
            let halves = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(area);
            draw_hbar(
                f,
                halves[0],
                r.primary_gain() as f64 / 40.0,
                " LNA ",
                &format!("{} dB", r.primary_gain()),
                theme.value,
                theme,
            );
            draw_hbar(
                f,
                halves[1],
                r.secondary_gain() as f64 / 62.0,
                " VGA ",
                &format!("{} dB", r.secondary_gain()),
                theme.value,
                theme,
            );
        }
        Mode::Narrow => {
            let line = Line::from(vec![
                Span::raw(" "),
                Span::styled("LNA:", Style::default().fg(theme.label)),
                Span::styled(
                    format!("{}dB", r.primary_gain()),
                    Style::default().fg(theme.value),
                ),
                Span::raw("  "),
                Span::styled("VGA:", Style::default().fg(theme.label)),
                Span::styled(
                    format!("{}dB", r.secondary_gain()),
                    Style::default().fg(theme.value),
                ),
            ]);
            f.render_widget(Paragraph::new(line), area);
        }
        Mode::Minimum => {
            let amp = if r.amp_enabled { "ON" } else { "OFF" };
            let line = Line::from(vec![
                Span::raw(" "),
                Span::styled(
                    format!("L:{} V:{} ", r.primary_gain(), r.secondary_gain()),
                    Style::default().fg(theme.value),
                ),
                Span::styled(format!("AMP:{}", amp), Style::default().fg(theme.label)),
            ]);
            f.render_widget(Paragraph::new(line), area);
        }
    }
}

/// SIGNAL zone: SNR / channel power / noise floor.
fn signal_line(state: &SdrMetrics, theme: &crate::Theme, mode: Mode) -> Line<'static> {
    let stale = fft_stale(state);

    let snr = state.signal.peak_to_nf_db;
    let pwr = state.signal.channel_power_dbfs;
    let nf = state
        .waterfall
        .last_fft
        .as_ref()
        .filter(|_| !stale)
        .map(|fr| fr.noise_floor);

    let snr_col = if stale {
        theme.stale
    } else {
        snr_color(snr, theme)
    };
    let pwr_finite = pwr.is_finite();
    let pwr_col = if stale || !pwr_finite {
        theme.stale
    } else {
        theme.value
    };

    let lbl = |s: &'static str| Span::styled(s, Style::default().fg(theme.label));
    let dash = || Span::styled("---".to_string(), Style::default().fg(theme.stale));

    let snr_val = |s: String| Span::styled(s, Style::default().fg(snr_col));
    let pwr_val = |s: String| Span::styled(s, Style::default().fg(pwr_col));
    let nf_val = |s: String| Span::styled(s, Style::default().fg(theme.value));

    let snr_num = if stale { None } else { Some(snr) };
    let pwr_num = if stale || !pwr_finite {
        None
    } else {
        Some(pwr)
    };

    match mode {
        Mode::Compact => {
            let mut spans = vec![Span::raw(" "), lbl("SNR ")];
            spans.push(match snr_num {
                Some(v) => snr_val(format!("{:.1} dB", v)),
                None => dash(),
            });
            spans.push(Span::raw("   "));
            spans.push(lbl("PWR "));
            spans.push(match pwr_num {
                Some(v) => pwr_val(format!("{:.1} dBFS", v)),
                None => dash(),
            });
            spans.push(Span::raw("   "));
            spans.push(lbl("NF "));
            spans.push(match nf {
                Some(v) => nf_val(format!("{:.1} dBFS", v)),
                None => dash(),
            });
            Line::from(spans)
        }
        Mode::Narrow => {
            let mut spans = vec![Span::raw(" "), lbl("SNR:")];
            spans.push(match snr_num {
                Some(v) => snr_val(format!("{:.1}", v)),
                None => dash(),
            });
            spans.push(Span::raw("  "));
            spans.push(lbl("PWR:"));
            spans.push(match pwr_num {
                Some(v) => pwr_val(format!("{:.0}", v)),
                None => dash(),
            });
            spans.push(Span::raw("  "));
            spans.push(lbl("NF:"));
            spans.push(match nf {
                Some(v) => nf_val(format!("{:.0}dBFS", v)),
                None => dash(),
            });
            Line::from(spans)
        }
        Mode::Minimum => {
            let mut spans = vec![Span::raw(" "), lbl("SNR:")];
            spans.push(match snr_num {
                Some(v) => snr_val(format!("{:.1}", v)),
                None => dash(),
            });
            spans.push(Span::raw(" "));
            spans.push(lbl("PWR:"));
            spans.push(match pwr_num {
                Some(v) => pwr_val(format!("{:.0}", v)),
                None => dash(),
            });
            Line::from(spans)
        }
    }
}

/// HEALTH zone: drop rate / buffer / saturation / RBW.
fn health_line(state: &SdrMetrics, theme: &crate::Theme, mode: Mode) -> Line<'static> {
    let hw_stale = !state.radio.hw_streaming;
    let fft_is_stale = fft_stale(state);

    let drops = state.signal.drops_per_sec;
    let buf = state.iq.buf_fill_pct;
    let sat = state.signal.adc_saturation_pct;
    let rbw = state
        .waterfall
        .last_fft
        .as_ref()
        .filter(|_| !fft_is_stale)
        .map(|fr| fr.enbw_hz);

    let lbl = |s: &'static str| Span::styled(s, Style::default().fg(theme.label));
    let dash = || Span::styled("---".to_string(), Style::default().fg(theme.stale));
    let val = |s: String, c: Color| Span::styled(s, Style::default().fg(c));

    let drop_c = if hw_stale {
        theme.stale
    } else {
        drop_color(drops, theme)
    };
    let buf_c = if hw_stale {
        theme.stale
    } else {
        buf_color(buf, theme)
    };
    let sat_c = if hw_stale {
        theme.stale
    } else {
        sat_color(sat, theme)
    };

    match mode {
        Mode::Compact => {
            let mut spans = vec![Span::raw(" "), lbl("DROP ")];
            spans.push(if hw_stale {
                dash()
            } else {
                val(format!("{}/s", drops), drop_c)
            });
            spans.push(Span::raw("   "));
            spans.push(lbl("BUF "));
            spans.push(if hw_stale {
                dash()
            } else {
                val(format!("{:.0}%", buf), buf_c)
            });
            spans.push(Span::raw("   "));
            spans.push(lbl("SAT "));
            spans.push(if hw_stale {
                dash()
            } else {
                val(format!("{:.1}%", sat), sat_c)
            });
            spans.push(Span::raw("   "));
            spans.push(lbl("RBW "));
            spans.push(match rbw {
                Some(v) => val(fmt_rbw(v), theme.value),
                None => dash(),
            });
            Line::from(spans)
        }
        Mode::Narrow => {
            let mut spans = vec![Span::raw(" "), lbl("DRP:")];
            spans.push(if hw_stale {
                dash()
            } else {
                val(format!("{}", drops), drop_c)
            });
            spans.push(Span::raw("  "));
            spans.push(lbl("BUF:"));
            spans.push(if hw_stale {
                dash()
            } else {
                val(format!("{:.0}%", buf), buf_c)
            });
            spans.push(Span::raw("  "));
            spans.push(lbl("SAT:"));
            spans.push(if hw_stale {
                dash()
            } else {
                val(format!("{:.1}%", sat), sat_c)
            });
            Line::from(spans)
        }
        Mode::Minimum => {
            let mut spans = vec![Span::raw(" "), lbl("DRP:")];
            spans.push(if hw_stale {
                dash()
            } else {
                val(format!("{}", drops), drop_c)
            });
            spans.push(Span::raw(" "));
            spans.push(lbl("SAT:"));
            spans.push(if hw_stale {
                dash()
            } else {
                val(format!("{:.0}%", sat), sat_c)
            });
            Line::from(spans)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_thresholds() {
        assert!(matches!(Mode::from_width(80), Mode::Compact));
        assert!(matches!(Mode::from_width(60), Mode::Compact));
        assert!(matches!(Mode::from_width(59), Mode::Narrow));
        assert!(matches!(Mode::from_width(40), Mode::Narrow));
        assert!(matches!(Mode::from_width(39), Mode::Minimum));
    }
}
