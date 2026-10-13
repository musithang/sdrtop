// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! 802.11b: the DSSS and HR/DSSS PHY (IEEE 802.11-2024, clauses 15 and
//! 16), as far as its header: a frame found, its rate and length read, its
//! airtime exact.

pub mod chips;
pub mod header;
pub mod receive;
#[cfg(test)]
pub mod transmit;
