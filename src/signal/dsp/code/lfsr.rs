// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Bit-stream whitening: the 7-bit LFSR every Bluetooth Low Energy packet is
//! scrambled with, seeded from the channel it is sent on.
//!
//! Source: Bluetooth Core Specification, Vol 6, Part B - polynomial
//! `x^7 + x^4 + 1`, seeded from the channel index. Verified two separate
//! ways rather than trusted on one citation:
//!
//! - [`seed`]'s formula is pinned against a worked numeric example found in
//!   independent public documentation: channel `0x25` seeds to `0x65`. See
//!   `the_seed_matches_its_own_worked_example`.
//! - The feedback taps are pinned by their own mathematical signature, not
//!   by citation alone. Several different tap pairs on a 7-bit register give
//!   a maximal-length (127-state) sequence, and a maximal length alone does
//!   not say *which* primitive polynomial produced it - so
//!   `the_output_satisfies_its_own_recurrence` checks that this sequence
//!   obeys the specific linear recurrence `x^7 + x^4 + 1` implies,
//!   `o[t] = o[t-3] xor o[t-7]`, which only the correct tap pair can satisfy.
//!   Found by brute-force search over every tap pair during this step's own
//!   development, not assumed from the first plausible-looking pair.

/// The whitening LFSR's initial state for `channel`, 0 to 39.
///
/// Bit 6 is always set; bits 0 to 5 are the channel index. `channel` values
/// of 64 and above have no meaning here and are masked rather than rejected,
/// since whitening has no way to refuse an out-of-range channel of its own -
/// the channel plan in `signal::ble::channel` is what enforces 0 to 39.
///
/// No consumer yet outside this module's own tests: nothing de-whitens a
/// real payload until B6 puts a real packet on screen. Applies to every item
/// below.
#[allow(dead_code)]
pub fn seed(channel: u8) -> u8 {
    0x40 | (channel & 0x3F)
}

/// One step: the bit whitened out, and the LFSR's next state.
#[allow(dead_code)]
fn step(state: u8) -> (bool, u8) {
    let out = state & 1 != 0;
    let feedback = ((state >> 4) ^ state) & 1;
    let next = (state >> 1) | (feedback << 6);
    (out, next)
}

/// Whiten `bits` in place, using the sequence [`seed`] generates for
/// `channel`.
///
/// **De-whitening is the same operation, not a second one.** The sequence
/// depends only on the channel, never on the data, so applying this twice
/// with the same channel is the identity - whitening and de-whitening are
/// the same XOR run once each way, which is why there is one function here
/// rather than two that would have to agree.
#[allow(dead_code)]
pub fn whiten(bits: &mut [bool], channel: u8) {
    let mut state = seed(channel);
    for bit in bits.iter_mut() {
        let (out, next) = step(state);
        *bit ^= out;
        state = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one worked example public documentation states directly.
    #[test]
    fn the_seed_matches_its_own_worked_example() {
        assert_eq!(seed(0x25), 0x65);
    }

    /// Bit 6 is always the constant, and the low six bits are always the
    /// channel, for every channel this arc actually uses.
    #[test]
    fn the_seed_carries_the_channel_in_its_low_six_bits() {
        for channel in 0..=39u8 {
            let s = seed(channel);
            assert_eq!(s & 0x40, 0x40, "channel {channel}: bit 6 not set");
            assert_eq!(s & 0x3F, channel, "channel {channel}: low bits wrong");
        }
    }

    /// A 7-bit LFSR with the right primitive polynomial visits all 127
    /// nonzero states before returning to its start. Several tap pairs on
    /// this register give *some* maximal sequence; this alone does not yet
    /// say it is the *right* one - `the_output_satisfies_its_own_recurrence`
    /// is what pins that down.
    #[test]
    fn the_lfsr_is_maximal_length() {
        let start = seed(0);
        let mut state = start;
        let mut seen = std::collections::HashSet::new();
        for _ in 0..127 {
            assert!(seen.insert(state), "state {state:#x} repeated early");
            let (_, next) = step(state);
            state = next;
        }
        assert_eq!(state, start, "127 steps did not return to the start");
    }

    /// The signature of `x^7 + x^4 + 1` specifically, not just of some
    /// maximal-length polynomial: the output sequence obeys
    /// `o[t] = o[t-3] xor o[t-7]` for every `t` past the seventh output.
    #[test]
    fn the_output_satisfies_its_own_recurrence() {
        let mut state = seed(17);
        let out: Vec<bool> = (0..60)
            .map(|_| {
                let (o, next) = step(state);
                state = next;
                o
            })
            .collect();
        for t in 7..out.len() {
            assert_eq!(
                out[t],
                out[t - 3] ^ out[t - 7],
                "recurrence broken at t={t}"
            );
        }
    }

    /// Whitening twice with the same channel is the identity - the property
    /// that makes de-whitening the same function as whitening.
    #[test]
    fn whitening_twice_with_the_same_channel_is_the_identity() {
        let original = [
            true, true, false, true, false, false, false, true, true, false, true, true, false,
            false, true, false,
        ];
        let mut bits = original;
        whiten(&mut bits, 6);
        assert_ne!(bits, original, "whitening did nothing");
        whiten(&mut bits, 6);
        assert_eq!(bits, original, "whitening twice did not return the input");
    }

    /// Two different channels really do scramble differently - a whitening
    /// step that ignored the channel would still pass every test above.
    #[test]
    fn different_channels_whiten_differently() {
        let data = [true; 32];
        let mut a = data;
        let mut b = data;
        whiten(&mut a, 0);
        whiten(&mut b, 37);
        assert_ne!(a, b);
    }
}
