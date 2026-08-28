//! The spectrum: the deck's primary instrument.
//!
//! Split by what each part draws rather than by when it runs, because the parts
//! are independent and the order they run in is the only thing that couples
//! them:
//!
//! - [`scale`]: hertz to canvas x to terminal column, the tuning-step ladder,
//!   and the frequency ruler the waterfall borrows when the two are bonded.
//! - [`view`]: which slice of the FFT frame is on screen. Everything downstream
//!   works in *its* coordinates.
//! - [`trace`]: the braille canvas - graticule, hold ghost, body, live edge,
//!   peak hold, reference lines, markers, cursor.
//! - [`labels`]: text drawn over the canvas, all of it collision-aware.
//! - [`axes`]: the frequency ruler, the tuning handle and the dBFS gutter.
//!
//! This module is the orchestration and nothing else: resolve the frame, carve
//! the rows, then call each part once in order.

mod axes;
mod labels;
mod scale;
mod trace;
mod view;

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::Line,
    widgets::{Borders, Paragraph},
    Frame,
};

use crate::state::SdrMetrics;
use crate::ui::chrome;
use crate::ui::chrome::frame;
use crate::ui::panel::{Bond, FrameTone, Panel, PanelChrome, Staleness, Tag};

pub(crate) use labels::detect_peaks;
pub use scale::{fmt_spectrum_step, freq_scale_spans, next_spectrum_step, prev_spectrum_step};

use scale::{freq_to_canvas_x, obw_bounds};
use trace::{LabGhosts, Layers, MarkerRules, Rules, Vertical};
use view::SpectrumView;

pub struct SpectrumPanel;

impl Panel for SpectrumPanel {
    fn name(&self) -> &'static str {
        "spectrum"
    }
    fn min_size(&self) -> (u16, u16) {
        (40, 10)
    }
    fn focus_key(&self) -> Option<char> {
        Some('e')
    }
    fn focus_bindings(&self) -> &'static [(&'static str, &'static str)] {
        &[
            ("← →", "Tune frequency"),
            ("[ ]", "Step size"),
            ("↑ ↓", "Zoom y-axis"),
            ("J K", "Cursor"),
            ("M", "Place/remove marker"),
            ("B", "Cycle channel BW on nearest marker"),
            ("H", "Hold/unhold frame"),
            ("D", "Draw style"),
        ]
    }

    fn chrome(&self, state: &SdrMetrics) -> PanelChrome {
        PanelChrome::deck("SP_ECTRUM")
            .stale_when(Staleness::FftAge)
            .tone(FrameTone::Accent)
            .tag_if(state.spectrum.hold.is_some(), Tag::Hold)
    }

    /// Engine-framed: `area` is the inner rect. The axes are tinted to match the
    /// border, so the colour is asked for by the same rule the frame was drawn
    /// with rather than re-derived here.
    fn render(
        &self,
        f: &mut Frame,
        area: Rect,
        state: &SdrMetrics,
        theme: &crate::Theme,
        focused: bool,
    ) {
        let border = frame::frame_color(&self.chrome(state), state, focused, theme);
        contents(f, area, state, theme, focused, Bond::None, border);
    }
}

/// Free render entry point for the **bonded** case, which the layout engine calls
/// directly instead of going through the registry.
///
/// `Bond::Below` drops the bottom border and the panel's own frequency-axis row -
/// the waterfall's top border becomes the shared ruler. Only the border *set* is
/// drawn here; the nameplate and the colour still come from the panel's own
/// [`PanelChrome`], so the bonded and standalone plates cannot drift apart.
pub fn render(
    f: &mut Frame,
    area: Rect,
    state: &SdrMetrics,
    theme: &crate::Theme,
    focused: bool,
    bond: Bond,
) {
    debug_assert_ne!(
        bond,
        Bond::None,
        "unbonded spectrum goes through the registry"
    );
    let chrome = SpectrumPanel.chrome(state);
    let stale = chrome.staleness.resolve(state);
    let border = frame::frame_color(&chrome, state, focused, theme);

    let block = chrome::deck_block_borders(border, bond_borders(bond)).title(Line::from(
        frame::title_spans(&chrome, SpectrumPanel.focus_key(), stale, theme),
    ));

    let inner = block.inner(area);
    f.render_widget(block, area);
    corner_accents(f, area, bond, border);
    contents(f, inner, state, theme, focused, bond, border);
}

/// The instrument itself, drawn into whatever inner rect the caller carved.
fn contents(
    f: &mut Frame,
    inner: Rect,
    state: &SdrMetrics,
    theme: &crate::Theme,
    focused: bool,
    bond: Bond,
    border: Color,
) {
    let Some(fft) = state.waterfall.last_fft.as_ref() else {
        f.render_widget(
            Paragraph::new("Waiting for RX\u{2026}")
                .alignment(Alignment::Center)
                .style(Style::default().fg(theme.label)),
            inner,
        );
        return;
    };

    // Shared frequency zoom: bonded below the waterfall, both plots narrow to
    // the same centre slice so the instrument zooms as one around the tuned
    // frequency. Standalone the spectrum shows the full span.
    let zoom = if bond == Bond::Below {
        state.waterfall.hz_zoom as usize
    } else {
        1
    };
    let Some(view) = SpectrumView::new(
        &fft.bins_dbfs,
        &fft.peak_hold,
        state.spectrum.hold.clone(),
        fft.center_freq_hz,
        fft.sample_rate,
        zoom,
    ) else {
        return;
    };

    draw_instrument(f, inner, state, theme, focused, bond, &view, fft, border);
}

/// Border set for the spectrum given its bond. `Bond::Below` drops the bottom
/// border (the waterfall's top border below becomes the shared ruler); otherwise
/// the panel is fully framed.
fn bond_borders(bond: Bond) -> Borders {
    if bond == Bond::Below {
        Borders::TOP | Borders::LEFT | Borders::RIGHT
    } else {
        Borders::ALL
    }
}

/// Bonded below, there is no bottom border to anchor `┗┛` against, so only the
/// top corners are reinforced.
fn corner_accents(f: &mut Frame, area: Rect, bond: Bond, border: Color) {
    if bond == Bond::Below {
        chrome::corner_accents_top(f, area, border);
    } else {
        chrome::corner_accents(f, area, border);
    }
}

/// The rows the instrument is carved into, given the panel's inner rect.
struct Rows {
    canvas: Rect,
    /// The frequency ruler, unless the bonded waterfall is carrying it.
    frequency: Option<Rect>,
    /// The tuning handle, in focus mode only.
    tuning: Option<Rect>,
    /// The dBFS gutter, the full height of the canvas.
    gutter: Rect,
}

impl Rows {
    /// Six columns of dBFS gutter, then the canvas with its optional rows below.
    fn split(inner: Rect, focused: bool, show_freq: bool) -> Self {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(6), Constraint::Min(1)])
            .split(inner);

        let mut constraints = vec![Constraint::Min(4)];
        if show_freq {
            constraints.push(Constraint::Length(1));
        }
        if focused {
            constraints.push(Constraint::Length(1));
        }

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints.clone())
            .split(cols[1]);
        let gutter_rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(cols[0]);

        Self {
            canvas: rows[0],
            frequency: show_freq.then(|| rows[1]),
            tuning: focused.then(|| rows[if show_freq { 2 } else { 1 }]),
            gutter: gutter_rows[0],
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_instrument(
    f: &mut Frame,
    inner: Rect,
    state: &SdrMetrics,
    theme: &crate::Theme,
    focused: bool,
    bond: Bond,
    view: &SpectrumView,
    fft: &crate::state::FftFrame,
    border: Color,
) {
    // Bonded below, the shared ruler covers the frequency scale, so that row is
    // reclaimed for the plot.
    let rows = Rows::split(inner, focused, bond != Bond::Below);
    let vert = Vertical::new(
        state.spectrum.y_min,
        state.spectrum.y_max,
        rows.canvas.height,
        theme,
    );
    let n = view.n();

    // Signal-characterization overlays belong to the lab_signal instrument only.
    let signal_overlay = state.ui.active_preset == "lab_signal";
    let obw = if signal_overlay && fft.occupied_bw_hz > 0 {
        let (lo, hi) = obw_bounds(fft.center_freq_hz, fft.occupied_bw_hz);
        (
            freq_to_canvas_x(lo, view.left_hz, view.bw, n),
            freq_to_canvas_x(hi, view.left_hz, view.bw, n),
        )
    } else {
        (None, None)
    };

    let cursor_x = state
        .spectrum
        .cursor_freq
        .and_then(|cf| freq_to_canvas_x(cf as f64, view.left_hz, view.bw, n));

    let markers = state
        .spectrum
        .markers
        .iter()
        .filter_map(|mk| {
            let at = |hz: f64| freq_to_canvas_x(hz, view.left_hz, view.bw, n);
            let (bw_lo, bw_hi) = match mk.channel_bw_hz {
                Some(ch) => {
                    let half = ch as f64 / 2.0;
                    (at(mk.freq_hz as f64 - half), at(mk.freq_hz as f64 + half))
                }
                None => (None, None),
            };
            let x = at(mk.freq_hz as f64);
            (x.is_some() || bw_lo.is_some() || bw_hi.is_some()).then_some(MarkerRules {
                x,
                bw_lo,
                bw_hi,
            })
        })
        .collect();

    // Lab "instrument mode" ghosts: a captured baseline and a set reference
    // level, drawn only in the measurement labs.
    let ghosts = if state.ui.is_lab_mode() {
        LabGhosts {
            trace: state.lab.ref_trace.clone(),
            ref_dbfs: state
                .lab
                .ref_dbfs
                .map(|r| r.clamp(vert.min, vert.max) as f64),
        }
    } else {
        LabGhosts {
            trace: None,
            ref_dbfs: None,
        }
    };

    trace::draw(
        f,
        rows.canvas,
        view,
        &vert,
        Layers {
            rules: Rules {
                markers,
                obw,
                cursor: cursor_x,
            },
            ghosts,
            style: state.spectrum.style,
            noise_floor: fft.noise_floor,
        },
        theme,
    );

    labels::band_plan(f, rows.canvas, view, theme);
    labels::peak_flags(f, rows.canvas, view, &vert, fft.noise_floor, theme);
    labels::marker_labels(f, rows.canvas, view, state, theme);
    if signal_overlay {
        labels::signal_annotations(
            f,
            rows.canvas,
            view,
            state,
            obw,
            fft.occupied_bw_hz,
            &vert,
            fft.noise_floor,
            theme,
        );
    }

    if let Some(area) = rows.frequency {
        axes::frequency(f, area, rows.canvas.width, view, border, theme);
    }
    if let Some(area) = rows.tuning {
        let cursor = state
            .spectrum
            .cursor_freq
            .and_then(|cf| view.level_at(cf).map(|pwr| (cf as f64 / 1_000_000.0, pwr)));
        axes::tuning(
            f,
            area,
            state.radio.frequency,
            state.spectrum.step_hz,
            cursor,
            theme,
        );
    }
    axes::db_gutter(f, rows.gutter, vert.min, vert.max, border, theme);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bond_below_drops_bottom_border() {
        // Bonded-below: no bottom border (the waterfall's top rule takes over);
        // every other edge stays.
        let b = bond_borders(Bond::Below);
        assert!(!b.contains(Borders::BOTTOM));
        assert!(b.contains(Borders::TOP | Borders::LEFT | Borders::RIGHT));
        // Standalone / above: fully framed.
        assert_eq!(bond_borders(Bond::None), Borders::ALL);
        assert_eq!(bond_borders(Bond::Above), Borders::ALL);
    }

    #[test]
    fn the_rows_shift_as_the_optional_ones_appear() {
        let inner = Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 20,
        };
        // Bonded below and unfocused: the canvas gets every row.
        let bare = Rows::split(inner, false, false);
        assert_eq!(bare.canvas.height, 20);
        assert!(bare.frequency.is_none() && bare.tuning.is_none());
        // The ruler takes one row off the plot.
        let ruled = Rows::split(inner, false, true);
        assert_eq!(ruled.canvas.height, 19);
        assert_eq!(ruled.frequency.unwrap().y, 19);
        // Focused without a ruler: the tuning handle takes that row instead.
        let tuned = Rows::split(inner, true, false);
        assert_eq!(tuned.canvas.height, 19);
        assert_eq!(tuned.tuning.unwrap().y, 19);
        // Both: ruler first, handle under it.
        let full = Rows::split(inner, true, true);
        assert_eq!(full.canvas.height, 18);
        assert_eq!(full.frequency.unwrap().y, 18);
        assert_eq!(full.tuning.unwrap().y, 19);
        // The gutter always matches the canvas, or the dB labels drift off the trace.
        assert_eq!(full.gutter.height, full.canvas.height);
        assert_eq!(full.gutter.width, 6);
    }
}
