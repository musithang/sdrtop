// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The menu: the app's launcher and its key reference.
//!
//! **Not a panel.** Full-screen UI in the family of [`crate::ui::overlay`] and
//! [`crate::ui::device_selector`], drawn outside the layout engine, into a
//! `Rect` the caller supplies: the whole screen at startup, a centred box over
//! the deck during a session. One function, two callers.
//!
//! Split by what each part draws, the way `panels/core/spectrum/` is:
//!
//! - [`model`]: presets to sections. The only part with logic, and the only part
//!   that never touches ratatui.
//! - [`sections`]: the left column.
//! - [`entries`]: the right column, a section's layouts.
//! - [`keys`]: the right column, the key reference.
//! - [`options`]: the right column, device settings.
//!
//! [`render`] is the orchestrator. It resolves the frame, carves the rows and
//! columns, and calls each part once. **The parts do not call each other.**

pub mod entries;
pub mod keys;
pub mod live;
pub mod model;
pub mod options;
pub mod sections;

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::state::{InputMode, MenuPane, MenuState, SdrMetrics};
use crate::ui::chrome;

use model::Menu;

/// Below this many columns the two columns stop fitting side by side, so the
/// left one folds away and the current section's title becomes the heading of
/// the single remaining column. The number keys are unaffected: they never
/// depended on the section list being visible.
const TWO_COLUMN_MIN: u16 = 50;

/// Width of the section column: "Command Rail" plus the cursor gutter and the
/// rule.
const LEFT_WIDTH: u16 = 16;

pub fn render(
    f: &mut Frame,
    area: Rect,
    m: &SdrMetrics,
    menu: &Menu,
    state: &MenuState,
    theme: &crate::Theme,
) {
    let block = chrome::deck_block(theme.border_accent).title(Line::from(chrome::nameplate(
        vec![Span::styled(
            format!(" sdrtop {} ", crate::cli::VERSION),
            Style::default()
                .fg(theme.value_hi)
                .add_modifier(Modifier::BOLD),
        )],
        theme.border_accent,
    )));
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    // Header, body, footer. The body wins when the terminal is too short for all
    // three: a menu with no list is not a menu.
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(inner);

    header(f, rows[0], m, theme);
    footer(f, rows[2], state, m, theme);

    // The cursor is cloned into the frame snapshot and arrives here without the
    // engine, so it is clamped rather than trusted. An out of range index would
    // panic during draw and take the terminal with it.
    let Some((si, ei)) = menu.clamp(state.section, state.entry) else {
        f.render_widget(
            Paragraph::new(Span::styled(
                "  no layouts are defined",
                Style::default().fg(theme.status_warn),
            )),
            rows[1],
        );
        return;
    };

    if rows[1].width >= TWO_COLUMN_MIN {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(LEFT_WIDTH), Constraint::Min(1)])
            .split(rows[1]);
        let row = sections::selected_row(menu, si, state.pane);
        sections::render(f, cols[0], menu, row, theme);
        right_pane(f, cols[1], m, menu, si, ei, state, false, theme);
    } else {
        right_pane(f, rows[1], m, menu, si, ei, state, true, theme);
    }
}

/// The one place that decides which pane the right column is showing. A new pane
/// is a variant here and a module beside `entries`, not a rewrite.
///
/// `folded` says whether the section list is off screen. It is passed in rather
/// than re-derived from `area.width`, because by the time this runs `area` is
/// only the *remaining* column: measuring that against the two-column threshold
/// put the folded form's heading on screen while the section list was still
/// beside it, at every terminal between roughly 66 and 82 columns.
#[allow(clippy::too_many_arguments)]
fn right_pane(
    f: &mut Frame,
    area: Rect,
    m: &SdrMetrics,
    menu: &Menu,
    section: usize,
    cursor: usize,
    state: &MenuState,
    folded: bool,
    theme: &crate::Theme,
) {
    // In the folded single-column form the pane names itself, because the list
    // that would otherwise name it is not on screen. The views always carry
    // their section's heading, in its colour (7.1).
    let (heading, body) = if folded || state.pane == MenuPane::Views {
        let split = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(1)])
            .split(area);
        (Some(split[0]), split[1])
    } else {
        (None, area)
    };
    let accent = sections::accent(section, theme);
    if let Some(heading) = heading {
        let line = match state.pane {
            MenuPane::Views => {
                let s = &menu.sections[section];
                let n = s.entries.len();
                let mut line = chrome::section(
                    &s.title,
                    &format!("{n} view{}", if n == 1 { "" } else { "s" }),
                    heading.width as usize,
                    theme,
                );
                // The section's name in the section's colour.
                if let Some(name) = line.spans.get_mut(1) {
                    name.style = name.style.fg(accent);
                }
                line
            }
            MenuPane::Keys => chrome::section("Keys", "", heading.width as usize, theme),
            MenuPane::Options => chrome::section("Options", "", heading.width as usize, theme),
        };
        f.render_widget(Paragraph::new(line), heading);
    }

    match state.pane {
        MenuPane::Views => {
            entries::render(f, body, m, &menu.sections[section], cursor, accent, theme)
        }
        MenuPane::Keys => keys::render(f, body, &m.caps, state.scroll, theme),
        MenuPane::Options => options::render(f, body, m, state.scroll, theme),
    }
}

/// Who you are and where the radio is pointing, so the menu is not a screen that
/// hides the one number you were watching.
fn header(f: &mut Frame, area: Rect, m: &SdrMetrics, theme: &crate::Theme) {
    // Whether the radio is streaming, beside where it is pointed: the live
    // lines under the views are only ever as live as this (7.2).
    let (dot, ink, word) = if m.radio.hw_streaming {
        ("\u{25cf}", theme.status_ok, "RX")
    } else {
        ("\u{25cb}", theme.stale, "stopped")
    };
    let line = Line::from(vec![
        Span::styled(
            format!(" {}", m.system.board_name),
            Style::default().fg(theme.value),
        ),
        Span::styled("   ", Style::default()),
        Span::styled(
            format!("{:.3} MHz", m.radio.frequency as f64 / 1e6),
            Style::default()
                .fg(theme.value_hi)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("   "),
        Span::styled(dot, Style::default().fg(ink)),
        Span::styled(format!(" {word}"), Style::default().fg(theme.label)),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

/// Show the keys for the active menu pane or editor
fn footer(f: &mut Frame, area: Rect, state: &MenuState, m: &SdrMetrics, theme: &crate::Theme) {
    let key = Style::default().fg(theme.border_accent);
    let what = Style::default().fg(theme.label);
    let mut spans = Vec::new();
    let numeric = m
        .device_options
        .get(state.scroll)
        .is_some_and(|option| option.integer_range.is_some());
    let bindings: &[(&str, &str)] =
        if matches!(m.ui.input_mode, InputMode::DeviceOptionInput { .. }) {
            &[("Enter", "apply"), ("Esc", "cancel")]
        } else if state.pane == MenuPane::Options && numeric {
            &[
                ("Enter", "number"),
                ("\u{2190}\u{2192}", "all choices"),
                ("Esc", "close"),
                ("Tab", "section"),
                ("\u{2191}\u{2193}", "option"),
            ]
        } else if state.pane == MenuPane::Options && !m.device_options.is_empty() {
            &[
                ("Tab", "section"),
                ("\u{2191}\u{2193}", "option"),
                ("\u{2190}\u{2192}", "value"),
                ("Enter", "next"),
                ("Esc", "close"),
            ]
        } else {
            &[
                ("Tab", "section"),
                ("\u{2191}\u{2193}", "move"),
                ("1-9", "open"),
                ("Enter", "open"),
                ("Esc", "close"),
            ]
        };
    for (k, w) in bindings {
        spans.push(Span::styled(format!(" {k} "), key));
        spans.push(Span::styled(*w, what));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LayoutConfig;
    use ratatui::{backend::TestBackend, Terminal};
    use std::sync::Arc;

    /// Render the menu at a fixed size and hand back the buffer as lines.
    ///
    /// The same idea as `state::fixture::draw`, but without the panel registry:
    /// the menu is not a panel, so it cannot go through
    /// `PanelRegistry::render_panel` and needs its own harness.
    fn draw(w: u16, h: u16, state: &MenuState) -> Vec<String> {
        draw_with_metrics(w, h, state, &SdrMetrics::fixture())
    }

    fn draw_with_metrics(w: u16, h: u16, state: &MenuState, metrics: &SdrMetrics) -> Vec<String> {
        let menu = model::build(&LayoutConfig::default_config().presets);
        let theme = crate::Theme::sdr();
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal
            .draw(|f| render(f, f.size(), metrics, &menu, state, &theme))
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| buf.get(x, y).symbol().to_string())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    fn at(section: usize, entry: usize) -> MenuState {
        MenuState {
            section,
            entry,
            pane: MenuPane::Views,
            scroll: 0,
        }
    }

    #[test]
    fn both_columns_are_drawn() {
        let all = draw(80, 24, &at(1, 0)).join("\n");
        for wanted in ["Command Rail", "Lab", "Sweep", "Micro", "IQ", "Timing"] {
            assert!(all.contains(wanted), "'{wanted}' missing from:\n{all}");
        }
    }

    /// The number beside a view is its slot, which is the key that selects it.
    /// If these ever disagree the menu is teaching a key that does not work.
    ///
    /// Matched as the rendered pair `"2  RF"` rather than "some line mentioning
    /// RF": the header says `HackRF One`, which contains `RF` and satisfied a
    /// looser version of this test for the wrong reason.
    #[test]
    fn the_numbers_shown_are_the_keys_that_work() {
        let all = draw(80, 24, &at(1, 0));
        let joined = all.join("\n");
        for (slot, title) in [(1, "IQ"), (2, "RF"), (3, "Timing"), (4, "Signal")] {
            let pair = format!("{slot}  {title}");
            assert!(
                all.iter().any(|l| l.contains(&pair)),
                "expected '{pair}' on a row, in:\n{joined}"
            );
        }
    }

    /// The header keeps the tuned frequency on screen, so opening the menu does
    /// not hide the one number you were watching.
    #[test]
    fn the_header_shows_the_device_and_the_frequency() {
        let all = draw(80, 24, &at(0, 0)).join("\n");
        assert!(all.contains("100.000 MHz"), "{all}");
    }

    /// A narrow terminal folds to one column instead of spilling, and names the
    /// section it is showing, since the list that would name it is gone.
    #[test]
    fn a_narrow_terminal_folds_to_one_column() {
        let all = draw(44, 16, &at(1, 0));
        let joined = all.join("\n");
        assert!(all.iter().all(|l| l.chars().count() <= 44), "{joined}");
        assert!(joined.contains("IQ"), "the views must survive:\n{joined}");
        assert!(
            joined.contains("LAB"),
            "the folded form must name its section:\n{joined}"
        );
    }

    /// The fold is decided by the whole width, not the remaining column.
    ///
    /// Found by running the real binary at 70 columns: the menu still had
    /// both columns while the right one, being only the remainder, measured
    /// below the threshold and drew the folded form's heading beside the list.
    /// The views now carry their section's heading in both forms (7.1), so
    /// what is checked is the fold itself, across that gap: the list on
    /// screen at every width from 56, gone below the threshold, and the
    /// heading naming the section either way.
    #[test]
    fn the_fold_follows_the_whole_width_and_the_heading_names_the_section() {
        for w in [56, 60, 70, 80, 120] {
            let all = draw(w, 20, &at(0, 0)).join("\n");
            assert!(
                all.contains("Command Rail"),
                "the section list must be on screen at {w}:\n{all}"
            );
            assert!(all.contains("COMMAND RAIL"), "no heading at {w}:\n{all}");
        }
        let folded = draw(44, 16, &at(0, 0)).join("\n");
        assert!(!folded.contains("Command Rail"), "{folded}");
        assert!(folded.contains("COMMAND RAIL"), "{folded}");
    }

    /// **Every NET view carries a live line** (7.2), `●` for the view
    /// running now and `○` with the session's figures otherwise; a section
    /// with nothing live keeps its two rows a view.
    #[test]
    fn net_views_carry_a_live_line() {
        let menu = model::build(&LayoutConfig::default_config().presets);
        let net = menu.sections.iter().position(|s| s.id == "net").unwrap();
        let mut m = SdrMetrics::fixture().streaming();
        m.ui.active_preset = "net_ble".to_string();
        m.net.ble_channel = Some(37);
        m.net.ble_channel_packets = [300, 100, 12];
        m.net.ble_channel_crc_ok = [290, 95, 11];
        let all = draw_with_metrics(100, 30, &at(net, 3), &m).join("\n");
        assert!(
            all.contains("\u{25cf} 412 packets this session, 96 % CRC ok"),
            "{all}"
        );
        assert!(
            all.contains("\u{25cb} no piconet heard this session; not listening now"),
            "{all}"
        );
        assert!(all.contains("NET"), "{all}");
        assert!(all.contains("5 views"), "{all}");
        // The selected view wears the bar, not a triangle.
        assert!(all.contains("\u{258c} 4  BLE"), "{all}");
        assert!(!all.contains("\u{25b8}"), "{all}");
        let lab = draw(100, 30, &at(1, 0)).join("\n");
        // Only the header's own stopped mark; no view line.
        assert_eq!(lab.matches('\u{25cb}').count(), 1, "{lab}");
        assert!(!lab.contains("not listening"), "{lab}");
    }

    /// The menu fits every width either side of the fold.
    #[test]
    fn the_menu_fits_every_width() {
        let menu = model::build(&LayoutConfig::default_config().presets);
        let net = menu.sections.iter().position(|s| s.id == "net").unwrap();
        for w in 30..130u16 {
            for h in [12u16, 20, 30] {
                for line in draw(w, h, &at(net, 4)) {
                    assert!(line.chars().count() <= w as usize, "{w}x{h}: {line:?}");
                }
            }
        }
    }

    /// The Keys pane is a row in the left column and the content of the right
    /// one, and it replaces an overlay that had drifted out of step with the
    /// dispatch. `keys.rs` owns the check that it cannot drift again.
    #[test]
    fn the_keys_pane_lists_the_global_keys() {
        let state = MenuState {
            section: 0,
            entry: 0,
            pane: MenuPane::Keys,
            scroll: 0,
        };
        let all = draw(90, 30, &state).join("\n");
        assert!(
            all.contains("Keys"),
            "the left column needs the row:\n{all}"
        );
        assert!(all.contains("start or stop RX"), "{all}");
        assert!(all.contains("type a frequency"), "{all}");
        // The section list is still there: the pane replaces the right column,
        // not the whole menu.
        assert!(all.contains("Command Rail"), "{all}");
    }

    /// The reference is taller than a short terminal, so it scrolls, and the
    /// scroll actually moves the content rather than being stored and ignored.
    #[test]
    fn the_keys_pane_scrolls() {
        let top = draw(
            90,
            18,
            &MenuState {
                section: 0,
                entry: 0,
                pane: MenuPane::Keys,
                scroll: 0,
            },
        )
        .join("\n");
        let down = draw(
            90,
            18,
            &MenuState {
                section: 0,
                entry: 0,
                pane: MenuPane::Keys,
                scroll: 8,
            },
        )
        .join("\n");
        assert!(top.contains("start or stop RX"), "{top}");
        assert!(!down.contains("start or stop RX"), "scrolled away:\n{down}");
        assert_ne!(top, down);
    }

    /// The Options pane is empty by design, so what is being pinned is that the
    /// emptiness is stated on screen. A pane that opens to blank space reads as
    /// a bug; one that says why it is blank reads as a decision.
    #[test]
    fn the_options_pane_admits_it_is_empty() {
        let state = MenuState {
            section: 0,
            entry: 0,
            pane: MenuPane::Options,
            scroll: 0,
        };
        let all = draw(90, 30, &state).join("\n");
        assert!(
            all.contains("Options"),
            "the left column needs the row:\n{all}"
        );
        assert!(all.contains("Settings will live here"), "{all}");
        // And the pane replaces the right column only, the same as Keys.
        assert!(all.contains("Command Rail"), "{all}");
    }

    #[test]
    fn a_short_folded_options_pane_keeps_the_selected_option_visible() {
        let state = MenuState {
            pane: MenuPane::Options,
            scroll: 5,
            ..MenuState::default()
        };
        let mut metrics = SdrMetrics::fixture();
        for index in 0..6 {
            Arc::make_mut(&mut metrics.device_options).push(crate::hardware::DeviceOption {
                id: format!("option-{index}"),
                label: format!("Option {index}"),
                choices: vec!["Off".into(), "On".into()],
                selected_choice: "On".into(),
                integer_range: None,
            });
        }

        let all = draw_with_metrics(40, 10, &state, &metrics).join("\n");
        assert!(all.contains("Option 5"), "{all}");
    }

    #[test]
    fn numeric_options_show_entry_hints_and_keep_the_accepted_value_in_the_editor() {
        let state = MenuState {
            pane: MenuPane::Options,
            ..MenuState::default()
        };
        let mut m = SdrMetrics::fixture();
        m.device_options = Arc::new(vec![crate::hardware::DeviceOption {
            id: "gain".into(),
            label: "Gain".into(),
            choices: vec!["0".into(), "-12".into()],
            selected_choice: "0".into(),
            integer_range: Some(-100..=100),
        }]);
        for (w, h) in [(40, 10), (90, 24)] {
            let all = draw_with_metrics(w, h, &state, &m).join("\n");
            assert!(all.contains("Enter number"), "{all}");
            assert!(all.contains("all choices"), "{all}");
        }
        m.ui.input_mode = InputMode::DeviceOptionInput {
            id: "gain".into(),
            error: None,
        };
        m.ui.input_buf = "-12".into();
        for (w, h) in [(40, 10), (90, 24)] {
            let all = draw_with_metrics(w, h, &state, &m).join("\n");
            for text in [
                "Gain: 0",
                "Value: -12\u{258c}",
                "Integer -100 to 100",
                "Enter apply",
                "Esc cancel",
            ] {
                assert!(all.contains(text), "'{text}' missing:\n{all}");
            }
            assert!(!all.contains("Enter number"), "{all}");
        }
        m.ui.input_mode = InputMode::DeviceOptionInput {
            id: "gain".into(),
            error: Some("Out of range: -100 to 100".into()),
        };
        for (w, h) in [(40, 6), (40, 7), (40, 8), (90, 5), (90, 6), (90, 7)] {
            let all = draw_with_metrics(w, h, &state, &m).join("\n");
            for text in ["Value: -12\u{258c}", "Enter apply", "Esc cancel"] {
                assert!(all.contains(text), "'{text}' missing:\n{all}");
            }
            if (w == 40 && h >= 7) || (w == 90 && h >= 6) {
                assert!(all.contains("Out of range:"), "{all}");
            }
        }
    }

    /// Small enough that nothing sensible fits. The requirement is only that it
    /// does not panic and does not draw outside its area.
    #[test]
    fn it_survives_a_tiny_terminal() {
        for (w, h) in [(40, 10), (20, 5), (8, 3), (4, 2)] {
            let lines = draw(w, h, &at(3, 3));
            assert_eq!(lines.len(), h as usize);
            assert!(lines.iter().all(|l| l.chars().count() <= w as usize));
        }
    }

    /// An out of range cursor is clamped, not indexed. `MenuState` is cloned into
    /// the frame snapshot and arrives here without the engine that built the
    /// table, so this is the path that must not panic.
    #[test]
    fn an_out_of_range_cursor_does_not_panic() {
        let all = draw(80, 24, &at(99, 99)).join("\n");
        assert!(all.contains("Micro"), "clamps to the last section:\n{all}");
    }
}
