// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The binary convolutional code, and the puncturing that raises its rate.

use super::signal_field::Coding;

/// The generators, g0 = 133 and g1 = 171 octal (17.3.5.6, Figure 17-8),
/// constraint length 7. Read most significant bit first, each is a tap on
/// the current input and then on the six bits before it.
const G0: u8 = 0o133;
const G1: u8 = 0o171;

/// Rate 1/2: for each input bit, output A (from g0) then B (from g1),
/// starting from the zero state (17.3.5.6).
pub fn encode(bits: &[bool]) -> Vec<bool> {
    // The window: the current bit in bit 6, the six before it below.
    let mut window: u8 = 0;
    let mut out = Vec::with_capacity(bits.len() * 2);
    for &b in bits {
        window = (window >> 1) | ((b as u8) << 6);
        out.push((window & G0).count_ones() % 2 == 1);
        out.push((window & G1).count_ones() % 2 == 1);
    }
    out
}

/// Puncture rate-1/2 output to `coding` (Figure 17-9): for 3/4, of every
/// three input bits' A and B pairs, A0 B0 A1 B2 are sent and B1, A2 stolen;
/// for 2/3, of every two, A0 B0 A1, B1 stolen.
pub fn puncture(coded: &[bool], coding: Coding) -> Vec<bool> {
    // Which of the A, B outputs of one period are kept, in transmit order.
    let keep: &[bool] = match coding {
        Coding::Half => return coded.to_vec(),
        Coding::TwoThirds => &[true, true, true, false],
        Coding::ThreeQuarters => &[true, true, true, false, false, true],
    };
    coded
        .iter()
        .zip(keep.iter().cycle())
        .filter_map(|(&b, &k)| k.then_some(b))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::wifi::annex::{self, MESSAGE};
    use crate::signal::wifi::scrambler::scrambled;
    use crate::signal::wifi::signal_field::{self, Coding, Rate};

    /// The SIGNAL field at rate 1/2, against Table I-8.
    #[test]
    fn the_examples_signal_field_encodes_to_table_i8() {
        let coded = encode(&signal_field::bits(Rate::R36, 100));
        annex::check_bits("I-8", &coded, 0xc6ac_cb44_d3d1_961d);
    }

    /// The DATA field at rate 3/4, against Table I-16.
    #[test]
    fn the_examples_data_field_encodes_to_table_i16() {
        let coded = puncture(
            &encode(&scrambled(&MESSAGE, Rate::R36, 54)),
            Coding::ThreeQuarters,
        );
        annex::check_bits("I-16", &coded, 0x7075_4807_760d_2e89);
    }

    /// Zeros in, zeros out: the code is linear and starts in the zero state.
    #[test]
    fn zeros_encode_to_zeros() {
        assert!(encode(&[false; 40]).iter().all(|&b| !b));
    }

    /// A single one rings out the two generators, A then B, bit by bit:
    /// 133 octal is 1011011 and 171 octal is 1111001, current bit first.
    #[test]
    fn an_impulse_rings_out_the_generators() {
        let mut input = vec![false; 7];
        input[0] = true;
        let out = encode(&input);
        let a: Vec<bool> = out.iter().step_by(2).copied().collect();
        let b: Vec<bool> = out.iter().skip(1).step_by(2).copied().collect();
        let bits = |s: &str| s.chars().map(|c| c == '1').collect::<Vec<_>>();
        assert_eq!(a, bits("1011011"));
        assert_eq!(b, bits("1111001"));
    }

    /// Every rate's punctured symbol is N_CBPS bits from N_DBPS in.
    #[test]
    fn a_symbols_worth_punctures_to_n_cbps() {
        for rate in [
            Rate::R6,
            Rate::R9,
            Rate::R18,
            Rate::R36,
            Rate::R48,
            Rate::R54,
        ] {
            let coded = puncture(&encode(&vec![true; rate.n_dbps()]), rate.coding());
            assert_eq!(coded.len(), rate.n_cbps(), "{rate:?}");
        }
    }
}
