// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! One 20 MHz OFDM symbol: 64 subcarriers, 48 of them data, 4 pilots, and
//! the training fields' fixed sequences.
//!
//! A symbol's spectrum is held as 64 values indexed by subcarrier from -32
//! to 31 (`at(k)` is where subcarrier `k` sits), the way the annex prints
//! them; [`crate::signal::wifi`]'s time-domain stages turn it into samples.

#[cfg(test)]
use num_complex::Complex;

/// A symbol's 64 subcarriers, -32 first.
#[cfg(test)]
pub type Spectrum = [Complex<f64>; 64];

/// Where subcarrier `k`, -32 to 31, sits in a [`Spectrum`].
#[cfg(test)]
pub fn at(k: i32) -> usize {
    (k + 32) as usize
}

/// The pilots and their values before polarity: +1 on -21, -7 and 7, -1 on
/// 21 (17.3.5.9, Equation 17-24).
pub const PILOTS: [(i32, f64); 4] = [(-21, 1.0), (-7, 1.0), (7, 1.0), (21, -1.0)];

/// The 48 data subcarriers in the order the points fill them: -26 to 26,
/// skipping 0 and the pilots (Equation 17-23's M(k)).
pub fn data_subcarriers() -> Vec<i32> {
    (-26..=26)
        .filter(|&k| k != 0 && !PILOTS.iter().any(|&(p, _)| p == k))
        .collect()
}

/// The 52 data subcarriers of HE-SIG-A and U-SIG, in the order their
/// points fill them: -28 to 28, skipping 0 and the pilots (27.3.11.7.4's
/// M_r20).
pub fn wide_data_subcarriers() -> Vec<i32> {
    (-28..=28)
        .filter(|&k| k != 0 && !PILOTS.iter().any(|&(p, _)| p == k))
        .collect()
}

/// The values HE and EHT put on the four edge subcarriers of the L-SIG and
/// the RL-SIG, for the receiver to measure them by (27.3.11.5, 802.11be
/// 36.3.12.5).
pub const EDGES: [(i32, f64); 4] = [(-28, -1.0), (-27, -1.0), (27, -1.0), (28, 1.0)];

/// The pilots' polarity in symbol `n` (SIGNAL is 0, the first DATA symbol 1):
/// the 127-long sequence of Equation 17-25, which is the data scrambler's
/// sequence from an all-ones register with 0 sent as +1 and 1 as -1.
pub fn polarity(n: usize) -> f64 {
    let s = super::scrambler::sequence_from_register([true; 7], 127);
    if s[n % 127] {
        -1.0
    } else {
        1.0
    }
}

/// The 48 points on their subcarriers, nothing else.
#[cfg(test)]
pub fn data_only(points: &[Complex<f64>]) -> Spectrum {
    assert_eq!(points.len(), 48, "a symbol carries 48 points");
    let mut out = [Complex::new(0.0, 0.0); 64];
    for (p, k) in points.iter().zip(data_subcarriers()) {
        out[at(k)] = *p;
    }
    out
}

/// Symbol `n`'s spectrum: its 48 points and the four pilots at that
/// symbol's polarity (Equation 17-22).
#[cfg(test)]
pub fn symbol(points: &[Complex<f64>], n: usize) -> Spectrum {
    let mut out = data_only(points);
    let p = polarity(n);
    for (k, v) in PILOTS {
        out[at(k)] = Complex::new(v * p, 0.0);
    }
    out
}

/// A 52-tone signalling symbol's spectrum (HE-SIG-A, U-SIG): its points on
/// [`wide_data_subcarriers`] and the four pilots at polarity p_n.
#[cfg(test)]
pub fn wide_symbol(points: &[Complex<f64>], n: usize) -> Spectrum {
    assert_eq!(points.len(), 52, "a wide symbol carries 52 points");
    let mut out = [Complex::new(0.0, 0.0); 64];
    for (p, k) in points.iter().zip(wide_data_subcarriers()) {
        out[at(k)] = *p;
    }
    let p = polarity(n);
    for (k, v) in PILOTS {
        out[at(k)] = Complex::new(v * p, 0.0);
    }
    out
}

/// The short training symbol's tones (Equation 17-6): every fourth
/// subcarrier from -24 to 24, each `+(1+j)` or `-(1+j)`, scaled by
/// `sqrt(13/6)` so twelve tones carry the power of 52.
#[cfg(test)]
const SHORT: [(i32, f64); 12] = [
    (-24, 1.0),
    (-20, -1.0),
    (-16, 1.0),
    (-12, -1.0),
    (-8, -1.0),
    (-4, 1.0),
    (4, -1.0),
    (8, -1.0),
    (12, 1.0),
    (16, 1.0),
    (20, 1.0),
    (24, 1.0),
];

/// The long training symbol, subcarriers -26 to 26 (Equation 17-8), 0 at DC:
/// the one table the receiver's references are built from too.
#[rustfmt::skip]
pub const LONG: [i8; 53] = [
    1, 1, -1, -1, 1, 1, -1, 1, -1, 1, 1, 1, 1, 1, 1, -1, -1, 1, 1, -1, 1, -1, 1, 1, 1, 1,
    0,
    1, -1, -1, 1, 1, -1, 1, -1, 1, -1, -1, -1, -1, -1, 1, 1, -1, -1, 1, -1, 1, -1, 1, 1, 1, 1,
];

/// The short training symbol's spectrum.
#[cfg(test)]
pub fn short_spectrum() -> Spectrum {
    let scale = (13.0f64 / 6.0).sqrt();
    let mut out = [Complex::new(0.0, 0.0); 64];
    for (k, sign) in SHORT {
        out[at(k)] = Complex::new(sign * scale, sign * scale);
    }
    out
}

/// The long training symbol's spectrum.
#[cfg(test)]
pub fn long_spectrum() -> Spectrum {
    let mut out = [Complex::new(0.0, 0.0); 64];
    for (i, &v) in LONG.iter().enumerate() {
        out[at(i as i32 - 26)] = Complex::new(v as f64, 0.0);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::wifi::annex::{self, MESSAGE};
    use crate::signal::wifi::bcc::{encode, puncture};
    use crate::signal::wifi::interleave::interleave;
    use crate::signal::wifi::map::map;
    use crate::signal::wifi::scrambler::scrambled;
    use crate::signal::wifi::signal_field::{self, Rate};

    /// The SIGNAL field's points, before the pilots, are Table I-10 (its
    /// pilot positions printed as X); with the pilots at polarity p0, I-11.
    #[test]
    fn the_signal_symbol_is_tables_i10_and_i11() {
        let bits = interleave(&encode(&signal_field::bits(Rate::R36, 100)), 1);
        let points = map(&bits, 1);
        annex::check_values("I-10", &data_only(&points), 0x8e56_8a0c_ac64_a125);
        annex::check_values("I-11", &symbol(&points, 0), 0x08d1_f9b2_c254_b2a2);
    }

    /// The first DATA symbol, pilots at polarity p1, is Table I-20; its
    /// first point, from bits 0111, sits on subcarrier 26 (I.1.6.3).
    #[test]
    fn the_first_data_symbol_is_table_i20() {
        let rate = Rate::R36;
        let coded = puncture(&encode(&scrambled(&MESSAGE, rate, 54)), rate.coding());
        let points = map(
            &interleave(&coded[..rate.n_cbps()], rate.n_bpsc()),
            rate.n_bpsc(),
        );
        let spectrum = symbol(&points, 1);
        assert_eq!(spectrum[at(26)], points[47]);
        annex::check_values("I-20", &spectrum, 0x0321_c0ec_c743_c645);
    }

    /// The data subcarriers are -26 to 26 without 0 and the four pilots,
    /// in order (Equation 17-23), and the pilots are where 17.3.5.9 says.
    #[test]
    fn the_layout_is_the_standards() {
        let data = data_subcarriers();
        assert_eq!(data.len(), 48);
        assert_eq!((data[0], data[47]), (-26, 26));
        assert!(data.windows(2).all(|w| w[0] < w[1]));
        for k in [0, -21, -7, 7, 21] {
            assert!(!data.contains(&k), "{k}");
        }
        assert_eq!(PILOTS.map(|(k, _)| k), [-21, -7, 7, 21]);
    }

    /// The wide symbol's 52 data subcarriers reach -28 and 28, skip DC
    /// and the four pilots, and rise; the edge values are 27.3.11.5's.
    #[test]
    fn the_wide_layout_is_the_standards() {
        let data = wide_data_subcarriers();
        assert_eq!(data.len(), 52);
        assert_eq!((data[0], data[51]), (-28, 28));
        assert!(data.windows(2).all(|w| w[0] < w[1]));
        for k in [0, -21, -7, 7, 21] {
            assert!(!data.contains(&k), "{k}");
        }
        assert_eq!(EDGES.map(|(k, _)| k), [-28, -27, 27, 28]);
    }

    /// The pilots' polarity is the scrambler's sequence from an all-ones
    /// register, 0 as +1 and 1 as -1 (17.3.5.10); Equation 17-25 opens
    /// 1, 1, 1, 1, -1, -1, -1, 1 and repeats every 127 symbols.
    #[test]
    fn the_polarity_is_the_scramblers_sequence() {
        let first: Vec<f64> = (0..8).map(polarity).collect();
        assert_eq!(first, [1.0, 1.0, 1.0, 1.0, -1.0, -1.0, -1.0, 1.0]);
        assert_eq!(polarity(3), polarity(130));
    }
}
