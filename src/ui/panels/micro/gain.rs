//! `micro_gain` — the field gain-staging view (`[0]` cycle, 3rd step).
//!
//! For setting gain fast on arrival: wide LNA/VGA bars, prominent ADC
//! utilisation, and a central gain-advisor verdict, with estimated NF and MDS
//! for context.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::state::SdrMetrics;
use crate::ui::panel::{Panel, PanelChrome};
use crate::ui::rf_calc::{adc_utilisation_ratio, estimate_mds_dbm, estimate_nf_db, gain_advice};
use crate::ui::widgets::charts::draw_hbar;
use crate::ui::widgets::micro_common::{fmt_freq_mhz, sat_color, status_badge};

pub struct MicroGainPanel;

impl Panel for MicroGainPanel {
    fn name(&self) -> &'static str {
        "micro_gain_panel"
    }
    fn min_size(&self) -> (u16, u16) {
        (40, 8)
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
        let r = &state.radio;
        let gm = &state.caps.gain;
        let lbl = |s: &'static str| Span::styled(s, Style::default().fg(theme.label));
        let dash = || Span::styled("---".to_string(), Style::default().fg(theme.stale));

        // 12 stacked rows; trailing Min(0) absorbs extra height.
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // 0 header
                Constraint::Length(1), // 1 blank
                Constraint::Length(1), // 2 LNA bar
                Constraint::Length(1), // 3 VGA bar
                Constraint::Length(1), // 4 AMP / total
                Constraint::Length(1), // 5 blank
                Constraint::Length(1), // 6 ADC util
                Constraint::Length(1), // 7 SAT
                Constraint::Length(1), // 8 NF
                Constraint::Length(1), // 9 MDS
                Constraint::Length(1), // 10 blank
                Constraint::Length(1), // 11 advisor
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
                    fmt_freq_mhz(r.frequency),
                    Style::default()
                        .fg(theme.value_hi)
                        .add_modifier(Modifier::BOLD),
                ),
            ])),
            rows[0],
        );

        // Gain bars (always valid — gain is configured even when stopped). The
        // primary stage is HackRF's LNA / RTL-SDR's tuner; the second stage (VGA)
        // exists only on HackRF.
        let primary_max = gm.primary_max_db().max(1) as f64;
        draw_hbar(
            f,
            rows[2],
            r.lna_gain as f64 / primary_max,
            &format!(" {}  ", gm.primary_label()),
            &format!("{} dB", r.lna_gain),
            theme.value,
            theme,
        );
        if gm.has_second_stage() {
            draw_hbar(
                f,
                rows[3],
                r.vga_gain as f64 / 62.0,
                " VGA  ",
                &format!("{} dB", r.vga_gain),
                theme.value,
                theme,
            );
        } else {
            f.render_widget(
                Paragraph::new(Line::from(vec![Span::raw(" "), lbl("VGA   "), dash()])),
                rows[3],
            );
        }

        // Front-end boost (AMP / AGC) + total gain.
        let total_gain = if gm.is_single() {
            r.lna_gain as i32
        } else {
            r.lna_gain as i32 + r.vga_gain as i32 + if r.amp_enabled { 14 } else { 0 }
        };
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::raw(" "),
                Span::styled(
                    format!("{}  ", gm.boost_label()),
                    Style::default().fg(theme.label),
                ),
                Span::styled(
                    if r.amp_enabled { "ON " } else { "OFF" },
                    Style::default().fg(if r.amp_enabled {
                        theme.status_ok
                    } else {
                        theme.value
                    }),
                ),
                Span::raw("    "),
                lbl("Total: "),
                Span::styled(
                    format!("{} dB", total_gain),
                    Style::default().fg(theme.value_hi),
                ),
            ])),
            rows[4],
        );

        // ADC utilisation.
        if stale {
            f.render_widget(
                Paragraph::new(Line::from(vec![Span::raw(" "), lbl("ADC util  "), dash()])),
                rows[6],
            );
        } else {
            let ratio = adc_utilisation_ratio(&state.iq.iq_amplitude_hist);
            let color = if ratio > 0.5 {
                theme.status_ok
            } else if ratio > 0.2 {
                theme.status_warn
            } else {
                theme.status_crit
            };
            draw_hbar(
                f,
                rows[6],
                ratio,
                " ADC util  ",
                &format!("{:.0}%", ratio * 100.0),
                color,
                theme,
            );
        }

        // SAT.
        let sat = state.signal.adc_saturation_pct;
        let sat_span = if stale {
            dash()
        } else {
            Span::styled(
                format!("{:.1}%", sat),
                Style::default().fg(sat_color(sat, theme)),
            )
        };
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::raw(" "),
                lbl("SAT       "),
                sat_span,
            ])),
            rows[7],
        );

        // Estimated NF + MDS — the Friis cascade model only applies to HackRF's
        // known 3-stage front end; single-tuner devices (RTL-SDR) show N/A.
        if state.caps.friis_applicable {
            let nf = estimate_nf_db(r.amp_enabled, r.lna_gain);
            let nf_color = if nf < 4.0 {
                theme.status_ok
            } else if nf < 8.0 {
                theme.status_warn
            } else {
                theme.status_crit
            };
            f.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::raw(" "),
                    lbl("Est. NF   "),
                    Span::styled(format!("~{:.1} dB", nf), Style::default().fg(nf_color)),
                ])),
                rows[8],
            );

            let (mds_str, mds_color) = match estimate_mds_dbm(r.bb_filter_hz, nf) {
                Some(mds) => {
                    let c = if mds < -95.0 {
                        theme.status_ok
                    } else if mds < -85.0 {
                        theme.status_warn
                    } else {
                        theme.status_crit
                    };
                    (format!("~{:.0} dBm", mds), c)
                }
                None => ("---".to_string(), theme.stale),
            };
            f.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::raw(" "),
                    lbl("MDS       "),
                    Span::styled(mds_str, Style::default().fg(mds_color)),
                ])),
                rows[9],
            );
        } else {
            f.render_widget(
                Paragraph::new(Line::from(vec![Span::raw(" "), lbl("Est. NF   "), dash()])),
                rows[8],
            );
            f.render_widget(
                Paragraph::new(Line::from(vec![Span::raw(" "), lbl("MDS       "), dash()])),
                rows[9],
            );
        }

        // Gain advisor — the headline verdict.
        let (advice_text, advice_sev) = if stale {
            ("--- (RX not streaming)", 0u8)
        } else {
            gain_advice(&state.iq.iq_amplitude_hist)
        };
        let advice_color = if stale {
            theme.stale
        } else {
            match advice_sev {
                2 => theme.status_crit,
                1 => theme.status_warn,
                _ => theme.status_ok,
            }
        };
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::raw(" "),
                Span::styled(
                    advice_text,
                    Style::default()
                        .fg(advice_color)
                        .add_modifier(Modifier::BOLD),
                ),
            ])),
            rows[11],
        );
    }
}
