// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The band across a panel's width: which megahertz cells each column covers,
//! which column a cell is drawn in, and the ruler written under them.
//!
//! **One mapping for every panel that draws the band across.** The occupancy
//! profile and the coexistence heatmap become the two halves of one instrument
//! (net-ux-polish-plan Stop 3), and a shared ruler is only honest if a column
//! means the same megahertz in both. Each computing its own would be two
//! answers to one question that agree until a width where rounding differs.
//!
//! A column covers `cells[lo..hi]`, `lo = x * CELLS / width`: when the panel is
//! narrower than the band a column holds several cells, and when it is wider a
//! cell spans several columns.

use crate::signal::net::{band, occupancy};

/// The duty the band's full scale stands for: the first of these at or above
/// the busiest cell.
const DUTY_SCALES: [f64; 5] = [0.05, 0.1, 0.2, 0.5, 1.0];

/// The smallest step of [`DUTY_SCALES`] that holds `busiest`.
///
/// **A stated scale, not a fixed one.** At a fixed 100 % a room whose busiest
/// megahertz is 8 % busy drew the profile's bars one row tall under ten empty
/// ones and the heatmap in the ramp's darkest tenth, nearly one colour; at a
/// scale that fits the room both use the whole of their range, and the scale
/// is written on the panel so a tall bar or a hot colour is never read as a
/// saturated channel. Steps rather than the exact maximum, so it holds still
/// while the readings wander, and never below 5 %, so a quiet room's noise is
/// not drawn as a wall.
pub fn full_scale(busiest: f64) -> f64 {
    DUTY_SCALES
        .iter()
        .copied()
        .find(|s| busiest <= *s)
        .unwrap_or(1.0)
}

/// The band's duty scale: one for the profile and the heatmap under it,
/// since they are one instrument and one quantity has one scale (rule 5).
/// It holds the busiest cell now and the busiest moment the history keeps,
/// so scrubbing back or scrolling never shows a colour or a bar past it.
pub fn duty_scale(state: &crate::state::SdrMetrics) -> f64 {
    let band = &state.net.band;
    let now = band.cells.iter().filter(|c| c.observed()).map(|c| c.duty);
    let kept = band
        .history
        .iter()
        .flat_map(|col| col.iter())
        .filter(|d| **d >= 0.0)
        .map(|d| f64::from(*d));
    full_scale(now.chain(kept).fold(0.0, f64::max))
}

/// `10 %`, `0.5 %`: a duty as the scale is written.
pub fn percent(duty: f64) -> String {
    let p = duty * 100.0;
    if p >= 1.0 {
        format!("{p:.0} %")
    } else {
        format!("{p:.1} %")
    }
}

/// The cells column `x` of a `width`-column band covers. Never empty.
///
/// `widgets::canvas::band_range`, so the heatmap's `canvas::fold` into `width`
/// columns and this mapping are one function rather than two that agree.
pub fn cells_of(x: usize, width: usize) -> std::ops::Range<usize> {
    crate::ui::widgets::canvas::band_range(occupancy::CELLS, width.max(1), x)
}

/// The column cell `cell` is drawn in: the first whose cells include it.
pub fn column_of(cell: usize, width: usize) -> usize {
    (0..width.max(1))
        .find(|&x| cells_of(x, width).contains(&cell))
        .unwrap_or(width.saturating_sub(1))
}

/// The Wi-Fi channel numbers written under the columns they are centred on,
/// where they fit without treading on each other.
pub fn ruler(width: usize) -> String {
    let mut row = vec![b' '; width];
    for ch in 1..=13u8 {
        let Some(hz) = band::wifi_centre_hz(ch) else {
            continue;
        };
        let Some(cell) = occupancy::cell_of(hz as f64) else {
            continue;
        };
        let at = cell * width / occupancy::CELLS;
        let label = ch.to_string();
        // Centred on the channel, and only when it does not tread on the number
        // beside it: a ruler that overwrites its own labels is worse than one
        // with gaps.
        let start = at.saturating_sub(label.len() / 2);
        if start + label.len() > width {
            continue;
        }
        if row[start..start + label.len()].iter().all(|c| *c == b' ')
            && (start == 0 || row[start - 1] == b' ')
        {
            row[start..start + label.len()].copy_from_slice(label.as_bytes());
        }
    }
    String::from_utf8(row).unwrap_or_default()
}

/// `2400 MHz ... 2483 MHz`, the band's two edges at the ends of the row.
pub fn edges(width: usize) -> String {
    format!(
        "{:<pad$}",
        format!("{} MHz", band::LOW_HZ / 1_000_000),
        pad = width.saturating_sub(8)
    ) + &format!("{} MHz", band::HIGH_HZ / 1_000_000)
}

/// The frame of one half of a bonded NET instrument: the panel's own chrome,
/// engine tags and border rule (`chrome::frame`), drawn with only `borders`,
/// the nameplate on the top edge or, for the lower half whose top edge is the
/// shared ruler, on the bottom one. Returns the rect inside, or `None` when
/// the frame leaves nothing to draw in.
#[allow(clippy::too_many_arguments)]
pub fn bonded_frame(
    panel: &dyn crate::ui::panel::Panel,
    f: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    state: &crate::state::SdrMetrics,
    theme: &crate::Theme,
    focused: bool,
    borders: ratatui::widgets::Borders,
    plate_below: bool,
) -> Option<ratatui::layout::Rect> {
    use crate::ui::chrome::frame;
    use ratatui::widgets::block::{Position, Title};
    use ratatui::widgets::{Block, BorderType};
    let chrome = panel.chrome(state).with_engine_tags(state);
    let stale = chrome.staleness.resolve(state);
    let colour = frame::frame_color(&chrome, state, focused, theme);
    let plate =
        ratatui::text::Line::from(frame::title_spans(&chrome, panel.focus_key(), stale, theme));
    let title = Title::from(plate).position(if plate_below {
        Position::Bottom
    } else {
        Position::Top
    });
    let block = Block::default()
        .borders(borders)
        .border_type(BorderType::Rounded)
        .border_style(ratatui::style::Style::default().fg(colour))
        .title(title);
    let inner = block.inner(area);
    f.render_widget(block, area);
    (inner.width > 0 && inner.height > 0).then_some(inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One scale for the profile and the heatmap: it holds the busiest cell
    /// now and the busiest moment kept, so a burst a minute ago is not drawn
    /// past the top of either.
    #[test]
    fn the_duty_scale_holds_now_and_the_history() {
        let mut m = crate::state::SdrMetrics::fixture();
        m.net.band.cells = vec![crate::state::CellReading::default(); occupancy::CELLS];
        assert_eq!(duty_scale(&m), 0.05);
        m.net.band.cells[3].windows = 10;
        m.net.band.cells[3].duty = 0.07;
        assert_eq!(duty_scale(&m), 0.1);
        let mut then = vec![-1.0f32; occupancy::CELLS];
        then[40] = 0.3;
        m.net.band.history = vec![then].into();
        assert_eq!(duty_scale(&m), 0.5);
        assert_eq!(percent(0.1), "10 %");
        assert_eq!(percent(0.005), "0.5 %");
    }

    /// Every cell is drawn in a column that covers it, and every column
    /// covers at least one cell, at every width a panel can have: the
    /// mapping and its inverse agree.
    #[test]
    fn a_cell_lands_in_a_column_that_covers_it() {
        for width in 1..200 {
            for cell in 0..occupancy::CELLS {
                let x = column_of(cell, width);
                assert!(x < width, "{width}: cell {cell} at {x}");
                assert!(cells_of(x, width).contains(&cell), "{width}: {cell}");
            }
            for x in 0..width {
                assert!(!cells_of(x, width).is_empty(), "{width}: {x}");
            }
        }
    }

    /// The band's first and last cells sit at the two edges, whatever the
    /// width.
    #[test]
    fn the_edges_of_the_band_are_the_edges_of_the_row() {
        for width in [20, 83, 100, 160] {
            assert_eq!(column_of(0, width), 0);
            assert_eq!(column_of(occupancy::CELLS - 1, width), width - 1);
        }
    }
}
