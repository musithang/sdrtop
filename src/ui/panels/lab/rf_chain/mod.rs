// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `RfChainPanel` - the RF Diagnostics column of the Lab RF bench ([6]).
//!
//! Reads the whole receive chain as one story: the per-stage **gain lineup** (level
//! after each stage), **gain staging** (LNA/VGA vs their optimal targets), the Friis
//! **noise figure** breakdown, **sensitivity** (MDS + noise-floor trend), and a
//! plain-language verdict with the action chips. All levels are *modeled / relative*
//! dBm anchored to the measured ADC level - useful for staging, not a wattmeter.
//!
//! - [`staging`]: the gain lineup and the two gain bars.
//! - [`noise`]: the Friis breakdown and the sensitivity read-out.
//! - [`verdict`]: the plain-language block, the chips and the foot.
//! - [`rf_bench`](super::rf_bench): the row vocabulary shared with `adc_loading`,
//!   the panel this one is the other half of.

mod noise;
mod staging;
mod verdict;

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::state::SdrMetrics;
use crate::ui::panel::{Panel, PanelChrome, Staleness, Tag};
use crate::ui::rf_calc::{
    cascade, level_lineup, staging_target, staging_verdict, system_nf_db, Stage,
};

use super::rf_bench::severity_color;

pub struct RfChainPanel;

/// Label column for this panel's rows: clears `LNA`, `VGA`, `MDS`, `sys`, `ADC`
/// and the stage labels. Narrower than the ADC column's, because these names are.
const LABEL_W: usize = 3;

/// Width reserved for a bar row's right-hand value (`20 / 40 dB`).
const VALW: usize = 10;

impl Panel for RfChainPanel {
    fn name(&self) -> &'static str {
        "rf_chain"
    }
    fn min_size(&self) -> (u16, u16) {
        (32, 16)
    }
    // `d` (Diagnostics) focuses the RF bench for its own actions; `r`/`f` are taken
    // globally (reset / frequency), so the panel takes a free mnemonic.
    fn focus_key(&self) -> Option<char> {
        Some('d')
    }
    fn focus_bindings(&self) -> &'static [(&'static str, &'static str)] {
        &[
            ("\u{2191}\u{2193}", "LNA"),
            ("[ ]", "VGA"),
            ("A", "auto-gain"),
            ("\u{23B5}", "freeze"),
        ]
    }

    fn chrome(&self, state: &SdrMetrics) -> PanelChrome {
        PanelChrome::new("RF _Diagnostics")
            .stale_when(Staleness::NotStreaming)
            .tag_if(state.lab.rf_freeze.is_some(), Tag::Frozen)
    }

    fn render(
        &self,
        f: &mut Frame,
        inner: Rect,
        state: &SdrMetrics,
        theme: &crate::Theme,
        _focused: bool,
    ) {
        let iw = inner.width as usize;

        if !state.radio.hw_streaming {
            f.render_widget(
                Paragraph::new(Span::styled(
                    "\u{2014}\u{2014}\u{2014}",
                    Style::default().fg(theme.label),
                )),
                inner,
            );
            return;
        }
        // No modelled cascade: the bench assumes the HackRF chain, and this
        // device is either one tuner or one whose stages we were never told the
        // noise figures for. The gain model says which.
        if !state.caps.friis_applicable {
            f.render_widget(Paragraph::new(single_tuner(state, theme)), inner);
            return;
        }

        // --- model (frozen snapshot when held, else live) ----------------------
        let fz = state.lab.rf_freeze.as_ref();
        let amp = fz.map(|f| f.amp_enabled).unwrap_or(state.radio.amp_enabled);
        let lna = fz.map(|f| f.lna_gain).unwrap_or(state.radio.lna_gain);
        let vga = fz.map(|f| f.vga_gain).unwrap_or(state.radio.vga_gain);
        let adc_peak = fz
            .map(|f| f.peak_dbfs)
            .unwrap_or(state.signal.adc_peak_dbfs) as f64;
        let snr = fz.map(|f| f.snr_db).unwrap_or(state.signal.peak_to_nf_db) as f64;
        let adc_rms = fz.map(|f| f.rms_dbfs).unwrap_or(state.signal.adc_rms_dbfs);
        let stages: Vec<Stage> = cascade(amp, lna, vga);
        let nf = system_nf_db(&stages);
        let levels = level_lineup(adc_peak, snr, &stages);
        let (verdict_word, sev) = staging_verdict(adc_peak);
        let (lna_opt, vga_opt) = staging_target(adc_peak, lna, vga);
        let sev_col = severity_color(sev, theme);

        let mut lines: Vec<Line> = Vec::new();
        staging::lineup(&mut lines, &levels, &stages, adc_peak, sev_col, iw, theme);
        lines.push(Line::raw(""));
        staging::staging(&mut lines, lna, vga, lna_opt, vga_opt, iw, theme);
        lines.push(Line::raw(""));
        noise::noise_figure(&mut lines, &stages, nf, iw, theme);
        lines.push(Line::raw(""));
        noise::sensitivity(&mut lines, state, nf, iw, theme);
        lines.push(Line::raw(""));
        verdict::draw(
            &mut lines,
            &verdict::Verdict {
                word: verdict_word,
                sev,
                sev_col,
                adc_peak,
                snr,
                adc_rms,
                amp,
                tracking: state.lab.rf_autotrack,
            },
            theme,
        );

        // Self-adjusting density: collapse spacers when short, grow them to fill when
        // tall (chrome::fit_spacers), so the pane breathes the same at every height -
        // consistent with the other lab side panels.
        crate::ui::chrome::fit_spacers(&mut lines, inner.height as usize);
        f.render_widget(Paragraph::new(lines), inner);
    }
}

/// What an RTL-SDR gets: one gain figure and an honest note that the rest of this
/// bench does not apply to a single-tuner front end.
fn single_tuner(state: &SdrMetrics, theme: &crate::Theme) -> Vec<Line<'static>> {
    vec![
        Line::from(Span::styled(
            format!(" {} gain ", state.caps.gain.primary_label().to_uppercase()),
            Style::default()
                .fg(theme.label)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::raw(" "),
            Span::styled(
                format!("{} dB", state.radio.lna_gain),
                Style::default()
                    .fg(theme.value_hi)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::raw(""),
        Line::from(Span::styled(
            format!(" {}", state.caps.gain.no_cascade_reason()),
            Style::default().fg(theme.stale),
        )),
    ]
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
}
