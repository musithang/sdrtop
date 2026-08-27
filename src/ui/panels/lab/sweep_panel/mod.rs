//! `sweep_panel` — the frequency-scanner display for the `lab_sweep` preset.
//!
//! The latest completed `SweepFrame` as a braille envelope over the swept band,
//! with a dBFS gutter, a frequency axis, a band-plan row and a status line. The
//! cursor and the peak/mean toggle come from the panel's focus mode.
//!
//! Split the way `panels/core/spectrum/` is, because it is the same kind of
//! instrument:
//!
//! - [`scale`]: the coordinate systems — the dBFS window, the gutter width, the
//!   height→colour gradient, and the two cursor mappings.
//! - [`envelope`]: the sweep projected onto the plot's horizontal resolution.
//! - [`trace`]: the canvas layers.
//! - [`axes`]: the dBFS gutter and the frequency row.
//! - [`bands`]: the band-plan label row.
//! - [`status`]: the bottom row.
//!
//! This function carves the rows and calls each part once; the parts do not call
//! each other.

mod axes;
mod bands;
mod envelope;
mod scale;
mod status;
mod trace;

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::state::SdrMetrics;
use crate::ui::panel::{Panel, PanelChrome};

use envelope::Envelope;
use scale::{Gradient, AXIS_W};

pub struct SweepPanel;

impl Panel for SweepPanel {
    fn name(&self) -> &'static str {
        "sweep_panel"
    }
    fn min_size(&self) -> (u16, u16) {
        (40, 10)
    }
    fn focus_key(&self) -> Option<char> {
        Some('g')
    }
    fn focus_bindings(&self) -> &'static [(&'static str, &'static str)] {
        &[
            ("←/→", "Cursor"),
            ("S/E", "Start/End"),
            ("M", "Peak/Mean"),
            ("+/-", "Dwell"),
            ("C", "Snapshot to log"),
            ("Enter", "Tune here"),
        ]
    }

    fn chrome(&self, state: &SdrMetrics) -> PanelChrome {
        // `g` is not a letter of "Sweep", so the engine advertises it in
        // brackets. The scan parameters ride along as the suffix, rebuilt every
        // frame so the band, dwell and cycle number stay live.
        let sw = &state.sweep;
        let step_mhz = sw.config.effective_step_hz(state.radio.config_sample_rate) as f64 / 1e6;
        PanelChrome::new("Sweep").suffix(format!(
            "  {:.1}\u{2013}{:.1} MHz \u{00b7} step {:.1} MHz \u{00b7} dwell {} ms \u{00b7} cycle #{}",
            sw.config.start_hz as f64 / 1e6,
            sw.config.stop_hz as f64 / 1e6,
            step_mhz,
            sw.config.dwell_ms,
            sw.cycle_count,
        ))
    }

    fn render(
        &self,
        f: &mut Frame,
        inner: Rect,
        state: &SdrMetrics,
        theme: &crate::Theme,
        _focused: bool,
    ) {
        let sw = &state.sweep;
        if inner.width <= AXIS_W + 2 || inner.height < 4 {
            return;
        }

        // Rows: plot area, x-axis labels, band-plan labels, status.
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(3),    // plot
                Constraint::Length(1), // freq axis
                Constraint::Length(1), // band plan
                Constraint::Length(1), // status
            ])
            .split(inner);

        let Some(frame) = sw.current_frame.as_ref() else {
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    if state.radio.hw_streaming {
                        "  Scanning\u{2026} first cycle in progress"
                    } else {
                        "  Waiting \u{2014} open lab_sweep with RX available"
                    },
                    Style::default().fg(theme.stale),
                ))),
                rows[0],
            );
            return;
        };

        let plot = rows[0];
        if plot.width <= AXIS_W || plot.height == 0 {
            return;
        }
        let gutter = Rect {
            width: AXIS_W,
            ..plot
        };
        let canvas = Rect {
            x: plot.x + AXIS_W,
            width: plot.width - AXIS_W,
            ..plot
        };
        let (plot_w, plot_h) = (canvas.width as usize, canvas.height as usize);
        if plot_w == 0 || plot_h == 0 {
            return;
        }

        let env = Envelope::project(frame, plot_w, sw.show_peak);
        let cursor = sw
            .cursor_frac
            .map(|frac| (scale::cursor_x(frac, env.len()), theme.value_hi));

        axes::draw_gutter(f, gutter, plot_h, theme);
        trace::draw(
            f,
            canvas,
            env.body.clone(),
            Gradient::new(plot_h, theme),
            cursor,
        );
        axes::draw_frequency(f, rows[1], frame.start_hz, frame.stop_hz, plot_w, theme);
        f.render_widget(
            Paragraph::new(bands::line(frame.start_hz, frame.stop_hz, plot_w, theme)),
            rows[2],
        );
        f.render_widget(
            Paragraph::new(status::line(state, frame, &env, theme)),
            rows[3],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::fixture::draw;

    fn swept() -> SdrMetrics {
        SdrMetrics::fixture()
            .streaming()
            .with_sweep(88_000_000, 108_000_000)
    }

    /// The nameplate carries the scan parameters, rebuilt every frame, so a
    /// reader can tell what band the plot is of without leaving the panel.
    #[test]
    fn the_nameplate_states_the_band_being_scanned() {
        let out = draw(SweepPanel, 90, 14, &swept()).join("\n");
        assert!(out.contains("88.0"), "start MHz missing:\n{out}");
        assert!(out.contains("108.0"), "stop MHz missing:\n{out}");
        assert!(out.contains("cycle #3"), "cycle number missing:\n{out}");
    }

    /// Without a cursor the status line is the cycle summary; with one it becomes
    /// the readout. Both are the same row, so only one can be right at a time.
    #[test]
    fn the_status_line_switches_between_progress_and_cursor() {
        let progress = draw(SweepPanel, 90, 14, &swept()).join("\n");
        assert!(
            progress.contains("pos "),
            "no progress readout:\n{progress}"
        );
        assert!(progress.contains("64/64"), "positions missing:\n{progress}");

        let mut m = swept();
        m.sweep.cursor_frac = Some(0.5);
        let cursor = draw(SweepPanel, 90, 14, &m).join("\n");
        assert!(cursor.contains("Cursor"), "no cursor readout:\n{cursor}");
        assert!(
            cursor.contains("98."),
            "the cursor should read the middle of 88–108 MHz:\n{cursor}"
        );
        assert!(
            !cursor.contains("pos "),
            "both readouts drawn at once:\n{cursor}"
        );
    }

    /// The axis labels bracket the swept band rather than the tuned frequency —
    /// the radio is parked somewhere inside the band while the plot spans it.
    #[test]
    fn the_axis_spans_the_band_not_the_current_tuning() {
        let out = draw(SweepPanel, 90, 14, &swept()).join("\n");
        // Whole MHz on the axis, not the nameplate's one decimal: the axis has
        // three labels to fit across the plot and the band is 20 MHz wide.
        assert!(out.contains(" 88 "), "left edge label missing:\n{out}");
        assert!(out.contains("108 MHz"), "right edge label missing:\n{out}");
        // The radio is parked at 100 MHz; the plot is of the band, not of it.
        assert!(out.contains(" 98 "), "midpoint label missing:\n{out}");
    }

    /// The panel bails out on a rect it cannot draw a plot in, instead of
    /// panicking on a zero-height layout or drawing a smear. The frame is still
    /// there, because the engine draws that.
    #[test]
    fn a_rect_too_small_to_plot_draws_nothing_but_the_frame() {
        // Too small is `inner.width <= AXIS_W + 2 || inner.height < 4`, and the
        // frame costs two of each — so a 9-wide or 5-tall panel has no plot.
        for (w, h) in [(9u16, 14u16), (90, 5), (8, 4)] {
            let out = draw(SweepPanel, w, h, &swept());
            assert_eq!(
                out.len(),
                h as usize,
                "{w}x{h} produced the wrong row count"
            );
            let body: String = out[1..out.len().saturating_sub(1)].join("");
            assert!(
                !body.contains("pos "),
                "{w}x{h} should be too small for the status line, got:\n{}",
                out.join("\n")
            );
        }
    }

    /// A sweep with no completed frame yet must not render a plot of nothing.
    #[test]
    fn no_frame_yet_is_not_a_flat_line_at_the_floor() {
        let mut m = swept();
        m.sweep.current_frame = None;
        let out = draw(SweepPanel, 90, 14, &m).join("\n");
        assert!(
            !out.contains('\u{2588}'),
            "an empty sweep drew filled cells:\n{out}"
        );
    }
}
