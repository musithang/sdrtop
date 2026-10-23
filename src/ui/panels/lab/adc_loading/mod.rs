// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `AdcLoadingPanel` - the ADC Loading column of the Lab RF bench (`[6]`).
//!
//! How hard the converter is actually driven: the **bit ladder** (how many
//! of its bits the samples use, one 6 dB band a bit, on the bench's dBFS
//! axis with where the peak belongs and where the radio clips), the staging
//! **verdict** with the distance to that clip, the **loading** read-out (peak,
//! rms, bits reached, clips), and a **modeled linearity** card where the chain
//! is one sdrtop models. The thesis: fill the converter without hitting the
//! clip, which is what positions the signal/noise gap the other two panels
//! draw.
//!
//! - [`ladder`]: the bands, the axis, the marks and the key.
//! - [`readouts`]: the blocks under it.
//! - [`rf_bench`](super::rf_bench): the row vocabulary shared with `rf_chain`,
//!   the panel this one is the other half of.

mod ladder;
mod readouts;

use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::state::SdrMetrics;
use crate::ui::panel::{Panel, PanelChrome, Staleness, Tag};
use crate::ui::rf_calc::{adc_loading, staging_verdict};

use super::rf_bench::severity_color;

pub struct AdcLoadingPanel;

/// Rows of the linearity card with the blank above it: the section line,
/// three rows and the caption.
const LINEARITY_ROWS: usize = 6;

impl Panel for AdcLoadingPanel {
    fn name(&self) -> &'static str {
        "adc_loading"
    }
    fn min_size(&self) -> (u16, u16) {
        (30, 18)
    }

    fn chrome(&self, state: &SdrMetrics) -> PanelChrome {
        PanelChrome::new("ADC Loading")
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
        if inner.width == 0 || inner.height == 0 {
            return;
        }
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

        let iw = inner.width as usize;
        let ih = inner.height as usize;

        // --- model (frozen snapshot when held, else live) ----------------------
        let fz = state.lab.rf_freeze.as_ref();
        let bands = fz.map(|f| &f.band_hist).unwrap_or(&state.iq.adc_band_hist);
        let n: u64 = bands.iter().sum();
        let peak = fz
            .map(|f| f.peak_dbfs)
            .unwrap_or(state.signal.adc_peak_dbfs) as f64;
        let rms = fz.map(|f| f.rms_dbfs).unwrap_or(state.signal.adc_rms_dbfs) as f64;
        let clip = fz
            .map(|f| f.clip_events)
            .unwrap_or(state.signal.adc_clip_events);
        let (lna_g, vga_g) = fz
            .map(|f| (f.lna_gain, f.vga_gain))
            .unwrap_or((state.radio.primary_gain(), state.radio.secondary_gain()));
        let bits = state.caps.sample_geometry.bits();
        let load = adc_loading(peak, rms, clip, n, bits);
        // Judged against this radio's ceiling, where its clip happens.
        let ceiling = state.ceiling_dbfs();
        let part = state.caps.gain.ceiling_part();
        let (verdict, sev) = staging_verdict(peak - ceiling);
        let sev_col = severity_color(sev, theme);

        // The ladder takes the height the read-out under it leaves.
        let below = 1
            + 5
            + usize::from(part.is_some())
            + if state.caps.friis_applicable {
                LINEARITY_ROWS
            } else {
                0
            };
        let bars = ih.saturating_sub(ladder::CHROME_ROWS + below).clamp(3, 16);

        let mut lines: Vec<Line> = Vec::new();
        ladder::draw(
            &mut lines,
            bands,
            &ladder::Levels {
                bits: bits as usize,
                peak_dbfs: peak,
                rms_dbfs: load.rms_dbfs,
                ceiling_dbfs: ceiling,
                sev_col,
            },
            iw,
            bars,
            theme,
        );
        lines.push(Line::raw(""));
        readouts::loading(&mut lines, &load, ceiling, part, verdict, sev_col, theme);
        if let Some(part) = part {
            readouts::ceiling(&mut lines, ceiling, part, theme);
        }
        // The quantisation ceiling on that card follows the device, but its
        // IIP3, IMD3 and SFDR figures are a specific front end's datasheet
        // wearing a generic name. Drawn only where the chain is one sdrtop
        // actually models, which is the same flag the Friis cascade uses and for
        // the same reason.
        if state.caps.friis_applicable {
            lines.push(Line::raw(""));
            readouts::linearity_card(&mut lines, lna_g, vga_g, bits, iw, theme);
        }

        // Self-adjusting density: collapse spacers when short, grow them to fill when
        // tall (chrome::fit_spacers), so the pane breathes the same at every height -
        // consistent with the other lab side panels.
        crate::ui::chrome::fit_spacers(&mut lines, ih);
        f.render_widget(Paragraph::new(lines), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_name_and_min_size() {
        let p = AdcLoadingPanel;
        assert_eq!(p.name(), "adc_loading");
        let (w, h) = p.min_size();
        assert!(w >= 16 && h >= 8);
    }

    /// The linearity card is a specific front end's datasheet. It is drawn for
    /// the radio it describes and for nothing else.
    ///
    /// Everything above it, the histogram and the loading read-out, is a real
    /// measurement of whatever converter is attached, so it stays.
    #[test]
    fn the_modeled_linearity_card_is_only_drawn_for_a_chain_we_model() {
        let hackrf = crate::state::fixture::draw(
            AdcLoadingPanel,
            60,
            30,
            &crate::state::SdrMetrics::fixture().streaming(),
        )
        .join("\n");
        assert!(hackrf.contains("LINEARITY"), "{hackrf}");
        assert!(hackrf.contains("IIP3"), "{hackrf}");

        let unknown = crate::state::fixture::draw(
            AdcLoadingPanel,
            60,
            30,
            &crate::state::SdrMetrics::fixture()
                .streaming()
                .named_chain(),
        )
        .join("\n");
        assert!(
            !unknown.contains("IIP3"),
            "a modeled intercept point for a chain we do not know:\n{unknown}"
        );
        assert!(
            unknown.contains("reached by the peak"),
            "the measured half must survive:\n{unknown}"
        );
    }

    /// The converter depth reaches the read-out. A 14-bit device must not be
    /// described with an 8-bit ceiling.
    #[test]
    fn the_readout_names_the_devices_own_bit_depth() {
        let unknown = crate::state::fixture::draw(
            AdcLoadingPanel,
            60,
            30,
            &crate::state::SdrMetrics::fixture()
                .streaming()
                .named_chain(),
        )
        .join("\n");
        assert!(unknown.contains(" of 14 "), "{unknown}");
        assert!(unknown.contains("14-bit"), "{unknown}");
    }

    /// An Airspy R2 on FM at Linearity 14, with the band counts a real
    /// capture of it gave (92.8 MHz, 2026-10-23).
    fn r2(peak: f32, rms: f32, bands: [u64; 12]) -> SdrMetrics {
        let mut m = SdrMetrics::fixture().streaming().band_measured_chain();
        let mut caps = (*m.caps).clone();
        caps.sample_geometry = crate::hardware::native::airspy::caps().sample_geometry;
        m.caps = std::sync::Arc::new(caps);
        m.signal.adc_peak_dbfs = peak;
        m.signal.adc_rms_dbfs = rms;
        m.iq.adc_band_hist[..12].copy_from_slice(&bands);
        m
    }

    const R2_LIN14: [u64; 12] = [
        0, 758, 3475937, 1336589, 600447, 293572, 146683, 73479, 36502, 18347, 9099, 8587,
    ];

    /// **The ladder says how much of the converter is used**: a column a
    /// bit, each with its share, on the dBFS axis with where the peak
    /// belongs and where the tuner clips; the verdict and its distance to
    /// that clip; the loading per axis. At the bench's own size on a laptop.
    #[test]
    fn an_r2_on_fm_reads_its_bits_and_its_clip() {
        let out = crate::state::fixture::draw(AdcLoadingPanel, 61, 26, &r2(-10.5, -13.8, R2_LIN14))
            .join("\n");
        for want in [
            "12-bit",
            "58",
            "22",
            "10",
            "-72",
            "-12",
            "\u{25b2} peak",
            "\u{25b3}",
            "\u{2501} where the peak belongs",
            "\u{250a} where it clips",
            "WELL-STAGED  7 dB under the tuner's clip",
            "-10 dBFS",
            "-17 dBFS",
            "per axis, crest 6.3 dB",
            "10.3 of 12",
            "reached by the peak",
            "none",
            "of 6.0M samples",
            "tuner clips at -3 dBFS, before the ADC: uncounted",
        ] {
            assert!(out.contains(want), "{want:?} in\n{out}");
        }
        assert!(!out.contains("ENOB") && !out.contains("HDRM"), "{out}");
    }

    /// **At the R2's ceiling the ADC bench says so**: a peak past -3 dBFS is
    /// clipping in the tuner, before the converter, which the clip counter
    /// cannot see; the counter's `none` is not left to speak alone.
    #[test]
    fn at_the_tuners_ceiling_the_bench_says_the_tuner_clips() {
        let hot = [
            4959545, 572451, 235810, 116678, 57705, 28802, 14662, 7128, 3653, 1812, 899, 855,
        ];
        let out =
            crate::state::fixture::draw(AdcLoadingPanel, 61, 26, &r2(-1.4, -1.0, hot)).join("\n");
        assert!(
            out.contains("CLIPPING  2 dB past the tuner's clip"),
            "{out}"
        );
        assert!(out.contains("83"), "{out}");
        assert!(
            out.contains("tuner clips at -3 dBFS, before the ADC: uncounted"),
            "{out}"
        );
        let hackrf = crate::state::fixture::draw(
            AdcLoadingPanel,
            61,
            40,
            &SdrMetrics::fixture().streaming(),
        )
        .join("\n");
        assert!(!hackrf.contains("tuner clips"), "{hackrf}");
        assert!(!hackrf.contains("where it clips"), "{hackrf}");
        assert!(hackrf.contains("under full scale"), "{hackrf}");
    }

    /// A HackRF on FM at a modest gain uses three of its eight bits, and the
    /// ladder shows the five empty ones (a real recording, 2026-09-27).
    #[test]
    fn a_weak_hackrf_shows_the_bits_it_leaves_empty() {
        let mut m = SdrMetrics::fixture().streaming();
        m.signal.adc_peak_dbfs = -28.2;
        m.signal.adc_rms_dbfs = -32.5;
        m.iq.adc_band_hist[..8].copy_from_slice(&[0, 0, 0, 0, 216965, 3869326, 2505593, 1408116]);
        let out = crate::state::fixture::draw(AdcLoadingPanel, 61, 26, &m).join("\n");
        for want in [
            "8-bit",
            "48",
            "31",
            "18",
            "WEAK  28 dB under full scale",
            "3.3 of 8",
            "LINEARITY",
        ] {
            assert!(out.contains(want), "{want:?} in\n{out}");
        }
    }

    /// Before the first window there is nothing to stack, and it says so.
    #[test]
    fn no_samples_yet_says_so() {
        let out = crate::state::fixture::draw(
            AdcLoadingPanel,
            61,
            26,
            &SdrMetrics::fixture().streaming(),
        )
        .join("\n");
        assert!(out.contains("no samples yet"), "{out}");
    }

    /// The smallest panel and the deepest converter: nothing panics, and the
    /// verdict survives.
    #[test]
    fn a_sixteen_bit_converter_fits_the_smallest_panel() {
        let mut m = SdrMetrics::fixture().streaming();
        let mut caps = (*m.caps).clone();
        caps.sample_geometry = crate::hardware::SampleGeometry {
            format: crate::hardware::SampleFormat::Int16,
            full_scale: 32768.0,
            converter_bits: None,
        };
        m.caps = std::sync::Arc::new(caps);
        m.iq.adc_band_hist = [1; crate::hardware::ADC_BANDS];
        let out = crate::state::fixture::draw(AdcLoadingPanel, 30, 18, &m).join("\n");
        assert!(
            out.contains("16-bit") || out.contains("WELL-STAGED"),
            "{out}"
        );
    }
}
