// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! IMAGE REJECTION section: the IRR bar and its 60-second trend.
//!
//! IRR is the one reading here where higher is better, so the bar's gradient runs
//! crit → ok rather than the other way, and the trend sparkline is auto-scaled:
//! IRR usually sits high and flat, and a fixed scale would show a straight line
//! whether it was steady or drifting a decibel a minute.

use ratatui::{
    style::Style,
    text::{Line, Span},
};

use crate::state::SdrMetrics;
use crate::ui::chrome::section;
use crate::ui::widgets::micro_common::spark_minmax;

use super::reading::Reading;
use super::rows::Rows;
use super::severity::irr_color;

/// The bar's full scale. Matches [`Reading::irr_text`]'s cap, so a `> 60 dB`
/// reading is a full bar rather than an overflowing one.
const IRR_FULL_SCALE_DB: f64 = 60.0;

/// Columns reserved for the trend's `±NN.N dB/60s` annotation.
///
/// The bars reserve their own value column, but the trend's annotation is wider,
/// so it cannot reuse the row vocabulary's `field_w` without overrunning the
/// right edge. Hence a budget of its own.
const TREND_ANN_W: usize = 13;
/// Columns the `" trend "` prefix costs.
const TREND_LEAD_W: usize = 7;

pub(super) fn lines(state: &SdrMetrics, r: &Reading, rows: &Rows) -> Vec<Line<'static>> {
    let theme = rows.theme;
    let color = irr_color(r.irr_db, theme);

    let mut out = vec![
        section(
            "Image rejection",
            "IRR \u{00b7} higher better",
            rows.iw,
            theme,
        ),
        Line::raw(""),
        rows.bar(
            "IRR",
            r.irr_db / IRR_FULL_SCALE_DB,
            theme.status_crit,
            theme.status_ok,
            color,
            r.irr_text(1),
        ),
        Line::raw(""),
    ];

    let history: Vec<f32> = state.iq.irr_history.iter().copied().collect();
    let spark_w = rows
        .iw
        .saturating_sub(TREND_LEAD_W + 1 + TREND_ANN_W)
        .max(1);
    let (spark, p2p) = spark_minmax(&history, spark_w);
    if !spark.is_empty() {
        let ann = format!("\u{00b1}{:.1} dB/60s", p2p / 2.0);
        let pad = rows
            .iw
            .saturating_sub(TREND_LEAD_W + spark.chars().count() + ann.chars().count());
        out.push(Line::from(vec![
            Span::raw(" "),
            Span::styled("trend", rows.label_style()),
            Span::raw(" "),
            Span::styled(spark, Style::default().fg(color)),
            Span::raw(" ".repeat(pad.max(1))),
            Span::styled(ann, Style::default().fg(rows.dim())),
        ]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Theme;

    /// With no history there is no trend row at all, rather than an empty one
    /// that eats a line of a short panel.
    #[test]
    fn no_history_draws_no_trend_row() {
        let t = Theme::sdr();
        let m = SdrMetrics::fixture();
        let rows = Rows::new(60, &t);
        let out = lines(&m, &Reading::of(&m), &rows);
        assert_eq!(out.len(), 4, "a trend row appeared with no history");
    }

    /// And with history it appears, annotated with the window's spread.
    #[test]
    fn history_adds_an_annotated_trend_row() {
        let t = Theme::sdr();
        let mut m = SdrMetrics::fixture();
        m.iq.irr_history = (0..60).map(|i| 40.0 + (i % 5) as f32).collect();
        let rows = Rows::new(60, &t);
        let out = lines(&m, &Reading::of(&m), &rows);
        assert_eq!(out.len(), 5);
        let text: String = out[4].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("trend"), "{text:?}");
        assert!(text.contains("dB/60s"), "{text:?}");
        // Peak-to-peak of 0..4 is 4, so the ± annotation is half of it.
        assert!(text.contains("\u{00b1}2.0"), "{text:?}");
    }
}
