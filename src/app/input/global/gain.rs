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

/// HackRF's LNA moves in 8 dB steps across 0–40 dB.
const LNA_STEP_DB: u32 = 8;
const LNA_MAX_DB: u32 = 40;
/// HackRF's VGA moves in 2 dB steps across 0–62 dB.
const VGA_STEP_DB: u32 = 2;
const VGA_MAX_DB: u32 = 62;
/// A SoapySDR device reports a continuous overall range and no step, so one is
/// chosen here. Small on purpose: the range can be 0 to 116 dB on a HackRF and
/// 0 to 0 on a sound card, and a big step would be useless at one end of that.
const SOAPY_STEP_DB: u32 = 1;

/// Next value for the primary front-end gain when stepping up/down: HackRF's LNA
/// moves in 8 dB steps (0–40); RTL-SDR's single tuner gain walks its discrete
/// table to the neighbouring entry.
pub(super) fn next_primary_gain(gain: &GainModel, current: u32, up: bool) -> u32 {
    match gain {
        GainModel::HackRf => {
            if up {
                (current + LNA_STEP_DB).min(LNA_MAX_DB)
            } else {
                current.saturating_sub(LNA_STEP_DB)
            }
        }
        GainModel::RtlSingle { gain_steps_db, .. } => {
            if gain_steps_db.is_empty() {
                return current;
            }
            let idx = gain_steps_db
                .iter()
                .enumerate()
                .min_by_key(|(_, &g)| (g as i64 - current as i64).abs())
                .map(|(i, _)| i)
                .unwrap_or(0);
            let new_idx = if up {
                (idx + 1).min(gain_steps_db.len() - 1)
            } else {
                idx.saturating_sub(1)
            };
            gain_steps_db[new_idx]
        }
        // A continuous range, so a fixed step is the only sensible answer. 1 dB
        // is deliberately fine rather than fast: a Soapy device's range can be
        // anything from 0 to 116 dB, and there is no per-device step to follow.
        GainModel::Soapy { min_db, max_db, .. } => {
            let hi = (*max_db).max(*min_db);
            if up {
                (current + SOAPY_STEP_DB).clamp(*min_db, hi)
            } else {
                current.saturating_sub(SOAPY_STEP_DB).clamp(*min_db, hi)
            }
        }
    }
}

/// Label for the primary gain stage in log messages.
pub(super) fn primary_gain_label(gain: &GainModel) -> &'static str {
    match gain {
        GainModel::HackRf => "LNA",
        GainModel::RtlSingle { .. } => "Tuner",
        GainModel::Soapy { .. } => "RF",
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
    if matches!(device.capabilities().gain, GainModel::Soapy { .. }) {
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
                primary_gain_label(gain),
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
    if !matches!(device.capabilities().gain, GainModel::HackRf) {
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

    fn rtl(steps: &[u32]) -> GainModel {
        GainModel::RtlSingle {
            gain_steps_db: steps.to_vec(),
        }
    }

    #[test]
    fn the_lna_walks_its_step_and_clamps_at_both_ends() {
        let g = GainModel::HackRf;
        assert_eq!(next_primary_gain(&g, 16, true), 24);
        assert_eq!(next_primary_gain(&g, 16, false), 8);
        assert_eq!(next_primary_gain(&g, LNA_MAX_DB, true), LNA_MAX_DB);
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
        assert_eq!(primary_gain_label(&GainModel::HackRf), "LNA");
        assert_eq!(primary_gain_label(&rtl(&[0, 9])), "Tuner");
    }

    /// The step and the maximum have to agree, or the top of the range is
    /// unreachable by stepping.
    #[test]
    fn the_lna_range_is_reachable_in_whole_steps() {
        const { assert!(LNA_MAX_DB.is_multiple_of(LNA_STEP_DB)) };
        const { assert!(VGA_MAX_DB.is_multiple_of(VGA_STEP_DB)) };
        let g = GainModel::HackRf;
        let mut v = 0;
        for _ in 0..100 {
            v = next_primary_gain(&g, v, true);
        }
        assert_eq!(
            v, LNA_MAX_DB,
            "stepping up should reach the maximum exactly"
        );
    }
}
