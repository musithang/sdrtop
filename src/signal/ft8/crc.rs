// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The 14-bit CRC that makes the 77-bit payload a 91-bit message-plus-CRC
//! word (the QEX paper, section 3): polynomial 0x6757, initial value zero,
//! computed on the payload zero-extended from 77 to 82 bits.
//!
//! The division is the reference program `gen_crc14`'s, bit by bit: the 82
//! bits followed by 14 zero bits, divided by the 15-bit polynomial, the
//! remainder is the check. Nothing here is on a hot path (one call per
//! candidate that LDPC accepted), so it stays in the form that can be read
//! against the program line by line.

/// `x^14 + x^13 + x^10 + x^9 + x^8 + x^6 + x^4 + x^2 + x + 1`, the paper's
/// 0x6757; `gen_crc14` spells it out as the bits 1,1,0,0,1,1,1,0,1,0,1,0,1,1,1.
const POLY: u32 = 0x6757;

/// The check of a 77-bit payload, given as the low 77 bits of `payload` with
/// the first transmitted bit at bit 76.
pub fn crc14(payload: u128) -> u16 {
    // 77 payload bits, 5 zero bits to make 82, 14 zero bits to divide into:
    // 96 bits, the top one first.
    let dividend = (payload & ((1u128 << 77) - 1)) << 19;
    let mut r: u32 = 0;
    for i in (0..96).rev() {
        r = (r << 1) | ((dividend >> i) & 1) as u32;
        if r & (1 << 14) != 0 {
            r ^= POLY;
        }
    }
    (r & 0x3FFF) as u16
}

/// The 91-bit message-plus-CRC word: the payload followed by its check.
pub fn with_crc(payload: u128) -> u128 {
    ((payload & ((1u128 << 77) - 1)) << 14) | u128::from(crc14(payload))
}

/// Whether a 91-bit word's last 14 bits are the check of its first 77.
pub fn check(word91: u128) -> bool {
    let payload = word91 >> 14;
    (word91 & 0x3FFF) as u16 == crc14(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A second, independent division: the CRC as the remainder of the whole
    /// 96-bit polynomial, by long division on a big integer the other way
    /// round (aligning the polynomial under each set bit from the top).
    fn crc_long_division(payload: u128) -> u16 {
        let mut d = (payload & ((1u128 << 77) - 1)) << 19;
        for bit in (14..96).rev() {
            if d & (1u128 << bit) != 0 {
                d ^= u128::from(POLY) << (bit - 14);
            }
        }
        d as u16
    }

    fn bits(s: &str) -> u128 {
        s.bytes()
            .fold(0, |acc, b| (acc << 1) | u128::from(b - b'0'))
    }

    #[test]
    fn the_shift_register_agrees_with_long_division() {
        // The usage example `gen_crc14` prints, and a sweep of single bits.
        let example =
            bits("00000000000000000000000000100000010011011111110011011100100010100001010000001");
        assert_eq!(crc14(example), crc_long_division(example));
        for k in 0..77 {
            let p = 1u128 << k;
            assert_eq!(crc14(p), crc_long_division(p), "bit {k}");
        }
    }

    #[test]
    fn a_zero_payload_has_a_zero_check() {
        assert_eq!(crc14(0), 0);
    }

    #[test]
    fn the_check_is_the_remainder_so_the_whole_word_divides() {
        // Message, five zeros, then the check in place of the fourteen zeros:
        // the polynomial divides it exactly.
        let p =
            bits("10110011100011110000101010101111000011110000111100001111000011110000111100001");
        let mut d = (p << 19) | u128::from(crc14(p));
        for bit in (14..96).rev() {
            if d & (1u128 << bit) != 0 {
                d ^= u128::from(POLY) << (bit - 14);
            }
        }
        assert_eq!(d, 0);
    }

    #[test]
    fn a_word_checks_and_any_flipped_bit_fails() {
        let p =
            bits("01110101010101001010101111100000101010100101011110000111101010101010101010101");
        let w = with_crc(p);
        assert!(check(w));
        for k in 0..91 {
            assert!(!check(w ^ (1u128 << k)), "bit {k}");
        }
    }
}
