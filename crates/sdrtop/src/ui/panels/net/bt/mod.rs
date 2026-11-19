// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Classic-Bluetooth-specific panels: the access-code hops, the piconet
//! roster, the packets and the bench. BLE's panels live beside this one in
//! `net::ble`, not in here - the two protocols share nothing above `signal::net::worker`, and
//! the panel layer keeps the same split.

pub mod bt_bench;
pub mod bt_hops;
pub mod bt_packets;
pub mod bt_piconets;
mod plot;
mod sections;
