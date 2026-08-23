//! Keyboard dispatch.
//!
//! Three layers, in this order:
//!
//! 1. **Input mode.** [`handle_key`] looks at `UiState::input_mode` first: while
//!    a frequency, sample rate, sweep bound or marker label is being typed, the
//!    keyboard belongs to [`text`] and nothing else sees it.
//! 2. **Panel focus.** [`handle_normal`] asks the layout engine which panel holds
//!    focus and hands the key to that panel's own handler — [`core`], [`bench`],
//!    [`signal`], [`sweep`] or [`rail`].
//! 3. **Global.** Anything a focus handler does not claim falls through to
//!    [`global`], which is also where an unfocused key lands.
//!
//! Every handler below the first layer takes the same [`InputCtx`], so adding a
//! panel handler means writing one function and one line in the dispatch table —
//! not threading six more parameters through the file.

mod bench;
mod core;
mod global;
mod rail;
mod signal;
mod sweep;
mod text;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use crossterm::event::{KeyCode, KeyEvent};

use crate::hardware;
use crate::state::{InputMode, SdrMetrics};
use crate::ui;

pub enum KeyAction {
    Continue,
    Quit,
}

/// Everything a key handler is allowed to touch.
///
/// This used to be seven parameters repeated on every handler and forwarded by
/// hand at each fall-through, which is why some handlers took `device` and some
/// did not, and why the waterfall's fall-through needed a separate
/// `handle_global_no_device` to express "not this one".
///
/// The shared references are `Copy`, so a handler can pull `state` and `device`
/// out into locals and still pass `ctx` on to [`global::handle`] afterwards —
/// that is what keeps the fall-through a single line.
pub(super) struct InputCtx<'a> {
    pub state: &'a Arc<Mutex<SdrMetrics>>,
    /// `None` in observer mode, when the radio belongs to another process — and
    /// temporarily `None` inside [`global::handle_no_device`].
    pub device: Option<&'a Arc<dyn hardware::SdrDevice>>,
    pub engine: &'a mut ui::LayoutEngine,
    pub show_help: &'a mut bool,
    pub show_footer: &'a mut bool,
    /// Focus key → panel name, harvested from the registry by `App::build_ui`.
    pub focus_keys: &'a HashMap<char, &'static str>,
}

/// The metrics, locked.
///
/// A poisoned mutex must not kill the TUI — a panic while one handler holds the
/// lock would otherwise take the whole app down on the *next* key. Recovering the
/// guard is that rule, and it was written out at all 122 lock sites in this module
/// before it was written down once here.
pub(super) fn metrics(state: &Arc<Mutex<SdrMetrics>>) -> MutexGuard<'_, SdrMetrics> {
    state.lock().unwrap_or_else(|e| e.into_inner())
}

#[allow(clippy::too_many_arguments)]
pub fn handle_key(
    key: KeyEvent,
    state: &Arc<Mutex<SdrMetrics>>,
    device: Option<&Arc<dyn hardware::SdrDevice>>,
    engine: &mut ui::LayoutEngine,
    show_help: &mut bool,
    show_footer: &mut bool,
    focus_keys: &HashMap<char, &'static str>,
) -> KeyAction {
    let mut ctx = InputCtx { state, device, engine, show_help, show_footer, focus_keys };
    let input_mode = metrics(state).ui.input_mode.clone();
    match input_mode {
        InputMode::Normal          => handle_normal(key, &mut ctx),
        InputMode::FrequencyInput  => { text::frequency(key, state, device);    KeyAction::Continue }
        InputMode::SampleRateInput => { text::sample_rate(key, state, device);  KeyAction::Continue }
        InputMode::MarkerNameInput => { text::marker_name(key, state);          KeyAction::Continue }
        InputMode::SweepStartInput => { text::sweep_range(key, state, true);    KeyAction::Continue }
        InputMode::SweepStopInput  => { text::sweep_range(key, state, false);   KeyAction::Continue }
    }
}

/// Fold an uppercase letter key onto its lowercase twin, leaving every other key
/// alone. See the note in [`handle_normal`] for why this exists and why it lives
/// there rather than at [`handle_key`].
///
/// ASCII only, and letters only: a shifted digit or symbol arrives as its own
/// character (`Shift+1` is `!`), so there is nothing to fold, and folding a
/// non-ASCII character would change what a non-English keyboard sent.
fn fold_key_case(key: KeyEvent) -> KeyEvent {
    match key.code {
        KeyCode::Char(c) if c.is_ascii_uppercase() =>
            KeyEvent { code: KeyCode::Char(c.to_ascii_lowercase()), ..key },
        _ => key,
    }
}

/// The dispatch table: focused panel name → that panel's handler.
///
/// The handler is named after the panel it serves, so this reads as a list of
/// pairs and a missing entry is visible rather than inferred.
fn handle_normal(key: KeyEvent, ctx: &mut InputCtx<'_>) -> KeyAction {
    // Every key hint in the app is capitalised — `[C] Snapshot to log`, `[Q] Quit`,
    // `[J K] Cursor`, `[S/E] Start/End` — but the handlers below are written against
    // lowercase, and only some of them spelled out `Char('c') | Char('C')`. So the
    // same displayed key worked with Shift on the IQ bench and did nothing on the
    // characterization panel next to it, and Shift+Q never quit anywhere.
    //
    // Folding case here rather than in twelve match arms makes it one rule that a
    // new handler cannot forget. It sits in `handle_normal`, not `handle_key`, so
    // the text-entry modes — a marker label is typed, capitals and all — are
    // untouched.
    let key = fold_key_case(key);
    let focused = ctx.engine.focused_panel_name().map(|s| s.to_string());

    match focused.as_deref() {
        Some("spectrum")                => core::spectrum(key, ctx),
        Some("waterfall")               => core::waterfall(key, ctx),
        Some("iq_diagnostics")          => bench::iq_diagnostics(key, ctx),
        Some("rf_chain")                => bench::rf_chain(key, ctx),
        Some("timing_vitals")           => bench::timing_vitals(key, ctx),
        Some("timing_diagnostics")      => bench::timing_diagnostics(key, ctx),
        Some("lab_banner")              => bench::lab_banner(key, ctx),
        Some("signal_metrics")          => signal::signal_metrics(key, ctx),
        Some("signal_characterization") => signal::signal_characterization(key, ctx),
        Some("fm_demod")                => signal::fm_demod(key, ctx),
        Some("sweep_panel")             => sweep::sweep_panel(key, ctx),
        Some("command_rail")            => rail::command_rail(key, ctx),
        _                               => global::handle(key, ctx),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn ev(code: KeyCode) -> KeyEvent { KeyEvent::new(code, KeyModifiers::NONE) }

    #[test]
    fn fold_key_case_matches_the_capitalised_hints() {
        // C6: every hint in the app is capitalised (`[C] Snapshot to log`,
        // `[Q] Quit`, `[S/E] Start/End`) while the handlers are written lowercase.
        for (from, to) in [('C', 'c'), ('Q', 'q'), ('J', 'j'), ('S', 's')] {
            assert_eq!(fold_key_case(ev(KeyCode::Char(from))).code, KeyCode::Char(to));
        }
        // Lowercase and non-letters pass through untouched.
        for c in ['c', '0', '[', '+', '/'] {
            assert_eq!(fold_key_case(ev(KeyCode::Char(c))).code, KeyCode::Char(c));
        }
        assert_eq!(fold_key_case(ev(KeyCode::Left)).code, KeyCode::Left);
    }

    #[test]
    fn fold_key_case_leaves_non_ascii_alone() {
        // A non-English layout can send letters whose "lowercase" is not what the
        // handlers match on; folding them would change the key that was pressed.
        for c in ['\u{00c1}', '\u{0150}', '\u{00dc}'] {
            assert_eq!(fold_key_case(ev(KeyCode::Char(c))).code, KeyCode::Char(c));
        }
    }

    #[test]
    fn fold_key_case_keeps_the_modifiers() {
        // Shift is still on the event after the fold; nothing downstream reads it
        // today, and a handler that starts to must see what was actually pressed.
        let k = KeyEvent::new(KeyCode::Char('C'), KeyModifiers::SHIFT);
        let folded = fold_key_case(k);
        assert_eq!(folded.code, KeyCode::Char('c'));
        assert_eq!(folded.modifiers, KeyModifiers::SHIFT);
    }
}
