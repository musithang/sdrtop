//! The number keys: `[1]`–`[9]` pick a layout, `[0]` cycles the micro family.
//!
//! Presets are data and the keys are code, so a key can always end up naming a
//! layout that does not exist - a renamed built-in, a deleted user file, a slot
//! wired before its preset was written. Every one of them therefore goes through
//! [`try_set_preset`], which either switches or says why it did not.

use std::sync::{Arc, Mutex};

use crate::state::{MicroView, SdrMetrics};
use crate::ui;

use super::super::{metrics, KeyAction};

/// Switch to `name` if the preset is defined, otherwise log that it is not yet
/// available. This keeps the number-key framework in place before the presets
/// themselves exist, so each one activates the moment it is added to the layout
/// config.
///
/// **All nine keys go through this.** `[1]`–`[4]` used to call `set_preset`
/// directly and then log `"Preset: spectrum"` unconditionally - but `set_preset`
/// silently declines a name it does not know, so with that preset missing the key
/// logged a switch that had not happened.
pub(in crate::app::input) fn try_set_preset(
    engine: &mut ui::LayoutEngine,
    state: &Arc<Mutex<SdrMetrics>>,
    name: &str,
) -> KeyAction {
    let mut m = metrics(state);
    if engine.has_preset(name) {
        engine.set_preset(name);
        m.push_log(format!("Preset: {}", name));
    } else {
        m.push_log(format!("Preset '{}' not yet available", name));
    }
    KeyAction::Continue
}

/// The `[0]` micro-ecosystem cycle. Entering from a non-micro preset lands on
/// `micro_main`; pressing `[0]` again while already in a micro preset advances
/// to the next view. A target whose preset is not yet defined is logged and
/// skipped, so the cycle never strands the user on a blank view while the micro
/// presets are still being built out.
pub(super) fn cycle_micro(engine: &mut ui::LayoutEngine, state: &Arc<Mutex<SdrMetrics>>) {
    // The sweep step is part of the cycle: entering micro_sweep starts a scan.
    const SWEEP_ACTIVE: bool = true;
    // Ask the engine, which owns the authoritative value, rather than the
    // per-frame mirror in `UiState`: this runs on a key, between draws.
    let target = match MicroView::from_preset(engine.active_preset()) {
        Some(current) => current.next(SWEEP_ACTIVE),
        None => MicroView::Main,
    };
    let mut m = metrics(state);
    if engine.has_preset(target.preset_name()) {
        engine.set_preset(target.preset_name());
        m.push_log(format!(
            "Micro: {} ({}/{})",
            target.label(),
            target.position(),
            MicroView::total(SWEEP_ACTIVE)
        ));
    } else {
        m.push_log(format!(
            "Preset '{}' not yet available",
            target.preset_name()
        ));
    }
}

#[cfg(test)]
mod tests {

    /// The number keys and the layouts they select, read out of the dispatch.
    ///
    /// The match arms are the list - one line each, naming the key and the
    /// preset together. Read as source text because nothing in the type system
    /// connects a `KeyCode` arm to a preset name, the same way the focus-key and
    /// built-in-preset checks work.
    fn number_key_presets() -> Vec<(char, String)> {
        let src = include_str!("mod.rs");
        regex_pairs(src)
    }

    /// `KeyCode::Char('N') => … try_set_preset(…, "name")`, allowing for the
    /// line breaks `cargo fmt` introduces on the longer arms.
    fn regex_pairs(src: &str) -> Vec<(char, String)> {
        let mut out = Vec::new();
        let mut rest = src;
        while let Some(i) = rest.find("KeyCode::Char('") {
            rest = &rest[i + "KeyCode::Char('".len()..];
            let Some(key) = rest.chars().next() else {
                break;
            };
            // Look only as far as the next arm.
            let arm_end = rest.find("KeyCode::").unwrap_or(rest.len());
            let arm = &rest[..arm_end];
            if let Some(j) = arm.find("try_set_preset(") {
                if let Some(q0) = arm[j..].find('"') {
                    let after = &arm[j + q0 + 1..];
                    if let Some(q1) = after.find('"') {
                        out.push((key, after[..q1].to_string()));
                    }
                }
            }
        }
        out
    }

    /// Nine distinct keys naming nine distinct presets. A duplicate on either
    /// side would make one key unreachable or one layout unreachable.
    #[test]
    fn the_number_keys_are_distinct_and_so_are_their_presets() {
        let pairs = number_key_presets();
        assert_eq!(pairs.len(), 9, "expected nine layout keys, got {pairs:?}");
        let mut keys: Vec<char> = pairs.iter().map(|(k, _)| *k).collect();
        let mut names: Vec<&str> = pairs.iter().map(|(_, n)| n.as_str()).collect();
        let (nk, nn) = (keys.len(), names.len());
        keys.sort_unstable();
        keys.dedup();
        names.sort_unstable();
        names.dedup();
        assert_eq!(keys.len(), nk, "a key appears twice");
        assert_eq!(names.len(), nn, "a preset appears twice");
    }

    /// Every preset a number key names has to exist, or the key is dead on
    /// arrival. The presets are TOML and the dispatch is Rust, so nothing but a
    /// check connects them.
    #[test]
    fn every_number_key_names_a_preset_that_exists() {
        let cfg = crate::config::LayoutConfig::default_config();
        for (key, name) in number_key_presets() {
            assert!(
                cfg.presets.contains_key(&name),
                "[{key}] selects '{name}', which is not a built-in preset"
            );
        }
    }

    /// `[0]` is the micro cycle, so it must not also be a layout slot.
    #[test]
    fn zero_is_the_micro_cycle_not_a_layout_slot() {
        assert!(number_key_presets().iter().all(|(k, _)| *k != '0'));
    }
}
