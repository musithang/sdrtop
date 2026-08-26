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
use crate::ui::panel::{Bond, FrameTone, Panel, PanelChrome, Staleness, Tag};

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

/// How old the newest frame may get before the readings are called stale.
const STALE_MS: u128 = 500;

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

/// Free render entry point for the **bonded** case, which the layout engine calls
/// directly instead of going through the registry.
///
/// `Bond::Above` drops the nameplate — its identity and live tags move to the
/// shared ruler's end-cap tabs — and overlays that ruler on the top border so the
/// two panels read as one instrument.
pub fn render(
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

    let stale = wf
        .last_fft
        .as_ref()
        .map(|fr| fr.timestamp.elapsed().as_millis() > STALE_MS)
        .unwrap_or(false);
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

    // The frequency window, narrowed by the shared zoom around the tuned centre.
    let window = wf
        .last_fft
        .as_ref()
        .map(|fr| {
            let visible = fr.sample_rate / wf.hz_zoom as f64;
            Window {
                left_hz: fr.center_freq_hz as f64 - visible / 2.0,
                bw: visible,
            }
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

    // Two rows of history per character cell, so every scroll figure exists in
    // both units: `skip_chars` on screen, `skip_data` into the buffer.
    let data_rows = plot.height as usize * 2;
    let max_scroll = buf.rows.len().saturating_sub(data_rows) / 2;
    let skip_data = wf.scroll_offset.min(max_scroll) * 2;

    let row_bins = buf.rows.front().map(|(_, r)| r.len()).unwrap_or(1);
    let columns = Columns::new(row_bins, wf.hz_zoom, cols);

    cells::draw(
        f, plot, &buf.rows, &columns, cursor_col, skip_data, wf.db_min, wf.palette, theme,
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
        wf.palette,
        theme,
    );
    if let Some(area) = indicator {
        axes::indicator(
            f,
            area,
            state,
            &buf.rows,
            &columns,
            skip_data,
            cursor_col,
            status.stride,
            theme,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
