// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The waterfall: signal history scrolling down under the spectrum.
//!
//! Split the same way the spectrum is, by what each part draws:
//!
//! - [`cells`]: the half-block grid, and `Columns`, the bin-to-column mapping
//!   the frequency zoom works through.
//! - [`bond`]: what replaces the nameplate when the panel is fused to the
//!   spectrum above it, including the shared frequency ruler.
//! - [`overlays`]: band names and elapsed-time ticks drawn over the grid.
//! - [`axes`]: the dBFS colour gutter and the focus-mode readout row.
//!
//! This module is the orchestration: resolve the state, carve the areas, call
//! each part once.

mod axes;
mod bond;
mod cells;
mod overlays;

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::Line,
    widgets::Paragraph,
    Frame,
};

use crate::state::SdrMetrics;
use crate::ui::chrome;
use crate::ui::chrome::frame;
use crate::ui::panel::{Bond, Bonding, FrameTone, Panel, PanelChrome, Staleness, Tag};

use axes::DB_COL;
use bond::Window;
use cells::Columns;

// ── Waterfall row-stride steps ────────────────────────────────────────────────

pub const WF_STRIDES: &[usize] = &[1, 2, 4, 8, 16, 32, 64];

pub fn prev_wf_stride(current: usize) -> usize {
    WF_STRIDES
        .iter()
        .rev()
        .find(|&&s| s < current)
        .copied()
        .unwrap_or(1)
}

pub fn next_wf_stride(current: usize) -> usize {
    WF_STRIDES
        .iter()
        .find(|&&s| s > current)
        .copied()
        .unwrap_or(64)
}

// ── Waterfall frequency zoom levels ──────────────────────────────────────────

pub const WF_ZOOM_LEVELS: &[u32] = &[1, 2, 4, 8, 16, 32];

pub fn prev_wf_zoom(current: u32) -> u32 {
    WF_ZOOM_LEVELS
        .iter()
        .rev()
        .find(|&&z| z < current)
        .copied()
        .unwrap_or(1)
}

pub fn next_wf_zoom(current: u32) -> u32 {
    WF_ZOOM_LEVELS
        .iter()
        .find(|&&z| z > current)
        .copied()
        .unwrap_or(32)
}

/// The live state the nameplate and the bonded status cap both report.
pub(super) struct Status {
    pub paused: bool,
    pub stale: bool,
    /// Frames averaged into each history row.
    pub stride: usize,
    /// How far back through the history the view is scrolled, in character rows.
    pub scroll: usize,
}

pub struct WaterfallPanel;

impl WaterfallPanel {
    pub fn new() -> Self {
        Self
    }
}

impl Panel for WaterfallPanel {
    fn name(&self) -> &'static str {
        "waterfall"
    }
    fn supports_acquisition(&self, _acquisition: crate::hardware::AcquisitionKind) -> bool {
        true
    }
    fn min_size(&self) -> (u16, u16) {
        (40, 5)
    }
    fn focus_key(&self) -> Option<char> {
        Some('l')
    }
    fn focus_bindings(&self) -> &'static [(&'static str, &'static str)] {
        &[
            ("↑ ↓", "Zoom colour scale"),
            ("+ -", "Frequency zoom"),
            ("J K", "Scroll history"),
            ("[ ]", "Row stride (speed)"),
            ("M", "Place/remove cursor"),
            ("← →", "Move cursor"),
            ("W", "Pause / resume"),
            ("P", "Colour palette"),
        ]
    }

    fn chrome(&self, state: &SdrMetrics) -> PanelChrome {
        let wf = &state.waterfall;
        let buf = &wf.buffer;
        PanelChrome::deck("WATERFA_LL")
            .stale_when(Staleness::FftAge)
            .tone(FrameTone::Accent)
            // `Paused` suppresses `[STALE]` and cools the frame itself, so the
            // plate reads `[PAUSED]` rather than answering the same question
            // twice. That rule lives in `chrome::frame`, not here.
            .tag_if(buf.paused, Tag::Paused)
            .tag_if(buf.row_stride > 1, Tag::Stride(buf.row_stride))
            .tag_if(wf.scroll_offset > 0, Tag::Scroll(wf.scroll_offset))
    }

    /// The lower half of the spectrum-over-waterfall instrument.
    fn bonding(&self) -> Option<Bonding> {
        Some(Bonding {
            role: Bond::Above,
            partner: "spectrum",
        })
    }

    fn render_bonded(
        &self,
        f: &mut Frame,
        area: Rect,
        state: &SdrMetrics,
        theme: &crate::Theme,
        focused: bool,
        bond: Bond,
    ) {
        render(f, area, state, theme, focused, bond);
    }

    /// Engine-framed: `area` is the inner rect.
    fn render(
        &self,
        f: &mut Frame,
        area: Rect,
        state: &SdrMetrics,
        theme: &crate::Theme,
        focused: bool,
    ) {
        let border = frame::frame_color(&self.chrome(state), state, focused, theme);
        contents(f, area, state, theme, focused, Bond::None, area, border);
    }
}

/// The **bonded** case, reached through [`Panel::render_bonded`] when the engine
/// stacks this panel under its declared partner.
///
/// `Bond::Above` drops the nameplate - its identity and live tags move to the
/// shared ruler's end-cap tabs - and overlays that ruler on the top border so the
/// two panels read as one instrument.
fn render(
    f: &mut Frame,
    area: Rect,
    state: &SdrMetrics,
    theme: &crate::Theme,
    focused: bool,
    bond: Bond,
) {
    debug_assert_eq!(
        bond,
        Bond::Above,
        "unbonded waterfall goes through the registry"
    );
    let chrome = WaterfallPanel.chrome(state);
    let border = frame::frame_color(&chrome, state, focused, theme);

    // Bonded, the top border is the shared ruler, so the nameplate is suppressed
    // and drawn back on as a cap once the plot area is known.
    let block = chrome::deck_block(border).title(Line::from(""));
    let inner = block.inner(area);
    f.render_widget(block, area);
    chrome::corner_accents(f, area, border);
    contents(f, inner, state, theme, focused, bond, area, border);
}

/// The grid and its axes, drawn into whatever inner rect the caller carved.
///
/// `outer` is the whole panel, which the bonded seam needs to reach the top and
/// bottom borders it writes the ruler and the status cap onto.
#[allow(clippy::too_many_arguments)]
fn contents(
    f: &mut Frame,
    inner: Rect,
    state: &SdrMetrics,
    theme: &crate::Theme,
    focused: bool,
    bond: Bond,
    outer: Rect,
    border: Color,
) {
    let bonded = bond == Bond::Above;
    let wf = &state.waterfall;
    let buf = &wf.buffer;

    if buf.rows.is_empty() {
        f.render_widget(
            Paragraph::new("Waiting for RX\u{2026}")
                .alignment(Alignment::Center)
                .style(Style::default().fg(theme.label)),
            inner,
        );
        return;
    }

    let stale = Staleness::FftAge.resolve(state);
    // Clamp the reported scroll to what the buffer can actually give, so the
    // bonded status cap never promises history that is not there. The content
    // height is the panel minus its two borders and the focus indicator row.
    let approx_content_h = outer.height.saturating_sub(3) as usize;
    let status = Status {
        paused: buf.paused,
        stale,
        stride: buf.row_stride,
        scroll: wf
            .scroll_offset
            .min(buf.rows.len().saturating_sub(approx_content_h * 2) / 2),
    };

    // In focus mode one row goes to the readout, as on the spectrum.
    let (content, indicator) = if focused && inner.height > 2 {
        let split = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(inner);
        (split[0], Some(split[1]))
    } else {
        (inner, None)
    };

    // The grid starts after the dB gutter, at the same x as the spectrum's plot.
    let plot = Rect {
        x: content.x + DB_COL,
        y: content.y,
        width: content.width.saturating_sub(DB_COL),
        height: content.height,
    };
    if plot.width == 0 {
        return;
    }
    let cols = plot.width as usize;

    // Two data rows occupy each character row
    let data_rows = plot.height as usize * 2;
    let max_scroll = buf.rows.len().saturating_sub(data_rows) / 2;
    let skip_data = wf.scroll_offset.min(max_scroll) * 2;

    let columns_for_row = |row_bins| columns_for_row(wf, row_bins, cols);
    let columns = buf
        .rows
        .get(skip_data)
        .and_then(|(_, row)| columns_for_row(row.len()));
    let window = columns
        .as_ref()
        .map(|columns| Window {
            left_hz: columns.window.left_hz,
            bw: columns.window.span_hz,
        })
        .unwrap_or(Window {
            left_hz: 0.0,
            bw: 1.0,
        });

    if bonded {
        bond::seam(f, outer, plot, window, &status, border, theme);
    }

    let cursor_col = wf.cursor_freq.and_then(|cf| {
        let frac = (cf as f64 - window.left_hz) / window.bw;
        (0.0..=1.0)
            .contains(&frac)
            .then(|| ((frac * cols as f64) as usize).min(cols - 1))
    });

    cells::draw(
        f,
        plot,
        &buf.rows,
        columns_for_row,
        cursor_col,
        skip_data,
        wf.db_min,
        wf.db_max,
        wf.palette,
        theme,
    );

    // Bonded, the spectrum above already carries the band plan; twice is noise.
    if !bonded {
        overlays::band_plan(f, plot, window.left_hz, window.bw, theme);
    }
    overlays::time_axis(f, plot, &buf.rows, skip_data, theme);

    axes::db_legend(
        f,
        Rect {
            width: DB_COL,
            ..content
        },
        wf.db_min,
        wf.db_max,
        wf.palette,
        theme,
    );
    if let Some(area) = indicator {
        axes::indicator(
            f,
            area,
            state,
            &buf.rows,
            columns.as_ref(),
            skip_data,
            cursor_col,
            status.stride,
            theme,
        );
    }
}

fn columns_for_row(
    wf: &crate::state::WaterfallState,
    row_bins: usize,
    cols: usize,
) -> Option<Columns> {
    let (axis, window) = match wf.last_fft.as_ref() {
        Some(frame) => (
            frame.bin_axis,
            frame.window_for_bins(row_bins, wf.hz_zoom as usize)?,
        ),
        None => {
            let axis = crate::state::BinAxis::FftBins;
            (axis, axis.window(0, 1.0, row_bins, wf.hz_zoom as usize)?)
        }
    };
    Some(Columns::new(window, cols, axis))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn paused_history_keeps_its_own_bin_window_after_fft_size_changes() {
        let mut state = crate::state::SdrMetrics::fixture().with_carrier(0.0, 70.0);
        state.waterfall.hz_zoom = 4;
        state.waterfall.buffer.paused = true;
        let old_row = Arc::clone(&state.waterfall.buffer.rows.front().unwrap().1);
        let frame = state.waterfall.last_fft.as_mut().unwrap();
        frame.bins_dbfs = Arc::new(vec![-90.0; 1024]);
        assert!(!state.waterfall.buffer.push(&frame.bins_dbfs));

        let columns = columns_for_row(&state.waterfall, old_row.len(), 64).unwrap();
        assert_eq!(columns.window.first_bin, 96);
        assert_eq!(columns.window.bin_count, 64);
        assert_eq!(columns.range(0), (96, 97));
        assert_eq!(columns.range(63), (159, 160));
        assert!(columns_for_row(&state.waterfall, 0, 64).is_none());
    }

    #[test]
    fn mixed_size_history_rows_draw_their_own_centre_slices() {
        use ratatui::{backend::TestBackend, Terminal};
        let state = crate::state::SdrMetrics::fixture().with_carrier(0.0, 70.0);
        let mut wf = state.waterfall;
        wf.hz_zoom = 4;
        let mut top = vec![-120.0; 256];
        let mut bottom = vec![-120.0; 1024];
        top[96] = -20.0;
        bottom[384] = -20.0;
        wf.buffer.rows.clear();
        wf.buffer
            .rows
            .push_back((std::time::Instant::now(), Arc::new(top)));
        wf.buffer
            .rows
            .push_back((std::time::Instant::now(), Arc::new(bottom)));
        let mut terminal = Terminal::new(TestBackend::new(64, 1)).unwrap();
        let theme = crate::theme::Theme::sdr();
        terminal
            .draw(|f| {
                cells::draw(
                    f,
                    Rect::new(0, 0, 64, 1),
                    &wf.buffer.rows,
                    |n| columns_for_row(&wf, n, 64),
                    None,
                    0,
                    wf.db_min,
                    wf.db_max,
                    wf.palette,
                    &theme,
                )
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer.get(0, 0).fg, buffer.get(0, 0).bg);
        assert_ne!(buffer.get(0, 0).fg, buffer.get(1, 0).fg);
        assert_ne!(buffer.get(0, 0).bg, buffer.get(1, 0).bg);
    }

    #[test]
    fn measured_waterfall_window_keeps_an_odd_span_endpoint() {
        let mut state = crate::state::SdrMetrics::fixture().with_carrier(0.0, 70.0);
        let frame = state.waterfall.last_fft.as_mut().unwrap();
        frame.bin_axis = crate::state::BinAxis::MeasuredPoints;
        frame.axis_start_hz = 100_000.0;
        frame.sample_rate = 3.0;
        frame.bins_dbfs = Arc::new(vec![-90.0; 4]);

        let columns = columns_for_row(&state.waterfall, 4, 4).unwrap();
        assert_eq!(columns.window.left_hz, 100_000.0);
        assert_eq!(columns.window.left_hz + columns.window.span_hz, 100_003.0);
    }

    #[test]
    fn the_legend_uses_both_level_bounds() {
        let mut state = SdrMetrics::fixture().streaming().with_carrier(0.0, 40.0);
        for (min, max, labels) in [
            (-120.0, 0.0, ["+0", "-60", "-120"]),
            (-110.0, -10.0, ["-10", "-60", "-110"]),
        ] {
            state.waterfall.db_min = min;
            state.waterfall.db_max = max;
            let rows = crate::state::fixture::draw(WaterfallPanel, 100, 20, &state);
            let gutter: String = rows
                .iter()
                .skip(1)
                .take(18)
                .flat_map(|row| row.chars().skip(1).take(DB_COL as usize))
                .collect();
            for label in labels {
                assert!(gutter.contains(label), "{label} missing from {gutter}");
            }
        }
    }

    #[test]
    fn the_ladders_walk_and_stop_at_their_ends() {
        assert_eq!(next_wf_stride(1), 2);
        assert_eq!(prev_wf_stride(2), 1);
        assert_eq!(prev_wf_stride(1), 1, "already at the bottom");
        assert_eq!(next_wf_stride(64), 64, "already at the top");
        assert_eq!(next_wf_zoom(1), 2);
        assert_eq!(prev_wf_zoom(1), 1);
        assert_eq!(next_wf_zoom(32), 32);
        // Off-ladder values snap to the neighbour in that direction.
        assert_eq!(next_wf_stride(5), 8);
        assert_eq!(prev_wf_stride(5), 4);
    }
}
