//! `[C]` command-rail focus: tune with the arrows, cycle the rail mode, recall a
//! saved frequency, and toggle the log overlay.

use crossterm::event::{KeyCode, KeyEvent};

use crate::state::RailMode;

use super::{global, metrics, InputCtx, KeyAction};

// ── Command Rail focus keys ───────────────────────────────────────────────────

/// `command_rail` focus (`[C]`): `←/→` tune by the spectrum step (which auto-
/// switches the lead card to Hunt), `Tab` cycles the mode manually. Recall slots
/// (`1·2·3·M`) and the log overlay (`L`) arrive in later steps. Every other key
/// falls through to the global handler (so `Esc` exits focus as usual).
pub(super) fn command_rail(key: KeyEvent, ctx: &mut InputCtx<'_>) -> KeyAction {
    let (state, device) = (ctx.state, ctx.device);
    match key.code {
        // Esc closes the log overlay first (if open), only then exits focus.
        KeyCode::Esc => {
            let closed = {
                let mut m = metrics(state);
                if m.ui.log_overlay {
                    m.ui.log_overlay = false;
                    true
                } else {
                    false
                }
            };
            if !closed {
                return global::handle(key, ctx);
            }
        }
        // Toggle the full-log overlay (in rail-focus; globally `l` focuses waterfall).
        KeyCode::Char('l') => {
            let mut m = metrics(state);
            m.ui.log_overlay = !m.ui.log_overlay;
        }
        KeyCode::Left | KeyCode::Right => {
            if let Some(device) = device {
                let caps = device.capabilities();
                let new_freq = {
                    let m = metrics(state);
                    let step = m.spectrum.step_hz;
                    if matches!(key.code, KeyCode::Left) {
                        m.radio.frequency.saturating_sub(step).max(caps.freq_min_hz)
                    } else {
                        (m.radio.frequency + step).min(caps.freq_max_hz)
                    }
                };
                let result = device.set_frequency(new_freq);
                let mut m = metrics(state);
                match result {
                    Ok(()) => {
                        m.radio.frequency = new_freq;
                        m.ui.note_mode_action(RailMode::Hunt);
                    }
                    Err(e) => m.push_log(format!("Tune error: {}", e)),
                }
            }
        }
        KeyCode::Tab => {
            let mut m = metrics(state);
            let mode = m.ui.cycle_rail_mode();
            m.push_log(format!("Rail mode: {}", mode.label()));
        }
        // Save the current tuning into a recall slot (free slot, else oldest).
        KeyCode::Char('m') => {
            let mut m = metrics(state);
            let freq = m.radio.frequency;
            let slot = m.ui.save_recall(freq);
            m.push_log(format!(
                "Recall {} ← {:.3} MHz",
                slot + 1,
                freq as f64 / 1e6
            ));
        }
        // Jump to recall slot 1/2/3 (rail-focus only; globally these switch presets).
        KeyCode::Char(c @ '1'..='3') => {
            let slot = c as usize - '1' as usize;
            let target = { metrics(state).ui.recall[slot] };
            match (target, device) {
                (Some(hz), Some(device)) => {
                    let result = device.set_frequency(hz);
                    let mut m = metrics(state);
                    match result {
                        Ok(()) => {
                            m.radio.frequency = hz;
                            m.ui.note_mode_action(RailMode::Hunt);
                            m.push_log(format!("Recall {} → {:.3} MHz", slot + 1, hz as f64 / 1e6));
                        }
                        Err(e) => m.push_log(format!("Recall error: {}", e)),
                    }
                }
                (None, _) => {
                    metrics(state)
                        .push_log(format!("Recall {} is empty — save with [M]", slot + 1));
                }
                _ => {}
            }
        }
        _ => return global::handle(key, ctx),
    }
    KeyAction::Continue
}
