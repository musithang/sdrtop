// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `IqConstellationPanel` - 2-D braille dot-cloud of recent I/Q samples.
//!
//! Each frame shows up to `CONSTELLATION_CAP` normalised (I, Q) pairs from
//! the RX hot-path, decimated 1 : 1024. The cloud's position reveals the DC
//! offset; its shape reveals amplitude/phase imbalance (circular = perfect,
//! elliptical = amplitude imbalance, tilted = phase imbalance). A unit circle
//! and faint I/Q axes give a fixed reference frame.
//!
//! Split the way a scope is built: what is measured, then what is drawn.
//!
//! - [`fit`]: the covariance ellipse and the stats read off it. Pure maths.
//! - [`cloud`]: the density buckets and their heat palette.
//! - [`plot`]: the canvas - reference frame, cloud, ellipse, DC crosshair.
//! - [`overlay`]: the text boxes drawn over the canvas.

mod cloud;
mod fit;
mod overlay;
mod plot;

use ratatui::{layout::Rect, style::Style, text::Span, widgets::Paragraph, Frame};

use crate::state::SdrMetrics;
use crate::ui::panel::{Panel, PanelChrome, Staleness};

pub struct IqConstellationPanel;

/// The canvas coordinate system, shared by everything that reads or draws it.
///
/// `BOUND` is the half-extent - slightly wider than the unit circle so the circle
/// border and labels are not clipped - and it is what `cloud::density_layers` bins
/// against as well as what `plot` sets as the canvas bounds. The two must agree:
/// a density grid measured over a different extent than the one drawn would colour
/// the wrong points hot.
const BOUND: f64 = 1.3;

/// Number of line segments used to approximate the unit circle.
const CIRCLE_SEGS: usize = 48;

impl Panel for IqConstellationPanel {
    fn name(&self) -> &'static str {
        "iq_constellation"
    }
    fn min_size(&self) -> (u16, u16) {
        (18, 10)
    }

    fn chrome(&self, _state: &SdrMetrics) -> PanelChrome {
        PanelChrome::new("IQ Constellation").stale_when(Staleness::NotStreaming)
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
            return placeholder(f, inner, "Waiting for RX\u{2026}", theme);
        }
        if state.iq.constellation.is_empty() {
            return placeholder(f, inner, "No samples yet\u{2026}", theme);
        }

        // Pre-collect coords into an owned Vec so the paint closure can own them.
        let coords: Vec<(f64, f64)> = state
            .iq
            .constellation
            .iter()
            .map(|&(i, q)| (i as f64, q as f64))
            .collect();

        // Density-coloured layers (cool→hot) and the fitted imbalance ellipse,
        // computed once outside the paint closure and moved in.
        let layers = cloud::density_layers(&coords);
        let ellipse = fit::fit_ellipse(&coords);
        let stats = fit::cloud_stats(&coords, ellipse);
        let dc = (state.iq.dc_offset_i as f64, state.iq.dc_offset_q as f64);

        plot::draw(f, inner, layers, ellipse, dc, theme);
        overlay::draw(f, inner, &stats, theme);
    }
}

/// The two "nothing to plot" states. Both are neutral: an idle scope is not a
/// fault, and neither reads as a measurement.
fn placeholder(f: &mut Frame, inner: Rect, text: &'static str, theme: &crate::Theme) {
    f.render_widget(
        Paragraph::new(Span::styled(text, Style::default().fg(theme.label))),
        inner,
    );
}

/// A ring of points, the shape both the fit and the density tests are written
/// against: `scale_i`/`scale_q` stretch it into the ellipse a given imbalance
/// would produce.
#[cfg(test)]
mod tests_support {
    use std::f64::consts::PI;

    pub(super) fn ring(n: usize, scale_i: f64, scale_q: f64) -> Vec<(f64, f64)> {
        (0..n)
            .map(|k| {
                let a = 2.0 * PI * k as f64 / n as f64;
                (scale_i * a.cos(), scale_q * a.sin())
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_name_and_min_size() {
        let p = IqConstellationPanel;
        assert_eq!(p.name(), "iq_constellation");
        let (w, h) = p.min_size();
        assert!(w > 0 && h > 0);
    }

    #[test]
    fn circle_segs_constant_is_positive_even() {
        // A `const` block, so an edit that breaks the invariant fails the build
        // rather than a test run. An odd segment count draws a circle whose two
        // halves do not meet.
        const { assert!(CIRCLE_SEGS > 0) };
        const {
            assert!(
                CIRCLE_SEGS.is_multiple_of(2),
                "even segments give a symmetric circle"
            )
        };
    }
}
