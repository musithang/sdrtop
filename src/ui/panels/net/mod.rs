// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The NET section's panels: the 2.4 GHz band, and what is on it.
//!
//! The section only exists on a radio that can reach the band and run at least
//! the cheapest mode; `signal::net::gate` decides that once at startup and the
//! presets naming these panels are dropped when it refuses. So nothing in here
//! has to check whether it should be on screen. If it is drawing, it may.
//!
//! Split by protocol, not left flat: [`ble`] is BLE-only, [`bt`] is classic
//! Bluetooth-only, and [`shared`] is protocol-agnostic and band-wide. This
//! mirrors `signal::ble` / `signal::bt` below it, which already made this
//! split - the panel layer had drifted out of step with it (one BLE panel
//! was named `bt_rf` until `dev_docs/net-ux-polish-plan.md`'s Tier 0 fixed
//! it), and the subdirectories exist so that drift cannot happen unnoticed
//! again: a panel's own path now says which group it belongs to.

pub mod ble;
pub mod bt;
pub mod shared;
