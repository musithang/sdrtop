// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Channel coding: whitening and error checking, shared across protocols the
//! way every other `dsp` module is. `viterbi.rs` (Wi-Fi's convolutional
//! decoder) arrives with that arc; [`lfsr`] and [`crc`] arrive with B5,
//! Bluetooth's own de-whitening and CRC-24.

pub mod crc;
pub mod lfsr;
