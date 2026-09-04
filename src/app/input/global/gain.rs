// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The gain keys: `↑`/`↓` on the primary stage, `[`/`]` on the VGA, `[A]` on the
//! front-end boost.
//!
//! All four share one shape - read the current value, ask the device to change
//! it, and only write the state back if the device agreed. The device call
//! happens with **no lock held**, which is why each of these takes the guard
//! twice rather than holding it across the USB round trip.
//!
//! `↑` and `↓` were 26 identical lines apart from one `true`/`false`, and `[` and
//! `]` twenty more; they are one function each now, with the direction as an
//! argument.

use crate::hardware::GainModel;
use crate::state::RailMode;

use super::super::{metrics, InputCtx};

/// HackRF's VGA moves in 2 dB steps across 0–62 dB.
const VGA_STEP_DB: u32 = 2;
const VGA_MAX_DB: u32 = 62;
/// How far a continuous stage moves per keypress.
///
/// A stage that reports no step of its own has to be given one. Small on
/// purpose: such a range can be 0 to 116 dB on a HackRF chain and 0 to 0 on a
/// sound card, and a big step would be useless at one end of that.
const CONTINUOUS_STEP_DB: u32 = 1;

/// Next value for the primary front-end gain when stepping up/down.
///
/// **Asked of the front stage, not of the device family.** A stage with a table
/// walks to the neighbouring entry, one with a grid moves by its own step, and
/// a continuous one moves by [`CONTINUOUS_STEP_DB`] because it has none to
/// follow. That covers a HackRF's 8 dB LNA, an RTL-SDR's irregular tuner table
/// and whatever a driver reported, without naming any of them.
///
/// A device whose front stage does not exist cannot be stepped and is left
/// alone: an RTL-SDR that reported no gain table is exactly that case.
pub(super) fn next_primary_gain(gain: &GainModel, current: u32, up: bool) -> u32 {
    let stages = gain.stages();
    let Some(front) = stages.first() else {
        return current;
    };

    if !front.table.is_empty() {
        let idx = front
            .table
            .iter()
            .enumerate()
            .min_by_key(|(_, &g)| (g - current as f64).abs().round() as i64)
            .map(|(i, _)| i)
            .unwrap_or(0);
        let next = if up {
            (idx + 1).min(front.table.len() - 1)
        } else {
            idx.saturating_sub(1)
        };
        return front.table[next].max(0.0).round() as u32;
    }

    let lo = front.min_db.max(0.0).round() as u32;
    let hi = front.max_db.max(front.min_db).max(0.0).round() as u32;
    let step = if front.step_db > 0.0 {
        (front.step_db.round() as u32).max(1)
    } else {
        CONTINUOUS_STEP_DB
    };
    if up {
        (current + step).clamp(lo, hi)
    } else {
        current.saturating_sub(step).clamp(lo, hi)
    }
}

/// `↑` / `↓` - step the primary front-end stage.
///
/// On a device that presents its stages separately, HackRF and RTL-SDR, this
/// moves the front one and nothing else, exactly as it always did.
///
/// On a **SoapySDR** device the knob is one figure for the whole chain, and
/// sdrtop now spreads that figure itself rather than handing it to `setGain`.
/// See [`step_distributed`].
pub(super) fn step_primary(ctx: &mut InputCtx<'_>, up: bool) {
    let Some(device) = ctx.device else { return };
    // A knob that moves a chain has to be distributed across it. Asked as
    // "more than one stage and no key of its own for the second", which is what
    // that has always meant: a HackRF drives its two stages from two keys, and
    // an RTL-SDR's single tuner is stepped directly.
    let gm = &device.capabilities().gain;
    if !gm.has_second_stage() && gm.stages().len() > 1 {
        step_distributed(ctx, up);
        return;
    }
    let gain = &device.capabilities().gain;
    let current = metrics(ctx.state).radio.primary_gain();
    let new_gain = next_primary_gain(gain, current, up);

    let result = device.set_lna_gain(new_gain);
    let mut m = metrics(ctx.state);
    match result {
        Ok(()) => {
            m.radio.set_primary_gain(new_gain);
            // On a single-tuner device, setting a manual gain turns the tuner AGC
            // off in hardware - keep the UI's AGC flag in sync.
            if gain.is_single() {
                m.radio.amp_enabled = false;
            }
            m.lab.rf_autotrack = false;
            m.ui.note_mode_action(RailMode::Bench);
            m.push_log(format!(
                "{} gain \u{2192} {} dB",
                gain.primary_label(),
                new_gain
            ));
        }
        Err(e) => m.push_log(format!("Gain error: {}", e)),
    }
}

/// `↑` / `↓` on a SoapySDR device: move the whole chain by one reachable step,
/// and place the result across the stages ourselves.
///
/// The step is measured in **reachable totals**, not in dB. Adding a fixed 1 dB
/// and redistributing lands on the same achievable figure again, because the
/// stages have grids: on a HackRF's 8 dB LNA and 2 dB VGA, 21 floors back to 20
/// and the readout would never move.
///
/// Every stage is set, not just the ones that changed. A driver is entitled to
/// have moved an element behind our back, and the cost of being sure is a
/// handful of USB control transfers on a keypress.
fn step_distributed(ctx: &mut InputCtx<'_>, up: bool) {
    let Some(device) = ctx.device else { return };
    let stages = device.capabilities().gain.stages();
    if stages.is_empty() {
        return;
    }
    let from = metrics(ctx.state).radio.total_gain();
    let want = crate::hardware::gain::next_total(&stages, from, up);
    let (values, achieved) = crate::hardware::gain::distribute(&stages, want);

    // Device calls with no lock held, as everywhere else in this file.
    let mut failed = None;
    for (index, (value, spec)) in values.iter().zip(&stages).enumerate() {
        if let Err(e) = device.set_stage_gain(index, &spec.name, *value) {
            failed = Some(format!("{}: {e}", spec.name));
            break;
        }
    }

    let mut m = metrics(ctx.state);
    match failed {
        Some(why) => m.push_log(format!("Gain error: {why}")),
        None => {
            m.radio.gains = values.clone();
            m.lab.rf_autotrack = false;
            m.ui.note_mode_action(RailMode::Bench);
            // The achieved total, never the request: a request that is not
            // reachable would put a number on screen the radio is not set to.
            let split: Vec<String> = stages
                .iter()
                .zip(&values)
                .map(|(s, v)| format!("{} {:.0}", s.name, v))
                .collect();
            m.push_log(format!(
                "RF gain \u{2192} {achieved:.0} dB  ({})",
                split.join(" \u{00b7} ")
            ));
        }
    }
}

/// `[` / `]` - step the VGA. HackRF-only; on a single-tuner device these no-op.
pub(super) fn step_vga(ctx: &mut InputCtx<'_>, up: bool) {
    let Some(device) = ctx.device else { return };
    if !device.capabilities().gain.has_second_stage() {
        return;
    }
    let new_gain = {
        let m = metrics(ctx.state);
        if up {
            (m.radio.secondary_gain() + VGA_STEP_DB).min(VGA_MAX_DB)
        } else {
            m.radio.secondary_gain().saturating_sub(VGA_STEP_DB)
        }
    };

    let result = device.set_vga_gain(new_gain);
    let mut m = metrics(ctx.state);
    match result {
        Ok(()) => {
            m.radio.set_secondary_gain(new_gain);
            m.lab.rf_autotrack = false;
            m.ui.note_mode_action(RailMode::Bench);
            m.push_log(format!("VGA gain \u{2192} {} dB", new_gain));
        }
        Err(e) => m.push_log(format!("VGA gain error: {}", e)),
    }
}

/// `[A]` - the front-end boost.
///
/// `amp_enabled` doubles as the boost toggle for both families: HackRF's RF amp,
/// RTL-SDR's tuner AGC. The label and the device call both follow the gain model.
pub(super) fn toggle_boost(ctx: &mut InputCtx<'_>) {
    // Plenty of SoapySDR devices have neither an RF amp nor an automatic gain
    // mode. Flipping `amp_enabled` for one of those would light a lamp on the
    // rail, the micro gain view and the lab banner for a stage that is not in
    // the radio, and add 14 dB to a gain total that never had it. The key says
    // nothing happened instead of pretending something did.
    //
    // Asked of the shared capabilities rather than of the device handle, for two
    // reasons: it is the same value the panels decide on, so the key and the
    // display cannot disagree; and it is answerable before the handle is, which
    // is what lets it be tested.
    let gm = metrics(ctx.state).caps.gain.clone();
    if !gm.has_boost() {
        metrics(ctx.state).push_log("This device has no front end boost to toggle");
        return;
    }

    let Some(device) = ctx.device else { return };
    let new_state = !metrics(ctx.state).radio.amp_enabled;
    // Which trait call, from the capability rather than from the device family:
    // a single-gain device drives an automatic gain mode, a staged one drives a
    // discrete amplifier. The label comes from the same place.
    let result = if gm.is_single() {
        device.set_tuner_agc(new_state)
    } else {
        device.set_amp_enable(new_state)
    };
    let label = gm.boost_label();

    let mut m = metrics(ctx.state);
    match result {
        Ok(()) => {
            m.radio.amp_enabled = new_state;
            m.lab.rf_autotrack = false;
            m.ui.note_mode_action(RailMode::Bench);
            m.push_log(format!(
                "{} {}",
                label,
                if new_state { "ON" } else { "OFF" }
            ));
        }
        Err(e) => m.push_log(format!("{} error: {}", label, e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::hardware::native::{hackrf, rtlsdr};

    fn rtl(steps: &[u32]) -> GainModel {
        rtlsdr::gain_model(steps)
    }

    #[test]
    fn the_lna_walks_its_step_and_clamps_at_both_ends() {
        let g = hackrf::gain_model();
        assert_eq!(next_primary_gain(&g, 16, true), 24);
        assert_eq!(next_primary_gain(&g, 16, false), 8);
        assert_eq!(next_primary_gain(&g, 40, true), 40);
        assert_eq!(next_primary_gain(&g, 0, false), 0);
    }

    /// A tuner's gains are a table, not an arithmetic series, so stepping moves to
    /// the neighbouring *entry* - and from a value between entries it snaps to the
    /// nearest first.
    #[test]
    fn a_tuner_steps_to_the_neighbouring_table_entry() {
        let g = rtl(&[0, 9, 14, 27, 37, 49]);
        assert_eq!(next_primary_gain(&g, 14, true), 27);
        assert_eq!(next_primary_gain(&g, 14, false), 9);
        assert_eq!(next_primary_gain(&g, 49, true), 49, "clamps at the top");
        assert_eq!(next_primary_gain(&g, 0, false), 0, "clamps at the bottom");
        // 15 is nearest 14, so up lands on the entry after 14.
        assert_eq!(next_primary_gain(&g, 15, true), 27);
    }

    /// A tuner that reported no gain table cannot be stepped, and must not panic
    /// trying - `observer_caps()` is exactly that case.
    #[test]
    fn an_empty_gain_table_leaves_the_gain_alone() {
        let g = rtl(&[]);
        assert_eq!(next_primary_gain(&g, 27, true), 27);
        assert_eq!(next_primary_gain(&g, 27, false), 27);
    }

    #[test]
    fn the_label_names_the_stage_the_device_actually_has() {
        assert_eq!(hackrf::gain_model().primary_label(), "LNA");
        assert_eq!(rtl(&[0, 9]).primary_label(), "Tuner");
    }

    /// The step and the maximum have to agree, or the top of the range is
    /// unreachable by stepping.
    #[test]
    fn the_lna_range_is_reachable_in_whole_steps() {
        const { assert!(VGA_MAX_DB.is_multiple_of(VGA_STEP_DB)) };
        let g = hackrf::gain_model();
        let mut v = 0;
        for _ in 0..100 {
            v = next_primary_gain(&g, v, true);
        }
        assert_eq!(v, 40, "stepping up should reach the maximum exactly");
    }
}
