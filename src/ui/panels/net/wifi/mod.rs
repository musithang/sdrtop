// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Wi-Fi's panels. Beside `net::ble` and `net::bt`, never inside either: the
//! Wi-Fi bench is a menu of its own and shares no panel with Bluetooth's.

pub mod standin;
pub mod survey;
