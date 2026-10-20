// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The host sdrtop runs on: the one place that knows which system this is.
//!
//! Code that differs by system branches on [`Host`], and keeps `cfg(target_os)`
//! to the few places where the system call itself differs. A [`Host`] is a
//! plain value, so the macOS side of a decision is tested on Linux too.

/// The host's family.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Host {
    Linux,
    MacOs,
}

impl Host {
    /// The host this build runs on.
    pub fn current() -> Host {
        if cfg!(target_os = "macos") {
            Host::MacOs
        } else {
            Host::Linux
        }
    }
}
