// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `GAIN LINEUP` and `GAIN STAGING` - the signal's level after each stage, and
//! where the two gain controls sit against their optimal targets.
//!
//! One module because they are the same question asked twice: the lineup says
//! what the current gains produce, the staging says what they should be.

use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};

use crate::hardware::{StageReading, StageSpec};
use crate::ui::chrome::section;
use crate::ui::rf_calc::{Stage, StageLevel};

use super::super::rf_bench::{bar_row, bar_width, row, Bar, Row};
use super::{LABEL_W, VALW};

/// `GAIN LINEUP` - the level at each node of the chain, the converter's dBFS
/// walked back through each stage's gain, ending at the ADC as read.
///
/// Where the stages are `digital` gain after the converter, the walk starts
/// at the converter, not the antenna: the first node is the level the stages
/// are handed (the samples less their gain), the antenna has no level, and the
/// last node is the word the samples are, not the ADC.
#[allow(clippy::too_many_arguments)]
pub(super) fn lineup(
    out: &mut Vec<Line<'static>>,
    levels: &[StageLevel],
    stages: &[Stage],
    adc_peak: f64,
    digital: bool,
    sev_col: Color,
    iw: usize,
    theme: &crate::Theme,
) {
    let dim = theme.border_dim;
    out.push(section("Gain lineup", "level after each stage", iw, theme));
    out.push(Line::raw(""));

    // **The label column is the widest name there actually is**, not a fixed
    // three. A driver names its own stages, and `IFGR` or `PREAMP` is as likely
    // as `LNA`; a three-wide column does not truncate them, it lets the row grow
    // past the pane and the terminal clips the reading off the right-hand end.
    // Which is the wrong thing to lose: the level is the measurement, the label
    // is only its name.
    // `word` closes a digital chain, and is wider than the `ADC` it replaces.
    let label_w = lineup_label_w(levels).max(if digital { 4 } else { 0 });

    // If both columns will not fit, the **middle one goes**. The per-stage gain
    // is context and is spelled out again in the staging block below; the level
    // is the reading and cannot be recovered from anywhere else on the panel.
    let widest_mid = levels
        .iter()
        .enumerate()
        .map(|(i, _)| mid_text(i, stages).chars().count())
        .chain(std::iter::once(4)) // the ADC row's "0 dB"
        .max()
        .unwrap_or(0);
    let widest_right = levels
        .iter()
        .map(|n| level_text(n.signal_dbm).chars().count())
        .chain(std::iter::once(
            format!("{adc_peak:.0} dBFS").chars().count(),
        ))
        .max()
        .unwrap_or(0);
    let show_mid = 1 + label_w + 1 + widest_mid + 1 + widest_right <= iw;

    // Ahead of digital stages the antenna's level is not known: the radio's
    // own AGCs, always on, sit between it and the converter.
    if digital {
        out.push(row(
            Row {
                label: "ANT",
                label_w,
                mid: String::new(),
                mid_col: dim,
                right: "\u{2014}".to_string(),
                right_col: theme.value,
            },
            iw,
            theme,
        ));
        out.push(Line::raw(""));
    }
    for (i, node) in levels.iter().enumerate() {
        let label = if digital && i == 0 {
            "ADC"
        } else {
            node.label.as_str()
        };
        out.push(row(
            Row {
                label: &clip(label, label_w),
                label_w,
                mid: if show_mid {
                    mid_text(i, stages)
                } else {
                    String::new()
                },
                mid_col: dim,
                right: level_text(node.signal_dbm),
                right_col: theme.value,
            },
            iw,
            theme,
        ));
        out.push(Line::raw(""));
    }
    // ADC node = the last stage's output, read in dBFS; after digital stages,
    // the word the samples are.
    out.push(row(
        Row {
            label: if digital { "word" } else { "ADC" },
            label_w,
            mid: if show_mid && !digital {
                "0 dB".to_string()
            } else {
                String::new()
            },
            mid_col: dim,
            right: format!("{adc_peak:.0} dBFS"),
            right_col: sev_col,
        },
        iw,
        theme,
    ));
}

/// Widest stage name in the lineup, bounded.
///
/// `ADC` is always present, so three is the floor. The ceiling stops one
/// verbose driver name from eating the row it is supposed to label.
pub(super) fn lineup_label_w(levels: &[StageLevel]) -> usize {
    levels
        .iter()
        .map(|n| n.label.chars().count())
        .max()
        .unwrap_or(LABEL_W)
        .clamp(LABEL_W, 8)
}

/// A name cut to the column, rather than allowed to push the row wider.
fn clip(label: &str, w: usize) -> String {
    if label.chars().count() <= w {
        label.to_string()
    } else {
        label.chars().take(w).collect()
    }
}

/// The middle column: what this node's stage contributed. The antenna is where
/// the signal arrives, so it contributes nothing and says so with a dash.
fn mid_text(index: usize, stages: &[Stage]) -> String {
    match index.checked_sub(1).and_then(|k| stages.get(k)) {
        // Rounded, as every other panel rounds a stage's gain.
        Some(stage) if stage.gain_db.is_finite() => format!("{:+.0} dB", stage.gain_db),
        _ => "\u{2014}".to_string(),
    }
}

/// A node's level, the converter's dBFS walked back through the gains (not
/// dBm: nothing on any radio here is calibrated to the antenna), or a dash
/// where it could not be worked out.
fn level_text(dbm: f64) -> String {
    if dbm.is_finite() {
        format!("{dbm:.0} dBFS")
    } else {
        "\u{2014}".to_string()
    }
}

/// `GAIN STAGING` - each stage against its own range, with the optimal target
/// ticked, and the target spelled out underneath when the chain is not there.
///
/// **One bar per stage the device has**, named and scaled by the driver's own
/// answer. It used to be exactly two bars, labelled LNA and VGA and measured
/// against 40 and 62: correct on a HackRF and wrong on anything else, including
/// a HackRF reached through SoapySDR, whose stage list is the driver's.
#[allow(clippy::too_many_arguments)]
pub(super) fn staging(
    out: &mut Vec<Line<'static>>,
    stages: &[StageSpec],
    current: &[f64],
    // `None` where no target can be offered: a stage whose dB is not
    // measured here, or one the tuner AGC is setting.
    targets: Option<&[f64]>,
    readings: &[Option<StageReading>],
    // Digital stages after the converter: the target places the level in
    // the samples, it is not a staging of the front end.
    digital: bool,
    iw: usize,
    theme: &crate::Theme,
) {
    let label_w = stages
        .iter()
        .map(|s| s.name.chars().count())
        .max()
        .unwrap_or(LABEL_W)
        .clamp(LABEL_W, 8);
    let bw = bar_width(iw, label_w, VALW);
    out.push(section(
        "Gain staging",
        if digital {
            "\u{2502} = target in the word"
        } else {
            "\u{2502} = optimal target"
        },
        iw,
        theme,
    ));
    out.push(Line::raw(""));

    for (index, spec) in stages.iter().enumerate() {
        let value = current.get(index).copied().unwrap_or(spec.min_db);
        let target = targets.and_then(|t| t.get(index).copied());
        let span = (spec.max_db - spec.min_db).max(1.0);
        // The front stage keeps the warmer ramp, as in the command rail: it is
        // the one whose setting decides the noise figure.
        let (lo, hi) = if index == 0 {
            (theme.status_ok, theme.value_hi)
        } else {
            (theme.border_accent, theme.status_warn)
        };
        // A gain against its range where it was measured; its position among
        // its settings, with no target, where it was not.
        let reading = readings
            .get(index)
            .copied()
            .flatten()
            .unwrap_or(StageReading::Db(value));
        let (bar_value, bar_max, tick, val_str) = match reading {
            StageReading::Position { at, of } => (at, of.max(1), None, format!("{at}/{of} step")),
            // The tuner's own setting, which sdrtop does not know.
            StageReading::Auto => (0, 1, None, "AGC".to_string()),
            // Over the stage's own range, from its deepest setting on an
            // attenuator (the HF+'s ATT runs -48 to 0 dB).
            StageReading::Db(_) => (
                (value - spec.min_db.min(0.0)).max(0.0).round() as u32,
                ((spec.max_db - spec.min_db.min(0.0)).round() as u32).max(1),
                target.map(|t| ((t - spec.min_db) / span).clamp(0.0, 1.0)),
                format!("{:.0} / {:.0} dB", value, spec.max_db),
            ),
        };
        out.push(bar_row(
            Bar {
                label: &clip(&spec.name, label_w),
                label_w,
                value: bar_value,
                max: bar_max,
                lo,
                hi,
                tick,
                val_str,
                val_col: theme.value,
            },
            bw,
            theme,
        ));
        out.push(Line::raw(""));
    }

    // No target, no verdict on it: "at optimum" against a target that was
    // never offered would be a claim about nothing.
    let Some(targets) = targets else {
        out.push(Line::from(vec![
            Span::raw(" "),
            Span::styled("opt ", Style::default().fg(theme.label)),
            Span::styled("\u{2014} no target here", Style::default().fg(theme.label)),
        ]));
        return;
    };
    let at_opt = stages.iter().enumerate().all(|(i, spec)| {
        let value = current.get(i).copied().unwrap_or(spec.min_db);
        let target = targets.get(i).copied().unwrap_or(value);
        (value - target).abs() < 0.5
    });
    out.push(Line::from(vec![
        Span::raw(" "),
        Span::styled("opt ", Style::default().fg(theme.label)),
        if at_opt {
            Span::styled("\u{2713} at optimum", Style::default().fg(theme.status_ok))
        } else {
            Span::styled(
                stages
                    .iter()
                    .enumerate()
                    .map(|(i, spec)| {
                        format!(
                            "{} {:.0}",
                            spec.name,
                            targets.get(i).copied().unwrap_or(0.0)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" \u{00b7} "),
                Style::default().fg(theme.status_warn),
            )
        },
    ]));
}
