// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Classic-Bluetooth-specific panels: B14/B15's own access-code hop scatter
//! and the piconet roster (net-ux-polish-plan 6.1). BLE's panels live beside this one in `net::ble`, not in
//! here - the two protocols share nothing above `signal::net::worker`, and
//! the panel layer keeps the same split.

pub mod bt_hops;
pub mod bt_piconets;
