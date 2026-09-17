// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Streaming access-code detection: the same check
//! [`super::access_code::find_access_code`] makes over a finished slice,
//! made incremental for a live bit stream that never ends and has no
//! boundary to hand it a slice of - B15's own reason to exist.
//!
//! Classic Bluetooth has no known preamble to trigger on the way
//! `signal::ble::detect::Detector` does: design section 1.4 is explicit that
//! the access code itself, keyed by a LAP no passive receiver knows in
//! advance, is the only thing there is to search for. So this runs the check
//! on every bit rather than after a trigger picks out where to look.
//!
//! **`O(1)` per bit, not a re-scan.** [`Detector::push`] keeps the last 64
//! bits packed into one `u64` shift register and checks it after every new
//! bit, rather than re-running [`super::access_code::find_access_code`] over
//! a growing buffer - the same per-candidate-position bound that function's
//! own doc already promises, carried over to a caller that never has a
//! finished slice to hand it.

use super::access_code::check_window;

/// One channel's live shift register: the last 64 bits received, oldest
/// first the way [`super::access_code::access_code_bits`] emits them,
/// checked after every new one.
#[derive(Default, Clone, Copy)]
pub struct Detector {
    /// The last 64 bits, packed the same way
    /// [`super::access_code::access_code_bits`] and
    /// [`super::access_code::gen_syncword`] agree on: bit 0 sent first. A new
    /// bit becomes the new bit 63 (most recently received); the old bit 0
    /// (the oldest) is the one that falls out.
    window: u64,
    /// How many bits have been pushed, capped at 64. Below 64, `window` is
    /// not yet a real 64-bit history and must not be checked - see
    /// [`Detector::push`]'s own doc for why that matters on channel start-up.
    filled: u8,
}

impl Detector {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one more sliced bit, and return the LAP if the last 64 bits now
    /// read as a clean access code.
    ///
    /// **Fewer than 64 bits ever seen answers `None` unconditionally.**
    /// [`super::access_code::gen_syncword`]`(0)` is
    /// [`super::access_code::DEFAULT_CODEWORD`], a real, checkable value - so
    /// the all-zero window this detector starts in would otherwise read as a
    /// free, invented match for LAP zero before a single real bit had
    /// arrived to earn it. Rule 2 refuses that: `filled` exists only to hold
    /// the line until 64 real bits have actually been pushed.
    pub fn push(&mut self, bit: bool) -> Option<u32> {
        self.window = (self.window >> 1) | ((bit as u64) << 63);
        // Counted *after* this bit lands, so the call that brings the count
        // from 63 to 64 - the one where the window first holds a real 64-bit
        // history - is the same call that checks it, rather than the one
        // after. Getting this one off cost the first of two back-to-back
        // codewords its own hit: by the next call the window had already
        // moved on.
        self.filled = self.filled.saturating_add(1);
        if self.filled < 64 {
            return None;
        }
        check_window(self.window)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::bt::access_code::access_code_bits;

    /// B15's own exit condition for this module: a clean access code fed one
    /// bit at a time, with real content ahead of it, is found at the instant
    /// its own last bit arrives - a live receiver's only warning that a
    /// window is complete.
    #[test]
    fn a_clean_access_code_fed_one_bit_at_a_time_is_found_at_its_last_bit() {
        let lap = 0x00c0_ffee;
        let preamble = [true, false, true, false, true, false, true, false];
        let mut det = Detector::new();
        let mut hits = Vec::new();
        for (i, b) in preamble
            .iter()
            .chain(access_code_bits(lap).iter())
            .enumerate()
        {
            if let Some(found) = det.push(*b) {
                hits.push((i, found));
            }
        }
        assert_eq!(hits, vec![(preamble.len() + 64 - 1, lap)]);
    }

    /// The first 63 bits of a real, valid codeword is not yet a 64-bit
    /// history - the window holds real content, but not all of it yet, and
    /// checking it early would either miss the match or, worse, find a
    /// different, accidental one.
    #[test]
    fn fewer_than_sixty_four_bits_of_a_real_codeword_never_matches_early() {
        let mut det = Detector::new();
        let bits = access_code_bits(0); // LAP zero's own real access code
        for &b in &bits[..63] {
            assert_eq!(det.push(b), None);
        }
    }

    /// The sixty-fourth bit of a real codeword completes the window and is
    /// found on the very call that completes it - not the call after, which
    /// is exactly the off-by-one `two_access_codes_back_to_back_are_both_
    /// found` below caught: the window has already moved on by then.
    #[test]
    fn the_sixty_fourth_bit_of_a_real_codeword_completes_the_match() {
        let mut det = Detector::new();
        let bits = access_code_bits(0);
        let mut last = None;
        for &b in &bits {
            last = det.push(b);
        }
        assert_eq!(last, Some(0));
    }

    /// The same noise-immunity discipline `access_code`'s own
    /// `noise_alone_does_not_manufacture_an_access_code` holds
    /// `find_access_code` to, run through the streaming path instead: a
    /// fixed, non-repeating pattern with no reason to line up with any LAP's
    /// codeword should not manufacture a hit.
    #[test]
    fn noise_alone_does_not_manufacture_a_hit() {
        let mut det = Detector::new();
        let mut hits = 0;
        for i in 0..5_000u32 {
            let bit = (i.wrapping_mul(2_654_435_761)) & 1 == 1;
            if det.push(bit).is_some() {
                hits += 1;
            }
        }
        assert_eq!(hits, 0, "{hits} spurious hits over 5000 pseudo-random bits");
    }

    /// Two distinct access codes, back to back with nothing between them,
    /// are both found - the detector does not need to be reset or rebuilt
    /// between packets, which is exactly what "streaming" has to mean for a
    /// channel with no fixed packet boundary known in advance.
    #[test]
    fn two_access_codes_back_to_back_are_both_found() {
        let (lap_a, lap_b) = (0x0011_2233, 0x00aa_bb00);
        let mut det = Detector::new();
        let mut hits = Vec::new();
        for b in access_code_bits(lap_a)
            .iter()
            .chain(access_code_bits(lap_b).iter())
        {
            if let Some(found) = det.push(*b) {
                hits.push(found);
            }
        }
        assert_eq!(hits, vec![lap_a, lap_b]);
    }
}
