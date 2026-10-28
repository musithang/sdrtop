// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `IqDiagnosticsPanel` - the front end's I/Q health, as three stacked blocks.
//!
//! The DC at the LO against the signal, the quadrature balance closed by the
//! image rejection it predicts, then one plain-language verdict and the live
//! correction state. Every reading is the **residual** after
//! any active correction, which is what makes a lit chip beside a bad number
//! meaningful: the correction is not keeping up.
//!
//! Split on the seam a bench instrument actually has - measure, then rate, then
//! draw:
//!
//! - [`reading`]: the numbers, computed once. Pure maths.
//! - [`severity`]: where each of them stops being fine. Thresholds + colours.
//! - [`rows`]: the row vocabulary the sections share, so every meter, bar and
//!   value starts and ends at the same column.
//! - [`dc`], [`quadrature`], [`image`]: the DC block, and the quadrature
//!   block's meters and the image they predict.
//! - [`verdict`]: which single thing to say. Drawing-free.
//! - [`controls`]: the action chips and the calibration's age.
//!
//! This function's job is to carve the stack and call each part once; the parts
//! do not call each other.

mod controls;
mod dc;
mod image;
mod quadrature;
mod reading;
mod rows;
mod severity;
mod verdict;

use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::state::SdrMetrics;
use crate::ui::panel::{Panel, PanelChrome, Staleness};

use reading::Reading;
use rows::Rows;

pub struct IqDiagnosticsPanel;

impl Panel for IqDiagnosticsPanel {
    fn name(&self) -> &'static str {
        "iq_diagnostics"
    }
    fn min_size(&self) -> (u16, u16) {
        (30, 12)
    }
    fn focus_key(&self) -> Option<char> {
        Some('i')
    }
    fn focus_bindings(&self) -> &'static [(&'static str, &'static str)] {
        &[
            ("D", "DC-block"),
            ("C", "auto-cal"),
            ("F", "freeze"),
            ("M", "pin"),
        ]
    }

    fn chrome(&self, _state: &SdrMetrics) -> PanelChrome {
        PanelChrome::new("_IQ Diagnostics").stale_when(Staleness::NotStreaming)
    }

    fn render(
        &self,
        f: &mut Frame,
        inner: Rect,
        state: &SdrMetrics,
        theme: &crate::Theme,
        _focused: bool,
    ) {
        if !state.radio.hw_streaming {
            f.render_widget(
                Paragraph::new(Span::styled("---", Style::default().fg(theme.label))),
                inner,
            );
            return;
        }

        let rows = Rows::new(inner.width as usize, theme);
        let r = Reading::of(state);

        let mut lines: Vec<Line> = Vec::new();
        lines.extend(dc::lines(&r, &rows));
        lines.push(Line::raw(""));
        lines.extend(quadrature::lines(&r, &rows));
        lines.push(Line::raw(""));
        lines.extend(image::lines(state, &r, &rows));
        lines.push(Line::raw(""));
        lines.extend(verdict::lines(&verdict::decide(&r, &state.iq.cal), &rows));
        lines.push(Line::raw(""));
        lines.extend(controls::lines(&state.iq.cal, &rows));

        // Self-adjusting density: collapse spacers when the pane is short, grow
        // them to fill when it is tall, so the stack breathes the same at every
        // height. The section nameplates keep the grouping either way.
        crate::ui::chrome::fit_spacers(&mut lines, inner.height as usize);
        f.render_widget(Paragraph::new(lines), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::fixture::draw;

    fn bench() -> SdrMetrics {
        let mut m = SdrMetrics::fixture().streaming();
        m.iq.dc_offset_i = 0.0008;
        m.iq.dc_offset_q = -0.0011;
        m.iq.iq_imbalance_db = 0.12;
        m.iq.phase_imbalance_deg = 0.4;
        m
    }

    /// A stopped radio shows no readings at all, rather than the last ones it
    /// happened to have. The frame's `[STALE]` tag is the engine's job.
    #[test]
    fn a_stopped_radio_draws_no_readings() {
        let out = draw(IqDiagnosticsPanel, 60, 24, &SdrMetrics::fixture()).join("\n");
        assert!(out.contains("---"), "{out}");
        assert!(
            !out.contains("DC AT THE LO"),
            "stale panel drew a section:\n{out}"
        );
        assert!(out.contains("STALE"), "the frame should be tagged:\n{out}");
    }

    /// **On a radio whose LO is off the centre the DC is not called the
    /// LO's.** The section says it measures 0 Hz, and the LO's own line is
    /// read off the spectrum where the LO is: here 20 dB over the -100 floor,
    /// 5 kHz up. With the LO at the centre the section reads as it always has.
    #[test]
    fn an_lo_off_the_centre_is_read_where_it_is() {
        let mut m = bench().with_carrier(1_000_000.0, 40.0);
        m.radio.lo_offset_hz = 5_000;
        let fr = m.waterfall.last_fft.as_mut().unwrap();
        let n = fr.bins_dbfs.len();
        let at = (n as f64 / 2.0 + 5_000.0 * n as f64 / fr.sample_rate).round() as usize;
        let mut bins = (*fr.bins_dbfs).clone();
        bins[at] = -80.0;
        fr.bins_dbfs = std::sync::Arc::new(bins);
        let out = draw(IqDiagnosticsPanel, 60, 34, &m).join("\n");
        assert!(out.contains("DC AT 0 HZ"), "{out}");
        assert!(!out.contains("DC AT THE LO"), "{out}");
        assert!(out.contains("-80 dBFS/bin"), "{out}");
        assert!(out.contains("at +5.0 kHz"), "{out}");
        let centred = draw(IqDiagnosticsPanel, 60, 34, &bench()).join("\n");
        assert!(centred.contains("DC AT THE LO"), "{centred}");
        assert!(!centred.contains("dBFS/bin"), "{centred}");
    }

    /// **Where the samples are turned off the LO, the balance is not the
    /// front end's, and nothing judges it.** A gain imbalance that would be a
    /// critical verdict on a radio mixing to 0 Hz gives no verdict, no meters
    /// and no predicted image here: the section says why, and the verdict
    /// judges the DC alone.
    #[test]
    fn a_balance_turned_off_the_lo_is_not_judged() {
        let mut m = bench();
        m.iq.iq_imbalance_db = 4.0;
        m.radio.lo_offset_hz = 5_000;
        let out = draw(IqDiagnosticsPanel, 60, 34, &m).join("\n");
        assert!(out.contains("not measured here"), "{out}");
        assert!(
            out.contains("Not measured: the samples are turned"),
            "{out}"
        );
        assert!(out.contains("+5.0 kHz off"), "{out}");
        assert!(!out.contains("QUADRATURE IMBALANCE"), "{out}");
        assert!(!out.contains("+4.00 dB"), "no gain meter: {out}");
        assert!(!out.contains("as gain and phase predict it"), "{out}");
        assert!(out.contains("DC OK"), "{out}");
        assert!(out.contains("LO    at +5.0 kHz, no spectrum yet"), "{out}");
        m.radio.lo_offset_hz = 0;
        let centred = draw(IqDiagnosticsPanel, 60, 34, &m).join("\n");
        assert!(centred.contains("QUADRATURE IMBALANCE"), "{centred}");
    }

    /// The blocks are present in the order the panel promises: the DC, the
    /// quadrature and the image it predicts, the verdict, the chips.
    #[test]
    fn the_blocks_appear_in_order() {
        let out = draw(IqDiagnosticsPanel, 60, 30, &bench()).join("\n");
        let dc = out.find("DC AT THE LO").expect("no DC block");
        let quad = out.find("QUADRATURE").expect("no quadrature block");
        let img = out.find("as gain and phase predict it").expect("no image");
        let verdict = out.find("IQ QUALITY OK").expect("no verdict");
        let chips = out.find("DC-block").expect("no chips");
        assert!(
            dc < quad && quad < img && img < verdict && verdict < chips,
            "out of order:\n{out}"
        );
    }

    /// **The DC is read against the signal**: a HackRF on FM carried its DC
    /// 12 dB over everything it received, and the panel says so in those
    /// words, with the level, the scale and the I and Q split.
    #[test]
    fn the_dc_is_read_against_the_signal() {
        let mut m = SdrMetrics::fixture().streaming();
        m.iq.dc_offset_i = -0.0052;
        m.iq.dc_offset_q = -0.0223;
        m.signal.adc_rms_dbfs = -45.1;
        m.iq.iq_imbalance_db = 0.65;
        m.iq.phase_imbalance_deg = 0.09;
        let out = draw(IqDiagnosticsPanel, 53, 26, &m).join("\n");
        for want in [
            "-33 dBFS",
            "12 dB over the signal",
            "\u{25cf}",
            "rms",
            "-0.52 % \u{00b7} -2.23 % of full scale",
            "gain",
            "+0.65 dB",
            "phase",
            "28.5 dB",
            "measured on the scope",
            "DC OFFSET HIGH",
            "The DC is 12 dB over the signal.",
            "no cal yet",
        ] {
            assert!(out.contains(want), "{want:?} in\n{out}");
        }
        for gone in ["MAG", "IMAGE REJECTION", "DC-block OFF", "\u{2212}28"] {
            assert!(!out.contains(gone), "{gone:?} in\n{out}");
        }
    }

    /// The chips fall back to single letters before the freeze chip would be
    /// clipped off the right edge.
    #[test]
    fn narrow_panels_drop_to_single_letter_chips() {
        let wide = draw(IqDiagnosticsPanel, 60, 30, &bench()).join("\n");
        assert!(wide.contains("DC-block "), "full labels missing:\n{wide}");

        let narrow = draw(IqDiagnosticsPanel, 34, 30, &bench()).join("\n");
        assert!(
            !narrow.contains("D DC-block"),
            "narrow panel kept the full chip labels:\n{narrow}"
        );
    }

    /// The panel fills whatever height it is given without spilling out of it.
    #[test]
    fn it_fits_every_height_from_min_size_upward() {
        let m = bench();
        let (min_w, min_h) = IqDiagnosticsPanel.min_size();
        for (w, h) in [(min_w, min_h), (44, 18), (60, 30), (80, 40)] {
            let out = draw(IqDiagnosticsPanel, w, h, &m);
            assert_eq!(out.len(), h as usize, "{w}x{h}: wrong row count");
            assert!(
                out.iter().all(|l| l.chars().count() <= w as usize),
                "{w}x{h}: a row overran the panel"
            );
        }
    }
}
