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
//! a step in the receive chain. [`gfsk`] sits beside it for the same reason:
//! turning bits into the waveform LE 1M transmits is not itself a receive
//! stage, but [`detect`] needs it to build the reference it correlates
//! against, since the advertising access address is known in advance.

pub mod channel;
pub mod detect;
pub mod gfsk;
pub mod pdu;
pub mod receive;
pub mod sync;
