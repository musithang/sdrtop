// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The keys that work everywhere: quit, help, presets, gain, RX on/off, and the
//! focus keys that enter a panel's own mode.
//!
//! Every focus handler falls through to [`handle`] for anything it does not
//! claim, so this is the bottom of the dispatch and the only place a key can be
//! claimed without a panel being focused.
//!
//! **The whole match stays here.** Each arm is one call, and the bodies live next
//! door - grouped by what the key does, not by which key it is:
//!
//! - [`radio`]: the keys that command the hardware.
//! - [`gain`]: the four gain keys.
//! - [`presets`]: the number keys.
//! - [`view`]: the keys that only change what is drawn.
//!
//! Keeping the match whole is deliberate: it is the one place to read every
//! global key, and `ui/menu/keys.rs` reads this file as source text to check the
//! on-screen key reference against it. Add an arm here, add its row there, or the
//! suite fails.

mod gain;
// `menu` reaches `try_set_preset` too: the menu and the number keys must load
// a layout the same way, or the two could drift.
pub(super) mod presets;
mod radio;
mod view;

use crossterm::event::{KeyCode, KeyEvent};

use super::{InputCtx, KeyAction};

pub(super) fn handle(key: KeyEvent, ctx: &mut InputCtx<'_>) -> KeyAction {
    match key.code {
        KeyCode::Esc => view::leave_focus_or_open_menu(ctx),
        KeyCode::Char('q') => return KeyAction::Quit,

        // ── The radio ───────────────────────────────────────────────────────
        KeyCode::Char(' ') => radio::toggle_rx(ctx),
        KeyCode::Char('r') => radio::reset_defaults(ctx),
        KeyCode::Char('f') => radio::begin_frequency_input(ctx),
        KeyCode::Char('s') => radio::begin_sample_rate_input(ctx),

        // ── Gain staging ────────────────────────────────────────────────────
        KeyCode::Up => gain::step_primary(ctx, true),
        KeyCode::Down => gain::step_primary(ctx, false),
        KeyCode::Char('[') => gain::step_vga(ctx, false),
        KeyCode::Char(']') => gain::step_vga(ctx, true),
        KeyCode::Char('a') => gain::toggle_boost(ctx),

        // ── Overlays and the drawn view ─────────────────────────────────────
        KeyCode::Tab => *ctx.show_footer = !*ctx.show_footer,
        KeyCode::Char('w') => view::toggle_waterfall_pause(ctx),
        KeyCode::Char('h') => view::toggle_hold(ctx),

        // ── Layouts ─────────────────────────────────────────────────────────
        // One arm for all nine. A digit means "the nth layout of the section I am
        // in", which is exactly what the menu shows, so the menu is a picture of
        // what these keys do right now rather than a second list to keep in step.
        //
        // This used to be nine hand-wired arms naming nine presets, which is why
        // `[1]` could go on meaning `command_rail` while the help overlay claimed
        // it meant `main`. There is now nothing here to disagree with.
        KeyCode::Char(c @ '1'..='9') => {
            let slot = c as u8 - b'0';
            // The name is copied out before `try_set_preset` takes `&mut engine`.
            let Some(name) = ctx.engine.preset_in_scope(slot).map(str::to_string) else {
                return KeyAction::Continue;
            };
            return presets::try_set_preset(ctx.engine, ctx.state, &name);
        }
        KeyCode::Char('p') => presets::cycle_in_scope(ctx),

        // ── Anything else: a panel's focus key, or nothing ──────────────────
        KeyCode::Char(c) => view::enter_focus(ctx, c),
        _ => {}
    }
    KeyAction::Continue
}

/// [`handle`] with the radio hidden.
///
/// The waterfall's focus handler never had a `device` parameter, so anything
/// falling through from it could not reach the hardware. Hiding the device here
/// keeps that exactly true now that the context carries one: `[Space]`, `[R]`,
/// `[F]` and the gain keys stay inert while the waterfall holds focus, rather
/// than quietly becoming live.
pub(super) fn handle_no_device(key: KeyEvent, ctx: &mut InputCtx<'_>) -> KeyAction {
    let saved = ctx.device.take();
    let action = handle(key, ctx);
    ctx.device = saved;
    action
}
#[cfg(test)]
mod tests {
    use super::super::metrics;
    use super::*;
    use crate::config::LayoutConfig;
    use crate::state::SdrMetrics;
    use crate::ui;
    use crate::ui::PanelRegistry;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    /// A context with no device: the observer-mode shape, and the one that makes
    /// the hardware keys' guards visible.
    struct Harness {
        state: Arc<Mutex<SdrMetrics>>,
        engine: ui::LayoutEngine,
        show_footer: bool,
        focus_keys: HashMap<char, &'static str>,
    }

    impl Harness {
        fn new() -> Self {
            let mut engine =
                ui::LayoutEngine::new(LayoutConfig::default_config(), PanelRegistry::new());
            engine.set_preset("command_rail");
            Self {
                state: Arc::new(Mutex::new(SdrMetrics::fixture())),
                engine,
                show_footer: true,
                focus_keys: HashMap::new(),
            }
        }

        fn press(&mut self, c: char) -> KeyAction {
            self.key(KeyCode::Char(c))
        }

        fn key(&mut self, code: KeyCode) -> KeyAction {
            let mut ctx = InputCtx {
                state: &self.state,
                device: None,
                engine: &mut self.engine,
                show_footer: &mut self.show_footer,
                focus_keys: &self.focus_keys,
            };
            handle(KeyEvent::from(code), &mut ctx)
        }

        fn log(&self) -> String {
            metrics(&self.state)
                .ui
                .log
                .iter()
                .map(|e| e.text.clone())
                .collect::<Vec<_>>()
                .join("\n")
        }
    }

    #[test]
    fn q_is_the_only_key_that_quits() {
        let mut h = Harness::new();
        assert_eq!(h.press('q'), KeyAction::Quit);
        for c in ['?', 'p', '1', 'w', 'a', 'z'] {
            assert_eq!(h.press(c), KeyAction::Continue, "'{c}' should not quit");
        }
        assert_eq!(h.key(KeyCode::Esc), KeyAction::Continue);
    }

    #[test]
    fn the_footer_toggle_toggles_rather_than_latching() {
        let mut h = Harness::new();
        assert!(h.show_footer);
        h.key(KeyCode::Tab);
        assert!(!h.show_footer);
        h.key(KeyCode::Tab);
        assert!(h.show_footer, "[Tab] must toggle, not latch");
    }

    #[test]
    fn a_number_key_switches_preset_and_says_which() {
        let mut h = Harness::new();
        h.press('2');
        assert_eq!(h.engine.active_preset(), "spectrum");
        assert!(h.log().contains("Preset"), "no log line:\n{}", h.log());
    }

    /// A digit means "the nth layout of the section I am in", so the same key
    /// lands somewhere else depending on where you are. This is the feature.
    #[test]
    fn a_number_key_selects_within_the_current_section() {
        let mut h = Harness::new();
        h.engine.set_preset("lab_iq");
        h.press('3');
        assert_eq!(h.engine.active_preset(), "lab_timing");

        h.engine.set_preset("command_rail");
        h.press('3');
        assert_eq!(h.engine.active_preset(), "waterfall");

        h.engine.set_preset("micro_main");
        h.press('3');
        assert_eq!(h.engine.active_preset(), "micro_gain");
    }

    /// The Sweep section holds two layouts, so `3` to `9` name nothing there.
    /// Silence, and specifically **not** a jump into another section: before this
    /// change `[3]` meant `waterfall` from anywhere at all.
    #[test]
    fn a_digit_past_the_end_of_a_section_does_nothing() {
        let mut h = Harness::new();
        h.engine.set_preset("lab_sweep");
        let before = metrics(&h.state).ui.log.len();
        for c in ['3', '7', '9'] {
            h.press(c);
            assert_eq!(h.engine.active_preset(), "lab_sweep", "'{c}' moved us");
        }
        assert_eq!(
            metrics(&h.state).ui.log.len(),
            before,
            "a key that names nothing should say nothing:\n{}",
            h.log()
        );
    }

    /// A slot whose preset has gone from the config says so rather than silently
    /// doing nothing. The menu is built once at construction, so the slot still
    /// resolves to a name; `try_set_preset` is what notices the layout is absent.
    #[test]
    fn a_slot_whose_preset_does_not_exist_logs_instead_of_switching() {
        let mut h = Harness::new();
        let keep = h.engine.active_preset().to_string();
        h.engine.config.presets.retain(|name, _| name == &keep);
        h.press('2');
        assert_eq!(
            h.engine.active_preset(),
            keep,
            "switched to a missing preset"
        );
        assert!(
            h.log().contains("not yet available"),
            "a dead slot must explain itself:\n{}",
            h.log()
        );
    }

    /// A number key must not claim a switch it did not make.
    ///
    /// `[1]`–`[4]` used to call `set_preset` directly and then log
    /// `"Preset: spectrum"` unconditionally - but `set_preset` silently declines
    /// a name it does not know, so with that preset missing the key logged a
    /// switch that had not happened. All nine now go through `try_set_preset`.
    #[test]
    fn a_layout_key_does_not_claim_a_switch_it_did_not_make() {
        let mut h = Harness::new();
        let keep = h.engine.active_preset().to_string();
        h.engine.config.presets.retain(|name, _| name == &keep);

        h.press('2'); // `spectrum` - one of the four that used to lie
        assert_eq!(
            h.engine.active_preset(),
            keep,
            "switched to a missing preset"
        );
        let log = h.log();
        assert!(
            log.contains("not yet available"),
            "the key should say the layout is missing:\n{log}"
        );
        assert!(
            !log.contains("Preset: spectrum"),
            "the key claimed a switch that did not happen:\n{log}"
        );
    }

    /// `[P]` walks the section it is in and closes on itself.
    ///
    /// It used to walk every preset in the config, alphabetically, which made it
    /// the one key that still crossed a section boundary in a design whose whole
    /// premise is that keys do not.
    #[test]
    fn p_cycles_within_the_section_and_returns() {
        let mut h = Harness::new();
        let start = h.engine.active_preset().to_string();
        let n = h.engine.scope().expect("a scope").entries.len();
        assert_eq!(n, 5, "Command Rail holds five layouts");
        for _ in 0..n {
            h.press('p');
        }
        assert_eq!(h.engine.active_preset(), start, "the cycle must close");
    }

    /// And it never leaves the section on the way round.
    #[test]
    fn p_never_leaves_the_section() {
        let mut h = Harness::new();
        h.engine.set_preset("lab_iq");
        for _ in 0..12 {
            h.press('p');
            assert!(
                h.engine.active_preset().starts_with("lab_"),
                "[P] left the Lab section and landed on {}",
                h.engine.active_preset()
            );
        }
    }

    /// `[0]` and `[?]` are retired. Neither may keep doing anything quietly: a
    /// key that still works but is documented nowhere is how the old help drifted
    /// in the first place.
    #[test]
    fn the_retired_keys_do_nothing() {
        let mut h = Harness::new();
        let preset = h.engine.active_preset().to_string();
        let logs_before = metrics(&h.state).ui.log.len();
        for c in ['0', '?'] {
            assert_eq!(h.press(c), KeyAction::Continue);
        }
        assert_eq!(h.engine.active_preset(), preset);
        assert_eq!(metrics(&h.state).ui.log.len(), logs_before);
    }

    /// `handle_no_device` exists so the waterfall's fall-through cannot reach the
    /// radio. With no device the hardware keys must be inert - not merely
    /// harmless, but leaving the requested state untouched.
    #[test]
    fn the_hardware_keys_are_inert_without_a_device() {
        let mut h = Harness::new();
        let before = {
            let m = metrics(&h.state);
            (
                m.radio.rx_enabled,
                m.radio.primary_gain(),
                m.radio.secondary_gain(),
            )
        };
        for c in [' ', 'r', 'f', 's'] {
            h.press(c);
        }
        let after = {
            let m = metrics(&h.state);
            (
                m.radio.rx_enabled,
                m.radio.primary_gain(),
                m.radio.secondary_gain(),
            )
        };
        assert_eq!(before, after, "a hardware key changed state with no device");
    }

    /// Esc leaves focus and clears everything focus mode put on screen. Missing
    /// one of these is how a cursor or a scroll offset survives into a panel that
    /// has no way to clear it.
    #[test]
    fn esc_clears_every_trace_of_focus_mode() {
        let mut h = Harness::new();
        h.engine.focus("waterfall");
        {
            let mut m = metrics(&h.state);
            m.ui.focused_panel = Some("waterfall".to_string());
            m.ui.log_overlay = true;
            m.spectrum.cursor_freq = Some(100_000_000);
            m.waterfall.scroll_offset = 7;
            m.waterfall.cursor_freq = Some(100_000_000);
        }
        h.key(KeyCode::Esc);

        assert!(h.engine.focused_panel_name().is_none());
        let m = metrics(&h.state);
        assert!(m.ui.focused_panel.is_none());
        assert!(!m.ui.log_overlay);
        assert!(m.spectrum.cursor_freq.is_none());
        assert_eq!(m.waterfall.scroll_offset, 0);
        assert!(m.waterfall.cursor_freq.is_none());
    }

    /// An unbound key must fall through silently rather than logging noise or
    /// changing anything.
    #[test]
    fn an_unbound_key_does_nothing_at_all() {
        let mut h = Harness::new();
        let preset = h.engine.active_preset().to_string();
        let logs_before = metrics(&h.state).ui.log.len();
        for c in ['x', 'y', 'z', '@'] {
            assert_eq!(h.press(c), KeyAction::Continue);
        }
        assert_eq!(h.engine.active_preset(), preset);
        assert_eq!(metrics(&h.state).ui.log.len(), logs_before);
    }
    /// The boost key on a device with no boost changes nothing and says so.
    ///
    /// Silently flipping `amp_enabled` would light a lamp on the rail, the micro
    /// gain view and the lab banner for a stage that is not in the radio, and
    /// the gain total would gain 14 dB that does not exist.
    #[test]
    fn the_boost_key_does_nothing_on_a_device_that_has_no_boost() {
        let mut h = Harness::new();
        {
            let mut m = metrics(&h.state);
            *m = SdrMetrics::fixture().named_chain_no_boost();
        }
        let before = metrics(&h.state).radio.amp_enabled;
        h.press('a');
        let m = metrics(&h.state);
        assert_eq!(m.radio.amp_enabled, before, "the flag must not move");
        assert!(
            m.ui.log
                .iter()
                .any(|e| e.text.contains("no front end boost")),
            "and the user is told why nothing happened"
        );
    }
}
