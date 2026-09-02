// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The row vocabulary the two Lab RF bench columns share.
//!
//! `adc_loading` and `rf_chain` are one instrument split across two panels - the
//! same gain staging, read from the ADC's end and from the antenna's end - so
//! their rows have to line up and grade the same. They already share the maths in
//! [`rf_calc`](crate::ui::rf_calc); this is the drawing half of that.
//!
//! Both panels also carried their own private copy of the section nameplate,
//! character for character identical to [`chrome::section`](crate::ui::chrome::section),
//! which already named `rf_chain` in its doc comment as one of its users. Those
//! copies are gone; there is nothing to share here because the shared thing was
//! always in `chrome`.

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use crate::ui::widgets::charts::gain_bar_colored;

/// Colour for a [`staging_verdict`](crate::ui::rf_calc::staging_verdict) severity.
///
/// Both panels print the same verdict from the same measured peak, so they must
/// grade it the same colour: a peak that reads red on one column and amber on the
/// other would be two instruments disagreeing about one number.
pub(super) fn severity_color(sev: u8, theme: &crate::Theme) -> Color {
    match sev {
        2 => theme.status_crit,
        1 => theme.status_warn,
        _ => theme.status_ok,
    }
}

/// ` LBL  mid ·········· right` - a label, a middle column, and a right-aligned
/// value pushed to the panel edge.
///
/// `label_w` differs between the two panels (4 for the ADC column's `HDRM` /
/// `peak` / `bits`, 3 for the chain's `LNA` / `MDS` / `sys`), so it is a
/// parameter rather than a constant: the columns are meant to line up *within* a
/// panel, not across the gap between them.
pub(super) struct Row<'a> {
    pub label: &'a str,
    pub label_w: usize,
    pub mid: String,
    pub mid_col: Color,
    pub right: String,
    pub right_col: Color,
}

pub(super) fn row(r: Row<'_>, iw: usize, theme: &crate::Theme) -> Line<'static> {
    let (label, label_w) = (r.label, r.label_w);
    let pad = iw.saturating_sub(1 + label_w + 1 + r.mid.chars().count() + r.right.chars().count());
    Line::from(vec![
        Span::raw(" "),
        Span::styled(
            format!("{label:<label_w$}"),
            Style::default().fg(theme.label),
        ),
        Span::raw(" "),
        Span::styled(r.mid, Style::default().fg(r.mid_col)),
        Span::raw(" ".repeat(pad.max(1))),
        Span::styled(
            r.right,
            Style::default()
                .fg(r.right_col)
                .add_modifier(Modifier::BOLD),
        ),
    ])
}

/// ` LBL [▇▇▇▅  ] value` - the app's standard eighth-block gradient gain bar (the
/// same widget as the command rail and the header's LNA·VGA), with an optional
/// `┊` tick overlaid on one cell to mark an optimal target.
///
/// `tick` is a fraction of the bar's full span, not a column: the caller knows
/// what its own scale means and the bar knows how wide it is, so converting here
/// keeps the two from drifting apart.
pub(super) struct Bar<'a> {
    pub label: &'a str,
    pub label_w: usize,
    pub value: u32,
    pub max: u32,
    pub lo: Color,
    pub hi: Color,
    pub tick: Option<f64>,
    pub val_str: String,
    pub val_col: Color,
}

pub(super) fn bar_row(b: Bar<'_>, bar_w: usize, theme: &crate::Theme) -> Line<'static> {
    let mut bar = gain_bar_colored(b.value, b.max, bar_w, b.lo, b.hi, theme.border_dim);
    if let Some(t) = b.tick {
        let tc = ((t.clamp(0.0, 1.0) * bar_w as f64).round() as usize).min(bar_w.saturating_sub(1));
        if tc < bar.len() {
            bar[tc] = Span::styled("\u{250a}".to_string(), Style::default().fg(theme.value_hi));
        }
    }
    let (label, label_w) = (b.label, b.label_w);
    let mut spans = vec![
        Span::raw(" "),
        Span::styled(
            format!("{label:<label_w$}"),
            Style::default().fg(theme.label),
        ),
        Span::raw(" "),
    ];
    spans.extend(bar);
    spans.push(Span::raw(" "));
    spans.push(Span::styled(
        b.val_str,
        Style::default().fg(b.val_col).add_modifier(Modifier::BOLD),
    ));
    Line::from(spans)
}

/// How wide the bar itself may be: the panel's width less the lead space, the
/// label column, the two gaps and the value field.
pub(super) fn bar_width(iw: usize, label_w: usize, val_w: usize) -> usize {
    iw.saturating_sub(1 + label_w + 1 + 1 + val_w).max(6)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn width(l: &Line<'_>) -> usize {
        l.spans.iter().map(|s| s.content.chars().count()).sum()
    }

    #[test]
    fn severity_colours_escalate() {
        let t = crate::Theme::sdr();
        assert_eq!(severity_color(0, &t), t.status_ok);
        assert_eq!(severity_color(1, &t), t.status_warn);
        assert_eq!(severity_color(2, &t), t.status_crit);
    }

    #[test]
    fn a_row_fills_the_panel_width() {
        let t = crate::Theme::sdr();
        let l = row(
            Row {
                label: "peak",
                label_w: 4,
                mid: "-12 dBFS".into(),
                mid_col: t.value,
                right: "31/127 cts".into(),
                right_col: t.value,
            },
            40,
            &t,
        );
        assert_eq!(width(&l), 40, "the right value lands on the panel edge");
    }

    #[test]
    fn a_row_that_cannot_fit_keeps_one_space_between_its_halves() {
        // Narrower than its own content: the padding must not vanish and glue the
        // middle column to the right-hand value.
        let t = crate::Theme::sdr();
        let l = row(
            Row {
                label: "peak",
                label_w: 4,
                mid: "-12 dBFS".into(),
                mid_col: t.value,
                right: "31/127 cts".into(),
                right_col: t.value,
            },
            10,
            &t,
        );
        assert!(width(&l) > 10, "it overflows rather than colliding");
        assert!(l.spans.iter().any(|s| s.content.as_ref() == " "));
    }

    #[test]
    fn the_tick_lands_inside_the_bar_at_both_extremes() {
        let t = crate::Theme::sdr();
        let bar_w = 12;
        for frac in [0.0, 0.5, 1.0, -0.3, 1.7] {
            let l = bar_row(
                Bar {
                    label: "LNA",
                    label_w: 3,
                    value: 20,
                    max: 40,
                    lo: t.status_ok,
                    hi: t.value_hi,
                    tick: Some(frac),
                    val_str: "20 / 40 dB".into(),
                    val_col: t.value,
                },
                bar_w,
                &t,
            );
            // lead(1) + label(3) + gap(1) + bar + gap(1) + value(10)
            assert_eq!(
                width(&l),
                1 + 3 + 1 + bar_w + 1 + 10,
                "tick at {frac} changed the width"
            );
            let ticks = l
                .spans
                .iter()
                .filter(|s| s.content.as_ref() == "\u{250a}")
                .count();
            assert_eq!(ticks, 1, "exactly one tick, at {frac}");
        }
    }

    #[test]
    fn no_tick_leaves_the_bar_alone() {
        let t = crate::Theme::sdr();
        let l = bar_row(
            Bar {
                label: "AMP",
                label_w: 3,
                value: 5,
                max: 1200,
                lo: t.status_ok,
                hi: t.status_crit,
                tick: None,
                val_str: "0.5 dB".into(),
                val_col: t.value,
            },
            12,
            &t,
        );
        assert!(!l.spans.iter().any(|s| s.content.as_ref() == "\u{250a}"));
    }

    #[test]
    fn bar_width_never_collapses_below_a_readable_minimum() {
        assert_eq!(bar_width(40, 3, 10), 40 - 16);
        // A panel too narrow for the arithmetic still gets a bar rather than none.
        assert_eq!(bar_width(4, 3, 10), 6);
        assert_eq!(bar_width(0, 4, 11), 6);
    }
}
