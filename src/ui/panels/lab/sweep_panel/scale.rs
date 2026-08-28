//! The plot's coordinate systems: the dBFS window, the gutter width, and how a
//! height maps to a colour.
//!
//! Everything that converts between a level, a screen position and a colour lives
//! here, so the gutter labels, the canvas bounds and the trace's shading cannot
//! disagree about what the top of the plot means.

use ratatui::style::Color;

use crate::palette::{magnitude_to_color_themed, ColorDepth};

/// dBFS window for the vertical axis.
pub(super) const Y_MIN: f32 = -100.0;
pub(super) const Y_MAX: f32 = 0.0;
/// Width of the left dBFS-label gutter.
pub(super) const AXIS_W: u16 = 5;

/// Height of the window in dB, floored at 1 so nothing divides by zero if the
/// constants are ever brought together.
pub(super) fn span_db() -> f32 {
    (Y_MAX - Y_MIN).max(1.0)
}

/// Dim a truecolor toward black by factor `f` (256/16 pass through). Matches the
/// spectrum's filled-body shading so the sweep envelope reads the same way.
fn dim(c: Color, f: f32) -> Color {
    match c {
        Color::Rgb(r, g, b) => Color::Rgb(
            (r as f32 * f) as u8,
            (g as f32 * f) as u8,
            (b as f32 * f) as u8,
        ),
        other => other,
    }
}

/// How much the filled body is dimmed relative to its top edge.
const BODY_DIM: f32 = 0.45;

/// The height→colour gradient, resolved once per frame.
///
/// **Height-only, and that is the point.** Colouring by anything that varies
/// frame to frame would make a steady carrier shimmer; a level always gets the
/// same colour, so the plot is stable and a change in colour means a change in
/// signal.
pub(super) struct Gradient {
    /// The dBFS level each band step sits at, bottom to top.
    level: Vec<f32>,
    /// Dimmed, for the filled body.
    body: Vec<Color>,
    /// Full brightness, for the top edge.
    edge: Vec<Color>,
}

impl Gradient {
    /// Four steps per character row, capped: enough that the shading is smooth on
    /// a tall pane, bounded so a very tall one does not spend the frame drawing
    /// bands nobody can distinguish.
    pub(super) fn new(plot_h: usize, theme: &crate::Theme) -> Self {
        let steps = (plot_h * 4).clamp(1, 512);
        let depth = ColorDepth::detect();
        let level: Vec<f32> = (0..steps)
            .map(|s| {
                let f = if steps > 1 {
                    s as f32 / (steps - 1) as f32
                } else {
                    0.0
                };
                Y_MIN + f * span_db()
            })
            .collect();
        let edge: Vec<Color> = level
            .iter()
            .map(|&y| magnitude_to_color_themed(y, Y_MIN, Y_MAX, depth, theme))
            .collect();
        let body = edge.iter().map(|&c| dim(c, BODY_DIM)).collect();
        Self { level, body, edge }
    }

    pub(super) fn steps(&self) -> usize {
        self.level.len()
    }

    pub(super) fn level(&self, step: usize) -> f32 {
        self.level[step]
    }

    pub(super) fn body(&self, step: usize) -> Color {
        self.body[step]
    }

    /// The edge colour for a level, clamped into the window.
    pub(super) fn edge_at(&self, level_db: f32) -> Color {
        let frac = ((level_db - Y_MIN) / span_db()).clamp(0.0, 1.0);
        let last = self.steps() - 1;
        self.edge[((frac * last as f32) as usize).min(last)]
    }
}

/// Where to draw the cursor line, in canvas x.
///
/// Endpoint convention: `frac` 0 is the first sample and 1 is the last, matching
/// the canvas bounds `[0, n-1]`.
pub(super) fn cursor_x(frac: f64, n: usize) -> f64 {
    let last = (n.saturating_sub(1)) as f64;
    (frac * last).clamp(0.0, last)
}

/// Which projected bucket the cursor's readout comes from.
///
/// Bucket convention: the same `(frac * width) as usize` that `SweepFrame::project`
/// uses to fill the buckets, so the number beside the cursor is read out of the
/// bucket that frequency was binned into.
///
/// **These two are not the same mapping**, and they differ by up to half a bucket:
/// `cursor_x` spreads `frac` over `n-1` intervals, `cursor_bucket` over `n`
/// bins. At the plot's resolution (two buckets per character cell) that is a
/// quarter of a cell, so it has never been visible, and unifying them would move
/// the drawn line. Named here rather than left as two expressions in two files
/// that look like they should agree.
pub(super) fn cursor_bucket(frac: f64, n: usize) -> usize {
    ((frac * n as f64) as usize).min(n.saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Theme;

    #[test]
    fn the_window_runs_the_right_way_up() {
        // A `const` block: the window running the wrong way up would draw the
        // whole plot inverted, and that is worth failing the build for.
        const { assert!(Y_MIN < Y_MAX, "the dBFS window must run bottom to top") };
        assert!((span_db() - 100.0).abs() < 1e-6);
    }

    /// A level at the bottom of the window and one at the top must not get the
    /// same colour, or the gradient is doing nothing.
    #[test]
    fn the_gradient_distinguishes_the_ends_of_the_window() {
        let t = Theme::sdr();
        let g = Gradient::new(10, &t);
        assert!(g.steps() > 1);
        assert_ne!(g.edge_at(Y_MIN), g.edge_at(Y_MAX));
        assert_eq!(g.level(0), Y_MIN);
        assert!((g.level(g.steps() - 1) - Y_MAX).abs() < 1e-4);
    }

    /// Out-of-window levels are clamped rather than indexing past the end.
    #[test]
    fn a_level_outside_the_window_still_has_a_colour() {
        let t = Theme::sdr();
        let g = Gradient::new(10, &t);
        assert_eq!(g.edge_at(-500.0), g.edge_at(Y_MIN));
        assert_eq!(g.edge_at(50.0), g.edge_at(Y_MAX));
    }

    /// A one-row plot still produces a usable gradient rather than an empty one.
    #[test]
    fn a_single_row_plot_has_at_least_one_band() {
        let t = Theme::sdr();
        let g = Gradient::new(0, &t);
        assert_eq!(g.steps(), 1);
        // And `edge_at` must not divide by `steps - 1 == 0`.
        let _ = g.edge_at(-50.0);
    }

    /// The two cursor conventions never disagree by more than one bucket, which
    /// is what makes the half-bucket offset a documented quirk and not a bug.
    #[test]
    fn the_two_cursor_conventions_stay_within_one_bucket() {
        for n in [2usize, 3, 17, 64, 240] {
            for i in 0..=20 {
                let frac = i as f64 / 20.0;
                let x = cursor_x(frac, n);
                let bucket = cursor_bucket(frac, n) as f64;
                assert!(
                    (x - bucket).abs() <= 1.0,
                    "n={n} frac={frac}: line at {x}, readout from {bucket}"
                );
            }
        }
    }

    #[test]
    fn the_cursor_stays_inside_the_plot_at_both_ends() {
        for n in [1usize, 2, 100] {
            assert_eq!(cursor_x(0.0, n), 0.0);
            assert!(cursor_x(1.0, n) <= (n.saturating_sub(1)) as f64);
            assert!(cursor_bucket(1.0, n) < n.max(1));
            assert_eq!(cursor_bucket(0.0, n), 0);
        }
    }
}
