// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! A two-series time plot in braille: both ends of a piconet on one time
//! axis and one scale, so the bench can say at a glance whether the master
//! and the slave read alike and whether either is moving.

use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};

/// One end's points for a plot: (seconds before now, value), oldest first,
/// each the mean of a few consecutive packets of that end.
pub(super) struct Series {
    pub points: Vec<(f64, f64)>,
    pub ink: Color,
}

/// The fewest points an end needs to be drawn: fewer is a guess at a shape.
const MIN_POINTS: usize = 4;

/// A braille cell's dot bits, by the dot's column (0 left, 1 right) and its
/// row from the top (0 to 3).
const DOT: [[u8; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];

/// Two ends on one time axis and one scale: `rows` rows of braille, each
/// point in its end's colour, consecutive points joined, the master's drawn
/// last so where both fall in one cell its colour shows (the slave's point
/// still sets the scale). The scale's ends are labelled on the left with
/// `places` decimals; one axis row under it says how far back it reaches
/// (`-60 s ... now`). The span is the oldest point either end has, at most
/// `max_span_s`. Empty when neither end has four points, or the width
/// leaves no room to plot.
pub(super) fn time_plot(
    series: [&Series; 2],
    rows: usize,
    width: usize,
    places: usize,
    max_span_s: f64,
    theme: &crate::Theme,
) -> Vec<Line<'static>> {
    let drawn: Vec<&Series> = series
        .iter()
        .copied()
        .filter(|s| s.points.len() >= MIN_POINTS)
        .collect();
    if drawn.is_empty() || rows == 0 {
        return Vec::new();
    }
    let oldest = drawn
        .iter()
        .flat_map(|s| s.points.iter().map(|p| p.0))
        .fold(0.0f64, f64::max);
    let span = oldest.min(max_span_s).max(1e-3);
    let within = |s: &Series| -> Vec<(f64, f64)> {
        s.points
            .iter()
            .copied()
            .filter(|p| p.0 <= span && p.1.is_finite())
            .collect()
    };
    let (mut lo, mut hi) = drawn
        .iter()
        .flat_map(|s| within(s))
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), p| {
            (lo.min(p.1), hi.max(p.1))
        });
    if !lo.is_finite() {
        return Vec::new();
    }
    // A flat trace still needs a scale: one step of the printed places
    // either side.
    if hi - lo < 1e-12 {
        let step = 10f64.powi(-(places as i32));
        lo -= step;
        hi += step;
    }
    let (top, bottom) = (format!("{hi:.places$}"), format!("{lo:.places$}"));
    let lw = top.chars().count().max(bottom.chars().count());
    let plot = width.saturating_sub(lw + 2);
    if plot < MIN_POINTS {
        return Vec::new();
    }

    let (dots_x, dots_y) = (plot * 2, rows * 4);
    let mut bits = vec![vec![0u8; plot]; rows];
    let mut ink: Vec<Vec<Option<Color>>> = vec![vec![None; plot]; rows];
    let place = |p: (f64, f64)| -> (i64, i64) {
        let x = ((span - p.0) / span * (dots_x - 1) as f64).round() as i64;
        let y = ((p.1 - lo) / (hi - lo) * (dots_y - 1) as f64).round() as i64;
        (
            x.clamp(0, dots_x as i64 - 1),
            (dots_y as i64 - 1 - y).clamp(0, dots_y as i64 - 1),
        )
    };
    let mut dot = |x: i64, y: i64, c: Color| {
        let (cx, cy) = ((x / 2) as usize, (y / 4) as usize);
        bits[cy][cx] |= DOT[(x % 2) as usize][(y % 4) as usize];
        ink[cy][cx] = Some(c);
    };
    // The slave first, so the master's colour is the one left where both
    // fall in one cell.
    for s in drawn.iter().rev() {
        let pts: Vec<(i64, i64)> = within(s).into_iter().map(place).collect();
        for w in pts.windows(2) {
            // Bresenham between consecutive points, so the trace reads as a
            // line rather than a scatter.
            let ((mut x0, mut y0), (x1, y1)) = (w[0], w[1]);
            let (dx, dy) = ((x1 - x0).abs(), -(y1 - y0).abs());
            let (sx, sy) = (if x0 < x1 { 1 } else { -1 }, if y0 < y1 { 1 } else { -1 });
            let mut err = dx + dy;
            loop {
                dot(x0, y0, s.ink);
                if x0 == x1 && y0 == y1 {
                    break;
                }
                let e2 = 2 * err;
                if e2 >= dy {
                    err += dy;
                    x0 += sx;
                }
                if e2 <= dx {
                    err += dx;
                    y0 += sy;
                }
            }
        }
        if let [only] = pts.as_slice() {
            dot(only.0, only.1, s.ink);
        }
    }

    let label = Style::default().fg(theme.label);
    let axis = Style::default().fg(theme.border_dim);
    let mut out: Vec<Line<'static>> = (0..rows)
        .map(|r| {
            let (text, tick) = match r {
                0 => (top.as_str(), '┤'),
                r if r + 1 == rows => (bottom.as_str(), '┤'),
                _ => ("", '│'),
            };
            let mut spans = vec![
                Span::styled(format!("{text:>lw$}"), label),
                Span::styled(format!(" {tick}"), axis),
            ];
            spans.extend((0..plot).map(|c| {
                let ch = char::from_u32(0x2800 + bits[r][c] as u32).unwrap_or(' ');
                match ink[r][c] {
                    Some(colour) => Span::styled(ch.to_string(), Style::default().fg(colour)),
                    None => Span::raw(ch.to_string()),
                }
            }));
            Line::from(spans)
        })
        .collect();
    let back = if span >= 10.0 {
        format!(" -{span:.0} s ")
    } else {
        format!(" -{span:.1} s ")
    };
    let dashes = plot.saturating_sub(back.chars().count() + 4);
    out.push(Line::from(vec![
        Span::raw(" ".repeat(lw + 1)),
        Span::styled("└".to_string(), axis),
        Span::styled(back, label),
        Span::styled("─".repeat(dashes), axis),
        Span::styled(" now".to_string(), label),
    ]));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(line: &Line<'static>) -> String {
        line.spans.iter().map(|s| s.content.to_string()).collect()
    }

    /// The braille cells of a plot row, with their colours, the label and
    /// the axis left off.
    fn cells(line: &Line<'static>) -> Vec<(char, Option<ratatui::style::Color>)> {
        line.spans
            .iter()
            .flat_map(|s| s.content.chars().map(move |c| (c, s.style.fg)))
            .filter(|(c, _)| ('\u{2800}'..='\u{28ff}').contains(c))
            .collect()
    }

    fn flat(value: f64, ink: ratatui::style::Color) -> Series {
        Series {
            points: (0..12).map(|k| (60.0 - k as f64 * 5.0, value)).collect(),
            ink,
        }
    }

    /// Both ends on one scale: the lower one's dots in the lower rows, the
    /// higher one's in the upper, the scale's ends labelled.
    #[test]
    fn both_ends_share_one_scale() {
        let theme = crate::Theme::sdr();
        let (m, s) = (theme.series_color(1), theme.value);
        let out = time_plot([&flat(0.310, m), &flat(0.333, s)], 6, 60, 3, 60.0, &theme);
        assert_eq!(out.len(), 7, "six rows and the axis");
        assert!(text(&out[0]).contains("0.333"), "{}", text(&out[0]));
        assert!(text(&out[5]).contains("0.310"), "{}", text(&out[5]));
        let top: Vec<_> = cells(&out[0])
            .into_iter()
            .filter(|(c, _)| *c != '\u{2800}')
            .collect();
        let bottom: Vec<_> = cells(&out[5])
            .into_iter()
            .filter(|(c, _)| *c != '\u{2800}')
            .collect();
        assert!(
            !top.is_empty() && top.iter().all(|(_, ink)| *ink == Some(s)),
            "{top:?}"
        );
        assert!(
            !bottom.is_empty() && bottom.iter().all(|(_, ink)| *ink == Some(m)),
            "{bottom:?}"
        );
    }

    /// A point lands at its time: 30 s ago on a 60 s axis in the middle, the
    /// newest at the right edge; the axis says how far back it reaches.
    #[test]
    fn a_point_lands_at_its_time() {
        let theme = crate::Theme::sdr();
        let one = Series {
            points: vec![
                (60.0, 1.0),
                (45.0, 1.0),
                (30.0, 5.0),
                (15.0, 1.0),
                (0.0, 1.0),
            ],
            ink: theme.value,
        };
        let none = Series {
            points: Vec::new(),
            ink: theme.value,
        };
        let out = time_plot([&one, &none], 4, 50, 1, 60.0, &theme);
        let top = cells(&out[0]);
        let lit: Vec<usize> = top
            .iter()
            .enumerate()
            .filter(|(_, (c, _))| *c != '\u{2800}')
            .map(|(i, _)| i)
            .collect();
        let middle = top.len() / 2;
        assert!(
            lit.iter().any(|&i| i.abs_diff(middle) <= 1),
            "{lit:?} of {}",
            top.len()
        );
        let bottom = cells(&out[3]);
        assert_ne!(
            bottom[bottom.len() - 1].0,
            '\u{2800}',
            "the newest at the edge"
        );
        let axis = text(&out[4]);
        assert!(axis.contains("-60 s") && axis.contains("now"), "{axis}");
    }

    /// Where the two ends fall in one cell the master's colour is drawn.
    #[test]
    fn each_end_wears_its_colour() {
        let theme = crate::Theme::sdr();
        let (m, s) = (theme.series_color(1), theme.value);
        let out = time_plot([&flat(2.0, m), &flat(2.0, s)], 3, 40, 1, 60.0, &theme);
        let lit: Vec<_> = out[..3]
            .iter()
            .flat_map(cells)
            .filter(|(c, _)| *c != '\u{2800}')
            .collect();
        assert!(!lit.is_empty());
        assert!(lit.iter().all(|(_, ink)| *ink == Some(m)), "{lit:?}");
    }

    /// Fewer than four points on either end is not a plot.
    #[test]
    fn too_few_points_draw_nothing() {
        let theme = crate::Theme::sdr();
        let few = Series {
            points: vec![(3.0, 1.0), (2.0, 2.0), (1.0, 3.0)],
            ink: theme.value,
        };
        let none = Series {
            points: Vec::new(),
            ink: theme.value,
        };
        assert!(time_plot([&few, &none], 6, 60, 1, 60.0, &theme).is_empty());
    }

    /// No row is wider than it was given, at any width.
    #[test]
    fn every_row_keeps_its_width() {
        let theme = crate::Theme::sdr();
        let (m, s) = (theme.series_color(1), theme.value);
        for w in [20usize, 40, 62, 90] {
            for line in time_plot([&flat(0.310, m), &flat(0.333, s)], 6, w, 3, 60.0, &theme) {
                assert!(line.width() <= w, "{w}: {}", text(&line));
            }
        }
    }
}
