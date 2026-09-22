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
use crate::state::{DeviceOptionUpdate, InputMode, MenuPane, MenuState, SdrMetrics};
use crate::ui::menu::{keys, sections};

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
        KeyCode::Esc if option_pending => {}
        KeyCode::Tab | KeyCode::BackTab if option_pending => {}
        KeyCode::Esc => close(ctx),
        KeyCode::Char('q') => return KeyAction::Quit,

        KeyCode::Right if state.pane == MenuPane::Options && has_options => {
            return request_option_change(ctx, state, 1);
        }
        KeyCode::Left if state.pane == MenuPane::Options && has_options => {
            return request_option_change(ctx, state, -1);
        }
        KeyCode::Tab | KeyCode::Right => move_row(ctx, state, 1),
        KeyCode::BackTab | KeyCode::Left => move_row(ctx, state, -1),
        KeyCode::Down => move_down(ctx, state, 1),
        KeyCode::Up => move_down(ctx, state, -1),

        // A digit opens that slot in this section, which is the same thing the
        // digit does on the deck. The menu is a picture of the number keys, so
        // the two must not be able to disagree.
        //
        // The Keys pane has no slots, so a digit there does nothing rather than
        // acting on whichever section the cursor last sat in.
        KeyCode::Char(c @ '1'..='9') if state.pane == MenuPane::Views => {
            let slot = c as u8 - b'0';
            let target = section_of(ctx, state).and_then(|s| {
                s.entries
                    .iter()
                    .find(|e| e.slot == Some(slot))
                    .map(|e| e.preset.clone())
            });
            if let Some(name) = target {
                return open(ctx, &name);
            }
        }
        KeyCode::Enter if state.pane == MenuPane::Views => {
            let target = ctx
                .engine
                .menu()
                .at(state.section, state.entry)
                .map(|e| e.preset.clone());
            if let Some(name) = target {
                return open(ctx, &name);
            }
        }
        KeyCode::Enter if state.pane == MenuPane::Options => {
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

/// Load a layout and close the menu.
fn open(ctx: &mut InputCtx<'_>, preset: &str) -> KeyAction {
    let action = global::presets::try_set_preset(ctx.engine, ctx.state, preset);
    close(ctx);
    action
}

fn close(ctx: &mut InputCtx<'_>) {
    metrics(ctx.state).ui.menu = None;
}

fn section_of<'a>(
    ctx: &'a InputCtx<'_>,
    state: MenuState,
) -> Option<&'a crate::ui::menu::model::Section> {
    let (si, _) = ctx.engine.menu().clamp(state.section, state.entry)?;
    ctx.engine.menu().sections.get(si)
}

/// Step down the left column: the sections, then the panes under the rule.
///
/// One index over both kinds of row, so `Tab` walks the column exactly as it is
/// drawn and cannot skip the rule or land on it.
fn move_row(ctx: &mut InputCtx<'_>, state: MenuState, step: isize) {
    let menu = ctx.engine.menu();
    let rows = sections::row_count(menu);
    if rows == 0 {
        return;
    }
    let Some((si, _)) = menu.clamp(state.section, state.entry) else {
        return;
    };
    let next = wrap(sections::selected_row(menu, si, state.pane), step, rows);

    let updated = match sections::row_target(menu, next) {
        Ok(section) => {
            // The new section may be shorter than the one we came from.
            let len = menu.sections[section].entries.len();
            MenuState {
                section,
                entry: state.entry.min(len.saturating_sub(1)),
                pane: MenuPane::Views,
                scroll: 0,
            }
        }
        // Leaving for a pane keeps `section` and `entry`, so coming back lands
        // where you were rather than at the top.
        Err(pane) => MenuState {
            pane,
            scroll: 0,
            ..state
        },
    };
    metrics(ctx.state).ui.menu = Some(updated);
}

/// Up and down: through a section's layouts, or through the key reference.
fn move_down(ctx: &mut InputCtx<'_>, state: MenuState, step: isize) {
    match state.pane {
        MenuPane::Views => {
            let Some((si, ei)) = ctx.engine.menu().clamp(state.section, state.entry) else {
                return;
            };
            let count = ctx.engine.menu().sections[si].entries.len();
            if count == 0 {
                return;
            }
            metrics(ctx.state).ui.menu = Some(MenuState {
                section: si,
                entry: wrap(ei, step, count),
                ..state
            });
        }
        // The reference is taller than a short terminal, so it scrolls. It does
        // not wrap: a list you are reading top to bottom should stop at the
        // bottom rather than silently start again.
        MenuPane::Keys => {
            let last = {
                let m = metrics(ctx.state);
                keys::row_count_for(&m.caps).saturating_sub(1)
            };
            let next = if step >= 0 {
                (state.scroll + 1).min(last)
            } else {
                state.scroll.saturating_sub(1)
            };
            metrics(ctx.state).ui.menu = Some(MenuState {
                scroll: next,
                ..state
            });
        }
        MenuPane::Options => {
            let count = metrics(ctx.state).device_options.len();
            if count == 0 {
                return;
            }
            metrics(ctx.state).ui.menu = Some(MenuState {
                scroll: wrap(state.scroll.min(count - 1), step, count),
                ..state
            });
        }
    }
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
                    .filter(|menu| menu.pane == MenuPane::Options)
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
                    .filter(|menu| menu.pane == MenuPane::Options)
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
                pane: MenuPane::Options,
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
        assert_eq!(m.ui.menu.unwrap().pane, MenuPane::Options);
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

    /// Tab walks the column exactly as it is drawn: every section, then Keys,
    /// then Options, then round to the top. The panes are the two rows under the
    /// rule, so this is what proves Options is reachable at all.
    #[test]
    fn tab_walks_the_sections_then_both_panes_then_wraps() {
        let mut h = Harness::new();
        let sections = h.engine.menu().sections.len();
        for _ in 0..sections - 1 {
            h.key(KeyCode::Tab);
        }
        assert_eq!(h.menu().pane, MenuPane::Views, "still on the last section");

        h.key(KeyCode::Tab);
        assert_eq!(h.menu().pane, MenuPane::Keys);
        h.key(KeyCode::Tab);
        assert_eq!(h.menu().pane, MenuPane::Options);
        h.key(KeyCode::Tab);
        assert_eq!(h.menu().pane, MenuPane::Views, "wraps back to the sections");
        assert_eq!(h.menu().section, 0);
    }

    /// Options holds nothing yet, so the keys that act on a list are quiet
    /// rather than acting on whichever section the cursor came from. A digit
    /// there must not load a layout behind the reader's back.
    #[test]
    fn options_has_nothing_to_steer() {
        let mut h = Harness::new();
        h.state.lock().unwrap().ui.menu = Some(MenuState {
            section: 1,
            entry: 2,
            pane: MenuPane::Options,
            scroll: 0,
        });
        let before = h.menu();

        for code in [
            KeyCode::Down,
            KeyCode::Up,
            KeyCode::Char('1'),
            KeyCode::Enter,
        ] {
            h.key(code);
            assert_eq!(h.menu(), before, "{code:?} moved something");
        }
        assert_eq!(
            h.engine.active_preset(),
            "command_rail",
            "no key in Options may load a layout"
        );
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
        assert_eq!(h.menu().scroll, 0);
        h.key(KeyCode::Up);
        assert_eq!(h.menu().scroll, 1);
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

    #[test]
    fn horizontal_keys_keep_the_old_navigation_for_empty_devices() {
        let mut h = Harness::new();
        h.state.lock().unwrap().ui.menu = Some(MenuState {
            pane: MenuPane::Options,
            ..MenuState::default()
        });

        h.key(KeyCode::Left);
        assert_eq!(h.menu().pane, MenuPane::Keys);

        h.state.lock().unwrap().ui.menu = Some(MenuState {
            pane: MenuPane::Options,
            ..MenuState::default()
        });
        h.key(KeyCode::Right);
        assert_eq!(h.menu().pane, MenuPane::Views);
        assert_eq!(h.menu().section, 0);
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
        assert_eq!(h.menu().pane, MenuPane::Options);
        h.key(KeyCode::BackTab);
        assert_eq!(h.menu().pane, MenuPane::Options);
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

    /// A pane is a detour, not a reset: the place you had in the list survives
    /// the visit, so stepping back onto the sections does not start you at the
    /// top of one.
    #[test]
    fn a_pane_visit_does_not_disturb_the_place_in_the_list() {
        let mut h = Harness::new();
        // The last section, because that is the one the panes are entered from.
        // Walking there first would move the cursor on its own and prove nothing
        // about the panes.
        let last = h.engine.menu().sections.len() - 1;
        let place = MenuState {
            section: last,
            entry: 1,
            pane: MenuPane::Views,
            scroll: 0,
        };
        h.state.lock().unwrap().ui.menu = Some(place);

        h.key(KeyCode::Tab);
        assert_eq!(h.menu().pane, MenuPane::Keys);
        assert_eq!((h.menu().section, h.menu().entry), (last, 1));
        h.key(KeyCode::Tab);
        assert_eq!(h.menu().pane, MenuPane::Options);
        assert_eq!((h.menu().section, h.menu().entry), (last, 1));
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
