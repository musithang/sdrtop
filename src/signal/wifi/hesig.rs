// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! HE-SIG-A (IEEE 802.11-2024, 27.3.11.7) and U-SIG (802.11be-2024,
//! 36.3.12.7): what an 802.11ax or 802.11be frame says about its network
//! before the MAC header.

use super::htsig::SigFault;

/// What a frame says of the network it belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bss {
    /// The BSS colour, 0 to 63: a label an access point picks, not an
    /// address, and one two networks can share.
    pub colour: u8,
    /// Sent to an access point.
    pub uplink: bool,
    /// The TXOP it protects, in microseconds; `None` when it says none.
    pub txop_us: Option<u32>,
}

/// Which HE PPDU the LENGTH remainder and the rotations allow, before its
/// SIG-A is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeKind {
    /// Remainder 1: SU or TB, told apart by SIG-A's Format bit.
    SuOrTb,
    /// Remainder 2 with a BPSK symbol after the RL-SIG's next.
    Mu,
    /// Remainder 2 with a QBPSK one: four SIG-A symbols.
    ErSu,
}

/// An HE PPDU, named.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HePpdu {
    Su,
    ErSu,
    Mu,
    Tb,
}

/// What an HE-SIG-A said that something here reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeSigA {
    pub ppdu: HePpdu,
    pub bss: Bss,
}

/// U-SIG's version-independent fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct USig {
    /// 0 for EHT; 1 to 7 for PHYs after it.
    pub version: u8,
    pub bss: Bss,
}

/// The TXOP field's duration (Table 27-19): 127 is none; otherwise B1 to
/// B6 in 8 us units when B0 is 0, or 512 us plus 128 us units when B0 is 1.
pub fn txop_us(code: u32) -> Option<u32> {
    match code {
        127.. => None,
        c if c & 1 == 0 => Some(8 * (c >> 1)),
        c => Some(512 + 128 * (c >> 1)),
    }
}

/// 52 decoded bits checked: the tail (bits 46 to 51) zero, then the CRC's
/// four bits at 42 to 45 against the CRC-8 over bits 0 to 41, its c7 to c4.
pub fn check(bits: &[bool]) -> Result<(), SigFault> {
    if bits[46..52].iter().any(|&b| b) {
        return Err(SigFault::Tail);
    }
    if super::htsig::crc8(&bits[..42])[..4] != bits[42..46] {
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

impl HeSigA {
    /// The fields of 52 bits, checked first, laid out as `kind` says.
    pub fn from_bits(bits: &[bool], kind: HeKind) -> Result<Self, SigFault> {
        check(bits)?;
        let txop_us = txop_us(number(&bits[26..33]));
        let (ppdu, colour, uplink) = match kind {
            // SU and ER SU (Table 27-19): Format B0, UL/DL B2, colour B8-13.
            HeKind::SuOrTb | HeKind::ErSu if bits[0] => {
                let ppdu = if kind == HeKind::ErSu {
                    HePpdu::ErSu
                } else {
                    HePpdu::Su
                };
                (ppdu, number(&bits[8..14]), bits[2])
            }
            // TB (Table 27-22): Format 0, colour B1-6, and uplink by what
            // it is, an answer to a trigger.
            HeKind::SuOrTb => (HePpdu::Tb, number(&bits[1..7]), true),
            // An ER SU whose Format bit says TB is no PPDU the table has.
            HeKind::ErSu => return Err(SigFault::Crc),
            // MU (Table 27-21): UL/DL B0, colour B5-10.
            HeKind::Mu => (HePpdu::Mu, number(&bits[5..11]), bits[0]),
        };
        Ok(Self {
            ppdu,
            bss: Bss {
                colour: colour as u8,
                uplink,
                txop_us,
            },
        })
    }
}

impl USig {
    /// The version-independent fields of 52 bits, checked first.
    pub fn from_bits(bits: &[bool]) -> Result<Self, SigFault> {
        check(bits)?;
        Ok(Self {
            version: number(&bits[..3]) as u8,
            bss: Bss {
                colour: number(&bits[7..13]) as u8,
                uplink: bits[6],
                txop_us: txop_us(number(&bits[13..20])),
            },
        })
    }
}

/// The TXOP field for a duration in microseconds (the inverse of
/// [`txop_us`], for durations it can carry exactly).
#[cfg(test)]
fn txop_code(us: Option<u32>) -> u32 {
    match us {
        None => 127,
        Some(d) if d < 512 => 2 * (d / 8),
        Some(d) => 2 * ((d - 512) / 128) + 1,
    }
}

/// Bits from `at`, `width` of them, set to `v`, LSB first.
#[cfg(test)]
fn put(b: &mut [bool; 52], at: usize, width: usize, v: u32) {
    for i in 0..width {
        b[at + i] = v >> i & 1 == 1;
    }
}

/// CRC-4 and a zero tail on 52 bits whose first 42 are set.
#[cfg(test)]
fn seal(mut b: [bool; 52]) -> [bool; 52] {
    let crc = super::htsig::crc8(&b[..42]);
    b[42..46].copy_from_slice(&crc[..4]);
    b
}

/// An HE-SIG-A's 52 bits as sent, reserved bits 1 as transmitted.
#[cfg(test)]
pub fn he_bits(sig: &HeSigA) -> [bool; 52] {
    let mut b = [false; 52];
    let txop = txop_code(sig.bss.txop_us);
    match sig.ppdu {
        HePpdu::Su | HePpdu::ErSu => {
            put(&mut b, 0, 1, 1);
            put(&mut b, 2, 1, sig.bss.uplink as u32);
            put(&mut b, 8, 6, sig.bss.colour as u32);
            put(&mut b, 14, 1, 1);
            put(&mut b, 26 + 14, 1, 1);
        }
        HePpdu::Mu => {
            put(&mut b, 0, 1, sig.bss.uplink as u32);
            put(&mut b, 5, 6, sig.bss.colour as u32);
            put(&mut b, 26 + 7, 1, 1);
        }
        HePpdu::Tb => {
            put(&mut b, 1, 6, sig.bss.colour as u32);
            put(&mut b, 23, 1, 1);
        }
    }
    put(&mut b, 26, 7, txop);
    seal(b)
}

/// A U-SIG's 52 bits as sent, its Disregard and Validate bits set as an
/// EHT MU PPDU's are.
#[cfg(test)]
pub fn usig_bits(sig: &USig) -> [bool; 52] {
    let mut b = [false; 52];
    put(&mut b, 0, 3, sig.version as u32);
    put(&mut b, 6, 1, sig.bss.uplink as u32);
    put(&mut b, 7, 6, sig.bss.colour as u32);
    put(&mut b, 13, 7, txop_code(sig.bss.txop_us));
    put(&mut b, 20, 6, 0b11_1111);
    put(&mut b, 26 + 2, 1, 1);
    put(&mut b, 26 + 8, 1, 1);
    seal(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bss(colour: u8, uplink: bool, txop_us: Option<u32>) -> Bss {
        Bss {
            colour,
            uplink,
            txop_us,
        }
    }

    /// TXOP's two scales and its none (Table 27-19).
    #[test]
    fn the_txop_has_two_scales() {
        assert_eq!(txop_us(127), None);
        assert_eq!(txop_us(0), Some(0));
        assert_eq!(txop_us(31 << 1), Some(248));
        assert_eq!(txop_us(8 << 1 | 1), Some(512 + 8 * 128));
        // B0 1 with B1 to B6 all ones is 127 itself: the longest is 62.
        assert_eq!(txop_us(62 << 1 | 1), Some(512 + 62 * 128));
        assert_eq!(txop_us(63 << 1 | 1), None);
        for us in [
            None,
            Some(0),
            Some(248),
            Some(504),
            Some(512),
            Some(1536),
            Some(8448),
        ] {
            assert_eq!(txop_us(txop_code(us)), us, "{us:?}");
        }
    }

    /// Every HE layout's fields come back from its bits, the colour where
    /// its table puts it; a TB PPDU is uplink by what it is.
    #[test]
    fn each_he_layout_goes_round() {
        let cases = [
            (HePpdu::Su, HeKind::SuOrTb, bss(23, true, Some(1536))),
            (HePpdu::Tb, HeKind::SuOrTb, bss(41, true, Some(248))),
            (HePpdu::Mu, HeKind::Mu, bss(63, false, None)),
            (HePpdu::ErSu, HeKind::ErSu, bss(5, false, Some(0))),
        ];
        for (ppdu, kind, bss) in cases {
            let sig = HeSigA { ppdu, bss };
            let b = he_bits(&sig);
            assert_eq!(check(&b), Ok(()), "{sig:?}");
            assert_eq!(HeSigA::from_bits(&b, kind), Ok(sig));
        }
        let su = he_bits(&HeSigA {
            ppdu: HePpdu::Su,
            bss: bss(23, true, None),
        });
        // Colour 23 at B8 to B13, LSB first; UL/DL at B2.
        let colour: Vec<bool> = su[8..14].to_vec();
        assert_eq!(colour, [true, true, true, false, true, false]);
        assert!(su[2]);
    }

    /// U-SIG's version-independent part, at any version.
    #[test]
    fn the_usig_goes_round_at_every_version() {
        for version in 0..8 {
            let sig = USig {
                version,
                bss: bss(version * 9, version % 2 == 0, Some(512)),
            };
            let b = usig_bits(&sig);
            assert_eq!(USig::from_bits(&b), Ok(sig), "{version}");
        }
    }

    /// A four-bit CRC lets one error in 16 through, single flips among
    /// them: of the 42 protected bits, most flips are caught, and every
    /// flip of the CRC's own four. A set tail bit fails the tail first.
    #[test]
    fn a_flipped_bit_is_caught() {
        let good = usig_bits(&USig {
            version: 0,
            bss: bss(17, false, Some(248)),
        });
        let escaped: Vec<usize> = (0..42)
            .filter(|&i| {
                let mut b = good;
                b[i] = !b[i];
                check(&b).is_ok()
            })
            .collect();
        eprintln!("single flips the four-bit CRC lets through: {escaped:?}");
        assert!(escaped.len() <= 42 / 8, "{escaped:?}");
        for i in 42..46 {
            let mut b = good;
            b[i] = !b[i];
            assert_eq!(check(&b), Err(SigFault::Crc), "bit {i}");
        }
        let mut b = good;
        b[50] = true;
        assert_eq!(check(&b), Err(SigFault::Tail));
    }
}
