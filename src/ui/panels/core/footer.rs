// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

use ratatui::{
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::Paragraph,
    Frame,
};

use crate::hardware::GainModel;
use crate::state::{InputMode, MicroView, SdrMetrics};
use crate::ui::chrome::frame;
use crate::ui::panel::{FrameStyle, FrameTone, Panel, PanelChrome};

const FOCUS_SEP: &str = "  ·  ";
const NORMAL_SEP: &str = " · ";
const MAX_CONTENT_LINES: u16 = 5;

/// Between groups: the radio, the section, the layouts and the way out read as
/// four things, not one long list.
const GROUP_SEP: &str = "  \u{2502}  ";

/// One group of footer items, drawn together. A group moves to the next line
/// whole, and breaks inside itself only when it is wider than the footer.
type Group = Vec<String>;

/// The radio group every section's footer opens with, from the key table
/// (`ui::menu::keys::GLOBAL`) and the gain model: the gain keys are named by
/// the stages the device reported and its boost by its own name, and a key
/// the device cannot use is not offered. A power-trace device (a tinySA)
/// takes a span where the others take a rate, and has no gain keys here.
fn radio_group(gm: &GainModel, span: bool, picked: Option<usize>) -> Group {
    use crate::ui::menu::keys::{Footer, GLOBAL};
    let stages = gm.stages();
    let mut out = Vec::new();
    for (_, rows) in GLOBAL {
        for row in rows.iter() {
            match row.footer {
                Footer::Radio(label) => {
                    let label = if span && row.ch == Some('s') {
                        "Span"
                    } else {
                        label
                    };
                    out.push(format!("[{}] {label}", row.key));
                }
                Footer::Gain if !span => match row.ch {
                    // The primary knob: the front stage where the second has
                    // a key of its own, the whole chain where it does not.
                    None => out.push(
                        match (
                            picked.and_then(|i| stages.get(i)),
                            gm.has_second_stage(),
                            stages.first(),
                        ) {
                            // One stage picked with `,` / `.`: the arrows move it.
                            (Some(one), _, _) => format!("[\u{2191}\u{2193}] {}", one.name),
                            (None, true, Some(first)) => {
                                format!("[\u{2191}\u{2193}] {}", first.name)
                            }
                            _ => "[\u{2191}\u{2193}] Gain".to_string(),
                        },
                    ),
                    Some('.') if stages.len() > 1 => out.push(format!(
                        "[, .] stage={}",
                        picked
                            .and_then(|i| stages.get(i))
                            .map_or("chain", |s| s.name.as_str())
                    )),
                    Some('[') if gm.has_second_stage() => {
                        if let Some(second) = stages.get(1) {
                            out.push(format!("[[ ]] {}", second.name));
                        }
                    }
                    Some('a') if gm.has_boost() => {
                        out.push(format!("[{}] {}", row.key, gm.boost_label()));
                    }
                    _ => {}
                },
                _ => {}
            }
        }
    }
    out
}

/// The section's own group: the keys the key table files under this menu
/// section, with the NET modes' current state beside their keys. Focus keys
/// are not here: every panel that takes focus names its letter in its own
/// title.
fn section_group(m: &SdrMetrics) -> Group {
    use crate::ui::menu::keys::{Footer, GLOBAL};
    let mut out = Vec::new();
    for (_, rows) in GLOBAL {
        for row in rows.iter() {
            let Footer::Section(sections, label) = row.footer else {
                continue;
            };
            if !sections.contains(&m.ui.section.as_str()) {
                continue;
            }
            out.push(match row.ch {
                Some('m') => format!("[{}] {label}={}", row.key, m.net.mode.label()),
                Some('i') => format!("[{}] {label}={}", row.key, m.net.address_display.label()),
                _ => format!("[{}] {label}", row.key),
            });
        }
    }
    out
}

/// The group every footer ends with.
fn tail_group() -> Group {
    use crate::ui::menu::keys::{Footer, GLOBAL};
    GLOBAL
        .iter()
        .flat_map(|(_, rows)| rows.iter())
        .filter_map(|row| match row.footer {
            Footer::Tail(label) => Some(format!("[{}] {label}", row.key)),
            _ => None,
        })
        .collect()
}

/// Width (terminal columns) below which the preset name is shown in short form.
const NARROW_COLS: u16 = 60;

/// Display label for a preset in the footer. Narrow terminals get an
/// abbreviated form for the few long names; everything else passes through.
fn preset_label(name: &str, narrow: bool) -> &str {
    if narrow {
        match name {
            "spectrum_waterfall" => "spec+wf",
            "spectrum" => "spec",
            "waterfall" => "wf",
            other => other,
        }
    } else {
        name
    }
}

/// Whether `name` is a micro ecosystem preset.
fn is_micro_preset(name: &str) -> bool {
    name.starts_with("micro_")
}

/// Condensed footer for the micro ecosystem: the essential field keys, the range
/// of number keys that switch view, and the `N/M` position.
///
/// The hint used to be `[0]\u{25B8}{next}`, naming the key that walked the cycle.
/// The cycle is gone: the views have number keys like every other section, so the
/// footer names the range that actually works.
fn micro_items(
    view: MicroView,
    scope: &[(Option<u8>, String, String)],
    narrow: bool,
    gm: &GainModel,
) -> Vec<String> {
    let sweep_active = true;
    let total = MicroView::total(sweep_active);
    let pos = view.position();
    let keys = match scope.iter().filter(|(slot, _, _)| slot.is_some()).count() {
        0 => "[1-9]".to_string(),
        n => format!("[1-{n}]"),
    };
    if narrow {
        vec![
            "[Q]".into(),
            "[Spc]".into(),
            "[↑↓]".into(),
            keys,
            format!("{}/{}", pos, total),
        ]
    } else {
        let mut v: Vec<String> = vec!["[Q]".into(), "[Spc]RX".into()];
        // Named by the model, as on every other footer.
        let stages = gm.stages();
        match (gm.has_second_stage(), stages.first(), stages.get(1)) {
            (true, Some(first), Some(second)) => {
                v.push(format!("[↑↓]{}", first.name));
                v.push(format!("[[]{}", second.name));
            }
            _ => v.push("[↑↓]Gain".into()),
        }
        v.push("[F]req".into());
        v.push(keys);
        v.push(format!("micro {}/{}", pos, total));
        v
    }
}

/// Navigation map for the active section: one entry per layout that has a number
/// key, under the title the menu gives it, with the current one marked `▸`.
///
/// Built from `UiState::scope`, which mirrors the engine's section into the frame
/// snapshot. It used to be built from a `LAB_FAMILY` table hard-coding
/// `[5]`-`[8]`, which stopped being true the moment the digits became
/// section-relative: the footer would have gone on advertising four keys that do
/// nothing, which is the same way the old help overlay came to claim `[1]` meant
/// `main`. A footer that names keys has to read the keys.
fn scope_map_items(active: &str, scope: &[(Option<u8>, String, String)]) -> Vec<String> {
    scope
        .iter()
        // A layout with no slot has no number key, so the map has nothing to
        // teach about it.
        .filter_map(|(slot, name, title)| slot.map(|s| (s, name, title)))
        .map(|(slot, name, title)| {
            if name == active {
                format!("[{}]\u{25B8}{}", slot, title)
            } else {
                format!("[{}] {}", slot, title)
            }
        })
        .collect()
}

/// The footer for the active layout, in groups: the radio, the section's own
/// keys, the section's layouts, and the way out. Each section gets a footer of
/// its own this way, because the middle two groups are the section's. Micro
/// keeps its condensed field footer.
fn normal_groups(m: &SdrMetrics, available_width: u16) -> Vec<Group> {
    let narrow = available_width < NARROW_COLS;
    let preset = m.ui.active_preset.as_str();
    if is_micro_preset(preset) {
        return vec![micro_items(
            m.ui.micro_view(),
            &m.ui.scope,
            narrow,
            &m.caps.gain,
        )];
    }
    let mut groups = vec![radio_group(
        &m.caps.gain,
        m.caps.sample_rate_is_span,
        m.ui.gain_stage,
    )];
    let section = section_group(m);
    if !section.is_empty() {
        groups.push(section);
    }
    let map = scope_map_items(preset, &m.ui.scope);
    groups.push(if map.is_empty() {
        // A layout the menu does not list has no number keys to map.
        vec![format!("[P] {}", preset_label(preset, narrow))]
    } else {
        map
    });
    groups.push(tail_group());
    groups
}

/// Lay the groups out in lines no wider than `inner_w`: each item paired with
/// whether it opens a group (drawn after the group rule rather than the item
/// dot). A group that fits goes whole onto the current line or the next.
fn wrap_groups(groups: &[Group], inner_w: usize) -> Vec<Vec<(bool, String)>> {
    let item_w = |s: &str| s.chars().count();
    let group_w = |g: &Group| {
        g.iter().map(|s| item_w(s)).sum::<usize>()
            + NORMAL_SEP.chars().count() * g.len().saturating_sub(1)
    };
    let (gsep, isep) = (GROUP_SEP.chars().count(), NORMAL_SEP.chars().count());
    let mut lines: Vec<Vec<(bool, String)>> = vec![Vec::new()];
    let mut w = 0usize;
    for g in groups.iter().filter(|g| !g.is_empty()) {
        let gw = group_w(g);
        if w > 0 && w + gsep + gw > inner_w {
            lines.push(Vec::new());
            w = 0;
        }
        for (k, item) in g.iter().enumerate() {
            let sep = match (w, k) {
                (0, _) => 0,
                (_, 0) => gsep,
                _ => isep,
            };
            // Inside a group wider than the line, break between its items.
            if w > 0 && w + sep + item_w(item) > inner_w {
                lines.push(Vec::new());
                w = 0;
            }
            let opens = k == 0 && w > 0;
            let sep = if w == 0 {
                0
            } else if k == 0 {
                gsep
            } else {
                isep
            };
            lines
                .last_mut()
                .expect("a line")
                .push((opens, item.clone()));
            w += sep + item_w(item);
        }
    }
    lines.retain(|l| !l.is_empty());
    if lines.is_empty() {
        lines.push(Vec::new());
    }
    lines
}

/// The wrapped groups as styled lines: a dim dot between items, a dim rule
/// between groups.
fn styled_group_lines(
    lines: Vec<Vec<(bool, String)>>,
    theme: &crate::Theme,
    max_lines: usize,
) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .take(max_lines.max(1))
        .map(|line| {
            let mut spans: Vec<Span> = Vec::new();
            for (i, (opens, item)) in line.iter().enumerate() {
                if i > 0 {
                    spans.push(Span::styled(
                        if *opens { GROUP_SEP } else { NORMAL_SEP }.to_string(),
                        Style::default().fg(theme.border_dim),
                    ));
                }
                spans.extend(item_spans(item, theme));
            }
            Line::from(spans)
        })
        .collect()
}

/// Break `items` into lines (groups) where no line exceeds `inner_w` display
/// columns. Returns the items per line, preserving boundaries so the renderer
/// can style each key/description independently.
fn wrap_items_grouped<S: AsRef<str>>(items: &[S], sep: &str, inner_w: usize) -> Vec<Vec<String>> {
    let sep_w = sep.chars().count();
    let mut lines: Vec<Vec<String>> = Vec::new();
    let mut cur: Vec<String> = Vec::new();
    let mut cur_w = 0usize;

    for item in items {
        let s = item.as_ref();
        let iw = s.chars().count();
        let needed = if cur.is_empty() { iw } else { sep_w + iw };
        if !cur.is_empty() && inner_w > 0 && cur_w + needed > inner_w {
            lines.push(std::mem::take(&mut cur));
            cur.push(s.to_string());
            cur_w = iw;
        } else {
            cur.push(s.to_string());
            cur_w += needed;
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

/// Break `items` into joined lines: the wrapping rule as text, for the tests.
#[cfg(test)]
fn wrap_items<S: AsRef<str>>(items: &[S], sep: &str, inner_w: usize) -> Vec<String> {
    let mut lines: Vec<String> = wrap_items_grouped(items, sep, inner_w)
        .into_iter()
        .map(|g| g.join(sep))
        .collect();
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Style one footer item into spans: a bright bolded key in faint brackets, a
/// dim description, and an accented `▸` active-marker. Items without a `[key]`
/// (e.g. `micro 1/5`) render as a single dim label.
fn item_spans(item: &str, theme: &crate::Theme) -> Vec<Span<'static>> {
    if item.starts_with('[') {
        if let Some(end) = item.find(']') {
            let inner = item[1..end].to_string(); // key text, e.g. "Q" / "↑↓" / "Space"
            let rest = &item[end + 1..]; // " Quit" or "▸signal" or ""
            let mut spans = vec![
                Span::styled("[", Style::default().fg(theme.border_dim)),
                Span::styled(
                    inner,
                    Style::default()
                        .fg(theme.value_hi)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled("]", Style::default().fg(theme.border_dim)),
            ];
            if let Some(name) = rest.strip_prefix('\u{25B8}') {
                // active preset/lab entry: ▸name highlighted
                spans.push(Span::styled(
                    "\u{25B8}",
                    Style::default().fg(theme.border_accent),
                ));
                spans.push(Span::styled(
                    name.to_string(),
                    Style::default().fg(theme.value_hi),
                ));
            } else if let Some((word, state)) = rest.split_once('=') {
                // A key with its state beside it (`[M] mode=SURVEY`): the
                // word in the label ink, the state as a value.
                spans.push(Span::styled(
                    format!("{word} "),
                    Style::default().fg(theme.label),
                ));
                spans.push(Span::styled(
                    state.to_string(),
                    Style::default()
                        .fg(theme.value)
                        .add_modifier(Modifier::BOLD),
                ));
            } else if !rest.is_empty() {
                spans.push(Span::styled(
                    rest.to_string(),
                    Style::default().fg(theme.label),
                ));
            }
            return spans;
        }
    }
    vec![Span::styled(
        item.to_string(),
        Style::default().fg(theme.label),
    )]
}

/// Assemble wrapped item groups into styled `Line`s, joining items with a dim
/// separator. `max_lines` clamps the output to what fits in the panel.
fn styled_lines(
    groups: Vec<Vec<String>>,
    sep: &str,
    theme: &crate::Theme,
    max_lines: usize,
) -> Vec<Line<'static>> {
    groups
        .into_iter()
        .take(max_lines.max(1))
        .map(|g| {
            let mut spans: Vec<Span> = Vec::new();
            for (i, item) in g.iter().enumerate() {
                if i > 0 && item.starts_with(NAME_MARK) {
                    spans.push(Span::raw(NAME_GAP));
                } else if i > 0 {
                    spans.push(Span::styled(
                        sep.to_string(),
                        Style::default().fg(theme.border_dim),
                    ));
                }
                spans.extend(item_spans(item, theme));
            }
            Line::from(spans)
        })
        .collect()
}

/// Public free function - called directly from the engine (bypasses dyn dispatch).
pub fn compute_footer_height(available_width: u16, state: &SdrMetrics) -> u16 {
    if !matches!(
        state.ui.input_mode,
        InputMode::Normal | InputMode::DeviceOptionInput { .. }
    ) || state.observer.active
    {
        return 3;
    }
    let inner_w = available_width.saturating_sub(2) as usize;
    let n = if state.ui.focused_panel.is_some() {
        focus_lines(state, inner_w).len()
    } else {
        wrap_groups(&normal_groups(state, available_width), inner_w).len()
    };
    (n as u16 + 2).clamp(3, MAX_CONTENT_LINES + 2)
}

pub struct FooterPanel;

impl Panel for FooterPanel {
    fn name(&self) -> &'static str {
        "footer"
    }
    fn supports_acquisition(&self, _acquisition: crate::hardware::AcquisitionKind) -> bool {
        true
    }
    fn min_size(&self) -> (u16, u16) {
        (40, 3)
    }

    fn preferred_height(&self, available_width: u16, state: &SdrMetrics) -> u16 {
        compute_footer_height(available_width, state)
    }

    fn chrome(&self, m: &SdrMetrics) -> PanelChrome {
        // The footer's border is an application-mode indicator rather than a
        // panel state: it reports what the keys along it are currently for.
        PanelChrome::untitled()
            .frame(FrameStyle::Deck)
            .tone(footer_tone(m))
    }

    fn render(
        &self,
        f: &mut Frame,
        inner: Rect,
        m: &SdrMetrics,
        theme: &crate::Theme,
        _focused: bool,
    ) {
        let inner_w = inner.width as usize;
        let max_lines = inner.height as usize;

        // Single-line data-entry prompt: dim label, the live buffer highlighted.
        let prompt = |s: String| -> Vec<Line<'static>> {
            vec![Line::from(Span::styled(
                s,
                Style::default().fg(theme.value),
            ))]
        };

        let lines: Vec<Line<'static>> = if m.observer.active {
            styled_lines(
                vec![vec![
                    "[Q] Quit".into(),
                    "[Esc] Menu".into(),
                    "(Observer Mode)".into(),
                ]],
                FOCUS_SEP,
                theme,
                max_lines,
            )
        } else {
            match m.ui.input_mode {
                InputMode::FrequencyInput => prompt(format!(
                    " Frequency (MHz): [{}▌]  [Enter] Confirm  [Esc] Cancel",
                    m.ui.input_buf
                )),
                InputMode::SampleRateInput => prompt(format!(
                    " {} ({:.1}–{:.1} MHz): [{}▌]  [Enter] Confirm  [Esc] Cancel",
                    if m.caps.sample_rate_is_span {
                        "Span"
                    } else {
                        "Sample rate"
                    },
                    m.caps.sample_rate_min_hz / 1e6,
                    m.caps.sample_rate_max_hz / 1e6,
                    m.ui.input_buf
                )),
                InputMode::SweepStartInput => prompt(format!(
                    " Sweep START (MHz): [{}▌]  [Enter] Confirm  [Esc] Cancel",
                    m.ui.input_buf
                )),
                InputMode::SweepStopInput => prompt(format!(
                    " Sweep STOP (MHz): [{}▌]  [Enter] Confirm  [Esc] Cancel",
                    m.ui.input_buf
                )),
                InputMode::MarkerNameInput => {
                    let freq_str = m
                        .spectrum
                        .pending_marker
                        .map(|f| format!("{:.3} MHz", f as f64 / 1_000_000.0))
                        .unwrap_or_default();
                    prompt(format!(
                        " Marker name at {}:  [{}▌]  [Enter] Confirm  [Esc] Cancel",
                        freq_str, m.ui.input_buf
                    ))
                }
                InputMode::ReferenceAccuracyInput { address } => {
                    let name = m
                        .net
                        .census
                        .devices
                        .iter()
                        .find(|d| d.address == address)
                        .map(|d| d.address_text(&m.net, None))
                        .unwrap_or_default();
                    prompt(format!(
                        " Trust {name} as the frequency reference, to ±ppm: [{}▌]  [Enter] Confirm  [Esc] Cancel",
                        m.ui.input_buf
                    ))
                }
                InputMode::Normal | InputMode::DeviceOptionInput { .. } => {
                    if m.ui.focused_panel.is_some() {
                        styled_lines(focus_lines(m, inner_w), FOCUS_SEP, theme, max_lines)
                    } else {
                        let groups = normal_groups(m, frame::outer_of(inner).width);
                        styled_group_lines(wrap_groups(&groups, inner_w), theme, max_lines)
                    }
                }
            }
        };

        f.render_widget(
            Paragraph::new(Text::from(lines)).alignment(Alignment::Center),
            inner,
        );
    }
}

/// Which palette slot the footer's frame is in, i.e. what the keys along it are
/// currently for.
///
/// This is the one border in the deck that reports an *application* mode rather
/// than a panel's own state, so it names its tone instead of taking the standard
/// focus-and-staleness rule.
fn footer_tone(m: &SdrMetrics) -> FrameTone {
    tone_for(
        m.observer.active,
        &m.ui.input_mode,
        m.ui.focused_panel.is_some(),
    )
}

/// The rule itself, on plain inputs so it can be tested without a snapshot.
fn tone_for(observer: bool, mode: &InputMode, panel_focused: bool) -> FrameTone {
    if observer {
        return FrameTone::Observer;
    }
    match mode {
        // Normal: lit while a panel is focused, because the keys along the
        // footer are that panel's, not the global set.
        InputMode::Normal | InputMode::DeviceOptionInput { .. } if panel_focused => {
            FrameTone::Focused
        }
        InputMode::Normal | InputMode::DeviceOptionInput { .. } => FrameTone::Dim,
        // Anything else is a half-typed value waiting on Enter.
        _ => FrameTone::Warn,
    }
}

/// What names the focused panel at the end of its keys: the dash marks it as
/// a name rather than a key, and `styled_lines` gives it a plain gap instead
/// of the separator.
const NAME_MARK: char = '\u{2014}';

/// The focus footer's lines: the panel's keys, wrapped, and the panel's name
/// after the last of them, or on a line of its own when the last is full.
/// Wrapped here rather than appended after, which cut the name off at the
/// frame on a long footer (`net_ble_packets` lost its last letter); the
/// height is counted from the same lines, so the two cannot disagree.
fn focus_lines(m: &SdrMetrics, inner_w: usize) -> Vec<Vec<String>> {
    let mut lines = wrap_items_grouped(&focus_items(m), FOCUS_SEP, inner_w);
    let Some(panel) = &m.ui.focused_panel else {
        return lines;
    };
    let name = format!("{NAME_MARK} {panel}");
    let used = |line: &Vec<String>| {
        line.iter().map(|i| i.chars().count()).sum::<usize>()
            + FOCUS_SEP.chars().count() * line.len().saturating_sub(1)
    };
    match lines.last_mut() {
        Some(last) if used(last) + NAME_GAP.len() + name.chars().count() <= inner_w => {
            last.push(name)
        }
        _ => lines.push(vec![name]),
    }
    lines
}

/// The gap before the panel's name, where keys get the separator.
const NAME_GAP: &str = "  ";

/// Build the ordered items list for focus-mode footer.
fn focus_items(m: &SdrMetrics) -> Vec<String> {
    let mut items: Vec<String> =
        m.ui.focused_panel_bindings
            .iter()
            .map(|(k, d)| format!("[{}] {}", k, d))
            .collect();
    // The Command Rail repurposes Tab as its mode cycle (HUNT·MONITOR·BENCH);
    // every other focused panel keeps Tab as the footer-hide toggle.
    let tab = if m.ui.focused_panel.as_deref() == Some("command_rail") {
        "[Tab] Mode"
    } else {
        "[Tab] Hide"
    };
    items.push(tab.to_string());
    items.push("[Esc] Exit focus".to_string());
    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::native::hackrf;

    #[test]
    fn the_footer_frame_reports_what_the_keys_are_for() {
        // Observer mode outranks everything: the radio is not ours to drive.
        assert_eq!(
            tone_for(true, &InputMode::Normal, false),
            FrameTone::Observer
        );
        assert_eq!(
            tone_for(true, &InputMode::FrequencyInput, true),
            FrameTone::Observer
        );
        // A half-typed value is the loudest thing the footer can be saying.
        for mode in [
            InputMode::FrequencyInput,
            InputMode::SampleRateInput,
            InputMode::SweepStartInput,
            InputMode::SweepStopInput,
            InputMode::MarkerNameInput,
            InputMode::ReferenceAccuracyInput { address: [0; 6] },
        ] {
            assert_eq!(tone_for(false, &mode, false), FrameTone::Warn);
        }
        // Otherwise it tracks whether the keys belong to a focused panel.
        assert_eq!(
            tone_for(false, &InputMode::Normal, true),
            FrameTone::Focused
        );
        assert_eq!(tone_for(false, &InputMode::Normal, false), FrameTone::Dim);
    }

    #[test]
    fn numeric_entry_keeps_the_normal_deck_footer() {
        let mut m = SdrMetrics::fixture();
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 5)).unwrap();
        let normal_height = compute_footer_height(80, &m);
        let mut draw = |m: &SdrMetrics| {
            terminal
                .draw(|f| FooterPanel.render(f, f.size(), m, &crate::Theme::sdr(), false))
                .unwrap();
            terminal.backend().buffer().clone()
        };
        let normal = draw(&m);
        m.ui.input_mode = InputMode::DeviceOptionInput {
            id: "level".into(),
            error: None,
        };
        m.ui.input_buf = "123456".into();
        assert_eq!(draw(&m), normal);
        assert_eq!(compute_footer_height(80, &m), normal_height);
        assert_eq!(tone_for(false, &m.ui.input_mode, false), FrameTone::Dim);
    }

    #[test]
    fn item_spans_styles_key_and_description() {
        let t = crate::theme::Theme::sdr();
        let spans = item_spans("[Q] Quit", &t);
        // [ key ] then description
        let contents: Vec<&str> = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(contents, vec!["[", "Q", "]", " Quit"]);
        // the key glyph is bold + highlighted
        assert!(spans[1].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(spans[1].style.fg, Some(t.value_hi));
        // the description is dim label
        assert_eq!(spans[3].style.fg, Some(t.label));
    }

    #[test]
    fn item_spans_highlights_active_marker() {
        let t = crate::theme::Theme::sdr();
        let spans = item_spans("[0]▸signal", &t);
        let contents: Vec<&str> = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(contents, vec!["[", "0", "]", "\u{25B8}", "signal"]);
        assert_eq!(spans[3].style.fg, Some(t.border_accent)); // ▸ accent
        assert_eq!(spans[4].style.fg, Some(t.value_hi)); // name highlighted
    }

    #[test]
    fn item_spans_plain_item_is_single_label() {
        let t = crate::theme::Theme::sdr();
        let spans = item_spans("micro 1/5", &t);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].content.as_ref(), "micro 1/5");
        assert_eq!(spans[0].style.fg, Some(t.label));
    }

    #[test]
    fn styled_lines_clamps_to_max() {
        let t = crate::theme::Theme::sdr();
        let groups = vec![
            vec!["[A] a".into()],
            vec!["[B] b".into()],
            vec!["[C] c".into()],
        ];
        let lines = styled_lines(groups, NORMAL_SEP, &t, 2);
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn wrap_items_splits_at_boundary() {
        let items = ["aaa", "bbb", "ccc"];
        // sep="  " (2), inner_w=7: "aaa  bbb"=8 > 7 → break after "aaa"
        let lines = wrap_items(&items, "  ", 7);
        assert_eq!(lines.len(), 3, "each item on its own line: {:?}", lines);
    }

    #[test]
    fn wrap_items_fits_all_on_one_line() {
        let items = ["aaa", "bbb"];
        // "aaa  bbb" = 8 chars, inner_w=10 → fits
        let lines = wrap_items(&items, "  ", 10);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0], "aaa  bbb");
    }

    /// The footer a layout would get: `preset` active, filed under `section`,
    /// whose layouts are `scope`, on a HackRF (the fixture's radio).
    /// A focus footer too long for one line keeps the panel's name whole:
    /// after the last key where it fits, on its own line where it does not,
    /// and the height the engine gives the footer counts that line.
    #[test]
    fn the_focused_panel_name_is_never_cut() {
        let mut m = SdrMetrics::fixture();
        m.ui.focused_panel = Some("net_ble_packets".to_string());
        m.ui.focused_panel_bindings = &[
            ("↑↓", "select a packet"),
            ("Enter", "filter to its address"),
            ("h", "hold the list"),
            ("2", "LE 1M or LE 2M"),
        ];
        for inner_w in 40..140 {
            let lines = focus_lines(&m, inner_w);
            let joined: Vec<String> = lines
                .iter()
                .map(|l| {
                    l.iter().enumerate().fold(String::new(), |acc, (i, item)| {
                        let sep = match (i, item.starts_with(NAME_MARK)) {
                            (0, _) => "",
                            (_, true) => NAME_GAP,
                            _ => FOCUS_SEP,
                        };
                        acc + sep + item
                    })
                })
                .collect();
            let last = joined.last().unwrap();
            assert!(
                last.ends_with("\u{2014} net_ble_packets"),
                "{inner_w}: {joined:?}"
            );
            assert!(
                joined.iter().all(|l| l.chars().count() <= inner_w),
                "{inner_w}: {joined:?}"
            );
            let height = compute_footer_height(inner_w as u16 + 2, &m) as usize;
            assert_eq!(
                height,
                (lines.len() + 2).clamp(3, MAX_CONTENT_LINES as usize + 2)
            );
        }
    }

    fn footer_at(
        preset: &str,
        section: &str,
        scope: Vec<(Option<u8>, String, String)>,
        width: u16,
    ) -> Vec<Group> {
        let mut m = SdrMetrics::fixture();
        m.ui.active_preset = preset.to_string();
        m.ui.section = section.to_string();
        m.ui.scope = scope;
        normal_groups(&m, width)
    }

    fn slots(entries: &[(&str, &str)]) -> Vec<(Option<u8>, String, String)> {
        entries
            .iter()
            .enumerate()
            .map(|(i, (preset, title))| (Some(i as u8 + 1), preset.to_string(), title.to_string()))
            .collect()
    }

    fn rail_scope() -> Vec<(Option<u8>, String, String)> {
        slots(&[
            ("command_rail", "Rail"),
            ("spectrum", "Spectrum"),
            ("waterfall", "Waterfall"),
        ])
    }

    fn lab_scope() -> Vec<(Option<u8>, String, String)> {
        slots(&[
            ("lab_iq", "IQ"),
            ("lab_rf", "RF"),
            ("lab_timing", "Timing"),
            ("lab_signal", "Signal"),
        ])
    }

    fn net_scope() -> Vec<(Option<u8>, String, String)> {
        slots(&[
            ("net", "Capability"),
            ("net_survey", "Survey"),
            ("net_census", "Census"),
            ("net_ble", "BLE"),
            ("net_bt", "Classic"),
        ])
    }

    /// The Micro section as the engine mirrors it: four layouts, slots 1 to 4.
    fn micro_scope() -> Vec<(Option<u8>, String, String)> {
        slots(&[
            ("micro_main", "Overview"),
            ("micro_signal", "Signal"),
            ("micro_gain", "Gain"),
            ("micro_health", "Health"),
        ])
    }

    /// **Each section's footer is its own**: the same radio group and way
    /// out, and between them the section's keys and its layouts, by the
    /// titles the menu uses, the current one marked.
    #[test]
    fn each_section_gets_its_own_footer() {
        let rail = footer_at("command_rail", "command_rail", rail_scope(), 200);
        assert_eq!(rail.len(), 4, "{rail:?}");
        assert_eq!(
            rail[0],
            vec![
                "[Space] RX",
                "[F] Freq",
                "[S] Rate",
                "[↑↓] LNA",
                "[[ ]] VGA",
                "[A] AMP",
                "[, .] stage=chain"
            ]
        );
        assert_eq!(rail[1], vec!["[W] Pause", "[H] Hold"]);
        assert_eq!(rail[2], vec!["[1]▸Rail", "[2] Spectrum", "[3] Waterfall"]);
        assert_eq!(rail[3], vec!["[Esc] Menu", "[Q] Quit"]);

        let lab = footer_at("lab_rf", "lab", lab_scope(), 200);
        assert_eq!(lab[1], vec!["[Y] Reference", "[W] Pause", "[H] Hold"]);
        assert_eq!(lab[2], vec!["[1] IQ", "[2]▸RF", "[3] Timing", "[4] Signal"]);

        // A section with no keys of its own has no middle group.
        let sweep = footer_at("lab_sweep", "sweep", slots(&[("lab_sweep", "Sweep")]), 200);
        assert_eq!(sweep.len(), 3, "{sweep:?}");
    }

    /// **NET's footer says its modes as they stand**: survey or lock and how
    /// addresses are shown, beside the keys that change them, and the export
    /// and reference keys that were nowhere on screen before.
    #[test]
    fn the_net_footer_carries_its_keys_and_their_state() {
        let mut m = SdrMetrics::fixture();
        m.ui.active_preset = "net_ble".to_string();
        m.ui.section = "net".to_string();
        m.ui.scope = net_scope();
        m.net.mode = crate::state::NetMode::Lock;
        m.net.address_display = crate::state::AddressDisplay::Masked;
        let groups = normal_groups(&m, 200);
        assert_eq!(
            groups[1],
            vec![
                "[M] mode=LOCK",
                "[I] addresses=masked",
                "[\u{2190} \u{2192}] channel",
                "[O] Export",
                "[Y] Reference"
            ]
        );
        assert!(groups[2].contains(&"[4]▸BLE".to_string()), "{groups:?}");
        // The value is drawn apart from its word.
        let spans = item_spans("[M] mode=LOCK", &crate::Theme::sdr());
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "[M] mode LOCK");
    }

    /// **No focus letters on the footer**: every panel that takes focus names
    /// its letter in its own title, so the footer holds only keys that are
    /// not a panel's.
    #[test]
    fn the_footer_names_only_keys_the_table_files_there() {
        use crate::ui::menu::keys::{Footer, GLOBAL};
        let filed: Vec<&str> = GLOBAL
            .iter()
            .flat_map(|(_, rows)| rows.iter())
            .filter(|r| !matches!(r.footer, Footer::No))
            .map(|r| r.key)
            .collect();
        for (preset, section, scope) in [
            ("command_rail", "command_rail", rail_scope()),
            ("lab_iq", "lab", lab_scope()),
            ("net_bt", "net", net_scope()),
        ] {
            for group in footer_at(preset, section, scope, 200) {
                for item in group {
                    let key = &item[1..item.find(']').unwrap()];
                    let ok = key.chars().all(|c| c.is_ascii_digit())
                        || matches!(key, "↑↓" | "[ " | ", .")
                        || filed.contains(&key);
                    assert!(
                        ok,
                        "{preset}: '{item}' is not a key the table files on the footer"
                    );
                }
            }
        }
    }

    /// **The gain keys are the device's**: named by the stages it reported
    /// and its boost by its own name, a single-knob device offered one knob,
    /// and a key it cannot use not offered at all.
    #[test]
    fn the_gain_keys_are_named_by_the_model() {
        let amp = GainModel::new(vec![], "RF", "RF")
            .with_gauge_fallback(116)
            .with_boost(crate::hardware::Boost::Element(
                crate::hardware::StageSpec::ranged("AMP", 0.0, 14.0, 14.0),
            ));
        assert_eq!(
            radio_group(&amp, false, None),
            vec!["[Space] RX", "[F] Freq", "[S] Rate", "[↑↓] Gain", "[A] AMP"]
        );
        // Three stages the driver called LNA, TIA and PGA, one knob, and an
        // automatic gain mode: no VGA anywhere, and an AGC that is one.
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
        let items = radio_group(&lime, false, None).join(" ");
        assert!(
            items.contains("[↑↓] Gain") && items.contains("[A] AGC"),
            "{items}"
        );
        assert!(!items.contains("VGA") && !items.contains("LNA"), "{items}");
        let none = GainModel::new(vec![], "RF", "RF").with_gauge_fallback(45);
        assert!(!radio_group(&none, false, None).join(" ").contains("[A]"));
    }

    /// **The arrows move what `,` / `.` picked**: with a stage picked the
    /// footer names it on the arrows and beside the picking keys, and a radio
    /// with one stage is offered nothing to pick.
    #[test]
    fn a_picked_stage_is_what_the_footer_says_the_arrows_move() {
        let hackrf = radio_group(&hackrf::gain_model(), false, Some(1)).join(" ");
        assert!(hackrf.contains("[↑↓] VGA"), "{hackrf}");
        assert!(hackrf.contains("[, .] stage=VGA"), "{hackrf}");
        let rtl = radio_group(
            &crate::hardware::native::rtlsdr::gain_model(&[0, 10]),
            false,
            None,
        );
        assert!(!rtl.join(" ").contains("[, .]"), "{rtl:?}");
    }

    /// A power-trace device (a tinySA) takes a span, and has no gain keys here.
    #[test]
    fn power_trace_footer_keeps_only_supported_radio_controls() {
        let items = radio_group(&hackrf::gain_model(), true, None).join(" ");
        assert_eq!(items, "[Space] RX [F] Freq [S] Span");
    }

    /// **A group moves whole**: where two fit on a line they share it, and
    /// where they do not the second starts the next line rather than
    /// breaking in the middle.
    #[test]
    fn groups_wrap_whole() {
        let groups = vec![
            vec!["[A] aaaa".to_string(), "[B] bbbb".to_string()],
            vec!["[C] cccc".to_string(), "[D] dddd".to_string()],
        ];
        // "[A] aaaa · [B] bbbb" is 19; with the rule and the second, 43.
        assert_eq!(wrap_groups(&groups, 43).len(), 1);
        let two = wrap_groups(&groups, 30);
        assert_eq!(two.len(), 2);
        assert_eq!(two[1][0], (false, "[C] cccc".to_string()));
        // A group wider than the line breaks inside itself.
        assert_eq!(wrap_groups(&groups[..1], 10).len(), 2);
    }

    #[test]
    fn preset_label_abbreviates_when_narrow() {
        assert_eq!(preset_label("spectrum_waterfall", true), "spec+wf");
        assert_eq!(
            preset_label("spectrum_waterfall", false),
            "spectrum_waterfall"
        );
        assert_eq!(preset_label("lab_iq", true), "lab_iq");
    }

    /// A layout the menu does not list has no number keys, and names itself.
    #[test]
    fn a_layout_outside_the_menu_names_itself() {
        let groups = footer_at("spectrum_waterfall", "", Vec::new(), 50);
        assert_eq!(groups[groups.len() - 2], vec!["[P] spec+wf"]);
    }

    #[test]
    fn micro_preset_shows_the_working_keys_and_the_position() {
        let groups = footer_at("micro_main", "micro", micro_scope(), 120);
        assert_eq!(groups.len(), 1);
        let items = &groups[0];
        // The hint names the range of keys that work, not the retired [0] cycle.
        assert!(items.iter().any(|i| i == "[1-4]"), "{items:?}");
        assert!(items.iter().all(|i| !i.starts_with("[0]")), "{items:?}");
        assert!(items.iter().any(|i| i == "micro 1/5"));
        assert!(items.iter().all(|i| !i.starts_with("[P]")));
        // Named by the model, not a fixed LNA and VGA.
        assert!(items.contains(&"[↑↓]LNA".to_string()), "{items:?}");
    }

    #[test]
    fn micro_footer_narrow_is_more_compact() {
        let mut m = SdrMetrics::fixture();
        m.ui.active_preset = "micro_signal".to_string();
        m.ui.section = "micro".to_string();
        m.ui.scope = micro_scope();
        let items = &normal_groups(&m, 50)[0];
        assert!(items.iter().any(|i| i == "[1-4]"), "{items:?}");
        assert!(items.iter().any(|i| i == "2/5"));
    }

    /// The map names the keys the section actually has, with the current layout
    /// marked. It used to name `[5]`-`[8]` from a table of its own; those keys do
    /// nothing now, and a footer that names keys has to read the keys.
    #[test]
    fn the_section_map_uses_the_real_slots_with_the_active_one_marked() {
        assert_eq!(
            scope_map_items("lab_rf", &lab_scope()),
            vec!["[1] IQ", "[2]▸RF", "[3] Timing", "[4] Signal"]
        );
    }

    /// A layout with no slot has no number key, so the map stays quiet about it
    /// rather than inventing one.
    #[test]
    fn the_section_map_skips_a_layout_with_no_slot() {
        let scope = vec![
            (Some(1), "lab_iq".to_string(), "IQ".to_string()),
            (None, "mine".to_string(), "Mine".to_string()),
            (Some(2), "lab_rf".to_string(), "RF".to_string()),
        ];
        assert_eq!(scope_map_items("lab_iq", &scope), vec!["[1]▸IQ", "[2] RF"]);
    }

    /// The footer must not advertise a key that does nothing. `[?]` and `[0]`
    /// are both retired, and this is the surface that used to name them.
    #[test]
    fn the_footer_names_no_retired_key() {
        for (preset, section, scope) in [
            ("main", "command_rail", Vec::new()),
            ("lab_iq", "lab", lab_scope()),
            ("micro_main", "micro", micro_scope()),
        ] {
            for item in footer_at(preset, section, scope, 120).concat() {
                assert!(!item.contains("[?]"), "{preset}: {item}");
                assert!(!item.starts_with("[0]"), "{preset}: {item}");
            }
        }
    }
}
