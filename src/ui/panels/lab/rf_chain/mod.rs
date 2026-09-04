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
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::state::SdrMetrics;
use crate::ui::panel::{Panel, PanelChrome, Staleness, Tag};
use crate::ui::rf_calc::{
    cascade, level_lineup, signal_lineup, staging_target, staging_verdict, system_nf_db, Stage,
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
            ("K", "noise sweep"),
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
        // --- model (frozen snapshot when held, else live) ----------------------
        let fz = state.lab.rf_freeze.as_ref();
        let amp = fz.map(|f| f.amp_enabled).unwrap_or(state.radio.amp_enabled);
        let lna = fz.map(|f| f.lna_gain).unwrap_or(state.radio.primary_gain());
        let vga = fz
            .map(|f| f.vga_gain)
            .unwrap_or(state.radio.secondary_gain());
        let adc_peak = fz
            .map(|f| f.peak_dbfs)
            .unwrap_or(state.signal.adc_peak_dbfs) as f64;
        let snr = fz.map(|f| f.snr_db).unwrap_or(state.signal.peak_to_nf_db) as f64;
        let adc_rms = fz.map(|f| f.rms_dbfs).unwrap_or(state.signal.adc_rms_dbfs);
        // **The gate is per block, not per panel.** Only two of the five blocks
        // below need per-stage noise figures. The level lineup needs gains, the
        // staging advice needs the ADC peak, and the verdict needs neither, so a
        // device whose chain is unmodelled loses the noise numbers and keeps
        // everything else. It used to lose the whole bench: three lines on a
        // twenty row panel.
        let modelled = state.caps.friis_applicable;
        let stages: Vec<Stage> = if modelled {
            cascade(amp, lna, vga)
        } else {
            // The driver's own stages, with the gains it is actually set to.
            state
                .caps
                .gain
                .stages()
                .iter()
                .enumerate()
                .map(|(i, s)| Stage::unmodelled(&s.name, state.radio.stage_gain(i)))
                .collect()
        };
        let nf = system_nf_db(&stages);
        let levels = if modelled {
            level_lineup(adc_peak, snr, &stages)
        } else {
            signal_lineup(adc_peak, &stages)
        };
        let (verdict_word, sev) = staging_verdict(adc_peak);
        let stage_specs = state.caps.gain.stages();
        let targets = staging_target(adc_peak, &stage_specs, &state.radio.gains);
        let sev_col = severity_color(sev, theme);

        let mut lines: Vec<Line> = Vec::new();
        staging::lineup(&mut lines, &levels, &stages, adc_peak, sev_col, iw, theme);
        lines.push(Line::raw(""));
        staging::staging(
            &mut lines,
            &stage_specs,
            &state.radio.gains,
            &targets,
            iw,
            theme,
        );
        lines.push(Line::raw(""));
        if modelled {
            noise::noise_figure(&mut lines, &stages, nf, iw, theme);
            lines.push(Line::raw(""));
            noise::sensitivity(&mut lines, state, nf, iw, theme);
        } else {
            noise::not_modelled(&mut lines, state.caps.gain.no_cascade_reason(), iw, theme);
        }
        lines.push(Line::raw(""));
        // Running or finished, never both: the progress block is the same
        // measurement mid-flight. It sits under SENSITIVITY on a modelled radio
        // so the measured knee and the Friis number can be read together - they
        // answer different questions, and seeing them apart is what makes a
        // disagreement between them findable.
        if state.lab.noise_sweep.is_some() {
            noise::sweep_progress(&mut lines, state, iw, theme);
        } else {
            noise::sweep_reading(&mut lines, state, iw, theme);
        }
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

    use crate::state::fixture::draw;
    use crate::state::SdrMetrics;

    #[test]
    fn an_idle_bench_spends_no_row_saying_the_sweep_is_idle() {
        let m = SdrMetrics::fixture().streaming().with_carrier(0.0, 40.0);
        let out = draw(RfChainPanel, 46, 30, &m).join("\n");
        assert!(!out.contains("NOISE STEP"), "{out}");
    }

    #[test]
    fn a_running_sweep_shows_its_stage_and_how_far_along_it_is() {
        let mut m = SdrMetrics::fixture().streaming().with_carrier(0.0, 40.0);
        let spec = m.caps.gain.stages().remove(0);
        let plan = crate::signal::noise_slope::plan_for(&spec);
        assert!(
            plan.len() > 2,
            "the fixture's front stage must be sweepable"
        );
        let mut sw = crate::signal::noise_slope::GainSweep::new(0, plan.clone(), 24.0);
        sw.begin();
        m.lab.noise_sweep = Some(sw);
        let out = draw(RfChainPanel, 46, 30, &m).join("\n");
        assert!(out.contains("NOISE STEP"), "{out}");
        assert!(out.contains(&spec.name), "{out}");
        assert!(out.contains(&format!("0/{}", plan.len())), "{out}");
    }

    /// The panel's prose as a reader sees it: frame columns removed and the line
    /// breaks closed up, so a sentence that `chrome::wrap` split across two rows
    /// still reads as one string. Asserting on the raw buffer instead makes a
    /// test that passes or fails on where the wrap happened to land.
    fn prose(rows: &[String]) -> String {
        rows.iter()
            .map(|r| r.trim_matches(|c| c == '\u{2502}' || c == ' '))
            .collect::<Vec<_>>()
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn with_reading(knee: Option<f64>, above: Option<f32>) -> SdrMetrics {
        use crate::signal::noise_slope::{Point, Reading};
        let mut m = SdrMetrics::fixture().streaming().with_carrier(0.0, 40.0);
        m.lab.noise_reading = Some(crate::state::NoiseReading {
            at_hz: 92_800_000,
            reading: Reading {
                points: vec![
                    Point {
                        gain_db: 0.0,
                        noise_dbfs: -84.0,
                    },
                    Point {
                        gain_db: 8.0,
                        noise_dbfs: -84.0,
                    },
                    Point {
                        gain_db: 16.0,
                        noise_dbfs: -83.0,
                    },
                    Point {
                        gain_db: 24.0,
                        noise_dbfs: -79.0,
                    },
                    Point {
                        gain_db: 32.0,
                        noise_dbfs: -71.0,
                    },
                    Point {
                        gain_db: 40.0,
                        noise_dbfs: -63.0,
                    },
                ],
                slope: 0.52,
                knee_db: knee,
                slope_above_knee: above,
            },
        });
        m
    }

    #[test]
    fn the_reading_leads_with_the_knee_and_never_quotes_the_span_slope() {
        let m = with_reading(Some(24.0), Some(0.94));
        let out = draw(RfChainPanel, 50, 34, &m).join("\n");
        assert!(out.contains("NOISE STEP"), "{out}");
        assert!(out.contains("LNA 24 dB and up"), "{out}");
        assert!(out.contains("0.94 dB/dB"), "{out}");
        // The span average describes neither half of the curve; it must not
        // appear anywhere on the bench.
        assert!(!out.contains("0.52"), "{out}");
    }

    #[test]
    fn the_reading_names_the_band_it_belongs_to() {
        // The knee moved from LNA 24 to 32 dB between the FM band and quiet UHF
        // on the same radio, so a reading with no frequency on it is a number
        // about somewhere else the moment the user retunes.
        let m = with_reading(Some(24.0), Some(0.94));
        let out = prose(&draw(RfChainPanel, 50, 34, &m));
        assert!(
            out.contains("NOISE STEP ╶────────── measured at 92.800 MHz"),
            "{out}"
        );
    }

    #[test]
    fn the_reading_says_what_it_is_not() {
        let m = with_reading(Some(24.0), Some(0.94));
        let out = prose(&draw(RfChainPanel, 50, 34, &m));
        assert!(
            out.contains("Not a noise figure: that needs a known source"),
            "{out}"
        );
    }

    #[test]
    fn no_knee_is_an_answer_rather_than_a_blank() {
        let m = with_reading(None, None);
        let out = prose(&draw(RfChainPanel, 50, 34, &m));
        assert!(out.contains("none in range"), "{out}");
        // With one regime in the data the span slope has no two halves to
        // average, so it is the honest summary here and is shown.
        assert!(out.contains("0.52 dB/dB"), "{out}");
        assert!(
            out.contains(
                "The floor never followed the gain, so the converter set it at every setting"
            ),
            "{out}"
        );
    }

    #[test]
    fn a_running_sweep_replaces_the_last_reading_rather_than_stacking_on_it() {
        let mut m = with_reading(Some(24.0), Some(0.94));
        let spec = m.caps.gain.stages().remove(0);
        let mut sw = crate::signal::noise_slope::GainSweep::new(
            0,
            crate::signal::noise_slope::plan_for(&spec),
            24.0,
        );
        sw.begin();
        m.lab.noise_sweep = Some(sw);
        let out = draw(RfChainPanel, 50, 34, &m).join("\n");
        assert!(out.contains("sweep running"), "{out}");
        assert!(!out.contains("and up"), "{out}");
    }

    #[test]
    fn the_footer_advertises_the_sweep_key() {
        let b = RfChainPanel.focus_bindings();
        assert!(b.contains(&("K", "noise sweep")), "{b:?}");
    }

    const W: u16 = 56;
    const H: u16 = 22;

    fn chain() -> SdrMetrics {
        SdrMetrics::fixture()
            .streaming()
            .named_chain()
            .with_carrier(0.0, 20.0)
    }

    /// The complaint this checkpoint answers. The bench used to return after
    /// three lines on any device without a modelled cascade, leaving most of a
    /// twenty row panel blank.
    #[test]
    fn an_unmodelled_device_gets_the_blocks_that_do_not_need_noise_figures() {
        let out = draw(RfChainPanel, W, H, &chain()).join("\n");
        assert!(out.contains("GAIN LINEUP"), "no level lineup:\n{out}");
        assert!(out.contains("GAIN STAGING"), "no staging advice:\n{out}");
        assert!(out.contains("ANT"), "no antenna node:\n{out}");
        assert!(out.contains("dBm"), "no levels:\n{out}");
        // And the verdict, which needs only the ADC peak.
        assert!(
            out.contains("STAGED") || out.contains("GAIN"),
            "no verdict:\n{out}"
        );
    }

    /// The two blocks that genuinely cannot work say so, in the space their
    /// numbers would have filled, with the reason this device gives.
    #[test]
    fn the_noise_blocks_name_what_is_missing_rather_than_printing_a_number() {
        let out = draw(RfChainPanel, W, H, &chain()).join("\n");
        assert!(out.contains("not modelled"), "{out}");
        assert!(
            out.contains("chain not modelled"),
            "the device's own reason:\n{out}"
        );
        // No noise **number** anywhere. The explanation may name MDS and NF, and
        // does, so this looks for a figure beside them rather than for the words.
        for word in ["MDS", "NF"] {
            assert!(
                !out.lines().any(|l| l.contains(word) && l.contains("dB")),
                "printed a {word} value it cannot know:\n{out}"
            );
        }
    }

    /// An RTL-SDR is the other unmodelled shape: one stage, and a different
    /// reason. Both used to print the same sentence.
    #[test]
    fn a_single_stage_device_gets_the_same_treatment_and_its_own_reason() {
        let m = SdrMetrics::fixture()
            .streaming()
            .rtlsdr()
            .with_carrier(0.0, 20.0);
        let out = draw(RfChainPanel, W, H, &m).join("\n");
        assert!(out.contains("GAIN LINEUP"), "{out}");
        assert!(out.contains("Tuner"), "the driver's own stage name:\n{out}");
        assert!(out.contains("single tuner"), "its own reason:\n{out}");
    }

    /// The modelled device is untouched: it still gets all five blocks.
    #[test]
    fn a_hackrf_still_gets_the_full_bench() {
        let m = SdrMetrics::fixture().streaming().with_carrier(0.0, 20.0);
        let out = draw(RfChainPanel, W, 34, &m).join("\n");
        assert!(out.contains("GAIN LINEUP"), "{out}");
        assert!(
            out.contains("MDS"),
            "the modelled bench keeps its sensitivity:\n{out}"
        );
        assert!(!out.contains("not modelled"), "{out}");
        assert!(out.contains("MIX"), "and its modelled mixer stage:\n{out}");
    }

    /// Build a device with `n` stages, each named `name`-ish and set to 10 dB.
    fn with_stages(n: usize, name: &str) -> SdrMetrics {
        use crate::hardware::StageSpec;
        let mut m = chain();
        let mut caps = (*m.caps).clone();
        caps.gain = crate::hardware::GainModel::new(
            (0..n)
                .map(|i| StageSpec::ranged(&format!("{name}{i}"), 0.0, 30.0, 1.0))
                .collect(),
            "RF",
            "RF",
        );
        m.caps = std::sync::Arc::new(caps);
        m.radio.gains = vec![10.0; n];
        m
    }

    /// One stage, two, three, six. The lineup is the device's own chain, so
    /// there is no count it can assume.
    #[test]
    fn the_lineup_draws_however_many_stages_there_are() {
        for n in [1usize, 2, 3, 6] {
            let out = draw(RfChainPanel, W, 34, &with_stages(n, "ST")).join("\n");
            for i in 0..n {
                assert!(
                    out.contains(&format!("ST{i}")),
                    "{n} stages: no ST{i}:\n{out}"
                );
            }
            assert!(out.contains("ANT") && out.contains("ADC"), "{n}:\n{out}");
        }
    }

    /// Six stages in forty columns: the case the plan named, and the one that
    /// used to clip `-58 dBm` down to `-58 `.
    #[test]
    fn six_stages_in_forty_columns_keep_every_reading() {
        let out = draw(RfChainPanel, 40, 30, &with_stages(6, "STAGE"));
        for line in &out {
            assert!(
                line.chars().count() <= 40,
                "a row grew past the pane: {line:?}"
            );
        }
        let joined = out.join("\n");
        assert_eq!(
            joined.matches("dBm").count(),
            7,
            "the antenna and six stages each keep their level:\n{joined}"
        );
        assert!(
            joined.contains("dBFS"),
            "and the ADC keeps its own:\n{joined}"
        );
    }

    /// A driver may name a stage anything. A long name is **cut to the column**
    /// rather than allowed to push the reading off the right-hand edge: the
    /// level is the measurement, the label is only its name.
    #[test]
    fn a_long_stage_name_is_clipped_rather_than_shoving_the_reading_out() {
        let out = draw(RfChainPanel, 32, 30, &with_stages(3, "PREAMPLIFIER"));
        for line in &out {
            assert!(line.chars().count() <= 32, "{line:?}");
        }
        let joined = out.join("\n");
        assert_eq!(
            joined.matches("dBm").count(),
            4,
            "antenna plus three stages:\n{joined}"
        );
        assert!(
            joined.contains("PREAMPLI"),
            "the name is cut, not dropped:\n{joined}"
        );
    }

    /// The reading stays flush against the right edge whatever the names are.
    ///
    /// The label column grows into the padding rather than pushing the value
    /// along, so a column of levels stays a column: readable down the panel
    /// without the eye having to find each number.
    #[test]
    fn the_reading_stays_flush_right_whatever_the_names_are() {
        // Columns, not bytes: an em dash in the middle column is one column and
        // three bytes, and measuring the wrong one makes aligned rows look ragged.
        let end_column = |line: &str| -> Option<usize> {
            let cols: Vec<char> = line.chars().collect();
            let text: String = cols.iter().collect();
            text.find("dBm")
                .map(|byte| text[..byte].chars().count() + 3)
        };
        let edges = |m: &SdrMetrics| -> Vec<usize> {
            draw(RfChainPanel, W, 30, m)
                .iter()
                .filter_map(|l| end_column(l))
                .collect()
        };
        let short = edges(&with_stages(2, "A"));
        let long = edges(&with_stages(2, "PREAMPLIFIER"));
        assert!(!short.is_empty());
        assert_eq!(short, long, "the name length must not move the reading");
        assert!(
            short.windows(2).all(|w| w[0] == w[1]),
            "and every row in the column agrees: {short:?}"
        );
    }
}
