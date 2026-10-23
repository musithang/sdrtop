// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The read-out under the chart: the carrier and its mirror, what the gap
//! between them is called, and where the predicted IRR is.
//!
//! Each carrier and mirror line is `mark · label · value · decoration`, and
//! the decoration is the part that goes when the column is narrow: the 32%
//! Lab IQ slot is often under 33 cells of inner width, and clipping a level
//! mid-word is worse than losing what only repeats the label.
//!
//! There is no DC line. The DC bin's level here and IQ Diagnostics' DC spike
//! are two different figures, and showing both under one name gave the bench
//! two answers to one question; on the chart, the DC is the LO's column.

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use super::detect::{ImageReadout, Reading};
use super::tint::{supp_color, Tint};

/// Number of leading spans a line always keeps: mark, label, value. Only what
/// follows those is droppable.
const ESSENTIAL_SPANS: usize = 3;

/// The lines, fitted to `iw`. `carrier_hz` is the carrier's *absolute*
/// frequency - the readout carries only its offset from the LO, and printing
/// that as though it were a tuned frequency is exactly the kind of near-miss
/// this panel must not make.
pub(super) fn lines(
    r: &ImageReadout,
    carrier_hz: f64,
    image_hz: f64,
    tint: &Tint,
    iw: usize,
    theme: &crate::Theme,
) -> Vec<Line<'static>> {
    let lbl = Style::default().fg(theme.label);
    let dim = Style::default().fg(theme.border_dim);
    if r.reading == Reading::OnTheLo {
        return vec![
            Line::from(Span::styled(
                format!(" the strongest signal, {:.3} MHz,", carrier_hz / 1e6),
                lbl,
            )),
            Line::from(Span::styled(" runs across the LO: its mirror is its", lbl)),
            Line::from(Span::styled(" own other half, not an image", lbl)),
            Line::raw(""),
            fit(
                vec![
                    Span::raw(" "),
                    Span::styled(
                        "NO IMAGE TO MEASURE",
                        Style::default()
                            .fg(theme.status_warn)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(""),
                    Span::styled("  tune it off the LO", dim),
                ],
                iw,
            ),
        ];
    }
    let level = |mark: &str, col, name: &str, hz: f64, dbfs: f32| {
        fit(
            vec![
                Span::styled(format!(" {mark} "), Style::default().fg(col)),
                Span::styled(format!("{name:<8}"), lbl),
                Span::styled(
                    format!("{:>12}", format!("{:.3} MHz", hz / 1e6)),
                    Style::default().fg(col).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("  {dbfs:.0} dBFS"),
                    Style::default().fg(theme.value),
                ),
            ],
            iw,
        )
    };
    let (head, col, why) = match r.reading {
        Reading::Measured(db) => (format!("IMAGE {db:.0} dB DOWN"), supp_color(db, theme), ""),
        // A bound is not graded: it says how good the front end is at least.
        Reading::AtLeast(db) => (
            format!("IMAGE \u{2265} {db:.0} dB DOWN"),
            theme.value_hi,
            "  under the floor",
        ),
        Reading::MirrorBusy(db) => (
            format!("MIRROR {db:.0} dB DOWN"),
            theme.status_warn,
            "  a signal of its own?",
        ),
        Reading::OnTheLo => unreachable!("answered above"),
    };
    vec![
        level(
            "\u{25bc}",
            tint.carrier,
            "carrier",
            carrier_hz,
            r.carrier_dbfs,
        ),
        level("\u{25bd}", tint.image, "mirror", image_hz, r.image_dbfs),
        Line::raw(""),
        fit(
            vec![
                Span::raw(" "),
                Span::styled(head, Style::default().fg(col).add_modifier(Modifier::BOLD)),
                Span::raw(""),
                Span::styled(why.to_string(), dim),
            ],
            iw,
        ),
    ]
}

/// The figure the chart writes beside the gap, or `None` where there is no
/// gap to draw.
pub(super) fn gap_label(reading: Reading) -> Option<String> {
    match reading {
        Reading::Measured(db) => Some(format!("{db:.0} dB")),
        Reading::AtLeast(db) => Some(format!("\u{2265}{db:.0} dB")),
        Reading::MirrorBusy(db) => Some(format!("{db:.0} dB?")),
        Reading::OnTheLo => None,
    }
}

/// The panel's closing line: where the figure this one checks lives.
pub(super) fn caption(theme: &crate::Theme) -> Line<'static> {
    Line::from(Span::styled(
        " the IRR its imbalance predicts \u{2190} left",
        Style::default().fg(theme.border_dim),
    ))
}

/// Drop trailing decoration spans until the line fits `iw`, never cutting into
/// the first [`ESSENTIAL_SPANS`].
fn fit(mut spans: Vec<Span<'static>>, iw: usize) -> Line<'static> {
    while spans.len() > ESSENTIAL_SPANS && width(&spans) > iw {
        spans.pop();
    }
    Line::from(spans)
}

fn width(spans: &[Span]) -> usize {
    spans.iter().map(|s| s.content.chars().count()).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spans() -> Vec<Span<'static>> {
        vec![
            Span::raw(" \u{25bc} "),           // 3
            Span::raw("CARRIER "),             // 8
            Span::raw("92.807 MHz"),           // 10
            Span::raw(" \u{00b7} -35.4 dBFS"), // 15
        ]
    }

    #[test]
    fn a_wide_column_keeps_the_decoration() {
        let l = fit(spans(), 40);
        assert_eq!(l.spans.len(), 4, "36 cells fit in 40");
    }

    #[test]
    fn a_narrow_column_drops_the_decoration_whole() {
        // 21 essential cells, 36 with the decoration: at 30 the tail goes rather
        // than the level being clipped mid-word.
        let l = fit(spans(), 30);
        assert_eq!(l.spans.len(), ESSENTIAL_SPANS);
        assert_eq!(width(&l.spans), 21);
    }

    #[test]
    fn the_essential_spans_are_never_cut() {
        // Narrower than the mark, label and value together: the line overflows and
        // the paragraph clips it, rather than the frequency silently vanishing.
        let l = fit(spans(), 4);
        assert_eq!(
            l.spans.len(),
            ESSENTIAL_SPANS,
            "the value must survive any width"
        );
        assert!(l.spans.iter().any(|s| s.content.contains("92.807")));
    }
}
