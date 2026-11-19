// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The canvas itself: dBFS rings, the cloud, the fitted ellipse and the DC
//! crosshair, in a square.
//!
//! **Square, or the picture lies.** A terminal cell is about twice as tall as
//! it is wide, so the canvas is two columns for every row; drawn across the
//! panel's whole width, as it used to be, a perfect circle came out half as
//! wide again as it was tall, on the one panel that teaches an ellipse as
//! amplitude imbalance.
//!
//! **Zoomed to the cloud, with the scale written on it.** The outer ring is
//! the scale the RX poll chose for this cloud, the rings inside it 6 dB
//! apart, each labelled in dBFS; full scale is drawn when it is in view, and
//! so is the clip of a radio that clips before its converter. A fixed frame
//! of full scale put a -33 dBFS signal into a single dot.
//!
//! Everything drawn here is computed before the paint closure is built and
//! moved into it: the closure runs inside `Canvas`, so it owns plain data
//! rather than borrowing the metrics snapshot.

use std::f64::consts::PI;

use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::canvas::{Canvas, Context, Line as CanvasLine, Points},
    Frame,
};

use super::cloud::HEAT;
use super::CIRCLE_SEGS;

/// The rings, outermost first, 6 dB apart.
const RINGS: usize = 3;

/// What the canvas draws besides the cloud.
pub(super) struct Face {
    /// The outer ring, dBFS: the canvas spans its radius each way.
    pub outer_db: f64,
    /// Where this radio clips before its converter, dBFS, when it does.
    pub clip_db: Option<f64>,
    pub ellipse: Option<(f64, f64, f64, f64, f64)>,
    /// The measured I/Q offset, as a fraction of full scale.
    pub dc: (f64, f64),
}

/// The radius of a level in dBFS, as a fraction of full scale.
pub(super) fn radius(dbfs: f64) -> f64 {
    10f64.powf(dbfs / 20.0)
}

/// Draw the scope face and the cloud on it, into a square `area`. `layers`
/// is the density-bucketed cloud from
/// [`cloud::density_layers`](super::cloud::density_layers) over the same
/// extent.
pub(super) fn draw(
    f: &mut Frame,
    area: Rect,
    layers: Vec<Vec<(f64, f64)>>,
    face: &Face,
    theme: &crate::Theme,
) {
    let r = radius(face.outer_db);
    let ring_col = theme.border_dim;
    let crit = theme.status_crit;
    let label_col = theme.label;
    let ellipse_col = theme.border_focused;
    let dc_col = theme.status_warn;
    let full_scale_in_view = face.outer_db >= 0.0;
    let rings: Vec<(f64, String)> = (0..RINGS)
        .map(|k| {
            let db = face.outer_db - 6.0 * k as f64;
            (radius(db), format!("{:.0}", db + 0.0))
        })
        .collect();
    // A clip inside the view; full scale is the outer ring itself.
    let clip = face.clip_db.filter(|&c| c < face.outer_db).map(radius);
    let (ellipse, (dc_i, dc_q)) = (face.ellipse, face.dc);

    f.render_widget(
        Canvas::default()
            .x_bounds([-r, r])
            .y_bounds([-r, r])
            .paint(move |ctx| {
                ctx.draw(&CanvasLine {
                    x1: -r,
                    y1: 0.0,
                    x2: r,
                    y2: 0.0,
                    color: ring_col,
                });
                ctx.draw(&CanvasLine {
                    x1: 0.0,
                    y1: -r,
                    x2: 0.0,
                    y2: r,
                    color: ring_col,
                });
                for (k, (rad, _)) in rings.iter().enumerate() {
                    // A hair inside, so the outer ring is not cut by the edge.
                    let col = if k == 0 && full_scale_in_view {
                        crit
                    } else {
                        ring_col
                    };
                    circle(ctx, rad * 0.999, col, k > 0);
                }
                if let Some(c) = clip {
                    circle(ctx, c, crit, true);
                }

                // Sparse (cool) layers first, the dense (hot) core on top.
                for (k, layer) in layers.iter().enumerate() {
                    ctx.draw(&Points {
                        coords: layer,
                        color: HEAT[k],
                    });
                }

                if let Some((cx, cy, a, b, th)) = ellipse {
                    let (ct, st) = (th.cos(), th.sin());
                    let mut prev: Option<(f64, f64)> = None;
                    for k in 0..=CIRCLE_SEGS {
                        let t = 2.0 * PI * k as f64 / CIRCLE_SEGS as f64;
                        let (ex, ey) = (a * t.cos(), b * t.sin());
                        let p = (cx + ex * ct - ey * st, cy + ex * st + ey * ct);
                        if let Some((px, py)) = prev {
                            ctx.draw(&CanvasLine {
                                x1: px,
                                y1: py,
                                x2: p.0,
                                y2: p.1,
                                color: ellipse_col,
                            });
                        }
                        prev = Some(p);
                    }
                }

                // The DC offset, crosshair arms in proportion to the view.
                let arm = r * 0.05;
                ctx.draw(&CanvasLine {
                    x1: dc_i - arm,
                    y1: dc_q,
                    x2: dc_i + arm,
                    y2: dc_q,
                    color: dc_col,
                });
                ctx.draw(&CanvasLine {
                    x1: dc_i,
                    y1: dc_q - arm,
                    x2: dc_i,
                    y2: dc_q + arm,
                    color: dc_col,
                });

                let text = |s: String| Line::from(Span::styled(s, Style::default().fg(label_col)));
                // Each ring's level where it crosses the diagonal, clear of
                // both axes.
                for (rad, db) in &rings {
                    ctx.print(rad * 0.72, rad * 0.72, text(db.clone()));
                }
                ctx.print(r * 0.92, -r * 0.08, text("I".to_string()));
                ctx.print(r * 0.03, r * 0.95, text("Q".to_string()));
            }),
        area,
    );
}

/// A circle about the origin, dashed where asked.
fn circle(ctx: &mut Context, rad: f64, color: Color, dashed: bool) {
    let n = 2 * CIRCLE_SEGS;
    for k in (0..n).filter(|k| !dashed || k % 2 == 0) {
        let a0 = 2.0 * PI * k as f64 / n as f64;
        let a1 = 2.0 * PI * (k + 1) as f64 / n as f64;
        ctx.draw(&CanvasLine {
            x1: rad * a0.cos(),
            y1: rad * a0.sin(),
            x2: rad * a1.cos(),
            y2: rad * a1.sin(),
            color,
        });
    }
}
