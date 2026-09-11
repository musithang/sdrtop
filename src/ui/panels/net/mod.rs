// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The NET section's panels: the 2.4 GHz band, and what is on it.
//!
//! The section only exists on a radio that can reach the band and run at least
//! the cheapest mode; `signal::net::gate` decides that once at startup and the
//! presets naming these panels are dropped when it refuses. So nothing in here
//! has to check whether it should be on screen. If it is drawing, it may.

pub mod ble_packets;
pub mod capability;
pub mod census;
pub mod coexist;
pub mod decode_health;
pub mod occupancy;
