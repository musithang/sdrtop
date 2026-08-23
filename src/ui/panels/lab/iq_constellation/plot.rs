//! The canvas itself: reference frame, cloud, fitted ellipse, DC crosshair.
//!
//! Everything drawn here is computed before the paint closure is built and moved
//! into it — the closure runs per resize inside `Canvas`, so it must own plain
//! data rather than borrow the metrics snapshot.

use std::f64::consts::PI;

use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::canvas::{Canvas, Line as CanvasLine, Points},
    Frame,
};

use super::cloud::HEAT;
use super::{BOUND, CIRCLE_SEGS};

/// Draw the scope face and the cloud on it. `layers` is the density-bucketed
/// cloud from [`cloud::density_layers`](super::cloud::density_layers), `ellipse`
/// the fit, `dc` the measured I/Q offset the crosshair marks.
pub(super) fn draw(
    f: &mut Frame, inner: Rect,
    layers: Vec<Vec<(f64, f64)>>,
    ellipse: Option<(f64, f64, f64, f64, f64)>,
    dc: (f64, f64),
    theme: &crate::Theme,
) {
    let (dc_i, dc_q) = dc;
    let axis_color    = theme.border_dim;
    let circle_color  = theme.border_dim;
    let ref_color     = theme.border_dim;
    let ellipse_color = theme.border_focused;
    let dc_color      = theme.status_warn;
    let label_color   = theme.label;

    f.render_widget(
        Canvas::default()
            .x_bounds([-BOUND, BOUND])
            .y_bounds([-BOUND, BOUND])
            .paint(move |ctx| {
                // I-axis (horizontal) + Q-axis (vertical)
                ctx.draw(&CanvasLine { x1: -BOUND, y1: 0.0, x2: BOUND, y2: 0.0, color: axis_color });
                ctx.draw(&CanvasLine { x1: 0.0, y1: -BOUND, x2: 0.0, y2: BOUND, color: axis_color });

                // Faint ±0.5 reference ring (inner scale).
                for k in 0..CIRCLE_SEGS {
                    let a0 = 2.0 * PI * k as f64 / CIRCLE_SEGS as f64;
                    let a1 = 2.0 * PI * (k + 1) as f64 / CIRCLE_SEGS as f64;
                    ctx.draw(&CanvasLine {
                        x1: 0.5 * a0.cos(), y1: 0.5 * a0.sin(),
                        x2: 0.5 * a1.cos(), y2: 0.5 * a1.sin(),
                        color: ref_color,
                    });
                }
                // Unit (1.0) reference circle.
                for k in 0..CIRCLE_SEGS {
                    let a0 = 2.0 * PI * k as f64 / CIRCLE_SEGS as f64;
                    let a1 = 2.0 * PI * (k + 1) as f64 / CIRCLE_SEGS as f64;
                    ctx.draw(&CanvasLine {
                        x1: a0.cos(), y1: a0.sin(),
                        x2: a1.cos(), y2: a1.sin(),
                        color: circle_color,
                    });
                }

                // Constellation cloud — sparse (cool) layers first, dense (hot)
                // core on top, for a phosphor-persistence look.
                for (k, layer) in layers.iter().enumerate() {
                    ctx.draw(&Points { coords: layer, color: HEAT[k] });
                }

                // Fitted imbalance ellipse: axis ratio = amplitude imbalance,
                // tilt = phase imbalance. Bright outline over the cloud.
                if let Some((cx, cy, a, b, th)) = ellipse {
                    let (ct, st) = (th.cos(), th.sin());
                    let mut prev: Option<(f64, f64)> = None;
                    for k in 0..=CIRCLE_SEGS {
                        let t = 2.0 * PI * k as f64 / CIRCLE_SEGS as f64;
                        let (ex, ey) = (a * t.cos(), b * t.sin());
                        let x = cx + ex * ct - ey * st;
                        let y = cy + ex * st + ey * ct;
                        if let Some((px, py)) = prev {
                            ctx.draw(&CanvasLine { x1: px, y1: py, x2: x, y2: y, color: ellipse_color });
                        }
                        prev = Some((x, y));
                    }
                }

                // DC offset crosshair (short arms centred on the measured offset).
                let arm = 0.07;
                ctx.draw(&CanvasLine { x1: dc_i - arm, y1: dc_q,       x2: dc_i + arm, y2: dc_q,       color: dc_color });
                ctx.draw(&CanvasLine { x1: dc_i,       y1: dc_q - arm, x2: dc_i,       y2: dc_q + arm, color: dc_color });

                // Reference labels (no live numbers — just orientation).
                let tick = |s: &str| Line::from(Span::styled(s.to_string(), Style::default().fg(label_color)));
                ctx.print(BOUND - 0.16, 0.10, tick("I"));
                ctx.print(0.07, BOUND - 0.08, tick("Q"));
                ctx.print(1.02, -0.14, tick("1.0"));
            }),
        inner,
    );
}
