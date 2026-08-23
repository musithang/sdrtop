//! The four read-out lines under the chart, plus the caption.
//!
//! Each line is `mark · label · value · decoration`, and the decoration is the
//! part that goes when the column is narrow: the 32% Lab IQ slot is often under
//! 33 cells of inner width, and clipping a level mid-word is worse than losing a
//! parenthetical that only names what the label already said.

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use super::detect::ImageReadout;
use super::tint::{supp_color, Tint};

/// Number of leading spans a line always keeps: mark, label, value. Only what
/// follows those is droppable.
const ESSENTIAL_SPANS: usize = 3;

/// The four lines, fitted to `iw`. `carrier_hz` is the carrier's *absolute*
/// frequency — the readout carries only its offset from the LO, and printing that
/// as though it were a tuned frequency is exactly the kind of near-miss this
/// panel must not make.
pub(super) fn lines(
    r: &ImageReadout, carrier_hz: f64, tint: &Tint, iw: usize, theme: &crate::Theme,
) -> Vec<Line<'static>> {
    let dimc = theme.border_dim;
    let lbl = Style::default().fg(theme.label);

    // Image level relative to the carrier: normally negative (image below); a
    // positive value flags that the "carrier" is weaker than its mirror.
    let rel = r.image_dbfs - r.carrier_dbfs;
    let rel_str = if rel <= 0.0 { format!("\u{2212}{:.1} dB", -rel) }
                  else          { format!("+{rel:.1} dB") };
    let supp_c = supp_color(r.suppression_db, theme);

    vec![
        fit(vec![
            Span::styled(" \u{25bc} ", Style::default().fg(tint.carrier)),
            Span::styled("CARRIER ", lbl),
            Span::styled(format!("{:.3} MHz", carrier_hz / 1e6),
                         Style::default().fg(tint.carrier).add_modifier(Modifier::BOLD)),
            Span::styled(format!(" \u{00b7} {:.1} dBFS", r.carrier_dbfs), Style::default().fg(dimc)),
        ], iw),
        fit(vec![
            Span::styled(" \u{25bc} ", Style::default().fg(tint.image)),
            Span::styled("IMAGE ", lbl),
            Span::styled(format!("\u{00b7} {:.1} dBFS", r.image_dbfs),
                         Style::default().fg(tint.image)),
            Span::styled(" (mirror)", lbl),
        ], iw),
        fit(vec![
            Span::styled(" \u{25ae} ", Style::default().fg(tint.dc)),
            Span::styled("DC spike ", lbl),
            Span::styled(format!("\u{00b7} {:.1} dBFS", r.dc_dbfs), Style::default().fg(tint.dc)),
            Span::styled(" (I/Q offset)", Style::default().fg(dimc)),
        ], iw),
        Line::from(vec![
            Span::raw(" "),
            Span::styled("image supp. ", lbl),
            Span::styled(rel_str,
                         Style::default().fg(supp_c).add_modifier(Modifier::BOLD)),
        ]),
    ]
}

/// The panel's closing line, echoed by the constellation panel's own caption so
/// the two Lab IQ panels read as one bench.
pub(super) fn caption(theme: &crate::Theme) -> Line<'static> {
    Line::from(vec![
        Span::raw(" "),
        Span::styled("image mirrors the carrier about the LO", Style::default().fg(theme.border_dim)),
    ])
}

/// Drop trailing decoration spans until the line fits `iw`, never cutting into
/// the first [`ESSENTIAL_SPANS`].
fn fit(mut spans: Vec<Span<'static>>, iw: usize) -> Line<'static> {
    while spans.len() > ESSENTIAL_SPANS && width(&spans) > iw { spans.pop(); }
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
            Span::raw(" \u{25bc} "),          // 3
            Span::raw("CARRIER "),            // 8
            Span::raw("92.807 MHz"),          // 10
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
        assert_eq!(l.spans.len(), ESSENTIAL_SPANS, "the value must survive any width");
        assert!(l.spans.iter().any(|s| s.content.contains("92.807")));
    }
}
