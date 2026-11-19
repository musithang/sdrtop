// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The block interleaver: one OFDM symbol's coded bits, spread across the
//! subcarriers and across the bits of each constellation point.
//!
//! Two permutations (17.3.5.7): the first sends adjacent coded bits to
//! subcarriers far apart, the second alternates them between the more and
//! less reliable bits of a constellation point. Both are tables of where
//! each index goes, so the inverse is the same tables read the other way.

/// The first permutation for a symbol of `n_cbps` bits: `out[k]` is `i`,
/// where coded bit `k` lands (Equation 17-15).
pub fn first_permutation(n_cbps: usize) -> Vec<usize> {
    (0..n_cbps)
        .map(|k| (n_cbps / 16) * (k % 16) + k / 16)
        .collect()
}

/// The second permutation: `out[i]` is `j` (Equations 17-16, 17-17), with
/// `s = max(N_BPSC / 2, 1)`.
pub fn second_permutation(n_cbps: usize, n_bpsc: usize) -> Vec<usize> {
    let s = (n_bpsc / 2).max(1);
    (0..n_cbps)
        .map(|i| s * (i / s) + (i + n_cbps - (16 * i / n_cbps)) % s)
        .collect()
}

/// Where each coded bit of a symbol goes: `j` for each `k`.
fn positions(n_cbps: usize, n_bpsc: usize) -> Vec<usize> {
    let second = second_permutation(n_cbps, n_bpsc);
    first_permutation(n_cbps)
        .into_iter()
        .map(|i| second[i])
        .collect()
}

/// One symbol's coded bits, interleaved: the bit at `k` moves to `j`.
pub fn interleave<T: Copy + Default>(bits: &[T], n_bpsc: usize) -> Vec<T> {
    let mut out = vec![T::default(); bits.len()];
    for (k, j) in positions(bits.len(), n_bpsc).into_iter().enumerate() {
        out[j] = bits[k];
    }
    out
}

/// The inverse (Equations 17-18, 17-19): the bit at `j` back to `k`, for
/// bits or for the readings a receiver has in their place.
pub fn deinterleave<T: Copy>(bits: &[T], n_bpsc: usize) -> Vec<T> {
    positions(bits.len(), n_bpsc)
        .into_iter()
        .map(|j| bits[j])
        .collect()
}

/// Where each of a 52-tone signalling symbol's BPSK coded bits goes
/// (HE-SIG-A, U-SIG): the first permutation with 13 columns and 4 rows
/// (Table 27-36, the operation of 21.3.10.8); BPSK has no second.
fn wide_positions() -> Vec<usize> {
    (0..52).map(|k| 4 * (k % 13) + k / 13).collect()
}

/// A 52-tone signalling symbol's coded bits, interleaved.
#[cfg(test)]
pub fn interleave_wide<T: Copy + Default>(bits: &[T]) -> Vec<T> {
    let mut out = vec![T::default(); bits.len()];
    for (k, j) in wide_positions().into_iter().enumerate() {
        out[j] = bits[k];
    }
    out
}

/// The inverse, for a 52-tone signalling symbol's readings.
pub fn deinterleave_wide<T: Copy>(bits: &[T]) -> Vec<T> {
    wide_positions().into_iter().map(|j| bits[j]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::wifi::annex::{self, MESSAGE};
    use crate::signal::wifi::bcc::{encode, puncture};
    use crate::signal::wifi::scrambler::scrambled;
    use crate::signal::wifi::signal_field::{self, Rate};

    /// The two permutations for 16-QAM, against Tables I-17 and I-18, and
    /// the annex's own worked index: k = 17 goes to i = 13, then j = 12.
    #[test]
    fn the_permutations_are_tables_i17_and_i18() {
        let first = first_permutation(192);
        let second = second_permutation(192, 4);
        assert_eq!((first[17], second[13]), (13, 12));
        annex::check_map("I-17", &first, 0x32eb_aa6e_a62c_afa5);
        annex::check_map("I-18", &second, 0x252e_22e6_2fb2_1125);
    }

    /// The SIGNAL field interleaved (BPSK, 48 bits), against Table I-9.
    #[test]
    fn the_examples_signal_field_interleaves_to_table_i9() {
        let coded = encode(&signal_field::bits(Rate::R36, 100));
        annex::check_bits("I-9", &interleave(&coded, 1), 0x698a_b93a_d934_ca99);
    }

    /// The first DATA symbol interleaved, against Table I-19.
    #[test]
    fn the_first_data_symbol_interleaves_to_table_i19() {
        let rate = Rate::R36;
        let coded = puncture(&encode(&scrambled(&MESSAGE, rate, 54)), rate.coding());
        let first = interleave(&coded[..rate.n_cbps()], rate.n_bpsc());
        annex::check_bits("I-19", &first, 0x250b_c80d_00b6_7dd8);
    }

    /// Deinterleaving undoes interleaving, at every rate's symbol size.
    /// The 52-tone permutation: adjacent coded bits four tones apart,
    /// every thirteenth bit on the next tone, and undone by its inverse.
    #[test]
    fn the_wide_permutation_is_thirteen_columns_of_four() {
        let p = wide_positions();
        assert_eq!(p.len(), 52);
        assert_eq!(&p[..4], &[0, 4, 8, 12]);
        assert_eq!((p[12], p[13], p[51]), (48, 1, 51));
        let bits: Vec<u8> = (0..52).collect();
        assert_eq!(deinterleave_wide(&interleave_wide(&bits)), bits);
    }

    #[test]
    fn deinterleave_undoes_interleave_at_every_rate() {
        for rate in [Rate::R6, Rate::R12, Rate::R24, Rate::R48] {
            let n = rate.n_cbps();
            let bits: Vec<bool> = (0..n).map(|i| (i * 7 + i / 3) % 5 < 2).collect();
            let there = interleave(&bits, rate.n_bpsc());
            assert_ne!(there, bits, "{rate:?}");
            assert_eq!(deinterleave(&there, rate.n_bpsc()), bits, "{rate:?}");
        }
    }
}
