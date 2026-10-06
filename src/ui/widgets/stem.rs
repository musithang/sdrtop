// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The stem plot: discrete arrivals, and nothing between them.
//!
//! For a power delay profile. The app has
//! a line chart and a bar chart already and neither will do, **because of what
//! they imply rather than how they look**.
//!
//! A power delay profile is a set of *arrivals*: the signal reached the antenna
//! at this delay, and at that one, and nowhere in between. A line chart joins
//! its points, which draws energy arriving continuously across the gap. A filled
//! bar chart gives each sample a width, which draws a distribution where there
//! are discrete echoes. Both are readable and both say something that is not
//! true. A stem draws a mark where the arrival is and a stalk down to the
//! baseline, and claims nothing about the space between two stalks - which is
//! the whole reason it is the shape a PDP is published in.
//!
//! That matters here more than usual: these taps become **metres of excess path
//! length**, so a reader points at a wall. An echo the chart invented is a wall
//! that is not there.

use ratatui::{
    style::Style,
    text::{Line, Span},
};

/// One arrival.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Tap {
    /// Where it arrived, in the x unit the caller is plotting.
    pub at: f64,
    /// How strong it was, in the y unit the caller is plotting.
    pub level: f64,
}

/// The mark at the top of a stem: the arrival itself.
const HEAD: char = '\u{25cf}';
/// The stalk below it, which says only "this reaches down to the baseline".
const STALK: char = '\u{2502}';

/// Draw `taps` into `width` by `height` character cells.
///
/// **`None` for an empty series, and that is the type doing the arguing.** No
/// taps is not a set of taps at the floor: a chart that drew a flat line at the
/// bottom would be reporting a measurement of an echo-free room, which is a
/// result, when what happened is that nothing was measured. Returning an option
/// makes the caller say which.
///
/// `x` and `y` are the axis ranges in data units. Taps outside them are dropped
/// rather than clamped to an edge, because a tap pinned to the last column is
/// indistinguishable from one that really arrived there.
pub(crate) fn draw(
    taps: &[Tap],
    x: std::ops::Range<f64>,
    y: std::ops::Range<f64>,
    width: usize,
    height: usize,
    theme: &crate::Theme,
) -> Option<Vec<Line<'static>>> {
    if width == 0 || height == 0 {
        return None;
    }
    // The tallest arrival in each column. A column is a resolution limit, and
    // what a reader needs from it is the loudest thing that arrived inside it.
    let mut heads: Vec<Option<usize>> = vec![None; width];
    for tap in taps {
        let (Some(col), Some(row)) = (column(tap.at, &x, width), row_of(tap.level, &y, height))
        else {
            continue;
        };
        heads[col] = Some(heads[col].map_or(row, |best| best.min(row)));
    }
    // Nothing landed on the axes at all: not a room without echoes, a plot of
    // the wrong window. The caller is made to say which.
    heads.iter().find(|h| h.is_some())?;

    Some(
        (0..height)
            .map(|r| {
                let spans = heads
                    .iter()
                    .map(|head| {
                        let (glyph, colour) = match head {
                            Some(h) if *h == r => (HEAD, theme.value_hi),
                            Some(h) if *h < r => (STALK, theme.value),
                            _ => (' ', theme.label),
                        };
                        Span::styled(glyph.to_string(), Style::default().fg(colour))
                    })
                    .collect::<Vec<_>>();
                Line::from(spans)
            })
            .collect(),
    )
}

/// Which column a value falls in, or `None` outside the axis.
pub(crate) fn column(value: f64, x: &std::ops::Range<f64>, width: usize) -> Option<usize> {
    if width == 0 || !value.is_finite() || !x.contains(&value) {
        return None;
    }
    let span = x.end - x.start;
    if span.is_nan() || span <= 0.0 {
        return None;
    }
    let at = ((value - x.start) / span * width as f64) as usize;
    Some(at.min(width - 1))
}

/// Which row a level falls on, counted from the top, or `None` off the axis.
pub(crate) fn row_of(level: f64, y: &std::ops::Range<f64>, height: usize) -> Option<usize> {
    if height == 0 || !level.is_finite() || !y.contains(&level) {
        return None;
    }
    let span = y.end - y.start;
    if span.is_nan() || span <= 0.0 {
        return None;
    }
    // Counted from the top, because a terminal draws downwards and the loudest
    // arrival belongs at the top of the plot.
    let from_bottom = ((level - y.start) / span * height as f64) as usize;
    Some(height - 1 - from_bottom.min(height - 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> crate::Theme {
        crate::Theme::sdr()
    }

    fn text(lines: &[Line<'static>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    /// **The one the plan names.** No taps is not a flat line at zero.
    #[test]
    fn an_empty_series_is_nothing_rather_than_a_floor() {
        assert!(draw(&[], 0.0..500.0, -60.0..0.0, 40, 10, &theme()).is_none());
        // And a series whose every tap is off the axis is equally nothing: it is
        // not a room with no echoes, it is a plot of the wrong window.
        let off = [Tap {
            at: 900.0,
            level: -10.0,
        }];
        assert!(draw(&off, 0.0..500.0, -60.0..0.0, 40, 10, &theme()).is_none());
    }

    /// A tap lands in the column its delay says, at three sizes.
    #[test]
    fn a_tap_lands_where_its_delay_puts_it() {
        let x = 0.0..500.0;
        for width in [20usize, 40, 79] {
            assert_eq!(column(0.0, &x, width), Some(0), "the start of the axis");
            assert_eq!(column(499.9, &x, width), Some(width - 1), "and the end");
            assert_eq!(column(250.0, &x, width), Some(width / 2));
            // Outside is dropped, not pinned to an edge: a tap on the last
            // column must mean it arrived there.
            assert_eq!(column(-1.0, &x, width), None);
            assert_eq!(column(500.0, &x, width), None);
            assert_eq!(column(f64::NAN, &x, width), None);
        }
        assert_eq!(column(1.0, &x, 0), None, "no columns, no answer");
    }

    /// A stronger tap is higher up, and the top row is the top of the axis.
    #[test]
    fn a_level_lands_on_the_row_its_strength_puts_it() {
        let y = -60.0..0.0;
        for height in [4usize, 10, 31] {
            assert_eq!(row_of(-0.001, &y, height), Some(0), "loudest at the top");
            assert_eq!(
                row_of(-60.0, &y, height),
                Some(height - 1),
                "and quietest at the bottom"
            );
            let mid = row_of(-30.0, &y, height).unwrap();
            assert!(mid > 0 && mid < height - 1, "height {height}: {mid}");
            assert_eq!(row_of(0.0, &y, height), None, "the top edge is exclusive");
            assert_eq!(row_of(-61.0, &y, height), None);
        }
    }

    /// A stem is a head and a stalk down to the baseline, and nothing else in
    /// its column.
    #[test]
    fn a_stem_reaches_from_its_arrival_to_the_baseline() {
        let taps = [Tap {
            at: 100.0,
            level: -20.0,
        }];
        let lines = draw(&taps, 0.0..500.0, -60.0..0.0, 20, 6, &theme()).unwrap();
        let rows = text(&lines);
        assert_eq!(rows.len(), 6);
        for r in &rows {
            assert_eq!(r.chars().count(), 20, "{r:?}");
        }
        let col = column(100.0, &(0.0..500.0), 20).unwrap();
        let head = row_of(-20.0, &(-60.0..0.0), 6).unwrap();
        let at = |row: usize| rows[row].chars().nth(col).unwrap();
        assert_eq!(at(head), HEAD, "the arrival");
        for r in head + 1..6 {
            assert_eq!(at(r), STALK, "the stalk at row {r}");
        }
        for r in 0..head {
            assert_eq!(at(r), ' ', "nothing above the arrival at row {r}");
        }
        // And the columns either side are empty: a stem claims nothing about
        // the space between two arrivals.
        for r in &rows {
            assert_eq!(r.chars().nth(col - 1).unwrap(), ' ');
            assert_eq!(r.chars().nth(col + 1).unwrap(), ' ');
        }
    }

    /// Two arrivals in one column: the stronger is drawn, because a column is a
    /// resolution limit and the loudest thing in it is what a reader needs.
    #[test]
    fn two_taps_in_one_column_show_the_stronger() {
        let x = 0.0..500.0;
        let taps = [
            Tap {
                at: 10.0,
                level: -40.0,
            },
            Tap {
                at: 11.0,
                level: -10.0,
            },
        ];
        let width = 20;
        assert_eq!(column(10.0, &x, width), column(11.0, &x, width));
        let lines = draw(&taps, x.clone(), -60.0..0.0, width, 8, &theme()).unwrap();
        let rows = text(&lines);
        let col = column(10.0, &x, width).unwrap();
        let strong = row_of(-10.0, &(-60.0..0.0), 8).unwrap();
        assert_eq!(rows[strong].chars().nth(col).unwrap(), HEAD);
        // One head in the column, not two.
        let heads = (0..8)
            .filter(|r| rows[*r].chars().nth(col) == Some(HEAD))
            .count();
        assert_eq!(heads, 1);
    }

    /// Whatever the size, the plot fills it exactly and never overruns.
    #[test]
    fn it_renders_at_every_size_without_overrunning() {
        let taps: Vec<Tap> = (0..40)
            .map(|i| Tap {
                at: i as f64 * 12.0,
                level: -(i as f64),
            })
            .collect();
        for width in 1..60usize {
            for height in 1..20usize {
                let Some(lines) = draw(&taps, 0.0..500.0, -60.0..0.0, width, height, &theme())
                else {
                    panic!("{width}x{height}: a series with taps in range drew nothing");
                };
                assert_eq!(lines.len(), height);
                for r in text(&lines) {
                    assert_eq!(r.chars().count(), width, "{width}x{height}: {r:?}");
                }
            }
        }
    }
}
