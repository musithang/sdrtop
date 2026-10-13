// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! HT-SIG (IEEE 802.11-2024, 19.3.9.4.3): the two QBPSK symbols an 802.11n
//! frame describes itself in, under a CRC-8; and the airtime of a
//! greenfield frame, which has no L-SIG to give it.

/// What an HT-SIG says that something here reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HtSig {
    /// The MCS index, 0 to 127.
    pub mcs: u8,
    /// A 40 MHz PPDU.
    pub cbw40: bool,
    /// HT Length: the PSDU's octets.
    pub length: u16,
    /// N_STS - N_SS, 0 to 3.
    pub stbc: u8,
    pub ldpc: bool,
    pub short_gi: bool,
    /// The extension spatial streams, 0 to 3.
    pub ness: u8,
}

/// Why a SIG field's 48 bits were not accepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SigFault {
    /// The six tail bits were not all zero.
    Tail,
    /// The CRC did not match.
    Crc,
}

impl SigFault {
    /// The check that refused it, in a word or two.
    pub fn label(self) -> &'static str {
        match self {
            SigFault::Tail => "tail not zero",
            SigFault::Crc => "CRC",
        }
    }
}

/// The CRC-8 of 19.3.9.4.4 over `bits`, c7 first as sent: generator
/// x^8 + x^2 + x + 1, the register preset to ones, the result complemented
/// (Figure 19-8). HE-SIG-A and U-SIG send its first four (27.3.11.7.3).
pub fn crc8(bits: &[bool]) -> [bool; 8] {
    // c[i] is the register's c_i.
    let mut c = [true; 8];
    for &m in bits {
        let feedback = m ^ c[7];
        for i in (1..8).rev() {
            c[i] = c[i - 1];
        }
        c[0] = feedback;
        c[1] ^= feedback;
        c[2] ^= feedback;
    }
    let mut out = [false; 8];
    for (o, i) in out.iter_mut().zip((0..8).rev()) {
        *o = !c[i];
    }
    out
}

/// 48 decoded bits checked: the tail (bits 42 to 47) zero, then the CRC
/// over bits 0 to 33 at bits 34 to 41. VHT-SIG-A puts its CRC in the same
/// place (21.3.8.3.3).
pub fn check(bits: &[bool]) -> Result<(), SigFault> {
    if bits[42..48].iter().any(|&b| b) {
        return Err(SigFault::Tail);
    }
    if crc8(&bits[..34])[..] != bits[34..42] {
        return Err(SigFault::Crc);
    }
    Ok(())
}

/// The integer LSB first in `bits`.
fn number(bits: &[bool]) -> u32 {
    bits.iter()
        .enumerate()
        .fold(0, |n, (i, &b)| n | (b as u32) << i)
}

/// Data bits a symbol for each base MCS (MCS mod 8), one stream, in 20 and
/// 40 MHz (Tables 19-27 and 19-28).
const N_DBPS_20: [u32; 8] = [26, 52, 78, 104, 156, 208, 234, 260];
const N_DBPS_40: [u32; 8] = [54, 108, 162, 216, 324, 432, 486, 540];

impl HtSig {
    /// The fields of 48 bits, checked first (Figure 19-6): HT-SIG1's MCS
    /// at 0 to 6, CBW at 7, HT Length at 8 to 23; HT-SIG2's STBC at 4 and
    /// 5, FEC at 6, Short GI at 7, N_ESS at 8 and 9.
    pub fn from_bits(bits: &[bool]) -> Result<Self, SigFault> {
        check(bits)?;
        let two = &bits[24..];
        Ok(Self {
            mcs: number(&bits[..7]) as u8,
            cbw40: bits[7],
            length: number(&bits[8..24]) as u16,
            stbc: number(&two[4..6]) as u8,
            ldpc: two[6],
            short_gi: two[7],
            ness: number(&two[8..10]) as u8,
        })
    }

    /// A greenfield frame's time on the air, in nanoseconds (Equations
    /// 19-92 and 19-93): HT-GF-STF and HT-LTF1, 8 us each; HT-SIG, 8 us;
    /// 4 us for each further HT-LTF; then the DATA symbols, 4 us or 3.6
    /// with the short guard. The symbols by Equation 19-32 for BCC. LDPC's
    /// count (19.3.11.7.5) and the unequal modulations are refused, not
    /// approximated.
    pub fn greenfield_airtime_ns(&self) -> Result<u32, &'static str> {
        if self.ldpc {
            return Err("LDPC: its symbol count is not computed here");
        }
        let (n_ss, n_dbps) = match (self.mcs, self.cbw40) {
            (0..=31, false) => (self.mcs as u32 / 8 + 1, N_DBPS_20[self.mcs as usize % 8]),
            (0..=31, true) => (self.mcs as u32 / 8 + 1, N_DBPS_40[self.mcs as usize % 8]),
            (32, true) => (1, 12),
            (33..=76, _) => return Err("an unequal modulation: not computed here"),
            _ => return Err("no such MCS at this width"),
        };
        let n_dbps = n_dbps * n_ss;
        let n_sts = n_ss + self.stbc as u32;
        let dltf = match n_sts {
            1 => 1,
            2 => 2,
            3 | 4 => 4,
            _ => return Err("more space-time streams than HT has"),
        };
        let eltf = [0, 1, 2, 4][self.ness as usize];
        let n_ltf = dltf + eltf;
        if n_ltf > 5 {
            return Err("more HT-LTFs than HT allows");
        }
        let symbol_ns = if self.short_gi { 3_600 } else { 4_000 };
        // One BCC encoder up to 300 Mb/s, two above (19.3.11.5).
        let n_es = if n_dbps * 1_000 / symbol_ns > 300 {
            2
        } else {
            1
        };
        let m_stbc = if self.stbc > 0 { 2 } else { 1 };
        // An HT Length of 0 is a sounding PPDU with no DATA field
        // (Table 19-11, NOTE).
        let n_sym = if self.length == 0 {
            0
        } else {
            m_stbc * (8 * self.length as u32 + 16 + 6 * n_es).div_ceil(m_stbc * n_dbps)
        };
        Ok(24_000 + 4_000 * (n_ltf - 1) + n_sym * symbol_ns)
    }
}

/// The 48 bits of `sig` as sent, with its CRC and a zero tail: Smoothing
/// and Not Sounding set, the reserved bit 1 as transmitted, no aggregation.
#[cfg(test)]
pub fn bits(sig: &HtSig) -> [bool; 48] {
    let mut b = [false; 48];
    let mut put = |at: usize, width: usize, v: u32| {
        for i in 0..width {
            b[at + i] = v >> i & 1 == 1;
        }
    };
    put(0, 7, sig.mcs as u32);
    put(7, 1, sig.cbw40 as u32);
    put(8, 16, sig.length as u32);
    put(24, 1, 1);
    put(25, 1, 1);
    put(26, 1, 1);
    put(28, 2, sig.stbc as u32);
    put(30, 1, sig.ldpc as u32);
    put(31, 1, sig.short_gi as u32);
    put(32, 2, sig.ness as u32);
    let crc = crc8(&b[..34]);
    b[34..42].copy_from_slice(&crc);
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(s: &str) -> Vec<bool> {
        s.chars()
            .filter(|c| !c.is_whitespace())
            .map(|c| c == '1')
            .collect()
    }

    /// The standard's own example (19.3.9.4.4): those 34 bits give
    /// 1 0 1 0 1 0 0 0, c7 first.
    #[test]
    fn the_crc_is_the_standards_example() {
        let m = word("1111 0001 0010 0110 0000 0000 1110 0000 00");
        assert_eq!(m.len(), 34);
        assert_eq!(crc8(&m).to_vec(), word("1010 1000"));
    }

    fn sample() -> HtSig {
        HtSig {
            mcs: 7,
            cbw40: false,
            length: 1500,
            stbc: 0,
            ldpc: false,
            short_gi: true,
            ness: 0,
        }
    }

    /// Every field's bits land where Figure 19-6 puts them, and come back.
    #[test]
    fn the_fields_go_round() {
        let cases = [
            sample(),
            HtSig {
                mcs: 31,
                cbw40: true,
                length: 65_535,
                stbc: 2,
                ldpc: true,
                short_gi: false,
                ness: 3,
            },
            HtSig {
                mcs: 0,
                length: 0,
                ..sample()
            },
        ];
        for sig in cases {
            let b = bits(&sig);
            assert_eq!(check(&b), Ok(()), "{sig:?}");
            assert_eq!(HtSig::from_bits(&b), Ok(sig));
        }
        let b = bits(&sample());
        // MCS 7 LSB first in bits 0 to 6; HT Length from bit 8; the
        // reserved bit (HT-SIG2 bit 2) set to 1 on transmit; Short GI at
        // HT-SIG2 bit 7.
        assert_eq!(&b[..7], &word("1110000")[..]);
        assert_eq!(&b[8..24], &word("0011101110100000")[..]);
        assert!(b[24 + 2] && b[24 + 7]);
        assert!(b[42..].iter().all(|&x| !x));
    }

    /// One flipped bit anywhere in the protected part fails the CRC; a set
    /// tail bit fails the tail first.
    #[test]
    fn a_flipped_bit_is_caught() {
        let good = bits(&sample());
        for i in 0..42 {
            let mut b = good;
            b[i] = !b[i];
            assert_eq!(check(&b), Err(SigFault::Crc), "bit {i}");
        }
        let mut b = good;
        b[45] = true;
        assert_eq!(check(&b), Err(SigFault::Tail));
    }

    /// Greenfield airtime by Equation 19-92, worked by hand: MCS 7, 1500
    /// octets, BCC, one stream. N_SYM = ceil((12000 + 16 + 6) / 260) = 47;
    /// 24 us of preamble and HT-SIG, then 47 symbols of 4 us, or 3.6 with
    /// the short guard; STBC doubles the streams (two HT-LTFs, +4 us) and
    /// pairs the symbols, 2 ceil(12022 / 520) = 48.
    #[test]
    fn greenfield_airtime_is_the_equation() {
        let long = HtSig {
            short_gi: false,
            ..sample()
        };
        assert_eq!(long.greenfield_airtime_ns(), Ok(212_000));
        assert_eq!(sample().greenfield_airtime_ns(), Ok(193_200));
        let stbc = HtSig { stbc: 1, ..long };
        assert_eq!(stbc.greenfield_airtime_ns(), Ok(220_000));
        // Two streams at MCS 15 (520 bits a symbol), and an extension
        // stream's HT-LTF: ceil(12022 / 520) = 24, three HT-LTFs.
        let two = HtSig {
            mcs: 15,
            ness: 1,
            ..long
        };
        assert_eq!(two.greenfield_airtime_ns(), Ok(24_000 + 8_000 + 24 * 4_000));
        // 40 MHz, MCS 7: 540 bits a symbol, ceil(12022 / 540) = 23.
        let wide = HtSig {
            cbw40: true,
            ..long
        };
        assert_eq!(wide.greenfield_airtime_ns(), Ok(24_000 + 23 * 4_000));
    }

    /// What the equation does not cover here is refused, never guessed:
    /// LDPC's symbol count, and the unequal modulations.
    #[test]
    fn ldpc_and_unequal_modulation_are_refused() {
        let ldpc = HtSig {
            ldpc: true,
            ..sample()
        };
        assert!(ldpc.greenfield_airtime_ns().is_err());
        let ueqm = HtSig {
            mcs: 40,
            ..sample()
        };
        assert!(ueqm.greenfield_airtime_ns().is_err());
    }
}
