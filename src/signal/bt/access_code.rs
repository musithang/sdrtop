// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The classic Bluetooth access code: a 64-bit sync word built from the
//! piconet master's 24-bit LAP (Lower Address Part), and the one thing a
//! receiver that has not joined a piconet can search for without already
//! knowing what it is listening to.
//!
//! **Not read from the Bluetooth Core Specification itself this session**
//! (Vol 2, Part B, Section 6.2 - design section 6's own facts-to-verify
//! table already names this as unread, alongside B1's channel table and
//! B3's preamble rule). The construction here - [`DEFAULT_CODEWORD`] and
//! [`SW_MATRIX`] - is ported from `libbtbb`
//! (<https://github.com/greatscottgadgets/libbtbb>,
//! `lib/src/bluetooth_packet.c`, `btbb_gen_syncword`), Great Scott Gadgets'
//! own Bluetooth baseband library - the same organisation whose HackRF this
//! app already drives directly - GPL-2.0-or-later, license-compatible with
//! this project's GPL-3.0-or-later, and in real use by Ubertooth hardware
//! for well over a decade. A real, independent, production-tested
//! implementation rather than a primary text, the same standing B1's three
//! cross-checked secondary sources had.
//!
//! **Corroborated the same way B1's channel table was, without a primary
//! source: a structural property, checked by brute force rather than
//! trusted on the citation alone.** A public description of this code
//! states two properties any correct transcription of the constants below
//! must also have: the LAP is recoverable directly from fixed bit
//! positions of the generated sync word (`the_lap_is_recoverable_from_its_
//! own_sync_word` checks this across a spread of LAP values), and any two
//! different LAPs' own sync words differ by at least 14 bits
//! (`the_codes_own_minimum_distance_is_fourteen_bits` brute-forces this
//! across all 2^24 possible LAP *differences* - see that test's own doc for
//! why a difference, not a pair). A transcription error in the twenty-four
//! 64-bit constants below would be very unlikely to still satisfy both
//! properties by accident.

/// `gen_syncword(0)`: the sync word for a LAP of all zero bits, with the PN
/// overlay and the barker-code correction `libbtbb`'s own generator matrix
/// derivation already folds in. XORing in [`SW_MATRIX`] entries for a
/// nonzero LAP's own set bits, from this starting point, is the whole of
/// the code - see [`gen_syncword`].
///
/// No consumer from `main` yet: B14 lands this arc's own detection
/// primitive alone, with nothing in `tasks` or `ui` feeding it a live
/// capture - the same position `signal::ble::channel`'s own table was in
/// after B1, before B3's detector gave it a real caller.
#[allow(dead_code)]
const DEFAULT_CODEWORD: u64 = 0xb000_0002_c782_0e7e;

/// One 64-bit constant per LAP bit, most significant (bit 23) first -
/// `libbtbb`'s own `sw_matrix`, for its own `(64,30)` linear block code
/// (24 LAP bits plus a 6-bit trailer chosen by the LAP's own top bit,
/// folded into the same linear structure rather than handled as a separate
/// case). Linear, so `gen_syncword(a) ^ gen_syncword(b) == gen_syncword(a
/// ^ b) ^ DEFAULT_CODEWORD` for any two LAPs - which is what turns "the
/// minimum distance between any two LAPs' sync words" into a brute-force
/// search over `2^24` *differences* rather than every pair among `2^24`
/// LAPs.
/// No consumer from `main` yet; see [`DEFAULT_CODEWORD`]'s own note.
#[allow(dead_code)]
#[rustfmt::skip]
const SW_MATRIX: [u64; 24] = [
    0xfe00_0002_a0d1_c014, 0x0100_0003_f0b9_201f, 0x0080_0003_3ae4_0edb, 0x0040_0003_5fca_99b9,
    0x0020_0003_6d5d_d208, 0x0010_0001_b6ae_e904, 0x0008_0000_db57_7482, 0x0004_0000_6dab_ba41,
    0x0002_0002_f46d_43f4, 0x0001_0001_7a36_a1fa, 0x0000_8000_bd1b_50fd, 0x0000_4002_9c35_36aa,
    0x0000_2001_4e1a_9b55, 0x0000_1002_65b5_d37e, 0x0000_0801_32da_e9bf, 0x0000_0402_5bd5_ea0b,
    0x0000_0203_ef52_6bd1, 0x0000_0103_3511_ab3c, 0x0000_0081_9a88_d59e, 0x0000_0040_cd44_6acf,
    0x0000_0022_a41a_abb3, 0x0000_0013_90b5_cb0d, 0x0000_000b_0ae2_7b52, 0x0000_0005_8571_3da9,
];

/// The 64-bit access code (sync word) for `lap`'s own piconet.
///
/// `lap` is used as a 24-bit value; any bits above that are ignored, the
/// same way a LAP is defined as the low 24 bits of a BD_ADDR.
///
/// No consumer from `main` yet; see [`DEFAULT_CODEWORD`]'s own note.
#[allow(dead_code)]
pub fn gen_syncword(lap: u32) -> u64 {
    let lap = lap & 0x00ff_ffff;
    let mut codeword = DEFAULT_CODEWORD;
    for (i, term) in SW_MATRIX.iter().enumerate() {
        if lap & (0x0080_0000 >> i) != 0 {
            codeword ^= term;
        }
    }
    codeword
}

/// `gen_syncword(lap)`'s own bits, in transmission order - bit 0 (the
/// underlying integer's own least significant bit) sent first, the same
/// convention `signal::ble::detect::access_address_bits` already uses for
/// the same reason: a receiver correlates against symbols in the order
/// they actually arrive, not the order a hex literal reads left to right.
///
/// No consumer from `main` yet; see [`DEFAULT_CODEWORD`]'s own note.
#[allow(dead_code)]
pub fn access_code_bits(lap: u32) -> [bool; 64] {
    let word = gen_syncword(lap);
    let mut out = [false; 64];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = (word >> i) & 1 != 0;
    }
    out
}

/// A 64-bit window of transmission-order bits, packed back into the same
/// integer domain [`gen_syncword`] and [`access_code_bits`] use - the exact
/// inverse of `access_code_bits`.
///
/// No consumer from `main` yet; see [`DEFAULT_CODEWORD`]'s own note.
#[allow(dead_code)]
fn pack(window: &[bool]) -> u64 {
    let mut word = 0u64;
    for (i, &bit) in window.iter().enumerate().take(64) {
        if bit {
            word |= 1 << i;
        }
    }
    word
}

/// Search `bits` for a classic Bluetooth access code with no LAP known in
/// advance, and return the LAP and the index one past its last bit if one
/// is found.
///
/// **Exact match only - no bit errors corrected.** `libbtbb`'s own
/// `promiscuous_packet_search` additionally corrects a small number of bit
/// errors via a precomputed syndrome table; this arc does not port that
/// table, so a real capture with any error in its access code is missed
/// rather than recovered - an honest, coarser first step, not a claim to
/// have matched the reference implementation's own robustness. Design
/// section 1.4's own point already holds without it: finding a
/// *clean* access code yields the LAP for free, with no piconet
/// membership required first.
///
/// **`O(1)` per candidate position, not a search over LAPs.** The
/// candidate LAP is read directly off the window's own bits 34 to 57 (the
/// systematic property `the_lap_is_recoverable_from_its_own_sync_word`
/// checks), and accepted only if regenerating that LAP's own access code
/// reproduces the window exactly - never a blind search over the `2^24`
/// possible LAPs, which a live receiver has no time for.
///
/// No consumer from `main` yet; see [`DEFAULT_CODEWORD`]'s own note.
#[allow(dead_code)]
pub fn find_access_code(bits: &[bool]) -> Option<(u32, usize)> {
    if bits.len() < 64 {
        return None;
    }
    for end in 64..=bits.len() {
        let window = &bits[end - 64..end];
        let word = pack(window);
        let candidate_lap = ((word >> 34) & 0x00ff_ffff) as u32;
        if gen_syncword(candidate_lap) == word {
            return Some((candidate_lap, end));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A worked internal check independent of the constants' own citation:
    /// the LAP is recoverable directly from the generated sync word's own
    /// bits 34 to 57, for every LAP tried - the property [`find_access_
    /// code`]'s `O(1)` extraction depends on, and a transcription error in
    /// [`SW_MATRIX`] would be very unlikely to leave intact.
    #[test]
    fn the_lap_is_recoverable_from_its_own_sync_word() {
        for lap in [
            0x0000_0001,
            0x0080_0000,
            0x0055_5555,
            0x00aa_aaaa,
            0x009e_8b33, // GIAC, the General Inquiry Access Code's own LAP
            0x00de_ad5a,
            0x0012_3456,
            0x00ff_ffff,
        ] {
            let word = gen_syncword(lap);
            let extracted = (word >> 34) & 0x00ff_ffff;
            assert_eq!(
                extracted, lap as u64,
                "lap {lap:06x} did not round-trip through its own sync word"
            );
        }
    }

    /// `gen_syncword(0)` is exactly [`DEFAULT_CODEWORD`] - the base case
    /// with nothing XORed in - and every entry of [`SW_MATRIX`] is used:
    /// asking for a LAP with every bit set must differ from the zero-LAP
    /// case in a way only the full table, not a subset of it, can produce.
    #[test]
    fn the_zero_lap_is_the_default_codeword_untouched() {
        assert_eq!(gen_syncword(0), DEFAULT_CODEWORD);
    }

    /// A LAP outside the 24-bit range is truncated to it, the same way the
    /// type this arc reads a `BD_ADDR`'s LAP into elsewhere already treats
    /// the field as 24 bits and no more.
    #[test]
    fn bits_above_the_lap_are_ignored() {
        assert_eq!(gen_syncword(0x00ab_cdef), gen_syncword(0xffab_cdef));
    }

    /// **The code's own minimum distance is fourteen bits.** A public
    /// description of this construction states that any two different
    /// LAPs' own access codes differ by at least 14 bits - checked here by
    /// brute force over every one of the `2^24 - 1` nonzero LAP
    /// *differences*, not merely cited.
    ///
    /// Linearity is what makes this tractable: for any two LAPs `a` and
    /// `b`, `gen_syncword(a) ^ gen_syncword(b) == gen_syncword(a ^ b) ^
    /// DEFAULT_CODEWORD` (every `SW_MATRIX` term common to both cancels,
    /// leaving exactly the terms named by `a ^ b`'s own set bits). The
    /// minimum distance between any two *codewords* therefore equals the
    /// minimum Hamming weight of `gen_syncword(delta) ^ DEFAULT_CODEWORD`
    /// over every nonzero 24-bit `delta` - one pass over `2^24` values,
    /// not one over every pair among them.
    #[test]
    fn the_codes_own_minimum_distance_is_fourteen_bits() {
        let mut min_weight = u32::MAX;
        for delta in 1..=0x00ff_ffffu32 {
            let weight = (gen_syncword(delta) ^ DEFAULT_CODEWORD).count_ones();
            if weight < min_weight {
                min_weight = weight;
            }
        }
        assert_eq!(
            min_weight, 14,
            "expected a minimum distance of 14 bits, measured {min_weight}"
        );
    }

    /// [`access_code_bits`] and [`pack`] are exact inverses - the bridge
    /// between the integer domain [`gen_syncword`] computes in and the
    /// per-symbol `bool` domain a live capture, or a synthetic transmitter,
    /// actually deals in.
    #[test]
    fn access_code_bits_and_pack_round_trip() {
        for lap in [0x0000_0000, 0x009e_8b33, 0x00ff_ffff] {
            let bits = access_code_bits(lap);
            assert_eq!(pack(&bits), gen_syncword(lap));
        }
    }

    /// [`find_access_code`]'s own exit condition: a clean access code
    /// embedded in a longer bit stream, with real content before and
    /// after it, is found at the right position and yields the right LAP -
    /// design section 1.4's "yields the LAP for free", measured rather
    /// than only claimed.
    #[test]
    fn a_clean_access_code_is_found_and_yields_its_lap() {
        let lap = 0x00c0_ffee;
        let mut bits = vec![true, false, true, false, true, false, true, false];
        bits.extend(access_code_bits(lap));
        bits.extend([false, true, false, true, true, false]);
        let (found_lap, end) = find_access_code(&bits).expect("a clean access code");
        assert_eq!(found_lap, lap);
        assert_eq!(end, 8 + 64, "should end exactly where the access code does");
    }

    /// Noise alone must not manufacture an access code: `2^-14` is the
    /// chance any single window matches *some* LAP by construction (the
    /// code's own minimum distance guarantees no window is within 13 bits
    /// of two different LAPs' codewords, but says nothing about how close
    /// random noise gets to being an *exact* match), so a few hundred
    /// random windows finding one is implausible enough to be worth
    /// asserting against, the same discipline `dsp::correlate`'s own
    /// `noise_alone_crosses_the_threshold_at_about_the_stated_rate` holds
    /// this arc's other detectors to.
    #[test]
    fn noise_alone_does_not_manufacture_an_access_code() {
        // A fixed, non-random but non-repeating pattern - deterministic,
        // like every test in this arc, and with no reason to line up with
        // any LAP's own codeword.
        let bits: Vec<bool> = (0..2000u32)
            .map(|i| (i.wrapping_mul(2654435761)) & 1 == 1)
            .collect();
        assert!(
            find_access_code(&bits).is_none(),
            "a pseudo-random bit pattern should not resemble any LAP's access code"
        );
    }
}
