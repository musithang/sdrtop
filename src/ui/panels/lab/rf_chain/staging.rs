//! `GAIN LINEUP` and `GAIN STAGING` — the signal's level after each stage, and
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

/// `GAIN LINEUP` — the modeled level at each node of the chain, ending at the
/// ADC read in dBFS.
pub(super) fn lineup(
    out: &mut Vec<Line<'static>>, levels: &[StageLevel], stages: &[Stage],
    adc_peak: f64, sev_col: Color, iw: usize, theme: &crate::Theme,
) {
    let dim = theme.border_dim;
    out.push(section("Gain lineup", "level after each stage", iw, theme));
    out.push(Line::raw(""));
    for (i, node) in levels.iter().enumerate() {
        let gain_str = if i == 0 { "\u{2014}".to_string() }
                       else { format!("{:+} dB", stages[i - 1].gain_db as i64) };
        out.push(row(Row {
            label: node.label, label_w: LABEL_W, mid: gain_str, mid_col: dim,
            right: format!("{:.0} dBm", node.signal_dbm), right_col: theme.value,
        }, iw, theme));
        out.push(Line::raw(""));
    }
    // ADC node = VGA output, read in dBFS.
    out.push(row(Row {
        label: "ADC", label_w: LABEL_W, mid: "0 dB".to_string(), mid_col: dim,
        right: format!("{adc_peak:.0} dBFS"), right_col: sev_col,
    }, iw, theme));
}

/// `GAIN STAGING` — LNA and VGA against their optimal targets, with the target
/// spelled out underneath when they are not already there.
pub(super) fn staging(
    out: &mut Vec<Line<'static>>, lna: u32, vga: u32, lna_opt: u32, vga_opt: u32,
    iw: usize, theme: &crate::Theme,
) {
    let bw = bar_width(iw, LABEL_W, VALW);
    out.push(section("Gain staging", "\u{2502} = optimal target", iw, theme));
    out.push(Line::raw(""));
    out.push(bar_row(Bar {
        label: "LNA", label_w: LABEL_W, value: lna, max: 40,
        lo: theme.status_ok, hi: theme.value_hi,
        tick: Some(lna_opt as f64 / 40.0),
        val_str: format!("{lna} / 40 dB"), val_col: theme.value,
    }, bw, theme));
    out.push(Line::raw(""));
    out.push(bar_row(Bar {
        label: "VGA", label_w: LABEL_W, value: vga, max: 62,
        lo: theme.border_accent, hi: theme.status_warn,
        tick: Some(vga_opt as f64 / 62.0),
        val_str: format!("{vga} / 62 dB"), val_col: theme.value,
    }, bw, theme));
    out.push(Line::raw(""));
    let at_opt = lna == lna_opt && vga == vga_opt;
    out.push(Line::from(vec![
        Span::raw(" "),
        Span::styled("opt ", Style::default().fg(theme.label)),
        if at_opt {
            Span::styled("\u{2713} at optimum", Style::default().fg(theme.status_ok))
        } else {
            Span::styled(format!("\u{2192} LNA {lna_opt} \u{00b7} VGA {vga_opt}"),
                         Style::default().fg(theme.status_warn))
        },
    ]));
}
