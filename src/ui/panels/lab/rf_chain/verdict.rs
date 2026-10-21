// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The verdict: three lines of plain language about the gain staging, the action
//! chips under them, and the status foot.
//!
//! The copy is chosen by the same `staging_verdict` word the ADC column prints,
//! so the two halves of the bench never disagree about what the radio is doing.

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

/// The three body lines for a verdict word. Split out from the drawing so the
/// wording is one table rather than a `match` buried in a render.
///
/// `_` is `WELL-STAGED`: it is the fallthrough because it is the case where
/// nothing needs saying about a fault, and a new verdict word appearing without
/// its own copy should read as "fine" rather than panic.
/// The verdict in three lines. `front` is the front stage's name and `trim`
/// the stage to take a few dB off at the end of the chain (the second stage
/// where the radio has one), so the advice names controls the radio has.
fn copy(
    word: &str,
    adc_peak: f64,
    headroom: f64,
    snr: f64,
    front: &str,
    trim: &str,
) -> [String; 3] {
    match word {
        "CLIPPING" => [
            format!("Signal at {adc_peak:.0} dBFS \u{2014} ADC saturating, waveform distorted."),
            format!("Reduce {front} or {trim} to restore headroom."),
            "SNR is lost wherever the ADC clips.".to_string(),
        ],
        "HOT" => [
            format!("Signal at {adc_peak:.0} dBFS \u{2014} {headroom:.0} dB from the rail."),
            "SNR intact but peaks may saturate.".to_string(),
            format!("Trim {trim} by 2\u{2013}4 dB to add margin."),
        ],
        "UNDER-UTILISED" => [
            format!("Signal at {adc_peak:.0} dBFS \u{2014} {headroom:.0} dB headroom,"),
            format!("ADC window under-used. Increase {front} gain"),
            "to lift the signal into the optimal zone.".to_string(),
        ],
        "WEAK" => [
            format!("Signal at {adc_peak:.0} dBFS \u{2014} ADC severely under-driven."),
            format!("Increase {front}; if maxed, check the antenna"),
            "or reduce the system noise figure.".to_string(),
        ],
        _ => [
            // WELL-STAGED
            format!("Signal at {adc_peak:.0} dBFS \u{2014} {headroom:.0} dB headroom,"),
            format!("{snr:.0} dB above the noise floor. SNR set at"),
            "the front end is fully preserved.".to_string(),
        ],
    }
}

/// What the verdict block needs beyond the theme.
pub(super) struct Verdict<'a> {
    pub word: &'a str,
    pub sev: u8,
    pub sev_col: Color,
    pub adc_peak: f64,
    pub snr: f64,
    pub adc_rms: f32,
    pub amp: bool,
    pub tracking: bool,
    /// The front stage's name, the second key's stage when there is one, and
    /// the boost's name when there is one.
    pub front: String,
    pub second: Option<String>,
    pub boost: Option<String>,
}

pub(super) fn draw(out: &mut Vec<Line<'static>>, v: &Verdict<'_>, theme: &crate::Theme) {
    let dim = theme.border_dim;
    let lbl = Style::default().fg(theme.label);
    let headroom = -v.adc_peak;

    let title_mark = if v.sev == 0 {
        "\u{2713}"
    } else if v.sev == 2 {
        "\u{26a0}"
    } else {
        "\u{00b7}"
    };
    out.push(Line::from(vec![
        Span::raw(" "),
        Span::styled(
            format!("{title_mark} GAIN {}", v.word),
            Style::default().fg(v.sev_col).add_modifier(Modifier::BOLD),
        ),
    ]));
    let trim = v.second.as_deref().unwrap_or(&v.front);
    for line in copy(v.word, v.adc_peak, headroom, v.snr, &v.front, trim) {
        out.push(Line::from(vec![Span::raw(" "), Span::styled(line, lbl)]));
    }

    // Action chips: auto-gain, and the radio's own gain keys by the names its
    // gain model gives them. Then the status foot.
    let chip = |label: &str, active: bool| -> Span<'static> {
        let bg = if active {
            theme.value_hi
        } else {
            theme.border_dim
        };
        Span::styled(
            format!(" {label} "),
            Style::default().bg(bg).fg(Color::Rgb(4, 6, 15)),
        )
    };
    let mut chips = vec![
        Span::raw(" "),
        chip("A auto-gain", v.tracking),
        Span::raw(" "),
        chip(&format!("\u{2191}\u{2193} {}", v.front), false),
    ];
    if let Some(second) = &v.second {
        chips.push(Span::raw(" "));
        chips.push(chip(&format!("[ ] {second}"), false));
    }
    out.push(Line::from(chips));
    let limited = if v.adc_rms > -50.0 {
        "analog-noise limited"
    } else {
        "quantisation limited"
    };
    // The boost by its own name, and nothing where the radio has none.
    let boost_txt = v.boost.as_deref().map(|name| {
        let state = if v.amp { "on" } else { "bypass" };
        format!("{name} {state} \u{00b7} ")
    });
    let ag_txt = if v.tracking {
        "auto-gain \u{2713} tracking"
    } else {
        "auto-gain idle"
    };
    out.push(Line::from(vec![
        Span::raw(" "),
        Span::styled(
            format!(
                "{}{ag_txt} \u{00b7} {limited}",
                boost_txt.unwrap_or_default()
            ),
            Style::default().fg(dim),
        ),
    ]));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::rf_calc::staging_verdict;

    #[test]
    fn every_verdict_staging_verdict_can_return_has_its_own_copy() {
        // Sweep the whole dBFS range the ADC can report and collect the words
        // `staging_verdict` actually produces, so a new one added there without copy
        // here fails this test instead of silently falling through to WELL-STAGED.
        let mut words: Vec<&str> = Vec::new();
        for db in -120..=0 {
            let (w, _) = staging_verdict(db as f64);
            if !words.contains(&w) {
                words.push(w);
            }
        }
        assert!(
            words.len() >= 4,
            "expected several distinct verdicts, got {words:?}"
        );
        let well_staged = copy("WELL-STAGED", -8.0, 8.0, 40.0, "LNA", "VGA");
        for w in &words {
            if *w == "WELL-STAGED" {
                continue;
            }
            assert_ne!(
                copy(w, -8.0, 8.0, 40.0, "LNA", "VGA"),
                well_staged,
                "{w} has no copy of its own and falls through to WELL-STAGED"
            );
        }
    }

    #[test]
    fn the_copy_quotes_the_measured_peak() {
        // Every branch leads with the number it is a verdict about; a body that
        // does not name the level reads as generic advice.
        for w in ["CLIPPING", "HOT", "UNDER-UTILISED", "WEAK", "WELL-STAGED"] {
            let c = copy(w, -3.0, 3.0, 22.0, "LNA", "VGA");
            assert!(c[0].contains("-3 dBFS"), "{w}: {:?}", c[0]);
        }
    }

    #[test]
    fn an_unknown_verdict_reads_as_well_staged_rather_than_panicking() {
        assert_eq!(
            copy("SOMETHING NEW", -8.0, 8.0, 40.0, "LNA", "VGA"),
            copy("WELL-STAGED", -8.0, 8.0, 40.0, "LNA", "VGA")
        );
    }
}
