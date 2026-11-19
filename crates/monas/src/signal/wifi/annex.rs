// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The worked example of IEEE 802.11-2024, Annex I.1, and how every stage of
//! [`super`] is held to it.
//!
//! **The annex's tables are not copied here.** The standard is IEEE's
//! copyright, licensed per reader. So each stage is checked two ways:
//!
//! - **In full, where a licensed copy is at hand.** The environment variable
//!   `SDRTOP_ANNEX_I1` names a JSON file of the annex's tables, extracted from
//!   a reader's own copy of the standard; with it set, a stage's output is
//!   compared with its table bit for bit, and the digest of that output is
//!   printed.
//! - **By digest, always.** The digest printed by that full comparison is the
//!   constant the stage is held to everywhere else, CI included: a reference
//!   generated from the code and certified against the annex, not typed from
//!   it (POLICY rule 7).
//!
//! The one table carried whole is the input: the message.

/// The example's PSDU (Annex I.1, Table I-1): a MAC header, the first 72
/// characters of Schiller's "Ode to Joy" with LF line breaks (the typo in
/// "Fire-insired" is the annex's own, and deliberate), and the FCS.
#[rustfmt::skip]
pub const MESSAGE: [u8; 100] = [
    0x04, 0x02, 0x00, 0x2E, 0x00, 0x60, 0x08, 0xCD, 0x37, 0xA6,
    0x00, 0x20, 0xD6, 0x01, 0x3C, 0xF1, 0x00, 0x60, 0x08, 0xAD,
    0x3B, 0xAF, 0x00, 0x00, b'J', b'o', b'y', b',', b' ', b'b',
    b'r', b'i', b'g', b'h', b't', b' ', b's', b'p', b'a', b'r',
    b'k', b' ', b'o', b'f', b' ', b'd', b'i', b'v', b'i', b'n',
    b'i', b't', b'y', b',', b'\n', b'D', b'a', b'u', b'g', b'h',
    b't', b'e', b'r', b' ', b'o', b'f', b' ', b'E', b'l', b'y',
    b's', b'i', b'u', b'm', b',', b'\n', b'F', b'i', b'r', b'e',
    b'-', b'i', b'n', b's', b'i', b'r', b'e', b'd', b' ', b'w',
    b'e', b' ', b't', b'r', b'e', b'a', 0x67, 0x33, 0x21, 0xB6,
];

/// FNV-1a, 64 bits: small, published, and the same on every machine.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// The digest of a bit stream, one byte a bit.
pub fn digest_bits(bits: &[bool]) -> u64 {
    fnv1a(&bits.iter().map(|&b| b as u8).collect::<Vec<u8>>())
}

/// The annex's tables, when `SDRTOP_ANNEX_I1` names a file of them.
pub fn full() -> Option<serde_json::Value> {
    let path = std::env::var_os("SDRTOP_ANNEX_I1")?;
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("SDRTOP_ANNEX_I1 is set but unreadable: {e}"));
    Some(serde_json::from_str(&text).expect("SDRTOP_ANNEX_I1 is not the tables' JSON"))
}

/// Hold a stage's bits to the annex: to `table` in full when a copy is at
/// hand (printing the digest it certifies), and to `digest` always.
pub fn check_bits(table: &str, bits: &[bool], digest: u64) {
    let got = digest_bits(bits);
    if let Some(full) = full() {
        let want: Vec<bool> = full[table]["bits"]
            .as_str()
            .unwrap_or_else(|| panic!("no bits for {table}"))
            .chars()
            .map(|c| c == '1')
            .collect();
        assert_eq!(bits.len(), want.len(), "{table}: length");
        if let Some(i) = (0..want.len()).find(|&i| bits[i] != want[i]) {
            panic!("{table}: bit {i} differs from the annex");
        }
        println!("{table} matches the annex; digest {got:#018x}");
    }
    assert_eq!(got, digest, "{table}: digest {got:#018x}");
}

/// Hold a permutation to the annex the same way: `map[k]` is where input
/// index `k` goes.
pub fn check_map(table: &str, map: &[usize], digest: u64) {
    let bytes: Vec<u8> = map.iter().flat_map(|&i| (i as u32).to_le_bytes()).collect();
    let got = fnv1a(&bytes);
    if let Some(full) = full() {
        let want: Vec<usize> = full[table]["map"]
            .as_array()
            .unwrap_or_else(|| panic!("no map for {table}"))
            .iter()
            .map(|v| v.as_u64().unwrap() as usize)
            .collect();
        assert_eq!(map, want.as_slice(), "{table}");
        println!("{table} matches the annex; digest {got:#018x}");
    }
    assert_eq!(got, digest, "{table}: digest {got:#018x}");
}

/// The digest of complex values, each part rounded to four decimals: one
/// place finer than the annex prints, and far coarser than any difference
/// between two correct computations of the same value.
pub fn digest_values(values: &[num_complex::Complex<f64>]) -> u64 {
    let q = |x: f64| ((x * 1e4).round() as i64).to_le_bytes();
    let bytes: Vec<u8> = values
        .iter()
        .flat_map(|v| [q(v.re), q(v.im)])
        .flatten()
        .collect();
    fnv1a(&bytes)
}

/// Hold complex values to the annex: to `table` in full when a copy is at
/// hand, each part within half a unit of the third decimal it is printed
/// to (a position the annex marks as a pilot's, printed `X`, is skipped),
/// and to `digest` always.
pub fn check_values(table: &str, values: &[num_complex::Complex<f64>], digest: u64) {
    let got = digest_values(values);
    if let Some(full) = full() {
        let want = full[table]["values"]
            .as_array()
            .unwrap_or_else(|| panic!("no values for {table}"));
        assert_eq!(values.len(), want.len(), "{table}: length");
        for (i, (v, w)) in values.iter().zip(want).enumerate() {
            let Some(pair) = w.as_array() else { continue };
            let (re, im) = (pair[0].as_f64().unwrap(), pair[1].as_f64().unwrap());
            let off = (v.re - re).abs().max((v.im - im).abs());
            assert!(
                off <= 0.0005 + 1e-9,
                "{table}: value {i} is {v}, the annex prints {re} {im}"
            );
        }
        println!("{table} matches the annex; digest {got:#018x}");
    }
    assert_eq!(got, digest, "{table}: digest {got:#018x}");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FNV-1a 64's own published check value: the digest of the one byte
    /// `a`. Held here so a slip in the hash cannot pass as a slip in a stage.
    #[test]
    fn the_digest_is_fnv1a_64() {
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
    }

    /// Bits are digested one byte each, so `[true, false]` is the digest of
    /// the bytes 1 and 0.
    #[test]
    fn bits_digest_as_one_byte_each() {
        assert_eq!(digest_bits(&[true, false]), fnv1a(&[1, 0]));
    }

    /// Values are digested at four decimals, so a difference far below the
    /// annex's three cannot change the digest, and a real one does.
    #[test]
    fn values_digest_at_four_decimals() {
        use num_complex::Complex;
        let a = [Complex::new(0.04600001, -0.0230)];
        let b = [Complex::new(0.04599999, -0.0230)];
        let c = [Complex::new(0.0462, -0.0230)];
        assert_eq!(digest_values(&a), digest_values(&b));
        assert_ne!(digest_values(&a), digest_values(&c));
    }

    /// The message is the annex's 100 octets, the poem's 72 characters among
    /// them.
    #[test]
    fn the_message_carries_the_poem() {
        let text = std::str::from_utf8(&MESSAGE[24..96]).unwrap();
        assert!(
            text.starts_with("Joy, bright spark of divinity,\n"),
            "{text}"
        );
        assert!(text.ends_with("Fire-insired we trea"), "{text}");
        if let Some(full) = full() {
            let want: Vec<u8> = full["I-1"]["octets"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_u64().unwrap() as u8)
                .collect();
            assert_eq!(MESSAGE.to_vec(), want);
        }
    }
}
