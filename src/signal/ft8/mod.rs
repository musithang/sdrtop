// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! FT8, as S. Franke, B. Somerville and J. Taylor define it in "The FT4 and
//! FT8 Communication Protocols" (QEX, July/August 2020), received with no
//! library: our own message unpacking, our own error correction, our own
//! demodulator.
//!
//! The paper's Appendix A completes its definitions with short public-domain
//! reference programs (`std_call_to_c28`, `grid4_to_g15`, `hashcodes`,
//! `gen_crc14` and the rest, its reference 14). Where a constant here comes
//! from one of them, the comment names the program; where it comes from the
//! paper, the table.
//!
//! Split the way a transmission is built, bottom up, each part a pure function
//! of its input and tested with no radio:
//!
//! - [`fields`]: one bit field (a call, a locator, a report, free text) to
//!   what it says, and the call sign hashes.
//! - [`hash`]: the calls heard in full, so a hashed one can be named.
//! - [`message`]: the 77-bit payload to a message of one of the paper's types.
//! - [`crc`]: the 14-bit check that turns 77 bits into 91.
//! - [`ldpc`]: the (174,91) code that protects those 91, and our decoder.
//!
//! **A field that does not decode refuses the message.** A payload whose
//! check passed but whose bits fall in a range the paper leaves unassigned is
//! not shown as a best guess (POLICY.md, rule 2); it is counted as malformed.

pub mod crc;
pub mod fields;
pub mod hash;
pub mod ldpc;
pub mod message;
