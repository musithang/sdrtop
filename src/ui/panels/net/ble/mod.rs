// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! BLE-specific panels: what this device is advertising, and how good its
//! own transmitter is. Classic Bluetooth's panels live beside this one in
//! `net::bt`, not in here - the two protocols share nothing above
//! `signal::net::worker`, and the panel layer keeps the same split.

pub mod ble_packets;
pub mod ble_rf;
