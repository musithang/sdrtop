// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! DC AT THE LO section: how strong the offset is, and how strong against
//! the signal.
//!
//! **Against the signal is the reading that matters.** A DC offset is a tone
//! at 0 Hz, and the same offset is a small spike under a strong signal and
//! the loudest thing in the capture under a weak one: a HackRF on FM at a
//! modest gain carried its DC 12 dB over everything it received. The section
//! used to show the offset four ways in full-scale units (I, Q, their
//! magnitude and the spike) and never that one; now it is the level, where it
//! sits against the signal's rms on a scale, and the I and Q split.

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::ui::chrome::section;

use super::reading::Reading;
use super::rows::{Rows, LEAD};
use super::severity::{dc_rel_color, spike_color, DC_REL_CRIT_DB, DC_REL_WARN_DB};

/// The scale's ends, dB against the signal's rms.
const SCALE_LO_DB: f64 = -60.0;
const SCALE_HI_DB: f64 = 20.0;

pub(super) fn lines(r: &Reading, rows: &Rows) -> Vec<Line<'static>> {
    let theme = rows.theme;
    let dim = rows.dim();
    let lead = |label: &str| {
        vec![
            Span::raw(" "),
            Span::styled(format!("{label:<5} "), rows.label_style()),
        ]
    };
    // On a radio whose LO is not at 0 Hz the DC is no longer its leak: the
    // section says where it measures, and the LO gets a line of its own.
    let mut out = if r.lo_offset_hz == 0 {
        vec![section("DC at the LO", "a tone at 0 Hz", rows.iw, theme)]
    } else {
        vec![section(
            "DC at 0 Hz",
            "the LO is off the centre",
            rows.iw,
            theme,
        )]
    };

    let mut level = lead("level");
    match (r.spike_dbfs, r.dc_over_signal_db) {
        (Some(s), Some(over)) => {
            level.push(Span::styled(
                format!("{s:.0} dBFS"),
                Style::default()
                    .fg(spike_color(s, theme))
                    .add_modifier(Modifier::BOLD),
            ));
            level.push(Span::styled(
                if over >= 0.0 {
                    format!("   {over:.0} dB over the signal")
                } else {
                    format!("   {:.0} dB under the signal", -over)
                },
                Style::default().fg(dc_rel_color(over, theme)),
            ));
        }
        _ => level.push(Span::styled("none", Style::default().fg(theme.status_ok))),
    }
    out.push(Line::from(level));
    out.push(Line::raw(""));

    // Where the DC sits against the signal: green well under it, amber
    // closing in, red within 10 dB or over.
    let w = rows.iw.saturating_sub(LEAD + 1).max(12);
    let x_of = |db: f64| {
        (((db - SCALE_LO_DB) / (SCALE_HI_DB - SCALE_LO_DB)).clamp(0.0, 1.0) * (w - 1) as f64)
            .round() as usize
    };
    let mut track: Vec<(char, ratatui::style::Color)> = (0..w)
        .map(|x| {
            let col = if x >= x_of(DC_REL_CRIT_DB) {
                theme.status_crit
            } else if x >= x_of(DC_REL_WARN_DB) {
                theme.status_warn
            } else {
                theme.status_ok
            };
            ('\u{2500}', col)
        })
        .collect();
    track[x_of(0.0)] = ('\u{253c}', theme.label);
    if let Some(over) = r.dc_over_signal_db {
        track[x_of(over)] = ('\u{25cf}', dc_rel_color(over, theme));
    }
    let mut scale = vec![Span::raw(" ".repeat(LEAD))];
    scale.extend(
        track
            .into_iter()
            .map(|(c, col)| Span::styled(c.to_string(), Style::default().fg(col))),
    );
    out.push(Line::from(scale));
    let mut axis = vec![' '; w];
    let mut put = |x: usize, s: &str| {
        let at = x.min(w.saturating_sub(s.len()));
        for (k, c) in s.chars().enumerate() {
            axis[at + k] = c;
        }
    };
    put(0, &format!("{SCALE_LO_DB:.0}"));
    put(
        x_of(DC_REL_WARN_DB).saturating_sub(1),
        &format!("{DC_REL_WARN_DB:.0}"),
    );
    put(x_of(0.0).saturating_sub(1), "rms");
    put(w, &format!("+{SCALE_HI_DB:.0}"));
    out.push(Line::from(vec![
        Span::raw(" ".repeat(LEAD)),
        Span::styled(
            axis.into_iter().collect::<String>(),
            Style::default().fg(dim),
        ),
    ]));
    out.push(Line::raw(""));

    let mut iq = lead("I · Q");
    iq.push(Span::styled(
        format!(
            "{:+.2} % \u{00b7} {:+.2} % of full scale",
            r.dc_i * 100.0,
            r.dc_q * 100.0
        ),
        Style::default().fg(theme.value),
    ));
    out.push(Line::from(iq));
    if r.lo_offset_hz != 0 {
        let at = format!("{:+.1} kHz", r.lo_offset_hz as f64 / 1e3);
        let mut lo = lead("LO");
        match r.lo_line_dbfs {
            Some(db) => {
                lo.push(Span::styled(
                    format!("{db:.0} dBFS/bin"),
                    Style::default().fg(spike_color(db, theme)),
                ));
                lo.push(Span::styled(
                    format!("   at {at}"),
                    Style::default().fg(dim),
                ));
            }
            None => lo.push(Span::styled(
                format!("at {at}, not in this span"),
                Style::default().fg(dim),
            )),
        }
        out.push(Line::from(lo));
    }
    out
}
