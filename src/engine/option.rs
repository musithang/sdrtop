// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! A radio's own options (a bias tee, a filter, a mode the driver offers):
//! setting one, and what follows from it. The terminal's menu and the
//! engine's command go through these two, so there is one way an option is
//! set.

use std::sync::{Arc, Mutex};

use crate::hardware::{DeviceOption, SdrDevice};
use crate::state::SdrMetrics;

/// Set an option with `set`, then read every option back with `refresh`,
/// which only runs when the radio took it: a refused choice leaves the
/// options as they were. Called with no lock held, both being device calls.
pub fn set(
    id: &str,
    choice: &str,
    set: impl FnOnce(&str, &str) -> anyhow::Result<()>,
    refresh: impl FnOnce() -> Vec<DeviceOption>,
) -> anyhow::Result<Vec<DeviceOption>> {
    set(id, choice)?;
    Ok(refresh())
}

/// Set the frequency again after an option that can move the tuning, and
/// keep the radio's word for where it landed. The frequency is read when the
/// option has been applied, not when it was asked for, so a retune made while
/// the option was being set is the one the radio gets. `None` for an option
/// that does not move the tuning.
pub fn retune_after(
    device: &dyn SdrDevice,
    id: &str,
    state: &Arc<Mutex<SdrMetrics>>,
) -> Option<Result<u64, String>> {
    if !device.option_moves_tuning(id) {
        return None;
    }
    let hz = state
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .radio
        .frequency;
    // Device call with no lock held, as everywhere else.
    Some(
        device
            .set_frequency(hz)
            .map_err(|error| format!("{error:#}")),
    )
}
