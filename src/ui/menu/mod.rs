// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The menu: the app's launcher and its key reference.
//!
//! **Not a panel.** Full-screen UI in the family of [`crate::ui::overlay`] and
//! [`crate::ui::device_selector`], drawn outside the layout engine, into a
//! `Rect` the caller supplies: the whole screen at startup, a centred box over
//! the deck during a session. One function, two callers.
//!
//! **Tiles above, what they open below.** A row of square tiles (Resume, Radio,
//! Lab, Options, Keys, Quit); under Radio and Lab a cascade of columns down a
//! tree sorted by where it listens, with a card saying what the row under the
//! cursor is; under Options, Keys and Quit their pane.
//!
//! Split by what each part draws, the way `panels/core/spectrum/` is:
//!
//! - [`model`]: presets to sections. Pure, and still what a number key counts in.
//! - [`tree`]: the sections hung on the tiles' trees, the refusals beside them.
//!   Pure, as `model` is.
//! - [`tiles`]: the top row.
//! - [`crumb`]: the whole path, over the cascade.
//! - [`cascade`]: the columns down the path.
//! - [`card`]: what the row under the cursor is; it draws [`ruler`] and
//!   [`map`].
//! - [`keys`]: the key reference, under Keys.
//! - [`options`]: device settings, under Options.
//! - [`ink`]: a tree row's colour role on the theme.
//! - [`live`]: the live line a view's card carries.
//!
//! [`render`] is the orchestrator. It resolves the frame, carves the rows, and
//! calls each part once. **The parts do not call each other.**

pub mod card;
pub mod cascade;
pub mod crumb;
pub mod ink;
pub mod keys;
pub mod live;
pub mod map;
pub mod model;
pub mod options;
pub mod ruler;
pub mod tiles;
pub mod tree;

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::state::{InputMode, MenuFocus, MenuState, MenuTile, SdrMetrics};
use crate::ui::chrome;
use crate::ui::LayoutEngine;

/// Rows the tile row leaves the menu below it at least, before it shrinks.
const BELOW_TILES: u16 = 18;

/// A body shorter than this has no tile row at all: what the selected tile
/// opens keeps every row, so the options editor stays on screen in a
/// terminal a few rows tall.
const TILES_MIN_BODY: u16 = 8;

/// The tree a tile opens, if it opens one.
pub fn tree_of(tile: MenuTile) -> Option<tree::Tile> {
    match tile {
        MenuTile::Radio => Some(tree::Tile::Radio),
        MenuTile::Lab | MenuTile::Resume => Some(tree::Tile::Lab),
        _ => None,
    }
}

/// The path the menu stands on in `tile`'s tree, clamped onto a real row.
pub fn path_in(engine: &LayoutEngine, state: &MenuState, tile: tree::Tile) -> Option<tree::Path> {
    let raw = match tile {
        tree::Tile::Radio => state.radio,
        tree::Tile::Lab => state.lab,
    };
    engine.menu_tree().clamp(tile, &raw.into())
}

/// The menu as it opens: on Resume when there is a view to resume and the
/// deck is on one the menu lists, on Lab otherwise, the focus on the tiles,
/// and the Lab tree already standing on the deck's view, so `Enter`
/// resumes and `↓` goes in exactly there.
pub fn opening(engine: &LayoutEngine, resume: bool) -> MenuState {
    let located = engine.menu_tree().locate(engine.active_preset());
    let resume = resume && located.is_some();
    MenuState {
        tile: if resume {
            MenuTile::Resume
        } else {
            MenuTile::Lab
        },
        focus: MenuFocus::Tiles,
        lab: located
            .as_ref()
            .map(|(_, path)| path.into())
            .unwrap_or_default(),
        resume,
        ..MenuState::default()
    }
}

/// The menu section the Keys pane is about: the group the Lab path stands
/// in, else the deck's own, else the first.
pub fn keys_section(engine: &LayoutEngine, state: &MenuState) -> usize {
    let tree = engine.menu_tree();
    let sections = &engine.menu().sections;
    path_in(engine, state, tree::Tile::Lab)
        .and_then(|p| tree.section_at(tree::Tile::Lab, &p))
        .and_then(|id| sections.iter().position(|s| s.id == id))
        .or_else(|| engine.menu().locate(engine.active_preset()).map(|(s, _)| s))
        .unwrap_or(0)
}

/// What the Resume tile says under its name: where the deck is.
fn resume_line(engine: &LayoutEngine) -> String {
    let tree = engine.menu_tree();
    let Some((tile, path)) = tree.locate(engine.active_preset()) else {
        return String::new();
    };
    let trail = tree.trail(tile, &path);
    match (trail.first(), trail.last()) {
        (Some(area), Some(view)) => format!("{} · {}", area.title, view.title),
        _ => String::new(),
    }
}

/// `controls` are the focus keys of the section [`keys_section`] names
/// (`ui::LayoutEngine::section_controls`), for the Keys pane.
pub fn render(
    f: &mut Frame,
    area: Rect,
    m: &SdrMetrics,
    engine: &LayoutEngine,
    state: &MenuState,
    controls: &[keys::Control],
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

    // Header, body, footer. The body wins when the terminal is too short for
    // all three: a menu with nothing to choose is not a menu.
    let header_h = u16::from(inner.height >= 3);
    let footer_h = u16::from(inner.height >= 3);
    header(
        f,
        Rect {
            height: header_h,
            ..inner
        },
        m,
        theme,
    );
    footer(
        f,
        Rect {
            y: inner.bottom().saturating_sub(footer_h),
            height: footer_h,
            ..inner
        },
        state,
        m,
        theme,
    );
    let mut body = Rect {
        y: inner.y + header_h,
        height: inner.height.saturating_sub(header_h + footer_h),
        ..inner
    };

    let faces = MenuTile::shown(state.resume);
    let on_tiles = state.focus == MenuFocus::Tiles;
    if body.height >= TILES_MIN_BODY {
        let size = tiles::Size::for_area(body.width, body.height, faces.len(), BELOW_TILES);
        let row = Rect {
            y: body.y + 1,
            height: size.height(),
            ..body
        };
        tiles::render(
            f,
            row,
            size,
            &faces,
            state.tile,
            on_tiles,
            &resume_line(engine),
            theme,
        );
        let used = 1 + size.height() + 1;
        body = Rect {
            y: body.y + used,
            height: body.height.saturating_sub(used),
            ..body
        };
    }
    let body = Rect {
        x: body.x + 1,
        width: body.width.saturating_sub(2),
        ..body
    };
    if body.height == 0 || body.width == 0 {
        return;
    }

    match tree_of(state.tile) {
        Some(tile) => below_tree(f, body, m, engine, state, tile, theme),
        None => below_pane(f, body, m, engine, state, controls, theme),
    }
}

/// Under Radio, Lab and Resume: the crumb, the columns and the card.
fn below_tree(
    f: &mut Frame,
    area: Rect,
    m: &SdrMetrics,
    engine: &LayoutEngine,
    state: &MenuState,
    tile: tree::Tile,
    theme: &crate::Theme,
) {
    let tree = engine.menu_tree();
    let path = path_in(engine, state, tile).unwrap_or_default();
    let trail = tree.trail(tile, &path);
    let (name, color) = match tile {
        tree::Tile::Lab => ("Lab", theme.value_hi),
        tree::Tile::Radio => ("Radio", theme.border_accent),
    };
    let views: usize = tree.root(tile).iter().map(tree::Node::view_count).sum();
    let hint = match views {
        0 => "later".to_string(),
        1 => "1 view".to_string(),
        n => format!("{n} views"),
    };
    crumb::render(
        f,
        Rect { height: 1, ..area },
        (name, color),
        &trail,
        &hint,
        theme,
    );
    let columns = Rect {
        y: area.y + 2,
        height: area.height.saturating_sub(2),
        ..area
    };
    if columns.height == 0 {
        return;
    }
    let cursor = state.focus == MenuFocus::Below;
    let card_area = cascade::render(f, columns, tree, tile, &path, cursor, theme);

    let screen = f.size();
    let view = trail.last().filter(|n| n.opens()).and_then(|n| n.view());
    let placed = view
        .map(|e| engine.placement_of(&e.preset, screen, m))
        .unwrap_or_default();
    let live = view.and_then(|e| live::line(&e.preset, m, std::time::Instant::now()));
    let left_off = (state.tile == MenuTile::Resume).then(|| {
        format!(
            "{:.3} MHz · {:.3} Msps",
            m.radio.frequency as f64 / 1e6,
            m.radio.config_sample_rate / 1e6
        )
    });
    let card = card::Card {
        trail: &trail,
        radio: (
            &m.system.board_name,
            m.caps.freq_min_hz,
            m.caps.freq_max_hz,
            &m.caps.freq_gaps,
        ),
        live,
        placed: &placed,
        screen,
        left_off: left_off.as_deref(),
    };
    card::render(f, card_area, &card, theme);
}

/// Under Options, Keys and Quit: a heading naming the pane, then the pane.
fn below_pane(
    f: &mut Frame,
    area: Rect,
    m: &SdrMetrics,
    engine: &LayoutEngine,
    state: &MenuState,
    controls: &[keys::Control],
    theme: &crate::Theme,
) {
    let (title, hint) = match state.tile {
        MenuTile::Options => ("Options", "this radio, and where you listen from"),
        MenuTile::Keys => ("Keys", "every key, the deck's own first"),
        _ => ("Quit", ""),
    };
    // Below a few rows the heading gives way: the pane's own rows matter
    // more than its name, which the tile row or the footer already says.
    let (heading, pane) = if area.height >= 5 {
        (
            Some(Rect { height: 1, ..area }),
            Rect {
                y: area.y + 2,
                height: area.height - 2,
                ..area
            },
        )
    } else {
        (None, area)
    };
    if let Some(heading) = heading {
        f.render_widget(
            Paragraph::new(chrome::section(title, hint, heading.width as usize, theme)),
            heading,
        );
    }
    match state.tile {
        MenuTile::Options => options::render(f, pane, m, state.scroll, theme),
        MenuTile::Keys => {
            let section = &engine.menu().sections;
            let name = section
                .get(keys_section(engine, state))
                .map_or("", |s| s.title.as_str());
            keys::render(f, pane, &m.caps, name, controls, state.scroll, theme)
        }
        _ => {
            let lines = vec![
                Line::from(Span::styled(
                    "Enter closes sdrtop.",
                    Style::default().fg(theme.value),
                )),
                Line::from(Span::styled(
                    "The tuning, the gains and the view you are on are saved on the way out.",
                    Style::default().fg(theme.label),
                )),
            ];
            f.render_widget(Paragraph::new(lines), pane);
        }
    }
}

/// Who you are and where the radio is pointing, so the menu is not a screen that
/// hides the one number you were watching.
fn header(f: &mut Frame, area: Rect, m: &SdrMetrics, theme: &crate::Theme) {
    if area.height == 0 {
        return;
    }
    // Whether the radio is streaming, beside where it is pointed: a card's
    // live line is only ever as live as this.
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

/// The keys that work where the focus is.
fn footer(f: &mut Frame, area: Rect, state: &MenuState, m: &SdrMetrics, theme: &crate::Theme) {
    if area.height == 0 {
        return;
    }
    let key = Style::default().fg(theme.border_accent);
    let what = Style::default().fg(theme.label);
    let mut spans = Vec::new();
    let numeric = m
        .device_options
        .get(state.scroll)
        .is_some_and(|option| option.integer_range.is_some());
    let below = state.focus == MenuFocus::Below;
    let bindings: &[(&str, &str)] =
        if matches!(m.ui.input_mode, InputMode::DeviceOptionInput { .. }) {
            &[("Enter", "apply"), ("Esc", "cancel")]
        } else if !below {
            match state.tile {
                MenuTile::Resume => &[
                    ("Enter", "resume"),
                    ("\u{2190}\u{2192}", "choose"),
                    ("\u{2193}", "go in"),
                    ("R L O K Q", "jump"),
                    ("Esc", "close"),
                ],
                MenuTile::Quit => &[
                    ("Enter", "quit"),
                    ("\u{2190}\u{2192}", "choose"),
                    ("Esc", "close"),
                ],
                _ => &[
                    ("\u{2190}\u{2192}", "choose"),
                    ("\u{2193} Enter", "go in"),
                    ("R L O K Q", "jump"),
                    ("Esc", "close"),
                ],
            }
        } else if state.tile == MenuTile::Options && numeric {
            &[
                ("Enter", "number"),
                ("\u{2190}\u{2192}", "all choices"),
                ("Esc", "close"),
                ("Tab", "tiles"),
                ("\u{2191}\u{2193}", "option"),
            ]
        } else if state.tile == MenuTile::Options && state.scroll >= m.device_options.len() {
            &[
                ("\u{2191}\u{2193}", "setting"),
                ("Enter", "type"),
                ("Tab", "tiles"),
                ("Esc", "close"),
            ]
        } else if state.tile == MenuTile::Options {
            &[
                ("Tab", "tiles"),
                ("\u{2191}\u{2193}", "option"),
                ("\u{2190}\u{2192}", "value"),
                ("Enter", "next"),
                ("Esc", "close"),
            ]
        } else if matches!(state.tile, MenuTile::Keys) {
            &[
                ("\u{2191}\u{2193}", "scroll"),
                ("Tab", "tiles"),
                ("Esc", "close"),
            ]
        } else {
            &[
                ("\u{2191}\u{2193}", "move"),
                ("\u{2192} Enter", "open"),
                ("\u{2190}", "back"),
                ("1-9", "open by number"),
                ("Tab", "tiles"),
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
    use crate::state::MenuPath;
    use ratatui::{backend::TestBackend, Terminal};
    use std::sync::Arc;

    fn engine() -> LayoutEngine {
        crate::app::App::build_ui(
            "command_rail",
            &std::collections::HashMap::new(),
            None,
            true,
        )
        .0
    }

    /// Render the menu at a fixed size and hand back the buffer as lines.
    ///
    /// The same idea as `state::fixture::draw`, but without the panel registry:
    /// the menu is not a panel, so it cannot go through
    /// `PanelRegistry::render_panel` and needs its own harness.
    fn draw(w: u16, h: u16, state: &MenuState) -> Vec<String> {
        draw_with_metrics(w, h, state, &SdrMetrics::fixture())
    }

    fn draw_with_metrics(w: u16, h: u16, state: &MenuState, m: &SdrMetrics) -> Vec<String> {
        draw_on(&engine(), w, h, state, m)
    }

    fn draw_on(
        engine: &LayoutEngine,
        w: u16,
        h: u16,
        state: &MenuState,
        m: &SdrMetrics,
    ) -> Vec<String> {
        let theme = crate::Theme::sdr();
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        let controls = engine.section_controls(keys_section(engine, state), m);
        terminal
            .draw(|f| render(f, f.size(), m, engine, state, &controls, &theme))
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

    /// The Lab path to the rows titled `want`, as the state holds it.
    fn lab(want: &[&str]) -> MenuPath {
        let e = engine();
        let tree = e.menu_tree();
        let mut rows = tree.root(tree::Tile::Lab);
        let mut path = Vec::new();
        for name in want {
            let i = rows
                .iter()
                .position(|n| n.is_stop() && n.title == *name)
                .unwrap_or_else(|| panic!("{name}"));
            path.push(i);
            rows = rows[i].children();
        }
        MenuPath::from_rows(&path)
    }

    fn in_lab(want: &[&str]) -> MenuState {
        MenuState {
            tile: MenuTile::Lab,
            focus: MenuFocus::Below,
            lab: lab(want),
            resume: true,
            ..MenuState::default()
        }
    }

    fn pane(tile: MenuTile, scroll: usize) -> MenuState {
        MenuState {
            tile,
            focus: MenuFocus::Below,
            scroll,
            ..MenuState::default()
        }
    }

    /// The tiles, the crumb, every column down the path and the card.
    #[test]
    fn the_whole_menu_is_drawn() {
        let all = draw(191, 41, &in_lab(&["Wi-Fi", "2.4 GHz", "Channel", "Frames"])).join("\n");
        for wanted in [
            "Resume",
            "Radio",
            "Lab",
            "Options",
            "Keys",
            "Quit",
            "── LAB › Wi-Fi › 2.4 GHz › Channel › Frames",
            "AREA",
            "BAND",
            "GROUP",
            "VIEW",
            "Enter or 2 opens it",
            "WHERE IT LISTENS",
            "WHAT OPENS",
        ] {
            assert!(all.contains(wanted), "'{wanted}' missing from:\n{all}");
        }
    }

    /// No Resume tile without something to resume.
    #[test]
    fn a_first_run_has_no_resume_tile() {
        let first = MenuState {
            resume: false,
            ..in_lab(&["Spectrum"])
        };
        let all = draw(191, 41, &first).join("\n");
        assert!(!all.contains("Resume"), "{all}");
        assert!(draw(191, 41, &in_lab(&["Spectrum"]))
            .join("\n")
            .contains("Resume"));
    }

    /// Resume says where the deck is, and its card where you left off.
    #[test]
    fn resume_says_where_you_left_off() {
        let mut e = engine();
        e.set_preset("wifi_frames");
        let state = MenuState {
            tile: MenuTile::Resume,
            focus: MenuFocus::Tiles,
            lab: lab(&["Wi-Fi", "2.4 GHz", "Channel", "Frames"]),
            resume: true,
            ..MenuState::default()
        };
        let all = draw_on(&e, 191, 41, &state, &SdrMetrics::fixture()).join("\n");
        assert!(all.contains("Wi-Fi · Frames"), "{all}");
        assert!(all.contains("WHERE YOU LEFT OFF"), "{all}");
        assert!(all.contains("100.000 MHz ·"), "{all}");
    }

    /// The number beside a view is its slot, which is the key that selects it.
    /// If these ever disagree the menu is teaching a key that does not work.
    #[test]
    fn the_numbers_shown_are_the_keys_that_work() {
        let all = draw(191, 41, &in_lab(&["Receiver", "IQ balance"]));
        let joined = all.join("\n");
        for (slot, title) in [
            (1, "IQ balance"),
            (2, "Gain & noise"),
            (3, "USB & timing"),
            (4, "Signal quality"),
        ] {
            let pair = format!("{slot} {title}");
            assert!(
                all.iter().any(|l| l.contains(&pair)),
                "'{pair}' in:\n{joined}"
            );
        }
    }

    /// The header keeps the tuned frequency on screen, so opening the menu does
    /// not hide the one number you were watching.
    #[test]
    fn the_header_shows_the_device_and_the_frequency() {
        let all = draw(80, 24, &in_lab(&["Spectrum"])).join("\n");
        assert!(
            all.contains("HackRF One") && all.contains("100.000 MHz"),
            "{all}"
        );
    }

    /// A view's card carries its live line: `●` when the view's receiver runs
    /// now, `○` with the session's figures otherwise.
    #[test]
    fn a_views_card_carries_its_live_line() {
        let mut m = SdrMetrics::fixture().streaming();
        m.ui.active_preset = "net_ble".to_string();
        m.net.ble_channel = Some(37);
        m.net.ble_channel_packets = [300, 100, 12];
        m.net.ble_channel_crc_ok = [290, 95, 11];
        let state = in_lab(&["Bluetooth", "2.4 GHz", "Low Energy", "Advertising"]);
        let le = draw_with_metrics(191, 41, &state, &m).join("\n");
        assert!(
            le.contains("\u{25cf} 412 packets this session, 96 % CRC ok"),
            "{le}"
        );
        let state = in_lab(&["Bluetooth", "2.4 GHz", "Classic", "Piconets nearby"]);
        let classic = draw_with_metrics(191, 41, &state, &m).join("\n");
        assert!(
            classic.contains("\u{25cb} no piconet heard this session; not listening now"),
            "{classic}"
        );
    }

    /// On a radio the gate refuses, Bluetooth and Wi-Fi are on the menu, grey,
    /// and the card says why in the gate's words.
    #[test]
    fn a_refused_view_is_shown_with_its_reason() {
        let refused = crate::app::App::build_ui(
            "command_rail",
            &std::collections::HashMap::new(),
            None,
            false,
        )
        .0;
        let state = MenuState {
            tile: MenuTile::Lab,
            focus: MenuFocus::Below,
            lab: {
                let tree = refused.menu_tree();
                let (_, path) = tree.locate("wifi_frames").expect("listed");
                (&path).into()
            },
            resume: true,
            ..MenuState::default()
        };
        let all = draw_on(&refused, 191, 41, &state, &SdrMetrics::fixture()).join("\n");
        assert!(all.contains("✗ Not on this radio"), "{all}");
        assert!(all.contains("This radio cannot reach the band."), "{all}");
        assert!(all.contains("refused"), "{all}");
        assert!(!all.contains("opens it"), "{all}");
        assert!(
            !all.contains("WHAT OPENS"),
            "a refused view has no placement:\n{all}"
        );
    }

    /// The menu fits every width and height, and never panics drawing them.
    #[test]
    fn the_menu_fits_every_size() {
        let states = [
            in_lab(&["Wi-Fi", "2.4 GHz", "Channel", "Frames"]),
            in_lab(&["Bluetooth", "2.4 GHz", "Low Energy"]),
            pane(MenuTile::Keys, 0),
            pane(MenuTile::Options, 0),
            pane(MenuTile::Quit, 0),
            MenuState {
                tile: MenuTile::Radio,
                ..MenuState::default()
            },
        ];
        for w in (20..200u16).step_by(7) {
            for h in [3u16, 6, 12, 24, 32, 41] {
                for state in &states {
                    for line in draw(w, h, state) {
                        assert!(line.chars().count() <= w as usize, "{w}x{h}: {line:?}");
                    }
                }
            }
        }
    }

    /// Paths past the end of the tree, and a tree that does not exist, are
    /// drawn as the nearest real row rather than a panic.
    #[test]
    fn an_out_of_range_path_does_not_panic() {
        let state = MenuState {
            lab: MenuPath::from_rows(&[200, 200, 200, 200, 200, 200, 200, 200, 200]),
            radio: MenuPath::from_rows(&[99]),
            focus: MenuFocus::Below,
            ..MenuState::default()
        };
        let all = draw(191, 41, &state).join("\n");
        assert!(all.contains("Compact"), "{all}");
        let radio = MenuState {
            tile: MenuTile::Radio,
            ..state
        };
        draw(191, 41, &radio);
    }

    /// **Keys, scoped**: opened with the Lab path in a group, the Keys pane
    /// lists that group's panels' focus keys and bindings first, generated
    /// from the panels, then the global keys.
    #[test]
    fn keys_list_the_groups_panels_first() {
        let keys_of = |want: &[&str]| {
            let state = MenuState {
                lab: lab(want),
                ..pane(MenuTile::Keys, 0)
            };
            draw(110, 48, &state).join("\n")
        };
        let le = keys_of(&["Bluetooth", "2.4 GHz", "Low Energy"]);
        assert!(le.contains("LOW ENERGY: FOCUS KEYS"), "{le}");
        assert!(
            le.contains("BLE Advertising") && le.contains("select a packet"),
            "{le}"
        );
        let all = keys_of(&["Bluetooth", "2.4 GHz", "Classic", "Packets"]);
        assert!(all.contains("CLASSIC: FOCUS KEYS"), "{all}");
        for (key, title, what) in [
            ('c', "Piconets", "select a piconet"),
            ('b', "Classic Bluetooth Hops", "zoom in time"),
        ] {
            let row = all
                .lines()
                .find(|l| l.contains(title))
                .unwrap_or_else(|| panic!("{title} missing:\n{all}"));
            assert!(row.contains(&format!("  {key}  ")), "{row}");
            assert!(all.contains(what), "{what}:\n{all}");
        }
        let block = all.find("CLASSIC: FOCUS KEYS").unwrap();
        let radio = all.find("THE RADIO").unwrap();
        assert!(block < radio, "the section's block comes first");
    }

    /// **Every focusable panel a section shows is in its block**: checked
    /// against the presets and the registry directly, for every section, so
    /// a new panel cannot be left out of the Keys pane.
    #[test]
    fn every_focusable_panel_is_in_its_sections_block() {
        let engine = engine();
        let m = SdrMetrics::fixture();
        let presets = &crate::config::LayoutConfig::default_config().presets;
        for (si, section) in engine.menu().sections.iter().enumerate() {
            let controls = engine.section_controls(si, &m);
            for entry in &section.entries {
                for spec in &presets[&entry.preset].panels {
                    let Some(panel) = engine.registered_panels().find(|p| p.name() == spec.name)
                    else {
                        continue;
                    };
                    if let Some(key) = panel.focus_key() {
                        assert!(
                            controls
                                .iter()
                                .any(|c| c.key == key && c.bindings == panel.focus_bindings()),
                            "{} ({key}) missing from {}",
                            spec.name,
                            section.title
                        );
                    }
                }
            }
        }
    }

    /// The Keys pane lists the global keys under its tile.
    #[test]
    fn the_keys_pane_lists_the_global_keys() {
        let all = draw(90, 40, &pane(MenuTile::Keys, 0)).join("\n");
        assert!(all.contains("KEYS"), "{all}");
        assert!(all.contains("start or stop RX"), "{all}");
        assert!(all.contains("type a frequency"), "{all}");
    }

    /// The reference is taller than a short terminal, so it scrolls, and the
    /// scroll actually moves the content rather than being stored and ignored.
    #[test]
    fn the_keys_pane_scrolls() {
        let at = |scroll| MenuState {
            lab: lab(&["Spectrum"]),
            ..pane(MenuTile::Keys, scroll)
        };
        let top = draw(90, 30, &at(0)).join("\n");
        let down = draw(90, 30, &at(30)).join("\n");
        assert!(top.contains("FOCUS KEYS"), "{top}");
        assert!(!down.contains("FOCUS KEYS"), "scrolled away:\n{down}");
        assert_ne!(top, down);
    }

    /// The Options pane says when the radio has nothing to set, and offers
    /// yours below it.
    #[test]
    fn the_options_pane_holds_the_radio_and_the_station() {
        let all = draw(90, 30, &pane(MenuTile::Options, 0)).join("\n");
        assert!(all.contains("OPTIONS"), "{all}");
        assert!(all.contains("nothing to set"), "{all}");
        assert!(all.contains("Station") && all.contains("Callsign"), "{all}");
        assert!(all.contains("Enter type"), "{all}");
    }

    /// Quit says what Enter does there, and what is kept.
    #[test]
    fn the_quit_pane_says_what_enter_does() {
        let state = MenuState {
            focus: MenuFocus::Tiles,
            ..pane(MenuTile::Quit, 0)
        };
        let all = draw(120, 41, &state).join("\n");
        assert!(all.contains("Enter closes sdrtop."), "{all}");
        assert!(all.contains("Enter quit"), "{all}");
    }

    #[test]
    fn a_short_options_pane_keeps_the_selected_option_visible() {
        let state = pane(MenuTile::Options, 5);
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
        let state = pane(MenuTile::Options, 0);
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
            let lines = draw(w, h, &in_lab(&["Receiver", "IQ balance"]));
            assert_eq!(lines.len(), h as usize);
            assert!(lines.iter().all(|l| l.chars().count() <= w as usize));
        }
    }
}
