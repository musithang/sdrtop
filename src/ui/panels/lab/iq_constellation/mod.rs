// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `IqConstellationPanel` - 2-D braille dot-cloud of recent I/Q samples.
//!
//! Each frame shows up to `CONSTELLATION_CAP` normalised (I, Q) pairs from
//! the RX hot-path, decimated at least 1 : 1024, **in a square zoomed to the
//! cloud** with dBFS rings, beside the one reading only this picture gives:
//! the envelope, how far from the centre the points sit. Its shape says what
//! the front end is doing: a ring is a constant envelope, a disc is noise, an
//! ellipse is amplitude imbalance and a tilt phase imbalance, an off-centre
//! cloud a DC offset, a cloud pushed square against the edge clipping. The
//! numbers for the imbalance and the DC are IQ Diagnostics', beside it.
//!
//! Split the way a scope is built: what is measured, then what is drawn.
//!
//! - [`fit`]: the covariance ellipse drawn over the cloud. Pure maths.
//! - [`envelope`]: `|z|`, its rms, spread and radial profile. Pure maths.
//! - [`cloud`]: the density buckets and their heat palette.
//! - [`plot`]: the square canvas - rings, cloud, ellipse, DC crosshair.
//! - [`side`]: the column beside it - envelope, scale, key.

mod cloud;
mod envelope;
mod fit;
mod plot;
mod side;

use ratatui::{layout::Rect, style::Style, text::Span, widgets::Paragraph, Frame};

use crate::state::{IqState, SdrMetrics};
use crate::ui::panel::{Panel, PanelChrome, Staleness};

pub struct IqConstellationPanel;

/// Number of line segments used to approximate a circle.
const CIRCLE_SEGS: usize = 48;

/// Columns the side column needs; narrower, the square has the panel alone.
const SIDE_W: u16 = 26;

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

        let coords: Vec<(f64, f64)> = state
            .iq
            .constellation
            .iter()
            .map(|&(i, q)| (i as f64, q as f64))
            .collect();

        let (square, side_area) = carve(inner);
        // The RX poll's scale, held against flicker; before its first window,
        // the one these points want.
        let outer_db = state
            .iq
            .constellation_outer_db
            .map(f64::from)
            .unwrap_or_else(|| {
                let ms =
                    coords.iter().map(|&(i, q)| i * i + q * q).sum::<f64>() / coords.len() as f64;
                IqState::outer_ring_for(10.0 * ms.max(1e-24).log10())
            });
        let r = plot::radius(outer_db);
        let ceiling = state.ceiling_dbfs();
        let face = plot::Face {
            outer_db,
            clip_db: (ceiling < 0.0).then_some(ceiling),
            ellipse: fit::fit_ellipse(&coords),
            dc: (state.iq.dc_offset_i as f64, state.iq.dc_offset_q as f64),
        };
        plot::draw(f, square, cloud::density_layers(&coords, r), &face, theme);

        if let Some(area) = side_area {
            let bins = side::profile_bins(area.width as usize);
            if let Some(env) = envelope::Envelope::of(&coords, r, bins) {
                let scale = side::Scale {
                    outer_db,
                    clip: face.clip_db.zip(state.ceiling()),
                    points: coords.len(),
                };
                f.render_widget(
                    Paragraph::new(side::lines(&env, &scale, area.width as usize, theme)),
                    area,
                );
            }
        }
    }
}

/// The square for the canvas, two columns a row, at the left; the column
/// beside it when there is room for one.
fn carve(inner: Rect) -> (Rect, Option<Rect>) {
    let room = inner.width.saturating_sub(SIDE_W + 1);
    let with_side = room >= 16;
    let across = if with_side { room } else { inner.width };
    let w = (2 * inner.height).min(across) & !1;
    let h = (w / 2).min(inner.height);
    let square = Rect {
        x: inner.x,
        y: inner.y + (inner.height - h) / 2,
        width: w,
        height: h,
    };
    let side = with_side.then(|| Rect {
        x: inner.x + w + 1,
        y: inner.y,
        width: inner.width - w - 1,
        height: inner.height,
    });
    (square, side)
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

    /// A streaming radio whose cloud is a ring of magnitude `amp`.
    fn with_ring(amp: f64) -> SdrMetrics {
        let mut m = SdrMetrics::fixture().streaming();
        m.iq.constellation = tests_support::ring(512, amp, amp)
            .into_iter()
            .map(|(i, q)| (i as f32, q as f32))
            .collect();
        m
    }

    fn draw(m: &SdrMetrics, w: u16, h: u16) -> String {
        crate::state::fixture::draw(IqConstellationPanel, w, h, m).join("\n")
    }

    /// **Square, two columns a row**, so a circle is drawn as one; the
    /// column takes what is left.
    #[test]
    fn the_canvas_is_square_and_the_column_takes_the_rest() {
        let inner = Rect::new(0, 0, 75, 24);
        let (square, side) = carve(inner);
        assert_eq!((square.width, square.height), (48, 24));
        assert_eq!(side.unwrap().width, 26);
        // Too narrow for both: the square has the panel alone.
        let (square, side) = carve(Rect::new(0, 0, 30, 20));
        assert!(side.is_none());
        assert_eq!(square.width, 2 * square.height);
    }

    /// **Zoomed to the cloud, the scale written on it**: a ring at -16 dBFS
    /// is drawn under an outer ring of -9 with rings at -15 and -21, and the
    /// column reads its envelope; no EVM, MER, eccentricity or centroid,
    /// which were IQ Diagnostics' figures measured a second time.
    #[test]
    fn a_ring_is_zoomed_to_and_read_as_an_envelope() {
        let out = draw(&with_ring(0.15), 77, 26);
        for want in [
            "-9",
            "-15",
            "-21",
            "ENVELOPE",
            "SCALE",
            "-16 dBFS",
            "0.0 %",
            "a ring 0 %, noise 52 %",
            "-9 dBFS",
            "6 dB",
            "full scale 9 dB out",
            "512 points drawn",
            "gain, phase, DC",
        ] {
            assert!(out.contains(want), "{want:?} in\n{out}");
        }
        for gone in ["EVM", "MER", "ecc", "tilt", "centroid"] {
            assert!(!out.contains(gone), "{gone:?} in\n{out}");
        }
    }

    /// The RX poll's held scale wins over the one the points would choose,
    /// so the picture does not change scale under them between polls.
    #[test]
    fn the_polls_scale_is_the_one_drawn() {
        let mut m = with_ring(0.2);
        m.iq.constellation_outer_db = Some(-3.0);
        let out = draw(&m, 77, 26);
        assert!(
            out.contains("-3 dBFS") && out.contains("full scale 3 dB out"),
            "{out}"
        );
    }

    /// On a radio that clips before its converter, the clip is drawn inside
    /// the view and named; at full scale the outer ring is full scale.
    #[test]
    fn the_tuners_clip_is_named_where_it_is_in_view() {
        let mut m = with_ring(0.6);
        m = m.band_measured_chain();
        m.iq.constellation_outer_db = Some(0.0);
        let out = draw(&m, 77, 26);
        assert!(out.contains("full scale"), "{out}");
        let mut far = with_ring(0.6).band_measured_chain();
        far.iq.constellation_outer_db = Some(-1.0);
        let out = draw(&far, 77, 26);
        assert!(out.contains("tuner clips -3"), "{out}");
        // A ceiling that holds says so.
        let mut hf = with_ring(0.6).airspyhf();
        hf.iq.constellation_outer_db = Some(-1.0);
        let out = draw(&hf, 77, 26);
        assert!(out.contains("digital AGC holds -12"), "{out}");
    }

    /// The smallest panel draws the square alone and does not panic.
    #[test]
    fn the_smallest_panel_draws_the_square_alone() {
        let out = draw(&with_ring(0.2), 18, 10);
        assert!(!out.contains("ENVELOPE"), "{out}");
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
