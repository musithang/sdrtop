// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The whole PPDU in time, at 20 Msample/s: the short and long training
//! fields, the SIGNAL symbol and the DATA symbols, one after another.
//!
//! **The window is the annex's, not a normative one.** Each field is its
//! inverse transform extended cyclically, one sample longer than its
//! duration, with the first and last samples at half weight (Annex I.1.3.1,
//! I.1.4.5); consecutive fields overlap by that one sample and are added
//! (I.1.7). The standard leaves the transition to the implementer
//! (17.3.2.5): this is the one its worked example uses, so it is the one a
//! receiver here is tested against.

use num_complex::Complex;
use rustfft::FftPlanner;

use super::bcc::{encode, puncture};
use super::interleave::interleave;
use super::map::map;
use super::ofdm::{long_spectrum, short_spectrum, symbol, Spectrum};
use super::scrambler::{scrambled, Layout};
use super::signal_field::{self, Rate};

/// One period of a spectrum in time: the 64-point inverse transform, the
/// subcarriers in the transform's order (0 to 31, then -32 to -1, Figure
/// 17-3), scaled by 1/64.
pub fn ifft(spectrum: &Spectrum) -> [Complex<f64>; 64] {
    let mut buf: Vec<Complex<f64>> = (0..64).map(|n| spectrum[(n + 32) % 64]).collect();
    FftPlanner::new().plan_fft_inverse(64).process(&mut buf);
    let mut out = [Complex::new(0.0, 0.0); 64];
    for (o, v) in out.iter_mut().zip(buf) {
        *o = v / 64.0;
    }
    out
}

/// `len` samples of one period read cyclically from `offset` samples
/// before its start, the ends at half weight.
fn windowed(period: &[Complex<f64>; 64], guard: usize, len: usize) -> Vec<Complex<f64>> {
    let mut out: Vec<Complex<f64>> = (0..len).map(|n| period[(n + 64 - guard) % 64]).collect();
    out[0] *= 0.5;
    out[len - 1] *= 0.5;
    out
}

/// The short training field: ten 0.8 us repeats, 8 us (17.3.3), and the
/// overlap sample.
pub fn short_training() -> Vec<Complex<f64>> {
    windowed(&ifft(&short_spectrum()), 0, 161)
}

/// The long training field: a 1.6 us guard and two 3.2 us symbols (17.3.3),
/// and the overlap sample.
pub fn long_training() -> Vec<Complex<f64>> {
    windowed(&ifft(&long_spectrum()), 32, 161)
}

/// One OFDM symbol in time: a 0.8 us guard and 3.2 us (Table 17-5), and the
/// overlap sample.
fn in_time(spectrum: &Spectrum) -> Vec<Complex<f64>> {
    windowed(&ifft(spectrum), 16, 81)
}

/// The SIGNAL symbol: BPSK at rate 1/2, pilots at polarity p0 (17.3.4).
pub fn signal_symbol(rate: Rate, length: usize) -> Vec<Complex<f64>> {
    signal_from_bits(&signal_field::bits(rate, length as u16))
}

/// A SIGNAL symbol carrying any 24 bits, valid or not: how a receiver's
/// checks are shown a field that should fail them.
pub fn signal_from_bits(bits: &[bool; 24]) -> Vec<Complex<f64>> {
    in_time(&symbol(&map(&interleave(&encode(bits), 1), 1), 0))
}

/// A signalling symbol in time from 48 coded bits: interleaved, BPSK,
/// turned by j when `quadrature` (QBPSK, Figure 19-7), pilots at polarity
/// p_n for the `n`-th symbol after the long training field.
pub fn sig_symbol(coded: &[bool], quadrature: bool, n: usize) -> Vec<Complex<f64>> {
    let turn = if quadrature {
        Complex::new(0.0, 1.0)
    } else {
        Complex::new(1.0, 0.0)
    };
    let points: Vec<Complex<f64>> = map(&interleave(coded, 1), 1)
        .into_iter()
        .map(|p| p * turn)
        .collect();
    in_time(&symbol(&points, n))
}

/// 48 bits of a two-symbol SIG field (HT-SIG, VHT-SIG-A) coded at rate 1/2
/// and sent as two symbols from the `first`-th after the long field, each
/// turned or not.
pub fn two_sig_symbols(
    bits: &[bool; 48],
    quadrature: [bool; 2],
    first: usize,
) -> [Vec<Complex<f64>>; 2] {
    let coded = encode(bits);
    [
        sig_symbol(&coded[..48], quadrature[0], first),
        sig_symbol(&coded[48..], quadrature[1], first + 1),
    ]
}

/// A symbol of 48 random points from `points`, pilots at polarity p_n: a
/// DATA symbol's stand-in.
pub fn random_symbol(
    rng: &mut crate::signal::dsp::testkit::Rng,
    points: &[Complex<f64>],
    n: usize,
) -> Vec<Complex<f64>> {
    let chosen: Vec<Complex<f64>> = (0..48)
        .map(|_| points[(rng.next_u64() % points.len() as u64) as usize])
        .collect();
    in_time(&symbol(&chosen, n))
}

/// HT-greenfield's first HT-LTF in 20 MHz (19.3.9.5, Equation 19-23): the
/// L-LTF's tones and +1, +1 on -28, -27, -1, -1 on 27, 28, behind a double
/// guard, twice; and the overlap sample.
pub fn greenfield_ltf() -> Vec<Complex<f64>> {
    let mut spectrum = long_spectrum();
    for (k, v) in [(-28, 1.0), (-27, 1.0), (27, -1.0), (28, -1.0)] {
        spectrum[super::ofdm::at(k)] = Complex::new(v, 0.0);
    }
    windowed(&ifft(&spectrum), 32, 161)
}

/// A preamble in time: `stf` from 0, `ltf` from 160, then each of
/// `symbols` 80 samples apart from 320, overlaps added.
pub fn assemble(
    stf: &[Complex<f64>],
    ltf: &[Complex<f64>],
    symbols: &[Vec<Complex<f64>>],
) -> Vec<Complex<f64>> {
    let mut out = vec![Complex::new(0.0, 0.0); 320 + 80 * symbols.len() + 1];
    let mut place = |at: usize, field: &[Complex<f64>]| {
        for (o, v) in out[at..at + field.len()].iter_mut().zip(field) {
            *o += v;
        }
    };
    place(0, stf);
    place(160, ltf);
    for (i, s) in symbols.iter().enumerate() {
        place(320 + 80 * i, s);
    }
    out
}

/// An L-SIG for `rate` and `length` as the `n`-th symbol after the long
/// field: 0 for the L-SIG itself, 1 for an RL-SIG (27.3.11.6).
pub fn l_sig_at(rate: Rate, length: u16, n: usize) -> Vec<Complex<f64>> {
    sig_symbol(&encode(&signal_field::bits(rate, length)), false, n)
}

/// An L-SIG as HE and EHT send it, as the `n`-th symbol after the long
/// field (0 the L-SIG, 1 its repeat): the edge subcarriers carrying
/// [`super::ofdm::EDGES`] for the receiver to measure them by.
pub fn wide_l_sig_at(rate: Rate, length: u16, n: usize) -> Vec<Complex<f64>> {
    let points = map(
        &interleave(&encode(&signal_field::bits(rate, length)), 1),
        1,
    );
    let mut spectrum = symbol(&points, n);
    for (k, v) in super::ofdm::EDGES {
        spectrum[super::ofdm::at(k)] = Complex::new(v, 0.0);
    }
    in_time(&spectrum)
}

/// A 52-bit SIG field (HE-SIG-A, U-SIG) coded at rate 1/2 and sent from the
/// `first`-th symbol after the long field: two symbols, each interleaved
/// over its 52 tones, or for an extended-range PPDU four, each followed by
/// its repeat sent without interleaving, the first repeat QBPSK
/// (27.3.11.7.4, 802.11be 36.3.12.7.3).
pub fn wide_sig_symbols(bits: &[bool; 52], er: bool, first: usize) -> Vec<Vec<Complex<f64>>> {
    use super::interleave::interleave_wide;
    use super::ofdm::wide_symbol;
    let coded = encode(bits);
    let send = |c: &[bool], interleaved: bool, turn: Complex<f64>, n: usize| {
        let c = if interleaved {
            interleave_wide(c)
        } else {
            c.to_vec()
        };
        let points: Vec<Complex<f64>> = map(&c, 1).into_iter().map(|p| p * turn).collect();
        in_time(&wide_symbol(&points, n))
    };
    let (one, j) = (Complex::new(1.0, 0.0), Complex::new(0.0, 1.0));
    let (a, b) = coded.split_at(52);
    if er {
        vec![
            send(a, true, one, first),
            send(a, false, j, first + 1),
            send(b, true, one, first + 2),
            send(b, false, one, first + 3),
        ]
    } else {
        vec![send(a, true, one, first), send(b, true, one, first + 1)]
    }
}

/// The DATA symbols, the n-th (from 1) with pilots at polarity p_n.
fn data_symbols(psdu: &[u8], rate: Rate, initial: u8) -> Vec<Vec<Complex<f64>>> {
    let coded = puncture(&encode(&scrambled(psdu, rate, initial)), rate.coding());
    coded
        .chunks(rate.n_cbps())
        .enumerate()
        .map(|(i, c)| {
            let points = map(&interleave(c, rate.n_bpsc()), rate.n_bpsc());
            in_time(&symbol(&points, i + 1))
        })
        .collect()
}

/// The whole PPDU for `psdu` at `rate`, scrambled from `initial`: training
/// from 0, SIGNAL from 16 us, DATA from 20 us, each field's overlap sample
/// added to the next field's first.
pub fn ppdu(psdu: &[u8], rate: Rate, initial: u8) -> Vec<Complex<f64>> {
    let n_sym = Layout::new(rate, psdu.len()).n_sym;
    let mut out = vec![Complex::new(0.0, 0.0); 400 + 80 * n_sym + 1];
    let mut place = |at: usize, field: &[Complex<f64>]| {
        for (o, v) in out[at..at + field.len()].iter_mut().zip(field) {
            *o += v;
        }
    };
    place(0, &short_training());
    place(160, &long_training());
    place(320, &signal_symbol(rate, psdu.len()));
    for (i, s) in data_symbols(psdu, rate, initial).iter().enumerate() {
        place(400 + 80 * i, s);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::wifi::annex::{self, MESSAGE};
    use crate::signal::wifi::ofdm::{long_spectrum, short_spectrum};
    use crate::signal::wifi::signal_field::Rate;

    /// The training sequences in frequency, Tables I-2 and I-5.
    #[test]
    fn the_training_spectra_are_tables_i2_and_i5() {
        annex::check_values("I-2", &short_spectrum(), 0xcd0b_5407_dad2_6da5);
        annex::check_values("I-5", &long_spectrum(), 0xded2_038f_3597_8d82);
    }

    /// One period of the short sequence in time, Table I-3: the 64-point
    /// inverse transform, scaled by 1/64.
    #[test]
    fn one_short_period_is_table_i3() {
        annex::check_values("I-3", &ifft(&short_spectrum()), 0x9582_8fd4_3c2d_2e45);
    }

    /// The short and long training fields, 161 samples each with the
    /// annex's half-weight ends, Tables I-4 and I-6; the short one repeats
    /// every 16 samples (0.8 us, 17.3.3).
    #[test]
    fn the_training_fields_are_tables_i4_and_i6() {
        let stf = short_training();
        assert_eq!(stf.len(), 161);
        assert!((1..144).all(|n| (stf[n] - stf[n + 16]).norm() < 1e-12));
        annex::check_values("I-4", &stf, 0xd121_f2e1_6717_247d);
        annex::check_values("I-6", &long_training(), 0xccda_06ba_cc14_087d);
    }

    /// The SIGNAL symbol in time, 81 samples, Table I-12.
    #[test]
    fn the_signal_symbol_in_time_is_table_i12() {
        annex::check_values(
            "I-12",
            &signal_symbol(Rate::R36, MESSAGE.len()),
            0x54f6_e2c5_5df3_845f,
        );
    }

    /// The whole PPDU, 881 samples (Tables I-22 to I-30): 16 us of
    /// training, 4 us of SIGNAL and six 4 us DATA symbols (Table 17-5), the
    /// last sample the closing half of the window.
    #[test]
    fn the_examples_whole_ppdu_is_tables_i22_to_i30() {
        let ppdu = ppdu(&MESSAGE, Rate::R36, 54);
        assert_eq!(ppdu.len(), 320 + 80 + 6 * 80 + 1);
        annex::check_values("PPDU", &ppdu, 0x0f1e_f134_2e08_9afa);
    }

    /// Our own frames, outside the annex: Liszt's Second Hungarian
    /// Rhapsody's name at every rate, each the length its rate gives it.
    #[test]
    fn our_own_frame_has_the_length_its_rate_gives_it() {
        let psdu = "Liszt Ferenc: II. magyar rapszódia, S.244/2".as_bytes();
        for rate in [
            Rate::R6,
            Rate::R9,
            Rate::R12,
            Rate::R18,
            Rate::R24,
            Rate::R36,
            Rate::R48,
            Rate::R54,
        ] {
            let n_sym = crate::signal::wifi::scrambler::Layout::new(rate, psdu.len()).n_sym;
            assert_eq!(
                ppdu(psdu, rate, 0x5d).len(),
                400 + 80 * n_sym + 1,
                "{rate:?}"
            );
        }
    }
}
