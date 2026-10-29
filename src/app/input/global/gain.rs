// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The gain keys: `↑`/`↓` on the primary stage (or on the one stage `,` / `.`
//! picked), `[`/`]` on the second stage, `[A]` on the front-end boost, and
//! `,` / `.` to pick a stage, from any section.
//!
//! All four share one shape - read the current value, ask the device to change
//! it, and only write the state back if the device agreed. The device call
//! happens with **no lock held**, which is why each of these takes the guard
//! twice rather than holding it across the USB round trip.
//!
//! `↑` and `↓` were 26 identical lines apart from one `true`/`false`, and `[` and
//! `]` twenty more; they are one function each now, with the direction as an
//! argument.

#[cfg(test)]
use crate::hardware::GainModel;
use crate::hardware::{Boost, StageReading, StageSpec};
use crate::state::RailMode;

use super::super::{metrics, InputCtx};

/// How far a continuous stage moves per keypress.
///
/// A stage that reports no step of its own has to be given one. Small on
/// purpose: such a range can be 0 to 116 dB on a HackRF chain and 0 to 0 on a
/// sound card, and a big step would be useless at one end of that.
const CONTINUOUS_STEP_DB: u32 = 1;

/// The next reachable value on one stage, up or down.
///
/// **The whole stepping rule, once.** A stage with a table walks to the
/// neighbouring entry; one with a grid moves by its own step; a continuous one
/// moves by [`CONTINUOUS_STEP_DB`] because it has none of its own to follow.
/// That covers a HackRF's 8 dB LNA and 2 dB VGA, an RTL-SDR's irregular tuner
/// table and whatever a driver reported, without naming any of them.
///
/// **A table entry comes back exactly**, never rounded to a whole dB: the
/// Airspy R2's mixer has 15.90, 16.16, 16.17 and 16.24 dB a position apart,
/// and rounded they are one 16 the key can never step past.
fn next_on_stage(stage: &StageSpec, current: f64, up: bool) -> f64 {
    if !stage.table.is_empty() {
        let idx = stage.position_of(current).unwrap_or(0) as usize;
        let next = if up {
            (idx + 1).min(stage.table.len() - 1)
        } else {
            idx.saturating_sub(1)
        };
        return stage.table[next];
    }

    let current = current.max(0.0).round() as u32;
    let lo = stage.min_db.max(0.0).round() as u32;
    let hi = stage.max_db.max(stage.min_db).max(0.0).round() as u32;
    let step = if stage.step_db > 0.0 {
        (stage.step_db.round() as u32).max(1)
    } else {
        CONTINUOUS_STEP_DB
    };
    let next = if up {
        (current + step).clamp(lo, hi)
    } else {
        current.saturating_sub(step).clamp(lo, hi)
    };
    f64::from(next)
}

/// Before a key sets stages the tuner AGC is setting, the AGC goes off: as
/// libairspy's presets switch it off before they set anything, and as an
/// RTL-SDR's manual gain always has. Said in the log. `false` when the AGC
/// would not switch off, and the key should not go on.
fn release_agc(ctx: &mut InputCtx<'_>, touched: &[usize]) -> bool {
    let Some(device) = ctx.device else {
        return false;
    };
    let governed: Vec<String> = {
        let m = metrics(ctx.state);
        let stages = m.caps.gain.stages();
        touched
            .iter()
            .filter(|&&i| m.agc_sets(i))
            .filter_map(|&i| stages.get(i).map(|s| s.name.clone()))
            .collect()
    };
    if governed.is_empty() {
        return true;
    }
    let result = device.set_tuner_agc(false);
    let mut m = metrics(ctx.state);
    match result {
        Ok(()) => {
            m.radio.amp_enabled = false;
            m.push_log(format!("AGC off: {} set by hand", governed.join(" and ")));
            true
        }
        Err(e) => {
            m.push_log(format!("AGC error: {e}"));
            false
        }
    }
}

/// How a stage's setting reads in the log, with the radio at `hz`.
///
/// Its dB where the dB holds, with its position when the stage is a table,
/// so two measured values that print alike are still two settings; its
/// position alone where the dB was not measured.
fn said(stage: &StageSpec, db: f64, hz: u64) -> String {
    match stage.reading(db, hz) {
        StageReading::Position { at, of } => format!("step {at}/{of}"),
        StageReading::Auto => "AGC".to_string(),
        StageReading::Db(db) if !stage.table.is_empty() => {
            let at = stage.position_of(db).unwrap_or(0);
            let of = stage.positions().unwrap_or(1).saturating_sub(1);
            let text = format!("{db:.2}");
            let text = text.trim_end_matches('0').trim_end_matches('.');
            format!("{text} dB ({at}/{of})")
        }
        StageReading::Db(db) => format!("{db:.0} dB"),
    }
}

/// Next value for the primary front-end gain when stepping up/down.
///
/// A device whose front stage does not exist cannot be stepped and is left
/// alone: an RTL-SDR that reported no gain table is exactly that case.
/// [`step_primary`] takes the front stage itself, for its log line; this is
/// the same rule as the tests pin it.
#[cfg(test)]
fn next_primary_gain(gain: &GainModel, current: f64, up: bool) -> f64 {
    match gain.stages().first() {
        Some(front) => next_on_stage(front, current, up),
        None => current,
    }
}

/// Walk the stage selection one place, with "the whole chain" as a real stop.
///
/// `None` is not a missing value here, it is the default position of the knob:
/// the ring runs `chain, stage 0, stage 1, ... , chain` so a user can always get
/// back to the one-knob control by pressing on rather than by remembering `Esc`.
pub(super) fn next_stage(current: Option<usize>, count: usize, forward: bool) -> Option<usize> {
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
pub(super) fn step_selected_stage(ctx: &mut InputCtx<'_>, up: bool) {
    let Some(device) = ctx.device else { return };
    let stages = device.capabilities().gain.stages();
    let Some(index) = metrics(ctx.state).ui.gain_stage else {
        return;
    };
    let Some(spec) = stages.get(index) else {
        return;
    };
    if !release_agc(ctx, &[index]) {
        return;
    }

    let (current, hz) = {
        let m = metrics(ctx.state);
        (m.radio.stage_gain(index), m.radio.frequency)
    };
    // A table is walked entry by entry, as the stage keys walk it; a step of
    // 1 dB snapped onto a table skips entries closer together than that.
    let next = if !spec.table.is_empty() {
        next_on_stage(spec, current, up)
    } else {
        let step = if spec.step_db > 0.0 {
            spec.step_db
        } else {
            1.0
        };
        let target = if up { current + step } else { current - step };
        // Snap towards the direction of travel, so a value that started off
        // the grid still moves rather than snapping back onto where it already
        // was.
        let next = if up {
            spec.snap(target.max(current + step * 0.5))
        } else {
            spec.snap_down(target)
        };
        next.clamp(spec.min_db, spec.max_db)
    };

    let result = device.set_stage_gain(index, &spec.name, next);
    let mut m = metrics(ctx.state);
    match result {
        Ok(()) => {
            m.radio.set_stage_gain(index, next);
            m.lab.rf_autotrack = false;
            m.ui.note_mode_action(RailMode::Bench);
            m.push_log(format!("{} \u{2192} {}", spec.name, said(spec, next, hz)));
        }
        Err(e) => m.push_log(format!("Gain error: {e}")),
    }
}

/// `,` / `.`: pick the previous or next gain stage the device reported, with
/// "the whole chain" as a stop in the ring. A mode, not a cursor: it stays
/// picked across layouts and focus until the ring comes back round, and the
/// footer says which stage `↑` / `↓` are moving.
pub(super) fn select_stage(ctx: &mut InputCtx<'_>, forward: bool) {
    let has_curves = ctx
        .device
        .is_some_and(|d| !d.capabilities().gain.curves().is_empty());
    if has_curves {
        select_on_curve_ring(ctx, forward);
        return;
    }
    let count = ctx.device.map(|d| d.capabilities().gain.stages().len());
    let Some(count) = count.filter(|n| *n > 1) else {
        return;
    };
    let mut m = metrics(ctx.state);
    m.ui.gain_stage = next_stage(m.ui.gain_stage, count, forward);
    let name =
        m.ui.gain_stage
            .and_then(|i| m.caps.gain.stages().get(i).map(|s| s.name.clone()));
    let msg = match name {
        Some(n) => format!("Gain: {n} alone"),
        None => "Gain: the whole chain".to_string(),
    };
    m.push_log(msg);
}

/// `,` / `.` on a device with vendor curves: the ring is the curves, then
/// each stage alone, and round. A curve is the chain's own knob there, so the
/// ring has no separate "whole chain" stop; picking a stage takes the knob off
/// the curve, which is what the stages by hand means.
fn select_on_curve_ring(ctx: &mut InputCtx<'_>, forward: bool) {
    let mut m = metrics(ctx.state);
    let gm = m.caps.gain.clone();
    let curves = gm.curves().len();
    let stages = gm.stages();
    let ring = curves + stages.len();
    let at = match (m.ui.gain_stage, m.radio.gain_curve) {
        (Some(stage), _) => Some(curves + stage),
        (None, Some(curve)) => Some(curve),
        (None, None) => None,
    };
    let next = match (at, forward) {
        (None, true) => 0,
        (None, false) => ring - 1,
        (Some(i), true) => (i + 1) % ring,
        (Some(i), false) => (i + ring - 1) % ring,
    };
    let msg = if next < curves {
        m.radio.gain_curve = Some(next);
        m.ui.gain_stage = None;
        format!("Gain: {} curve", gm.curves()[next].name)
    } else {
        m.radio.gain_curve = None;
        m.ui.gain_stage = Some(next - curves);
        format!("Gain: {} alone", stages[next - curves].name)
    };
    m.push_log(msg);
}

/// `↑` / `↓` on a vendor curve: one step along it from where the chain is,
/// every stage set by position to that step's entries.
///
/// From the nearest step, so a chain moved off the curve by hand rejoins it
/// where it is rather than jumping back to where the curve last was.
fn step_curve(ctx: &mut InputCtx<'_>, curve: usize, up: bool) {
    let Some(device) = ctx.device else { return };
    let gm = &device.capabilities().gain;
    let Some(c) = gm.curves().get(curve) else {
        return;
    };
    let stages = gm.stages();
    let every: Vec<usize> = (0..stages.len()).collect();
    if !release_agc(ctx, &every) {
        return;
    }
    let (from, hz) = {
        let m = metrics(ctx.state);
        (c.nearest_step(&stages, &m.radio.gains), m.radio.frequency)
    };
    let step = if up {
        (from + 1).min(c.last_step())
    } else {
        from.saturating_sub(1)
    };
    let values = c.values(step, &stages);

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
            let split: Vec<String> = stages
                .iter()
                .zip(&values)
                .map(|(s, &v)| format!("{} {}", s.name, said(s, v, hz)))
                .collect();
            m.radio.gains = values;
            m.lab.rf_autotrack = false;
            m.ui.note_mode_action(RailMode::Bench);
            m.push_log(format!(
                "{} {step}/{} \u{2192} {}",
                c.name,
                c.last_step(),
                split.join(" \u{00b7} ")
            ));
        }
    }
}

/// `↑` / `↓` - step the primary front-end stage.
///
/// On a device that presents its stages separately, this moves the front one
/// and nothing else. A HackRF is that device: two stages, two keys.
///
/// On one whose knob is a single figure for a chain of several stages, sdrtop
/// spreads that figure itself rather than handing it to the driver. See
/// [`step_distributed`].
pub(super) fn step_primary(ctx: &mut InputCtx<'_>, up: bool) {
    let Some(device) = ctx.device else { return };
    // On a vendor curve the knob is the curve.
    let curve = metrics(ctx.state).radio.gain_curve;
    if let Some(curve) = curve.filter(|&c| c < device.capabilities().gain.curves().len()) {
        step_curve(ctx, curve, up);
        return;
    }
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
    let Some(front) = gain.stages().into_iter().next() else {
        return;
    };
    if !release_agc(ctx, &[0]) {
        return;
    }
    let (current, hz) = {
        let m = metrics(ctx.state);
        (m.radio.stage_gain(0), m.radio.frequency)
    };
    let new_gain = next_on_stage(&front, current, up);

    // By position, with the exact value: the one setter every backend has for
    // every stage.
    let result = device.set_stage_gain(0, &front.name, new_gain);
    let mut m = metrics(ctx.state);
    match result {
        Ok(()) => {
            m.radio.set_stage_gain(0, new_gain);
            // On a single-tuner device, setting a manual gain turns the tuner AGC
            // off in hardware - keep the UI's AGC flag in sync.
            if gain.is_single() {
                m.radio.amp_enabled = false;
            }
            m.lab.rf_autotrack = false;
            m.ui.note_mode_action(RailMode::Bench);
            m.push_log(format!(
                "{} gain \u{2192} {}",
                gain.primary_label(),
                said(&front, new_gain, hz)
            ));
        }
        Err(e) => m.push_log(format!("Gain error: {}", e)),
    }
}

/// `↑` / `↓` on a device whose knob moves a whole chain: step by one reachable
/// total, and place the result across the stages ourselves.
///
/// Handing a driver a total and letting it split is what this replaced. A
/// `SoapyHackRF` splits VGA-first, which is the arrangement with the worst
/// noise figure of the ones available, and it is not monotonic: turning the
/// knob up could drop the LNA from 32 dB to 19.
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
    let every: Vec<usize> = (0..stages.len()).collect();
    if !release_agc(ctx, &every) {
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

/// `[` / `]` - step the second stage. No-op on a device that does not give one
/// a key of its own, which today is everything but a HackRF and an Airspy R2.
///
/// The step comes from the stage, not from a datasheet written down here: the
/// 2 dB grid is the model's to state, the same way the front stage's 8 dB is.
pub(super) fn step_vga(ctx: &mut InputCtx<'_>, up: bool) {
    let Some(device) = ctx.device else { return };
    let gm = &device.capabilities().gain;
    if !gm.has_second_stage() {
        return;
    }
    let stages = gm.stages();
    let Some(second) = stages.get(1) else { return };
    if !release_agc(ctx, &[1]) {
        return;
    }
    let label = second.name.clone();
    let (new_gain, hz) = {
        let m = metrics(ctx.state);
        (
            next_on_stage(second, m.radio.stage_gain(1), up),
            m.radio.frequency,
        )
    };

    // By position, not through `set_vga_gain`: on an R2 the second stage is
    // the mixer, and a VGA setter it does not have answered `Ok` and moved
    // nothing.
    let result = device.set_stage_gain(1, &label, new_gain);
    let mut m = metrics(ctx.state);
    match result {
        Ok(()) => {
            m.radio.set_stage_gain(1, new_gain);
            m.lab.rf_autotrack = false;
            m.ui.note_mode_action(RailMode::Bench);
            m.push_log(format!(
                "{label} gain \u{2192} {}",
                said(second, new_gain, hz)
            ));
        }
        Err(e) => m.push_log(format!("{label} gain error: {}", e)),
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
    // **Which trait call, asked of the mechanism rather than of the shape.**
    // This used to ask `is_single()`, which happened to give the right answer
    // and meant the wrong thing: "one knob" is not the same question as "the
    // boost is an automatic gain mode". A device with one knob and a discrete
    // amplifier exists, and a HackRF reached through SoapySDR is one.
    //
    // Getting it backwards is silent, which is why it is worth spelling out.
    // `RtlDevice` does not implement `set_amp_enable` and `HackRfDevice` does
    // not implement `set_tuner_agc`; both fall back to the trait's `Ok(())`. A
    // misrouted key would report success and toggle nothing.
    let result = match gm.boost() {
        Some(Boost::GainMode) => device.set_tuner_agc(new_state),
        Some(Boost::Element(_)) => device.set_amp_enable(new_state),
        // Unreachable: `has_boost()` above returned early.
        None => Ok(()),
    };
    let label = gm.boost_label();

    let mut m = metrics(ctx.state);
    match result {
        Ok(()) => {
            m.radio.amp_enabled = new_state;
            m.lab.rf_autotrack = false;
            m.ui.note_mode_action(RailMode::Bench);
            m.push_log(boost_line(&gm, new_state));
        }
        Err(e) => m.push_log(format!("{} error: {}", label, e)),
    }
}

/// What the boost key logs. On digital stages the radio's AGCs are always on
/// and the switch only says who sets the stage they govern, so the line says
/// that rather than "ON" for a control that adds nothing.
fn boost_line(gm: &crate::hardware::GainModel, on: bool) -> String {
    let label = gm.boost_label();
    let governed = gm
        .stages()
        .iter()
        .enumerate()
        .find(|&(i, _)| gm.auto_governs(i))
        .map(|(_, s)| s.name.clone());
    match (gm.stages_are_digital(), governed) {
        (true, Some(stage)) if on => {
            format!("{label} ON: the radio sets {stage}; its AGCs are always on")
        }
        (true, Some(stage)) => format!("{label} OFF: {stage} by hand"),
        _ => format!("{label} {}", if on { "ON" } else { "OFF" }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::hardware::native::{hackrf, rtlsdr};

    /// On the HF+ the boost key says who sets ATT, the radio's AGCs being
    /// always on; elsewhere it says ON and OFF as it always has.
    #[test]
    fn the_boost_line_says_what_the_switch_does() {
        let hf = crate::hardware::native::airspyhf::gain_model();
        assert_eq!(
            boost_line(&hf, true),
            "AGC ON: the radio sets ATT; its AGCs are always on"
        );
        assert_eq!(boost_line(&hf, false), "AGC OFF: ATT by hand");
        assert_eq!(boost_line(&hackrf::gain_model(), true), "AMP ON");
        assert_eq!(boost_line(&rtl(&[0, 9]), false), "AGC OFF");
    }

    fn rtl(steps: &[u32]) -> GainModel {
        rtlsdr::gain_model(steps)
    }

    #[test]
    fn the_lna_walks_its_step_and_clamps_at_both_ends() {
        let g = hackrf::gain_model();
        assert_eq!(next_primary_gain(&g, 16.0, true), 24.0);
        assert_eq!(next_primary_gain(&g, 16.0, false), 8.0);
        assert_eq!(next_primary_gain(&g, 40.0, true), 40.0);
        assert_eq!(next_primary_gain(&g, 0.0, false), 0.0);
    }

    /// A tuner's gains are a table, not an arithmetic series, so stepping moves to
    /// the neighbouring *entry* - and from a value between entries it snaps to the
    /// nearest first.
    #[test]
    fn a_tuner_steps_to_the_neighbouring_table_entry() {
        let g = rtl(&[0, 9, 14, 27, 37, 49]);
        assert_eq!(next_primary_gain(&g, 14.0, true), 27.0);
        assert_eq!(next_primary_gain(&g, 14.0, false), 9.0);
        assert_eq!(next_primary_gain(&g, 49.0, true), 49.0, "clamps at the top");
        assert_eq!(
            next_primary_gain(&g, 0.0, false),
            0.0,
            "clamps at the bottom"
        );
        // 15 is nearest 14, so up lands on the entry after 14.
        assert_eq!(next_primary_gain(&g, 15.0, true), 27.0);
    }

    /// A tuner that reported no gain table cannot be stepped, and must not panic
    /// trying - `observer_caps()` is exactly that case.
    #[test]
    fn an_empty_gain_table_leaves_the_gain_alone() {
        let g = rtl(&[]);
        assert_eq!(next_primary_gain(&g, 27.0, true), 27.0);
        assert_eq!(next_primary_gain(&g, 27.0, false), 27.0);
    }

    /// Which trait call `[A]` makes follows the boost's mechanism, and getting
    /// it backwards is **silent**: `RtlDevice` does not implement
    /// `set_amp_enable` and `HackRfDevice` does not implement `set_tuner_agc`,
    /// so a misrouted key reports success and toggles nothing.
    ///
    /// This pins the fact the routing reads. Testing the call itself would need
    /// a fake device, and there is deliberately no such thing in this tree.
    #[test]
    fn each_backend_boost_is_the_mechanism_its_device_implements() {
        assert!(
            matches!(hackrf::gain_model().boost(), Some(Boost::Element(e)) if e.name == "AMP"),
            "the RF amp is a switch, driven with set_amp_enable"
        );
        assert!(
            matches!(rtl(&[0, 9, 28]).boost(), Some(Boost::GainMode)),
            "the tuner AGC is a gain mode, driven with set_tuner_agc"
        );
    }

    #[test]
    fn the_label_names_the_stage_the_device_actually_has() {
        assert_eq!(hackrf::gain_model().primary_label(), "LNA");
        assert_eq!(rtl(&[0, 9]).primary_label(), "Tuner");
    }

    /// The step and the maximum have to agree, or the top of the range is
    /// unreachable by stepping. Asked of the model rather than of a constant
    /// here, because the model is where the grid now lives.
    #[test]
    fn the_lna_range_is_reachable_in_whole_steps() {
        let g = hackrf::gain_model();
        for stage in g.stages() {
            assert!(
                (stage.max_db / stage.step_db).fract() < f64::EPSILON,
                "{} spans {} in steps of {}, which does not divide",
                stage.name,
                stage.max_db,
                stage.step_db
            );
        }
        let mut v = 0.0;
        for _ in 0..100 {
            v = next_primary_gain(&g, v, true);
        }
        assert_eq!(v, 40.0, "stepping up should reach the maximum exactly");
    }

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
