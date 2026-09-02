// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The panels themselves, grouped by which screen they belong to.
//!
//! - [`core`]: the everyday views. Header, spectrum, waterfall, the Command Rail,
//!   the signal strip, the log and the footer.
//! - [`lab`]: the measurement benches behind `5` to `9`, plus the two thin
//!   instrument bars that wrap them.
//! - [`micro`]: the compact field views behind `0`.
//!
//! Every one of them implements [`crate::ui::panel::Panel`] and is registered in
//! `app::builder`. A panel that is not registered cannot be named in a preset.

pub mod core;
pub mod lab;
pub mod micro;
