// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The menu's own keys, live only while the menu is open.
//!
//! Layer 2 of the dispatch: above panel focus, below text entry. The menu is
//! modal but it is **not** an `InputMode`: the input variants are all text being
//! typed, and this is not text.
//!
//! Modal here means modal. Nothing falls through to [`super::global`] except the
//! quit key, so `[F]` cannot start a frequency entry behind the menu and `[W]`
//! cannot pause a waterfall you are not looking at.

use crossterm::event::{KeyCode, KeyEvent};

use crate::event::{DeviceOptionCompletion, DeviceOptionRequest};
use crate::state::{DeviceOptionUpdate, InputMode, MenuFocus, MenuState, MenuTile, SdrMetrics};
use crate::ui::menu::{self, keys, tree};

use super::{global, metrics, InputCtx, KeyAction};

pub(super) fn handle(key: KeyEvent, ctx: &mut InputCtx<'_>) -> KeyAction {
    // Nothing to steer. Should not happen, since the caller only routes here
    // while the menu is open, but closing is a better answer than a panic.
    let (state, has_options, option_pending) = {
        let m = metrics(ctx.state);
        (
            m.ui.menu,
            !m.device_options.is_empty(),
            m.ui.device_option_update.is_pending(),
        )
    };
    let Some(state) = state else {
        return KeyAction::Continue;
    };

    match key.code {
        KeyCode::Esc | KeyCode::Tab | KeyCode::BackTab if option_pending => {
            return KeyAction::Continue
        }
        KeyCode::Esc => {
            close(ctx);
            return KeyAction::Continue;
        }
        KeyCode::Char('q') => return KeyAction::Quit,
        KeyCode::Tab | KeyCode::BackTab => {
            let focus = match state.focus {
                MenuFocus::Tiles => MenuFocus::Below,
                MenuFocus::Below => MenuFocus::Tiles,
            };
            set(ctx, enter(ctx, MenuState { focus, ..state }));
            return KeyAction::Continue;
        }
        _ => {}
    }

    // A digit opens that slot in the group the Lab path stands in: the same
    // thing the digit does on the deck, so the two cannot disagree.
    if let KeyCode::Char(c @ '1'..='9') = key.code {
        if menu::tree_of(state.tile) == Some(tree::Tile::Lab) {
            return open_slot(ctx, state, c as u8 - b'0');
        }
        return KeyAction::Continue;
    }

    // A tile's letter selects it from anywhere in the menu, so no pane is
    // a place you can only leave by closing the menu.
    if let KeyCode::Char(c) = key.code {
        if let Some(tile) = by_letter(c) {
            set(
                ctx,
                MenuState {
                    tile,
                    focus: MenuFocus::Tiles,
                    scroll: 0,
                    ..state
                },
            );
            return KeyAction::Continue;
        }
    }

    match state.focus {
        MenuFocus::Tiles => tiles_key(key.code, ctx, state),
        MenuFocus::Below => match state.tile {
            MenuTile::Options => options_key(key.code, ctx, state, has_options),
            MenuTile::Keys => keys_key(key.code, ctx, state),
            MenuTile::Quit => match key.code {
                KeyCode::Enter => KeyAction::Quit,
                KeyCode::Up | KeyCode::Left => {
                    set(
                        ctx,
                        MenuState {
                            focus: MenuFocus::Tiles,
                            ..state
                        },
                    );
                    KeyAction::Continue
                }
                _ => KeyAction::Continue,
            },
            MenuTile::Resume | MenuTile::Radio | MenuTile::Lab => tree_key(key.code, ctx, state),
        },
    }
}

/// On the row of tiles: choose one, jump to one by its letter, go in.
fn tiles_key(code: KeyCode, ctx: &mut InputCtx<'_>, state: MenuState) -> KeyAction {
    let shown = MenuTile::shown(state.resume);
    let at = shown.iter().position(|t| *t == state.tile).unwrap_or(0);
    let pick = |tile| MenuState {
        tile,
        scroll: 0,
        ..state
    };
    match code {
        // Stops at the ends: a row you cannot see wrap is a row you lose
        // your place in.
        KeyCode::Left => set(ctx, pick(shown[at.saturating_sub(1)])),
        KeyCode::Right => set(ctx, pick(shown[(at + 1).min(shown.len() - 1)])),
        KeyCode::Enter if state.tile == MenuTile::Resume => {
            let active = ctx.engine.active_preset().to_string();
            return open(ctx, &active);
        }
        KeyCode::Enter if state.tile == MenuTile::Quit => return KeyAction::Quit,
        KeyCode::Enter | KeyCode::Down if state.tile != MenuTile::Quit => {
            set(
                ctx,
                enter(
                    ctx,
                    MenuState {
                        focus: MenuFocus::Below,
                        ..state
                    },
                ),
            );
        }
        _ => {}
    }
    KeyAction::Continue
}

/// The tile a letter selects: the one drawn in its corner. `q` is not here:
/// it quits from anywhere, as on the deck; `Q` selects the Quit tile.
fn by_letter(c: char) -> Option<MenuTile> {
    match c {
        'r' | 'R' => Some(MenuTile::Radio),
        'l' | 'L' => Some(MenuTile::Lab),
        'o' | 'O' => Some(MenuTile::Options),
        'k' | 'K' => Some(MenuTile::Keys),
        'Q' => Some(MenuTile::Quit),
        _ => None,
    }
}

/// Going below Resume is going into the Lab tree, at the view Resume would
/// open; any tree is entered on a real row.
fn enter(ctx: &InputCtx<'_>, state: MenuState) -> MenuState {
    if state.focus != MenuFocus::Below {
        return state;
    }
    let state = if state.tile == MenuTile::Resume {
        MenuState {
            tile: MenuTile::Lab,
            ..state
        }
    } else {
        state
    };
    match menu::tree_of(state.tile) {
        Some(tile) => with_path(state, tile, menu::path_in(ctx.engine, &state, tile)),
        None => state,
    }
}

fn with_path(state: MenuState, tile: tree::Tile, path: Option<tree::Path>) -> MenuState {
    let Some(path) = path else { return state };
    match tile {
        tree::Tile::Lab => MenuState {
            lab: (&path).into(),
            ..state
        },
        tree::Tile::Radio => MenuState {
            radio: (&path).into(),
            ..state
        },
    }
}

/// In a tree: up and down the column, right into what the row holds or the
/// view it is, left back out, and out of the first column to the tiles.
fn tree_key(code: KeyCode, ctx: &mut InputCtx<'_>, state: MenuState) -> KeyAction {
    let Some(tile) = menu::tree_of(state.tile) else {
        return KeyAction::Continue;
    };
    let tree = ctx.engine.menu_tree();
    let Some(path) = menu::path_in(ctx.engine, &state, tile) else {
        return KeyAction::Continue;
    };
    let up_to_tiles = MenuState {
        focus: MenuFocus::Tiles,
        ..state
    };
    let next = match code {
        KeyCode::Up => {
            let moved = tree.step(tile, &path, false);
            if moved == path && path.rows.len() == 1 {
                set(ctx, up_to_tiles);
                return KeyAction::Continue;
            }
            Some(moved)
        }
        KeyCode::Down => Some(tree.step(tile, &path, true)),
        KeyCode::Right | KeyCode::Enter => {
            let node = tree.node(tile, &path);
            if let Some(view) = node.filter(|n| n.opens()).and_then(|n| n.view()) {
                let preset = view.preset.clone();
                return open(ctx, &preset);
            }
            // A refused or later row opens nothing; its card says why.
            tree.descend(tile, &path)
        }
        KeyCode::Left => match tree.ascend(tile, &path) {
            Some(up) => Some(up),
            None => {
                set(ctx, up_to_tiles);
                return KeyAction::Continue;
            }
        },
        _ => None,
    };
    if let Some(next) = next {
        set(ctx, with_path(state, tile, Some(next)));
    }
    KeyAction::Continue
}

/// The Options pane: up and down the settings, left and right through a
/// setting's values, Enter for a number. Up from the first setting goes
/// back to the tiles; with nothing to set, left does too.
fn options_key(
    code: KeyCode,
    ctx: &mut InputCtx<'_>,
    state: MenuState,
    has_options: bool,
) -> KeyAction {
    match code {
        KeyCode::Right if has_options => return request_option_change(ctx, state, 1),
        KeyCode::Left if has_options => return request_option_change(ctx, state, -1),
        KeyCode::Left => set(
            ctx,
            MenuState {
                focus: MenuFocus::Tiles,
                ..state
            },
        ),
        // Up from the first setting goes back to the tiles, as it does out
        // of the key reference; the list stops at its ends rather than
        // wrapping under the cursor.
        KeyCode::Up if state.scroll == 0 || !has_options => set(
            ctx,
            MenuState {
                focus: MenuFocus::Tiles,
                ..state
            },
        ),
        KeyCode::Up => set(
            ctx,
            MenuState {
                scroll: state.scroll - 1,
                ..state
            },
        ),
        KeyCode::Down => {
            let count = metrics(ctx.state).device_options.len();
            if count > 0 {
                set(
                    ctx,
                    MenuState {
                        scroll: (state.scroll + 1).min(count - 1),
                        ..state
                    },
                );
            }
        }
        KeyCode::Enter => {
            let mut m = metrics(ctx.state);
            if m.ui.device_option_update.is_pending() {
                return KeyAction::Continue;
            }
            if let Some(option) = m
                .device_options
                .get(state.scroll)
                .filter(|option| option.integer_range.is_some())
            {
                m.ui.input_mode = InputMode::DeviceOptionInput {
                    id: option.id.clone(),
                    error: None,
                };
                m.ui.input_buf.clear();
                return KeyAction::Continue;
            }
            // Release the metrics lock before the cycling handler acquires it
            drop(m);
            return request_option_change(ctx, state, 1);
        }
        _ => {}
    }
    KeyAction::Continue
}

/// The key reference is taller than a short terminal, so it scrolls. It does
/// not wrap: a list you are reading top to bottom should stop at the bottom
/// rather than silently start again. Up from the top goes to the tiles.
fn keys_key(code: KeyCode, ctx: &mut InputCtx<'_>, state: MenuState) -> KeyAction {
    match code {
        KeyCode::Up | KeyCode::Left if state.scroll == 0 => set(
            ctx,
            MenuState {
                focus: MenuFocus::Tiles,
                ..state
            },
        ),
        KeyCode::Up => set(
            ctx,
            MenuState {
                scroll: state.scroll - 1,
                ..state
            },
        ),
        KeyCode::Down => {
            let last = {
                let m = metrics(ctx.state);
                let section = menu::keys_section(ctx.engine, &state);
                let controls = ctx.engine.section_controls(section, &m);
                keys::row_count_for(&m.caps, &controls).saturating_sub(1)
            };
            set(
                ctx,
                MenuState {
                    scroll: (state.scroll + 1).min(last),
                    ..state
                },
            );
        }
        _ => {}
    }
    KeyAction::Continue
}

/// Open the view with `slot` in the group the Lab path stands in. A refused
/// view is not in the engine's table, so it opens nothing here either.
fn open_slot(ctx: &mut InputCtx<'_>, state: MenuState, slot: u8) -> KeyAction {
    let tree = ctx.engine.menu_tree();
    let Some(path) = menu::path_in(ctx.engine, &state, tree::Tile::Lab) else {
        return KeyAction::Continue;
    };
    let Some(id) = tree.section_at(tree::Tile::Lab, &path) else {
        return KeyAction::Continue;
    };
    let target = ctx
        .engine
        .menu()
        .sections
        .iter()
        .find(|s| s.id == id)
        .and_then(|s| s.entries.iter().find(|e| e.slot == Some(slot)))
        .map(|e| e.preset.clone());
    match target {
        Some(name) => open(ctx, &name),
        None => KeyAction::Continue,
    }
}

fn set(ctx: &mut InputCtx<'_>, state: MenuState) {
    metrics(ctx.state).ui.menu = Some(state);
}

/// Load a layout and close the menu.
fn open(ctx: &mut InputCtx<'_>, preset: &str) -> KeyAction {
    let action = global::presets::try_set_preset(ctx.engine, ctx.state, preset);
    close(ctx);
    action
}

fn close(ctx: &mut InputCtx<'_>) {
    metrics(ctx.state).ui.menu = None;
}

fn request_option_change(ctx: &mut InputCtx<'_>, menu: MenuState, step: isize) -> KeyAction {
    let mut m = metrics(ctx.state);
    if m.ui.device_option_update.is_pending() {
        return KeyAction::Continue;
    }
    let Some(option) = m.device_options.get(menu.scroll) else {
        return KeyAction::Continue;
    };
    if option.choices.len() < 2 {
        return KeyAction::Continue;
    }
    let requested = {
        let Some(current) = option
            .choices
            .iter()
            .position(|choice| choice == &option.selected_choice)
        else {
            let label = option.label.clone();
            m.push_log(format!(
                "{label} error: selected choice is not advertised by the device"
            ));
            return KeyAction::Continue;
        };
        let selected = wrap(current, step, option.choices.len());
        if option.choices[selected] == option.selected_choice {
            return KeyAction::Continue;
        }
        (
            option.id.clone(),
            option.label.clone(),
            option.choices[selected].clone(),
        )
    };

    let request = DeviceOptionRequest {
        id: requested.0,
        label: requested.1,
        choice: requested.2,
    };
    submit_option_request(&mut m, request)
}

pub(super) fn submit_option_request(m: &mut SdrMetrics, request: DeviceOptionRequest) -> KeyAction {
    if m.ui.device_option_update.is_pending() {
        return KeyAction::Continue;
    }
    m.ui.device_option_update = DeviceOptionUpdate::Pending {
        request: request.clone(),
        quit_requested: false,
    };
    KeyAction::ApplyDeviceOption(request)
}

pub(super) fn complete_device_option(
    state: &std::sync::Arc<std::sync::Mutex<SdrMetrics>>,
    completion: DeviceOptionCompletion,
) -> bool {
    let mut m = metrics(state);
    let DeviceOptionUpdate::Pending {
        request,
        quit_requested,
    } = &m.ui.device_option_update
    else {
        return false;
    };
    let request = request.clone();
    let quit_requested = *quit_requested;

    match completion.result {
        Ok(options) => {
            crate::hardware::debug_assert_device_options(&options);
            let selected_id =
                m.ui.menu
                    .filter(|menu| menu.tile == MenuTile::Options)
                    .and_then(|menu| m.device_options.get(menu.scroll))
                    .map(|option| option.id.clone());
            m.device_options = std::sync::Arc::new(options);
            let last_option = m.device_options.len().saturating_sub(1);
            let preserved_index = selected_id
                .as_ref()
                .and_then(|id| m.device_options.iter().position(|option| &option.id == id));
            if let Some(menu) =
                m.ui.menu
                    .as_mut()
                    .filter(|menu| menu.tile == MenuTile::Options)
            {
                menu.scroll = preserved_index.unwrap_or_else(|| menu.scroll.min(last_option));
            }
            let updated_choice = m
                .device_options
                .iter()
                .find(|option| option.id == request.id)
                .map(|option| option.selected_choice.clone());
            m.ui.device_option_update = DeviceOptionUpdate::Idle;
            if let Some(choice) = updated_choice {
                m.push_log(format!("{} set to {choice}", request.label));
            } else {
                m.push_log(format!("{} updated", request.label));
            }
        }
        Err(message) => {
            m.ui.device_option_update = DeviceOptionUpdate::Failed { id: request.id };
            m.push_log(format!("{} error: {message}", request.label));
        }
    }
    quit_requested
}

/// `i + step` modulo `len`, for a step of -1 or 1. Pure, so the wrap-around at
/// both ends is testable without a terminal.
fn wrap(i: usize, step: isize, len: usize) -> usize {
    debug_assert!(len > 0);
    if step >= 0 {
        (i + step as usize) % len
    } else {
        (i + len - (step.unsigned_abs() % len)) % len
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LayoutConfig;
    use crate::hardware::DeviceOption;
    use crate::state::SdrMetrics;
    use crate::ui::{self, PanelRegistry};
    use crossterm::event::KeyEvent;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    /// Drives the menu layer the way the app does, minus the terminal. Same
    /// shape as the harness in `global/mod.rs`, and deliberately device-free:
    /// nothing the menu does should reach the radio.
    struct Harness {
        state: Arc<Mutex<SdrMetrics>>,
        engine: ui::LayoutEngine,
        show_footer: bool,
        focus_keys: crate::app::FocusKeys,
    }

    impl Harness {
        fn new() -> Self {
            let mut engine =
                ui::LayoutEngine::new(LayoutConfig::default_config(), PanelRegistry::new());
            engine.set_preset("command_rail");
            let state = Arc::new(Mutex::new(SdrMetrics::fixture()));
            state.lock().unwrap().ui.menu = Some(MenuState::default());
            Self {
                state,
                engine,
                show_footer: true,
                focus_keys: HashMap::new(),
            }
        }

        fn key(&mut self, code: KeyCode) -> KeyAction {
            super::super::handle_key(
                KeyEvent::from(code),
                &self.state,
                None,
                &mut self.engine,
                &mut self.show_footer,
                &self.focus_keys,
            )
        }

        fn type_number(&mut self, value: &str) {
            for c in value.chars() {
                assert_eq!(self.key(KeyCode::Char(c)), KeyAction::Continue);
            }
        }

        fn menu(&self) -> MenuState {
            self.state.lock().unwrap().ui.menu.expect("menu is open")
        }

        fn show_options(&mut self, options: Vec<DeviceOption>) {
            let mut m = self.state.lock().unwrap();
            m.device_options = Arc::new(options);
            m.ui.menu = Some(MenuState {
                tile: MenuTile::Options,
                focus: MenuFocus::Below,
                ..MenuState::default()
            });
        }
    }

    fn option(id: &str, label: &str, selected: &str) -> DeviceOption {
        described_option(id, label, &["Narrow", "Wide"], selected)
    }

    fn described_option(id: &str, label: &str, choices: &[&str], selected: &str) -> DeviceOption {
        DeviceOption {
            id: id.into(),
            label: label.into(),
            choices: choices.iter().map(|choice| (*choice).into()).collect(),
            selected_choice: selected.into(),
            integer_range: None,
        }
    }

    fn integer_option(selected: &str) -> DeviceOption {
        DeviceOption {
            id: "gain".into(),
            label: "Gain".into(),
            choices: (-100..=100).map(|value| value.to_string()).collect(),
            selected_choice: selected.into(),
            integer_range: Some(-100..=100),
        }
    }

    #[test]
    fn numeric_entry_begins_without_writing_and_cancel_keeps_the_value() {
        let mut h = Harness::new();
        h.show_options(vec![integer_option("0")]);
        assert_eq!(h.key(KeyCode::Enter), KeyAction::Continue);
        h.type_number("-12");
        h.key(KeyCode::Backspace);
        {
            let m = metrics(&h.state);
            assert_eq!(m.ui.input_buf, "-1");
            assert!(matches!(
                &m.ui.input_mode,
                InputMode::DeviceOptionInput { id, error: None } if id == "gain"
            ));
            assert_eq!(m.ui.device_option_update, DeviceOptionUpdate::Idle);
            assert_eq!(m.device_options[0].selected_choice, "0");
        }
        assert_eq!(h.key(KeyCode::Esc), KeyAction::Continue);
        let m = metrics(&h.state);
        assert!(matches!(m.ui.input_mode, InputMode::Normal));
        assert!(m.ui.input_buf.is_empty());
        assert_eq!(m.ui.device_option_update, DeviceOptionUpdate::Idle);
        assert_eq!(m.device_options[0].selected_choice, "0");
        assert_eq!(m.ui.menu.unwrap().tile, MenuTile::Options);
    }

    #[test]
    fn numeric_submission_emits_one_request_and_waits_for_authoritative_refresh() {
        let mut h = Harness::new();
        h.show_options(vec![integer_option("0"), option("mode", "Mode", "Narrow")]);
        h.key(KeyCode::Enter);
        h.type_number("-12");
        let request = DeviceOptionRequest {
            id: "gain".into(),
            label: "Gain".into(),
            choice: "-12".into(),
        };
        assert_eq!(
            h.key(KeyCode::Enter),
            KeyAction::ApplyDeviceOption(request.clone())
        );
        for code in [KeyCode::Enter, KeyCode::Left, KeyCode::Right] {
            assert_eq!(h.key(code), KeyAction::Continue);
        }
        {
            let m = metrics(&h.state);
            assert_eq!(m.device_options[0].selected_choice, "0");
            assert!(matches!(m.ui.input_mode, InputMode::Normal));
            assert!(m.ui.input_buf.is_empty());
            assert_eq!(
                m.ui.device_option_update,
                DeviceOptionUpdate::Pending {
                    request,
                    quit_requested: false,
                }
            );
        }
        h.key(KeyCode::Down);
        assert!(!complete_device_option(
            &h.state,
            DeviceOptionCompletion {
                result: Ok(vec![option("mode", "Mode", "Wide"), integer_option("-10")]),
            }
        ));
        assert_eq!(h.menu().scroll, 0);
        let m = metrics(&h.state);
        assert_eq!(m.device_options[1].selected_choice, "-10");
        assert_eq!(m.ui.device_option_update, DeviceOptionUpdate::Idle);
        assert!(m.ui.log.back().unwrap().text.contains("Gain set to -10"));
    }

    #[test]
    fn numeric_input_rejects_invalid_out_of_range_and_unadvertised_values() {
        for value in [
            "",
            "-",
            "101",
            "-101",
            "2147483648",
            "1.5",
            "+1",
            " 1",
            "1 ",
            "1e1",
            "auto",
            "12",
        ] {
            let mut h = Harness::new();
            let mut option = integer_option("0");
            option.choices.retain(|choice| choice != "12");
            h.show_options(vec![option]);
            h.key(KeyCode::Enter);
            metrics(&h.state).ui.input_buf = value.into();
            assert_eq!(h.key(KeyCode::Enter), KeyAction::Continue, "{value}");
            let m = metrics(&h.state);
            assert_eq!(m.ui.device_option_update, DeviceOptionUpdate::Idle);
            assert_eq!(m.device_options[0].selected_choice, "0");
            assert_eq!(m.ui.input_buf, value);
            let expected = match value {
                "101" | "-101" => "Out of range: -100 to 100",
                "12" => "Not advertised. Closest choice: 11",
                _ => "Enter a signed 32-bit integer",
            };
            assert!(
                matches!(
                    &m.ui.input_mode,
                    InputMode::DeviceOptionInput { error: Some(message), .. }
                        if message == expected
                ),
                "{value}"
            );
        }
    }

    #[test]
    fn numeric_keystrokes_ignore_an_embedded_minus() {
        let mut h = Harness::new();
        h.show_options(vec![integer_option("0")]);
        h.key(KeyCode::Enter);
        h.type_number("1-2");
        assert_eq!(metrics(&h.state).ui.input_buf, "12");
    }

    #[test]
    fn overflowing_numeric_input_is_preserved_and_never_submitted() {
        let mut h = Harness::new();
        h.show_options(vec![integer_option("0")]);
        h.key(KeyCode::Enter);
        let input = "9".repeat(100);
        h.type_number(&input);
        assert_eq!(h.key(KeyCode::Enter), KeyAction::Continue);
        let m = metrics(&h.state);
        assert_eq!(m.ui.input_buf, input);
        assert_eq!(m.ui.device_option_update, DeviceOptionUpdate::Idle);
        assert!(matches!(&m.ui.input_mode,
            InputMode::DeviceOptionInput { error: Some(error), .. }
            if error == "Enter a signed 32-bit integer"));
    }

    #[test]
    fn repeated_invalid_submissions_stay_inline_without_writes_or_logs() {
        let mut h = Harness::new();
        let mut option = integer_option("0");
        option.choices = ["0", "10"].map(String::from).to_vec();
        h.show_options(vec![option]);
        h.key(KeyCode::Enter);
        h.type_number("9");
        let log_count = metrics(&h.state).ui.log.len();
        for _ in 0..3 {
            assert_eq!(h.key(KeyCode::Enter), KeyAction::Continue);
        }
        let m = metrics(&h.state);
        assert_eq!(m.ui.log.len(), log_count);
        assert_eq!(m.ui.input_buf, "9");
        assert_eq!(m.device_options[0].selected_choice, "0");
        assert_eq!(m.ui.device_option_update, DeviceOptionUpdate::Idle);
        assert!(matches!(&m.ui.input_mode,
            InputMode::DeviceOptionInput { error: Some(error), .. }
            if error == "Not advertised. Closest choice: 10"));
    }

    #[test]
    fn editing_an_invalid_number_clears_the_error_and_allows_resubmission() {
        let mut h = Harness::new();
        h.show_options(vec![integer_option("0")]);
        h.key(KeyCode::Enter);
        h.type_number("-");
        h.key(KeyCode::Enter);
        h.key(KeyCode::Backspace);
        assert!(matches!(
            metrics(&h.state).ui.input_mode,
            InputMode::DeviceOptionInput { error: None, .. }
        ));
        h.type_number("-100");
        assert!(
            matches!(h.key(KeyCode::Enter), KeyAction::ApplyDeviceOption(request)
            if request.choice == "-100")
        );
    }

    #[test]
    fn numeric_entry_keeps_auto_available_through_choice_arrows() {
        let mut h = Harness::new();
        let mut option = DeviceOption {
            id: "level".into(),
            label: "Level".into(),
            choices: std::iter::once("auto".to_string())
                .chain((0..=31).map(|value| value.to_string()))
                .collect(),
            selected_choice: "auto".into(),
            integer_range: Some(0..=31),
        };
        h.show_options(vec![option.clone()]);
        h.key(KeyCode::Enter);
        assert!(metrics(&h.state).ui.input_buf.is_empty());
        h.type_number("-1");
        assert_eq!(h.key(KeyCode::Enter), KeyAction::Continue);
        h.key(KeyCode::Esc);
        h.key(KeyCode::Enter);
        h.type_number("31");
        assert!(
            matches!(h.key(KeyCode::Enter), KeyAction::ApplyDeviceOption(request)
            if request.choice == "31")
        );
        option.selected_choice = "31".into();
        complete_device_option(
            &h.state,
            DeviceOptionCompletion {
                result: Ok(vec![option]),
            },
        );
        assert!(
            matches!(h.key(KeyCode::Right), KeyAction::ApplyDeviceOption(request)
            if request.choice == "auto")
        );
    }

    #[test]
    fn unchanged_numeric_values_skip_writes_after_normalization() {
        for input in ["0", "-0", "000"] {
            let mut h = Harness::new();
            h.show_options(vec![integer_option("0")]);
            h.key(KeyCode::Enter);
            h.type_number(input);
            assert_eq!(h.key(KeyCode::Enter), KeyAction::Continue);
            let m = metrics(&h.state);
            assert_eq!(m.ui.device_option_update, DeviceOptionUpdate::Idle);
            assert!(matches!(m.ui.input_mode, InputMode::Normal));
        }
    }

    #[test]
    fn numeric_submission_uses_the_canonical_advertised_string() {
        for (input, expected) in [("100", "100"), ("-0012", "-12")] {
            let mut h = Harness::new();
            h.show_options(vec![integer_option("0")]);
            h.key(KeyCode::Enter);
            h.type_number(input);
            assert!(matches!(
                h.key(KeyCode::Enter),
                KeyAction::ApplyDeviceOption(request) if request.choice == expected
            ));
        }
        let mut h = Harness::new();
        let mut option = integer_option("0");
        option.choices.retain(|choice| choice != "12");
        option.choices.push("012".into());
        h.show_options(vec![option]);
        h.key(KeyCode::Enter);
        h.type_number("12");
        assert_eq!(h.key(KeyCode::Enter), KeyAction::Continue);
        assert_eq!(
            metrics(&h.state).ui.device_option_update,
            DeviceOptionUpdate::Idle
        );
    }

    #[test]
    fn numeric_entry_cannot_start_during_another_option_update() {
        let mut h = Harness::new();
        h.show_options(vec![option("mode", "Mode", "Narrow"), integer_option("0")]);
        let KeyAction::ApplyDeviceOption(request) = h.key(KeyCode::Enter) else {
            panic!("discrete change did not create a request");
        };
        h.key(KeyCode::Down);
        assert_eq!(h.key(KeyCode::Enter), KeyAction::Continue);
        let m = metrics(&h.state);
        assert!(matches!(m.ui.input_mode, InputMode::Normal));
        assert_eq!(
            m.ui.device_option_update,
            DeviceOptionUpdate::Pending {
                request,
                quit_requested: false,
            }
        );
    }

    #[test]
    fn integer_choices_require_explicit_numeric_metadata() {
        let mut h = Harness::new();
        let mut option = integer_option("0");
        option.integer_range = None;
        h.show_options(vec![option]);
        assert!(
            matches!(h.key(KeyCode::Enter), KeyAction::ApplyDeviceOption(request)
            if request.choice == "1")
        );
        assert!(matches!(metrics(&h.state).ui.input_mode, InputMode::Normal));
    }

    #[test]
    fn numeric_editor_resolves_its_option_by_id_and_rechecks_current_metadata() {
        let mut h = Harness::new();
        h.show_options(vec![integer_option("0"), option("mode", "Mode", "Narrow")]);
        h.key(KeyCode::Enter);
        h.type_number("12");
        Arc::make_mut(&mut metrics(&h.state).device_options).swap(0, 1);
        assert!(
            matches!(h.key(KeyCode::Enter), KeyAction::ApplyDeviceOption(request)
            if request.id == "gain" && request.choice == "12")
        );

        for removed in [false, true] {
            let mut h = Harness::new();
            h.show_options(vec![integer_option("0")]);
            h.key(KeyCode::Enter);
            h.type_number("12");
            if removed {
                metrics(&h.state).device_options = Arc::new(Vec::new());
            } else {
                Arc::make_mut(&mut metrics(&h.state).device_options)[0].integer_range = None;
            }
            assert_eq!(h.key(KeyCode::Enter), KeyAction::Continue);
            assert!(matches!(
                metrics(&h.state).ui.input_mode,
                InputMode::DeviceOptionInput { error: Some(_), .. }
            ));
            assert_eq!(
                metrics(&h.state).ui.device_option_update,
                DeviceOptionUpdate::Idle
            );
        }
    }

    #[test]
    fn numeric_completion_preserves_quit_request_on_success_and_failure() {
        for failed in [false, true] {
            let mut h = Harness::new();
            h.show_options(vec![integer_option("0")]);
            h.key(KeyCode::Enter);
            h.type_number("1");
            assert!(matches!(
                h.key(KeyCode::Enter),
                KeyAction::ApplyDeviceOption(_)
            ));
            assert_eq!(h.key(KeyCode::Char('q')), KeyAction::Quit);
            assert!(!metrics(&h.state).ui.device_option_update.request_quit());
            let result = if failed {
                Err("device rejected choice".into())
            } else {
                Ok(vec![integer_option("1")])
            };
            assert!(complete_device_option(
                &h.state,
                DeviceOptionCompletion { result }
            ));
            let m = metrics(&h.state);
            if failed {
                assert_eq!(m.device_options[0].selected_choice, "0");
                assert_eq!(
                    m.ui.device_option_update,
                    DeviceOptionUpdate::Failed { id: "gain".into() }
                );
            } else {
                assert_eq!(m.device_options[0].selected_choice, "1");
                assert_eq!(m.ui.device_option_update, DeviceOptionUpdate::Idle);
            }
        }
    }

    /// The path to the rows titled `want` in the Lab tree.
    fn lab_path(h: &Harness, want: &[&str]) -> crate::state::MenuPath {
        let tree = h.engine.menu_tree();
        let mut rows = tree.root(tree::Tile::Lab);
        let mut path = Vec::new();
        for name in want {
            let i = rows
                .iter()
                .position(|n| n.is_stop() && n.title == *name)
                .unwrap();
            path.push(i);
            rows = rows[i].children();
        }
        crate::state::MenuPath::from_rows(&path)
    }

    fn titles(h: &Harness, path: crate::state::MenuPath) -> Vec<String> {
        h.engine
            .menu_tree()
            .trail(tree::Tile::Lab, &path.into())
            .iter()
            .map(|n| n.title.clone())
            .collect()
    }

    /// Left and right choose a tile and stop at the ends; a tile's letter
    /// selects it from anywhere on the row.
    #[test]
    fn the_tiles_are_chosen_by_arrows_and_letters() {
        let mut h = Harness::new();
        h.state.lock().unwrap().ui.menu = Some(MenuState {
            resume: true,
            ..MenuState::default()
        });
        h.key(KeyCode::Left);
        h.key(KeyCode::Left);
        h.key(KeyCode::Left);
        assert_eq!(h.menu().tile, MenuTile::Resume, "stops at the first");
        for (c, tile) in [
            ('k', MenuTile::Keys),
            ('O', MenuTile::Options),
            ('r', MenuTile::Radio),
            ('L', MenuTile::Lab),
            ('Q', MenuTile::Quit),
        ] {
            h.key(KeyCode::Char(c));
            assert_eq!(h.menu().tile, tile, "{c}");
        }
        h.key(KeyCode::Right);
        assert_eq!(h.menu().tile, MenuTile::Quit, "stops at the last");
        assert_eq!(
            h.key(KeyCode::Enter),
            KeyAction::Quit,
            "Enter on Quit quits"
        );
    }

    /// A tile's letter leaves any pane for that tile: Options is never a
    /// place only Esc gets you out of.
    #[test]
    fn a_tiles_letter_leaves_any_pane() {
        let mut h = Harness::new();
        h.show_options(vec![option("bandwidth", "Bandwidth", "Narrow")]);
        h.key(KeyCode::Char('l'));
        assert_eq!(h.menu().tile, MenuTile::Lab);
        assert_eq!(h.menu().focus, MenuFocus::Tiles);
    }

    /// Down goes into the tree, Tab goes back and forth.
    #[test]
    fn down_and_tab_move_between_the_tiles_and_what_is_below() {
        let mut h = Harness::new();
        assert_eq!(h.menu().focus, MenuFocus::Tiles);
        h.key(KeyCode::Down);
        assert_eq!(h.menu().focus, MenuFocus::Below);
        h.key(KeyCode::Tab);
        assert_eq!(h.menu().focus, MenuFocus::Tiles);
        h.key(KeyCode::Tab);
        assert_eq!(h.menu().focus, MenuFocus::Below);
    }

    /// Right goes into what a row holds, through Bluetooth's single band;
    /// left comes back the same way, and out of the first column to the tiles.
    #[test]
    fn the_tree_is_walked_in_and_out() {
        let mut h = Harness::new();
        let bluetooth = lab_path(&h, &["Bluetooth"]);
        h.state.lock().unwrap().ui.menu = Some(MenuState {
            focus: MenuFocus::Below,
            lab: bluetooth,
            ..MenuState::default()
        });
        h.key(KeyCode::Right);
        assert_eq!(
            titles(&h, h.menu().lab),
            ["Bluetooth", "2.4 GHz", "Overview"]
        );
        h.key(KeyCode::Down);
        assert_eq!(
            titles(&h, h.menu().lab),
            ["Bluetooth", "2.4 GHz", "Low Energy"]
        );
        h.key(KeyCode::Right);
        assert_eq!(
            titles(&h, h.menu().lab),
            ["Bluetooth", "2.4 GHz", "Low Energy", "LE devices"]
        );
        h.key(KeyCode::Left);
        h.key(KeyCode::Left);
        assert_eq!(titles(&h, h.menu().lab), ["Bluetooth"]);
        h.key(KeyCode::Left);
        assert_eq!(h.menu().focus, MenuFocus::Tiles);
        assert_eq!(titles(&h, h.menu().lab), ["Bluetooth"], "the place is kept");
    }

    /// Enter on a view opens it and closes the menu; a digit opens that slot
    /// in the group the path stands in, on the tiles or in the tree.
    #[test]
    fn enter_and_digits_open_views() {
        let mut h = Harness::new();
        let iq = lab_path(&h, &["Receiver", "IQ balance"]);
        h.state.lock().unwrap().ui.menu = Some(MenuState {
            focus: MenuFocus::Below,
            lab: iq,
            ..MenuState::default()
        });
        h.key(KeyCode::Enter);
        assert_eq!(h.engine.active_preset(), "lab_iq");
        assert!(h.state.lock().unwrap().ui.menu.is_none(), "the menu closed");

        h.state.lock().unwrap().ui.menu = Some(MenuState {
            lab: iq,
            ..MenuState::default()
        });
        h.key(KeyCode::Char('3'));
        assert_eq!(h.engine.active_preset(), "lab_timing");
    }

    /// Resume's Enter goes back to the deck's view; its Down goes into the
    /// Lab tree standing on that view.
    #[test]
    fn resume_goes_back_or_goes_in() {
        let mut h = Harness::new();
        h.engine.set_preset("lab_rf");
        let opening = crate::ui::menu::opening(&h.engine, true);
        assert_eq!(opening.tile, MenuTile::Resume);
        h.state.lock().unwrap().ui.menu = Some(opening);
        h.key(KeyCode::Down);
        assert_eq!(h.menu().tile, MenuTile::Lab);
        assert_eq!(titles(&h, h.menu().lab), ["Receiver", "Gain & noise"]);

        h.engine.set_preset("command_rail");
        h.state.lock().unwrap().ui.menu = Some(MenuState {
            tile: MenuTile::Resume,
            resume: true,
            ..opening
        });
        h.key(KeyCode::Enter);
        assert_eq!(h.engine.active_preset(), "command_rail");
        assert!(h.state.lock().unwrap().ui.menu.is_none());
    }

    /// Options with nothing to set: no key there loads a layout, and up goes
    /// back to the tiles.
    #[test]
    fn options_has_nothing_to_steer() {
        let mut h = Harness::new();
        h.show_options(Vec::new());
        let before = h.menu();
        for code in [KeyCode::Down, KeyCode::Char('1'), KeyCode::Enter] {
            h.key(code);
            assert_eq!(h.menu(), before, "{code:?} moved something");
        }
        assert_eq!(
            h.engine.active_preset(),
            "command_rail",
            "no key in Options may load a layout"
        );
        h.key(KeyCode::Up);
        assert_eq!(h.menu().focus, MenuFocus::Tiles);
    }

    #[test]
    fn options_move_up_and_down_through_device_settings() {
        let mut h = Harness::new();
        h.show_options(vec![
            option("bandwidth", "Bandwidth", "Narrow"),
            option("mode", "Mode", "Wide"),
        ]);

        h.key(KeyCode::Down);
        assert_eq!(h.menu().scroll, 1);
        h.key(KeyCode::Down);
        assert_eq!(h.menu().scroll, 1, "stops at the last");
        h.key(KeyCode::Up);
        assert_eq!(h.menu().scroll, 0);
        assert_eq!(h.menu().focus, MenuFocus::Below);
        h.key(KeyCode::Up);
        assert_eq!(h.menu().focus, MenuFocus::Tiles, "up from the first leaves");
        assert_eq!(h.menu().tile, MenuTile::Options);
    }

    #[test]
    fn horizontal_value_keys_do_not_leave_options_when_values_exist() {
        let mut h = Harness::new();
        h.show_options(vec![option("bandwidth", "Bandwidth", "Narrow")]);
        let before = h.menu();

        let action = h.key(KeyCode::Right);

        assert_eq!(h.menu(), before);
        assert!(matches!(action, KeyAction::ApplyDeviceOption(_)));
    }

    /// With nothing to set, left leaves Options for the tiles, as it leaves a
    /// tree, and right does nothing.
    #[test]
    fn horizontal_keys_leave_an_empty_options_pane() {
        let mut h = Harness::new();
        h.show_options(Vec::new());
        h.key(KeyCode::Right);
        assert_eq!(h.menu().focus, MenuFocus::Below);
        h.key(KeyCode::Left);
        assert_eq!(h.menu().focus, MenuFocus::Tiles);
        assert_eq!(h.menu().tile, MenuTile::Options);
    }

    #[test]
    fn a_successful_completion_refreshes_all_options() {
        let mut h = Harness::new();
        h.show_options(vec![option("bandwidth", "Bandwidth", "Narrow")]);
        let KeyAction::ApplyDeviceOption(_) = h.key(KeyCode::Right) else {
            panic!("value change did not create a request");
        };

        complete_device_option(
            &h.state,
            DeviceOptionCompletion {
                result: Ok(vec![
                    option("bandwidth", "Bandwidth", "Wide"),
                    described_option("attenuation", "Attenuation", &["0 dB", "10 dB"], "0 dB"),
                ]),
            },
        );

        let m = h.state.lock().unwrap();
        assert_eq!(m.device_options[0].selected_choice, "Wide");
        assert_eq!(m.device_options[1].selected_choice, "0 dB");
        assert!(matches!(
            m.ui.device_option_update,
            DeviceOptionUpdate::Idle
        ));
        assert!(m
            .ui
            .log
            .back()
            .is_some_and(|entry| entry.text.contains("Bandwidth set to Wide")));
    }

    #[test]
    fn a_failed_completion_keeps_state_and_surfaces_the_error() {
        let mut h = Harness::new();
        h.show_options(vec![option("bandwidth", "Bandwidth", "Narrow")]);
        let before = h.state.lock().unwrap().device_options.clone();
        let KeyAction::ApplyDeviceOption(_) = h.key(KeyCode::Right) else {
            panic!("value change did not create a request");
        };

        complete_device_option(
            &h.state,
            DeviceOptionCompletion {
                result: Err("device rejected choice".to_string()),
            },
        );

        let m = h.state.lock().unwrap();
        assert_eq!(m.device_options, before);
        assert!(matches!(
            m.ui.device_option_update,
            DeviceOptionUpdate::Failed { .. }
        ));
        assert!(m.ui.log.back().is_some_and(|entry| entry
            .text
            .contains("Bandwidth error: device rejected choice")));
    }

    #[test]
    fn a_successful_refresh_logs_the_authoritative_choice() {
        let mut h = Harness::new();
        h.show_options(vec![option("bandwidth", "Bandwidth", "Narrow")]);
        let KeyAction::ApplyDeviceOption(_) = h.key(KeyCode::Right) else {
            panic!("value change did not create a request");
        };

        complete_device_option(
            &h.state,
            DeviceOptionCompletion {
                result: Ok(vec![described_option(
                    "bandwidth",
                    "Bandwidth",
                    &["Narrow", "Wide", "Auto"],
                    "Auto",
                )]),
            },
        );

        let m = h.state.lock().unwrap();
        assert_eq!(m.device_options[0].selected_choice, "Auto");
        assert!(m
            .ui
            .log
            .back()
            .is_some_and(|entry| entry.text.contains("Bandwidth set to Auto")));
    }

    #[test]
    fn a_disappearing_option_gets_a_neutral_success_log() {
        let mut h = Harness::new();
        h.show_options(vec![option("bandwidth", "Bandwidth", "Narrow")]);
        let KeyAction::ApplyDeviceOption(_) = h.key(KeyCode::Right) else {
            panic!("value change did not create a request");
        };

        complete_device_option(
            &h.state,
            DeviceOptionCompletion {
                result: Ok(vec![option("mode", "Mode", "Narrow")]),
            },
        );

        let m = h.state.lock().unwrap();
        assert!(m
            .ui
            .log
            .back()
            .is_some_and(|entry| entry.text.as_ref() == "Bandwidth updated"));
    }

    #[test]
    fn a_pending_change_keeps_navigation_and_quit_responsive() {
        let mut h = Harness::new();
        h.show_options(vec![
            option("bandwidth", "Bandwidth", "Narrow"),
            option("mode", "Mode", "Wide"),
        ]);

        assert!(matches!(
            h.key(KeyCode::Right),
            KeyAction::ApplyDeviceOption(_)
        ));
        assert_eq!(h.key(KeyCode::Left), KeyAction::Continue);
        h.key(KeyCode::Down);
        assert_eq!(h.menu().scroll, 1);
        h.key(KeyCode::Esc);
        assert!(h.state.lock().unwrap().ui.menu.is_some());
        h.key(KeyCode::Tab);
        assert_eq!(h.menu().focus, MenuFocus::Below, "Tab waits for the device");
        h.key(KeyCode::BackTab);
        assert_eq!(h.menu().focus, MenuFocus::Below);
        assert_eq!(h.key(KeyCode::Char('q')), KeyAction::Quit);
    }

    #[test]
    fn a_refresh_preserves_the_highlighted_option_by_id() {
        let mut h = Harness::new();
        h.show_options(vec![
            option("bandwidth", "Bandwidth", "Narrow"),
            option("mode", "Mode", "Narrow"),
        ]);
        let KeyAction::ApplyDeviceOption(_) = h.key(KeyCode::Right) else {
            panic!("value change did not create a request");
        };
        h.key(KeyCode::Down);

        complete_device_option(
            &h.state,
            DeviceOptionCompletion {
                result: Ok(vec![
                    option("mode", "Mode", "Narrow"),
                    option("bandwidth", "Bandwidth", "Wide"),
                ]),
            },
        );

        assert_eq!(
            h.menu().scroll,
            0,
            "Mode remains highlighted after reordering"
        );
    }

    #[test]
    fn unchanged_and_single_choice_options_do_not_start_writes() {
        let mut h = Harness::new();
        h.show_options(vec![described_option("mode", "Mode", &["Only"], "Only")]);

        assert_eq!(h.key(KeyCode::Right), KeyAction::Continue);
        h.show_options(vec![described_option(
            "mode",
            "Mode",
            &["Same", "Same"],
            "Same",
        )]);
        assert_eq!(h.key(KeyCode::Right), KeyAction::Continue);
        assert!(matches!(
            h.state.lock().unwrap().ui.device_option_update,
            DeviceOptionUpdate::Idle
        ));
    }

    /// A pane is a detour, not a reset: the place in the Lab tree survives a
    /// visit to Options and Keys, and to the Radio tree.
    #[test]
    fn a_pane_visit_does_not_disturb_the_place_in_the_tree() {
        let mut h = Harness::new();
        let frames = lab_path(&h, &["Wi-Fi", "2.4 GHz", "Channel", "Frames"]);
        h.state.lock().unwrap().ui.menu = Some(MenuState {
            lab: frames,
            ..MenuState::default()
        });
        for c in ['o', 'k', 'r', 'l'] {
            h.key(KeyCode::Char(c));
            assert_eq!(h.menu().lab, frames, "{c}");
        }
        h.key(KeyCode::Down);
        assert_eq!(
            titles(&h, h.menu().lab),
            ["Wi-Fi", "2.4 GHz", "Channel", "Frames"]
        );
    }

    #[test]
    fn wrap_moves_forward_and_back() {
        assert_eq!(wrap(0, 1, 4), 1);
        assert_eq!(wrap(3, 1, 4), 0, "forward off the end comes round");
        assert_eq!(wrap(1, -1, 4), 0);
        assert_eq!(wrap(0, -1, 4), 3, "back off the front comes round");
    }

    #[test]
    fn wrap_handles_a_single_entry() {
        assert_eq!(wrap(0, 1, 1), 0);
        assert_eq!(wrap(0, -1, 1), 0);
    }
}
