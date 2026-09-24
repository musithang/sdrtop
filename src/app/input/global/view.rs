// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The keys that change what is on screen without touching the radio: leaving
//! focus, the overlays, the waterfall pause and the spectrum hold, and entering
//! a panel's own mode.

use std::sync::Arc;

use super::super::{metrics, InputCtx};

/// `[Esc]` is one rule applied repeatedly: up one level.
///
/// With a panel focused that means leaving focus. With nothing focused there was
/// nothing above the deck until now, and now there is the menu, so the new
/// meaning fills a hole rather than taking a job away: `Esc` on an unfocused
/// deck did nothing at all before this.
pub(super) fn leave_focus_or_open_menu(ctx: &mut InputCtx<'_>) {
    if ctx.engine.focused_panel_name().is_some() {
        leave_focus(ctx);
        return;
    }
    open_menu(ctx);
}

/// Open the menu with the cursor on the active preset.
///
/// The cursor starting there is what makes `Enter` resume: the most ordinary
/// thing a person does with this screen needs no key of its own. A preset the
/// menu hides, or one no longer in the config, starts the cursor at the top
/// rather than refusing to open.
pub(super) fn open_menu(ctx: &mut InputCtx<'_>) {
    let active = ctx.engine.active_preset().to_string();
    let (section, entry) = ctx.engine.menu().locate(&active).unwrap_or((0, 0));
    metrics(ctx.state).ui.menu = Some(crate::state::MenuState {
        section,
        entry,
        pane: crate::state::MenuPane::Views,
        scroll: 0,
    });
}

/// `[Esc]` - leave focus mode, and clear **everything** focus mode put on screen.
///
/// Missing one of these is how a cursor or a scroll offset survives into a panel
/// that has no way to clear it: the keys that would move it belong to a focus
/// handler that is no longer running.
pub(super) fn leave_focus(ctx: &mut InputCtx<'_>) {
    let mut m = metrics(ctx.state);
    end_focus(ctx.engine, &mut m);
}

/// End whatever focus there is, and put the panel back as it was before the
/// first key: a cursor nobody is steering any more should not stay on
/// screen. The one path every way out of a
/// focus takes: `Esc`, a letter that focuses another panel, a layout switch
/// that takes the panel off screen, the sweep's jump to the spectrum.
pub(in crate::app::input) fn end_focus(
    engine: &mut crate::ui::LayoutEngine,
    m: &mut crate::state::SdrMetrics,
) {
    let Some(panel) = engine.focused_panel_name().map(str::to_string) else {
        return;
    };
    engine.clear_focus();
    m.ui.focused_panel = None;
    m.ui.focused_panel_bindings = &[];
    m.ui.log_overlay = false;
    reset_positions(&panel, m);
}

/// Panels whose focus moves nothing that stays on screen: their keys set a
/// mode (a freeze, a marker, the demod channel, a reference level) or run a
/// measurement, and a mode is a setting the user chose, not a cursor left
/// behind. The structural test holds every focusable panel to either this
/// list or an arm of [`reset_positions`].
#[cfg(test)]
pub(in crate::app::input) const NO_POSITION: &[&str] = &[
    "iq_diagnostics",
    "rf_chain",
    "timing_vitals",
    "timing_diagnostics",
    "lab_banner",
    "signal_metrics",
    "signal_characterization",
    "fm_demod",
    "net_capability",
    // Its keys tune, cycle the lead card and recall; the gain stage it once
    // picked is a mode every section has now, not a Rail position.
    "command_rail",
];

/// What "as it was before the first key" means, panel by panel: every
/// **position** (a cursor, a selection, a scrubbed time, a scrolled history)
/// goes; every **mode** chosen on purpose (the BLE filter and hold, the PHY,
/// the census sort, the hop zoom, the waterfall pause) stays. Decided with
/// Viktor, 2026-09-24: a cursor nobody is using any more "olyan, mintha ott
/// ragadna".
///
/// The spectrum and waterfall cursors go whatever was focused, as they always
/// did: the spectrum's focus places the waterfall's cursor too.
pub(in crate::app::input) fn reset_positions(panel: &str, m: &mut crate::state::SdrMetrics) {
    m.spectrum.cursor_freq = None;
    m.waterfall.scroll_offset = 0;
    m.waterfall.cursor_freq = None;
    match panel {
        "sweep_panel" => m.sweep.cursor_frac = None,
        "net_occupancy" => m.net.band_cursor = Default::default(),
        "net_coexist" => m.net.band_scrub = None,
        // Kept for the carry into the BLE list before it is cleared: the
        // one selection that outlives its focus, because it is on its way
        // somewhere.
        "net_census" => {
            m.net.census.chosen = m.net.census.selection.selected;
            m.net.census.selection = Default::default();
        }
        "net_ble_packets" => m.net.ble_view.selection = Default::default(),
        "net_bt_piconets" => m.net.bt_view = Default::default(),
        // The zoom is a mode; where in time the window ends is a position.
        "net_bt_hops" => {
            m.net.bt_view = Default::default();
            m.net.hop_view.back_ms = 0;
        }
        _ => {}
    }
}

/// `[W]` - pause the waterfall in place.
pub(super) fn toggle_waterfall_pause(ctx: &mut InputCtx<'_>) {
    let mut m = metrics(ctx.state);
    m.waterfall.buffer.paused = !m.waterfall.buffer.paused;
    let word = if m.waterfall.buffer.paused {
        "paused"
    } else {
        "resumed"
    };
    m.push_log(format!("Waterfall {}", word));
}

/// `[H]` - freeze a ghost trace over the spectrum, or clear it.
///
/// The frame is cloned out under its own guard before the second one is taken:
/// `hold` keeps an `Arc` of the bins, so holding one guard while reading through
/// another would be the same lock twice.
pub(super) fn toggle_hold(ctx: &mut InputCtx<'_>) {
    let held = {
        let m = metrics(ctx.state);
        m.waterfall
            .last_fft
            .as_ref()
            .map(|fr| Arc::clone(&fr.bins_dbfs))
    };
    let mut m = metrics(ctx.state);
    if m.spectrum.hold.is_some() {
        m.spectrum.hold = None;
        m.push_log("Hold: off");
    } else if let Some(bins) = held {
        m.spectrum.hold = Some(bins);
        m.push_log("Hold: on \u{2014} ghost spectrum frozen");
    }
}

/// Any other letter: enter that panel's focus mode, if the key claims one and the
/// panel is actually on screen.
///
/// The visibility check is what stops a key focusing a panel the active preset
/// does not draw - the footer would then advertise bindings for something the
/// user cannot see.
pub(super) fn enter_focus(ctx: &mut InputCtx<'_>, key: char) {
    // The first panel claiming the letter that is on screen: a letter may be
    // shared by panels no one layout shows together (`app::FocusKeys`).
    let Some(panel) = ctx.focus_keys.get(&key).and_then(|panels| {
        panels
            .iter()
            .copied()
            .find(|p| ctx.engine.is_panel_visible(p))
    }) else {
        return;
    };
    // Moving focus straight to another panel ends the first one's properly.
    if ctx.engine.focused_panel_name() != Some(panel) {
        let mut m = metrics(ctx.state);
        end_focus(ctx.engine, &mut m);
    }
    ctx.engine.focus(panel);
    let bindings = ctx.engine.get_panel_bindings(panel);
    let mut m = metrics(ctx.state);
    m.ui.focused_panel = Some(panel.to_string());
    m.ui.focused_panel_bindings = bindings;
}
