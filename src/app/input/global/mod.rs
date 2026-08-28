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
//! global key, and `check-docs.py` reads it as source text to check the manual
//! against.

mod gain;
mod presets;
mod radio;
mod view;

use crossterm::event::{KeyCode, KeyEvent};

use super::{InputCtx, KeyAction};

pub(super) fn handle(key: KeyEvent, ctx: &mut InputCtx<'_>) -> KeyAction {
    match key.code {
        KeyCode::Esc => view::leave_focus(ctx),
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
        KeyCode::Char('?') => *ctx.show_help = !*ctx.show_help,
        KeyCode::Tab => *ctx.show_footer = !*ctx.show_footer,
        KeyCode::Char('w') => view::toggle_waterfall_pause(ctx),
        KeyCode::Char('h') => view::toggle_hold(ctx),

        // ── Layouts ─────────────────────────────────────────────────────────
        KeyCode::Char('p') => {
            ctx.engine.cycle_preset();
            let name = ctx.engine.active_preset().to_string();
            super::metrics(ctx.state).push_log(format!("Preset: {}", name));
        }
        KeyCode::Char('1') => {
            return presets::try_set_preset(ctx.engine, ctx.state, "command_rail")
        }
        KeyCode::Char('2') => return presets::try_set_preset(ctx.engine, ctx.state, "spectrum"),
        KeyCode::Char('3') => return presets::try_set_preset(ctx.engine, ctx.state, "waterfall"),
        KeyCode::Char('4') => {
            return presets::try_set_preset(ctx.engine, ctx.state, "spectrum_waterfall")
        }
        KeyCode::Char('5') => return presets::try_set_preset(ctx.engine, ctx.state, "lab_iq"),
        KeyCode::Char('6') => return presets::try_set_preset(ctx.engine, ctx.state, "lab_rf"),
        KeyCode::Char('7') => return presets::try_set_preset(ctx.engine, ctx.state, "lab_timing"),
        KeyCode::Char('8') => return presets::try_set_preset(ctx.engine, ctx.state, "lab_signal"),
        KeyCode::Char('9') => return presets::try_set_preset(ctx.engine, ctx.state, "lab_sweep"),
        KeyCode::Char('0') => presets::cycle_micro(ctx.engine, ctx.state),

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
        show_help: bool,
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
                show_help: false,
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
                show_help: &mut self.show_help,
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
    fn the_overlay_keys_toggle_rather_than_latch() {
        let mut h = Harness::new();
        assert!(!h.show_help);
        h.press('?');
        assert!(h.show_help);
        h.press('?');
        assert!(!h.show_help, "[?] must toggle, not latch");

        assert!(h.show_footer);
        h.key(KeyCode::Tab);
        assert!(!h.show_footer);
    }

    /// The number keys hard-code preset names, so a renamed preset would leave a
    /// slot pointing at nothing. `try_set_preset` is what makes that say so
    /// instead of silently doing nothing.
    #[test]
    fn a_number_key_switches_preset_and_says_which() {
        let mut h = Harness::new();
        h.press('2');
        assert_eq!(h.engine.active_preset(), "spectrum");
        assert!(h.log().contains("Preset"), "no log line:\n{}", h.log());
    }

    #[test]
    fn a_slot_whose_preset_does_not_exist_logs_instead_of_switching() {
        let mut h = Harness::new();
        // Empty the preset table down to the one we are on, so every named slot
        // resolves to nothing. The engine must stay put and say why.
        let keep = h.engine.active_preset().to_string();
        h.engine.config.presets.retain(|name, _| name == &keep);
        h.press('7');
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

    #[test]
    fn p_cycles_through_every_preset_and_returns() {
        let mut h = Harness::new();
        let n = h.engine.preset_names().len();
        let start = h.engine.active_preset().to_string();
        for _ in 0..n {
            h.press('p');
        }
        assert_eq!(h.engine.active_preset(), start, "the cycle must close");
    }

    /// `handle_no_device` exists so the waterfall's fall-through cannot reach the
    /// radio. With no device the hardware keys must be inert - not merely
    /// harmless, but leaving the requested state untouched.
    #[test]
    fn the_hardware_keys_are_inert_without_a_device() {
        let mut h = Harness::new();
        let before = {
            let m = metrics(&h.state);
            (m.radio.rx_enabled, m.radio.lna_gain, m.radio.vga_gain)
        };
        for c in [' ', 'r', 'f', 's'] {
            h.press(c);
        }
        let after = {
            let m = metrics(&h.state);
            (m.radio.rx_enabled, m.radio.lna_gain, m.radio.vga_gain)
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
}
