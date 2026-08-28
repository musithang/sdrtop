//! The trace itself: the braille dot-matrix canvas the spectrum is drawn on.
//!
//! ratatui's `Canvas` rasterises every shape onto a 2x4 braille dot grid per
//! cell, which is what gives the phosphor dot-matrix instrument look at a
//! density that does not change with terminal size.
//!
//! The layers are drawn back to front, and the order is the whole design:
//! graticule, hold ghost, filled body, live edge, peak hold, reference lines,
//! then the full-height rules (markers, OBW, cursor) last so nothing buries the
//! thing the user placed by hand.

use std::sync::Arc;

use ratatui::{
    layout::Rect,
    style::Color,
    widgets::canvas::{Canvas, Line as CanvasLine, Points},
    Frame,
};

use crate::palette::{magnitude_to_color_themed, ColorDepth};
use crate::state::SpectrumStyle;

use super::scale::dim;
use super::view::SpectrumView;

/// The dB window and the per-dot-row colour bands derived from it.
///
/// Colour depends **only** on vertical position, never on the fluctuating
/// per-bin dB, so the trace can never flicker in colour frame to frame. One
/// band per braille dot-row gives a gap-free fill.
pub(super) struct Vertical {
    pub min: f32,
    pub max: f32,
    pub span: f32,
    steps: usize,
    band_y: Vec<f32>,
    /// The soft glowing body.
    band_dim: Vec<Color>,
    /// The crisp full-brightness top edge.
    band_bright: Vec<Color>,
}

impl Vertical {
    pub fn new(min: f32, max: f32, canvas_height: u16, theme: &crate::Theme) -> Self {
        let span = (max - min).max(1e-3);
        let steps = (canvas_height as usize * 4).clamp(1, 512);
        let depth = ColorDepth::detect();
        let frac_of = |s: usize| {
            if steps > 1 {
                s as f32 / (steps - 1) as f32
            } else {
                0.0
            }
        };
        let height_color =
            |s: usize| magnitude_to_color_themed(min + frac_of(s) * span, min, max, depth, theme);
        Self {
            min,
            max,
            span,
            steps,
            band_y: (0..steps).map(|s| min + frac_of(s) * span).collect(),
            band_dim: (0..steps).map(|s| dim(height_color(s), 0.45)).collect(),
            band_bright: (0..steps).map(height_color).collect(),
        }
    }

    /// How far *down* the window a level sits, `0.0` at the top edge and `1.0`
    /// at the bottom. The row-from-level conversion every annotation needs, so
    /// a label lands on the same row as the line it names.
    pub fn frac_down_to(&self, level: f32) -> f32 {
        ((self.max - level.clamp(self.min, self.max)) / self.span).clamp(0.0, 1.0)
    }
}

/// Which colour band a level falls in. The one place height becomes colour, so
/// the body, the live edge and the scatter dots can never disagree.
fn band_of(level: f32, min: f32, span: f32, steps: usize) -> usize {
    let frac = ((level - min) / span).clamp(0.0, 1.0);
    ((frac * (steps - 1) as f32) as usize).min(steps - 1)
}

/// One marker's vertical rules: the marker itself and its channel-bandwidth
/// edges, in canvas x. Any of the three may be off-screen.
pub(super) struct MarkerRules {
    pub x: Option<f64>,
    pub bw_lo: Option<f64>,
    pub bw_hi: Option<f64>,
}

/// The full-height vertical rules drawn over the trace.
pub(super) struct Rules {
    pub markers: Vec<MarkerRules>,
    /// Occupied-bandwidth band edges, `lab_signal` only.
    pub obw: (Option<f64>, Option<f64>),
    pub cursor: Option<f64>,
}

/// The lab "instrument mode" ghosts: a captured baseline trace and a set
/// reference level. Both are `None` outside a `lab_*` preset.
pub(super) struct LabGhosts {
    pub trace: Option<Arc<Vec<f32>>>,
    pub ref_dbfs: Option<f64>,
}

/// Colours the paint closure needs. Resolved up front because the closure
/// cannot borrow the theme.
struct Palette {
    grid: Color,
    hold: Color,
    peak_hold: Color,
    noise_floor: Color,
    cal: Color,
    ref_line: Color,
    marker: Color,
    channel_bw: Color,
    obw: Color,
    cursor: Color,
}

impl Palette {
    fn new(theme: &crate::Theme) -> Self {
        Self {
            // A faint reference grid, like a spectrum-analyser screen.
            grid: theme.stale,
            // A soft dimmed phosphor, so the frozen snapshot reads as "past"
            // without competing with the live trace.
            hold: dim(theme.border_focused, 0.50),
            peak_hold: theme.peak_hold,
            noise_floor: theme.noise_floor,
            cal: theme.observer,
            ref_line: theme.value_hi,
            marker: theme.status_warn,
            channel_bw: theme.border_accent,
            obw: dim(theme.border_accent, 0.55),
            cursor: theme.value_hi,
        }
    }
}

/// Everything drawn over the trace, gathered so the orchestrator hands the
/// canvas one description of what goes on it rather than a parameter list.
pub(super) struct Layers {
    pub rules: Rules,
    pub ghosts: LabGhosts,
    pub style: SpectrumStyle,
    pub noise_floor: f32,
}

/// Paint the trace and everything drawn on the canvas itself.
pub(super) fn draw(
    f: &mut Frame,
    area: Rect,
    view: &SpectrumView,
    vert: &Vertical,
    layers: Layers,
    theme: &crate::Theme,
) {
    let Layers {
        rules,
        ghosts,
        style,
        noise_floor,
    } = layers;
    let pal = Palette::new(theme);
    let n = view.n();
    let (y_min, y_max) = (vert.min as f64, vert.max as f64);

    // The closure takes ownership, so everything it needs is moved in.
    let bins = Arc::clone(&view.bins);
    let peaks = Arc::clone(&view.peaks);
    let held = view.held.clone();
    let (band_y, band_dim, band_bright) = (
        vert.band_y.clone(),
        vert.band_dim.clone(),
        vert.band_bright.clone(),
    );
    let (steps, v_min, v_max, span) = (vert.steps, vert.min, vert.max, vert.span);

    f.render_widget(
        Canvas::default()
            .x_bounds([0.0, (n - 1.0).max(0.0)])
            .y_bounds([y_min, y_max])
            .paint(move |ctx| {
                let bright_at = |level: f32| band_bright[band_of(level, v_min, span, steps)];
                let rule = |ctx: &mut ratatui::widgets::canvas::Context, x: f64, color: Color| {
                    ctx.draw(&CanvasLine {
                        x1: x,
                        y1: y_min,
                        x2: x,
                        y2: y_max,
                        color,
                    });
                };
                let level_line =
                    |ctx: &mut ratatui::widgets::canvas::Context, y: f64, color: Color| {
                        ctx.draw(&CanvasLine {
                            x1: 0.0,
                            y1: y,
                            x2: n - 1.0,
                            y2: y,
                            color,
                        });
                    };
                let series =
                    |ctx: &mut ratatui::widgets::canvas::Context, v: &[f32], color: Color| {
                        for i in 1..v.len() {
                            ctx.draw(&CanvasLine {
                                x1: (i - 1) as f64,
                                y1: v[i - 1].clamp(v_min, v_max) as f64,
                                x2: i as f64,
                                y2: v[i].clamp(v_min, v_max) as f64,
                                color,
                            });
                        }
                    };

                // 0. Graticule - the dB and frequency reference grid, drawn first
                //    so only the parts above the signal show through.
                for i in 0..=4 {
                    level_line(ctx, y_min + (y_max - y_min) * (i as f64 / 4.0), pal.grid);
                    rule(ctx, (n - 1.0).max(0.0) * (i as f64 / 4.0), pal.grid);
                }
                // 1. Hold ghost - the entire frozen spectrum as a soft outline.
                if let Some(ref h) = held {
                    series(ctx, h, pal.hold);
                }
                // 2. Filled body - solid horizontal runs per band, continuous so
                //    no isolated dots blink on and off as bins jitter. Braille
                //    dims it into a glow under its crisp edge; Fill keeps it at
                //    full brightness as a heavy body; Scatter has none.
                if style != SpectrumStyle::Scatter {
                    for s in 0..steps {
                        let yb = band_y[s];
                        let color = if style == SpectrumStyle::Fill {
                            band_bright[s]
                        } else {
                            band_dim[s]
                        };
                        let mut i = 0usize;
                        while i < bins.len() {
                            if bins[i] >= yb {
                                let start = i;
                                while i < bins.len() && bins[i] >= yb {
                                    i += 1;
                                }
                                ctx.draw(&CanvasLine {
                                    x1: start as f64,
                                    y1: yb as f64,
                                    x2: (i - 1) as f64,
                                    y2: yb as f64,
                                    color,
                                });
                            } else {
                                i += 1;
                            }
                        }
                    }
                }
                // 3. Live edge - the crisp trace connecting bin tops, coloured by
                //    height. Only Braille draws it: Fill's bright body is its own
                //    edge, Scatter has no line.
                if style == SpectrumStyle::Braille {
                    for i in 1..bins.len() {
                        let (y0, y1) =
                            (bins[i - 1].clamp(v_min, v_max), bins[i].clamp(v_min, v_max));
                        ctx.draw(&CanvasLine {
                            x1: (i - 1) as f64,
                            y1: y0 as f64,
                            x2: i as f64,
                            y2: y1 as f64,
                            color: bright_at((y0 + y1) * 0.5),
                        });
                    }
                }
                // 3b. Scatter - an airy dot per bin at its top, no fill or line.
                if style == SpectrumStyle::Scatter {
                    for i in 0..bins.len() {
                        let yv = bins[i].clamp(v_min, v_max);
                        ctx.draw(&Points {
                            coords: &[(i as f64, yv as f64)],
                            color: bright_at(yv),
                        });
                    }
                }
                // 4. Peak hold - one connected gold line tracing the decaying max
                //    envelope. A line stays calm where scattered points blink.
                series(ctx, &peaks, pal.peak_hold);
                // 5. Noise floor reference line.
                level_line(ctx, noise_floor.clamp(v_min, v_max) as f64, pal.noise_floor);
                // 5b. CAL reference-trace ghost - the captured baseline, drawn only
                //     when it matches the current bin count, i.e. at the zoom it
                //     was captured at.
                if let Some(ref tr) = ghosts.trace {
                    if tr.len() == bins.len() {
                        series(ctx, tr, pal.cal);
                    }
                }
                // 5c. REF level - a horizontal line at the set dBFS.
                if let Some(ry) = ghosts.ref_dbfs {
                    level_line(ctx, ry, pal.ref_line);
                }
                // 6. Markers and their channel-bandwidth edges.
                for md in &rules.markers {
                    if let Some(cx) = md.x {
                        rule(ctx, cx, pal.marker);
                    }
                    if let Some(lo) = md.bw_lo {
                        rule(ctx, lo, pal.channel_bw);
                    }
                    if let Some(hi) = md.bw_hi {
                        rule(ctx, hi, pal.channel_bw);
                    }
                }
                // 6b. OBW band edges (lab_signal only).
                if let Some(x) = rules.obw.0 {
                    rule(ctx, x, pal.obw);
                }
                if let Some(x) = rules.obw.1 {
                    rule(ctx, x, pal.obw);
                }
                // 7. Tuning cursor - full height, always on top.
                if let Some(cx) = rules.cursor {
                    rule(ctx, cx, pal.cursor);
                }
            }),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn band_of_is_monotone_and_never_indexes_past_the_palette() {
        let t = crate::theme::Theme::sdr();
        let v = Vertical::new(-100.0, 0.0, 20, &t);
        let mut last = 0;
        for s in 0..v.steps {
            let got = band_of(v.band_y[s], v.min, v.span, v.steps);
            assert!(got < v.steps, "band {s} indexed past the palette");
            assert!(got >= last, "band {s} went backwards");
            // `band_y[s]` does not always map back to exactly `s`: the round trip
            // through f32 lands a hair under the boundary and truncates to `s-1`.
            // That is why the filled body indexes `band_bright[s]` directly rather
            // than going through here - one is a band's own colour, the other is
            // "the colour for this level", and they are allowed to differ by one
            // step without either being wrong.
            assert!(s - got <= 1, "band {s} drifted to {got}");
            last = got;
        }
    }

    #[test]
    fn colour_tracks_height_and_nothing_else() {
        let t = crate::theme::Theme::sdr();
        let v = Vertical::new(-100.0, 0.0, 20, &t);
        let at = |lv| v.band_bright[band_of(lv, v.min, v.span, v.steps)];
        // The same level always gets the same colour, whatever else is on screen:
        // this is what stops the trace flickering frame to frame.
        assert_eq!(at(-50.0), at(-50.0));
        assert_ne!(at(-100.0), at(0.0), "the gradient does vary with height");
        // Out-of-window levels clamp to the ends rather than indexing past them.
        assert_eq!(at(-500.0), at(-100.0));
        assert_eq!(at(500.0), at(0.0));
    }

    #[test]
    fn a_flat_window_still_produces_a_usable_gradient() {
        let t = crate::theme::Theme::sdr();
        // y_min == y_max would divide by zero without the span floor.
        let v = Vertical::new(-50.0, -50.0, 10, &t);
        assert!(v.span > 0.0);
        assert_eq!(band_of(-50.0, v.min, v.span, v.steps), 0);
        // A zero-height canvas still has one band, not none.
        let flat = Vertical::new(-100.0, 0.0, 0, &t);
        assert_eq!(flat.steps, 1);
        assert_eq!(band_of(-30.0, flat.min, flat.span, flat.steps), 0);
    }
}
