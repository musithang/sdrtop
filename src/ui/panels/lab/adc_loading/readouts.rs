// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The read-out under the ladder: the verdict and the loading, where the
//! radio clips before its converter, and the modeled linearity card.
//!
//! One module because they are one stack of fixed-height blocks, all reading
//! the same `adc_loading` / `linearity` model.

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use crate::ui::chrome::section;
use crate::ui::rf_calc::{linearity, AdcLoading};

use super::super::rf_bench::{row, Row};

/// Label column for the linearity card's rows: clears `P1dB`, `IIP3`, `SFDR`.
const LABEL_W: usize = 4;

/// Where the radio stops its samples before its converter, and that the
/// clip count above cannot see it: neither a clip nor a hold there touches a
/// converter rail.
pub(super) fn ceiling(
    out: &mut Vec<Line<'static>>,
    level: &str,
    c: &crate::hardware::Ceiling,
    theme: &crate::Theme,
) {
    // Short enough for the bench at a laptop's width, where a longer line
    // was cut live; wider still, it is cut at the panel's edge.
    let part = c.part;
    let text = match c.kind {
        crate::hardware::CeilingKind::Clips => {
            format!(" {part} clips at {level} dBFS, before the ADC: uncounted")
        }
        crate::hardware::CeilingKind::Holds => {
            format!(" {part} holds peaks {level} dBFS: gain cut unseen")
        }
    };
    out.push(Line::from(Span::styled(
        text,
        Style::default().fg(theme.label),
    )));
}

/// The staging verdict and how far the peak sits from where this radio
/// clips, then the loading in one column of labels and one of values: peak,
/// rms, the bits the peak reaches, and the clips counted at the rails.
pub(super) fn loading(
    out: &mut Vec<Line<'static>>,
    load: &AdcLoading,
    ceiling: f64,
    part: Option<&crate::hardware::Ceiling>,
    verdict: &str,
    sev_col: Color,
    theme: &crate::Theme,
) {
    let dim = theme.border_dim;
    // Between the two figures as shown, rounded as `{:.0}` rounds them, so
    // -3 and a peak printed as -10 are 7 dB apart on screen as well, never 8.
    // `+ 0.0` turns a peak exactly at the ceiling into 0 dB, not -0 dB.
    let headroom = crate::ui::rf_calc::shown_headroom_db(ceiling, load.peak_dbfs);
    let whose = match part {
        Some(c) => format!("the {}'s {}", c.part, c.kind.noun()),
        None => "full scale".to_string(),
    };
    out.push(Line::from(vec![
        Span::raw(" "),
        Span::styled(
            verdict.to_string(),
            Style::default().fg(sev_col).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            if headroom == 0.0 {
                format!("  at {whose}")
            } else if headroom > 0.0 {
                format!("  {headroom:.0} dB under {whose}")
            } else {
                format!("  {:.0} dB past {whose}", -headroom)
            },
            Style::default().fg(theme.value),
        ),
    ]));
    let row = |label: &str, value: String, col: Color, rest: String| {
        Line::from(vec![
            Span::styled(format!(" {label:<6}"), Style::default().fg(theme.label)),
            Span::styled(format!("{value:<11}"), Style::default().fg(col)),
            Span::styled(rest, Style::default().fg(dim)),
        ])
    };
    out.push(row(
        "peak",
        format!("{:.0} dBFS", load.peak_dbfs),
        sev_col,
        format!("{} of {} counts", load.peak_counts, load.full_scale_counts),
    ));
    out.push(row(
        "rms",
        format!("{:.0} dBFS", load.rms_dbfs),
        theme.value,
        format!("per axis, crest {:.1} dB", load.crest_db),
    ));
    out.push(row(
        "bits",
        format!("{:.1} of {}", load.bits_reached, load.bits),
        theme.value_hi,
        "reached by the peak".to_string(),
    ));
    let (clips, clip_col) = if load.clip_events == 0 {
        ("none".to_string(), theme.status_ok)
    } else {
        (load.clip_events.to_string(), theme.status_crit)
    };
    let n = if load.n >= 1_000_000 {
        format!("{:.1}M", load.n as f64 / 1e6)
    } else if load.n >= 1000 {
        format!("{}k", load.n / 1000)
    } else {
        load.n.to_string()
    };
    out.push(row(
        "clips",
        clips,
        clip_col,
        format!("at the rails, of {n} samples"),
    ));
}

/// `LINEARITY` - modeled, not measured: P1dB headroom, IIP3 / IMD3 and SFDR
/// against the converter's own quantisation limit. Followed by the panel's
/// teaching caption.
pub(super) fn linearity_card(
    out: &mut Vec<Line<'static>>,
    lna_g: u32,
    vga_g: u32,
    bits: u8,
    iw: usize,
    theme: &crate::Theme,
) {
    let dim = theme.border_dim;
    let r = |label: &'static str, mid: String, mid_col: Color, right: String, right_col: Color| {
        row(
            Row {
                label,
                label_w: LABEL_W,
                mid,
                mid_col,
                right,
                right_col,
            },
            iw,
            theme,
        )
    };

    let lin = linearity(lna_g, vga_g, bits);
    out.push(section("Linearity", "modeled", iw, theme));
    out.push(r(
        "P1dB",
        format!("{:.0} dB hdrm", lin.p1db_headroom_db),
        theme.value,
        "compression".to_string(),
        dim,
    ));
    out.push(r(
        "IIP3",
        format!("{:+.0} dBm", lin.iip3_dbm),
        theme.value,
        format!("IMD3 {:.0} dBc", lin.imd3_dbc),
        theme.value,
    ));
    out.push(r(
        "SFDR",
        format!("{:.0} dB", lin.sfdr_db),
        theme.value_hi,
        format!("{bits}-bit \u{2264}{:.0}", lin.sfdr_limit_db),
        dim,
    ));

    // Teaching caption.
    out.push(Line::from(Span::styled(
        " fill the range without hitting the rails",
        Style::default().fg(dim),
    )));
}
