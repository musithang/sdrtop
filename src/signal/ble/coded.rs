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
//! **The packet, in bits, both ways.** Beside the primitives, the packet
//! as section 2.2 lays it out: [`sync_symbols`] (the preamble and a known
//! access address through FEC block 1, what a detector looks for),
//! [`read_block1`] (the access address and the Coding Indicator, with how
//! much the decoder repaired), [`peek_header`] (the PDU's length before the
//! rest has arrived) and [`read_block2`] (the PDU and its CRC, de-whitened,
//! for `pdu::decode`). They take one reading a symbol, positive for a 1, and
//! never see a sample: finding the packet in a stream, its symbol timing
//! and its carrier offset are the receiver's work. The test-only
//! [`transmit`] builds a packet from the Core's text, so the readers are
//! held to the specification rather than to themselves.

use crate::signal::dsp::code::lfsr::whiten;

/// The 8-symbol unit the Coded PHY's own preamble repeats ten times -
/// section 2.2.1's own exact words: "10 repetitions of the symbol pattern
/// '00111100' (in transmission order)". Deliberately not the LE 1M/2M
/// alternating pattern [`super::detect::preamble_bits`] computes: this PHY's
/// own preamble is a fixed constant, unrelated to the Access Address that
/// follows it (which, unlike 1M/2M, is itself FEC-encoded before
/// transmission - a real, structural difference this module's own doc
/// names but does not yet build a detector for).
pub const PREAMBLE_SYMBOL: [bool; 8] = [false, false, true, true, true, true, false, false];

/// The Coded PHY's own full preamble: [`PREAMBLE_SYMBOL`] repeated ten
/// times, eighty symbols in total - section 2.2.1's own stated length.
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
type State = [bool; 3];

const STATE_COUNT: usize = 8;

fn state_to_index(s: State) -> usize {
    ((s[0] as usize) << 2) | ((s[1] as usize) << 1) | (s[2] as usize)
}

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
    viterbi(coded.len(), true, |k, g0, g1| hamming(coded, k, g0, g1))
}

/// [`decode`] for a stream whose encoder has not yet been terminated: the
/// traceback starts from whichever state the received bits reach most
/// cheaply rather than from state zero. What reads a packet's header before
/// the rest of FEC block 2 has arrived; its last few bits are the least
/// settled, so a caller decodes some way past what it needs.
#[allow(dead_code)]
pub fn decode_unterminated(coded: &[bool]) -> Option<Vec<bool>> {
    viterbi(coded.len(), false, |k, g0, g1| hamming(coded, k, g0, g1))
}

/// Soft-decision [`decode`]: one reading a coded bit, positive for a 1, its
/// size the confidence. A branch costs the readings it contradicts and
/// gains the ones it agrees with, so a faint reading on the wrong side of
/// zero is outvoted by confident neighbours, where slicing it first would
/// have counted it as a whole error.
#[allow(dead_code)]
pub fn decode_soft(readings: &[f32]) -> Option<Vec<bool>> {
    viterbi(readings.len(), true, |k, g0, g1| {
        correlation(readings, k, g0, g1)
    })
}

/// [`decode_soft`] without termination, as [`decode_unterminated`].
#[allow(dead_code)]
pub fn decode_soft_unterminated(readings: &[f32]) -> Option<Vec<bool>> {
    viterbi(readings.len(), false, |k, g0, g1| {
        correlation(readings, k, g0, g1)
    })
}

/// A branch's hard cost at step `k`: the coded bits it disagrees with.
fn hamming(coded: &[bool], k: usize, g0: bool, g1: bool) -> f32 {
    f32::from(u8::from(g0 != coded[2 * k]) + u8::from(g1 != coded[2 * k + 1]))
}

/// A branch's soft cost at step `k`: minus the readings it expects to be
/// positive, plus the ones it expects negative.
fn correlation(r: &[f32], k: usize, g0: bool, g1: bool) -> f32 {
    let one = |expected: bool, v: f32| if expected { -v } else { v };
    one(g0, r[2 * k]) + one(g1, r[2 * k + 1])
}

/// The trellis every decoder shares, over `len` coded bits with `cost`
/// pricing each branch. `terminated` ends the traceback at state zero
/// (every FEC block of this PHY ends with a termination sequence);
/// otherwise at the cheapest state, the earliest of equals. Hard costs are
/// small whole numbers, exact in an `f32`, so the hard decoders choose
/// exactly as they did with integer metrics.
fn viterbi(
    len: usize,
    terminated: bool,
    cost: impl Fn(usize, bool, bool) -> f32,
) -> Option<Vec<bool>> {
    if len == 0 || !len.is_multiple_of(2) {
        return None;
    }
    let steps = len / 2;

    let mut metric = [f32::INFINITY; STATE_COUNT];
    metric[0] = 0.0;
    let mut back: Vec<[Option<(usize, bool)>; STATE_COUNT]> = Vec::with_capacity(steps);

    for k in 0..steps {
        let mut next_metric = [f32::INFINITY; STATE_COUNT];
        let mut step_back: [Option<(usize, bool)>; STATE_COUNT] = [None; STATE_COUNT];
        for (s, &m) in metric.iter().enumerate() {
            if m.is_infinite() {
                continue;
            }
            for input in [false, true] {
                let (g0, g1, next) = step(index_to_state(s), input);
                let candidate = m + cost(k, g0, g1);
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
    // at whatever cost the received readings give it, so this is never
    // actually unreached - checked rather than assumed, since an
    // `.unwrap()` here would be a claim about the trellis this function
    // does not otherwise need to prove to itself.
    let end = if terminated {
        0
    } else {
        (0..STATE_COUNT).min_by(|&a, &b| metric[a].total_cmp(&metric[b]))?
    };
    if metric[end].is_infinite() {
        return None;
    }

    let mut bits = vec![false; steps];
    let mut state = end;
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

/// FEC block 2's coding scheme, as the Coding Indicator names it (section
/// 2.2.3, Table 2.2): `0b00` is S=8, `0b01` is S=2, and the other two values
/// are reserved, which [`Coding::from_ci`] refuses rather than reads as
/// either.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Coding {
    S2,
    S8,
}

impl Coding {
    /// The CI's two bits in transmission order, least significant first
    /// (section 1.2); `None` for a reserved value.
    pub fn from_ci(ci: [bool; 2]) -> Option<Self> {
        match ci {
            [false, false] => Some(Self::S8),
            [true, false] => Some(Self::S2),
            _ => None,
        }
    }

    pub fn ci(self) -> [bool; 2] {
        match self {
            Self::S8 => [false, false],
            Self::S2 => [true, false],
        }
    }

    /// Symbols on the air per uncoded bit: two coded bits a bit, then one
    /// symbol a coded bit at S=2 or four at S=8 (section 3.3.2, Table 3.1).
    pub fn symbols_per_bit(self) -> usize {
        match self {
            Self::S2 => 2,
            Self::S8 => 8,
        }
    }

    /// Symbols per coded bit: the pattern mapper's P.
    fn symbols_per_coded_bit(self) -> usize {
        self.symbols_per_bit() / 2
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::S2 => "S2",
            Self::S8 => "S8",
        }
    }
}

/// The preamble's length in symbols (section 2.2.1).
pub const PREAMBLE_SYMBOLS: usize = 80;

/// FEC block 1's uncoded bits: the Access Address, the CI and TERM1
/// (section 2.2, Table 2.1).
pub const BLOCK1_BITS: usize = 32 + 2 + 3;

/// FEC block 1 on the air, always at S=8: 296 symbols.
pub const BLOCK1_SYMBOLS: usize = BLOCK1_BITS * 8;

/// How far past the header [`peek_header`] decodes before trusting it, in
/// uncoded bits: the traceback's last bits are the least settled, and five
/// constraint lengths is the usual depth by which a K=4 code's paths have
/// merged.
pub const HEADER_LOOKAHEAD_BITS: usize = 20;

/// The 24-bit CRC and the 3-bit TERM2 that follow the PDU in FEC block 2.
const BLOCK2_TRAILER_BITS: usize = 24 + 3;

/// FEC block 2's length in symbols for a PDU of `pdu_bits` (header and
/// payload); the CRC and TERM2 are added here.
pub fn block2_symbols(coding: Coding, pdu_bits: usize) -> usize {
    (pdu_bits + BLOCK2_TRAILER_BITS) * coding.symbols_per_bit()
}

/// The symbols every packet on `access_address` starts with, whatever its
/// CI and PDU: the preamble, then the access address's 256 symbols through
/// the encoder and the S=8 mapper. The encoder is causal, so those cannot
/// depend on the CI that follows them. On the advertising channels the
/// address is a constant, so these are known in full before anything is
/// heard: 336 symbols to look for.
pub fn sync_symbols(access_address: u32) -> Vec<bool> {
    let mut out = preamble_bits().to_vec();
    let aa = super::detect::access_address_bits(access_address);
    out.extend(map(&encode(&aa), Coding::S8));
    out
}

/// What FEC block 1 said: the access address, the coding of block 2 (`None`
/// for a reserved CI) and how many of its symbols the decoder had to
/// overrule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Block1 {
    pub access_address: u32,
    pub coding: Option<Coding>,
    pub repairs: u32,
}

/// How the readers turn readings into a decision: sliced to bits first and
/// decoded on their Hamming distance, or decoded on the readings
/// themselves. Both are kept so one can be measured against the other
/// (`coded_bench`); [`DECISIONS`] is the one the readers use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decisions {
    /// The reference the bench holds soft decisions to.
    #[allow(dead_code)]
    Hard,
    Soft,
}

/// The decisions [`read_block1`], [`peek_header`] and [`read_block2`] make.
pub const DECISIONS: Decisions = Decisions::Soft;

/// FEC block 1 from its [`BLOCK1_SYMBOLS`] readings, one a symbol, positive
/// for a 1. `None` when there are fewer.
pub fn read_block1(symbols: &[f32]) -> Option<Block1> {
    read_block1_by(symbols, DECISIONS)
}

/// [`read_block1`] with the decisions named.
pub fn read_block1_by(symbols: &[f32], decisions: Decisions) -> Option<Block1> {
    let symbols = symbols.get(..BLOCK1_SYMBOLS)?;
    let (bits, repairs) = read_block(symbols, Coding::S8, true, decisions)?;
    let mut access_address = 0u32;
    for (i, &b) in bits[..32].iter().enumerate() {
        access_address |= u32::from(b) << i;
    }
    Some(Block1 {
        access_address,
        coding: Coding::from_ci([bits[32], bits[33]]),
        repairs,
    })
}

/// The PDU header's 16 bits, de-whitened, before FEC block 2 is complete:
/// decoded without termination from the header and [`HEADER_LOOKAHEAD_BITS`]
/// beyond it. `None` until that much has arrived.
pub fn peek_header(symbols: &[f32], coding: Coding, channel: u8) -> Option<[bool; 16]> {
    peek_header_by(symbols, coding, channel, DECISIONS)
}

/// [`peek_header`] with the decisions named.
pub fn peek_header_by(
    symbols: &[f32],
    coding: Coding,
    channel: u8,
    decisions: Decisions,
) -> Option<[bool; 16]> {
    let need = (16 + HEADER_LOOKAHEAD_BITS) * coding.symbols_per_bit();
    let (bits, _) = read_block(symbols.get(..need)?, coding, false, decisions)?;
    let mut header = [false; 16];
    header.copy_from_slice(&bits[..16]);
    whiten(&mut header, channel);
    Some(header)
}

/// FEC block 2 whole: the PDU and its CRC, de-whitened, ready for
/// `pdu::decode`, and the repairs. Its length comes from the header the
/// readings themselves carry; `None` until that many have arrived.
pub fn read_block2(symbols: &[f32], coding: Coding, channel: u8) -> Option<(Vec<bool>, u32)> {
    read_block2_by(symbols, coding, channel, DECISIONS)
}

/// [`read_block2`] with the decisions named.
pub fn read_block2_by(
    symbols: &[f32],
    coding: Coding,
    channel: u8,
    decisions: Decisions,
) -> Option<(Vec<bool>, u32)> {
    let header = peek_header_by(symbols, coding, channel, decisions)?;
    let length = super::pdu::length(&header)? as usize;
    let need = block2_symbols(coding, 16 + 8 * length);
    let (mut bits, repairs) = read_block(symbols.get(..need)?, coding, true, decisions)?;
    bits.truncate(bits.len() - 3);
    whiten(&mut bits, channel);
    Some((bits, repairs))
}

/// One FEC block's readings to its uncoded bits (termination included when
/// `terminated`) and the count of symbols whose sign disagrees with the
/// decision once it is encoded and mapped again: what the code corrected.
fn read_block(
    symbols: &[f32],
    coding: Coding,
    terminated: bool,
    decisions: Decisions,
) -> Option<(Vec<bool>, u32)> {
    let soft = demap(symbols, coding);
    let bits = match decisions {
        Decisions::Hard => {
            let len = soft.len();
            let hard: Vec<bool> = soft.iter().map(|&v| v > 0.0).collect();
            viterbi(len, terminated, |k, g0, g1| hamming(&hard, k, g0, g1))?
        }
        Decisions::Soft => viterbi(soft.len(), terminated, |k, g0, g1| {
            correlation(&soft, k, g0, g1)
        })?,
    };
    let again = map(&encode(&bits), coding);
    let repairs = again
        .iter()
        .zip(symbols)
        .filter(|(&want, &got)| want != (got > 0.0))
        .count() as u32;
    Some((bits, repairs))
}

/// Coded bits to symbols: Table 3.1, the identity at S=2 and four symbols a
/// bit at S=8.
fn map(coded: &[bool], coding: Coding) -> Vec<bool> {
    match coding {
        Coding::S2 => coded.to_vec(),
        Coding::S8 => coded.iter().flat_map(|&b| pattern_map_s8(b)).collect(),
    }
}

/// Readings to one value a coded bit, positive for a 1. At S=8 the four
/// readings of a coded bit are weighed against Table 3.1's two patterns
/// together (`1100` against `0011`), so one reading the noise pushed across
/// zero does not decide the bit alone; at S=2 each reading is a coded bit.
fn demap(symbols: &[f32], coding: Coding) -> Vec<f32> {
    let p = coding.symbols_per_coded_bit();
    symbols
        .chunks_exact(p)
        .map(|c| match c {
            [r0, r1, r2, r3] => r0 + r1 - r2 - r3,
            [r] => *r,
            _ => unreachable!("P is 1 or 4"),
        })
        .collect()
}

/// A packet's symbols on the air, from its PDU and CRC as sent (`air`:
/// whitened, header through CRC): the preamble (2.2.1); FEC block 1, the
/// access address, the CI and TERM1 through the encoder and the S=8 mapper;
/// FEC block 2, `air` and TERM2 through the encoder and `coding`'s mapper
/// (2.2, 3.3). What the measurement path times and reads a packet against,
/// once the receiver has decoded what it carried.
pub fn symbols_of(access_address: u32, coding: Coding, air: &[bool]) -> Vec<bool> {
    let mut out = preamble_bits().to_vec();
    let mut block1: Vec<bool> = super::detect::access_address_bits(access_address).to_vec();
    block1.extend(coding.ci());
    block1.extend([false; 3]);
    out.extend(map(&encode(&block1), Coding::S8));
    let mut block2 = air.to_vec();
    block2.extend([false; 3]);
    out.extend(map(&encode(&block2), coding));
    out
}

/// A whole packet's symbols, built from the Core's text alone and never from
/// the readers above: [`symbols_of`] on the PDU, its CRC and the whitening as
/// `pdu::encode` builds them for the uncoded PHYs (3.1.1, 3.2).
#[cfg(test)]
pub fn transmit(
    access_address: u32,
    coding: Coding,
    channel: u8,
    header_byte0: u8,
    payload: &[u8],
) -> Vec<bool> {
    symbols_of(
        access_address,
        coding,
        &super::pdu::encode(channel, header_byte0, payload),
    )
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

    /// Table 2.2: 0b00 is S=8, 0b01 is S=2, sent least significant bit first.
    #[test]
    fn the_coding_indicator_reads_as_table_2_2() {
        assert_eq!(Coding::from_ci([false, false]), Some(Coding::S8));
        assert_eq!(Coding::from_ci([true, false]), Some(Coding::S2));
        assert_eq!(Coding::S2.ci(), [true, false]);
        assert_eq!(Coding::S8.ci(), [false, false]);
    }

    #[test]
    fn a_reserved_coding_indicator_is_refused() {
        assert_eq!(Coding::from_ci([false, true]), None);
        assert_eq!(Coding::from_ci([true, true]), None);
    }

    /// Table 2.1's own extremes: 462 us (S=2, a 16-bit PDU) and 17040 us
    /// (S=8, 2056 bits), at one symbol a microsecond.
    #[test]
    fn a_packet_lasts_what_table_2_1_says() {
        let fixed = PREAMBLE_SYMBOLS + BLOCK1_SYMBOLS;
        assert_eq!(fixed + block2_symbols(Coding::S2, 16), 462);
        assert_eq!(fixed + block2_symbols(Coding::S8, 2056), 17040);
    }

    #[test]
    fn the_sync_symbols_are_the_preamble_then_the_coded_address() {
        let aa = crate::signal::ble::detect::ADVERTISING_ACCESS_ADDRESS;
        let s = sync_symbols(aa);
        assert_eq!(s.len(), 336);
        assert_eq!(&s[..80], &preamble_bits()[..]);
        // Whatever the CI and the PDU, the packet starts with them.
        let sent = transmit(aa, Coding::S2, 37, 0x07, &[0x01, 0x00]);
        assert_eq!(&sent[..336], &s[..]);
    }

    fn soft(symbols: &[bool]) -> Vec<f32> {
        symbols
            .iter()
            .map(|&b| if b { 1.0 } else { -1.0 })
            .collect()
    }

    /// Built from the Core's chain, read back through the receiver's: the
    /// access address, the scheme, the PDU, a passing CRC, nothing repaired.
    #[test]
    fn a_coded_packet_reads_back_through_the_chain() {
        let aa = crate::signal::ble::detect::ADVERTISING_ACCESS_ADDRESS;
        for coding in [Coding::S8, Coding::S2] {
            let payload = [0x05, 0x18, 0x23, 0x31, 0x09, 0x64, 0x40, 0x00];
            let sym = transmit(aa, coding, 38, 0x07, &payload);
            let pdu_bits = (2 + payload.len()) * 8;
            assert_eq!(
                sym.len(),
                PREAMBLE_SYMBOLS + BLOCK1_SYMBOLS + block2_symbols(coding, pdu_bits)
            );
            let body = &sym[PREAMBLE_SYMBOLS..];
            let b1 = read_block1(&soft(&body[..BLOCK1_SYMBOLS])).unwrap();
            assert_eq!(
                (b1.access_address, b1.coding, b1.repairs),
                (aa, Some(coding), 0)
            );
            let b2 = &body[BLOCK1_SYMBOLS..];
            let header = peek_header(&soft(b2), coding, 38).unwrap();
            assert_eq!(
                crate::signal::ble::pdu::length(&header),
                Some(payload.len() as u8)
            );
            let (bits, repairs) = read_block2(&soft(b2), coding, 38).unwrap();
            let p = crate::signal::ble::pdu::decode(&bits).unwrap();
            assert!(p.crc_ok, "{coding:?}");
            assert_eq!(p.payload, payload);
            assert_eq!(repairs, 0);
        }
    }

    /// The header can be read before block 2 is complete, from a lookahead
    /// past it alone.
    #[test]
    fn the_header_is_read_before_the_packet_ends() {
        let aa = crate::signal::ble::detect::ADVERTISING_ACCESS_ADDRESS;
        for coding in [Coding::S8, Coding::S2] {
            let payload = [0x42; 30];
            let sym = transmit(aa, coding, 39, 0x07, &payload);
            let b2 = &sym[PREAMBLE_SYMBOLS + BLOCK1_SYMBOLS..];
            let enough = (16 + HEADER_LOOKAHEAD_BITS) * 2 * coding.symbols_per_bit() / 2;
            let header = peek_header(&soft(&b2[..enough]), coding, 39).unwrap();
            assert_eq!(crate::signal::ble::pdu::length(&header), Some(30));
            assert_eq!(peek_header(&soft(&b2[..enough - 1]), coding, 39), None);
        }
    }

    /// Three symbols of block 1 flipped: still read, and counted.
    #[test]
    fn block_one_repairs_what_it_fixes() {
        let aa = crate::signal::ble::detect::ADVERTISING_ACCESS_ADDRESS;
        let sym = transmit(aa, Coding::S8, 37, 0x07, &[0x01, 0x00]);
        let mut b1 = soft(&sym[PREAMBLE_SYMBOLS..PREAMBLE_SYMBOLS + BLOCK1_SYMBOLS]);
        for i in [5, 101, 230] {
            b1[i] = -b1[i];
        }
        let read = read_block1(&b1).unwrap();
        assert_eq!(
            (read.access_address, read.coding, read.repairs),
            (aa, Some(Coding::S8), 3)
        );
    }

    /// Block 2 at S=2 has no pattern mapper to absorb an error: a flipped
    /// symbol is a flipped coded bit, which the convolutional code repairs.
    #[test]
    fn block_two_repairs_at_s2_too() {
        let aa = crate::signal::ble::detect::ADVERTISING_ACCESS_ADDRESS;
        let payload = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66];
        let sym = transmit(aa, Coding::S2, 37, 0x07, &payload);
        let mut b2 = soft(&sym[PREAMBLE_SYMBOLS + BLOCK1_SYMBOLS..]);
        b2[40] = -b2[40];
        let (bits, repairs) = read_block2(&b2, Coding::S2, 37).unwrap();
        let p = crate::signal::ble::pdu::decode(&bits).unwrap();
        assert!(p.crc_ok);
        assert_eq!(repairs, 1);
    }

    /// An unterminated stream decodes to its best end state: everything but
    /// the last few bits, which the traceback has not yet settled, is right.
    #[test]
    fn an_unterminated_stream_decodes_all_but_its_tail() {
        let mut rng = Rng::new(31);
        let bits: Vec<bool> = (0..60).map(|_| rng.next_u64() & 1 == 1).collect();
        let decoded = decode_unterminated(&encode(&bits)).unwrap();
        assert_eq!(decoded.len(), bits.len());
        assert_eq!(&decoded[..40], &bits[..40]);
    }

    /// With no noise, soft and hard agree bit for bit.
    #[test]
    fn soft_and_hard_agree_on_a_clean_stream() {
        let mut rng = Rng::new(7);
        let bits: Vec<bool> = (0..200)
            .map(|_| rng.next_u64() & 1 == 1)
            .chain([false; 3])
            .collect();
        let coded = encode(&bits);
        let readings: Vec<f32> = coded.iter().map(|&b| if b { 1.0 } else { -1.0 }).collect();
        assert_eq!(decode_soft(&readings), decode(&coded));
        assert_eq!(decode_soft(&readings), Some(bits));
    }

    /// A faint wrong reading weighs less than the confident ones around it:
    /// three coded bits within four read barely across zero are decoded
    /// right from their neighbours' confidence, where slicing them hard
    /// leaves the decoder three errors in a span too short for this code,
    /// and it chooses wrong.
    #[test]
    fn soft_decisions_weigh_confidence() {
        let bits = vec![
            true, false, true, true, false, false, true, false, false, false,
        ];
        let coded = encode(&bits);
        let mut r: Vec<f32> = coded.iter().map(|&b| if b { 1.0 } else { -1.0 }).collect();
        for i in [2, 3, 5] {
            r[i] *= -0.05;
        }
        assert_eq!(decode_soft(&r), Some(bits.clone()));
        let hard: Vec<bool> = r.iter().map(|&v| v > 0.0).collect();
        assert_ne!(decode(&hard), Some(bits), "hard decisions get this wrong");
    }
}
