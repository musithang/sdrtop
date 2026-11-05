// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

#[derive(Clone, Default)]
pub struct ObserverState {
    pub active: bool,
    pub device: Option<String>,
    pub serial: Option<String>,
    pub usb: Option<String>,
    pub connected: Option<String>,
    pub owner: Option<String>,
    pub cmdline: Option<String>,
    /// The holder's CPU share, every core together, as our own is shown.
    pub owner_cpu_pct: Option<f32>,
    pub owner_ram_mb: Option<u64>,
    pub owner_uptime: Option<String>,
    /// For a radio held through a service, what can be said instead of a
    /// holder. See `hardware::discovery::Relay`.
    pub relay: Option<RelayAccount>,
}

/// A radio held through a service, as observer mode sees it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RelayAccount {
    pub through: &'static str,
    pub api: &'static str,
    /// The programs of this user with the API loaded, sdrtop aside, as
    /// `name (PID n)`; `None` where this system cannot tell.
    pub users: Option<Vec<String>>,
}
