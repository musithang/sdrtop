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
//! - The feedback taps are pinned by their own mathematical signature:
//!   `the_output_satisfies_its_own_recurrence` checks the sequence obeys the
//!   recurrence `x^7 + x^4 + 1` implies, `o[t] = o[t-3] xor o[t-7]`.
//! - **And the structure is pinned by real packets, which is the check that
//!   matters.** The register is the specification's own figure: positions 0
//!   to 6, the output taken from position 6 and fed back into position 0, and
//!   an exclusive-or on the way into position 4 (a Galois register). Until
//!   2026-09-18 this file stepped a *Fibonacci* register instead - feedback
//!   from positions 2 and 6 into position 0 - found by searching tap pairs
//!   for one that passed the two tests above. It passed both: a Fibonacci and
//!   a Galois register with the same polynomial produce the same sequence,
//!   only from a different point in it for the same starting contents. The
//!   specification gives the starting contents for the Galois register, so
//!   the Fibonacci one whitened with the right sequence at the wrong phase:
//!   the first few bits agreed, everything after them did not, and not one
//!   real packet ever passed its CRC while every synthetic one, whitened by
//!   this same function on the way out, did. The maximal-length and
//!   recurrence tests cannot tell the two structures apart; bits from a real
//!   transmitter can, and `signal::ble::pdu`'s
//!   `a_real_over_the_air_packet_dewhitens_to_a_clean_crc` holds this file
//!   to them.
//!
//! **The mapping from the specification's positions to this `u8`**: bit `j`
//! holds position `6 - j`. So bit 0 is position 6, the output; bit 6 is
//! position 0, which the specification sets to one; and bits 5 down to 0
//! hold positions 1 to 6, which it sets to the channel index most
//! significant bit first - which is the channel index itself, unshifted.

/// The whitening LFSR's initial state for `channel`, 0 to 39.
///
/// Bit 6 is always set; bits 0 to 5 are the channel index. `channel` values
/// of 64 and above have no meaning here and are masked rather than rejected,
/// since whitening has no way to refuse an out-of-range channel of its own -
/// the channel plan in `signal::ble::channel` is what enforces 0 to 39.
///
/// See the module doc for why bit 6 and the unshifted channel are exactly the
/// specification's positions 0 to 6.
pub fn seed(channel: u8) -> u8 {
    0x40 | (channel & 0x3F)
}

/// One step: the bit whitened out, and the LFSR's next state.
///
/// Every position moves one along (bit `j` to bit `j - 1`), the output goes
/// round into position 0 (bit 6), and on its way into position 4 (bit 2) the
/// bit from position 3 is exclusive-ored with the output - the
/// specification's figure, in the module doc's mapping.
fn step(state: u8) -> (bool, u8) {
    let out = state & 1;
    let next = ((state >> 1) | (out << 6)) ^ (out << 2);
    (out != 0, next)
}

/// Whiten `bits` in place, using the sequence [`seed`] generates for
/// `channel`.
///
/// **De-whitening is the same operation, not a second one.** The sequence
/// depends only on the channel, never on the data, so applying this twice
/// with the same channel is the identity - whitening and de-whitening are
/// the same XOR run once each way, which is why there is one function here
/// rather than two that would have to agree.
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
