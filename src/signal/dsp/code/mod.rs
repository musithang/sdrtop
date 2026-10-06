// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Channel coding: whitening and error checking, shared across protocols the
//! way every other `dsp` module is: [`lfsr`] and [`crc`], Bluetooth's
//! de-whitening and CRC-24. LE Coded's convolutional code is decoded in
//! `signal::ble::coded`.

pub mod crc;
pub mod lfsr;
