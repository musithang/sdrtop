// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Classic Bluetooth (BR/EDR): the other protocol module under the
//! Bluetooth arc (`bluetooth-bench-design.md`; the checkpoint plan is
//! `bluetooth-bench-plan.md`), sibling to `signal::ble` and independent of
//! it - `net-foundation-design.md` section 10's own rule 2, "the protocol
//! modules do not know each other."
//!
//! **Hopping is the whole difficulty here, and B14 does not touch it.**
//! Design section 1.4: 79 channels, 1600 hops a second, a sequence that
//! depends on the master's own clock and address - none of which a passive
//! receiver knows in advance. What *can* be found without joining a
//! piconet is [`access_code`]: every classic packet's own access code is
//! derived from the master's LAP by a public, fixed construction, so
//! correlating a live capture for *any* valid access code finds packets and
//! yields the LAP for free, with no piconet membership required first.
//!
//! Split the same way `signal::ble` is - detect, sync, decode, measure -
//! as each stage arrives; B14 landed [`access_code`] alone. B15 adds
//! [`channel`] (which of the 79 channels a capture can see at all),
//! [`detect`] (the same access-code check, made incremental for a live bit
//! stream with no end) and [`receive`] (the per-channel front end that
//! actually produces one), and wires classic Bluetooth into
//! `signal::net::worker` for the first time - `net_bt_hops`, not a payload
//! decode, which is a later stage still.

pub mod access_code;
pub mod channel;
pub mod detect;
pub mod header;
pub mod receive;
