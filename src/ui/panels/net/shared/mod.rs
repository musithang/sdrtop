// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Protocol-agnostic, band-wide panels: what this radio can reach, who is
//! on the band, how it is stepping on itself, and what the receiver missed.
//! None of these belong to BLE or to classic Bluetooth exclusively, which is
//! why they sit apart from both in their own module rather than in either.

pub mod capability;
pub mod census;
pub mod coexist;
pub mod decode_health;
pub mod occupancy;
