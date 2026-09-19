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
