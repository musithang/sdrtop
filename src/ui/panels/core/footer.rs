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

const NORMAL_ITEMS: &[&str] = &[
    "[Q] Quit",
    "[Space] RX",
    "[↑↓] LNA",
    "[[] VGA",
    "[A] AMP",
    "[F] Freq",
    "[S] Rate",
    "[R] Reset",
    "[?] Help",
    "[Tab] Hide",
];

/// Base normal-mode key hints, adapted to the device's gain model: HackRF shows
/// LNA/VGA/AMP; a single-tuner device (RTL-SDR) shows one gain + AGC, no VGA.
fn base_normal_items(gm: &GainModel) -> Vec<String> {
    if gm.is_single() {
        vec![
            "[Q] Quit".into(),
            "[Space] RX".into(),
            "[↑↓] Gain".into(),
            "[A] AGC".into(),
            "[F] Freq".into(),
            "[S] Rate".into(),
            "[R] Reset".into(),
            "[?] Help".into(),
            "[Tab] Hide".into(),
        ]
    } else {
        NORMAL_ITEMS.iter().map(|s| s.to_string()).collect()
    }
}

/// Width (terminal columns) below which the preset name is shown in short form.
const NARROW_COLS: u16 = 60;

/// The lab preset family, in reserved number-key order. The footer shows these
/// as a navigation map when one of them is the active preset; only the ones
/// that actually exist (synced into `preset_names`) are listed.
const LAB_FAMILY: &[(&str, &str)] = &[
    ("5", "lab_iq"),
    ("6", "lab_rf"),
    ("7", "lab_timing"),
    ("8", "lab_signal"),
];

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

/// Whether `name` belongs to the lab preset family.
fn is_lab_preset(name: &str) -> bool {
    LAB_FAMILY.iter().any(|(_, n)| *n == name)
}

/// Whether `name` is a micro ecosystem preset (entered via the `[0]` cycle).
fn is_micro_preset(name: &str) -> bool {
    name.starts_with("micro_")
}

/// Condensed footer for the micro ecosystem: the essential field keys plus the
/// `[0]▸{next}` hint and the `N/M` cycle position.
fn micro_items(view: MicroView, narrow: bool, gm: &GainModel) -> Vec<String> {
    // The sweep step is part of the [0] cycle.
    let sweep_active = true;
    let next = view.next(sweep_active);
    let total = MicroView::total(sweep_active);
    let pos = view.position();
    if narrow {
        vec![
            "[Q]".into(),
            "[Spc]".into(),
            "[↑↓]".into(),
            format!("[0]▸{}", next.label()),
            format!("{}/{}", pos, total),
        ]
    } else {
        let mut v: Vec<String> = vec!["[Q]".into(), "[Spc]RX".into()];
        if gm.is_single() {
            v.push("[↑↓]Gain".into());
        } else {
            v.push("[↑↓]LNA".into());
            v.push("[[]VGA".into());
        }
        v.push("[F]req".into());
        v.push(format!("[0]▸{}", next.label()));
        v.push(format!("micro {}/{}", pos, total));
        v
    }
}

/// Navigation map for the lab family: one entry per defined lab preset, with
/// the active one marked `▸`. Returns empty if none are available.
fn lab_map_items(active: &str, available: &[String]) -> Vec<String> {
    LAB_FAMILY
        .iter()
        .filter(|(_, name)| available.iter().any(|p| p == name))
        .map(|(key, name)| {
            if *name == active {
                format!("[{}]▸{}", key, name)
            } else {
                format!("[{}] {}", key, name)
            }
        })
        .collect()
}

/// The normal-mode footer items for the active preset:
/// - micro presets → a condensed field-key set with the `[0]` cycle hint;
/// - lab presets   → the fixed keys plus the lab navigation map;
/// - everything else → the fixed keys plus the `[P] {preset}` hint.
fn normal_items(
    active_preset: &str,
    available: &[String],
    micro_view: MicroView,
    available_width: u16,
    gm: &GainModel,
) -> Vec<String> {
    let narrow = available_width < NARROW_COLS;
    if is_micro_preset(active_preset) {
        return micro_items(micro_view, narrow, gm);
    }
    let mut items: Vec<String> = base_normal_items(gm);
    if is_lab_preset(active_preset) {
        items.extend(lab_map_items(active_preset, available));
    } else {
        items.push(format!("[P] {}", preset_label(active_preset, narrow)));
    }
    items
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

/// Break `items` into joined lines (used for height measurement).
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
                if i > 0 {
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

fn count_lines<S: AsRef<str>>(items: &[S], sep: &str, inner_w: usize) -> usize {
    wrap_items(items, sep, inner_w).len()
}

/// Public free function — called directly from the engine (bypasses dyn dispatch).
pub fn compute_footer_height(available_width: u16, state: &SdrMetrics) -> u16 {
    if !matches!(state.ui.input_mode, InputMode::Normal) || state.observer.active {
        return 3;
    }
    let inner_w = available_width.saturating_sub(2) as usize;
    let n = if state.ui.focused_panel.is_some() {
        count_lines(&focus_items(state), FOCUS_SEP, inner_w)
    } else {
        count_lines(
            &normal_items(
                &state.ui.active_preset,
                &state.ui.preset_names,
                state.ui.micro_view,
                available_width,
                &state.caps.gain,
            ),
            NORMAL_SEP,
            inner_w,
        )
    };
    (n as u16 + 2).min(MAX_CONTENT_LINES + 2).max(3)
}

pub struct FooterPanel;

impl Panel for FooterPanel {
    fn name(&self) -> &'static str {
        "footer"
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
                    "[?] Help".into(),
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
                    " Sample rate ({:.1}–{:.1} MHz): [{}▌]  [Enter] Confirm  [Esc] Cancel",
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
                InputMode::Normal => {
                    if let Some(panel_name) = &m.ui.focused_panel {
                        let items = focus_items(m);
                        let groups = wrap_items_grouped(&items, FOCUS_SEP, inner_w);
                        let mut wrapped = styled_lines(groups, FOCUS_SEP, theme, max_lines);
                        if let Some(last) = wrapped.last_mut() {
                            last.spans.push(Span::styled(
                                format!("  — {}", panel_name),
                                Style::default().fg(theme.label),
                            ));
                        }
                        wrapped
                    } else {
                        let items = normal_items(
                            &m.ui.active_preset,
                            &m.ui.preset_names,
                            m.ui.micro_view,
                            frame::outer_of(inner).width,
                            &m.caps.gain,
                        );
                        let groups = wrap_items_grouped(&items, NORMAL_SEP, inner_w);
                        styled_lines(groups, NORMAL_SEP, theme, max_lines)
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
        InputMode::Normal if panel_focused => FrameTone::Focused,
        InputMode::Normal => FrameTone::Dim,
        // Anything else is a half-typed value waiting on Enter.
        _ => FrameTone::Warn,
    }
}

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

    #[test]
    fn normal_items_wrap_at_80_cols() {
        let n = count_lines(NORMAL_ITEMS, NORMAL_SEP, 78);
        assert!(
            n >= 2,
            "normal items at inner_w=78 should need >=2 lines, got {}",
            n
        );
    }

    #[test]
    fn normal_items_fit_at_200_cols() {
        let n = count_lines(NORMAL_ITEMS, NORMAL_SEP, 198);
        assert_eq!(
            n, 1,
            "normal items at inner_w=198 should fit on 1 line, got {}",
            n
        );
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

    #[test]
    fn normal_items_appends_preset_entry() {
        let items = normal_items("main", &[], MicroView::Main, 120, &GainModel::HackRf);
        assert_eq!(items.last().map(String::as_str), Some("[P] main"));
        assert_eq!(items.len(), NORMAL_ITEMS.len() + 1);
    }

    #[test]
    fn normal_items_uses_short_preset_when_narrow() {
        let items = normal_items(
            "spectrum_waterfall",
            &[],
            MicroView::Main,
            50,
            &GainModel::HackRf,
        );
        assert_eq!(items.last().map(String::as_str), Some("[P] spec+wf"));
    }

    #[test]
    fn micro_preset_shows_condensed_footer_with_next_and_position() {
        // From micro_main (Main), the [0] hint points at the next view (signal)
        // and the position reads 1/5 (the cycle includes the sweep step).
        let items = normal_items("micro_main", &[], MicroView::Main, 120, &GainModel::HackRf);
        assert!(items.iter().any(|i| i == "[0]▸signal"));
        assert!(items.iter().any(|i| i == "micro 1/5"));
        // No [P] hint and none of the long normal items in micro mode.
        assert!(items.iter().all(|i| !i.starts_with("[P]")));
        assert!(!items.contains(&"[R] Reset".to_string()));
    }

    #[test]
    fn micro_footer_narrow_is_more_compact() {
        let items = normal_items(
            "micro_signal",
            &[],
            MicroView::Signal,
            50,
            &GainModel::HackRf,
        );
        assert!(items.iter().any(|i| i == "[0]▸gain"));
        assert!(items.iter().any(|i| i == "2/5"));
    }

    #[test]
    fn lab_map_lists_only_available_presets_with_active_marked() {
        let available = vec![
            "lab_iq".to_string(),
            "lab_rf".to_string(),
            "lab_signal".to_string(),
        ];
        let map = lab_map_items("lab_rf", &available);
        // lab_timing [7] is not available → excluded.
        assert_eq!(map, vec!["[5] lab_iq", "[6]▸lab_rf", "[8] lab_signal"]);
    }

    #[test]
    fn normal_items_shows_lab_map_in_lab_preset() {
        let available = vec!["lab_iq".to_string(), "lab_rf".to_string()];
        let items = normal_items(
            "lab_iq",
            &available,
            MicroView::Main,
            120,
            &GainModel::HackRf,
        );
        // No [P] entry in lab mode; the map entries are appended instead.
        assert!(items.iter().all(|i| !i.starts_with("[P]")));
        assert!(items.contains(&"[5]▸lab_iq".to_string()));
        assert!(items.contains(&"[6] lab_rf".to_string()));
    }
}
