// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Bluetooth Low Energy: one of the two protocol modules under the Bluetooth
//! arc (`bluetooth-bench-design.md`; the checkpoint plan is
//! `bluetooth-bench-plan.md`).
//!
//! Split the way `net-foundation-design.md` section 10 requires of every
//! protocol module: detect, sync, decode, measure, in that order, each in its
//! own file, with none of it knowing classic Bluetooth (`signal::bt`, not yet
//! built) or Wi-Fi exist. [`channel`] comes before any of those stages: it is
//! arithmetic the whole arc needs (which frequency a channel index names), not
//! a step in the receive chain.

pub mod channel;
#[cfg(test)]
pub mod testkit;
