// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! CRC-24, the integrity check every Bluetooth Low Energy packet ends with.
//!
//! Source: Bluetooth Core Specification, Vol 6, Part B, section 3.1.1 -
//! polynomial `0x00065B`, reflected input and output, initial value
//! `0x555555` for advertising channel PDUs and test packets. (A data channel
//! connection's own CRC initial value is carried in `CONNECT_IND` and is a
//! later step's concern; nothing here assumes the advertising value where it
//! should not.)
//!
//! **Verified against an independent, authoritative catalogue, not the
//! specification restated from memory.** The CRC RevEng catalogue
//! (<https://reveng.sourceforge.io/crc-catalogue/17plus.htm>) lists this exact
//! algorithm as `CRC-24/BLE`, names the same specification section as its
//! own source, and - the part worth more than the citation - publishes a
//! check value: the CRC of the ASCII bytes `"123456789"` is `0xC25A56`.
//! `matches_the_reveng_check_value` computes that value with the algorithm
//! below and asserts the two agree. An independent party's own computed
//! answer, not a formula trusted on citation alone, is the strongest
//! verification available without a licensed copy of the specification
//! itself.
//!
//! Reflected input and output means BLE's own bit order - least-significant-
//! bit-first, the same convention `signal::ble::detect::access_address_bits`
//! already follows - is handled by reflecting the polynomial and the initial
//! value once at the top, then running an ordinary right-shifting register
//! with each byte's bits taken in the order they already are. This is the
//! standard technique for a reflected CRC (the same one that makes CRC-32's
//! well-known `0xEDB88320` the bit-reversal of its own polynomial
//! `0x04C11DB7`), not something specific to Bluetooth.

/// No consumer yet outside this module's own tests: nothing decodes a full
/// PDU to check against a CRC until B6 puts a real packet on screen. Applies
/// to every item below.
const POLY: u32 = 0x00065B;
const INIT: u32 = 0x555555;
const MASK: u32 = 0x00FF_FFFF;

/// Reverse the low 24 bits of `x`.
fn reflect24(mut x: u32) -> u32 {
    let mut r = 0u32;
    for _ in 0..24 {
        r = (r << 1) | (x & 1);
        x >>= 1;
    }
    r
}

/// CRC-24/BLE of `data`: reflected input and output, the advertising-channel
/// and test-packet initial value.
pub fn crc24_ble(data: &[u8]) -> u32 {
    let rev_poly = reflect24(POLY);
    let mut reg = reflect24(INIT);
    for &byte in data {
        reg ^= byte as u32;
        for _ in 0..8 {
            if reg & 1 != 0 {
                reg = (reg >> 1) ^ rev_poly;
            } else {
                reg >>= 1;
            }
        }
    }
    reg & MASK
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The catalogue's own check value. If this passes, the polynomial, the
    /// initial value and the reflection are all right at once - a wrong
    /// value in any of the three would move this number.
    #[test]
    fn matches_the_reveng_check_value() {
        assert_eq!(crc24_ble(b"123456789"), 0xC2_5A56);
    }

    /// The result never exceeds 24 bits, on an input long enough that a
    /// missing mask would show up as a wider number.
    #[test]
    fn the_result_stays_within_twenty_four_bits() {
        let data = [0xFFu8; 64];
        assert!(crc24_ble(&data) <= 0x00FF_FFFF);
    }

    /// One changed bit changes the check - the cheap sanity a degenerate
    /// (always-zero-feedback, or similar) implementation would fail.
    #[test]
    fn a_single_changed_byte_changes_the_result() {
        let a = crc24_ble(b"advertising channel PDU");
        let b = crc24_ble(b"advertising channel PDX");
        assert_ne!(a, b);
    }

    /// The empty message still produces a value - the initial value alone,
    /// reflected - rather than panicking on a zero-length slice.
    #[test]
    fn the_empty_message_does_not_panic() {
        assert_eq!(crc24_ble(&[]), reflect24(INIT));
    }
}
