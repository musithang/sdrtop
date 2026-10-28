// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The column beside the square: the envelope, the scale, the key.
//!
//! **What this panel has that the rest of the bench does not**, and nothing
//! else. It used to carry its own EVM, MER, eccentricity, tilt and centroid:
//! the "EVM" was the ring's spread under a symbol-referenced name, and the
//! rest were IQ Diagnostics' gain, phase and DC measured a second time from
//! the 1024 points drawn here, beside the same quantities measured from every
//! sample, with two different numbers. The column says where those are.

use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};

use crate::ui::chrome::section;

use super::envelope::{Envelope, NOISE_SPREAD};

/// What the column reads besides the envelope.
pub(super) struct Scale<'a> {
    pub outer_db: f64,
    /// Where the radio stops the samples before its converter, when it
    /// does, and the part that clips or holds them there.
    pub clip: Option<(f64, &'a crate::hardware::Ceiling)>,
    pub points: usize,
}

/// Rows of the radial profile.
const PROFILE_ROWS: usize = 4;

/// Vertical ⅛-block ramp, 1 … 8 eighths.
const EIGHTHS: [char; 8] = [
    '\u{2581}', '\u{2582}', '\u{2583}', '\u{2584}', '\u{2585}', '\u{2586}', '\u{2587}', '\u{2588}',
];

/// The profile's width in a column `w` wide: a cell each side to spare.
pub(super) fn profile_bins(w: usize) -> usize {
    w.saturating_sub(2).max(1)
}

pub(super) fn lines(
    env: &Envelope,
    scale: &Scale,
    w: usize,
    theme: &crate::Theme,
) -> Vec<Line<'static>> {
    let dim = theme.border_dim;
    let mut out = vec![section("Envelope", "|z|", w, theme)];

    // |z| from the centre out to the outer ring, as tall as its share.
    let top = env.profile.iter().copied().max().unwrap_or(0).max(1) as f64;
    for row in 0..PROFILE_ROWS {
        let floor = (PROFILE_ROWS - 1 - row) * 8;
        let bar: String = env
            .profile
            .iter()
            .map(|&c| {
                let mut e = (c as f64 / top * (PROFILE_ROWS * 8) as f64).round() as usize;
                if c > 0 {
                    e = e.max(1);
                }
                match e.saturating_sub(floor).min(8) {
                    0 => ' ',
                    k => EIGHTHS[k - 1],
                }
            })
            .collect();
        out.push(Line::from(vec![
            Span::raw(" "),
            Span::styled(bar, Style::default().fg(theme.value_hi)),
        ]));
    }
    let edge = format!("{:.0}", scale.outer_db + 0.0);
    let bins = env.profile.len();
    out.push(Line::from(Span::styled(
        format!(" 0{edge:>w$}", w = bins.saturating_sub(1)),
        Style::default().fg(dim),
    )));
    out.push(row(
        "rms",
        format!("{:.0} dBFS", env.rms_dbfs),
        theme.value,
        "",
        theme,
    ));
    out.push(row(
        "spread",
        format!("{:.1} %", env.spread * 100.0),
        theme.value_hi,
        " rms",
        theme,
    ));
    out.push(Line::from(Span::styled(
        format!(" a ring 0 %, noise {:.0} %", NOISE_SPREAD * 100.0),
        Style::default().fg(dim),
    )));
    out.push(Line::raw(""));

    out.push(section("Scale", "", w, theme));
    out.push(row(
        "outer",
        format!("{:.0} dBFS", scale.outer_db),
        theme.value,
        "",
        theme,
    ));
    out.push(row(
        "rings",
        "6 dB".to_string(),
        theme.value,
        " apart",
        theme,
    ));
    out.push(if scale.outer_db >= 0.0 {
        Line::from(Span::styled(
            " \u{2500} full scale",
            Style::default().fg(theme.status_crit),
        ))
    } else if let Some((at, c)) = scale.clip.filter(|&(at, _)| at < scale.outer_db) {
        Line::from(Span::styled(
            format!(" \u{2504} {} {} {at:.0}", c.part, c.kind.verb()),
            Style::default().fg(theme.status_crit),
        ))
    } else {
        Line::from(Span::styled(
            format!(" full scale {:.0} dB out", -scale.outer_db),
            Style::default().fg(dim),
        ))
    });
    out.push(Line::raw(""));

    out.push(Line::from(vec![
        Span::styled(" \u{25ef} ", Style::default().fg(theme.border_focused)),
        Span::styled("rms fit  ", Style::default().fg(dim)),
        Span::styled("+ ", Style::default().fg(theme.status_warn)),
        Span::styled("DC", Style::default().fg(dim)),
    ]));
    out.push(Line::from(Span::styled(
        format!(" {} points drawn", scale.points),
        Style::default().fg(dim),
    )));
    out.push(Line::from(Span::styled(
        " gain, phase, DC \u{2190} left",
        Style::default().fg(dim),
    )));
    out
}

fn row(label: &str, value: String, col: Color, rest: &str, theme: &crate::Theme) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!(" {label:<7}"), Style::default().fg(theme.label)),
        Span::styled(value, Style::default().fg(col)),
        Span::styled(rest.to_string(), Style::default().fg(theme.border_dim)),
    ])
}
