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

/// Label column for the noise step block.
///
/// Wider than the panel's `LABEL_W` because these rows are named rather than
/// coded, and `row` treats `label_w` as a minimum: a longer label does not get
/// truncated, it silently pushes the right-hand reading off the end of the line.
const STEP_LABEL_W: usize = 5;

/// Per-stage own NF as bars, then the Friis system total.
///
/// The total can sit *below* the worst individual stage, and that is not a bug:
/// the LNA's gain suppresses the noise of everything after it. The per-stage bars
/// are what make that legible rather than surprising.
/// What stands in for the two noise blocks on a device whose chain sdrtop has
/// never been told the noise figures for.
///
/// **It says what is missing and why**, in the space the numbers would have
/// filled, rather than leaving a hole. The reason comes from the gain model, so
/// an RTL-SDR reads "single tuner, no cascade" and a SoapySDR device reads
/// "chain not modelled": two different facts that used to share one sentence.
pub(super) fn not_modelled(
    out: &mut Vec<Line<'static>>,
    reason: &str,
    iw: usize,
    theme: &crate::Theme,
) {
    out.push(section("Noise figure", "not modelled", iw, theme));
    out.push(Line::raw(""));
    out.push(Line::from(vec![
        Span::raw(" "),
        Span::styled(reason.to_string(), Style::default().fg(theme.stale)),
    ]));
    out.push(Line::raw(""));
    out.push(Line::from(vec![
        Span::raw(" "),
        Span::styled(
            "NF and MDS need each stage's own noise figure,".to_string(),
            Style::default().fg(theme.label),
        ),
    ]));
    out.push(Line::from(vec![
        Span::raw(" "),
        Span::styled(
            "which no driver reports. The levels above are".to_string(),
            Style::default().fg(theme.label),
        ),
    ]));
    out.push(Line::from(vec![
        Span::raw(" "),
        Span::styled(
            "measured and do not need it.".to_string(),
            Style::default().fg(theme.label),
        ),
    ]));
}

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
                label: &s.label,
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
/// A tuned frequency for the section hint: three decimals, the way the header
/// spells it, so the two can be compared at a glance.
fn fmt_tuned(hz: u64) -> String {
    format!("{:.3} MHz", hz as f64 / 1e6)
}

fn fmt_mhz(hz: u32) -> String {
    if hz >= 1_000_000 {
        format!("{:.0} MHz", hz as f64 / 1e6)
    } else if hz > 0 {
        format!("{} kHz", hz / 1000)
    } else {
        "\u{2014}".to_string()
    }
}

/// What the last completed sweep found.
///
/// **The claim, written before the block was drawn:** this says where the
/// converter stops limiting the receiver. It does not say how noisy the front
/// end is - that needs a known source at the input, which sdrtop has no way to
/// ask for. The last line of the block says so on screen, because a bench that
/// prints a slope next to a modelled noise figure invites exactly that reading.
///
/// The knee is the headline and the span slope is deliberately not shown: it
/// averages the converter-limited half with the front-end-limited half and
/// describes neither. See `Reading::slope_above_knee`.
pub(super) fn sweep_reading(
    out: &mut Vec<Line<'static>>,
    state: &SdrMetrics,
    iw: usize,
    theme: &crate::Theme,
) {
    let Some(nr) = state.lab.noise_reading.as_ref() else {
        return;
    };
    let r = &nr.reading;
    let stage = state
        .caps
        .gain
        .stages()
        .first()
        .map(|s| s.name.clone())
        .unwrap_or_else(|| "gain".to_string());

    // **The frequency is in the hint, always.** The knee is a property of the
    // band as much as of the radio, so a reading that outlives a retune has to
    // say where it came from; naming it every time is one rule instead of a
    // staleness rule that can disagree with itself.
    out.push(section(
        "Noise step",
        &format!("measured at {}", fmt_tuned(nr.at_hz)),
        iw,
        theme,
    ));
    out.push(Line::raw(""));

    // The knee, and how completely the front end had taken over above it.
    let (mid, mid_col, right) = match (r.knee_db, r.slope_above_knee) {
        (Some(k), Some(above)) => (
            format!("{stage} {k:.0} dB and up"),
            theme.value,
            format!("{above:.2} dB/dB"),
        ),
        // No knee: the floor never followed the gain at any setting the sweep
        // tried. That is an answer, not a missing one, and here the span slope
        // *is* the right number to quote - with only one regime in the data
        // there are no two halves for it to average together.
        _ => (
            "none in range".to_string(),
            theme.stale,
            format!("{:.2} dB/dB", r.slope),
        ),
    };
    out.push(row(
        Row {
            label: "knee",
            label_w: STEP_LABEL_W,
            mid,
            mid_col,
            right,
            right_col: theme.value_hi,
        },
        iw,
        theme,
    ));

    // The measured floor across the sweep, as it was walked.
    let floors: Vec<f32> = r.points.iter().map(|p| p.noise_dbfs).collect();
    if let (Some(lo), Some(hi)) = (floors.first(), floors.last()) {
        let ends = format!("{lo:.0} \u{2192} {hi:.0} dBFS");
        let spark_w = iw
            .saturating_sub(1 + LABEL_W + 1 + ends.chars().count() + 1)
            .max(4);
        let (spark, _) = spark_minmax(&floors, spark_w);
        out.push(row(
            Row {
                label: "floor",
                label_w: STEP_LABEL_W,
                mid: spark,
                mid_col: theme.value,
                right: ends,
                right_col: theme.value_hi,
            },
            iw,
            theme,
        ));
    }

    out.push(Line::raw(""));
    let claim = match r.knee_db {
        Some(k) => format!(
            "Under {k:.0} dB the converter sets the floor, not the RF. Not a noise figure: that needs a known source."
        ),
        None => "The floor never followed the gain, so the converter set it at every setting. Not a noise figure: that needs a known source.".to_string(),
    };
    for text in crate::ui::chrome::wrap(&claim, iw.saturating_sub(2), 4) {
        out.push(Line::from(vec![
            Span::raw(" "),
            Span::styled(text, Style::default().fg(theme.label)),
        ]));
    }
}

/// The noise step measurement, while it is running.
///
/// Draws nothing at all when no sweep is under way: an idle instrument should
/// not spend a row saying it is idle.
pub(super) fn sweep_progress(
    out: &mut Vec<Line<'static>>,
    state: &SdrMetrics,
    iw: usize,
    theme: &crate::Theme,
) {
    let Some(sw) = state.lab.noise_sweep.as_ref() else {
        return;
    };
    let (done, total) = sw.steps();
    let name = state
        .caps
        .gain
        .stages()
        .get(sw.stage())
        .map(|s| s.name.clone())
        .unwrap_or_else(|| "stage".to_string());
    let at = state
        .radio
        .gains
        .get(sw.stage())
        .copied()
        .unwrap_or_default();

    out.push(section("Noise Step", "sweep running", iw, theme));
    out.push(Line::raw(""));
    // Six cells, not ten: the row also carries a stage name, a value and a
    // count, and at the panel's minimum width a wider bar is the part that
    // gets truncated.
    const CELLS: usize = 6;
    let filled = (sw.progress() * CELLS as f32)
        .round()
        .clamp(0.0, CELLS as f32) as usize;
    let bar: String = "\u{2588}".repeat(filled) + &"\u{2591}".repeat(CELLS - filled);
    out.push(row(
        Row {
            label: "sweep",
            label_w: STEP_LABEL_W,
            mid: format!("{name} {at:.0} dB"),
            mid_col: theme.value,
            right: format!("{done}/{total} {bar}"),
            right_col: theme.value_hi,
        },
        iw,
        theme,
    ));
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
