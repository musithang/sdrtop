//! The braille canvas: filled body, top edge, cursor glow — in that order, so
//! each layer sits over the one before it.

use ratatui::{
    layout::Rect,
    style::Color,
    widgets::canvas::{Canvas, Line as CanvasLine},
    Frame,
};

use super::scale::{Gradient, Y_MAX, Y_MIN};

pub(super) fn draw(
    f: &mut Frame,
    area: Rect,
    body: Vec<f32>,
    gradient: Gradient,
    cursor: Option<(f64, Color)>,
) {
    let x_max = (body.len() as f64 - 1.0).max(0.0);
    f.render_widget(
        Canvas::default()
            .x_bounds([0.0, x_max])
            .y_bounds([Y_MIN as f64, Y_MAX as f64])
            .paint(move |ctx| {
                // 1. Filled body. One horizontal run per band step, so a plateau
                // costs one line rather than one per column.
                for step in 0..gradient.steps() {
                    let level = gradient.level(step);
                    let color = gradient.body(step);
                    let mut i = 0usize;
                    while i < body.len() {
                        if body[i] >= level {
                            let start = i;
                            while i < body.len() && body[i] >= level {
                                i += 1;
                            }
                            ctx.draw(&CanvasLine {
                                x1: start as f64,
                                y1: level as f64,
                                x2: (i - 1) as f64,
                                y2: level as f64,
                                color,
                            });
                        } else {
                            i += 1;
                        }
                    }
                }
                // 2. Crisp top edge, coloured by the mean height of the segment.
                for i in 1..body.len() {
                    let (y0, y1) = (body[i - 1], body[i]);
                    ctx.draw(&CanvasLine {
                        x1: (i - 1) as f64,
                        y1: y0 as f64,
                        x2: i as f64,
                        y2: y1 as f64,
                        color: gradient.edge_at((y0 + y1) * 0.5),
                    });
                }
                // 3. Cursor glow: a full-height column, drawn last so it is not
                // buried under the fill.
                if let Some((cx, color)) = cursor {
                    ctx.draw(&CanvasLine {
                        x1: cx,
                        y1: Y_MIN as f64,
                        x2: cx,
                        y2: Y_MAX as f64,
                        color,
                    });
                }
            }),
        area,
    );
}
