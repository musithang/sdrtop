//! The number keys: `[1]`–`[9]` pick a layout, `[0]` cycles the micro family.
//!
//! Presets are data and the keys are code, so a key can always end up naming a
//! layout that does not exist - a renamed built-in, a deleted user file, a slot
//! whose preset was removed from the config. Every one of them therefore goes
//! through [`try_set_preset`], which either switches or says why it did not.

use std::sync::{Arc, Mutex};

use crate::state::SdrMetrics;
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

/// `[P]`: the next layout in this section, wrapping at its end.
///
/// This used to walk every preset in the config, alphabetically, which made it
/// the one key that still crossed a section boundary in a design whose whole
/// premise is that keys do not. It also meant `[P]` from a lab bench could land
/// you on a micro field view, which is not a neighbour of anything.
pub(super) fn cycle_in_scope(ctx: &mut super::super::InputCtx<'_>) {
    let Some(next) = ctx.engine.next_in_scope() else {
        return;
    };
    try_set_preset(ctx.engine, ctx.state, &next);
}
