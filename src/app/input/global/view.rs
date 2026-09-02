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
    if ctx.engine.focused_panel_name().is_none() {
        return;
    }
    ctx.engine.clear_focus();
    let mut m = metrics(ctx.state);
    m.ui.focused_panel = None;
    m.ui.focused_panel_bindings = &[];
    m.ui.log_overlay = false;
    m.spectrum.cursor_freq = None;
    m.waterfall.scroll_offset = 0;
    m.waterfall.cursor_freq = None;
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
    let Some(&panel) = ctx.focus_keys.get(&key) else {
        return;
    };
    if !ctx.engine.is_panel_visible(panel) {
        return;
    }
    ctx.engine.focus(panel);
    let bindings = ctx.engine.get_panel_bindings(panel);
    let mut m = metrics(ctx.state);
    m.ui.focused_panel = Some(panel.to_string());
    m.ui.focused_panel_bindings = bindings;
}
