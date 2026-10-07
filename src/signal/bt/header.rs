// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Classic Bluetooth's 18-bit packet header: FEC(1/3) decode, dewhitening,
//! and the HEC/UAP relationship. Checking a header's HEC needs the piconet's
//! UAP, which no packet states, but which can be inferred over several
//! packets from the same LAP.
//!
//! **Harder than "infer the UAP" alone, and said so before landing
//! anything.** The header is whitened - XORed with a pseudo-random
//! sequence - seeded by the piconet master's own clock, CLK1-6, six bits a
//! passive receiver does not know either. [`candidate_uaps`] tries all 64
//! possible values and returns 64 candidate UAPs, at most one of which is
//! real; [`PiconetClock`] is the piece that finds out which, by narrowing
//! across several headers from the same LAP rather than trusting any one
//! of them.
//!
//! **And even that narrows to two, not one - measured, not assumed, after
//! a first attempt (independent per-header set intersection) measurably
//! could not narrow past 32.** [`PiconetClock`]'s own doc has both
//! findings; [`PiconetClock::narrowed`] reports the true floor rather than
//! picking one of the two survivors and hoping.
//!
//! **Not read from the Bluetooth Core Specification itself** (Vol 2, Part
//! B). Ported instead from `libbtbb`
//! (<https://github.com/greatscottgadgets/libbtbb>,
//! `lib/src/bluetooth_packet.c`: `unfec13`, `unwhiten`, `uap_from_hec`, the
//! `INDICES` and `WHITENING_DATA` tables, and the `PACKET_TYPE_*` constants
//! from `lib/src/bluetooth_packet.h`), the same organisation and the same
//! standing `access_code`'s port of `btbb_gen_syncword` has: GPL-2.0-or-
//! later, license-compatible with this project's GPL-3.0-or-later, and in
//! real use by Ubertooth hardware for well over a decade.
//!
//! **Bit convention matches the rest of `signal::bt`, because it is the
//! same convention.** `libbtbb`'s own `air_to_host8/16/32` map array index `i`
//! (the `i`-th bit received) to bit `i` of the resulting integer - exactly
//! `signal::bt::access_code::pack`'s own convention. Every
//! function below takes and returns bits in that same order.

/// The header proper, once FEC(1/3) has recovered it from the 54 bits the
/// air actually carries and dewhitening has removed the piconet's own
/// scrambling - 3 bits LT_ADDR, 4 bits TYPE, 3 bits FLOW/ARQN/SEQN packed
/// as `flags`, 8 bits HEC.
pub const HEADER_BITS: usize = 18;

/// How many bits FEC(1/3) actually reads off the air for one header - three
/// repeats of each of the 18 host bits.
pub const HEADER_AIR_BITS: usize = HEADER_BITS * 3;

/// How many bits sit between the end of the 64-bit access code
/// (`signal::bt::detect::Detector`'s own trigger point) and the header's
/// own first air bit - a fixed trailer with no content read here.
/// `libbtbb`'s own `try_clock`/`btbb_decode_header` skip 68 bits past the
/// start of their own `pkt->symbols`, which begins at the 64-bit sync word
/// (not the 4-bit preamble a real receiver also sees but `Detector` never
/// stores): `68 - 64 = 4`.
pub const TRAILER_BITS: usize = 4;

/// Decode 1/3-rate FEC: three like symbols in a row, majority-voted.
///
/// `air` must be exactly [`HEADER_AIR_BITS`] long. Returns `None` when too
/// many of the eighteen triples disagree internally - `libbtbb`'s own
/// quality gate, ported as `< length / 4` genuinely disagreeing triples
/// rather than trusting a decode built from more disagreement than that
/// implies a clean capture. This is a refusal, not a correction: unlike
/// `signal::bt::access_code`'s own exact-match access code, a bit that
/// disagrees across its own three repeats is still repaired by the
/// majority vote, but three-way agreement failing on a quarter or more of
/// the header means the capture itself was not clean enough to trust at
/// all.
pub fn unfec13(air: &[bool]) -> Option<[bool; HEADER_BITS]> {
    if air.len() != HEADER_AIR_BITS {
        return None;
    }
    let mut out = [false; HEADER_BITS];
    let mut disagreements = 0usize;
    for i in 0..HEADER_BITS {
        let (a, b, c) = (air[3 * i], air[3 * i + 1], air[3 * i + 2]);
        out[i] = (a && b) || (b && c) || (c && a);
        if (a != b) || (b != c) || (c != a) {
            disagreements += 1;
        }
    }
    if disagreements < HEADER_BITS / 4 {
        Some(out)
    } else {
        None
    }
}

/// One period of the whitening LFSR's own output, and the 64 starting
/// points - one per possible CLK1-6 value - `libbtbb`'s own `INDICES` and
/// `WHITENING_DATA` tables, ported verbatim.
const WHITENING_INDICES: [u8; 64] = [
    99, 85, 17, 50, 102, 58, 108, 45, 92, 62, 32, 118, 88, 11, 80, 2, 37, 69, 55, 8, 20, 40, 74,
    114, 15, 106, 30, 78, 53, 72, 28, 26, 68, 7, 39, 113, 105, 77, 71, 25, 84, 49, 57, 44, 61, 117,
    10, 1, 123, 124, 22, 125, 111, 23, 42, 126, 6, 112, 76, 24, 48, 43, 116, 0,
];

#[rustfmt::skip]
const WHITENING_DATA: [bool; 127] = {
    const fn b(n: u8) -> bool { n != 0 }
    [
        b(1), b(1), b(1), b(0), b(0), b(0), b(1), b(1), b(1), b(0), b(1), b(1), b(0), b(0), b(0), b(1),
        b(0), b(1), b(0), b(0), b(1), b(0), b(1), b(1), b(1), b(1), b(1), b(0), b(1), b(0), b(1), b(0),
        b(1), b(0), b(0), b(0), b(0), b(1), b(0), b(1), b(1), b(0), b(1), b(1), b(1), b(1), b(0), b(0),
        b(1), b(1), b(1), b(0), b(0), b(1), b(0), b(1), b(0), b(1), b(1), b(0), b(0), b(1), b(1), b(0),
        b(0), b(0), b(0), b(0), b(1), b(1), b(0), b(1), b(1), b(0), b(1), b(0), b(1), b(1), b(1), b(0),
        b(1), b(0), b(0), b(0), b(1), b(1), b(0), b(0), b(1), b(0), b(0), b(0), b(1), b(0), b(0), b(0),
        b(0), b(0), b(0), b(1), b(0), b(0), b(1), b(0), b(0), b(1), b(1), b(0), b(1), b(0), b(0), b(1),
        b(1), b(1), b(1), b(0), b(1), b(1), b(1), b(0), b(0), b(0), b(0), b(1), b(1), b(1), b(1),
    ]
};

/// Remove the whitening from [`HEADER_BITS`] bits, given the piconet's own
/// CLK1-6 (only its low 6 bits are used, matching `libbtbb`'s own `clock &
/// 0x3f`). The header is always dewhitened from its own first bit -
/// `libbtbb`'s own `unwhiten(..., skip=0, ...)` for a header, as opposed to
/// a payload, which starts further into the same LFSR sequence.
pub(crate) fn unwhiten_header(bits: &[bool; HEADER_BITS], clk6: u8) -> [bool; HEADER_BITS] {
    unwhiten_at(bits, clk6, 0).try_into().unwrap()
}

/// Remove the whitening from an arbitrary run of bits starting `skip` bits
/// into the same LFSR sequence [`unwhiten_header`] always starts at 0 -
/// `libbtbb`'s own `unwhiten(..., skip, ...)`, generalised for
/// [`super::payload`]'s own need: a classic BT payload is whitened by the
/// *same* stream as its own header, continuing from bit 18 rather than
/// restarting - every `libbtbb` call site that dewhitens a payload or a
/// payload header passes `skip = 18` (`HEADER_BITS`), never a fresh origin.
pub(crate) fn unwhiten_at(bits: &[bool], clk6: u8, skip: usize) -> Vec<bool> {
    let mut index =
        (WHITENING_INDICES[(clk6 & 0x3f) as usize] as usize + skip) % WHITENING_DATA.len();
    let mut out = Vec::with_capacity(bits.len());
    for &bit in bits {
        out.push(bit ^ WHITENING_DATA[index]);
        index = (index + 1) % WHITENING_DATA.len();
    }
    out
}

/// Reverse the eight bits of a byte - `libbtbb`'s own `reverse`, needed
/// because [`uap_from_hec`]'s LFSR runs the opposite bit order the rest of
/// `signal::bt` does.
pub(crate) fn reverse_bits(byte: u8) -> u8 {
    let mut out = 0u8;
    for i in 0..8 {
        if byte & (1 << i) != 0 {
            out |= 1 << (7 - i);
        }
    }
    out
}

/// Recover the Upper Address Part directly from one header's own 10 data
/// bits (LT_ADDR, TYPE, FLOW, ARQN, SEQN, host order) and its 8-bit HEC -
/// `libbtbb`'s own `uap_from_hec`, an exact inversion of the HEC's own
/// LFSR, not a search over the 256 possible UAPs.
///
/// **Answers every input, correct or not.** Nothing here knows whether
/// `data`/`hec` came from a real header or from noise the FEC quality gate
/// let through by chance - it is [`PiconetClock`]'s job to tell a genuine,
/// recurring UAP apart from the essentially random answer a wrong CLK1-6
/// guess or a corrupted capture produces.
pub(crate) fn uap_from_hec(data: u16, hec: u8) -> u8 {
    let mut hec = hec;
    for i in (0..10).rev() {
        if hec & 0x80 != 0 {
            hec ^= 0x65;
        }
        let bit = ((hec >> 7) ^ ((data >> i) as u8)) & 0x01;
        hec = (hec << 1) | bit;
    }
    reverse_bits(hec)
}

/// Every CLK1-6 guess's own candidate UAP for one still-whitened header -
/// exactly one entry real, the rest as good as random. [`PiconetClock`]
/// narrows across several of these, from several headers sharing a LAP,
/// rather than trusting any single one.
pub fn candidate_uaps(whitened: &[bool; HEADER_BITS]) -> [u8; 64] {
    let mut out = [0u8; 64];
    for (clk6, slot) in out.iter_mut().enumerate() {
        let bits = unwhiten_header(whitened, clk6 as u8);
        let data = pack_bits(&bits[0..10]);
        let hec = pack_bits(&bits[10..18]) as u8;
        *slot = uap_from_hec(data, hec);
    }
    out
}

/// Pack up to 16 air-order bits into an integer, bit `i` to bit `i` -
/// `libbtbb`'s own `air_to_host16`/`air_to_host8`, and
/// `signal::bt::access_code::pack`'s own convention already.
pub(crate) fn pack_bits(bits: &[bool]) -> u16 {
    let mut word = 0u16;
    for (i, &bit) in bits.iter().enumerate().take(16) {
        if bit {
            word |= 1 << i;
        }
    }
    word
}

/// The 4-bit `TYPE` field, all sixteen values - `libbtbb`'s own
/// `PACKET_TYPE_*` constants (`lib/src/bluetooth_packet.h`), a total
/// mapping: every possible 4-bit value names a real packet type, so
/// decoding one never needs an "unknown" case.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PacketType {
    Null,
    Poll,
    Fhs,
    Dm1,
    Dh1,
    Hv1,
    Hv2,
    Hv3,
    Dv,
    Aux1,
    Dm3,
    Dh3,
    Ev4,
    Ev5,
    Dm5,
    Dh5,
}

impl PacketType {
    /// The 4-bit `TYPE` value, the inverse of [`Self::from_code`]: the
    /// declaration order is the field's order.
    pub fn code(self) -> u8 {
        self as u8
    }

    /// The type a 4-bit `TYPE` value names; the upper bits are ignored.
    pub fn from_code(bits: u8) -> Self {
        Self::from_bits(bits)
    }

    /// Every packet this TYPE code can be, basic-rate name first.
    ///
    /// **The header does not say which.** A TYPE code means one packet on an
    /// ACL link in basic rate, another once the link has switched to the EDR
    /// packet table (which the link managers agree between themselves, out
    /// of the header's sight), and others again on a SCO or eSCO voice link.
    /// On the air a phone playing music to headphones sent code 10 almost
    /// throughout: a DM3 by the basic-rate table, a 2-DH3 in fact, with the
    /// switch to the EDR table seen in its own link-manager traffic. So every
    /// reading is named, rather than the likeliest.
    ///
    /// The codes are the Core's packet type tables (Vol 2 Part B 6.5) as
    /// test-instrument and reference documentation quote them; the Core text
    /// itself would not load here.
    pub fn names(self) -> &'static [&'static str] {
        match self {
            PacketType::Null => &["NULL"],
            PacketType::Poll => &["POLL"],
            PacketType::Fhs => &["FHS"],
            PacketType::Dm1 => &["DM1"],
            PacketType::Dh1 => &["DH1", "2-DH1"],
            PacketType::Hv1 => &["HV1"],
            PacketType::Hv2 => &["HV2", "2-EV3"],
            PacketType::Hv3 => &["HV3", "EV3", "3-EV3"],
            PacketType::Dv => &["DV", "3-DH1"],
            PacketType::Aux1 => &["AUX1"],
            PacketType::Dm3 => &["DM3", "2-DH3"],
            PacketType::Dh3 => &["DH3", "3-DH3"],
            PacketType::Ev4 => &["EV4", "2-EV5"],
            PacketType::Ev5 => &["EV5", "3-EV5"],
            PacketType::Dm5 => &["DM5", "2-DH5"],
            PacketType::Dh5 => &["DH5", "3-DH5"],
        }
    }

    /// Every reading of the code, as a panel and an export show it:
    /// `DM3/2-DH3`.
    pub fn shown(self) -> String {
        self.names().join("/")
    }

    /// The basic-rate name alone: the table as the header decode was first
    /// written, which the tests still list.
    #[cfg(test)]
    pub fn label(self) -> &'static str {
        match self {
            PacketType::Null => "NULL",
            PacketType::Poll => "POLL",
            PacketType::Fhs => "FHS",
            PacketType::Dm1 => "DM1",
            PacketType::Dh1 => "DH1",
            PacketType::Hv1 => "HV1",
            PacketType::Hv2 => "HV2",
            PacketType::Hv3 => "HV3",
            PacketType::Dv => "DV",
            PacketType::Aux1 => "AUX1",
            PacketType::Dm3 => "DM3",
            PacketType::Dh3 => "DH3",
            PacketType::Ev4 => "EV4",
            PacketType::Ev5 => "EV5",
            PacketType::Dm5 => "DM5",
            PacketType::Dh5 => "DH5",
        }
    }

    fn from_bits(bits: u8) -> Self {
        match bits & 0x0f {
            0 => PacketType::Null,
            1 => PacketType::Poll,
            2 => PacketType::Fhs,
            3 => PacketType::Dm1,
            4 => PacketType::Dh1,
            5 => PacketType::Hv1,
            6 => PacketType::Hv2,
            7 => PacketType::Hv3,
            8 => PacketType::Dv,
            9 => PacketType::Aux1,
            10 => PacketType::Dm3,
            11 => PacketType::Dh3,
            12 => PacketType::Ev4,
            13 => PacketType::Ev5,
            14 => PacketType::Dm5,
            _ => PacketType::Dh5,
        }
    }
}

/// A fully decoded header: dewhitened with the confirmed CLK1-6 that made
/// its own HEC agree with the piconet's already-confirmed UAP.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub lt_addr: u8,
    pub packet_type: PacketType,
    pub flags: u8,
    pub hec: u8,
    /// The CLK1-6 that reproduced this header under the UAP it was decoded
    /// with - not part of any wire format, but needed by [`super::payload`]
    /// to dewhiten this same packet's payload, which continues the header's
    /// own whitening stream rather than starting a fresh one.
    pub clk6: u8,
}

/// Every `(UAP, CLK1-6)` a lone header allows for the given UAPs: what a
/// [`PiconetClock`] seeded by it would hand the tie-break. For tests.
#[cfg(test)]
pub(crate) fn pairs_for(whitened: &[bool; HEADER_BITS], uaps: &[u8]) -> Vec<(u8, u8)> {
    candidate_uaps(whitened)
        .iter()
        .enumerate()
        .filter(|(_, u)| uaps.contains(u))
        .map(|(clk6, &u)| (u, clk6 as u8))
        .collect()
}

/// Decode a header at a known CLK1-6 under a known UAP: `None` when the two
/// do not agree, the HEC naming another UAP at that clock.
pub fn decode_at(whitened: &[bool; HEADER_BITS], uap: u8, clk6: u8) -> Option<Header> {
    let bits = unwhiten_header(whitened, clk6);
    let data = pack_bits(&bits[0..10]);
    let hec = pack_bits(&bits[10..18]) as u8;
    (uap_from_hec(data, hec) == uap).then(|| Header {
        lt_addr: pack_bits(&bits[0..3]) as u8,
        packet_type: PacketType::from_bits(pack_bits(&bits[3..7]) as u8),
        flags: pack_bits(&bits[7..10]) as u8,
        hec,
        clk6,
    })
}

/// The first CLK1-6 that reproduces a header under a UAP, and the header
/// read at it.
///
/// **Not how a live header is read**: about one header in five has a
/// second clock that gives the same UAP, under which it reads as another
/// packet type and LT_ADDR. Only the piconet's own clock knows which one
/// is right ([`PiconetClock::clocks_for`]); this remains for tests that
/// build a header whose clock is the only one.
#[cfg(test)]
pub fn decode_with_uap(whitened: &[bool; HEADER_BITS], uap: u8) -> Option<Header> {
    (0u8..64).find_map(|clk6| decode_at(whitened, uap, clk6))
}

/// One slot, 625 us: how often CLK1-6 steps. It is bits 1 to 6 of CLK, so
/// it moves at half CLK's 3.2 kHz.
pub const SLOT_US: f64 = 625.0;

/// Narrows a piconet's own CLK1-6 *and* UAP together from several headers
/// sharing one LAP, none individually trustworthy on its own - `libbtbb`'s
/// own `btbb_uap_from_header` (`lib/src/bluetooth_piconet.c`), ported for
/// the reason its own first attempt at this module (a plain set
/// intersection across headers) turned out not to work at all.
///
/// **Why a set intersection cannot work, measured rather than reasoned
/// out in advance.** [`uap_from_hec`] is linear enough that
/// [`candidate_uaps`]'s own 64-element output, for a fixed real UAP, is a
/// *fixed set added the same way no matter what the rest of the header
/// says* - confirmed directly: two headers with the same real UAP but
/// completely different LT_ADDR, TYPE, flags, CLK1-6 and HEC produced
/// byte-for-byte identical 32-element candidate sets. Intersecting many
/// such headers never narrows past that same 32, because every header
/// contributes the identical evidence. No amount of HEC alone breaks this;
/// something else about a piconet has to.
///
/// **What actually breaks it: real elapsed time.** CLK1-6 does not reset
/// between packets - it keeps advancing, **once a slot** ([`SLOT_US`]):
/// CLK itself ticks at 3.2 kHz, but CLK1-6 is its bits 1 to 6 and so
/// steps at half that. Counting it at the full CLK rate, as this port first
/// did, eliminated the true hypothesis at the first header one slot on; on
/// the air every piconet's candidates fell back to 32 again and again, and
/// the headers' own ground truth (two real devices, their addresses read
/// off them) showed the clock stepping once per 625 us. `libbtbb`'s own
/// design keeps 64 *persistent* hypotheses, one per guess at the very
/// first header's own CLK1-6, indexed by that guess rather than by the UAP
/// it happened to produce. For every later header, hypothesis `count`
/// predicts the *current* clock as `count` plus however many slots have
/// elapsed, and is kept only if that predicted clock still reproduces the
/// exact UAP it committed to the first time. A wrong guess drifts out of
/// step with the real header content packet after packet and is
/// eliminated; the true guess, and only the true guess, keeps agreeing
/// with itself forever. This needs the caller to know when each header
/// arrived, on one clock.
///
/// **Slots are counted from the previous header, not the first.** Packets
/// start on slot boundaries, so the time between two headers of one
/// piconet is a whole number of slots give or take the transmitter's
/// jitter, and rounding it is safe. Rounding the time since the *first*
/// header is not: the two clocks, ours and the piconet's, drift apart by
/// their crystals' difference, 16 ppm measured on one real device, which
/// is half a slot in 20 seconds. Chained header to header, the drift in
/// any one gap is a few microseconds.
///
/// **The honest floor this reaches is two candidates, not one - measured,
/// not assumed, and said so rather than quietly reported as a confirmed
/// single answer.** Running this against a synthetic piconet converges
/// smoothly (64 to 8 to 4...) and then sticks at exactly two surviving
/// hypotheses, indefinitely, holding two genuinely *different* UAP
/// values - not two labels for the same answer. `libbtbb`'s own
/// `crc_check`, called alongside this same loop, resolves the tie using
/// the *payload's* CRC - a register run over the whole payload rather than
/// the header's own 10 bits, long enough that the same algebra no longer
/// leaves a twin standing. This module does not decode a payload, so it
/// does not chase that tie-break; [`Self::narrowed`] reports the true
/// floor a header-only receiver can reach, honestly, rather than picking
/// one of the two and hoping.
///
/// **Wired live**: `signal::bt::receive` captures each header and
/// its stream tick, and `signal::net::worker` folds them in per LAP. Once a
/// LAP's UAP is one value the worker reads its headers with
/// [`decode_at`] at the clock [`Self::clocks_for`] gives, and the hypotheses still
/// standing ([`Self::hypotheses`]) are shown beside what they say.
pub struct PiconetClock {
    /// When the last header arrived (us, on the stream's clock) and how many
    /// slots after the first it was - `None` before the first header.
    last: Option<(f64, i64)>,
    /// One candidate per possible CLK1-6 for that first header - `libbtbb`'s
    /// own `clock6_candidates`. `None` once eliminated.
    candidates: [Option<u8>; 64],
}

impl Default for PiconetClock {
    fn default() -> Self {
        Self {
            last: None,
            candidates: [None; 64],
        }
    }
}

impl PiconetClock {
    #[cfg(test)]
    pub fn new() -> Self {
        Self::default()
    }

    /// The distinct UAP values still consistent with every header seen so
    /// far, sorted, ascending - empty before the first header, one in the
    /// lucky case a short capture never actually exercises the tie
    /// (`Self`'s own doc explains why one is not the guaranteed floor
    /// exactly two is), otherwise settling at exactly two once enough
    /// headers have been observed to eliminate everything else.
    /// How many of the 64 CLK1-6 hypotheses are still standing: 64 before
    /// the first header has been folded in against a second, fewer as
    /// elapsed time rules them out, `0` before any header at all.
    pub fn hypotheses(&self) -> u8 {
        self.candidates.iter().flatten().count() as u8
    }

    /// The CLK1-6 at `tick` of every hypothesis still standing that names
    /// `uap`: the clocks a header arriving then can be read at under it.
    /// Usually one; two when two hypotheses with the same UAP survive, and
    /// then a header cannot be read with certainty.
    pub fn clocks_for(&self, uap: u8, at_us: f64) -> Vec<u8> {
        let Some(slot) = self.slot_at(at_us) else {
            return Vec::new();
        };
        let elapsed = slot.rem_euclid(64) as usize;
        let mut clocks: Vec<u8> = self
            .candidates
            .iter()
            .enumerate()
            .filter(|(_, c)| **c == Some(uap))
            .map(|(count, _)| ((count + elapsed) % 64) as u8)
            .collect();
        clocks.sort_unstable();
        clocks.dedup();
        clocks
    }

    /// Keep only the hypothesis whose clock at `tick` is `clk6`: a payload
    /// that checked out at that clock has shown it is the piconet's own,
    /// so every header after it has one clock to be read at.
    pub fn pin(&mut self, at_us: f64, clk6: u8) {
        let Some(slot) = self.slot_at(at_us) else {
            return;
        };
        let elapsed = slot.rem_euclid(64) as usize;
        for (count, slot) in self.candidates.iter_mut().enumerate() {
            if (count + elapsed) % 64 != clk6 as usize {
                *slot = None;
            }
        }
    }

    pub fn narrowed(&self) -> Vec<u8> {
        let mut uaps: Vec<u8> = self.candidates.iter().flatten().copied().collect();
        uaps.sort_unstable();
        uaps.dedup();
        uaps
    }

    /// Slots since the first header, for a header arriving at `at_us`:
    /// the last header's count plus the gap since it, rounded to whole
    /// slots.
    fn slot_at(&self, at_us: f64) -> Option<i64> {
        let (last_us, last_slot) = self.last?;
        Some(last_slot + ((at_us - last_us) / SLOT_US).round() as i64)
    }

    fn seed(&mut self, at_us: f64, whitened: &[bool; HEADER_BITS]) {
        self.last = Some((at_us, 0));
        let seed = candidate_uaps(whitened);
        for (count, slot) in self.candidates.iter_mut().enumerate() {
            *slot = Some(seed[count]);
        }
    }

    /// Fold in one more header, at CLK1-6 tick `tick` - any consistent
    /// integer count of ticks since some fixed reference, not necessarily
    /// this LAP's own first header, since this method finds its own
    /// reference the first time it is called.
    ///
    /// **A contradiction re-seeds from this header rather than sticking.**
    /// `libbtbb`'s own `reset()`: if every hypothesis disagrees with this
    /// header, the accumulated evidence was never one consistent piconet
    /// to begin with (a false access-code detection on this LAP, most
    /// plausibly), and clinging to it would refuse every future header
    /// too. This header becomes the new "first" instead.
    pub fn observe(&mut self, at_us: f64, whitened: &[bool; HEADER_BITS]) {
        let Some(slot) = self.slot_at(at_us) else {
            self.seed(at_us, whitened);
            return;
        };
        self.last = Some((at_us, slot));
        let elapsed = slot.rem_euclid(64) as u64;
        let mut remaining = 0u32;
        for count in 0..64u64 {
            let Some(expected) = self.candidates[count as usize] else {
                continue;
            };
            let clock = ((count + elapsed) % 64) as u8;
            let bits = unwhiten_header(whitened, clock);
            let data = pack_bits(&bits[0..10]);
            let hec = pack_bits(&bits[10..18]) as u8;
            if uap_from_hec(data, hec) == expected {
                remaining += 1;
            } else {
                self.candidates[count as usize] = None;
            }
        }
        if remaining == 0 {
            self.seed(at_us, whitened);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A structural check independent of the ported constants' own
    /// citation, the same discipline the access code module holds
    /// itself to - and independent of any second, separately-ported
    /// algorithm this module has no citation for either.
    ///
    /// **[`uap_from_hec`] claims to invert a real computation, which means
    /// it must be a bijection in `hec` for any fixed `data`: 256 possible
    /// received checksums, 256 distinct answers, none repeated.** A
    /// function built from information-losing steps (an unconditional AND
    /// or OR, a truncating cast) could still look plausible on a single
    /// worked example while quietly colliding elsewhere; checking every one
    /// of the 256 inputs for several `data` values is exhaustive, not a
    /// sample.
    #[test]
    fn uap_from_hec_is_a_bijection_in_hec_for_any_fixed_data() {
        for data in [0x000u16, 0x155, 0x2aa, 0x3ff, 0x1a3, 0x0c7] {
            let mut seen = [false; 256];
            for hec in 0u16..256 {
                let uap = uap_from_hec(data, hec as u8);
                assert!(
                    !seen[uap as usize],
                    "data {data:#05x}: hec {hec:#04x} collided with an earlier one at uap {uap:#04x}"
                );
                seen[uap as usize] = true;
            }
        }
    }

    /// Build a synthetic, still-whitened header from data and HEC bits
    /// chosen freely, with the UAP that makes them consistent computed
    /// *from* them by [`uap_from_hec`] itself - not from a second,
    /// independently-ported forward algorithm this module has no citation
    /// for. [`uap_from_hec`]'s own correctness is corroborated separately,
    /// structurally, by `uap_from_hec_is_a_bijection_in_hec_for_any_fixed_
    /// data` above.
    fn synthetic_header(data10: u16, hec: u8, clk6: u8) -> ([bool; HEADER_BITS], u8) {
        let mut host = [false; HEADER_BITS];
        for (i, slot) in host[0..10].iter_mut().enumerate() {
            *slot = (data10 >> i) & 1 != 0;
        }
        for (i, slot) in host[10..18].iter_mut().enumerate() {
            *slot = (hec >> i) & 1 != 0;
        }
        // Whitening is XOR with a fixed sequence, its own inverse - the
        // same call turns real (host) bits into their whitened, on-air
        // form as turns whitened bits back.
        let whitened = unwhiten_header(&host, clk6);
        (whitened, uap_from_hec(data10, hec))
    }

    /// The `hec` that makes a chosen `data` consistent with a chosen
    /// target UAP, found by search rather than by a forward algorithm -
    /// legitimate because [`uap_from_hec`]'s own bijection property
    /// (proven above) guarantees exactly one exists.
    fn hec_for(data10: u16, target_uap: u8) -> u8 {
        (0u16..256)
            .map(|h| h as u8)
            .find(|&h| uap_from_hec(data10, h) == target_uap)
            .expect("uap_from_hec is a bijection in hec, so some hec must map to any target uap")
    }

    /// [`unfec13`]: a clean, tripled header round-trips
    /// exactly.
    #[test]
    fn a_clean_tripled_header_round_trips_exactly() {
        let bits = [
            true, false, true, false, true, true, false, false, true, false, true, true, false,
            true, false, false, true, true,
        ];
        let mut air = Vec::with_capacity(HEADER_AIR_BITS);
        for b in bits {
            air.extend([b, b, b]);
        }
        assert_eq!(unfec13(&air).unwrap(), bits);
    }

    /// A single bit error inside one triple is corrected by the majority
    /// vote rather than corrupting the recovered bit.
    #[test]
    fn a_single_bit_error_per_triple_is_corrected() {
        let bits = [true; HEADER_BITS];
        let mut air = Vec::with_capacity(HEADER_AIR_BITS);
        for b in bits {
            air.extend([b, b, b]);
        }
        // Flip one of the three copies of every sixth triple - three of
        // eighteen triples get a single flipped bit each, comfortably under
        // the four disagreeing triples that would refuse the header
        // (`HEADER_BITS / 4 == 4`).
        for i in (0..HEADER_BITS).step_by(6) {
            air[3 * i] = !air[3 * i];
        }
        assert_eq!(unfec13(&air).unwrap(), bits);
    }

    /// Too many disagreeing triples is refused rather than guessed at.
    #[test]
    fn too_many_disagreeing_triples_is_refused() {
        let mut air = vec![true; HEADER_AIR_BITS];
        // Five triples (of eighteen) with no majority - `false, true, ...`
        // is not a tie in a 3-vote scheme, so make each of the five triples
        // itself genuinely mixed enough to count as a disagreement.
        for i in 0..5 {
            air[3 * i] = false;
        }
        assert!(unfec13(&air).is_none());
    }

    /// The wrong length is refused, not read past the end or padded.
    #[test]
    fn the_wrong_length_is_refused() {
        assert!(unfec13(&[true; HEADER_AIR_BITS - 1]).is_none());
        assert!(unfec13(&[true; HEADER_AIR_BITS + 1]).is_none());
    }

    /// [`candidate_uaps`] always includes the true UAP among its 64 - the
    /// property [`PiconetClock`]'s whole design leans on.
    #[test]
    fn the_true_uap_is_always_among_the_sixty_four_candidates() {
        for uap in [0x00u8, 0x3c, 0x81, 0xff] {
            for clk6 in [0u8, 1, 31, 63] {
                let lt_addr = 0b101u8;
                let packet_type = 0b0100u8; // DH1
                let flags = 0b011u8;
                let data10 = (lt_addr as u16) | ((packet_type as u16) << 3) | ((flags as u16) << 7);
                let hec = hec_for(data10, uap);
                let (whitened, real_uap) = synthetic_header(data10, hec, clk6);
                assert_eq!(real_uap, uap);
                let candidates = candidate_uaps(&whitened);
                assert!(
                    candidates.contains(&uap),
                    "uap {uap:#04x} clk6 {clk6} missing from {candidates:?}"
                );
            }
        }
    }

    /// [`decode_with_uap`]: given the confirmed UAP,
    /// the header's real fields come back, whichever CLK1-6 the header
    /// actually used.
    #[test]
    fn decode_with_uap_recovers_the_real_fields() {
        let uap = 0x77u8;
        let clk6 = 29u8;
        let lt_addr = 0b110u8;
        let packet_type_bits = 0b0100u8; // DH1
        let flags = 0b101u8;
        let data10 = (lt_addr as u16) | ((packet_type_bits as u16) << 3) | ((flags as u16) << 7);
        let hec = hec_for(data10, uap);
        let (whitened, _) = synthetic_header(data10, hec, clk6);
        let decoded = decode_with_uap(&whitened, uap).expect("should decode");
        assert_eq!(decoded.lt_addr, lt_addr);
        assert_eq!(decoded.packet_type, PacketType::Dh1);
        assert_eq!(decoded.flags, flags);
        assert_eq!(decoded.hec, hec);
        assert_eq!(decoded.clk6, clk6);
    }

    /// **Why a header is read at the piconet's clock and not the first that
    /// fits.** This DM3 header, sent at CLK1-6 44, has a second clock, 19,
    /// that gives the same UAP, and under it the header reads as a DH3. The
    /// first clock that fits is 19. The piconet's clock, seeded by an
    /// earlier header 25 ticks before, gives 44 alone once the other
    /// hypothesis has fallen.
    #[test]
    fn a_header_is_read_at_the_piconets_clock_not_the_first_that_fits() {
        let uap = 0x3au8;
        let data10 = 0b011u16 | ((PacketType::Dm3.code() as u16) << 3) | (0b101 << 7);
        let hec = hec_for(data10, uap);
        let (whitened, _) = synthetic_header(data10, hec, 44);
        let fits: Vec<u8> = (0..64)
            .filter(|&c| decode_at(&whitened, uap, c).is_some())
            .collect();
        assert_eq!(fits, vec![19, 44], "two clocks, one UAP");
        assert_eq!(
            decode_at(&whitened, uap, 19).unwrap().packet_type,
            PacketType::Dh3
        );
        assert_eq!(
            decode_at(&whitened, uap, 44).unwrap().packet_type,
            PacketType::Dm3
        );

        // The same piconet 25 slots earlier, at clock 19, then this header.
        let (earlier, _) = synthetic_header(0b010 | (4 << 3), hec_for(0b010 | (4 << 3), uap), 19);
        let mut clock = PiconetClock::new();
        clock.observe(100.0 * SLOT_US, &earlier);
        clock.observe(125.0 * SLOT_US, &whitened);
        assert!(
            clock.clocks_for(uap, 125.0 * SLOT_US).contains(&44),
            "{:?}",
            clock.clocks_for(uap, 125.0 * SLOT_US)
        );
        assert!(!clock.clocks_for(uap, 125.0 * SLOT_US).contains(&19));

        // A payload that checked out at 44 pins the clock: from then on one
        // hypothesis, one clock, one UAP.
        clock.pin(125.0 * SLOT_US, 44);
        assert_eq!(clock.clocks_for(uap, 125.0 * SLOT_US), vec![44]);
        assert_eq!(
            clock.clocks_for(uap, 131.0 * SLOT_US),
            vec![50],
            "and it runs on"
        );
        assert_eq!(clock.narrowed(), vec![uap]);
    }

    /// The wrong UAP finds no CLK1-6 that reproduces it - vanishingly
    /// unlikely on real data, and this receiver says so rather than
    /// returning a header that does not actually match.
    #[test]
    fn the_wrong_uap_usually_decodes_to_nothing() {
        let uap = 0x77u8;
        let clk6 = 29u8;
        let data10 = 0b1_010_101_010u16;
        let hec = hec_for(data10, uap);
        let (whitened, _) = synthetic_header(data10, hec, clk6);
        assert!(decode_with_uap(&whitened, uap ^ 0x01).is_none());
    }

    /// Every one of the sixteen `TYPE` values names a real packet type -
    /// `libbtbb`'s own total mapping, checked directly rather than trusted.
    #[test]
    fn every_four_bit_type_value_names_a_packet_type() {
        let labels: Vec<&str> = (0u8..16)
            .map(|b| PacketType::from_bits(b).label())
            .collect();
        assert_eq!(
            labels,
            vec![
                "NULL", "POLL", "FHS", "DM1", "DH1", "HV1", "HV2", "HV3", "DV", "AUX1", "DM3",
                "DH3", "EV4", "EV5", "DM5", "DH5",
            ]
        );
    }

    /// Build the header a real piconet would actually send at `elapsed`
    /// ticks past `true_first_clk6` - the piconet's own CLK1-6 keeps
    /// advancing between packets, which is the fact [`PiconetClock`]'s
    /// whole design leans on and a set-intersection approach (this
    /// module's own first, abandoned attempt) could not use.
    fn header_at(
        true_first_clk6: u8,
        elapsed: i64,
        data10: u16,
        true_uap: u8,
    ) -> [bool; HEADER_BITS] {
        let this_clk6 = ((true_first_clk6 as i64 + elapsed).rem_euclid(64)) as u8;
        let hec = hec_for(data10, true_uap);
        synthetic_header(data10, hec, this_clk6).0
    }

    /// [`PiconetClock`], honestly: several headers from the same LAP, real elapsed CLK1-6
    /// ticks between them, narrow from 64 down to exactly two candidates -
    /// which the same headers, examined without elapsed time, provably
    /// cannot do at all (see this module's own doc for the measurement
    /// that found that out), and which never narrows to fewer than two no
    /// matter how many more headers follow (the other measurement the
    /// same doc records).
    #[test]
    fn several_headers_narrow_via_elapsed_time_to_exactly_two_candidates() {
        let true_uap = 0x9au8;
        let true_first_clk6 = 41u8;
        let mut clock = PiconetClock::new();
        let observations: [(i64, u16); 8] = [
            (0, 0b0_100_011_101),
            (17, 0b1_010_100_010),
            (40, 0b0_001_010_111),
            (103, 0b1_111_001_000),
            (161, 0b0_000_111_010),
            (222, 0b1_101_000_101),
            (275, 0b0_110_101_100),
            (338, 0b1_001_010_011),
        ];
        for &(elapsed, data10) in &observations {
            let whitened = header_at(true_first_clk6, elapsed, data10, true_uap);
            clock.observe(elapsed as f64 * SLOT_US, &whitened);
        }
        let narrowed = clock.narrowed();
        assert!(
            narrowed.contains(&true_uap),
            "true uap {true_uap:#04x} missing from {narrowed:?}"
        );
        assert_eq!(
            narrowed.len(),
            2,
            "expected the measured floor of two candidates, got {narrowed:?}"
        );
    }

    /// A capture that starts mid-piconet - the very first header this
    /// instance sees is not really the piconet's own first packet - still
    /// narrows correctly, because nothing here needs to know the piconet's
    /// true clock origin, only ticks relative to whichever header arrives
    /// first.
    #[test]
    fn a_capture_starting_mid_piconet_still_narrows() {
        let true_uap = 0x33u8;
        let true_first_clk6 = 5u8;
        let mut clock = PiconetClock::new();
        // The caller's own "tick 0" is an arbitrary large offset - this
        // instance never sees the piconet's real origin.
        let base = 900_000i64;
        let observations: [(i64, u16); 6] = [
            (base, 0b1_000_110_001),
            (base + 30, 0b0_101_001_110),
            (base + 88, 0b1_010_111_000),
            (base + 145, 0b0_001_100_101),
            (base + 210, 0b1_111_010_010),
            (base + 260, 0b0_100_001_111),
        ];
        for &(tick, data10) in &observations {
            let whitened = header_at(true_first_clk6, tick, data10, true_uap);
            clock.observe(tick as f64 * SLOT_US, &whitened);
        }
        assert!(
            clock.narrowed().contains(&true_uap),
            "{:?}",
            clock.narrowed()
        );
    }

    /// A LAP whose first few "headers" are not really one consistent
    /// piconet (a false access-code detection, most plausibly) does not
    /// stay stuck refusing forever - it re-seeds from whichever header
    /// comes next, and a real, consistent run starting there still
    /// narrows correctly.
    #[test]
    fn an_inconsistent_start_recovers_once_real_headers_follow() {
        let mut clock = PiconetClock::new();
        // Two unrelated, mutually inconsistent "headers" - random noise
        // that happened to pass the FEC quality gate, not a real piconet.
        clock.observe(0.0, &[true; HEADER_BITS]);
        clock.observe(5.0 * SLOT_US, &[false; HEADER_BITS]);

        // Now a real, consistent piconet starts - re-seeded from here.
        let true_uap = 0x64u8;
        let true_first_clk6 = 12u8;
        let observations: [(i64, u16); 6] = [
            (100, 0b1_100_010_001),
            (140, 0b0_011_101_110),
            (190, 0b1_000_111_010),
            (240, 0b0_110_001_101),
            (300, 0b1_001_010_000),
            (355, 0b0_101_100_011),
        ];
        for &(tick, data10) in &observations {
            let whitened = header_at(true_first_clk6, tick, data10, true_uap);
            clock.observe(tick as f64 * SLOT_US, &whitened);
        }
        assert!(
            clock.narrowed().contains(&true_uap),
            "{:?}",
            clock.narrowed()
        );
    }

    /// Before any header at all, nothing is narrowed - an empty answer,
    /// not a fabricated one.
    #[test]
    fn nothing_narrowed_before_the_first_header() {
        assert!(PiconetClock::new().narrowed().is_empty());
    }
    /// **What the air showed: CLK1-6 steps once a slot, 625 us.** A piconet
    /// sending a header every slot, its CLK1-6 moving one step each time,
    /// on a clock of ours that its own sits 16 ppm from (measured on a real
    /// device), with 25 us of jitter either way on every header, for a
    /// minute: the true hypothesis is never lost. Counted at the full CLK
    /// rate, or rounded from the first header, it would be within seconds.
    #[test]
    fn the_clock_steps_once_a_slot_through_jitter_and_drift() {
        let true_uap = 0x67u8;
        let first = 30u8;
        let mut clock = PiconetClock::new();
        // An arbitrary stream origin and a phase inside the slot: neither
        // may matter.
        let origin_us = 123_456.7;
        let mut slot = 0i64;
        let mut lost = 0;
        let mut widest_after_converging = 0;
        for k in 0..1_200u64 {
            // A header every 80 slots (50 ms), give or take a few.
            slot += 80 + (k % 7) as i64;
            let jitter = ((k * 7919) % 51) as f64 - 25.0;
            let at_us = origin_us + slot as f64 * SLOT_US * (1.0 + 16e-6) + jitter;
            let data10 = ((k * 613) % 1024) as u16;
            let whitened = header_at(first, slot, data10, true_uap);
            clock.observe(at_us, &whitened);
            if !clock.narrowed().contains(&true_uap) {
                lost += 1;
            }
            // A lost hypothesis does not show as a missing UAP: the clock
            // reseeds, and a fresh seed holds the true UAP again among 32.
            // It shows as the candidates widening after they had narrowed.
            if k >= 100 {
                widest_after_converging = widest_after_converging.max(clock.narrowed().len());
            }
        }
        assert_eq!(lost, 0, "the true UAP fell out {lost} times");
        assert_eq!(
            widest_after_converging, 2,
            "the clock reseeded after converging"
        );
        assert_eq!(clock.narrowed().len(), 2, "{:?}", clock.narrowed());
    }

    /// Two headers one slot apart: CLK1-6 has moved one step, not two.
    #[test]
    fn one_slot_is_one_step() {
        let uap = 0x3au8;
        let mut clock = PiconetClock::new();
        clock.observe(1_000.0, &header_at(24, 0, 0b0_011_000_101, uap));
        clock.observe(1_000.0 + SLOT_US, &header_at(24, 1, 0b1_100_001_010, uap));
        assert_eq!(clock.clocks_for(uap, 1_000.0 + SLOT_US), vec![25]);
    }

    /// Each code names every packet it can be, the basic-rate name first,
    /// and the codes that mean one thing everywhere name one.
    #[test]
    fn a_code_names_every_packet_it_can_be() {
        assert_eq!(PacketType::from_code(10).shown(), "DM3/2-DH3");
        assert_eq!(PacketType::from_code(14).shown(), "DM5/2-DH5");
        assert_eq!(PacketType::from_code(7).shown(), "HV3/EV3/3-EV3");
        for code in [0u8, 1, 2, 3, 5, 9] {
            assert_eq!(PacketType::from_code(code).names().len(), 1, "{code}");
        }
        for code in 0..16u8 {
            let t = PacketType::from_code(code);
            assert_eq!(t.names()[0], t.label(), "{code}: basic rate first");
        }
    }
}
