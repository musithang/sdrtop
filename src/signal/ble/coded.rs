// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! LE Coded PHY's own forward error correction: B18, "the K=4 FEC and the
//! S=8 pattern mapper" (the master step table's own words). A rate-1/2,
//! constraint-length-4 convolutional code, and the "pattern mapper" that
//! further expands each coded bit for the S=8 coding scheme (S=2 leaves it
//! alone) - together the reason LE Coded reaches roughly four times LE
//! 1M's own range at roughly an eighth its data rate.
//!
//! **Read directly from the primary source this session - the Bluetooth
//! SIG's own public Core Specification, not a secondary account of it,**
//! unlike most of this arc's own citations (design section 6's own facts-
//! to-verify table drops its LE Coded row because of this). Core-54,
//! Volume 6 (Low Energy Controller), Part B (Link Layer Specification):
//! sections 2.2 through 2.2.3 (packet structure - [`PREAMBLE_SYMBOL`],
//! the FEC block split, the Coding Indicator) and 3.3.1/3.3.2 (the
//! encoder and the pattern mapper themselves), fetched from
//! <https://www.bluetooth.com/wp-content/uploads/Files/Specification/HTML/Core-54/out/en/low-energy-controller/link-layer-specification.html>.
//!
//! **Primitives only - the same honest scope every large piece of this
//! arc has landed with first.** This module encodes and decodes a bare
//! bit stream; it does not yet know a packet's own preamble, Access
//! Address, Coding Indicator or CRC, and nothing in `signal::ble::receive`
//! calls it. Wiring an actual LE Coded live receiver - detecting a
//! preamble and Access Address that are themselves FEC-encoded (256 air
//! symbols for the 32-bit Access Address alone, not the 32 raw symbols LE
//! 1M/2M correlate against), reading the Coding Indicator to learn which
//! scheme FEC block 2 uses, and running this decoder on the result - is
//! real remaining work, on the scale of B6's own first receiver, not
//! assumed done here.

/// The 8-symbol unit the Coded PHY's own preamble repeats ten times -
/// section 2.2.1's own exact words: "10 repetitions of the symbol pattern
/// '00111100' (in transmission order)". Deliberately not the LE 1M/2M
/// alternating pattern [`super::detect::preamble_bits`] computes: this PHY's
/// own preamble is a fixed constant, unrelated to the Access Address that
/// follows it (which, unlike 1M/2M, is itself FEC-encoded before
/// transmission - a real, structural difference this module's own doc
/// names but does not yet build a detector for).
#[allow(dead_code)]
pub const PREAMBLE_SYMBOL: [bool; 8] = [false, false, true, true, true, true, false, false];

/// The Coded PHY's own full preamble: [`PREAMBLE_SYMBOL`] repeated ten
/// times, eighty symbols in total - section 2.2.1's own stated length.
#[allow(dead_code)]
pub fn preamble_bits() -> [bool; 80] {
    let mut out = [false; 80];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = PREAMBLE_SYMBOL[i % 8];
    }
    out
}

/// The convolutional FEC encoder's own memory: the three most recently
/// encoded input bits, most recent first - `state[0]` is what section
/// 3.3.1's own generator polynomials call the `x` term, `state[1]` the
/// `x^2` term, `state[2]` the `x^3` term. Eight possible values, since
/// three bits of memory - the number [`decode`]'s own trellis has exactly
/// that many states for.
#[allow(dead_code)]
type State = [bool; 3];

#[allow(dead_code)]
const STATE_COUNT: usize = 8;

#[allow(dead_code)]
fn state_to_index(s: State) -> usize {
    ((s[0] as usize) << 2) | ((s[1] as usize) << 1) | (s[2] as usize)
}

#[allow(dead_code)]
fn index_to_state(i: usize) -> State {
    [(i >> 2) & 1 != 0, (i >> 1) & 1 != 0, i & 1 != 0]
}

/// One encoder step: given the state *before* this input bit, the two
/// coded output bits and the state *after* it.
///
/// **Exactly section 3.3.1's own two generator polynomials, not a second,
/// independently-derived pair.** `G0(x) = 1 + x + x^2 + x^3` reads every
/// one of the four taps (the input bit itself, `x^0`, plus all three
/// stored ones); `G1(x) = 1 + x^2 + x^3` skips the `x^1` tap - the only
/// difference between the two. "The bit coming from generator polynomial
/// `G0` (`a0`) is transmitted first; the bit coming from generator
/// polynomial `G1` (`a1`) is transmitted second" is section 3.3.1's own
/// sentence, and is exactly the order returned here and consumed by
/// [`encode`].
#[allow(dead_code)]
fn step(state: State, input: bool) -> (bool, bool, State) {
    let taps = [input, state[0], state[1], state[2]];
    let g0 = taps[0] ^ taps[1] ^ taps[2] ^ taps[3];
    let g1 = taps[0] ^ taps[2] ^ taps[3];
    let next = [input, state[0], state[1]];
    (g0, g1, next)
}

/// Encode `bits` with the rate-1/2, constraint-length-4 convolutional code,
/// starting from the all-zero state section 3.3.1 itself specifies
/// ("initial state... is set to all zeros"). Two coded bits out for every
/// one in, `a0` then `a1` per [`step`]'s own doc.
///
/// **Does not append a termination sequence.** "An input sequence of three
/// consecutive zeros always brings the... encoder back to its original
/// state" (section 3.3.1) is a property of *any* three zero bits fed
/// through, not something this function adds on its own behalf - a caller
/// building a real FEC block appends its own three zero bits (TERM1 or
/// TERM2, section 2.2) before calling this, the same way it is the
/// caller's job to know a block even needs one.
#[allow(dead_code)]
pub fn encode(bits: &[bool]) -> Vec<bool> {
    let mut state: State = [false; 3];
    let mut out = Vec::with_capacity(bits.len() * 2);
    for &b in bits {
        let (g0, g1, next) = step(state, b);
        out.push(g0);
        out.push(g1);
        state = next;
    }
    out
}

/// Decode a rate-1/2, constraint-length-4 convolutionally-encoded stream
/// via hard-decision Viterbi - the maximum-likelihood input sequence,
/// found rather than guessed at a single position, which is the whole
/// reason this code corrects real bit errors instead of only detecting
/// their presence.
///
/// **Assumes the encoder ended in its own all-zero state, because every
/// block this PHY defines (FEC block 1's Access Address/CI/TERM1, FEC
/// block 2's PDU/CRC/TERM2) always does** - each one's own trailing three
/// bits are a termination sequence for exactly this reason (section 2.2).
/// Only paths ending at state zero are ever considered; this is what lets
/// a handful of real bit errors be corrected rather than merely
/// distributed across an otherwise-ambiguous final state.
///
/// `None` only when `coded` cannot possibly be a real codeword: an odd
/// length (this code has no puncturing, so every real one is even) or
/// empty. A structurally valid stream always decodes to *something* - the
/// all-zero path from state zero to state zero always exists as a
/// candidate, at whatever Hamming cost the real errors give it - so this
/// never refuses on account of the errors themselves, only on a stream
/// that could not have come from this encoder at all.
#[allow(dead_code)]
pub fn decode(coded: &[bool]) -> Option<Vec<bool>> {
    if coded.is_empty() || !coded.len().is_multiple_of(2) {
        return None;
    }
    let steps = coded.len() / 2;

    const UNREACHED: u32 = u32::MAX;
    let mut metric = [UNREACHED; STATE_COUNT];
    metric[0] = 0;
    let mut back: Vec<[Option<(usize, bool)>; STATE_COUNT]> = Vec::with_capacity(steps);

    for k in 0..steps {
        let r0 = coded[2 * k];
        let r1 = coded[2 * k + 1];
        let mut next_metric = [UNREACHED; STATE_COUNT];
        let mut step_back: [Option<(usize, bool)>; STATE_COUNT] = [None; STATE_COUNT];
        for (s, &m) in metric.iter().enumerate() {
            if m == UNREACHED {
                continue;
            }
            for input in [false, true] {
                let (g0, g1, next) = step(index_to_state(s), input);
                let dist = u32::from(g0 != r0) + u32::from(g1 != r1);
                let candidate = m + dist;
                let next_idx = state_to_index(next);
                if candidate < next_metric[next_idx] {
                    next_metric[next_idx] = candidate;
                    step_back[next_idx] = Some((s, input));
                }
            }
        }
        metric = next_metric;
        back.push(step_back);
    }

    // The all-zero-input path from state 0 always reaches state 0 again,
    // at whatever cost the real received bits give it, so this is never
    // actually `UNREACHED` - checked rather than assumed, since an
    // `.unwrap()` here would be a claim about the trellis this function
    // does not otherwise need to prove to itself.
    if metric[0] == UNREACHED {
        return None;
    }

    let mut bits = vec![false; steps];
    let mut state = 0usize;
    for k in (0..steps).rev() {
        let (prev, input) = back[k][state]?;
        bits[k] = input;
        state = prev;
    }
    Some(bits)
}

/// The S=8 coding scheme's own expansion of one coded bit into four
/// symbols - Table 3.1's own two rows, "in transmission order": a 0
/// becomes `0011`, a 1 becomes `1100`. S=2 has no equivalent function
/// here because Table 3.1's own P=1 row is the identity - one input bit,
/// one output symbol, unchanged.
#[allow(dead_code)]
pub fn pattern_map_s8(bit: bool) -> [bool; 4] {
    if bit {
        [true, true, false, false]
    } else {
        [false, false, true, true]
    }
}

/// The inverse of [`pattern_map_s8`], by minimum Hamming distance rather
/// than majority vote - `0011` and `1100` both carry exactly two set bits,
/// so counting them cannot tell the two apart at all; only *which*
/// positions are set does. A tie (distance 2 to both, the worst two
/// symbols wrong) breaks toward `true`, arbitrarily - documented rather
/// than silently one way, since nothing in the cited table says which way
/// a tie should fall.
#[allow(dead_code)]
pub fn pattern_demap_s8(symbols: [bool; 4]) -> bool {
    const ZERO: [bool; 4] = [false, false, true, true];
    const ONE: [bool; 4] = [true, true, false, false];
    let dist_zero = symbols
        .iter()
        .zip(ZERO.iter())
        .filter(|(a, b)| a != b)
        .count();
    let dist_one = symbols
        .iter()
        .zip(ONE.iter())
        .filter(|(a, b)| a != b)
        .count();
    dist_one <= dist_zero
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::dsp::testkit::Rng;

    /// Section 2.2.1's own exact preamble pattern and length, checked
    /// directly rather than trusted from the constant's own name.
    #[test]
    fn the_preamble_is_ten_repetitions_of_the_cited_pattern() {
        let bits = preamble_bits();
        assert_eq!(bits.len(), 80);
        for chunk in bits.chunks(8) {
            assert_eq!(chunk, PREAMBLE_SYMBOL);
        }
    }

    /// A hand-worked example directly from section 3.3.1's own two
    /// generator polynomials, not a second, independently-derived
    /// formula: starting from the all-zero state, one input bit `1`
    /// gives `a0 = 1 XOR 0 XOR 0 XOR 0 = 1` (every tap of `G0` reads, and
    /// only the input tap is set) and `a1 = 1 XOR 0 XOR 0 = 1` (`G1`
    /// skips the `x` tap, which is the only one that would have differed
    /// here anyway). A second input bit `0` right after gives
    /// `a0 = 0 XOR 1 XOR 0 XOR 0 = 1` (the previous input is now the `x`
    /// tap) and `a1 = 0 XOR 0 XOR 0 = 0` (`G1` never reads the `x` tap at
    /// all).
    #[test]
    fn the_encoder_matches_a_hand_worked_example_from_the_cited_polynomials() {
        let coded = encode(&[true, false]);
        assert_eq!(coded, vec![true, true, true, false]);
    }

    /// [`decode`]'s own exit condition with no errors at all: encoding
    /// then decoding recovers exactly the bits that went in, across many
    /// random sequences each properly terminated (three zero bits, this
    /// arc's own convention for every real FEC block), not just the one
    /// hand-worked example above.
    #[test]
    fn encoding_then_decoding_with_no_errors_recovers_the_input() {
        let mut rng = Rng::new(11);
        for _ in 0..50 {
            let len = 4 + (rng.next_u64() % 60) as usize;
            let mut bits: Vec<bool> = (0..len).map(|_| rng.next_u64() & 1 == 1).collect();
            bits.extend([false, false, false]); // termination
            let coded = encode(&bits);
            assert_eq!(decode(&coded), Some(bits));
        }
    }

    /// The actual reason a convolutional code exists: a real bit error in
    /// the coded stream - not just noise the round trip above never sees -
    /// is still corrected, measured directly rather than asserted from a
    /// textbook distance figure this session never looked up.
    #[test]
    fn a_single_coded_bit_error_is_always_corrected() {
        let mut rng = Rng::new(22);
        for _ in 0..50 {
            let len = 8 + (rng.next_u64() % 60) as usize;
            let mut bits: Vec<bool> = (0..len).map(|_| rng.next_u64() & 1 == 1).collect();
            bits.extend([false, false, false]);
            let mut coded = encode(&bits);
            let flip = (rng.next_u64() % coded.len() as u64) as usize;
            coded[flip] = !coded[flip];
            assert_eq!(decode(&coded), Some(bits), "flipped bit {flip}");
        }
    }

    /// The wrong length (not a multiple of two coded bits per input bit,
    /// this code's own rate) is refused, not padded or truncated.
    #[test]
    fn an_odd_length_is_refused() {
        assert_eq!(decode(&[true, false, true]), None);
        assert_eq!(decode(&[]), None);
    }

    /// Table 3.1's own two rows, checked directly: a 0 becomes `0011`, a
    /// 1 becomes `1100`, "in transmission order".
    #[test]
    fn the_pattern_mapper_matches_the_cited_table() {
        assert_eq!(pattern_map_s8(false), [false, false, true, true]);
        assert_eq!(pattern_map_s8(true), [true, true, false, false]);
    }

    /// [`pattern_demap_s8`] inverts [`pattern_map_s8`] exactly with no
    /// errors, and still recovers the right bit with one symbol wrong -
    /// the actual reason S=8 costs four times S=2's own symbol count for
    /// the same coded bit.
    #[test]
    fn pattern_demap_recovers_the_bit_even_with_one_symbol_wrong() {
        for bit in [false, true] {
            let mapped = pattern_map_s8(bit);
            assert_eq!(pattern_demap_s8(mapped), bit);
            for flip in 0..4 {
                let mut corrupted = mapped;
                corrupted[flip] = !corrupted[flip];
                assert_eq!(
                    pattern_demap_s8(corrupted),
                    bit,
                    "bit {bit} flip position {flip}"
                );
            }
        }
    }
}
