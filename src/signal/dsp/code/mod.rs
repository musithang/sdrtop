// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Channel coding: whitening, error checking and error correction, shared
//! across protocols the way every other `dsp` module is: [`lfsr`] and
//! [`crc`], Bluetooth's de-whitening and CRC-24, and [`conv`], the
//! convolutional codes LE Coded and 802.11 both use and their Viterbi
//! decoder.

pub mod conv;
pub mod crc;
pub mod lfsr;
