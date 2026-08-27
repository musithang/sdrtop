//! `sweep_panel` — the frequency-scanner display for the `lab_sweep` preset.
//!
//! Renders the latest completed `SweepFrame` as a vertical dBFS bar plot with the
//! sweep band on the x-axis, a band-plan label row underneath, and a status line.
//! The cursor and peak/mean toggle are driven from the panel's focus mode.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        canvas::{Canvas, Line as CanvasLine},
        Paragraph,
    },
    Frame,
};

use crate::palette::{magnitude_to_color_themed, ColorDepth};
use crate::state::SdrMetrics;
use crate::ui::panel::{Panel, PanelChrome};
use crate::ui::widgets::band_plan::{band_at, BAND_PLAN};

pub struct SweepPanel;

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

/// dBFS window for the vertical axis.
const Y_MIN: f32 = -100.0;
const Y_MAX: f32 = 0.0;
/// Width of the left dBFS-label gutter.
const AXIS_W: u16 = 5;

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
            "  {:.1}–{:.1} MHz · step {:.1} MHz · dwell {} ms · cycle #{}",
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
                        "  Scanning… first cycle in progress"
                    } else {
                        "  Waiting — open lab_sweep with RX available"
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
        // Left dBFS gutter + braille envelope canvas to its right.
        let gutter = Rect {
            x: plot.x,
            y: plot.y,
            width: AXIS_W,
            height: plot.height,
        };
        let canvas_area = Rect {
            x: plot.x + AXIS_W,
            y: plot.y,
            width: plot.width - AXIS_W,
            height: plot.height,
        };
        let plot_w = canvas_area.width as usize; // cells — used by the axis, band-plan and cursor maths
        let plot_h = canvas_area.height as usize;
        if plot_w == 0 || plot_h == 0 {
            return;
        }

        let span_db = (Y_MAX - Y_MIN).max(1.0);

        // dBFS gutter labels: top = Y_MAX, middle, bottom = Y_MIN.
        let mut gutter_lines: Vec<Line> = Vec::with_capacity(plot_h);
        for r in 0..plot_h {
            let s = if r == 0 {
                format!("{:>4} ", Y_MAX as i32)
            } else if r == plot_h - 1 {
                format!("{:>4} ", Y_MIN as i32)
            } else if plot_h >= 5 && r == plot_h / 2 {
                format!("{:>4} ", ((Y_MAX + Y_MIN) / 2.0) as i32)
            } else {
                "     ".to_string()
            };
            gutter_lines.push(Line::from(Span::styled(
                s,
                Style::default().fg(theme.label),
            )));
        }
        f.render_widget(Paragraph::new(gutter_lines), gutter);

        // Braille envelope: project at 2 dots per cell so a single-bin peak still
        // shows as a thin spike instead of being lost in a full-cell block.
        let n_cols = (plot_w * 2).max(2);
        let raw = frame.project(n_cols, sw.show_peak);
        // Envelope heights, clamped into the window (non-finite → floor → no fill).
        let body: Vec<f32> = raw
            .iter()
            .map(|&v| {
                if v.is_finite() {
                    v.clamp(Y_MIN, Y_MAX)
                } else {
                    Y_MIN
                }
            })
            .collect();
        let n = body.len();

        // Height-only colour (stable, never flickers): a dimmed gradient body with a
        // crisp top edge, same language as the spectrum.
        let depth = ColorDepth::detect();
        let v_steps = (plot_h * 4).clamp(1, 512);
        let band_y: Vec<f32> = (0..v_steps)
            .map(|s| {
                Y_MIN
                    + (if v_steps > 1 {
                        s as f32 / (v_steps - 1) as f32
                    } else {
                        0.0
                    }) * span_db
            })
            .collect();
        let band_dim: Vec<Color> = band_y
            .iter()
            .map(|&yb| {
                dim(
                    magnitude_to_color_themed(yb, Y_MIN, Y_MAX, depth, theme),
                    0.45,
                )
            })
            .collect();
        let band_bright: Vec<Color> = band_y
            .iter()
            .map(|&yb| magnitude_to_color_themed(yb, Y_MIN, Y_MAX, depth, theme))
            .collect();

        let cursor_x = sw
            .cursor_frac
            .map(|fr| (fr * (n - 1) as f64).clamp(0.0, (n - 1) as f64));
        let cursor_color = theme.value_hi;
        let x_max = (n as f64 - 1.0).max(0.0);

        f.render_widget(
            Canvas::default()
                .x_bounds([0.0, x_max])
                .y_bounds([Y_MIN as f64, Y_MAX as f64])
                .paint(move |ctx| {
                    // 1. Filled body — dimmed height-gradient runs per band step.
                    for s in 0..v_steps {
                        let yb = band_y[s];
                        let (ybf, color) = (yb as f64, band_dim[s]);
                        let mut i = 0usize;
                        while i < body.len() {
                            if body[i] >= yb {
                                let start = i;
                                while i < body.len() && body[i] >= yb {
                                    i += 1;
                                }
                                ctx.draw(&CanvasLine {
                                    x1: start as f64,
                                    y1: ybf,
                                    x2: (i - 1) as f64,
                                    y2: ybf,
                                    color,
                                });
                            } else {
                                i += 1;
                            }
                        }
                    }
                    // 2. Crisp top edge — connect column tops, coloured by height.
                    for i in 1..body.len() {
                        let (y0, y1) = (body[i - 1], body[i]);
                        let frac = (((y0 + y1) * 0.5 - Y_MIN) / span_db).clamp(0.0, 1.0);
                        let idx = ((frac * (v_steps - 1) as f32) as usize).min(v_steps - 1);
                        ctx.draw(&CanvasLine {
                            x1: (i - 1) as f64,
                            y1: y0 as f64,
                            x2: i as f64,
                            y2: y1 as f64,
                            color: band_bright[idx],
                        });
                    }
                    // 3. Cursor glow — a full-height column at the cursor position.
                    if let Some(cx) = cursor_x {
                        ctx.draw(&CanvasLine {
                            x1: cx,
                            y1: Y_MIN as f64,
                            x2: cx,
                            y2: Y_MAX as f64,
                            color: cursor_color,
                        });
                    }
                }),
            canvas_area,
        );

        // Cursor marker overlaid as a status (drawn in the status line below).
        let cursor_hz = sw.cursor_frac.map(|fr| frame.freq_at_fraction(fr));

        // X-axis: start / mid / stop MHz, left-padded past the gutter.
        let axis = format!(
            "{}{:<width$}{:^midw$}{:>endw$}",
            " ".repeat(AXIS_W as usize),
            format!("{:.0}", frame.start_hz as f64 / 1e6),
            format!("{:.0}", (frame.start_hz + frame.stop_hz) as f64 / 2e6),
            format!("{:.0} MHz", frame.stop_hz as f64 / 1e6),
            width = plot_w / 3,
            midw = plot_w / 3,
            endw = plot_w - 2 * (plot_w / 3),
        );
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                axis,
                Style::default().fg(theme.label),
            ))),
            rows[1],
        );

        // Band-plan label row: place each overlapping band's name at its centre x.
        f.render_widget(
            Paragraph::new(band_plan_line(
                frame.start_hz,
                frame.stop_hz,
                plot_w,
                AXIS_W as usize,
                theme,
            )),
            rows[2],
        );

        // Status line: cursor readout, else the cycle summary.
        let status = match cursor_hz {
            Some(hz) => {
                let frac = sw.cursor_frac.unwrap_or(0.0);
                let bucket = ((frac * n as f64) as usize).min(n.saturating_sub(1));
                let level = raw.get(bucket).copied().unwrap_or(f32::NEG_INFINITY);
                let level_str = if level.is_finite() {
                    format!("{:.1} dBFS", level)
                } else {
                    "—".to_string()
                };
                let band = band_at(hz)
                    .map(|b| format!("  [{}]", b))
                    .unwrap_or_default();
                Line::from(vec![
                    Span::styled(" Cursor ", Style::default().fg(theme.label)),
                    Span::styled(
                        format!("{:.3} MHz", hz as f64 / 1e6),
                        Style::default()
                            .fg(theme.value_hi)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("  "),
                    Span::styled(level_str, Style::default().fg(theme.value)),
                    Span::styled(band, Style::default().fg(theme.status_ok)),
                ])
            }
            None => Line::from(vec![
                Span::styled(" pos ", Style::default().fg(theme.label)),
                Span::styled(
                    format!("{}/{}", sw.positions_done, sw.positions_total),
                    Style::default().fg(theme.value),
                ),
                Span::styled("  ·  cycle ", Style::default().fg(theme.label)),
                Span::styled(
                    format!(
                        "#{} ({:.1}s)",
                        frame.cycle_count,
                        frame.cycle_duration_ms as f64 / 1000.0
                    ),
                    Style::default().fg(theme.value),
                ),
                Span::styled("  ·  ", Style::default().fg(theme.label)),
                Span::styled(
                    if sw.show_peak { "PEAK" } else { "MEAN" },
                    Style::default().fg(theme.value_hi),
                ),
                Span::styled(
                    format!("  ·  {:.0}s ago", frame.timestamp.elapsed().as_secs_f64()),
                    Style::default().fg(theme.stale),
                ),
                Span::styled(
                    "  ·  focus [G] for cursor",
                    Style::default().fg(theme.stale),
                ),
            ]),
        };
        f.render_widget(Paragraph::new(status), rows[3]);
    }
}

/// Build the band-plan label row: each known band overlapping `[start, stop]`
/// gets its name placed at its centre x (within the plot area, after the gutter).
fn band_plan_line(
    start_hz: u64,
    stop_hz: u64,
    plot_w: usize,
    gutter: usize,
    theme: &crate::Theme,
) -> Line<'static> {
    let mut row = vec![' '; gutter + plot_w];
    if stop_hz > start_hz {
        let span = (stop_hz - start_hz) as f64;
        for &(bs, be, name) in BAND_PLAN {
            if be <= start_hz || bs >= stop_hz {
                continue;
            }
            let centre = (bs.max(start_hz) + be.min(stop_hz)) / 2;
            let frac = (centre - start_hz) as f64 / span;
            let col = gutter + ((frac * plot_w as f64) as usize).min(plot_w.saturating_sub(1));
            // Place the name starting at `col`, not overwriting earlier labels.
            for (k, ch) in name.chars().enumerate() {
                let idx = col + k;
                if idx < row.len() && row[idx] == ' ' {
                    row[idx] = ch;
                }
            }
        }
    }
    Line::from(Span::styled(
        row.into_iter().collect::<String>(),
        Style::default().fg(theme.border_dim),
    ))
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
