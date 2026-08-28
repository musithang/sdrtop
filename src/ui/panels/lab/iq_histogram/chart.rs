//! The log-scaled bin chart: filled columns with an outline over their tops.
//!
//! Same visual language as the spectrum trace, so the two read alike. Colour is
//! by zone, which is why the boundaries live in [`super::zones`] rather than as
//! comparisons written out here.

use ratatui::{
    layout::Rect,
    widgets::canvas::{Canvas, Line as CanvasLine},
    Frame,
};

use super::zones::{self, Zone, BINS};

pub(super) fn draw(
    f: &mut Frame,
    area: Rect,
    hist: &[u64; BINS],
    n_bins: usize,
    theme: &crate::Theme,
) {
    // The canvas paint closure is `move`, so every colour it needs is resolved
    // out of the theme first - a borrow of `theme` cannot cross into it.
    let low = theme.label;
    let mid = theme.status_ok;
    let clip = theme.status_crit;
    let color_of = move |bin: f64| match zones::zone_of(bin as usize) {
        Zone::Clip => clip,
        Zone::Mid => mid,
        Zone::Low => low,
    };

    let bins = zones::heights(hist, n_bins);
    f.render_widget(
        Canvas::default()
            .x_bounds([0.0, n_bins as f64])
            .y_bounds([0.0, 1.0])
            .paint(move |ctx| {
                for &(x, h) in &bins {
                    ctx.draw(&CanvasLine {
                        x1: x + 0.5,
                        y1: 0.0,
                        x2: x + 0.5,
                        y2: h,
                        color: color_of(x),
                    });
                }
                // Outline connecting the bin tops, drawn after the fill so it
                // sits over it.
                for i in 1..bins.len() {
                    let (x0, h0) = bins[i - 1];
                    let (x1, h1) = bins[i];
                    ctx.draw(&CanvasLine {
                        x1: x0 + 0.5,
                        y1: h0,
                        x2: x1 + 0.5,
                        y2: h1,
                        color: color_of(x1),
                    });
                }
            }),
        area,
    );
}
