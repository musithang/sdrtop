// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The row vocabulary this panel's three zones share.
//!
//! A padded label field, the `---` stale form, an inline trend, and the
//! late-by bar. These were four locals inside `render` - `lbl`, `val`, `dim`,
//! `dash` - repeated identically in `timing_vitals`, which is the panel this one
//! sits beside in `lab_timing`.
//!
//! The label column is [`LABEL_W`], the same width `timing_vitals` uses, so the
//! two columns of the bench line up across the gap between them. It used to be an
//! independent `11` in each panel.

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::ui::widgets::micro_common::sparkline;

/// Width of the label field. Clears the longest labels here (`Throughput` =
/// 10) and keeps a separating space, so no value butts up against
/// its label - and matches `timing_vitals` on the other half of the bench.
pub(super) const LABEL_W: usize = 11;

/// Inline trend sparkline width.
pub(super) const SPARK_W: usize = 18;

pub(super) struct Rows<'a> {
    pub iw: usize,
    pub stale: bool,
    pub theme: &'a crate::Theme,
    lbl: Style,
    val: Style,
    dim: Style,
}

impl<'a> Rows<'a> {
    pub(super) fn new(iw: usize, stale: bool, theme: &'a crate::Theme) -> Self {
        Self {
            iw,
            stale,
            theme,
            lbl: Style::default().fg(theme.label),
            val: Style::default().fg(theme.value),
            dim: Style::default().fg(theme.stale),
        }
    }

    pub(super) fn lbl(&self) -> Style {
        self.lbl
    }
    pub(super) fn val(&self) -> Style {
        self.val
    }
    pub(super) fn dim(&self) -> Style {
        self.dim
    }

    /// The `[R]` / `[C]` key style used by the footer hint line.
    pub(super) fn key(&self) -> Style {
        Style::default()
            .fg(self.theme.value_hi)
            .add_modifier(Modifier::BOLD)
    }

    /// Pad a label to the shared column so values line up down the zone.
    pub(super) fn field(&self, name: &str) -> Span<'static> {
        crate::ui::chrome::field(name, LABEL_W, self.theme)
    }

    pub(super) fn dash(&self) -> Span<'static> {
        Span::styled("---".to_string(), self.dim)
    }

    /// `field(name)` then either the value spans or a dash. The shape almost every
    /// row in this panel has.
    pub(super) fn row(&self, name: &str, live: Vec<Span<'static>>) -> Line<'static> {
        if self.stale {
            Line::from(vec![self.field(name), self.dash()])
        } else {
            let mut spans = vec![self.field(name)];
            spans.extend(live);
            Line::from(spans)
        }
    }

    /// A labelled inline sparkline, blank while stale.
    pub(super) fn trend(&self, name: &str, hist: &[f64], tail: &'static str) -> Line<'static> {
        // Nothing to draw and nothing to label: a `60 s` beside an empty
        // trend reads as a trend of nothing.
        let (spark, tail) = if self.stale || hist.is_empty() {
            (String::new(), String::new())
        } else {
            (sparkline(hist, SPARK_W), tail.to_string())
        };
        Line::from(vec![
            self.field(name),
            Span::styled(spark, self.val),
            Span::styled(tail, self.dim),
        ])
    }
}

/// One late-by bar: how late a callback came, as a fraction of the budget,
/// on a log scale from 1 % at the left to 10× at the right, the budget `┊`
/// two thirds of the way. A linear bar to twice the budget, as this was,
/// filled to the end at 2× and drew 5× and 8× the same.
pub(super) fn late_bar(
    value: u64,
    budget: u64,
    bar_w: usize,
    theme: &crate::Theme,
) -> (Vec<Span<'static>>, ratatui::style::Color) {
    let ratio = value as f64 / budget.max(1) as f64;
    let col = if ratio > 1.0 {
        theme.status_crit
    } else if ratio > 0.5 {
        theme.status_warn
    } else {
        theme.status_ok
    };
    let fill = late_col(ratio, bar_w);
    let tick = late_col(1.0, bar_w);
    let bar = (0..bar_w)
        .map(|x| {
            if x == tick {
                Span::styled("\u{250a}".to_string(), Style::default().fg(theme.label))
            } else if x <= fill && value > 0 {
                Span::styled("\u{2501}".to_string(), Style::default().fg(col))
            } else {
                Span::styled(
                    "\u{2500}".to_string(),
                    Style::default().fg(theme.border_dim),
                )
            }
        })
        .collect();
    (bar, col)
}

/// The column of a late-by ratio on the bar: 1 % at 0, 10x at the end.
pub(super) fn late_col(ratio: f64, bar_w: usize) -> usize {
    let decades = (ratio.max(0.01).log10() + 2.0) / 3.0;
    (decades.clamp(0.0, 1.0) * bar_w.saturating_sub(1) as f64).round() as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    fn width(spans: &[Span]) -> usize {
        spans.iter().map(|s| s.content.chars().count()).sum()
    }

    /// Exactly the bar's width, the budget marker once, two thirds along.
    #[test]
    fn the_late_bar_has_one_marker_at_the_budget() {
        let t = Theme::sdr();
        let (spans, _) = late_bar(88, 600, 31, &t);
        assert_eq!(width(&spans), 31);
        let ticks: Vec<usize> = spans
            .iter()
            .enumerate()
            .filter(|(_, s)| s.content.contains('\u{250a}'))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(ticks, vec![20]);
    }

    /// **5x and 8x the budget read apart**, where a bar to twice the budget
    /// drew both full; and a wild overrun or a zero budget still fits.
    #[test]
    fn far_past_the_budget_still_reads_apart_and_fits() {
        assert!(late_col(5.0, 31) < late_col(8.0, 31));
        assert_eq!(late_col(10.0, 31), 30);
        assert_eq!(late_col(100.0, 31), 30);
        let t = Theme::sdr();
        assert_eq!(width(&late_bar(u64::MAX / 2, 600, 12, &t).0), 12);
        assert_eq!(width(&late_bar(0, 0, 12, &t).0), 12);
    }

    /// Stale rows say `---` and carry no value, live rows carry the value.
    #[test]
    fn a_row_hides_its_value_while_stale() {
        let t = Theme::sdr();
        let live = Rows::new(50, false, &t);
        let dead = Rows::new(50, true, &t);
        let value = vec![Span::styled("4096 µs".to_string(), live.val())];
        let text = |l: Line<'static>| -> String {
            l.spans.iter().map(|s| s.content.to_string()).collect()
        };
        assert!(text(live.row("Period", value.clone())).contains("4096"));
        let d = text(dead.row("Period", value));
        assert!(d.contains("---") && !d.contains("4096"), "{d:?}");
    }
}
