// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The DATA field's bits (SERVICE, PSDU, tail, pad) and the scrambler they
//! go through.
//!
//! **The register is written as the standard draws it**, not borrowed from
//! the Bluetooth whitening that shares its polynomial: `x^7 + x^4 + 1` in
//! another structure gives the right sequence at the wrong phase, which no
//! test of the sequence's own properties notices and every real frame does.
//! So the sequence is checked against the annex's own, bit for bit.

use super::signal_field::Rate;

/// The DATA field's sizes for a PSDU of `length` octets at `rate`
/// (17.3.5.4, Equations 17-11 to 17-13): 16 SERVICE bits, the PSDU, six
/// tail bits, then pad to a whole number of symbols.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub n_sym: usize,
    pub n_data: usize,
    pub n_pad: usize,
}

impl Layout {
    pub fn new(rate: Rate, length: usize) -> Self {
        let need = 16 + 8 * length + 6;
        let n_sym = need.div_ceil(rate.n_dbps());
        let n_data = n_sym * rate.n_dbps();
        Self {
            n_sym,
            n_data,
            n_pad: n_data - need,
        }
    }
}

/// The DATA field before scrambling: SERVICE (16 zeros, 17.3.5.2), the PSDU
/// with each octet bit 0 first (17.3.5.5), six tail zeros (17.3.5.3) and
/// the pad zeros (17.3.5.4).
pub fn data_bits(psdu: &[u8], rate: Rate) -> Vec<bool> {
    let layout = Layout::new(rate, psdu.len());
    let mut bits = vec![false; 16];
    for &octet in psdu {
        bits.extend((0..8).map(|i| octet >> i & 1 == 1));
    }
    bits.resize(layout.n_data, false);
    bits
}

/// The scrambling sequence whose first seven bits are `initial`, bit 0 first
/// (SCRAMBLER_INITIAL_VALUE, Table 17-7), continued by the generator
/// `x^7 + x^4 + 1` (Equation 17-14, Figure 17-7): each bit is the one seven
/// back XOR the one four back.
pub fn sequence(initial: u8, n: usize) -> Vec<bool> {
    let mut s: Vec<bool> = (0..7).map(|i| initial >> i & 1 == 1).collect();
    extend(&mut s, n);
    s
}

/// The sequence from a register state `x1` to `x7` as Figure 17-7 labels
/// them: `x1` holds the bit just sent, `x7` the one sent seven bits ago.
pub fn sequence_from_register(register: [bool; 7], n: usize) -> Vec<bool> {
    // The register is the last seven bits of the sequence, newest first.
    let mut s: Vec<bool> = register.iter().rev().copied().collect();
    extend(&mut s, n + 7);
    s.split_off(7)
}

fn extend(s: &mut Vec<bool>, n: usize) {
    while s.len() < n {
        let k = s.len();
        s.push(s[k - 7] ^ s[k - 4]);
    }
    s.truncate(n);
}

/// `bits` XOR `sequence`, bit by bit: scrambling and descrambling alike.
pub fn apply(bits: &[bool], sequence: &[bool]) -> Vec<bool> {
    bits.iter().zip(sequence).map(|(&b, &s)| b ^ s).collect()
}

/// The DATA field as transmitted before coding: scrambled from `initial`,
/// then the six tail bits set back to zero so the encoder ends in its zero
/// state (17.3.5.3).
pub fn scrambled(psdu: &[u8], rate: Rate, initial: u8) -> Vec<bool> {
    let bits = data_bits(psdu, rate);
    let mut out = apply(&bits, &sequence(initial, bits.len()));
    let tail = 16 + 8 * psdu.len();
    for b in &mut out[tail..tail + 6] {
        *b = false;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::wifi::annex::{self, MESSAGE};
    use crate::signal::wifi::signal_field::Rate;

    /// The example's sizes, as I.1.5.1 states them: six symbols of 144 data
    /// bits, 864 in all, 42 of them pad.
    #[test]
    fn the_examples_data_field_is_six_symbols() {
        let f = Layout::new(Rate::R36, MESSAGE.len());
        assert_eq!((f.n_sym, f.n_data, f.n_pad), (6, 864, 42));
    }

    /// The bits before scrambling, against Table I-13.
    #[test]
    fn the_examples_data_bits_are_table_i13() {
        annex::check_bits(
            "I-13",
            &data_bits(&MESSAGE, Rate::R36),
            0x8d67_ce5d_781c_dcb5,
        );
    }

    /// The sequence for the example's initial value, 54, against Table I-14:
    /// its first seven bits are the value itself, LSB first (Table 17-7).
    #[test]
    fn the_sequence_for_54_is_table_i14() {
        let s = sequence(54, 127);
        assert_eq!(&s[..7], &[false, true, true, false, true, true, false]);
        annex::check_bits("I-14", &s, 0x8fcb_db54_b775_cb0b);
    }

    /// The scrambled bits, tail zeroed after scrambling, against Table I-15.
    #[test]
    fn the_examples_scrambled_bits_are_table_i15() {
        let bits = scrambled(&MESSAGE, Rate::R36, 54);
        annex::check_bits("I-15", &bits, 0x40ab_d798_f605_2440);
        let tail = 16 + 8 * MESSAGE.len();
        assert!(bits[tail..tail + 6].iter().all(|&b| !b), "the tail is zero");
    }

    /// The register's period is 127, and from an all-ones register it runs
    /// as 17.3.5.5's NOTE 1 begins: 00001110 11110010.
    #[test]
    fn the_sequence_repeats_every_127_bits() {
        let s = sequence(0b111_1111, 127 * 2);
        assert_eq!(&s[..127], &s[127..]);
        let from_ones: Vec<bool> = sequence_from_register([true; 7], 16);
        let want = [0, 0, 0, 0, 1, 1, 1, 0, 1, 1, 1, 1, 0, 0, 1, 0];
        assert_eq!(from_ones, want.map(|b| b == 1).to_vec());
    }

    /// Scrambling twice with one sequence gives the input back: the same
    /// scrambler descrambles (17.3.5.5).
    #[test]
    fn scrambling_twice_is_the_identity() {
        let bits = data_bits(&MESSAGE, Rate::R36);
        let once = apply(&bits, &sequence(54, bits.len()));
        assert_ne!(once, bits);
        assert_eq!(apply(&once, &sequence(54, bits.len())), bits);
    }
}
