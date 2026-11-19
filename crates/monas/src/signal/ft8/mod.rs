// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! FT8 in the engine: the decoder (`monas_ft8`, every part of it named here
//! as it always was, so `signal::ft8::cycle::decode` still resolves), the
//! worker that runs it on the radio's stream and publishes what it heard,
//! and the sections of the screen whose being shown turns that feed on.

pub use monas_ft8::*;

pub mod worker;

/// The menu section the FT8 view is filed under, in the Radio tile.
pub const SECTION: &str = "ft8";

/// The FT8 Lab's section, in the Lab tile: the same decoder measured.
pub const LAB_SECTION: &str = "ft8_lab";

/// Both FT8 sections. Either being on screen turns the FT8 feed on, so the
/// one worker, its stations and its selection carry across the two: going
/// from World to the Lab and back loses no cycle.
pub const SECTIONS: [&str; 2] = [SECTION, LAB_SECTION];
