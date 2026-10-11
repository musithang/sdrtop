// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The rates of the 20 MHz OFDM PHY, and the SIGNAL field that names one.

/// The eight rates of a 20 MHz channel (IEEE 802.11-2024, Table 17-4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rate {
    R6,
    R9,
    R12,
    R18,
    R24,
    R36,
    R48,
    R54,
}

/// The convolutional code's rate (17.3.5.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Coding {
    Half,
    TwoThirds,
    ThreeQuarters,
}

impl Rate {
    /// R1 to R4, in transmit order (Table 17-6).
    pub fn rate_bits(self) -> [bool; 4] {
        let r = match self {
            Rate::R6 => 0b1101,
            Rate::R9 => 0b1111,
            Rate::R12 => 0b0101,
            Rate::R18 => 0b0111,
            Rate::R24 => 0b1001,
            Rate::R36 => 0b1011,
            Rate::R48 => 0b0001,
            Rate::R54 => 0b0011,
        };
        // Written as the table prints them, R1 leftmost.
        [r & 8 != 0, r & 4 != 0, r & 2 != 0, r & 1 != 0]
    }

    /// Coded bits per subcarrier, N_BPSC (Table 17-4).
    pub fn n_bpsc(self) -> usize {
        match self {
            Rate::R6 | Rate::R9 => 1,
            Rate::R12 | Rate::R18 => 2,
            Rate::R24 | Rate::R36 => 4,
            Rate::R48 | Rate::R54 => 6,
        }
    }

    /// The code's rate (Table 17-4).
    pub fn coding(self) -> Coding {
        match self {
            Rate::R6 | Rate::R12 | Rate::R24 => Coding::Half,
            Rate::R48 => Coding::TwoThirds,
            Rate::R9 | Rate::R18 | Rate::R36 | Rate::R54 => Coding::ThreeQuarters,
        }
    }

    /// Coded bits per OFDM symbol, N_CBPS: 48 data subcarriers (Table 17-5)
    /// of N_BPSC each.
    pub fn n_cbps(self) -> usize {
        48 * self.n_bpsc()
    }

    /// Data bits per OFDM symbol, N_DBPS: N_CBPS at the code's rate.
    pub fn n_dbps(self) -> usize {
        match self.coding() {
            Coding::Half => self.n_cbps() / 2,
            Coding::TwoThirds => self.n_cbps() * 2 / 3,
            Coding::ThreeQuarters => self.n_cbps() * 3 / 4,
        }
    }
}

/// The SIGNAL field's 24 bits in transmit order (17.3.4, Figure 17-5): RATE,
/// a reserved zero, the 12-bit LENGTH in octets LSB first, even parity over
/// bits 0 to 16, and six tail zeros.
pub fn bits(rate: Rate, length: u16) -> [bool; 24] {
    debug_assert!(length < 4096, "LENGTH is 12 bits");
    let mut out = [false; 24];
    out[..4].copy_from_slice(&rate.rate_bits());
    for i in 0..12 {
        out[5 + i] = length >> i & 1 == 1;
    }
    out[17] = out[..17].iter().filter(|&&b| b).count() % 2 == 1;
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::wifi::annex;

    /// The example's SIGNAL field: 36 Mb/s and 100 octets, against Table I-7.
    #[test]
    fn the_examples_signal_field_is_table_i7() {
        let bits = bits(Rate::R36, 100);
        annex::check_bits("I-7", &bits, 0x8f23_d944_5fca_1dd7);
    }

    /// The parity bit is even over bits 0 to 16, so it follows the length.
    #[test]
    fn the_parity_follows_the_length() {
        let p = |len| bits(Rate::R36, len)[17];
        assert_ne!(p(100), p(101));
        for len in [0u16, 1, 2, 100, 4095] {
            let ones = bits(Rate::R6, len)[..18].iter().filter(|&&b| b).count();
            assert_eq!(ones % 2, 0, "{len}");
        }
    }

    /// Every rate's RATE bits, and what each carries per symbol, as Tables
    /// 17-4 and 17-6 give them.
    #[test]
    fn every_rate_is_as_the_tables_give_it() {
        let rows = [
            (Rate::R6, "1101", 1, 48, 24),
            (Rate::R9, "1111", 1, 48, 36),
            (Rate::R12, "0101", 2, 96, 48),
            (Rate::R18, "0111", 2, 96, 72),
            (Rate::R24, "1001", 4, 192, 96),
            (Rate::R36, "1011", 4, 192, 144),
            (Rate::R48, "0001", 6, 288, 192),
            (Rate::R54, "0011", 6, 288, 216),
        ];
        for (rate, r, bpsc, cbps, dbps) in rows {
            let got: String = rate
                .rate_bits()
                .iter()
                .map(|&b| if b { '1' } else { '0' })
                .collect();
            assert_eq!(got, r, "{rate:?}");
            assert_eq!(rate.n_bpsc(), bpsc, "{rate:?}");
            assert_eq!(rate.n_cbps(), cbps, "{rate:?}");
            assert_eq!(rate.n_dbps(), dbps, "{rate:?}");
        }
    }

    /// The tail is six zeros and bit 4 is reserved as zero, whatever is sent.
    #[test]
    fn the_reserved_bit_and_the_tail_are_zero() {
        let b = bits(Rate::R54, 4095);
        assert!(!b[4]);
        assert!(b[18..].iter().all(|&x| !x));
    }
}
