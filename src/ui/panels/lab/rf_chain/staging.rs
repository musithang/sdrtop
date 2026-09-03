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

use crate::ui::chrome::section;
use crate::ui::rf_calc::{Stage, StageLevel};

use super::super::rf_bench::{bar_row, bar_width, row, Bar, Row};
use super::{LABEL_W, VALW};

/// `GAIN LINEUP` - the modeled level at each node of the chain, ending at the
/// ADC read in dBFS.
pub(super) fn lineup(
    out: &mut Vec<Line<'static>>,
    levels: &[StageLevel],
    stages: &[Stage],
    adc_peak: f64,
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
    let label_w = lineup_label_w(levels);

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
        .map(|n| format!("{:.0} dBm", n.signal_dbm).chars().count())
        .chain(std::iter::once(
            format!("{adc_peak:.0} dBFS").chars().count(),
        ))
        .max()
        .unwrap_or(0);
    let show_mid = 1 + label_w + 1 + widest_mid + 1 + widest_right <= iw;

    for (i, node) in levels.iter().enumerate() {
        out.push(row(
            Row {
                label: &clip(&node.label, label_w),
                label_w,
                mid: if show_mid {
                    mid_text(i, stages)
                } else {
                    String::new()
                },
                mid_col: dim,
                right: format!("{:.0} dBm", node.signal_dbm),
                right_col: theme.value,
            },
            iw,
            theme,
        ));
        out.push(Line::raw(""));
    }
    // ADC node = the last stage's output, read in dBFS.
    out.push(row(
        Row {
            label: "ADC",
            label_w,
            mid: if show_mid {
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
        Some(stage) => format!("{:+} dB", stage.gain_db as i64),
        None => "\u{2014}".to_string(),
    }
}

/// `GAIN STAGING` - LNA and VGA against their optimal targets, with the target
/// spelled out underneath when they are not already there.
pub(super) fn staging(
    out: &mut Vec<Line<'static>>,
    lna: u32,
    vga: u32,
    lna_opt: u32,
    vga_opt: u32,
    iw: usize,
    theme: &crate::Theme,
) {
    let bw = bar_width(iw, LABEL_W, VALW);
    out.push(section(
        "Gain staging",
        "\u{2502} = optimal target",
        iw,
        theme,
    ));
    out.push(Line::raw(""));
    out.push(bar_row(
        Bar {
            label: "LNA",
            label_w: LABEL_W,
            value: lna,
            max: 40,
            lo: theme.status_ok,
            hi: theme.value_hi,
            tick: Some(lna_opt as f64 / 40.0),
            val_str: format!("{lna} / 40 dB"),
            val_col: theme.value,
        },
        bw,
        theme,
    ));
    out.push(Line::raw(""));
    out.push(bar_row(
        Bar {
            label: "VGA",
            label_w: LABEL_W,
            value: vga,
            max: 62,
            lo: theme.border_accent,
            hi: theme.status_warn,
            tick: Some(vga_opt as f64 / 62.0),
            val_str: format!("{vga} / 62 dB"),
            val_col: theme.value,
        },
        bw,
        theme,
    ));
    out.push(Line::raw(""));
    let at_opt = lna == lna_opt && vga == vga_opt;
    out.push(Line::from(vec![
        Span::raw(" "),
        Span::styled("opt ", Style::default().fg(theme.label)),
        if at_opt {
            Span::styled("\u{2713} at optimum", Style::default().fg(theme.status_ok))
        } else {
            Span::styled(
                format!("\u{2192} LNA {lna_opt} \u{00b7} VGA {vga_opt}"),
                Style::default().fg(theme.status_warn),
            )
        },
    ]));
}
