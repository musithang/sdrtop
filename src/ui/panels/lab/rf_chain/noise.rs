// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `NOISE FIGURE` and `SENSITIVITY` - what the chain costs in noise, and how
//! faint a signal it can still hear.
//!
//! One module because the second reads out of the first: the MDS is derived from
//! the Friis total this section computes.

use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};

use crate::state::SdrMetrics;
use crate::ui::chrome::section;
use crate::ui::rf_calc::{estimate_mds_dbm, Stage};
use crate::ui::widgets::micro_common::spark_minmax;

use super::super::rf_bench::{bar_row, bar_width, row, Bar, Row};
use super::{LABEL_W, VALW};

/// Per-stage own NF as bars, then the Friis system total.
///
/// The total can sit *below* the worst individual stage, and that is not a bug:
/// the LNA's gain suppresses the noise of everything after it. The per-stage bars
/// are what make that legible rather than surprising.
pub(super) fn noise_figure(
    out: &mut Vec<Line<'static>>,
    stages: &[Stage],
    nf: f64,
    iw: usize,
    theme: &crate::Theme,
) {
    let bw = bar_width(iw, LABEL_W, VALW);
    out.push(section("Noise figure", "Friis cascade", iw, theme));
    out.push(Line::raw(""));
    for s in stages {
        out.push(bar_row(
            Bar {
                label: s.label,
                label_w: LABEL_W,
                value: (s.nf_db * 100.0) as u32,
                max: 1200,
                lo: theme.status_ok,
                hi: theme.status_crit,
                tick: None,
                val_str: format!("{:.1} dB", s.nf_db),
                val_col: theme.value,
            },
            bw,
            theme,
        ));
        out.push(Line::raw(""));
    }
    let nf_col = nf_color(nf, theme);
    out.push(row(
        Row {
            label: "sys",
            label_w: LABEL_W,
            mid: "NF total".to_string(),
            mid_col: theme.label,
            right: format!("{nf:.1} dB"),
            right_col: nf_col,
        },
        iw,
        theme,
    ));
}

/// MDS for the current baseband filter, and the 60 s noise-floor trend.
pub(super) fn sensitivity(
    out: &mut Vec<Line<'static>>,
    state: &SdrMetrics,
    nf: f64,
    iw: usize,
    theme: &crate::Theme,
) {
    let dim = theme.border_dim;
    out.push(section("Sensitivity", "noise floor trend", iw, theme));
    out.push(Line::raw(""));
    let mds_str = match estimate_mds_dbm(state.radio.bb_filter_hz, nf) {
        Some(mds) => format!("{mds:.0} dBm"),
        None => "\u{2014}".to_string(),
    };
    out.push(row(
        Row {
            label: "MDS",
            label_w: LABEL_W,
            mid: format!("({} BW)", fmt_mhz(state.radio.bb_filter_hz)),
            mid_col: dim,
            right: mds_str,
            right_col: theme.value_hi,
        },
        iw,
        theme,
    ));
    let floor: Vec<f32> = state.signal.nf_history.iter().copied().collect();
    let spark_w = iw.saturating_sub(1 + 5 + 1 + 12).max(4);
    let (spark, p2p) = spark_minmax(&floor, spark_w);
    if !spark.is_empty() {
        let ann = format!("\u{00b1}{:.1} dB/60s", p2p / 2.0);
        let pad = iw.saturating_sub(7 + spark.chars().count() + ann.chars().count());
        out.push(Line::from(vec![
            Span::raw(" "),
            Span::styled("floor", Style::default().fg(theme.label)),
            Span::raw(" "),
            Span::styled(spark, Style::default().fg(theme.value)),
            Span::raw(" ".repeat(pad.max(1))),
            Span::styled(ann, Style::default().fg(dim)),
        ]));
    }
}

/// Colour for a system noise figure. Under 6 dB is a good receiver, under 10 dB
/// is workable, above that the front end is costing real sensitivity.
fn nf_color(nf: f64, theme: &crate::Theme) -> Color {
    if nf < 6.0 {
        theme.status_ok
    } else if nf < 10.0 {
        theme.status_warn
    } else {
        theme.status_crit
    }
}

/// The baseband filter width, in whichever unit reads cleanly. `—` when there is
/// no filter set: an MDS quoted against a zero bandwidth would be meaningless.
fn fmt_mhz(hz: u32) -> String {
    if hz >= 1_000_000 {
        format!("{:.0} MHz", hz as f64 / 1e6)
    } else if hz > 0 {
        format!("{} kHz", hz / 1000)
    } else {
        "\u{2014}".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_mhz_units() {
        assert_eq!(fmt_mhz(2_000_000), "2 MHz");
        assert_eq!(fmt_mhz(500_000), "500 kHz");
        assert_eq!(fmt_mhz(0), "\u{2014}");
    }

    #[test]
    fn nf_colour_escalates_with_the_system_figure() {
        let t = crate::Theme::sdr();
        assert_eq!(nf_color(3.5, &t), t.status_ok);
        assert_eq!(nf_color(8.0, &t), t.status_warn);
        assert_eq!(nf_color(14.0, &t), t.status_crit);
        // The boundaries belong to the better grade.
        assert_eq!(nf_color(6.0, &t), t.status_warn);
        assert_eq!(nf_color(10.0, &t), t.status_crit);
    }
}
