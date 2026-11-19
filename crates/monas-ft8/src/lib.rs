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
//! - [`modulate`]: the codeword as 79 tones and the tones as a waveform.
//! - [`search`]: where in a cycle transmissions start, and on what tone.
//! - [`demod`]: a candidate to soft bits; a decode to what it measured.
//! - [`floor`]: the noise under the band in one cycle.
//! - [`cycle`]: one 15 s cycle to its decodes, subtracting and searching again.
//! - [`trace`]: what one decoded transmission looked like on the way in.
//! - [`bands`]: the conventional FT8 dial frequencies.
//! - [`channel`]: the radio's stream to FT8's 12 000 samples a second.
//! - [`clock`]: when each sample was taken, in UTC.
//!
//! The threads that run it all on the radio's stream and publish what they
//! heard are the engine's, beside its state; nothing here knows they exist.
//!
//! **A field that does not decode refuses the message.** A payload whose
//! check passed but whose bits fall in a range the paper leaves unassigned is
//! not shown as a best guess (POLICY.md, rule 2); it is counted as malformed.

// The documentation is its developers', so a public item's comment may
// point at the private one that explains it. rustdoc still resolves every
// such link, and a broken one fails the doc build as it always did.
#![allow(rustdoc::private_intra_doc_links)]

pub mod bands;
pub mod channel;
pub mod clock;
#[cfg(test)]
mod compare;
#[cfg(any(test, feature = "testing"))]
pub mod conformance;
pub mod crc;
pub mod cycle;
pub mod demod;
pub mod fields;
pub mod floor;
pub mod hash;
pub mod ldpc;
pub mod message;
pub mod modulate;
pub mod search;
pub mod trace;
