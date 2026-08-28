//! The row vocabulary this panel's five zones share.
//!
//! Two shapes: a **trend block** (a heading line with an inline 60 s sparkline
//! under it) and a **captioned bar row** (fixed label column, computed bar width,
//! value tail). Plus the convention that a stopped radio reads `---` rather than
//! the last number it happened to have.
//!
//! The label column is [`LABEL_W`], and it is the same 11 columns
//! `timing_diagnostics` uses on the other half of the `lab_timing` bench — the
//! two panels are read side by side, so their values line up down one column
//! across the gap. That was two independent `11`s before this split.

use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};

use crate::ui::widgets::charts::gain_bar_colored;
use crate::ui::widgets::micro_common::sparkline;

/// Width of the label column. Shared with `timing_diagnostics`; see the module
/// note. Wide enough for `Bus throughput`'s neighbours without a value butting
/// up against its label.
pub(super) const LABEL_W: usize = 11;

/// A resource's load, in percent, at which it stops being comfortable.
///
/// Deliberately one pair for two readings — CPU load and peak ring-buffer fill.
/// Both answer "how much of this resource is being used", so grading them
/// differently would mean a machine at 70 % CPU and a buffer at 70 % full got
/// different colours for the same degree of pressure.
pub(super) const LOAD_WARN_PCT: f64 = 50.0;
pub(super) const LOAD_CRIT_PCT: f64 = 80.0;

pub(super) fn load_color(pct: f64, theme: &crate::Theme) -> Color {
    if pct >= LOAD_CRIT_PCT {
        theme.status_crit
    } else if pct >= LOAD_WARN_PCT {
        theme.status_warn
    } else {
        theme.status_ok
    }
}

pub(super) struct Rows<'a> {
    pub iw: usize,
    /// The radio is not streaming, so every measured value is a leftover.
    pub stale: bool,
    pub theme: &'a crate::Theme,
    lbl: Style,
    val: Style,
    dim: Style,
    spark_w: usize,
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
            spark_w: iw.saturating_sub(2).max(4),
        }
    }

    pub(super) fn lbl(&self) -> Style {
        self.lbl
    }

    pub(super) fn dim(&self) -> Style {
        self.dim
    }

    /// The stand-in for a number that is not being measured right now.
    pub(super) fn dash(&self) -> Span<'static> {
        Span::styled("---".to_string(), self.dim)
    }

    /// A heading row plus its 60 s trend. The sparkline is blank while stale:
    /// history from a stopped stream is not a trend, it is a leftover.
    pub(super) fn trend(&self, heading: Vec<Span<'static>>, hist: Vec<f64>) -> [Line<'static>; 2] {
        let s = if self.stale {
            String::new()
        } else {
            sparkline(&hist, self.spark_w)
        };
        [
            Line::from(heading),
            Line::from(vec![Span::raw(" "), Span::styled(s, self.val)]),
        ]
    }

    /// A captioned ⅛-block bar that never lets the value collide with the bar:
    /// fixed label column, computed bar width, value tail.
    pub(super) fn bar(
        &self,
        label: &'static str,
        ratio: f64,
        value_str: String,
        lo: Color,
        hi: Color,
        val_col: Color,
    ) -> Line<'static> {
        let vw = value_str.chars().count() + 1;
        let bar_w = self.iw.saturating_sub(1 + LABEL_W + 1 + vw).max(4);
        let mut spans = vec![
            Span::raw(" "),
            Span::styled(format!("{label:<LABEL_W$}"), self.lbl),
            Span::raw(" "),
        ];
        if self.stale {
            spans.extend(gain_bar_colored(
                0,
                1000,
                bar_w,
                lo,
                hi,
                self.theme.border_dim,
            ));
            spans.push(Span::styled(" ---".to_string(), self.dim));
        } else {
            let v = (ratio.clamp(0.0, 1.0) * 1000.0) as u32;
            spans.extend(gain_bar_colored(
                v,
                1000,
                bar_w,
                lo,
                hi,
                self.theme.border_dim,
            ));
            spans.push(Span::styled(
                format!(" {value_str}"),
                Style::default().fg(val_col),
            ));
        }
        Line::from(spans)
    }

    /// A `label value  tail` heading, or the label and a dash while stale.
    pub(super) fn heading(
        &self,
        label: &'static str,
        value: String,
        value_color: Color,
        tail: String,
    ) -> Vec<Span<'static>> {
        if self.stale {
            vec![Span::raw(" "), Span::styled(label, self.lbl), self.dash()]
        } else {
            vec![
                Span::raw(" "),
                Span::styled(label, self.lbl),
                Span::styled(value, Style::default().fg(value_color)),
                Span::styled(tail, self.lbl),
            ]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Theme;

    fn text(line: &Line<'static>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    /// The bar row's total width must not depend on how long the value is, or
    /// the bars step in and out down the panel.
    #[test]
    fn bar_rows_end_in_the_same_column_whatever_the_value() {
        let t = Theme::sdr();
        for iw in [30usize, 44, 56, 72, 100] {
            let r = Rows::new(iw, false, &t);
            let short = r.bar("fill depth", 0.5, "5%".into(), t.status_ok, t.status_crit, t.value);
            let long = r.bar("link util", 0.5, "100%".into(), t.status_ok, t.status_crit, t.value);
            assert_eq!(
                text(&short).chars().count(),
                text(&long).chars().count(),
                "iw={iw}: bar rows must be the same width"
            );
        }
    }

    /// A stopped radio must not show a stale sparkline or a stale number.
    #[test]
    fn stale_rows_show_no_history_and_no_value() {
        let t = Theme::sdr();
        let r = Rows::new(56, true, &t);
        let [head, spark] = r.trend(
            r.heading("Sample drops ", "12/s".into(), t.status_crit, "  session 9".into()),
            (0..40).map(|i| i as f64).collect(),
        );
        assert!(text(&head).contains("---"), "{:?}", text(&head));
        assert!(!text(&head).contains("12/s"));
        assert_eq!(text(&spark).trim(), "", "a stale trend drew history");

        let bar = r.bar("fill depth", 0.9, "90%".into(), t.status_ok, t.status_crit, t.value);
        assert!(text(&bar).contains("---"), "{:?}", text(&bar));
        assert!(!text(&bar).contains("90%"));
    }

    /// Live, the same rows carry their numbers.
    #[test]
    fn live_rows_carry_their_values() {
        let t = Theme::sdr();
        let r = Rows::new(56, false, &t);
        let [head, spark] = r.trend(
            r.heading("Sample drops ", "12/s".into(), t.status_crit, "  session 9".into()),
            (0..40).map(|i| (i % 7) as f64).collect(),
        );
        assert!(text(&head).contains("12/s"));
        assert!(!text(&spark).trim().is_empty(), "a live trend drew nothing");
    }

    #[test]
    fn the_load_scale_escalates_and_is_shared() {
        let t = Theme::sdr();
        assert_eq!(load_color(0.0, &t), t.status_ok);
        assert_eq!(load_color(LOAD_WARN_PCT, &t), t.status_warn);
        assert_eq!(load_color(LOAD_CRIT_PCT, &t), t.status_crit);
        assert!(LOAD_WARN_PCT < LOAD_CRIT_PCT);
    }

    /// A panel too narrow for the label plus a value still draws a usable bar
    /// rather than underflowing to nothing.
    #[test]
    fn a_very_narrow_panel_keeps_a_drawable_bar() {
        let t = Theme::sdr();
        let r = Rows::new(6, false, &t);
        let bar = r.bar("fill depth", 0.5, "50%".into(), t.status_ok, t.status_crit, t.value);
        assert!(!text(&bar).is_empty());
    }
}
