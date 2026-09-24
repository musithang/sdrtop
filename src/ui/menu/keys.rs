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

use crate::hardware::{DeviceCapabilities, GainModel};
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
    /// More than one stage reported, so picking one means something.
    SeveralStages,
}

impl Needs {
    fn met(&self, model: &GainModel) -> bool {
        match self {
            Needs::Always => true,
            Needs::SecondStage => model.has_second_stage(),
            Needs::Boost => model.has_boost(),
            Needs::SeveralStages => model.stages().len() > 1,
        }
    }
}

/// Where a key is named on the deck's footer, if anywhere: the footer is
/// built from this table, the same one the Keys pane draws and the dispatch
/// check reads, so the three cannot disagree.
pub enum Footer {
    /// Not on the footer; the Keys pane and `keys.md` have it.
    No,
    /// The radio group every section's footer opens with, under this short
    /// label.
    Radio(&'static str),
    /// A gain row: its label is the gain model's (the stage the driver named,
    /// the boost's own name), never this table's.
    Gain,
    /// The section's own group, in the sections named (menu section ids),
    /// under this short label.
    Section(&'static [&'static str], &'static str),
    /// The group every footer ends with.
    Tail(&'static str),
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
    /// Where the deck's footer names it.
    pub footer: Footer,
}

const fn b(key: &'static str, ch: Option<char>, what: &'static str) -> Binding {
    Binding {
        key,
        ch,
        what,
        single: OnSingle::Same,
        needs: Needs::Always,
        footer: Footer::No,
    }
}

/// [`b`], named on the footer.
const fn f(key: &'static str, ch: Option<char>, what: &'static str, footer: Footer) -> Binding {
    Binding {
        key,
        ch,
        what,
        single: OnSingle::Same,
        needs: Needs::Always,
        footer,
    }
}

/// Everything `input/global/mod.rs` claims, grouped the way that file groups its
/// match arms so the two read in the same order.
pub const GLOBAL: &[(&str, &[Binding])] = &[
    (
        "The radio",
        &[
            f("Space", Some(' '), "start or stop RX", Footer::Radio("RX")),
            b("R", Some('r'), "reset everything to defaults"),
            f("F", Some('f'), "type a frequency", Footer::Radio("Freq")),
            f("S", Some('s'), "type a sample rate", Footer::Radio("Rate")),
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
                footer: Footer::Gain,
            },
            Binding {
                key: "[",
                ch: Some('['),
                what: "VGA gain down",
                single: OnSingle::Same,
                needs: Needs::SecondStage,
                footer: Footer::Gain,
            },
            Binding {
                key: "]",
                ch: Some(']'),
                what: "VGA gain up",
                single: OnSingle::Same,
                needs: Needs::SecondStage,
                // `[` names the pair on the footer.
                footer: Footer::No,
            },
            Binding {
                key: "A",
                ch: Some('a'),
                what: "front end boost: the RF amp",
                single: OnSingle::Reword("front end boost: the tuner AGC"),
                needs: Needs::Boost,
                footer: Footer::Gain,
            },
            Binding {
                key: ",",
                ch: Some(','),
                what: "pick the previous gain stage, or the whole chain",
                single: OnSingle::Same,
                needs: Needs::SeveralStages,
                // `.` names the pair on the footer.
                footer: Footer::No,
            },
            Binding {
                key: ".",
                ch: Some('.'),
                what: "pick the next gain stage, or the whole chain",
                single: OnSingle::Same,
                needs: Needs::SeveralStages,
                footer: Footer::Gain,
            },
        ],
    ),
    (
        "The view",
        &[
            f(
                "M",
                Some('m'),
                "NET: survey the band, or lock where you are",
                Footer::Section(&["net"], "mode"),
            ),
            f(
                "I",
                Some('i'),
                "in NET: addresses in full, by vendor and kind, or masked",
                Footer::Section(&["net"], "addresses"),
            ),
            f(
                "\u{2190} \u{2192}",
                None,
                "in NET, locked: the next advertising channel, or the next block of the band",
                Footer::Section(&["net"], "channel"),
            ),
            f(
                "O",
                Some('o'),
                "in NET: write the band, the census, the BLE packets, their error curve and the classic hits to files",
                Footer::Section(&["net"], "Export"),
            ),
            f(
                "Y",
                Some('y'),
                "on a standard station: set the frequency reference",
                Footer::Section(&["lab", "net"], "Reference"),
            ),
            f(
                "W",
                Some('w'),
                "pause or resume the waterfall",
                Footer::Section(&["command_rail", "lab"], "Pause"),
            ),
            f(
                "H",
                Some('h'),
                "freeze a ghost trace, or clear it",
                Footer::Section(&["command_rail", "lab"], "Hold"),
            ),
            b("Tab", None, "show or hide the footer"),
        ],
    ),
    (
        "Layouts and session",
        &[
            b("1-9", None, "the nth layout of this section"),
            b("P", Some('p'), "next layout in this section"),
            f(
                "Esc",
                None,
                "up one level, or open this menu",
                Footer::Tail("Menu"),
            ),
            f("Q", Some('q'), "quit, saving the config", Footer::Tail("Quit")),
        ],
    ),
];

/// The reference as lines, for the given device's gain model.
///
/// Separate from drawing so the scroll arithmetic and the row count can be
/// tested without a terminal.
#[cfg(test)]
fn lines(model: &GainModel, iw: usize, theme: &crate::Theme) -> Vec<Line<'static>> {
    lines_for(model, false, iw, theme)
}

/// A gain row in the device's own words: the stage the arrows move, the
/// second stage by the name the device gave it, the boost by its own name.
/// `None` for every other row, which reads the same on every radio.
fn gain_wording(model: &GainModel, binding: &Binding) -> Option<String> {
    let stages = model.stages();
    Some(match (binding.footer_is_gain(), binding.ch) {
        (true, None) => match (model.has_second_stage(), stages.first()) {
            (true, Some(first)) => format!("{} gain, down and up", first.name),
            _ => "gain, down and up: the whole chain, or the stage picked with , and .".to_string(),
        },
        (true, Some('[')) => format!("{} gain down", stages.get(1)?.name),
        (_, Some(']')) => format!("{} gain up", stages.get(1)?.name),
        (true, Some('a')) => format!("front end boost: {}", model.boost_label()),
        _ => return None,
    })
}

impl Binding {
    fn footer_is_gain(&self) -> bool {
        matches!(self.footer, Footer::Gain)
    }
}

/// One panel's controls, as its section's block in the Keys pane lists
/// them (net-ux-polish-plan 7.3): read from the registry at the moment the
/// menu is drawn (`ui::LayoutEngine::section_controls`), never typed here,
/// so a panel that gains a binding shows it without a second list to keep
/// in step (POLICY rule 7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Control {
    pub key: char,
    /// The panel's title, the focus-key marker taken out.
    pub title: String,
    pub bindings: &'static [(&'static str, &'static str)],
}

/// The column the bindings start in: the key, and the longest title a
/// section has, within reason.
const TITLE_W: usize = 24;

/// The selected section's block: its panels' focus keys and what each does
/// while focused, bindings wrapped under their panel rather than cut.
fn section_block(
    title: &str,
    controls: &[Control],
    iw: usize,
    theme: &crate::Theme,
) -> Vec<Line<'static>> {
    if controls.is_empty() {
        return Vec::new();
    }
    let key_style = Style::default()
        .fg(theme.border_accent)
        .add_modifier(Modifier::BOLD);
    let mut out = vec![chrome::section(
        &format!("{title}: focus keys"),
        "from the panels",
        iw,
        theme,
    )];
    let lead = 5 + TITLE_W;
    for c in controls {
        let what = c
            .bindings
            .iter()
            .map(|(k, w)| format!("{k} {w}"))
            .collect::<Vec<_>>()
            .join(" \u{00b7} ");
        let mut name: String = c.title.chars().take(TITLE_W - 1).collect();
        if c.title.chars().count() >= TITLE_W {
            name.pop();
            name.push('\u{2026}');
        }
        let chunks = if iw > lead + 10 {
            chrome::wrap(&what, iw - lead, 4)
        } else {
            Vec::new()
        };
        out.push(Line::from(vec![
            Span::styled(format!("  {}  ", c.key), key_style),
            Span::styled(
                format!("{name:<TITLE_W$}"),
                Style::default().fg(theme.value),
            ),
            Span::styled(
                chunks.first().cloned().unwrap_or_default(),
                Style::default().fg(theme.label),
            ),
        ]));
        for chunk in chunks.into_iter().skip(1) {
            out.push(Line::from(vec![
                Span::raw(" ".repeat(lead)),
                Span::styled(chunk, Style::default().fg(theme.label)),
            ]));
        }
    }
    out.push(Line::from(""));
    out
}

fn lines_for(
    model: &GainModel,
    sample_rate_is_span: bool,
    iw: usize,
    theme: &crate::Theme,
) -> Vec<Line<'static>> {
    let single = model.is_single();
    let key_style = Style::default()
        .fg(theme.border_accent)
        .add_modifier(Modifier::BOLD);
    let what_style = Style::default().fg(theme.value);

    let mut out = Vec::new();
    for (group, bindings) in GLOBAL {
        if sample_rate_is_span && *group == "Gain" {
            continue;
        }
        // Which of this group's keys this device actually has. Collected first
        // so a group that filters down to nothing takes its heading with it: a
        // section title over empty space reads as a rendering bug, not as
        // "your radio has none of these".
        let shown: Vec<&Binding> = bindings
            .iter()
            .filter(|binding| {
                binding.needs.met(model) && !(sample_rate_is_span && binding.ch == Some('m'))
            })
            .collect();
        if shown.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(Line::from(""));
        }
        out.push(chrome::section(group, "", iw, theme));
        for binding in shown {
            let what = gain_wording(model, binding).unwrap_or_else(|| {
                match (sample_rate_is_span, binding.ch, single, &binding.single) {
                    (true, Some('s'), _, _) => "type a span",
                    (_, _, true, OnSingle::Reword(text)) => text,
                    _ => binding.what,
                }
                .to_string()
            });
            out.push(Line::from(vec![
                Span::styled(format!("  {:<7}", binding.key), key_style),
                Span::styled(what.to_string(), what_style),
            ]));
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
pub fn render(
    f: &mut Frame,
    area: Rect,
    caps: &DeviceCapabilities,
    section: &str,
    controls: &[Control],
    scroll: usize,
    theme: &crate::Theme,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let mut all = section_block(section, controls, area.width as usize, theme);
    all.extend(lines_for(
        &caps.gain,
        caps.sample_rate_is_span,
        area.width as usize,
        theme,
    ));
    let visible = area.height as usize;
    let first = scroll.min(all.len().saturating_sub(visible));
    let shown: Vec<Line> = all.into_iter().skip(first).take(visible).collect();
    f.render_widget(Paragraph::new(shown), area);
}

/// How many rows the reference needs, so the caller can tell whether scrolling
/// is possible at all.
pub fn row_count_for(caps: &DeviceCapabilities, controls: &[Control]) -> usize {
    let theme = crate::Theme::sdr();
    section_block("", controls, 40, &theme).len()
        + lines_for(&caps.gain, caps.sample_rate_is_span, 40, &theme).len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::native::{hackrf, rtlsdr};

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
            // `input::fold_key_case` lowers every letter before dispatch, so an
            // arm on a capital can never fire: it would be a key that is
            // documented, tested through a harness that skips the fold, and dead
            // on a real keyboard. Shift+A was once added that way and would have
            // toggled the amp instead.
            assert!(
                !c.is_ascii_uppercase(),
                "global arm on '{c}' can never fire: letters are folded to lower case"
            );
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
        let hackrf = lines(&hackrf::gain_model(), 40, &theme);
        let chain = lines(
            &GainModel::new(vec![], "RF", "RF").with_gauge_fallback(116),
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
            !text(&chain).contains("front end boost"),
            "offered a boost it does not have:\n{}",
            text(&chain)
        );
        assert!(!text(&chain).contains("VGA"), "and no second stage either");
        // Two VGA rows, the boost, and the two stage-picking rows, which need
        // more than one stage and this chain reports none.
        assert_eq!(
            chain.len(),
            hackrf.len() - 5,
            "two VGA rows, the boost, and , ."
        );
        // The group survives, because the primary gain key is still there.
        assert!(text(&chain).contains("GAIN"), "{}", text(&chain));
    }

    /// The RTL-SDR reference drops the VGA rows rather than describing a knob
    /// that device does not have, which is what the old overlay did by hand.
    #[test]
    fn a_single_knob_device_gets_a_shorter_reference() {
        let theme = crate::Theme::sdr();
        let hackrf = lines(&hackrf::gain_model(), 40, &theme).len();
        let rtl = lines(&rtlsdr::gain_model(&[0, 10, 20]), 40, &theme).len();
        // The two VGA rows, and `,` / `.`: one tuner stage is nothing to pick.
        assert_eq!(rtl, hackrf - 4, "the two VGA rows and , . should be gone");
    }

    #[test]
    fn power_trace_reference_keeps_only_supported_controls() {
        let theme = crate::Theme::sdr();
        let lines = lines_for(
            &GainModel::new(Vec::new(), "RF", "RF").with_gauge_fallback(0),
            true,
            80,
            &theme,
        );
        let text = lines
            .iter()
            .map(Line::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("type a span"));
        assert!(!text.contains("GAIN"));
        assert!(!text.contains("survey the band"));
    }

    /// **The reference speaks the device's words**: three stages the driver
    /// called LNA, TIA and PGA and an automatic gain mode read as one chain,
    /// its stages to pick, and an AGC, with no VGA anywhere.
    #[test]
    fn the_gain_rows_use_the_devices_names() {
        let theme = crate::Theme::sdr();
        let lime = GainModel::new(
            vec![
                crate::hardware::StageSpec::ranged("LNA", 0.0, 30.0, 1.0),
                crate::hardware::StageSpec::ranged("TIA", 0.0, 12.0, 1.0),
                crate::hardware::StageSpec::ranged("PGA", -12.0, 19.0, 1.0),
            ],
            "RF",
            "RF",
        )
        .with_boost(crate::hardware::Boost::GainMode);
        let text = lines(&lime, 90, &theme)
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            text.contains("the whole chain, or the stage picked"),
            "{text}"
        );
        assert!(text.contains("pick the next gain stage"), "{text}");
        assert!(text.contains("front end boost: AGC"), "{text}");
        assert!(!text.contains("VGA"), "{text}");
        let hackrf = lines(&hackrf::gain_model(), 90, &theme)
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(hackrf.contains("LNA gain, down and up"), "{hackrf}");
        assert!(hackrf.contains("VGA gain down"), "{hackrf}");
        assert!(hackrf.contains("front end boost: AMP"), "{hackrf}");
    }
}
