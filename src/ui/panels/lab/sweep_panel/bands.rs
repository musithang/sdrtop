//! The band-plan label row: which allocations the sweep is crossing.
//!
//! Pure string building, so the placement can be checked without rendering. It
//! had no tests before the split, and it is the one row here that can silently
//! mislabel the plot: a name placed at the wrong column claims a signal belongs
//! to a service it does not.

use ratatui::{
    style::Style,
    text::{Line, Span},
};

use crate::ui::widgets::band_plan::BAND_PLAN;

/// Build the band-plan label row: each known band overlapping `[start, stop]`
/// gets its name placed at its centre x (within the plot area, after the gutter).
///
/// First name wins on a collision. Labels are written into a blank row and never
/// overwrite a character already placed, so two bands whose centres land close
/// together give a clipped second name rather than a mangled overlap of both.
pub(super) fn row(start_hz: u64, stop_hz: u64, plot_w: usize, gutter: usize) -> String {
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
            for (k, ch) in name.chars().enumerate() {
                let idx = col + k;
                if idx < row.len() && row[idx] == ' ' {
                    row[idx] = ch;
                }
            }
        }
    }
    row.into_iter().collect()
}

pub(super) fn line(
    start_hz: u64,
    stop_hz: u64,
    plot_w: usize,
    theme: &crate::Theme,
) -> Line<'static> {
    Line::from(Span::styled(
        row(start_hz, stop_hz, plot_w, super::scale::AXIS_W as usize),
        Style::default().fg(theme.border_dim),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const GUTTER: usize = 5;

    /// The row is exactly as wide as the gutter plus the plot, whatever it holds:
    /// a longer row would wrap and a shorter one would leave the plot's right
    /// edge unlabelled.
    #[test]
    fn the_row_is_always_the_width_of_the_plot() {
        for (w, (a, b)) in [
            (40usize, (88_000_000u64, 108_000_000u64)),
            (120, (100_000_000, 1_000_000_000)),
            (10, (3_000_000, 4_000_000)),
            (1, (88_000_000, 108_000_000)),
        ] {
            assert_eq!(row(a, b, w, GUTTER).chars().count(), GUTTER + w);
        }
    }

    /// A band the sweep does not cross must not be labelled.
    #[test]
    fn only_bands_the_sweep_crosses_are_named() {
        // 3–4 MHz crosses nothing in the plan.
        assert!(row(3_000_000, 4_000_000, 60, GUTTER).trim().is_empty());
        // 88–108 MHz is broadcast FM.
        assert!(row(88_000_000, 108_000_000, 60, GUTTER).contains("FM"));
    }

    /// Names land under the part of the plot they describe, not at the edges.
    #[test]
    fn a_name_sits_over_the_band_it_describes() {
        // A 0–200 MHz scan puts FM (88–108) just under half way across.
        let r = row(0, 200_000_000, 100, GUTTER);
        let at = r.find("FM").expect("FM should be labelled");
        let frac = (at - GUTTER) as f64 / 100.0;
        assert!(
            (0.40..0.60).contains(&frac),
            "FM landed at {frac:.2} of the plot: {r:?}"
        );
    }

    /// A degenerate band (stop at or below start) is a blank row rather than a
    /// division by zero.
    #[test]
    fn an_empty_span_is_a_blank_row() {
        assert!(row(100_000_000, 100_000_000, 40, GUTTER).trim().is_empty());
        assert!(row(200_000_000, 100_000_000, 40, GUTTER).trim().is_empty());
    }

    /// Nothing is ever written outside the row, however close to the right edge a
    /// band's centre falls.
    #[test]
    fn a_name_at_the_right_edge_is_clipped_not_wrapped() {
        for w in 1..40usize {
            let r = row(0, 2_000_000_000, w, GUTTER);
            assert_eq!(r.chars().count(), GUTTER + w, "w={w} overflowed");
        }
    }

    /// The first name placed keeps its columns, so overlapping labels degrade to
    /// a clipped second name instead of interleaved characters.
    #[test]
    fn overlapping_labels_do_not_interleave() {
        // A very narrow plot forces many bands onto few columns.
        let r = row(0, 2_000_000_000, 12, GUTTER);
        assert_eq!(r.chars().count(), GUTTER + 12);
        assert!(!r.trim().is_empty(), "expected some label to survive");
    }
}
