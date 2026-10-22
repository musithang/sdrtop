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

mod export;
mod gain;
// `menu` reaches `try_set_preset` too: the menu and the number keys must load
// a layout the same way, or the two could drift.
pub(super) mod presets;
mod radio;
pub(super) mod view;

use crossterm::event::{KeyCode, KeyEvent};

use super::{InputCtx, KeyAction};

pub(super) fn handle(key: KeyEvent, ctx: &mut InputCtx<'_>) -> KeyAction {
    match key.code {
        KeyCode::Esc => view::leave_focus_or_open_menu(ctx),
        KeyCode::Char('q') => return KeyAction::Quit,

        // ── The radio ───────────────────────────────────────────────────────
        KeyCode::Char(' ') => radio::toggle_rx(ctx),
        KeyCode::Char('r') => radio::reset_defaults(ctx),
        KeyCode::Char('y') => radio::capture_reference(ctx),
        // Section-scoped, and it declines rather than absorbing: see the `m` arm.
        KeyCode::Char('o') => {
            if !export::export_section(ctx) {
                view::enter_focus(ctx, 'o');
            }
        }
        // NET, locked: the next or previous place this view listens.
        KeyCode::Left | KeyCode::Right => {
            radio::step_net_channel(ctx, key.code == KeyCode::Right);
        }
        KeyCode::Char('f') => radio::begin_frequency_input(ctx),
        KeyCode::Char('s') => radio::begin_sample_rate_input(ctx),
        // Section-scoped, and it declines rather than absorbing: outside NET
        // this is still the FM demodulator's focus key.
        KeyCode::Char('m') => {
            if !radio::toggle_net_mode(ctx) {
                view::enter_focus(ctx, 'm');
            }
        }
        // Section-scoped and declining, like `m`: outside NET this is still the
        // IQ diagnostics panel's focus key, and no NET preset shows that panel.
        KeyCode::Char('i') => {
            if !radio::cycle_address_display(ctx) {
                view::enter_focus(ctx, 'i');
            }
        }

        // ── Gain staging ────────────────────────────────────────────────────
        // With one stage picked (`,` / `.`), the arrows move that stage alone.
        KeyCode::Up | KeyCode::Down if super::metrics(ctx.state).ui.gain_stage.is_some() => {
            gain::step_selected_stage(ctx, matches!(key.code, KeyCode::Up))
        }
        KeyCode::Up => gain::step_primary(ctx, true),
        KeyCode::Down => gain::step_primary(ctx, false),
        KeyCode::Char(',') => gain::select_stage(ctx, false),
        KeyCode::Char('.') => gain::select_stage(ctx, true),
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
        focus_keys: crate::app::FocusKeys,
        device: Option<Arc<dyn crate::hardware::SdrDevice>>,
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
                device: None,
            }
        }

        /// The keys on a fixture Airspy R2 at 92.8 MHz, inside its LNA's
        /// measured band, with its stages at `gains`.
        fn on_r2(r2: &crate::hardware::native::airspy::bench::BenchR2, gains: [f64; 3]) -> Self {
            let mut h = Self::new();
            h.device = Some(Arc::clone(&r2.device));
            let mut m = metrics(&h.state);
            m.caps = Arc::new(r2.device.capabilities().clone());
            m.radio.frequency = 92_800_000;
            m.radio.gains = gains.to_vec();
            drop(m);
            h
        }

        fn press(&mut self, c: char) -> KeyAction {
            self.key(KeyCode::Char(c))
        }

        fn key(&mut self, code: KeyCode) -> KeyAction {
            let mut ctx = InputCtx {
                state: &self.state,
                device: self.device.as_ref(),
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

    /// **A section-scoped key declines outside its section rather than
    /// absorbing.**
    ///
    /// `m` toggles the NET survey/lock mode, and `m` was already the FM
    /// demodulator's focus key. A global arm that simply returned would have
    /// made that panel unreachable on every deck outside NET, and nothing would
    /// have said so: `every_focusable_panel_has_a_dispatch_arm` checks that a
    /// focusable panel *has* a handler, not that its key still gets there.
    ///
    /// The rule generalises past this key, which is why it is written as one: a
    /// global arm added for one section must fall through when that section is
    /// not on screen, or it quietly takes a key away from the rest of the deck.
    /// Locked on the BLE view, `→` asks for the next advertising channel,
    /// and a second press before the first is applied goes on from it;
    /// surveying, the key says what it is for and asks for nothing.
    #[test]
    fn the_arrows_step_a_locked_view_and_explain_themselves_otherwise() {
        let mut h = Harness::new();
        {
            let mut m = metrics(&h.state);
            m.ui.section = crate::signal::net::SECTION.to_string();
            m.ui.active_preset = "net_ble".to_string();
            m.radio.frequency = 2_402_000_000;
        }
        h.key(KeyCode::Right);
        assert!(metrics(&h.state).net.lock_at.is_none());
        assert!(h.log().contains("[M] locks"), "{}", h.log());

        metrics(&h.state).net.mode = crate::state::NetMode::Lock;
        h.key(KeyCode::Right);
        let first = metrics(&h.state).net.lock_at.clone().unwrap();
        assert_eq!(first.tune_hz, 2_426_000_000);
        h.key(KeyCode::Right);
        assert_eq!(
            metrics(&h.state).net.lock_at.clone().unwrap().tune_hz,
            2_480_000_000
        );
        h.key(KeyCode::Left);
        assert_eq!(
            metrics(&h.state).net.lock_at.clone().unwrap().tune_hz,
            2_426_000_000
        );
    }

    #[test]
    fn a_section_scoped_key_does_not_shadow_a_panel_focus_key() {
        let mut h = Harness::new();
        h.focus_keys.insert('m', vec!["fm_demod"]);
        h.engine.set_preset("lab_signal");

        // Outside NET, `m` is still the focus key it has always been.
        assert!(!metrics(&h.state).ui.is_net_section());
        h.press('m');
        assert_eq!(
            metrics(&h.state).ui.focused_panel.as_deref(),
            Some("fm_demod"),
            "the global arm swallowed the focus key"
        );
        assert_eq!(metrics(&h.state).net.mode, crate::state::NetMode::Survey);

        // Inside it, the same key is the mode and nothing gets focused.
        let mut h = Harness::new();
        h.focus_keys.insert('m', vec!["fm_demod"]);
        metrics(&h.state).ui.section = crate::signal::net::SECTION.to_string();
        h.press('m');
        assert_eq!(metrics(&h.state).net.mode, crate::state::NetMode::Lock);
        assert_eq!(metrics(&h.state).ui.focused_panel, None);
        h.press('m');
        assert_eq!(metrics(&h.state).net.mode, crate::state::NetMode::Survey);
        assert!(h.log().contains("surveying the band"), "{}", h.log());
    }

    /// `i` cycles the address display inside NET and says so in the log;
    /// outside it, `i` is still the IQ diagnostics focus key, the same
    /// declining rule as `m` above.
    #[test]
    fn the_address_key_cycles_inside_net_and_focuses_outside_it() {
        use crate::state::AddressDisplay;
        let mut h = Harness::new();
        h.focus_keys.insert('i', vec!["iq_diagnostics"]);
        h.engine.set_preset("lab_iq");
        h.press('i');
        assert_eq!(metrics(&h.state).net.address_display, AddressDisplay::Full);
        assert_eq!(
            metrics(&h.state).ui.focused_panel.as_deref(),
            Some("iq_diagnostics"),
            "the address switch swallowed the focus key"
        );

        let mut h = Harness::new();
        h.focus_keys.insert('i', vec!["iq_diagnostics"]);
        metrics(&h.state).ui.section = crate::signal::net::SECTION.to_string();
        h.press('i');
        assert_eq!(metrics(&h.state).net.address_display, AddressDisplay::Oui);
        assert_eq!(metrics(&h.state).ui.focused_panel, None);
        assert!(h.log().contains("addresses shown: oui"), "{}", h.log());
        h.press('i');
        assert_eq!(
            metrics(&h.state).net.address_display,
            AddressDisplay::Masked
        );
        h.press('i');
        assert_eq!(metrics(&h.state).net.address_display, AddressDisplay::Full);
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
        for c in ['x', 'z', '@'] {
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

    // ── The gain keys on a radio whose stages are measured tables ───────────

    use crate::hardware::native::airspy::bench::BenchR2;

    fn gains(h: &Harness) -> Vec<f64> {
        metrics(&h.state).radio.gains.clone()
    }

    /// **Every gain key reaches its own register.** `↑` the LNA, `]` the
    /// mixer, and the VGA once `,` has picked it: each lands on the next
    /// measured position, in the radio and in the state alike. `]` used to
    /// call a setter the R2 does not have and report the mixer moved.
    #[test]
    fn on_the_r2_every_gain_key_reaches_its_own_register() {
        let r2 = BenchR2::new();
        let mut h = Harness::on_r2(&r2, [23.40, 8.25, 0.0]);
        h.key(KeyCode::Up);
        h.press(']');
        assert_eq!(r2.registers(), [9, 7, -1]);
        assert_eq!(gains(&h), [26.96, 9.41, 0.0]);
        // Backwards from the stages by hand, the ring's last stop: the VGA.
        h.press(',');
        h.key(KeyCode::Up);
        assert_eq!(r2.registers(), [9, 7, 1]);
        assert_eq!(gains(&h), [26.96, 9.41, 3.40]);
    }

    /// **A measured table is walked position by position**, however close two
    /// of its values are: the mixer's 15.90, 16.16, 16.17 and 16.24 dB are
    /// four settings, not one 16 dB that the key cannot leave.
    #[test]
    fn a_measured_table_is_walked_position_by_position() {
        let r2 = BenchR2::new();
        let mut h = Harness::on_r2(&r2, [11.28, 15.90, 0.0]);
        let mut mixer = Vec::new();
        for _ in 0..3 {
            h.press(']');
            mixer.push((r2.registers()[1], gains(&h)[1]));
        }
        assert_eq!(mixer, [(13, 16.16), (14, 16.17), (15, 16.24)]);
        h.key(KeyCode::Up);
        assert_eq!((r2.registers()[0], gains(&h)[0]), (4, 11.75));
        h.key(KeyCode::Down);
        h.key(KeyCode::Down);
        assert_eq!((r2.registers()[0], gains(&h)[0]), (2, 9.09));
    }

    /// The stage picked on the ring walks its table the same way.
    #[test]
    fn a_picked_stage_walks_its_table_too() {
        let r2 = BenchR2::new();
        let mut h = Harness::on_r2(&r2, [27.76, 8.25, 0.0]);
        // Linearity, Sensitivity, then the LNA alone.
        for _ in 0..3 {
            h.press('.');
        }
        h.key(KeyCode::Up);
        assert_eq!((r2.registers()[0], gains(&h)[0]), (11, 28.19));
        h.key(KeyCode::Down);
        assert_eq!((r2.registers()[0], gains(&h)[0]), (10, 27.76));
    }

    /// **The log says where the stage is**: its measured dB and its position
    /// inside the band the dB holds in, the position alone outside it.
    #[test]
    fn the_log_names_a_measured_stage_by_its_reading() {
        let r2 = BenchR2::new();
        let mut h = Harness::on_r2(&r2, [23.40, 15.90, 0.0]);
        h.press(']');
        assert!(
            h.log().contains("MIX gain \u{2192} 16.16 dB (13/15)"),
            "{}",
            h.log()
        );
        metrics(&h.state).radio.frequency = 943_400_000;
        h.key(KeyCode::Up);
        assert!(
            h.log().contains("LNA gain \u{2192} step 9/14"),
            "{}",
            h.log()
        );
    }

    /// **`[R]` puts the knob where a first run starts it**: on Linearity at
    /// step 10, every stage set by position in the radio and in the state,
    /// from a chain that was off the curve and on no curve at all.
    #[test]
    fn reset_puts_the_r2_back_on_its_start_curve() {
        let r2 = BenchR2::new();
        let mut h = Harness::on_r2(&r2, [36.02, 12.03, 26.79]);
        h.press('r');
        assert_eq!(gains(&h), [16.74, 1.14, 33.51]);
        assert_eq!(metrics(&h.state).radio.gain_curve, Some(0));
        assert_eq!(r2.registers(), [6, 1, 10]);
    }

    /// **`[A]` switches the R2's AGC, and a hand on a stage it governs
    /// switches it off first**: as libairspy's own presets do, and as an
    /// RTL-SDR's manual gain always has. Said in the log, both ways.
    #[test]
    fn the_agc_is_a_key_and_a_hand_on_its_stages_switches_it_off() {
        let r2 = BenchR2::new();
        let mut h = Harness::on_r2(&r2, [0.0, 0.0, 0.0]);
        on_curve(&h, 0, 10);
        h.press('a');
        assert!(metrics(&h.state).radio.amp_enabled);
        h.key(KeyCode::Up);
        assert!(!metrics(&h.state).radio.amp_enabled);
        assert_eq!(r2.registers(), [8, 0, 10], "Linearity 11, set by hand");
        assert!(h.log().contains("AGC off"), "{}", h.log());
        // The VGA has no AGC: moving it alone leaves the AGC on.
        h.press('a');
        h.press(',');
        h.key(KeyCode::Up);
        assert!(metrics(&h.state).radio.amp_enabled);
    }

    fn on_curve(h: &Harness, curve: usize, step: usize) {
        let mut m = metrics(&h.state);
        let gm = m.caps.gain.clone();
        m.radio.gain_curve = Some(curve);
        m.radio.gains = gm.curves()[curve].values(step, &gm.stages());
    }

    /// **On a curve, `↑↓` walks it**: one step, all three stages set by
    /// position to that step's entries, and the log names the curve, the step
    /// and what each stage now is.
    #[test]
    fn on_a_curve_the_arrows_walk_it_and_set_every_stage() {
        let r2 = BenchR2::new();
        let mut h = Harness::on_r2(&r2, [0.0, 0.0, 0.0]);
        on_curve(&h, 0, 10);
        h.key(KeyCode::Up);
        // Linearity 11: LNA 8, mixer 0, VGA 10.
        assert_eq!(r2.registers(), [8, 0, 10]);
        assert_eq!(gains(&h), [23.40, 0.0, 33.51]);
        assert!(h.log().contains("Linearity 11/21"), "{}", h.log());
        for _ in 0..30 {
            h.key(KeyCode::Down);
        }
        assert_eq!(r2.registers(), [0, 0, 4], "the bottom of the curve");
        on_curve(&h, 1, 12);
        h.key(KeyCode::Up);
        // Sensitivity 13: LNA 14, mixer 9, VGA 5.
        assert_eq!(r2.registers(), [14, 9, 5]);
    }

    /// **A chain set off the curve by hand rejoins it at its nearest step**:
    /// the first press moves from there, not from wherever the curve was.
    #[test]
    fn a_hand_set_chain_rejoins_the_curve_at_its_nearest_step() {
        let r2 = BenchR2::new();
        // LNA 14, mixer 15, VGA 15: nearest Linearity 21 (14, 12, 13).
        let mut h = Harness::on_r2(&r2, [36.02, 16.24, 49.96]);
        metrics(&h.state).radio.gain_curve = Some(0);
        h.key(KeyCode::Down);
        // Linearity 20: LNA 14, mixer 12, VGA 12.
        assert_eq!(r2.registers(), [14, 12, 12]);
    }

    /// **The ring holds the curves, then each stage alone**: `.` from the
    /// stages by hand goes to Linearity, Sensitivity, the LNA, mixer and VGA
    /// alone, and round again; picking a stage takes the knob off the curve.
    #[test]
    fn the_ring_runs_through_the_curves_then_each_stage() {
        let r2 = BenchR2::new();
        let mut h = Harness::on_r2(&r2, [0.0, 0.0, 0.0]);
        let mut seen = Vec::new();
        for _ in 0..6 {
            h.press('.');
            let m = metrics(&h.state);
            seen.push((m.radio.gain_curve, m.ui.gain_stage));
        }
        assert_eq!(
            seen,
            [
                (Some(0), None),
                (Some(1), None),
                (None, Some(0)),
                (None, Some(1)),
                (None, Some(2)),
                (Some(0), None),
            ]
        );
        h.press(',');
        let picked = {
            let m = metrics(&h.state);
            (m.radio.gain_curve, m.ui.gain_stage)
        };
        assert_eq!(picked, (None, Some(2)), "and backwards");
        assert!(h.log().contains("Gain: Linearity curve"), "{}", h.log());
    }
}
