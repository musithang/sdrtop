// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The payload's bit fields, each to what it says: the QEX paper's Table 2
//! tags, decoded by the rules of its Appendix A and the reference programs
//! that complete it.
//!
//! Every decoder here is total over its field's width and refuses the values
//! the paper leaves unassigned, rather than mapping them onto something near.
//! The encoders exist for the tests (a field decoded is checked by encoding
//! it back) and for the hash, which a receiver computes from a call it heard.

use std::fmt;

/// `c28` values below this are tokens (DE, QRZ, CQ and its modifiers): the
/// reference programs' `NTOKENS`.
pub const NTOKENS: u32 = 2_063_592;

/// The number of 22-bit hash codes `c28` reserves after the tokens: `MAX22`.
pub const MAX22: u32 = 4_194_304;

/// `g15` values below this are 4-character locators: `MAXGRID4`, 18 x 18 x
/// 10 x 10 (Appendix A).
pub const MAXGRID4: u32 = 32_400;

/// `g25`'s range: 18 x 18 x 10 x 10 x 24 x 24 six-character locators.
const MAXGRID6: u32 = 18 * 18 * 10 * 10 * 24 * 24;

/// The standard call's six positions (`std_call_to_c28`): the first may be
/// blank, the second is a letter or digit, the third a digit, the last three
/// a letter or blank.
const A1: &[u8] = b" 0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const A2: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const A4: &[u8] = b" ABCDEFGHIJKLMNOPQRSTUVWXYZ";

/// The 38 characters of a nonstandard call and of the hash input
/// (`nonstd_to_c58`, `hashcodes`).
const C38: &[u8] = b" 0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ/";

/// The 42 characters of free text (`free_text_to_f71`).
const C42: &[u8] = b" 0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ+-./?";

/// The multiplier of the call sign hash (`hashcodes`).
const HASH_PRIME: u64 = 47_055_833_459;

/// A field whose bits the paper gives no meaning: the message is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Malformed(pub &'static str);

impl fmt::Display for Malformed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unassigned {} value", self.0)
    }
}

/// What a `c28` field carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum C28 {
    De,
    Qrz,
    /// CQ, with its modifier if any: three digits or one to four letters.
    Cq(Option<String>),
    /// A 22-bit hash of a call sent elsewhere in full.
    Hash22(u32),
    /// A standard call, as sent (no suffix; the suffix bits are separate).
    Call(String),
}

/// `c28` to what it says (the paper's Table 7).
pub fn c28_decode(n: u32) -> Result<C28, Malformed> {
    match n {
        0 => Ok(C28::De),
        1 => Ok(C28::Qrz),
        2 => Ok(C28::Cq(None)),
        3..=1002 => Ok(C28::Cq(Some(format!("{:03}", n - 3)))),
        // CQ A to CQ ZZZZ: the letters in base 27 with blank as zero, offset
        // by 1003. Table 7's four ranges (1004-1029, 1031-1731, 1760-20685,
        // 21443-532443) are exactly the values whose digits have no blank
        // after a letter; the gaps between them are those that do.
        1004..=532_443 => letters(n - 1003).map(|s| C28::Cq(Some(s))),
        _ if n < NTOKENS => Err(Malformed("c28 token")),
        _ if n < NTOKENS + MAX22 => Ok(C28::Hash22(n - NTOKENS)),
        _ => std_call(n - NTOKENS - MAX22).map(C28::Call),
    }
}

/// A CQ modifier's letters, base 27, blank as zero, at most four.
fn letters(mut v: u32) -> Result<String, Malformed> {
    let mut out = Vec::new();
    while v > 0 {
        out.push((v % 27) as u8);
        v /= 27;
    }
    if out.len() > 4 || out.contains(&0) {
        return Err(Malformed("c28 token"));
    }
    Ok(out.iter().rev().map(|&d| A4[d as usize] as char).collect())
}

/// A standard call from its 28-bit index (`std_call_to_c28` backwards).
fn std_call(mut v: u32) -> Result<String, Malformed> {
    let i6 = v % 27;
    v /= 27;
    let i5 = v % 27;
    v /= 27;
    let i4 = v % 27;
    v /= 27;
    let i3 = v % 10;
    v /= 10;
    let i2 = v % 36;
    v /= 36;
    let i1 = v;
    if i1 >= 37 {
        return Err(Malformed("c28 call"));
    }
    // A blank suffix letter followed by a letter is no call.
    let suffix = [i4, i5, i6];
    if suffix.windows(2).any(|w| w[0] == 0 && w[1] != 0) {
        return Err(Malformed("c28 call"));
    }
    let mut s = String::with_capacity(6);
    if i1 != 0 {
        s.push(A1[i1 as usize] as char);
    }
    s.push(A2[i2 as usize] as char);
    s.push((b'0' + i3 as u8) as char);
    for i in suffix.into_iter().filter(|&i| i != 0) {
        s.push(A4[i as usize] as char);
    }
    Ok(s)
}

/// A standard call to its `c28` value, or `None` if it is not one: one or two
/// prefix characters, then a digit in the third place (counting a blank first
/// place for a one-character prefix), then up to three letters.
pub fn std_call_c28(call: &str) -> Option<u32> {
    let b = call.as_bytes();
    // The call area's digit must land in the third place: a one-character
    // prefix gets a leading blank, and trailing blanks fill the suffix. A
    // prefix may hold a digit of its own (4X5TY), so the third place is
    // looked at first.
    let digit = |i: usize| b.get(i).is_some_and(u8::is_ascii_digit);
    let six: Vec<u8> = if digit(2) {
        b.to_vec()
    } else if digit(1) {
        std::iter::once(b' ').chain(b.iter().copied()).collect()
    } else {
        return None;
    };
    if six.len() > 6 || six.len() < 3 {
        return None;
    }
    let mut six = six;
    six.resize(6, b' ');
    let at = |table: &[u8], c: u8| table.iter().position(|&t| t == c).map(|i| i as u32);
    let i1 = at(A1, six[0])?;
    let i2 = at(A2, six[1])?;
    let i3 = u32::from(six[2].checked_sub(b'0').filter(|d| *d < 10)?);
    let i4 = at(A4, six[3])?;
    let i5 = at(A4, six[4])?;
    let i6 = at(A4, six[5])?;
    let n = NTOKENS
        + MAX22
        + 36 * 10 * 27 * 27 * 27 * i1
        + 10 * 27 * 27 * 27 * i2
        + 27 * 27 * 27 * i3
        + 27 * 27 * i4
        + 27 * i5
        + i6;
    // Decoding must give back exactly the call: no internal blanks.
    (std_call(n - NTOKENS - MAX22).ok()?.as_str() == call).then_some(n)
}

/// What a `g15` field carries (`grid4_to_g15`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum G15 {
    Grid(String),
    Blank,
    Rrr,
    Rr73,
    R73,
    /// A signal report in dB, -30 to +99.
    Report(i8),
}

/// `g15` to what it says.
pub fn g15_decode(v: u32) -> Result<G15, Malformed> {
    if v < MAXGRID4 {
        let field_lon = v / (18 * 100);
        let field_lat = v / 100 % 18;
        let sq_lon = v / 10 % 10;
        let sq_lat = v % 10;
        return Ok(G15::Grid(
            [
                (b'A' + field_lon as u8) as char,
                (b'A' + field_lat as u8) as char,
                (b'0' + sq_lon as u8) as char,
                (b'0' + sq_lat as u8) as char,
            ]
            .iter()
            .collect(),
        ));
    }
    // `grid4_to_g15`: blank 1, RRR 2, RR73 3, 73 4, a report r as r + 35.
    match v - MAXGRID4 {
        1 => Ok(G15::Blank),
        2 => Ok(G15::Rrr),
        3 => Ok(G15::Rr73),
        4 => Ok(G15::R73),
        r @ 5..=134 => Ok(G15::Report((r as i32 - 35) as i8)),
        _ => Err(Malformed("g15")),
    }
}

/// A `g15` back to its value, for the tests.
#[cfg(test)]
pub fn g15_encode(g: &G15) -> u32 {
    match g {
        G15::Grid(s) => {
            let b = s.as_bytes();
            u32::from(b[0] - b'A') * 1800
                + u32::from(b[1] - b'A') * 100
                + u32::from(b[2] - b'0') * 10
                + u32::from(b[3] - b'0')
        }
        G15::Blank => MAXGRID4 + 1,
        G15::Rrr => MAXGRID4 + 2,
        G15::Rr73 => MAXGRID4 + 3,
        G15::R73 => MAXGRID4 + 4,
        G15::Report(r) => (i32::from(*r) + 35) as u32 + MAXGRID4,
    }
}

/// `g25` to a six-character locator (`grid6_to_g25` backwards).
pub fn g25_decode(v: u32) -> Result<String, Malformed> {
    if v >= MAXGRID6 {
        return Err(Malformed("g25"));
    }
    let sub_lat = v % 24;
    let sub_lon = v / 24 % 24;
    let sq_lat = v / 576 % 10;
    let sq_lon = v / 5760 % 10;
    let field_lat = v / 57_600 % 18;
    let field_lon = v / 1_036_800;
    Ok([
        (b'A' + field_lon as u8) as char,
        (b'A' + field_lat as u8) as char,
        (b'0' + sq_lon as u8) as char,
        (b'0' + sq_lat as u8) as char,
        (b'A' + sub_lon as u8) as char,
        (b'A' + sub_lat as u8) as char,
    ]
    .iter()
    .collect())
}

/// A six-character locator to `g25`, for the tests.
#[cfg(test)]
pub fn g25_encode(s: &str) -> u32 {
    let b = s.as_bytes();
    u32::from(b[0] - b'A') * 18 * 10 * 10 * 24 * 24
        + u32::from(b[1] - b'A') * 10 * 10 * 24 * 24
        + u32::from(b[2] - b'0') * 10 * 24 * 24
        + u32::from(b[3] - b'0') * 24 * 24
        + u32::from(b[4] - b'A') * 24
        + u32::from(b[5] - b'A')
}

/// Eleven characters in base 38, the first the most significant.
fn base38(text: &str) -> Option<u64> {
    let padded = format!("{text:<11}");
    if padded.len() != 11 {
        return None;
    }
    padded.bytes().try_fold(0u64, |n, c| {
        let j = C38.iter().position(|&t| t == c)? as u64;
        Some(n * 38 + j)
    })
}

/// `c58`: a nonstandard call of up to eleven characters, left-adjusted.
pub fn c58_decode(mut v: u64) -> Result<String, Malformed> {
    let mut chars = [b' '; 11];
    for slot in chars.iter_mut().rev() {
        *slot = C38[(v % 38) as usize];
        v /= 38;
    }
    if v != 0 {
        return Err(Malformed("c58"));
    }
    let s = String::from_utf8_lossy(&chars).trim().to_string();
    if s.is_empty() || s.contains(' ') {
        return Err(Malformed("c58"));
    }
    Ok(s)
}

/// A nonstandard call to `c58`, for the tests.
#[cfg(test)]
pub fn c58_encode(call: &str) -> u64 {
    base38(call).expect("a c58 call")
}

/// `f71`: free text of up to thirteen characters, right-adjusted.
pub fn f71_decode(mut v: u128) -> Result<String, Malformed> {
    let mut chars = [b' '; 13];
    for slot in chars.iter_mut().rev() {
        *slot = C42[(v % 42) as usize];
        v /= 42;
    }
    if v != 0 {
        return Err(Malformed("f71"));
    }
    Ok(String::from_utf8_lossy(&chars).trim().to_string())
}

/// Free text to `f71`, for the tests.
#[cfg(test)]
pub fn f71_encode(text: &str) -> u128 {
    format!("{text:>13}").bytes().fold(0u128, |n, c| {
        n * 42 + C42.iter().position(|&t| t == c).expect("an f71 character") as u128
    })
}

/// A call's three hashes (`hashcodes`): the call left-adjusted in eleven
/// characters, read in base 38, multiplied by a fixed prime with the 64-bit
/// product wrapping, and its top 10, 12 or 22 bits kept. `None` for text that
/// is not made of the 38 characters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hashes {
    pub h10: u32,
    pub h12: u32,
    pub h22: u32,
}

pub fn hashes(call: &str) -> Option<Hashes> {
    let n = base38(call)?;
    let p = HASH_PRIME.wrapping_mul(n);
    Some(Hashes {
        h10: (p >> 54) as u32,
        h12: (p >> 52) as u32,
        h22: (p >> 42) as u32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_7_tokens_and_cq_modifiers() {
        assert_eq!(c28_decode(0), Ok(C28::De));
        assert_eq!(c28_decode(1), Ok(C28::Qrz));
        assert_eq!(c28_decode(2), Ok(C28::Cq(None)));
        assert_eq!(c28_decode(3), Ok(C28::Cq(Some("000".into()))));
        assert_eq!(c28_decode(1002), Ok(C28::Cq(Some("999".into()))));
        // Each of Table 7's letter ranges, at both ends.
        for (n, s) in [
            (1004, "A"),
            (1029, "Z"),
            (1031, "AA"),
            (1731, "ZZ"),
            (1760, "AAA"),
            (20685, "ZZZ"),
            (21443, "AAAA"),
            (532_443, "ZZZZ"),
        ] {
            assert_eq!(c28_decode(n), Ok(C28::Cq(Some(s.into()))), "{n}");
        }
        // The gaps between them are unassigned.
        for n in [1003, 1030, 1732, 1759, 20686, 21442, 532_444, NTOKENS - 1] {
            assert!(c28_decode(n).is_err(), "{n}");
        }
    }

    #[test]
    fn hashes_and_calls_sit_where_table_7_puts_them() {
        assert_eq!(c28_decode(NTOKENS), Ok(C28::Hash22(0)));
        assert_eq!(c28_decode(NTOKENS + MAX22 - 1), Ok(C28::Hash22(MAX22 - 1)));
        // The first index of the call range is a blank, a zero, a zero digit
        // and a blank suffix: "00", which still reads as what was sent.
        assert_eq!(c28_decode(NTOKENS + MAX22), Ok(C28::Call("00".into())));
        // The top of 28 bits is the last call: "ZZ9ZZZ".
        assert_eq!(c28_decode((1 << 28) - 1), Ok(C28::Call("ZZ9ZZZ".into())));
    }

    #[test]
    fn standard_calls_match_std_call_to_c28() {
        // Values worked out by `std_call_to_c28`'s formula.
        for (call, n) in [
            ("K1ABC", 10_214_965),
            ("W9XYZ", 12_751_800),
            ("KH1ABC", 158_427_955),
            ("K1AB", 10_214_962),
            ("4X5TY", 48_296_356),
        ] {
            assert_eq!(std_call_c28(call), Some(n), "{call}");
            assert_eq!(c28_decode(n), Ok(C28::Call(call.into())), "{call}");
        }
        for not_std in ["PJ4/K1ABC", "YW18FIFA", "K1ABCD", "1", ""] {
            assert_eq!(std_call_c28(not_std), None, "{not_std}");
        }
    }

    #[test]
    fn a_blank_inside_a_suffix_is_no_call() {
        // " K1 A ": suffix blank then A.
        let n = NTOKENS + MAX22 + 10 * 27 * 27 * 27 * 20 + 27 * 27 * 27 + 27;
        assert_eq!(c28_decode(n), Err(Malformed("c28 call")));
    }

    #[test]
    fn g15_grids_reports_and_words() {
        assert_eq!(g15_decode(0), Ok(G15::Grid("AA00".into())));
        assert_eq!(g15_decode(MAXGRID4 - 1), Ok(G15::Grid("RR99".into())));
        for g in [
            G15::Grid("FN20".into()),
            G15::Grid("JN97".into()),
            G15::Blank,
            G15::Rrr,
            G15::Rr73,
            G15::R73,
            G15::Report(-30),
            G15::Report(-8),
            G15::Report(0),
            G15::Report(99),
        ] {
            assert_eq!(g15_decode(g15_encode(&g)), Ok(g.clone()), "{g:?}");
        }
        assert!(g15_decode(MAXGRID4).is_err());
        assert!(g15_decode(MAXGRID4 + 135).is_err());
        assert!(g15_decode((1 << 15) - 1).is_err());
    }

    #[test]
    fn g25_round_trips_and_refuses_beyond_the_grid() {
        for g in ["AA00AA", "JN97LM", "IO91NP", "RR99XX"] {
            assert_eq!(g25_decode(g25_encode(g)).as_deref(), Ok(g));
        }
        assert!(g25_decode(MAXGRID6).is_err());
    }

    #[test]
    fn c58_and_f71_round_trip() {
        for c in ["PJ4/K1ABC", "YW18FIFA", "KH1/KH7Z", "A"] {
            assert_eq!(c58_decode(c58_encode(c)).as_deref(), Ok(c));
        }
        for t in ["TNX BOB 73 GL", "CQ", "A+B-C./?", ""] {
            assert_eq!(f71_decode(f71_encode(t)).as_deref(), Ok(t));
        }
        // 42^13 is less than 2^71: the top of the field is unassigned.
        assert!(f71_decode((1u128 << 71) - 1).is_err());
        assert!(c58_decode((1u64 << 58) - 1).is_err());
    }

    #[test]
    fn hashes_match_hashcodes() {
        // Worked out by `hashcodes`'s algorithm, the product wrapping at 64 bits.
        for (call, h10, h12, h22) in [
            ("K1ABC", 712, 2851, 2_920_267),
            ("PJ4/K1ABC", 346, 1387, 1_420_834),
            ("YW18FIFA", 188, 753, 771_524),
            ("KH1/KH7Z", 201, 806, 825_805),
        ] {
            assert_eq!(hashes(call), Some(Hashes { h10, h12, h22 }), "{call}");
        }
        assert_eq!(hashes("K1ABC!"), None);
        assert_eq!(hashes("TWELVECHARSX"), None);
    }
}
