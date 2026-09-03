// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `[C]` command-rail focus: tune with the arrows, cycle the rail mode, recall a
//! saved frequency, and toggle the log overlay.

use crossterm::event::{KeyCode, KeyEvent};

use crate::state::RailMode;

use super::{global, metrics, InputCtx, KeyAction};

// ── Command Rail focus keys ───────────────────────────────────────────────────

/// Walk the stage selection one place, with "the whole chain" as a real stop.
///
/// `None` is not a missing value here, it is the default position of the knob:
/// the ring runs `chain, stage 0, stage 1, ... , chain` so a user can always get
/// back to the one-knob control by pressing on rather than by remembering `Esc`.
fn next_stage(current: Option<usize>, count: usize, forward: bool) -> Option<usize> {
    if count == 0 {
        return None;
    }
    match (current, forward) {
        (None, true) => Some(0),
        (None, false) => Some(count - 1),
        (Some(i), true) if i + 1 < count => Some(i + 1),
        (Some(_), true) => None,
        (Some(0), false) => None,
        (Some(i), false) => Some(i - 1),
    }
}

/// Move one stage by its own step, leaving every other stage where it is.
///
/// The device's own grid decides the step: a HackRF's LNA moves in 8 dB and its
/// VGA in 2, and a driver that reports no step gets 1 dB. Nothing is
/// redistributed, which is the whole point of the mode.
fn step_selected_stage(ctx: &mut InputCtx<'_>, up: bool) {
    let Some(device) = ctx.device else { return };
    let stages = device.capabilities().gain.stages();
    let Some(index) = metrics(ctx.state).ui.gain_stage else {
        return;
    };
    let Some(spec) = stages.get(index) else {
        return;
    };

    let current = metrics(ctx.state).radio.stage_gain(index);
    let step = if spec.step_db > 0.0 {
        spec.step_db
    } else {
        1.0
    };
    let target = if up { current + step } else { current - step };
    // Snap towards the direction of travel, so a value that started off the grid
    // still moves rather than snapping back onto where it already was.
    let next = if up {
        spec.snap(target.max(current + step * 0.5))
    } else {
        spec.snap_down(target)
    };
    let next = next.clamp(spec.min_db, spec.max_db);

    let result = device.set_stage_gain(index, &spec.name, next);
    let mut m = metrics(ctx.state);
    match result {
        Ok(()) => {
            m.radio.set_stage_gain(index, next);
            m.lab.rf_autotrack = false;
            m.ui.note_mode_action(RailMode::Bench);
            m.push_log(format!("{} \u{2192} {next:.0} dB", spec.name));
        }
        Err(e) => m.push_log(format!("Gain error: {e}")),
    }
}

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
                // Leaving focus puts the knob back to the whole chain. The
                // stage *values* stay where they were put: undoing a deliberate
                // setting on the way out would be the surprising half.
                metrics(state).ui.gain_stage = None;
                return global::handle(key, ctx);
            }
        }
        // `,` / `.` walk the device's own stage list, and `None` at either end
        // hands `↑`/`↓` back to the whole-chain knob. Deliberately not `[` / `]`:
        // those already adjust the VGA on a HackRF, and taking a working key
        // away to add a feature is a poor trade.
        KeyCode::Char(',') | KeyCode::Char('.') => {
            let stages = device.map(|d| d.capabilities().gain.stages().len());
            if let Some(count) = stages.filter(|n| *n > 0) {
                let mut m = metrics(state);
                let forward = matches!(key.code, KeyCode::Char('.'));
                m.ui.gain_stage = next_stage(m.ui.gain_stage, count, forward);
                let msg = match m.ui.gain_stage {
                    Some(_) => "stage selected".to_string(),
                    None => "whole chain".to_string(),
                };
                m.push_log(format!("Gain focus: {msg}"));
            }
        }
        // `↑` / `↓` with a stage selected move that stage alone. With none
        // selected they fall through to the whole-chain knob, unchanged.
        KeyCode::Up | KeyCode::Down if metrics(state).ui.gain_stage.is_some() => {
            step_selected_stage(ctx, matches!(key.code, KeyCode::Up));
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The ring includes "the whole chain", so pressing on always gets back to
    /// the default knob without having to remember that `Esc` also does it.
    #[test]
    fn the_stage_ring_passes_through_the_whole_chain() {
        // Two stages, forwards: chain, 0, 1, chain.
        let mut at = None;
        let seen: Vec<Option<usize>> = (0..4)
            .map(|_| {
                at = next_stage(at, 2, true);
                at
            })
            .collect();
        assert_eq!(seen, vec![Some(0), Some(1), None, Some(0)]);

        // And backwards is the exact reverse.
        let mut at = None;
        let seen: Vec<Option<usize>> = (0..4)
            .map(|_| {
                at = next_stage(at, 2, false);
                at
            })
            .collect();
        assert_eq!(seen, vec![Some(1), Some(0), None, Some(1)]);
    }

    /// One stage is still a ring, just a shorter one.
    #[test]
    fn a_single_stage_device_toggles_between_it_and_the_chain() {
        assert_eq!(next_stage(None, 1, true), Some(0));
        assert_eq!(next_stage(Some(0), 1, true), None);
        assert_eq!(next_stage(Some(0), 1, false), None);
    }

    /// A device with no stages has nothing to point at, and the mode must not
    /// offer a selection that would then index nothing.
    #[test]
    fn a_device_with_no_stages_cannot_be_pointed_at_one() {
        assert_eq!(next_stage(None, 0, true), None);
        assert_eq!(next_stage(Some(3), 0, false), None);
    }
}
