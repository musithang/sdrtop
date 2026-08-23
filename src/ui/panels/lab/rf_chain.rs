//! `RfChainPanel` — the RF Diagnostics column of the Lab RF bench ([6]).
//!
//! Reads the whole receive chain as one story: the per-stage **gain lineup** (level
//! after each stage), **gain staging** (LNA/VGA vs their optimal targets), the Friis
//! **noise figure** breakdown, **sensitivity** (MDS + noise-floor trend), and a
//! plain-language verdict with the action chips. All levels are *modeled / relative*
//! dBm anchored to the measured ADC level — useful for staging, not a wattmeter.

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::state::SdrMetrics;
use crate::ui::widgets::charts::gain_bar_colored;
use crate::ui::widgets::micro_common::spark_minmax;
use crate::ui::panel::{Panel, PanelChrome, Staleness, Tag};
use crate::ui::rf_calc::{
    cascade, estimate_mds_dbm, level_lineup, staging_target, staging_verdict, system_nf_db, Stage,
};

pub struct RfChainPanel;

fn fmt_mhz(hz: u32) -> String {
    if hz >= 1_000_000 { format!("{:.0} MHz", hz as f64 / 1e6) }
    else if hz > 0     { format!("{} kHz", hz / 1000) }
    else               { "—".to_string() }
}

impl Panel for RfChainPanel {
    fn name(&self) -> &'static str { "rf_chain" }
    fn min_size(&self) -> (u16, u16) { (32, 16) }
    // `d` (Diagnostics) focuses the RF bench for its own actions; `r`/`f` are taken
    // globally (reset / frequency), so the panel takes a free mnemonic.
    fn focus_key(&self) -> Option<char> { Some('d') }
    fn focus_bindings(&self) -> &'static [(&'static str, &'static str)] {
        &[("\u{2191}\u{2193}", "LNA"), ("[ ]", "VGA"), ("A", "auto-gain"), ("\u{23B5}", "freeze")]
    }

    fn chrome(&self, state: &SdrMetrics) -> PanelChrome {
        PanelChrome::new("RF _Diagnostics")
            .stale_when(Staleness::NotStreaming)
            .tag_if(state.lab.rf_freeze.is_some(), Tag::Frozen)
    }

    fn render(&self, f: &mut Frame, inner: Rect, state: &SdrMetrics, theme: &crate::Theme, _focused: bool) {
        let stale = !state.radio.hw_streaming;

        let iw  = inner.width as usize;
        let dim = theme.border_dim;
        let lbl = Style::default().fg(theme.label);

        if stale {
            f.render_widget(
                Paragraph::new(Span::styled("\u{2014}\u{2014}\u{2014}", lbl)), inner);
            return;
        }

        // Single-tuner (RTL-SDR): the cascade bench assumes the HackRF chain.
        if !state.caps.friis_applicable {
            let lines = vec![
                Line::from(Span::styled(" TUNER gain ", Style::default().fg(theme.label).add_modifier(Modifier::BOLD))),
                Line::from(vec![
                    Span::raw(" "),
                    Span::styled(format!("{} dB", state.radio.lna_gain),
                                 Style::default().fg(theme.value_hi).add_modifier(Modifier::BOLD)),
                ]),
                Line::raw(""),
                Line::from(Span::styled(" single-tuner \u{2014} cascade N/A", Style::default().fg(theme.stale))),
            ];
            f.render_widget(Paragraph::new(lines), inner);
            return;
        }

        // --- model (frozen snapshot when held, else live) ----------------------
        let fz = state.lab.rf_freeze.as_ref();
        let amp = fz.map(|f| f.amp_enabled).unwrap_or(state.radio.amp_enabled);
        let lna = fz.map(|f| f.lna_gain).unwrap_or(state.radio.lna_gain);
        let vga = fz.map(|f| f.vga_gain).unwrap_or(state.radio.vga_gain);
        let adc_peak = fz.map(|f| f.peak_dbfs).unwrap_or(state.signal.adc_peak_dbfs) as f64;
        let snr = fz.map(|f| f.snr_db).unwrap_or(state.signal.peak_to_nf_db) as f64;
        let adc_rms  = fz.map(|f| f.rms_dbfs).unwrap_or(state.signal.adc_rms_dbfs);
        let stages: Vec<Stage> = cascade(amp, lna, vga);
        let nf  = system_nf_db(&stages);
        let levels = level_lineup(adc_peak, snr, &stages);
        let (verdict_word, sev) = staging_verdict(adc_peak);
        let (lna_opt, vga_opt)  = staging_target(adc_peak, lna, vga);
        let sev_col = match sev {
            2 => theme.status_crit, 1 => theme.status_warn, _ => theme.status_ok,
        };

        // --- helpers -----------------------------------------------------------
        let section = |name: &str, hint: &str| -> Line<'static> {
            let label = name.to_uppercase();
            let left = label.chars().count() + 5;
            let hint_w = if hint.is_empty() { 0 } else { hint.chars().count() + 1 };
            let dashes = iw.saturating_sub(left + hint_w);
            let mut spans = vec![
                Span::styled("\u{251c}\u{2574} ".to_string(), Style::default().fg(dim)),
                Span::styled(label, Style::default().fg(theme.label).add_modifier(Modifier::BOLD)),
                Span::styled(" \u{2576}".to_string(), Style::default().fg(dim)),
                Span::styled("\u{2500}".repeat(dashes), Style::default().fg(dim)),
            ];
            if !hint.is_empty() {
                spans.push(Span::styled(format!(" {hint}"), Style::default().fg(dim)));
            }
            Line::from(spans)
        };
        // " LBL  mid............ right" — label + a mid column + a right-aligned value.
        let row3 = |label: &str, mid: String, mid_col: Color, right: String, right_col: Color| -> Line<'static> {
            let pad = iw.saturating_sub(1 + 3 + 1 + mid.chars().count() + right.chars().count());
            Line::from(vec![
                Span::raw(" "),
                Span::styled(format!("{label:<3}"), lbl),
                Span::raw(" "),
                Span::styled(mid, Style::default().fg(mid_col)),
                Span::raw(" ".repeat(pad.max(1))),
                Span::styled(right, Style::default().fg(right_col).add_modifier(Modifier::BOLD)),
            ])
        };
        // " LBL [bar] value" — the app's standard eighth-block gradient gain bar
        // (same widget as the command rail / header LNA·VGA), with an optional `┊`
        // optimal-target tick overlaid on one cell.
        const VALW: usize = 10;
        let bar_w = iw.saturating_sub(1 + 3 + 1 + 1 + VALW).max(6);
        let bar_row = |label: &str, val: u32, max: u32, lo: Color, hi: Color,
                       tick: Option<f64>, val_str: String, val_col: Color| -> Line<'static> {
            let mut bar = gain_bar_colored(val, max, bar_w, lo, hi, dim);
            if let Some(t) = tick {
                let tc = ((t.clamp(0.0, 1.0) * bar_w as f64).round() as usize)
                    .min(bar_w.saturating_sub(1));
                if tc < bar.len() {
                    bar[tc] = Span::styled("\u{250a}".to_string(), Style::default().fg(theme.value_hi));
                }
            }
            let mut spans = vec![
                Span::raw(" "),
                Span::styled(format!("{label:<3}"), lbl),
                Span::raw(" "),
            ];
            spans.extend(bar);
            spans.push(Span::raw(" "));
            spans.push(Span::styled(val_str, Style::default().fg(val_col).add_modifier(Modifier::BOLD)));
            Line::from(spans)
        };

        let mut lines: Vec<Line> = Vec::new();

        // --- GAIN LINEUP -------------------------------------------------------
        lines.push(section("Gain lineup", "level after each stage"));
        lines.push(Line::raw(""));
        for (i, node) in levels.iter().enumerate() {
            let gain_str = if i == 0 { "\u{2014}".to_string() }
                           else { format!("{:+} dB", stages[i - 1].gain_db as i64) };
            lines.push(row3(node.label, gain_str, dim,
                            format!("{:.0} dBm", node.signal_dbm), theme.value));
            lines.push(Line::raw(""));
        }
        // ADC node = VGA output, read in dBFS.
        lines.push(row3("ADC", "0 dB".to_string(), dim, format!("{adc_peak:.0} dBFS"), sev_col));
        lines.push(Line::raw(""));

        // --- GAIN STAGING ------------------------------------------------------
        lines.push(section("Gain staging", "\u{2502} = optimal target"));
        lines.push(Line::raw(""));
        lines.push(bar_row("LNA", lna, 40, theme.status_ok, theme.value_hi,
                           Some(lna_opt as f64 / 40.0), format!("{lna} / 40 dB"), theme.value));
        lines.push(Line::raw(""));
        lines.push(bar_row("VGA", vga, 62, theme.border_accent, theme.status_warn,
                           Some(vga_opt as f64 / 62.0), format!("{vga} / 62 dB"), theme.value));
        lines.push(Line::raw(""));
        let at_opt = lna == lna_opt && vga == vga_opt;
        lines.push(Line::from(vec![
            Span::raw(" "),
            Span::styled("opt ", lbl),
            if at_opt {
                Span::styled("\u{2713} at optimum", Style::default().fg(theme.status_ok))
            } else {
                Span::styled(format!("\u{2192} LNA {lna_opt} \u{00b7} VGA {vga_opt}"),
                             Style::default().fg(theme.status_warn))
            },
        ]));
        lines.push(Line::raw(""));

        // --- NOISE FIGURE ------------------------------------------------------
        // Per-stage own NF (visible bars); the Friis system total can sit *below* the
        // worst stage because the LNA gain suppresses everything after it.
        lines.push(section("Noise figure", "Friis cascade"));
        lines.push(Line::raw(""));
        for s in &stages {
            lines.push(bar_row(s.label, (s.nf_db * 100.0) as u32, 1200,
                               theme.status_ok, theme.status_crit, None,
                               format!("{:.1} dB", s.nf_db), theme.value));
            lines.push(Line::raw(""));
        }
        let nf_col = if nf < 6.0 { theme.status_ok } else if nf < 10.0 { theme.status_warn } else { theme.status_crit };
        lines.push(row3("sys", "NF total".to_string(), theme.label, format!("{nf:.1} dB"), nf_col));
        lines.push(Line::raw(""));

        // --- SENSITIVITY -------------------------------------------------------
        lines.push(section("Sensitivity", "noise floor trend"));
        lines.push(Line::raw(""));
        let mds_str = match estimate_mds_dbm(state.radio.bb_filter_hz, nf) {
            Some(mds) => format!("{mds:.0} dBm"),
            None      => "\u{2014}".to_string(),
        };
        lines.push(row3("MDS", format!("({} BW)", fmt_mhz(state.radio.bb_filter_hz)), dim,
                        mds_str, theme.value_hi));
        let floor: Vec<f32> = state.signal.nf_history.iter().copied().collect();
        let spark_w = iw.saturating_sub(1 + 5 + 1 + 12).max(4);
        let (spark, p2p) = spark_minmax(&floor, spark_w);
        if !spark.is_empty() {
            let ann = format!("\u{00b1}{:.1} dB/60s", p2p / 2.0);
            let pad = iw.saturating_sub(7 + spark.chars().count() + ann.chars().count());
            lines.push(Line::from(vec![
                Span::raw(" "),
                Span::styled("floor", lbl),
                Span::raw(" "),
                Span::styled(spark, Style::default().fg(theme.value)),
                Span::raw(" ".repeat(pad.max(1))),
                Span::styled(ann, Style::default().fg(dim)),
            ]));
        }
        lines.push(Line::raw(""));

        // --- VERDICT -----------------------------------------------------------
        let headroom = -adc_peak;
        let title_mark = if sev == 0 { "\u{2713}" } else if sev == 2 { "\u{26a0}" } else { "\u{00b7}" };
        lines.push(Line::from(vec![
            Span::raw(" "),
            Span::styled(format!("{title_mark} GAIN {verdict_word}"),
                         Style::default().fg(sev_col).add_modifier(Modifier::BOLD)),
        ]));
        let body = |t: String| Line::from(vec![Span::raw(" "), Span::styled(t, lbl)]);
        let (b1, b2, b3) = match verdict_word {
            "CLIPPING" => (
                format!("Signal at {adc_peak:.0} dBFS \u{2014} ADC saturating, waveform distorted."),
                "Reduce LNA or VGA to restore headroom.".to_string(),
                "SNR is lost wherever the ADC clips.".to_string(),
            ),
            "HOT" => (
                format!("Signal at {adc_peak:.0} dBFS \u{2014} {headroom:.0} dB from the rail."),
                "SNR intact but peaks may saturate.".to_string(),
                "Trim VGA by 2\u{2013}4 dB to add margin.".to_string(),
            ),
            "UNDER-UTILISED" => (
                format!("Signal at {adc_peak:.0} dBFS \u{2014} {headroom:.0} dB headroom,"),
                "ADC window under-used. Increase LNA gain".to_string(),
                "to lift the signal into the optimal zone.".to_string(),
            ),
            "WEAK" => (
                format!("Signal at {adc_peak:.0} dBFS \u{2014} ADC severely under-driven."),
                "Increase LNA; if maxed, check the antenna".to_string(),
                "or reduce the system noise figure.".to_string(),
            ),
            _ => ( // WELL-STAGED
                format!("Signal at {adc_peak:.0} dBFS \u{2014} {headroom:.0} dB headroom,"),
                format!("{snr:.0} dB above the noise floor. SNR set at"),
                "the front end is fully preserved.".to_string(),
            ),
        };
        lines.push(body(b1));
        lines.push(body(b2));
        lines.push(body(b3));

        // Action chips (idle until Step 7 wires auto-gain) + status foot.
        let chip = |label: &str, active: bool| -> Span<'static> {
            let bg = if active { theme.value_hi } else { theme.border_dim };
            Span::styled(format!(" {label} "), Style::default().bg(bg).fg(Color::Rgb(4, 6, 15)))
        };
        let tracking = state.lab.rf_autotrack;
        lines.push(Line::from(vec![
            Span::raw(" "),
            chip("A auto-gain", tracking), Span::raw(" "),
            chip("\u{2191}\u{2193} LNA", false), Span::raw(" "),
            chip("[ ] VGA", false),
        ]));
        let limited = if adc_rms > -50.0 { "analog-noise limited" }
                      else { "quantisation limited" };
        let amp_txt = if amp { "AMP on" } else { "AMP bypass" };
        let ag_txt  = if tracking { "auto-gain \u{2713} tracking" } else { "auto-gain idle" };
        lines.push(Line::from(vec![
            Span::raw(" "),
            Span::styled(format!("{amp_txt} \u{00b7} {ag_txt} \u{00b7} {limited}"),
                         Style::default().fg(dim)),
        ]));

        // Self-adjusting density: collapse spacers when short, grow them to fill when
        // tall (chrome::fit_spacers), so the pane breathes the same at every height —
        // consistent with the other lab side panels.
        crate::ui::chrome::fit_spacers(&mut lines, inner.height as usize);
        f.render_widget(Paragraph::new(lines), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_name_and_min_size() {
        let p = RfChainPanel;
        assert_eq!(p.name(), "rf_chain");
        let (w, h) = p.min_size();
        assert!(w >= 16 && h >= 8);
    }

    #[test]
    fn fmt_mhz_units() {
        assert_eq!(fmt_mhz(2_000_000), "2 MHz");
        assert_eq!(fmt_mhz(500_000), "500 kHz");
        assert_eq!(fmt_mhz(0), "\u{2014}");
    }
}
