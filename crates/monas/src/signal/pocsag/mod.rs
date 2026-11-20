// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! POCSAG in the engine: the decoder (`monas_pocsag`, every part of it named
//! here as it always was), the worker that runs it on the radio's stream and
//! publishes what it heard, and the section of the screen whose being shown
//! turns that feed on.

pub use monas_pocsag::*;

pub mod worker;

/// The menu section the pager views are filed under: the one whose being on
/// screen turns the pager feed on.
pub const SECTION: &str = "pocsag";
