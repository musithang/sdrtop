//! `ImageScopePanel` — the LO-centred image-rejection scope for the Lab IQ preset.
//!
//! Reads the existing fftshifted FFT frame (DC/LO at the centre bin) and tells the
//! quadrature story directly: the strongest **carrier**, its **mirror image**
//! reflected about the LO, and the residual **DC spike** at the centre. The gap
//! between carrier and image is the measured image suppression — the empirical
//! counterpart to the computed IRR shown one panel left.
//!
//! Split the same way as its bench partner `iq_constellation`: what is measured,
//! then what is drawn.
//!
//! - [`detect`]: where the carrier and its mirror are. Pure, and `pub(crate)` at
//!   the [`carrier_image`] end so the marker bar and the `[M]` pin get the same
//!   answer the scope draws.
//! - [`chart`]: the block-cell bar chart and the frequency window it spans.
//! - [`readout`]: the four lines under it, and the caption.
//! - [`tint`]: the three colours carrier / image / DC wear everywhere.

mod chart;
mod detect;
mod readout;
mod tint;

pub(crate) use detect::carrier_image;

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
    pub carrier_hz:     u64,
    pub image_hz:       u64,
    pub carrier_dbfs:   f32,
    pub image_dbfs:     f32,
    /// carrier − image, in dB (positive = image is below the carrier).
    pub suppression_db: f32,
}

impl Panel for ImageScopePanel {
    fn name(&self) -> &'static str { "image_scope" }
    fn min_size(&self) -> (u16, u16) { (28, 12) }

    fn chrome(&self, _state: &SdrMetrics) -> PanelChrome {
        PanelChrome::new("Image-Rejection Scope").stale_when(Staleness::NotStreaming)
    }

    fn render(&self, f: &mut Frame, inner: Rect, state: &SdrMetrics, theme: &crate::Theme, _focused: bool) {
        if !state.radio.hw_streaming {
            return placeholder(f, inner, "\u{2014}\u{2014}\u{2014}", theme);
        }
        let Some(frame) = state.waterfall.last_fft.as_ref() else {
            return placeholder(f, inner, "Waiting for RX\u{2026}", theme);
        };
        let hint = detect::carrier_hint_bin(state, frame);
        let bins = &frame.bins_dbfs;
        let rate = frame.sample_rate;
        let Some(r) = detect::detect_image(bins, rate, frame.noise_floor, hint) else {
            return placeholder(f, inner, "No signal yet\u{2026}", theme);
        };

        let iw = inner.width as usize;
        let ih = inner.height as usize;
        let tint = tint::Tint::new(theme);
        let win = chart::Window::new(frame.center_freq_hz as f64, r.carrier_offset_hz, rate);

        // The read-outs are always shown, so they are built first and the chart
        // gets whatever height is left over.
        let readouts = readout::lines(&r, win.carrier_hz, &tint, iw, theme);
        let chart_w = iw.saturating_sub(chart::GUTTER + 1);
        let reserved = 1 /*marker*/ + 1 /*axis*/ + 1 /*gap*/ + readouts.len() + 1 /*caption*/;
        // Fill all the vertical room left after the chrome — a taller chart gives
        // finer dBFS resolution and matches the mockup's full-height scope.
        let chart_h = ih.saturating_sub(reserved);

        let mut lines: Vec<Line> = Vec::new();
        chart::draw(&mut lines, bins, rate, &win, chart_w, chart_h, &tint);
        lines.extend(readouts);
        lines.push(readout::caption(theme));

        // Self-adjusting density: drop only as many airy spacers as needed to fit,
        // spread evenly, so a short pane keeps balanced breathing room. (chrome)
        crate::ui::chrome::collapse_spacers(&mut lines, ih);
        f.render_widget(Paragraph::new(lines), inner);
    }
}

/// The three "nothing to plot" states: RX stopped, no frame yet, no carrier in
/// the frame. All neutral — none of them is a fault.
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
}
