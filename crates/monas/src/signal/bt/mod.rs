// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Classic Bluetooth (BR/EDR), the sibling of `signal::ble` and independent
//! of it: the protocol modules do not know each other.
//!
//! **Hopping is the whole difficulty here, and nothing here follows it.**
//! 79 channels, 1600 hops a second, a sequence that depends on the master's
//! own clock and address - none of which a passive
//! receiver knows in advance. What *can* be found without joining a
//! piconet is [`access_code`]: every classic packet's own access code is
//! derived from the master's LAP by a public, fixed construction, so
//! correlating a live capture for *any* valid access code finds packets and
//! yields the LAP for free, with no piconet membership required first.
//!
//! Split the same way `signal::ble` is - detect, sync, decode, measure:
//!
//! - [`access_code`]: the construction, and the exact-match search;
//! - [`channel`]: which of the 79 channels a capture can see at all;
//! - [`detect`]: the same access-code check, made incremental for a live
//!   bit stream with no end;
//! - [`receive`]: the per-channel front end that produces that stream, and
//!   captures each header and a raw payload region after it;
//! - [`header`]: FEC(1/3), dewhitening, the HEC/UAP relationship, and the
//!   elapsed-clock inference that narrows a LAP's UAP to two candidates, a
//!   measured floor rather than one;
//! - [`payload`]: a DH1/DH3/DH5 payload's CRC-16, which breaks that
//!   two-candidate tie when `signal::net::worker` calls
//!   [`payload::break_uap_tie`] for a LAP whose UAP has not resolved;
//! - [`lmp`]: the link manager's messages in a passing payload's body, by
//!   the Core's names;
//! - [`piconet`] and [`slots`]: the roster, and slot timing.

pub mod access_code;
pub mod channel;
pub mod detect;
pub mod header;
pub mod lmp;
pub mod payload;
pub mod piconet;
pub mod receive;
pub mod slots;
