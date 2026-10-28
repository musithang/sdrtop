// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `ImageScopePanel` - the LO-centred image-rejection scope for the Lab IQ preset.
//!
//! Reads the existing fftshifted FFT frame (DC/LO at the centre bin) and tells the
//! quadrature story directly: the strongest **carrier** and its **mirror**
//! reflected about the LO. The gap between them, drawn on the mirror's column,
//! is the measured image suppression - the empirical counterpart to the IRR
//! the imbalance predicts one panel left - where it is one: a mirror on the
//! floor is a lower bound, and a signal running across the LO has no image
//! to measure at all ([`detect::Reading`]).
//!
//! Split the same way as its bench partner `iq_constellation`: what is measured,
//! then what is drawn.
//!
//! - [`detect`]: where the carrier and its mirror are. Pure, and `pub(crate)` at
//!   the [`carrier_image`] end so the marker bar and the `[M]` pin get the same
//!   answer the scope draws.
//! - [`chart`]: the block-cell bar chart and the frequency window it spans.
//! - [`readout`]: the lines under it, and the caption.
//! - [`tint`]: the colours carrier and image wear everywhere.

mod chart;
mod detect;
mod readout;
mod tint;

pub(crate) use detect::{carrier_image, Reading};

use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::state::SdrMetrics;
use crate::ui::panel::{Panel, PanelChrome, Staleness};

pub struct ImageScopePanel;

/// Carrier + mirror image read-out in absolute terms, for the Lab IQ marker bar.
///
/// Lives here rather than in [`detect`] because it is this module's published
/// result: `image_scope::CarrierImage` is the path other code names it by, the
/// same way `signal_characterization::VerdictLevel` works.
pub(crate) struct CarrierImage {
    pub carrier_hz: u64,
    pub image_hz: u64,
    pub carrier_dbfs: f32,
    pub image_dbfs: f32,
    /// What the gap between them can honestly be called.
    pub reading: Reading,
}

impl Panel for ImageScopePanel {
    fn name(&self) -> &'static str {
        "image_scope"
    }
    fn min_size(&self) -> (u16, u16) {
        (28, 12)
    }

    fn chrome(&self, _state: &SdrMetrics) -> PanelChrome {
        PanelChrome::new("Image-Rejection Scope").stale_when(Staleness::NotStreaming)
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
            return placeholder(f, inner, "\u{2014}\u{2014}\u{2014}", theme);
        }
        let Some(frame) = state.waterfall.last_fft.as_ref() else {
            return placeholder(f, inner, "Waiting for RX\u{2026}", theme);
        };
        let hint = detect::carrier_hint_bin(state, frame);
        let bins = &frame.bins_dbfs;
        let rate = frame.sample_rate;
        let lo = detect::lo_bin(state, frame);
        let Some(r) = detect::detect_image(bins, rate, frame.noise_floor, hint, lo) else {
            return placeholder(f, inner, "No signal yet\u{2026}", theme);
        };

        let iw = inner.width as usize;
        let ih = inner.height as usize;
        let tint = tint::Tint::new(theme);
        let win = chart::Window::new(
            frame.center_freq_hz as f64,
            state.radio.lo_offset_hz as f64,
            r.carrier_offset_hz,
            rate,
        );

        // The read-outs are always shown, so they are built first and the chart
        // gets whatever height is left over.
        let readouts = readout::lines(&r, win.carrier_hz, win.image_hz, &tint, iw, theme);
        let chart_w = iw.saturating_sub(chart::GUTTER + 1);
        let reserved = 1 /*marker*/ + 1 /*axis*/ + 1 /*gap*/ + readouts.len() + 1 /*caption*/;
        let chart_h = ih.saturating_sub(reserved);
        let gap = readout::gap_label(r.reading).map(|label| chart::Gap {
            carrier_dbfs: r.carrier_dbfs,
            image_dbfs: r.image_dbfs,
            label,
        });

        let mut lines: Vec<Line> = Vec::new();
        chart::draw(
            &mut lines,
            bins,
            rate,
            frame.noise_floor,
            &win,
            gap.as_ref(),
            chart_w,
            chart_h,
            &tint,
        );
        lines.extend(readouts);
        lines.push(readout::caption(theme));

        // Self-adjusting density: drop only as many airy spacers as needed to fit,
        // spread evenly, so a short pane keeps balanced breathing room. (chrome)
        crate::ui::chrome::collapse_spacers(&mut lines, ih);
        f.render_widget(Paragraph::new(lines), inner);
    }
}

/// The three "nothing to plot" states: RX stopped, no frame yet, no carrier in
/// the frame. All neutral - none of them is a fault.
fn placeholder(f: &mut Frame, inner: Rect, text: &'static str, theme: &crate::Theme) {
    f.render_widget(
        Paragraph::new(Span::styled(text, Style::default().fg(theme.label))),
        inner,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_name_is_stable() {
        assert_eq!(ImageScopePanel.name(), "image_scope");
    }

    /// A carrier 1 MHz off the LO at 40 dB over a -100 floor, its frame
    /// edited by `edit`.
    fn scope(edit: impl FnOnce(&mut Vec<f32>)) -> String {
        let mut m = SdrMetrics::fixture()
            .streaming()
            .with_carrier(1_000_000.0, 40.0);
        let fr = m.waterfall.last_fft.as_mut().unwrap();
        let mut bins = (*fr.bins_dbfs).clone();
        edit(&mut bins);
        fr.bins_dbfs = std::sync::Arc::new(bins);
        crate::state::fixture::draw(ImageScopePanel, 61, 26, &m).join("\n")
    }

    /// **The gap is drawn where it is measured**: the carrier's level carried
    /// to its mirror's column, the rejection beside it, and the verdict in
    /// plain words, positive.
    #[test]
    fn an_image_over_the_floor_is_measured_and_drawn() {
        // 1 MHz at 10 Msps over 256 bins is 25.6 bins: carrier 154, mirror 102.
        let out = scope(|b| b[102] = -90.0);
        for want in [
            "\u{25bc} carrier",
            "\u{25bd} mirror",
            "IMAGE 30 dB DOWN",
            "30 dB",
            "\u{254c}",
            "LO ",
            "the IRR its imbalance predicts",
        ] {
            assert!(out.contains(want), "{want:?} in\n{out}");
        }
        assert!(!out.contains("DC spike") && !out.contains("supp"), "{out}");
    }

    /// **The mirror is looked for about the LO the radio reports.** With the
    /// LO half a megahertz above the centre, the 1 MHz carrier's mirror is at
    /// the centre (bin 128), not at -1 MHz: an image there is measured, and
    /// the same frame read as if the LO were at the centre finds only the
    /// floor where it looks.
    #[test]
    fn the_mirror_follows_an_lo_off_the_centre() {
        let draw = |lo_offset_hz: i64| {
            let mut m = SdrMetrics::fixture()
                .streaming()
                .with_carrier(1_000_000.0, 40.0);
            m.radio.lo_offset_hz = lo_offset_hz;
            let fr = m.waterfall.last_fft.as_mut().unwrap();
            let mut bins = (*fr.bins_dbfs).clone();
            bins[128] = -90.0;
            fr.bins_dbfs = std::sync::Arc::new(bins);
            crate::state::fixture::draw(ImageScopePanel, 61, 26, &m).join("\n")
        };
        let off = draw(500_000);
        assert!(off.contains("IMAGE 30 dB DOWN"), "{off}");
        let centred = draw(0);
        assert!(centred.contains("IMAGE \u{2265} 40 dB DOWN"), "{centred}");
    }

    /// A mirror on the floor is a lower bound, said as one.
    #[test]
    fn a_mirror_on_the_floor_is_a_lower_bound() {
        let out = scope(|_| {});
        assert!(out.contains("IMAGE \u{2265} 40 dB DOWN"), "{out}");
        assert!(out.contains("under the floor"), "{out}");
    }

    /// **A signal across the LO gets no figure**: the case a station tuned
    /// to the centre showed as a few dB of image rejection.
    #[test]
    fn a_signal_across_the_lo_gets_no_figure() {
        let out = scope(|b| {
            for v in b.iter_mut().take(156).skip(100) {
                *v = -70.0;
            }
        });
        assert!(out.contains("NO IMAGE TO MEASURE"), "{out}");
        assert!(out.contains("runs across the LO"), "{out}");
        assert!(
            !out.contains("dB DOWN") && !out.contains('\u{25bd}'),
            "{out}"
        );
    }
}
