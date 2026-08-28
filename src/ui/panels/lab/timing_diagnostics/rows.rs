//! The row vocabulary this panel's three zones share.
//!
//! A padded label field, the `---` stale form, an inline trend, and the deadline
//! budget bar. These were four locals inside `render` — `lbl`, `val`, `dim`,
//! `dash` — repeated identically in `timing_vitals`, which is the panel this one
//! sits beside in `lab_timing`.
//!
//! The label column is [`LABEL_W`], the same width `timing_vitals` uses, so the
//! two columns of the bench line up across the gap between them. It used to be an
//! independent `11` in each panel.

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::ui::widgets::charts::gain_bar_colored;
use crate::ui::widgets::micro_common::sparkline;

/// Width of the label field. Clears the longest labels here (`Host drift`,
/// `Throughput` = 10) and keeps a separating space, so no value butts up against
/// its label — and matches `timing_vitals` on the other half of the bench.
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
        let (spark, tail) = if self.stale {
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

/// One deadline-budget bar in the shared lab bar language: a `gain_bar_colored`
/// ⅛-block fill graded green→red across `bar_w`, with the budget marker `┊`
/// overlaid at mid-bar (full scale = 2 × budget, so the tick sits at the centre
/// and a value that reaches past it is over budget). Same look as the RF-chain
/// gain bars and their optimal-target tick.
pub(super) fn budget_bar(
    value: u64,
    budget: u64,
    bar_w: usize,
    theme: &crate::Theme,
) -> Vec<Span<'static>> {
    let full_scale = (budget.max(1) * 2) as u32;
    let val = value.min(full_scale as u64) as u32;
    let mut bar = gain_bar_colored(
        val,
        full_scale,
        bar_w,
        theme.status_ok,
        theme.status_crit,
        theme.border_dim,
    );
    let tc = ((0.5 * bar_w as f64).round() as usize).min(bar_w.saturating_sub(1));
    if tc < bar.len() {
        bar[tc] = Span::styled("\u{250a}".to_string(), Style::default().fg(theme.value_hi));
    }
    bar
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    #[test]
    fn budget_bar_width_and_single_marker() {
        let t = Theme::sdr();
        let spans = budget_bar(88, 600, 20, &t);
        // Exactly bar_w cells, and the budget marker appears exactly once.
        assert_eq!(
            spans
                .iter()
                .map(|s| s.content.chars().count())
                .sum::<usize>(),
            20
        );
        assert_eq!(
            spans
                .iter()
                .filter(|s| s.content.contains('\u{250a}'))
                .count(),
            1
        );
    }

    /// The marker sits at mid-bar because full scale is twice the budget, so a
    /// value *at* budget fills exactly half. That is the whole reading.
    #[test]
    fn a_value_at_budget_reaches_the_marker() {
        let t = Theme::sdr();
        let at = budget_bar(600, 600, 20, &t);
        let pos = at
            .iter()
            .position(|s| s.content.contains('\u{250a}'))
            .unwrap();
        assert!(
            (9..=11).contains(&pos),
            "the budget tick should sit mid-bar, got {pos}"
        );
    }

    /// A value far over budget is clamped to the bar rather than overflowing it.
    #[test]
    fn a_wild_overrun_still_fits_the_bar() {
        let t = Theme::sdr();
        for w in [6usize, 12, 20, 60] {
            let spans = budget_bar(u64::MAX / 2, 600, w, &t);
            assert_eq!(
                spans
                    .iter()
                    .map(|s| s.content.chars().count())
                    .sum::<usize>(),
                w
            );
        }
    }

    /// A zero budget must not divide by zero or blank the bar.
    #[test]
    fn a_zero_budget_still_draws() {
        let t = Theme::sdr();
        let spans = budget_bar(0, 0, 12, &t);
        assert_eq!(
            spans
                .iter()
                .map(|s| s.content.chars().count())
                .sum::<usize>(),
            12
        );
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
