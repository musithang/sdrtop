// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The binary convolutional code, and the puncturing that raises its rate.

use super::signal_field::Coding;
use crate::signal::dsp::code::conv::{correlation, Code};

/// The code: constraint length 7, generators g0 = 133 and g1 = 171 octal,
/// A before B (17.3.5.6, Figure 17-8), in `dsp::code::conv`'s convention,
/// which is the one this clause writes them in.
pub const CODE: Code = Code::new(7, 0o133, 0o171);

/// Rate 1/2 from the zero state (17.3.5.6).
pub fn encode(bits: &[bool]) -> Vec<bool> {
    CODE.encode(bits)
}

/// Which of one period's A and B outputs are sent, in transmit order
/// (Figure 17-9): for 3/4, of three input bits' pairs, A0 B0 A1 B2 are sent
/// and B1, A2 stolen; for 2/3, of two, A0 B0 A1, B1 stolen. Rate 1/2 sends
/// everything.
fn sent(coding: Coding) -> &'static [bool] {
    match coding {
        Coding::Half => &[true, true],
        Coding::TwoThirds => &[true, true, true, false],
        Coding::ThreeQuarters => &[true, true, true, false, false, true],
    }
}

/// Puncture rate-1/2 output to `coding`.
#[cfg(test)]
pub fn puncture(coded: &[bool], coding: Coding) -> Vec<bool> {
    coded
        .iter()
        .zip(sent(coding).iter().cycle())
        .filter_map(|(&b, &k)| k.then_some(b))
        .collect()
}

/// Received readings (positive for a 1) back to rate 1/2: each where its
/// bit was sent from, and a zero for each stolen bit, an erasure that
/// favours neither value, so the decoder weighs it as nothing heard
/// ("a dummy zero metric", 17.3.5.6).
pub fn depuncture(readings: &[f32], coding: Coding) -> Vec<f32> {
    let pattern = sent(coding);
    let kept = pattern.iter().filter(|&&k| k).count();
    let mut out = Vec::with_capacity(readings.len() * pattern.len() / kept);
    let mut next = readings.iter();
    'periods: loop {
        for &k in pattern {
            if k {
                match next.next() {
                    Some(&r) => out.push(r),
                    None => break 'periods,
                }
            } else {
                out.push(0.0);
            }
        }
    }
    // A period cut short ends where its last reading went.
    out
}

/// Decode readings at `coding` with the Viterbi decoder. `terminated` for a
/// field that ends in its six tail zeros, the SIGNAL field (17.3.4.4); not
/// for DATA, whose tail sits after the PSDU and before the pad, so the pad
/// (scrambled zeros) takes the encoder out of the zero state again
/// (17.3.5.3, 17.3.5.4). `None` if what is left is not whole steps.
pub fn decode(readings: &[f32], coding: Coding, terminated: bool) -> Option<Vec<bool>> {
    let r = depuncture(readings, coding);
    if r.is_empty() || !r.len().is_multiple_of(2) {
        return None;
    }
    CODE.decode(r.len() / 2, terminated, |k, a, b| correlation(&r, k, a, b))
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

    /// Readings, +1 for a 1 and -1 for a 0.
    fn readings(bits: &[bool]) -> Vec<f32> {
        bits.iter().map(|&b| if b { 1.0 } else { -1.0 }).collect()
    }

    /// Depuncturing puts every received reading back where its bit was
    /// sent from and a zero, an erasure, where a bit was stolen.
    #[test]
    fn depuncturing_restores_positions_with_erasures() {
        let sent: Vec<bool> = (0..12).map(|i| i % 3 != 1).collect();
        let back = depuncture(
            &readings(&puncture(&sent, Coding::ThreeQuarters)),
            Coding::ThreeQuarters,
        );
        assert_eq!(back.len(), 12);
        for (i, (&r, &b)) in back.iter().zip(&sent).enumerate() {
            let stolen = matches!(i % 6, 3 | 4);
            assert_eq!(
                r,
                if stolen {
                    0.0
                } else if b {
                    1.0
                } else {
                    -1.0
                },
                "{i}"
            );
        }
        let two = depuncture(&[1.0, -1.0, 1.0], Coding::TwoThirds);
        assert_eq!(two, [1.0, -1.0, 1.0, 0.0]);
    }

    /// The annex's coded bits decode back to its source bits: the SIGNAL
    /// field at rate 1/2 (Table I-8 to I-7), the DATA field at 3/4 (I-16 to
    /// I-15), the coded bits being the ones held to the annex above.
    #[test]
    fn the_annexes_coded_bits_decode_to_its_source_bits() {
        let signal = signal_field::bits(Rate::R36, 100);
        let coded = readings(&encode(&signal));
        assert_eq!(
            decode(&coded, Coding::Half, true).as_deref(),
            Some(&signal[..])
        );

        let data = scrambled(&MESSAGE, Rate::R36, 54);
        let coded = readings(&puncture(&encode(&data), Coding::ThreeQuarters));
        assert_eq!(decode(&coded, Coding::ThreeQuarters, false), Some(data));
    }

    /// At every rate a frame decodes through scattered errors, the
    /// erasures the puncturing left counting for nothing either way.
    #[test]
    fn every_coding_decodes_through_scattered_errors() {
        let mut data: Vec<bool> = (0..480).map(|i| (i * 11 + i / 5) % 7 < 3).collect();
        data.extend([false; 6]);
        for coding in [Coding::Half, Coding::TwoThirds, Coding::ThreeQuarters] {
            let mut r = readings(&puncture(&encode(&data), coding));
            for i in (17..r.len()).step_by(61) {
                r[i] = -r[i];
            }
            assert_eq!(
                decode(&r, coding, true).as_deref(),
                Some(&data[..]),
                "{coding:?}"
            );
        }
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
