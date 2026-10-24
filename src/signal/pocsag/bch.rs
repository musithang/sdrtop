// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The POCSAG codeword's protection: BCH(31,21) and an even parity bit, as
//! ITU-R M.584-2 Annex 1 §1.4 defines them.
//!
//! A codeword is 32 bits sent most significant first, so the Recommendation's
//! bit No. 1 is bit 31 of the `u32` and its bit No. 32 is bit 0. Bits 1 to 21
//! are the information, 22 to 31 the check bits (the remainder of the
//! information, as the coefficients of x^30 to x^10, divided by the generator
//! `x^10 + x^9 + x^8 + x^6 + x^5 + x^3 + 1`), and bit 32 makes the parity of
//! all 32 even.
//!
//! **What a repair is.** The BCH code corrects any one or two wrong bits in
//! its 31; the parity bit then tells a two-bit repair from a three-bit error
//! it would have miscorrected. So: a clean syndrome with odd parity is a wrong
//! parity bit; a one-bit syndrome with odd parity is that bit and the parity
//! bit; a two-bit syndrome with odd parity is at least three errors, and is
//! refused rather than "repaired" into a different codeword. Every bit flipped
//! is kept, by position, because how many bits a page needed repairing is part
//! of what the page says about the link.

use std::sync::OnceLock;

/// The batch's synchronization codeword (Table 1).
pub const SYNC: u32 = 0x7CD2_15D8;

/// The idle codeword (Table 2): a valid address codeword never allocated to a
/// pager, sent wherever a batch has nothing to carry.
pub const IDLE: u32 = 0x7A89_C197;

/// `x^10 + x^9 + x^8 + x^6 + x^5 + x^3 + 1`.
const GENERATOR: u32 = 0b111_0110_1001;

/// The remainder of the 31-bit block `block` (bit 30 the coefficient of x^30)
/// divided by the generator: zero for a codeword.
fn remainder(block: u32) -> u32 {
    let mut r = block & 0x7FFF_FFFF;
    for bit in (10..31).rev() {
        if r & (1 << bit) != 0 {
            r ^= GENERATOR << (bit - 10);
        }
    }
    r & 0x3FF
}

/// The full codeword for 21 information bits, given in a codeword's own
/// place (bits 31 to 11 of the `u32`): check bits and parity added.
pub fn encode(info: u32) -> u32 {
    let info = info & 0xFFFF_F800;
    let block = info >> 1;
    let word = info | (remainder(block) << 1);
    word | (word.count_ones() & 1)
}

/// The Recommendation's bit No. (1 to 32) of `u32` bit `bit`.
pub fn bit_no(bit: u32) -> u8 {
    (32 - bit) as u8
}

/// A codeword after its check: the corrected word, and every bit that was
/// flipped to get there (as a mask over the `u32`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Checked {
    pub word: u32,
    pub flipped: u32,
}

impl Checked {
    /// How many bits the repair changed.
    pub fn repairs(&self) -> u32 {
        self.flipped.count_ones()
    }
}

/// Syndrome to the error it stands for, over the 31-bit block: every single
/// and double error. 31 + 465 patterns, all with distinct syndromes, since
/// the code's minimum distance is 5.
fn table() -> &'static [u32; 1024] {
    static TABLE: OnceLock<[u32; 1024]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut t = [0u32; 1024];
        for i in 0..31 {
            let e = 1u32 << i;
            t[remainder(e) as usize] = e;
            for j in 0..i {
                let e2 = e | (1 << j);
                t[remainder(e2) as usize] = e2;
            }
        }
        t
    })
}

/// Check and repair a received codeword, or refuse it.
///
/// `None` is a codeword with more errors than the code can repair. It is not
/// guessed at: the page it belongs to says a codeword was lost there.
pub fn check(received: u32) -> Option<Checked> {
    let block = received >> 1;
    let syndrome = remainder(block);
    let pattern = if syndrome == 0 {
        0
    } else {
        table()[syndrome as usize]
    };
    if syndrome != 0 && pattern == 0 {
        return None;
    }
    let mut flipped = pattern << 1;
    let mut word = received ^ flipped;
    if word.count_ones() & 1 == 1 {
        // The parity bit disagrees with what the BCH repair left. With fewer
        // than two repairs that is the parity bit itself; after two, it is a
        // third error the BCH code cannot see, and the repair is wrong.
        if pattern.count_ones() >= 2 {
            return None;
        }
        flipped |= 1;
        word ^= 1;
    }
    Some(Checked { word, flipped })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Recommendation calls the idle codeword "a valid address codeword":
    /// so it must pass this check untouched. A wrong generator or bit order
    /// here fails it.
    #[test]
    fn the_idle_word_is_a_codeword() {
        assert_eq!(
            check(IDLE),
            Some(Checked {
                word: IDLE,
                flipped: 0
            })
        );
        assert_eq!(encode(IDLE), IDLE);
    }

    /// Table 1 and Table 2, bit by bit from bit No. 1.
    #[test]
    fn sync_and_idle_are_the_tables() {
        let bits = |w: u32| -> String {
            (0..32)
                .rev()
                .map(|b| if w >> b & 1 == 1 { '1' } else { '0' })
                .collect()
        };
        assert_eq!(bits(SYNC), "01111100110100100001010111011000");
        assert_eq!(bits(IDLE), "01111010100010011100000110010111");
    }

    /// Any one or two wrong bits anywhere in the 32 are repaired, and every
    /// flipped bit is reported.
    #[test]
    fn one_and_two_bit_errors_are_repaired() {
        let word = encode(0x1234_5000 | 0x8000_0000);
        for i in 0..32 {
            let got = check(word ^ (1 << i)).expect("one error");
            assert_eq!(got.word, word, "bit {i}");
            assert_eq!(got.flipped, 1 << i);
            for j in 0..i {
                let e = (1 << i) | (1 << j);
                let got = check(word ^ e).unwrap_or_else(|| panic!("bits {i} {j}"));
                assert_eq!((got.word, got.flipped), (word, e), "bits {i} {j}");
            }
        }
    }

    /// Three wrong bits are never repaired into a different codeword: the
    /// parity bit or the syndrome refuses every one of them.
    #[test]
    fn three_bit_errors_are_refused_not_miscorrected() {
        let word = encode(0x5A5A_5800);
        let mut refused = 0;
        for i in 0..32 {
            for j in 0..i {
                for k in 0..j {
                    let got = check(word ^ (1 << i) ^ (1 << j) ^ (1 << k));
                    assert!(
                        got.is_none_or(|c| c.word == word),
                        "{i} {j} {k} miscorrected"
                    );
                    refused += usize::from(got.is_none());
                }
            }
        }
        assert_eq!(refused, 32 * 31 * 30 / 6);
    }

    #[test]
    fn bit_numbers_are_the_recommendations() {
        assert_eq!(bit_no(31), 1, "the flag bit");
        assert_eq!(bit_no(0), 32, "the parity bit");
    }
}
