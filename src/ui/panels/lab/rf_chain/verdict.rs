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
/// `clipper` is the part that stops the samples before the converter, on a
/// radio whose ceiling is not the converter's.
/// `digital` is a radio whose stages are digital gain after the converter:
/// there they only place the level in the samples, so the advice says that
/// and promises nothing for the SNR, which the radio's own AGCs set.
#[allow(clippy::too_many_arguments)]
fn copy(
    word: &str,
    adc_peak: f64,
    headroom: f64,
    snr: f64,
    front: Control<'_>,
    trim: Control<'_>,
    clipper: Option<&crate::hardware::Ceiling>,
    digital: bool,
) -> [String; 3] {
    // "Reduce ATT" could mean either way on an attenuator, so the advice says
    // which: more of it to bring the level down, less to bring it up.
    let lower = if front.attenuates == trim.attenuates {
        let verb = if front.attenuates { "More" } else { "Reduce" };
        format!("{verb} {} or {}", front.name, trim.name)
    } else {
        format!("{} or {}", front.lower(true), trim.lower(false))
    };
    if digital {
        if let Some(lines) = placement_copy(word, adc_peak, headroom, front, trim, &lower) {
            return lines;
        }
    }
    let (front, trim_name) = (front, trim.name);
    let (saturating, rail) = match clipper {
        Some(c) => (
            format!("the {} clipping, before the ADC", c.part),
            format!("the {}'s {}", c.part, c.kind.noun()),
        ),
        None => (
            "ADC saturating, waveform distorted".to_string(),
            "the rail".to_string(),
        ),
    };
    let holder = clipper.map_or("the radio", |c| c.part);
    match word {
        "HELD" => [
            format!("Signal at {adc_peak:.0} dBFS \u{2014} the {holder} holding it there."),
            format!("{lower} to bring it under the hold:"),
            "past it the level is not the antenna's.".to_string(),
        ],
        "CLIPPING" => [
            format!("Signal at {adc_peak:.0} dBFS \u{2014} {saturating}."),
            format!("{lower} to restore headroom."),
            "SNR is lost wherever the signal clips.".to_string(),
        ],
        "HOT" => [
            format!("Signal at {adc_peak:.0} dBFS \u{2014} {headroom:.0} dB from {rail}."),
            "SNR intact but peaks may saturate.".to_string(),
            format!("Trim {trim_name} by 2\u{2013}4 dB to add margin."),
        ],
        "UNDER-UTILISED" => [
            format!("Signal at {adc_peak:.0} dBFS \u{2014} {headroom:.0} dB headroom,"),
            if front.attenuates {
                format!("ADC window under-used. Less {}", front.name)
            } else {
                format!("ADC window under-used. Increase {} gain", front.name)
            },
            "to lift the signal into the optimal zone.".to_string(),
        ],
        "WEAK" => [
            format!("Signal at {adc_peak:.0} dBFS \u{2014} ADC severely under-driven."),
            if front.attenuates {
                format!("Less {}; at 0 dB, check the antenna", front.name)
            } else {
                format!("Increase {}; if maxed, check the antenna", front.name)
            },
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

/// The verdict for digital stages, where the gain only places the level in
/// the samples; `None` for a word whose copy says nothing about the SNR
/// already (`HELD`, `CLIPPING`).
fn placement_copy(
    word: &str,
    adc_peak: f64,
    headroom: f64,
    front: Control<'_>,
    trim: Control<'_>,
    lower: &str,
) -> Option<[String; 3]> {
    let raise = if front.attenuates == trim.attenuates {
        let verb = if front.attenuates { "less" } else { "more" };
        format!("{verb} {} or {}", front.name, trim.name)
    } else {
        let one =
            |c: Control<'_>| format!("{} {}", if c.attenuates { "less" } else { "more" }, c.name);
        format!("{} or {}", one(front), one(trim))
    };
    let head = format!("Signal at {adc_peak:.0} dBFS \u{2014} {headroom:.0} dB headroom,");
    Some(match word {
        "UNDER-UTILISED" | "WEAK" => [
            head,
            format!("low in the word: {raise} lifts it;"),
            "the radio's AGCs set the SNR, not these.".to_string(),
        ],
        "HOT" => [
            format!("Signal at {adc_peak:.0} dBFS \u{2014} {headroom:.0} dB from the hold."),
            "Near it the radio cuts its own gain:".to_string(),
            format!("{lower} keeps it clear."),
        ],
        "WELL-STAGED" => [
            head,
            "well placed. ATT and LNA place it in the".to_string(),
            "word only; the radio's AGCs set the SNR.".to_string(),
        ],
        _ => return None,
    })
}

/// The verdict word as shown: on digital stages a placement, not a staging
/// of the front end.
fn shown_word(word: &str, digital: bool) -> &str {
    match (word, digital) {
        ("WELL-STAGED", true) => "WELL PLACED",
        ("UNDER-UTILISED" | "WEAK", true) => "LOW IN THE WORD",
        _ => word,
    }
}

/// A gain control the advice names: its name, and whether it is an
/// attenuator (every setting at or under 0 dB), where "more" is less level.
#[derive(Clone, Copy)]
pub(super) struct Control<'a> {
    pub name: &'a str,
    pub attenuates: bool,
}

impl<'a> Control<'a> {
    /// A control that adds gain.
    #[cfg(test)]
    fn gain(name: &'a str) -> Self {
        Self {
            name,
            attenuates: false,
        }
    }

    /// The words that take the level down with it: "more ATT", "reduce LNA",
    /// capitalised to start a sentence.
    fn lower(self, first: bool) -> String {
        let verb = match (self.attenuates, first) {
            (true, true) => "More",
            (true, false) => "more",
            (false, true) => "Reduce",
            (false, false) => "reduce",
        };
        format!("{verb} {}", self.name)
    }
}

/// What the verdict block needs beyond the theme.
pub(super) struct Verdict<'a> {
    pub word: &'a str,
    pub sev: u8,
    pub sev_col: Color,
    pub adc_peak: f64,
    /// The radio's ceiling, dBFS, and the part that clips or holds there when
    /// it is not the converter: the headroom and the words are about it.
    pub ceiling: f64,
    pub clipper: Option<&'a crate::hardware::Ceiling>,
    pub snr: f64,
    pub adc_rms: f32,
    pub amp: bool,
    pub tracking: bool,
    /// The front stage's name, the second key's stage when there is one, and
    /// the boost's name when there is one.
    pub front: String,
    pub second: Option<String>,
    pub boost: Option<String>,
    /// Whether the front stage and the second are attenuators.
    pub front_attenuates: bool,
    pub second_attenuates: bool,
    /// Whether the stages are digital gain after the converter.
    pub digital: bool,
}

pub(super) fn draw(out: &mut Vec<Line<'static>>, v: &Verdict<'_>, theme: &crate::Theme) {
    let dim = theme.border_dim;
    let lbl = Style::default().fg(theme.label);
    // As every other panel shows it, so the figure equals the difference of
    // the two levels printed beside it.
    let headroom = crate::ui::rf_calc::shown_headroom_db(v.ceiling, v.adc_peak);

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
            format!(
                "{title_mark} {} {}",
                if v.digital { "LEVEL" } else { "GAIN" },
                shown_word(v.word, v.digital)
            ),
            Style::default().fg(v.sev_col).add_modifier(Modifier::BOLD),
        ),
    ]));
    let front = Control {
        name: &v.front,
        attenuates: v.front_attenuates,
    };
    let trim = match v.second.as_deref() {
        Some(name) => Control {
            name,
            attenuates: v.second_attenuates,
        },
        None => front,
    };
    for line in copy(
        v.word, v.adc_peak, headroom, v.snr, front, trim, v.clipper, v.digital,
    ) {
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
    // Behind digital stages the noise is the radio's, set before the gain
    // the keys move; which kind limits it is not seen from here.
    let limited = if v.digital {
        "SNR set in the radio"
    } else if v.adc_rms > -50.0 {
        "analog-noise limited"
    } else {
        "quantisation limited"
    };
    // The boost by its own name, and nothing where the radio has none. On
    // digital stages the radio's AGCs are always on, and the switch only
    // says who sets the front stage.
    let boost_txt = v.boost.as_deref().map(|name| {
        if v.digital {
            if v.amp {
                format!("{name} sets {} \u{00b7} ", v.front)
            } else {
                format!("{} by hand \u{00b7} ", v.front)
            }
        } else {
            let state = if v.amp { "on" } else { "bypass" };
            format!("{name} {state} \u{00b7} ")
        }
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
    use crate::ui::rf_calc::staging_verdict_for;

    #[test]
    fn every_verdict_staging_verdict_can_return_has_its_own_copy() {
        // Sweep the whole dBFS range the ADC can report and collect the words
        // `staging_verdict` actually produces, so a new one added there without copy
        // here fails this test instead of silently falling through to WELL-STAGED.
        let mut words: Vec<&str> = Vec::new();
        for db in -120..=0 {
            for kind in [None, Some(crate::hardware::CeilingKind::Holds)] {
                let (w, _) = staging_verdict_for(db as f64, kind);
                if !words.contains(&w) {
                    words.push(w);
                }
            }
        }
        assert!(
            words.len() >= 4,
            "expected several distinct verdicts, got {words:?}"
        );
        let well_staged = copy(
            "WELL-STAGED",
            -8.0,
            8.0,
            40.0,
            Control::gain("LNA"),
            Control::gain("VGA"),
            None,
            false,
        );
        for w in &words {
            if *w == "WELL-STAGED" {
                continue;
            }
            assert_ne!(
                copy(
                    w,
                    -8.0,
                    8.0,
                    40.0,
                    Control::gain("LNA"),
                    Control::gain("VGA"),
                    None,
                    false
                ),
                well_staged,
                "{w} has no copy of its own and falls through to WELL-STAGED"
            );
        }
    }

    #[test]
    fn the_copy_quotes_the_measured_peak() {
        // Every branch leads with the number it is a verdict about; a body that
        // does not name the level reads as generic advice.
        for w in [
            "CLIPPING",
            "HELD",
            "HOT",
            "UNDER-UTILISED",
            "WEAK",
            "WELL-STAGED",
        ] {
            let c = copy(
                w,
                -3.0,
                3.0,
                22.0,
                Control::gain("LNA"),
                Control::gain("VGA"),
                None,
                false,
            );
            assert!(c[0].contains("-3 dBFS"), "{w}: {:?}", c[0]);
        }
    }

    #[test]
    fn an_unknown_verdict_reads_as_well_staged_rather_than_panicking() {
        assert_eq!(
            copy(
                "SOMETHING NEW",
                -8.0,
                8.0,
                40.0,
                Control::gain("LNA"),
                Control::gain("VGA"),
                None,
                false
            ),
            copy(
                "WELL-STAGED",
                -8.0,
                8.0,
                40.0,
                Control::gain("LNA"),
                Control::gain("VGA"),
                None,
                false
            )
        );
    }
}
