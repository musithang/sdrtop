//! The two axes: the dBFS gutter down the left and the frequency row underneath.
//!
//! Both are built as plain strings first so the arithmetic can be tested without
//! a frame — an axis that mislabels the plot is worse than no axis, and it is the
//! kind of error that is invisible until someone trusts a reading off it.

use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use super::scale::{AXIS_W, Y_MAX, Y_MIN};

/// Below this many rows there is no room for a middle label without it crowding
/// the top or bottom one.
const MID_LABEL_MIN_H: usize = 5;

/// The gutter column, top to bottom: `Y_MAX`, optionally the midpoint, `Y_MIN`.
pub(super) fn gutter_labels(plot_h: usize) -> Vec<String> {
    (0..plot_h)
        .map(|r| {
            if r == 0 {
                format!("{:>4} ", Y_MAX as i32)
            } else if r == plot_h - 1 {
                format!("{:>4} ", Y_MIN as i32)
            } else if plot_h >= MID_LABEL_MIN_H && r == plot_h / 2 {
                format!("{:>4} ", ((Y_MAX + Y_MIN) / 2.0) as i32)
            } else {
                " ".repeat(AXIS_W as usize)
            }
        })
        .collect()
}

/// `start … mid … stop MHz`, left-padded past the gutter so the labels sit under
/// the plot rather than under the dBFS column.
pub(super) fn frequency_row(start_hz: u64, stop_hz: u64, plot_w: usize) -> String {
    let third = plot_w / 3;
    format!(
        "{}{:<width$}{:^midw$}{:>endw$}",
        " ".repeat(AXIS_W as usize),
        format!("{:.0}", start_hz as f64 / 1e6),
        format!("{:.0}", (start_hz + stop_hz) as f64 / 2e6),
        format!("{:.0} MHz", stop_hz as f64 / 1e6),
        width = third,
        midw = third,
        endw = plot_w - 2 * third,
    )
}

pub(super) fn draw_gutter(f: &mut Frame, area: Rect, plot_h: usize, theme: &crate::Theme) {
    let lines: Vec<Line> = gutter_labels(plot_h)
        .into_iter()
        .map(|s| Line::from(Span::styled(s, Style::default().fg(theme.label))))
        .collect();
    f.render_widget(Paragraph::new(lines), area);
}

pub(super) fn draw_frequency(
    f: &mut Frame,
    area: Rect,
    start_hz: u64,
    stop_hz: u64,
    plot_w: usize,
    theme: &crate::Theme,
) {
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            frequency_row(start_hz, stop_hz, plot_w),
            Style::default().fg(theme.label),
        ))),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_gutter_labels_the_ends_of_the_window() {
        let g = gutter_labels(9);
        assert_eq!(g.len(), 9);
        assert!(g[0].trim() == "0", "top should be Y_MAX: {:?}", g[0]);
        assert!(g[8].trim() == "-100", "bottom should be Y_MIN: {:?}", g[8]);
        assert!(
            g[4].trim() == "-50",
            "middle should be the midpoint: {:?}",
            g[4]
        );
        // Every row is the same width, or the plot beside it would step in and out.
        assert!(g.iter().all(|s| s.chars().count() == AXIS_W as usize));
    }

    /// A short plot drops the middle label rather than crowding three labels into
    /// four rows.
    #[test]
    fn a_short_gutter_keeps_only_the_two_ends() {
        let g = gutter_labels(4);
        assert_eq!(g.len(), 4);
        assert_eq!(g[0].trim(), "0");
        assert_eq!(g[3].trim(), "-100");
        assert!(g[1].trim().is_empty() && g[2].trim().is_empty());
    }

    #[test]
    fn a_one_row_gutter_does_not_panic() {
        let g = gutter_labels(1);
        assert_eq!(g.len(), 1);
        // Row 0 is both the first and the last; the first branch wins.
        assert_eq!(g[0].trim(), "0");
        assert!(gutter_labels(0).is_empty());
    }

    /// The frequency row spans the swept band and starts past the gutter, so the
    /// labels line up with the plot they describe.
    #[test]
    fn the_frequency_row_spans_the_band_past_the_gutter() {
        let row = frequency_row(88_000_000, 108_000_000, 60);
        assert!(
            row.starts_with(&" ".repeat(AXIS_W as usize)),
            "the row must clear the gutter: {row:?}"
        );
        assert!(row.contains("88"), "{row:?}");
        assert!(row.contains("98"), "midpoint missing: {row:?}");
        assert!(row.trim_end().ends_with("108 MHz"), "{row:?}");
        assert_eq!(
            row.chars().count(),
            AXIS_W as usize + 60,
            "the row must be exactly the gutter plus the plot"
        );
    }

    /// A very narrow plot still produces a row of the right width rather than one
    /// that overflows and wraps.
    #[test]
    fn a_narrow_frequency_row_still_fits_its_width() {
        for w in [1usize, 2, 3, 7, 12] {
            let row = frequency_row(88_000_000, 108_000_000, w);
            assert!(
                row.chars().count() >= AXIS_W as usize + w,
                "w={w}: {row:?} is shorter than the plot"
            );
        }
    }
}
