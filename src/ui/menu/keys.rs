// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The key reference, and the check that stops it drifting.
//!
//! The overlay this replaces was a hardcoded block of text that claimed
//! `[1] Preset: main` long after `[1]` had come to mean `command_rail`. Nothing
//! connected the two, so nothing noticed, and the help shipped wrong for
//! releases. The tests at the bottom of this file are that connection: they read
//! `input/global/mod.rs` as source text and refuse to let the two disagree.
//!
//! The trick is not new here. `builder/registry.rs` reads the dispatch table the
//! same way to prove every focusable panel has a handler. Presets are data and
//! the dispatch is code, so a text check is the only joint available.

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::hardware::GainModel;
use crate::ui::chrome;

/// How a row reads on a single-knob device: RTL-SDR has one stepped tuner gain
/// and an AGC where HackRF has LNA, VGA and an RF amp.
pub enum OnSingle {
    /// Same wording on both.
    Same,
    /// Different wording.
    Reword(&'static str),
}

/// What a device has to have for a key to do anything.
///
/// This used to be an `OnSingle::Hide`, which hid `[` and `]` because the device
/// was "single knob". That was the right answer for the wrong reason: they are
/// hidden because there is no second gain stage. A SoapySDR device has no second
/// stage either, and often no front end boost, and the reference has to be able
/// to say so without pretending it is an RTL-SDR.
pub enum Needs {
    Always,
    SecondStage,
    Boost,
}

impl Needs {
    fn met(&self, model: &GainModel) -> bool {
        match self {
            Needs::Always => true,
            Needs::SecondStage => model.has_second_stage(),
            Needs::Boost => model.has_boost(),
        }
    }
}

/// A global key and what it does.
pub struct Binding {
    pub key: &'static str,
    /// The character in `KeyCode::Char('x')` this row documents, for the check
    /// below. `None` for keys that are not `Char`, such as Esc, Tab and the
    /// arrows, and for the digit range, which is one arm covering nine keys.
    ///
    /// Read only by the checks at the bottom of this file, and that is the whole
    /// point of it: the pane draws `key` and `what`, while this field is what
    /// ties the row to a real match arm.
    #[cfg_attr(not(test), allow(dead_code))]
    pub ch: Option<char>,
    pub what: &'static str,
    pub single: OnSingle,
    /// What the device must have for this key to exist at all.
    pub needs: Needs,
}

const fn b(key: &'static str, ch: Option<char>, what: &'static str) -> Binding {
    Binding {
        key,
        ch,
        what,
        single: OnSingle::Same,
        needs: Needs::Always,
    }
}

/// Everything `input/global/mod.rs` claims, grouped the way that file groups its
/// match arms so the two read in the same order.
pub const GLOBAL: &[(&str, &[Binding])] = &[
    (
        "The radio",
        &[
            b("Space", Some(' '), "start or stop RX"),
            b("R", Some('r'), "reset everything to defaults"),
            b("F", Some('f'), "type a frequency"),
            b("S", Some('s'), "type a sample rate"),
        ],
    ),
    (
        "Gain",
        &[
            Binding {
                key: "\u{2191} \u{2193}",
                ch: None,
                what: "LNA gain, down and up",
                single: OnSingle::Reword("tuner gain, down and up, in steps"),
                needs: Needs::Always,
            },
            Binding {
                key: "[",
                ch: Some('['),
                what: "VGA gain down",
                single: OnSingle::Same,
                needs: Needs::SecondStage,
            },
            Binding {
                key: "]",
                ch: Some(']'),
                what: "VGA gain up",
                single: OnSingle::Same,
                needs: Needs::SecondStage,
            },
            Binding {
                key: "A",
                ch: Some('a'),
                what: "front end boost: the RF amp",
                single: OnSingle::Reword("front end boost: the tuner AGC"),
                needs: Needs::Boost,
            },
        ],
    ),
    (
        "The view",
        &[
            b("W", Some('w'), "pause or resume the waterfall"),
            b("H", Some('h'), "freeze a ghost trace, or clear it"),
            b("Tab", None, "show or hide the footer"),
        ],
    ),
    (
        "Layouts and session",
        &[
            b("1-9", None, "the nth layout of this section"),
            b("P", Some('p'), "next layout in this section"),
            b("Esc", None, "up one level, or open this menu"),
            b("Q", Some('q'), "quit, saving the config"),
        ],
    ),
];

/// The reference as lines, for the given device's gain model.
///
/// Separate from drawing so the scroll arithmetic and the row count can be
/// tested without a terminal.
fn lines(model: &GainModel, iw: usize, theme: &crate::Theme) -> Vec<Line<'static>> {
    let single = model.is_single();
    let key_style = Style::default()
        .fg(theme.border_accent)
        .add_modifier(Modifier::BOLD);
    let what_style = Style::default().fg(theme.value);

    let mut out = Vec::new();
    for (group, bindings) in GLOBAL {
        // Which of this group's keys this device actually has. Collected first
        // so a group that filters down to nothing takes its heading with it: a
        // section title over empty space reads as a rendering bug, not as
        // "your radio has none of these".
        let shown: Vec<&Binding> = bindings.iter().filter(|b| b.needs.met(model)).collect();
        if shown.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(Line::from(""));
        }
        out.push(chrome::section(group, "", iw, theme));
        for binding in shown {
            let what = match (single, &binding.single) {
                (true, OnSingle::Reword(text)) => *text,
                _ => binding.what,
            };
            out.push(Line::from(vec![
                Span::styled(format!("  {:<7}", binding.key), key_style),
                Span::styled(what.to_string(), what_style),
            ]));
        }
    }
    out
}

pub fn render(f: &mut Frame, area: Rect, model: &GainModel, scroll: usize, theme: &crate::Theme) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let all = lines(model, area.width as usize, theme);
    let visible = area.height as usize;
    let first = scroll.min(all.len().saturating_sub(visible));
    let shown: Vec<Line> = all.into_iter().skip(first).take(visible).collect();
    f.render_widget(Paragraph::new(shown), area);
}

/// How many rows the reference needs, so the caller can tell whether scrolling
/// is possible at all.
pub fn row_count(model: &GainModel) -> usize {
    lines(model, 40, &crate::Theme::sdr()).len()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every character key `input/global/mod.rs` claims is documented here.
    ///
    /// `1` to `9` are skipped: one match arm covers all nine and the reference
    /// documents them as a range. `0` is not skipped, because it is still its own
    /// arm doing its own thing.
    #[test]
    fn every_global_key_appears_in_the_reference() {
        let src = include_str!("../../app/input/global/mod.rs");
        let documented: Vec<char> = GLOBAL
            .iter()
            .flat_map(|(_, bs)| bs.iter().filter_map(|b| b.ch))
            .collect();

        let mut missing = Vec::new();
        let mut rest = src;
        while let Some(i) = rest.find("KeyCode::Char('") {
            rest = &rest[i + "KeyCode::Char('".len()..];
            let Some(c) = rest.chars().next() else { break };
            if ('1'..='9').contains(&c) {
                continue;
            }
            if !documented.contains(&c) && !missing.contains(&c) {
                missing.push(c);
            }
        }
        assert!(
            missing.is_empty(),
            "these global keys are not in the Keys pane: {missing:?}. \
             Add them to GLOBAL in this file."
        );
    }

    /// And nothing documented has quietly stopped existing. The pair matters:
    /// the first test catches an undocumented key, this one catches a key the
    /// reference still promises after the arm was deleted.
    #[test]
    fn the_reference_documents_no_dead_keys() {
        let src = include_str!("../../app/input/global/mod.rs");
        for (group, bindings) in GLOBAL {
            for binding in *bindings {
                let Some(c) = binding.ch else { continue };
                assert!(
                    src.contains(&format!("KeyCode::Char('{c}')")),
                    "the Keys pane lists [{}] under '{group}', but no arm handles it",
                    binding.key
                );
            }
        }
    }

    /// A key is documented once. Two rows for one key means two answers to one
    /// question, and the reader has no way to tell which is current.
    #[test]
    fn no_key_is_documented_twice() {
        let mut seen = Vec::new();
        for (_, bindings) in GLOBAL {
            for binding in *bindings {
                if let Some(c) = binding.ch {
                    assert!(!seen.contains(&c), "[{c}] is documented twice");
                    seen.push(c);
                }
            }
        }
    }

    /// A device with no automatic gain mode and no second stage is offered
    /// neither, and the reference is three rows shorter than a HackRF's.
    ///
    /// This is not a hypothetical device: `SoapySDRUtil --probe="driver=hackrf"`
    /// reports `Supports AGC: NO`, so a HackRF reached through SoapySDR is
    /// exactly this shape.
    #[test]
    fn a_device_with_no_boost_is_not_told_about_one() {
        let theme = crate::Theme::sdr();
        let hackrf = lines(&GainModel::HackRf, 40, &theme);
        let soapy = lines(
            &GainModel::Soapy {
                min_db: 0,
                max_db: 116,
                elements: vec![],
                agc: false,
            },
            40,
            &theme,
        );
        let text = |ls: &[Line<'static>]| {
            ls.iter()
                .map(|l| l.to_string())
                .collect::<Vec<_>>()
                .join("\n")
        };
        assert!(text(&hackrf).contains("front end boost"));
        assert!(
            !text(&soapy).contains("front end boost"),
            "offered a boost it does not have:\n{}",
            text(&soapy)
        );
        assert!(!text(&soapy).contains("VGA"), "and no second stage either");
        assert_eq!(soapy.len(), hackrf.len() - 3, "two VGA rows and the boost");
        // The group survives, because the primary gain key is still there.
        assert!(text(&soapy).contains("GAIN"), "{}", text(&soapy));
    }

    /// The RTL-SDR reference drops the VGA rows rather than describing a knob
    /// that device does not have, which is what the old overlay did by hand.
    #[test]
    fn a_single_knob_device_gets_a_shorter_reference() {
        let theme = crate::Theme::sdr();
        let hackrf = lines(&GainModel::HackRf, 40, &theme).len();
        let rtl = lines(
            &GainModel::RtlSingle {
                gain_steps_db: vec![0, 10, 20],
            },
            40,
            &theme,
        )
        .len();
        assert_eq!(rtl, hackrf - 2, "the two VGA rows should be gone");
    }
}
