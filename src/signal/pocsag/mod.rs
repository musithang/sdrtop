// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! POCSAG, the radio-paging code of ITU-R M.584-2 Annex 1, received with no
//! library: our own codeword check, our own page assembly, our own
//! demodulator.
//!
//! Split the way a page is built, bottom up, each part a pure function of its
//! input and tested with no radio:
//!
//! - [`bch`]: one codeword, checked and repaired, or refused.
//! - [`page`]: batches to pages: the address from the codeword and its frame,
//!   the message across codewords and batches, what was lost kept as lost.
//!
//! **A page is not testimony on its own** (POLICY.md, rule 3). Every page
//! keeps the codewords it was read from and what each needed repairing, so
//! the screen can say what the link took, not only what it carried.

pub mod bch;
pub mod page;
