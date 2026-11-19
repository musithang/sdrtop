// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Classic Bluetooth's payload CRC-16. `PiconetClock` narrows a LAP's UAP to
//! exactly two candidates, measured as a genuine floor a header alone cannot
//! break. `libbtbb`'s own `crc_check` breaks that same tie using the
//! *payload's* CRC, a register run long enough that the header's own algebra no
//! longer leaves a twin standing. [`break_uap_tie`] is this module's own
//! version of that.
//!
//! **Scope: the six ACL data types.** DH1/DH3/DH5 carry their payload with
//! no FEC, so decoding one is whitening plus a CRC-16 - the same shape of
//! work [`super::header`] already did for the header. DM1/DM3/DM5 carry it
//! under the rate 2/3 FEC, a (15,10) shortened Hamming code: [`unfec23`]
//! first, on the whitened stream, then the same dewhitening and CRC, since
//! a transmitter whitens before it encodes. The voice-carrying and control
//! types (FHS, HV1-3, DV, EV4-5) stay out of scope.
//!
//! **The FEC's table is worked out, not typed.** [`FEC23_COLUMNS`] is
//! derived from the generator polynomial, g(D) = (D+1)(D^4+D+1), and a test
//! holds the result to `libbtbb`'s own hand-written `fec23_gen_matrix`: the
//! polynomial as the secondary literature on the baseband gives it, the
//! table as the port's source has it, and a test standing between them.
//!
//! **Not read from the Bluetooth Core Specification itself** -
//! the same standing every fact in [`super::header`] has. Ported instead
//! from `libbtbb` (<https://github.com/greatscottgadgets/libbtbb>,
//! `lib/src/bluetooth_packet.c`: `crcgen`, `decode_payload_header`, `DH`,
//! `payload_crc`), GPL-2.0-or-later, the same standing [`super::header`]'s
//! own port already has.
//!
//! **The bytes are kept.** [`read_payload`] hands back the body and its
//! LLID beside the CRC verdict, because a link manager message (LLID 0b11)
//! is read from them; a body whose CRC failed comes back too, marked so.
//!
//! **Wired to a live receiver the same day, unlike [`super::header`]'s own
//! first landing.** `signal::bt::receive::Receiver` captures a fixed,
//! generous window of raw bits after every header (DH5's own worst-case
//! payload length, regardless of what type the header actually turns out
//! to name - a lane cannot know that without a confirmed UAP), and
//! `signal::net::worker` calls [`break_uap_tie`] whenever a LAP's own
//! `PiconetClock` has more than one candidate still standing, keeping the
//! answer once it resolves rather than re-deriving it every header
//! (`resolved_bt_uap`'s own doc there says why).

use super::header::{self, HEADER_BITS};

/// A payload's own header: two or three fields packed into the first byte
/// (single-slot packets) or two bytes (multi-slot) of the payload region -
/// `libbtbb`'s own `decode_payload_header`, but only as much of it as
/// [`read_payload`] needs (FLOW is carried through for a future consumer;
/// nothing here reads it yet).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PayloadHeader {
    pub llid: u8,
    pub flow: bool,
    /// Total payload region length in bytes - the payload header itself,
    /// the body, and the trailing 2-byte CRC, all three included. This is
    /// `libbtbb`'s own convention (`pkt->payload_length`), not just the
    /// body length the LENGTH field's raw value names.
    pub payload_length: usize,
}

/// How many bits DH1's own payload header is - one byte, LLID(2) FLOW(1)
/// LENGTH(5) - versus DH3/DH5's two bytes, LLID(2) FLOW(1) LENGTH(10) (the
/// three bits between FLOW and LENGTH that do not appear in either sum are
/// `libbtbb`'s own reading of the field, not something this port adds
/// meaning to). `None` for every packet type this module does not decode -
/// a scope decision, not a limit of the algorithm itself.
pub fn payload_header_bits(packet_type: header::PacketType) -> Option<usize> {
    use header::PacketType::*;
    match packet_type {
        Dh1 | Dm1 => Some(8),
        Dh3 | Dh5 | Dm3 | Dm5 => Some(16),
        _ => None,
    }
}

/// Whether a type's payload travels under the rate 2/3 FEC.
pub fn fec23_protected(packet_type: header::PacketType) -> bool {
    use header::PacketType::*;
    matches!(packet_type, Dm1 | Dm3 | Dm5)
}

/// Raw air bits that carry `data_bits` of payload: the same number without
/// FEC, and 15 for every 10 (the last codeword padded) with it.
pub fn raw_bits_for(packet_type: header::PacketType, data_bits: usize) -> usize {
    if fec23_protected(packet_type) {
        data_bits.div_ceil(10) * 15
    } else {
        data_bits
    }
}

/// The (15,10) code's generator polynomial, D^5 + D^4 + D^2 + 1, which is
/// (D+1)(D^4+D+1): bit `k` the coefficient of D^k.
const FEC23_G: u32 = 0b11_0101;

/// The five parity bits each data bit contributes on its own, bit `j` the
/// `j`-th parity bit on the air. A codeword's parity is the XOR of the
/// columns of its set data bits, and a single flipped data bit shows up as
/// its own column in the syndrome.
///
/// The first data bit on the air is the highest-degree coefficient, as a
/// shift-register encoder sends it: parity = D^5 d(D) mod g(D), its
/// coefficients sent from D^4 down. Every column has odd weight (the
/// factor D+1), so a double error's syndrome is even and matches no column
/// and no single parity bit: it is caught, never "corrected" into other
/// data.
pub(crate) const FEC23_COLUMNS: [u8; 10] = fec23_columns();

const fn fec23_columns() -> [u8; 10] {
    let mut columns = [0u8; 10];
    let mut i = 0;
    while i < 10 {
        // D^(9-i) shifted up by the five parity places, reduced mod g.
        let mut poly: u32 = 1 << (9 - i + 5);
        let mut degree = 14;
        while degree >= 5 {
            if poly & (1 << degree) != 0 {
                poly ^= FEC23_G << (degree - 5);
            }
            degree -= 1;
        }
        // Coefficient of D^4 goes on the air first.
        let mut parity = 0u8;
        let mut j = 0;
        while j < 5 {
            if poly & (1 << (4 - j)) != 0 {
                parity |= 1 << j;
            }
            j += 1;
        }
        columns[i] = parity;
        i += 1;
    }
    columns
}

/// Undo the rate 2/3 FEC: `data_bits` of data out of the codewords that
/// carry them, one data error per codeword corrected. `None` when a
/// codeword holds an error the code cannot place (two or more), or when
/// `raw` is shorter than the codewords needed.
///
/// `libbtbb`'s own `unfec23`, the table derived instead of typed.
pub(crate) fn unfec23(raw: &[bool], data_bits: usize) -> Option<Vec<bool>> {
    let codewords = data_bits.div_ceil(10);
    if raw.len() < codewords * 15 {
        return None;
    }
    let mut out = Vec::with_capacity(codewords * 10);
    for word in raw.as_chunks::<15>().0.iter().take(codewords) {
        let mut data = [false; 10];
        data.copy_from_slice(&word[..10]);
        let received = header::pack_bits(&word[10..15]) as u8;
        let expected = data
            .iter()
            .zip(FEC23_COLUMNS)
            .filter(|(bit, _)| **bit)
            .fold(0u8, |p, (_, column)| p ^ column);
        let syndrome = received ^ expected;
        // One bit set: a parity bit took the error and the data is whole.
        if syndrome.count_ones() > 1 {
            let at = FEC23_COLUMNS.iter().position(|&c| c == syndrome)?;
            data[at] = !data[at];
        }
        out.extend(data);
    }
    out.truncate(data_bits);
    Some(out)
}

/// The largest legal total payload length (header + body + CRC, in bytes)
/// for each supported type - `libbtbb`'s own per-type `max_length`, guarding
/// against a corrupted LENGTH field claiming an impossible size rather than
/// trusting it outright.
pub fn max_payload_length(packet_type: header::PacketType) -> Option<usize> {
    use header::PacketType::*;
    match packet_type {
        Dh1 => Some(30),
        Dh3 => Some(187),
        Dh5 => Some(343),
        Dm1 => Some(20),
        Dm3 => Some(125),
        Dm5 => Some(228),
        _ => None,
    }
}

/// Decode a payload header already dewhitened - `bits.len()` must be
/// exactly 8 or 16, [`payload_header_bits`]'s own two legal answers.
pub fn decode_payload_header(bits: &[bool]) -> Option<PayloadHeader> {
    let llid = header::pack_bits(&bits[0..2]) as u8;
    let flow = bits[2];
    let payload_length = match bits.len() {
        16 => header::pack_bits(&bits[3..13]) as usize + 4,
        8 => header::pack_bits(&bits[3..8]) as usize + 3,
        _ => return None,
    };
    Some(PayloadHeader {
        llid,
        flow,
        payload_length,
    })
}

/// `libbtbb`'s own `crcgen`: a CRC-16 register seeded from the piconet's
/// UAP (reversed into the top byte) rather than the usual all-zero or
/// all-one start, run bit by bit over the payload's own header and body.
/// The register's final value is compared directly against the trailing 16
/// bits the packet itself carries - [`verify_crc`]'s own job, not this
/// function's. `pub(crate)` rather than private so `signal::bt::receive`'s
/// own tests can build a real, correctly-CRC'd synthetic payload through
/// the full GFSK chain, the same reason `header::uap_from_hec` and
/// friends are `pub(crate)` rather than private.
pub(crate) fn crcgen(bits: &[bool], uap: u8) -> u16 {
    let mut reg: u16 = (header::reverse_bits(uap) as u16) << 8;
    for &bit in bits {
        let fed_bit = (reg & 0x0001) ^ (bit as u16);
        reg = (reg >> 1) | (fed_bit << 15);
        reg ^= (reg & 0x8000) >> 5;
        reg ^= (reg & 0x8000) >> 12;
    }
    reg
}

/// Why a payload's CRC could not be checked. The reasons mean different
/// things on the air, so they are kept apart rather than folded into one
/// "not read".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unchecked {
    /// A type sdrtop has no payload reader for (HV, DV, EV, FHS, AUX1).
    NoReader,
    /// The capture ended before the length the payload's own header claims:
    /// the window, a block edge, or a block the feed lost.
    CutShort,
    /// A rate 2/3 codeword the FEC cannot place: a damaged capture, or a
    /// payload that is not basic rate at all, which the header alone cannot
    /// tell apart.
    FecFailed,
    /// A payload header whose length cannot hold a CRC.
    BadLength,
}

impl Unchecked {
    /// The words the packet list prints.
    pub fn words(self) -> &'static str {
        match self {
            Unchecked::NoReader => "type not read",
            Unchecked::CutShort => "cut short",
            Unchecked::FecFailed => "FEC failed",
            Unchecked::BadLength => "bad length",
        }
    }
}

/// A payload whose CRC was checked: its LLID and body, as sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Payload {
    pub crc_ok: bool,
    pub llid: u8,
    /// The body between the payload header and the CRC, bytes in the
    /// order sent, each byte's first bit its least significant.
    pub body: Vec<u8>,
}

/// Read a captured payload region against a specific CLK1-6/UAP guess,
/// dewhitening as it goes - the header/payload's shared whitening stream,
/// continued from bit [`HEADER_BITS`] rather than restarted (see
/// [`header::unwhiten_at`]'s own doc) - and check its CRC-16.
///
/// `raw` is the still-whitened payload region, starting from its own first
/// bit (the payload header's own first bit), in whatever length the caller
/// happened to capture - this function reads only as much of it as the
/// payload header itself says the packet actually is.
///
/// The bytes come back whether the CRC passed or not, and
/// [`Payload::crc_ok`] says which; where no verdict is possible, the
/// reason ([`Unchecked`]), never a verdict invented when the data to
/// compute it is missing (`POLICY.md` rule 2).
pub fn read_payload(
    raw: &[bool],
    clk6: u8,
    packet_type: header::PacketType,
    uap: u8,
) -> Result<Payload, Unchecked> {
    let header_bits_len = payload_header_bits(packet_type).ok_or(Unchecked::NoReader)?;
    let max_len = max_payload_length(packet_type).ok_or(Unchecked::NoReader)?;
    // Data bits out of the raw capture: as they are, or through the FEC.
    // A codeword the FEC cannot place is no verdict, not a failed CRC: it
    // says the capture is damaged, whichever UAP is being tried.
    let data = |bits: usize| -> Result<Vec<bool>, Unchecked> {
        let raw_bits = raw_bits_for(packet_type, bits);
        if raw.len() < raw_bits {
            return Err(Unchecked::CutShort);
        }
        if fec23_protected(packet_type) {
            unfec23(&raw[..raw_bits], bits).ok_or(Unchecked::FecFailed)
        } else {
            Ok(raw[..bits].to_vec())
        }
    };
    let dewhitened_header = header::unwhiten_at(&data(header_bits_len)?, clk6, HEADER_BITS);
    let payload_header = decode_payload_header(&dewhitened_header).ok_or(Unchecked::BadLength)?;
    let payload_length = payload_header.payload_length.min(max_len);
    let total_bits = payload_length * 8;
    // The payload header and the CRC both have to fit, or there is no body
    // between them to hand back.
    if total_bits < header_bits_len + 16 {
        return Err(Unchecked::BadLength);
    }
    let dewhitened = header::unwhiten_at(&data(total_bits)?, clk6, HEADER_BITS);
    let received = header::pack_bits(&dewhitened[total_bits - 16..total_bits]);
    let computed = crcgen(&dewhitened[..total_bits - 16], uap);
    let body = dewhitened[header_bits_len..total_bits - 16]
        .chunks(8)
        .map(|b| header::pack_bits(b) as u8)
        .collect();
    Ok(Payload {
        crc_ok: received == computed,
        llid: payload_header.llid,
        body,
    })
}

/// [`read_payload`]'s CRC verdict alone, or the reason there is none.
pub fn check_crc(
    raw: &[bool],
    clk6: u8,
    packet_type: header::PacketType,
    uap: u8,
) -> Result<bool, Unchecked> {
    read_payload(raw, clk6, packet_type, uap).map(|p| p.crc_ok)
}

/// [`check_crc`]'s verdict, or `None` whatever the reason: for a caller
/// that only needs to know whether the CRC passed, as the UAP tie-break.
pub fn verify_crc(
    raw: &[bool],
    clk6: u8,
    packet_type: header::PacketType,
    uap: u8,
) -> Option<bool> {
    check_crc(raw, clk6, packet_type, uap).ok()
}

/// Break [`super::header::PiconetClock`]'s own measured two-candidate
/// floor, using one packet's payload rather than another header.
///
/// `candidates` are `(UAP, CLK1-6)` pairs: each UAP still standing, at each
/// clock the piconet's own clock gives it for this header
/// ([`header::PiconetClock::clocks_for`]). **The clock is not searched
/// for.** About one header in five has a second clock giving the same UAP,
/// under which it reads as another packet type; taking the first clock
/// that fitted read those headers wrong and checked their payloads against
/// the wrong whitening. Each pair's header is read at its clock, and its
/// payload checked at the same clock.
///
/// Returns the one `(UAP, CLK1-6)` whose payload checks out: the UAP, and
/// the clock this header was sent at, which is what lets its piconet's
/// clock be pinned to one hypothesis. `None` when none does (an unsupported
/// type, a damaged or short capture), and `None` too when more than one
/// does: that is still a tie, never a pick.
pub fn break_uap_tie(
    candidates: &[(u8, u8)],
    header_whitened: &[bool; HEADER_BITS],
    payload_raw: &[bool],
) -> Option<(u8, u8)> {
    let mut passed = candidates.iter().filter(|&&(uap, clk6)| {
        header::decode_at(header_whitened, uap, clk6)
            .is_some_and(|h| verify_crc(payload_raw, clk6, h.packet_type, uap) == Some(true))
    });
    let first = *passed.next()?;
    passed.next().is_none().then_some(first)
}

#[cfg(test)]
mod tests {
    use super::*;
    use header::PacketType;

    /// Bits of a `u16`, air order (bit `i` to bit `i`) - the reverse of
    /// [`header::pack_bits`], needed only to build synthetic test fixtures.
    fn bits_of_u16(value: u16, count: usize) -> Vec<bool> {
        (0..count).map(|i| (value >> i) & 1 != 0).collect()
    }

    fn bits_of_u8(value: u8, count: usize) -> Vec<bool> {
        bits_of_u16(value as u16, count)
    }

    /// Build a synthetic, still-whitened DH1/DH3/DH5 payload region from a
    /// chosen body, and the whitened 18-bit header it belongs to (host
    /// fields chosen freely; the UAP that makes the header consistent is
    /// derived from them by `uap_from_hec` itself, matching
    /// `header.rs`'s own test discipline rather than a second, uncited
    /// forward algorithm).
    fn synthetic_packet(
        packet_type: PacketType,
        clk6: u8,
        uap: u8,
        body: &[u8],
    ) -> ([bool; HEADER_BITS], Vec<bool>) {
        synthetic_with_llid(packet_type, clk6, uap, 0b10, body)
    }

    /// [`synthetic_packet`] with the payload header's LLID chosen: 0b10 is
    /// L2CAP data, 0b11 a link manager message.
    fn synthetic_with_llid(
        packet_type: PacketType,
        clk6: u8,
        uap: u8,
        llid: u8,
        body: &[u8],
    ) -> ([bool; HEADER_BITS], Vec<bool>) {
        assert!(
            payload_header_bits(packet_type).is_some(),
            "test helper only knows the ACL data types"
        );
        let type_bits = packet_type.code();
        let lt_addr = 0b011u8;
        let flags = 0b101u8;
        let data10 = (lt_addr as u16) | ((type_bits as u16) << 3) | ((flags as u16) << 7);
        let hec = (0u16..256)
            .map(|h| h as u8)
            .find(|&h| header::uap_from_hec(data10, h) == uap)
            .expect("uap_from_hec is a bijection in hec for a fixed data10");
        let mut header_host = [false; HEADER_BITS];
        for (i, slot) in header_host[0..10].iter_mut().enumerate() {
            *slot = (data10 >> i) & 1 != 0;
        }
        for (i, slot) in header_host[10..18].iter_mut().enumerate() {
            *slot = (hec >> i) & 1 != 0;
        }
        let header_whitened: [bool; HEADER_BITS] = header::unwhiten_at(&header_host, clk6, 0)
            .try_into()
            .unwrap();

        let header_bits_len = payload_header_bits(packet_type).unwrap();
        let mut host = Vec::new();
        let flow = false;
        if header_bits_len == 16 {
            host.extend(bits_of_u8(llid, 2));
            host.push(flow);
            host.extend(bits_of_u16(body.len() as u16, 10));
            host.extend(std::iter::repeat_n(false, 3));
        } else {
            host.extend(bits_of_u8(llid, 2));
            host.push(flow);
            host.extend(bits_of_u8(body.len() as u8, 5));
        }
        for &byte in body {
            host.extend(bits_of_u8(byte, 8));
        }
        let crc = crcgen(&host, uap);
        host.extend(bits_of_u16(crc, 16));

        let payload_whitened = header::unwhiten_at(&host, clk6, HEADER_BITS);
        // A transmitter whitens, then encodes.
        let payload_raw = if fec23_protected(packet_type) {
            fec23_encode(&payload_whitened)
        } else {
            payload_whitened
        };
        (header_whitened, payload_raw)
    }

    /// The rate 2/3 encoder, for fixtures: each 10 data bits followed by
    /// their five parity bits, the last word padded with zeros.
    fn fec23_encode(data: &[bool]) -> Vec<bool> {
        let mut out = Vec::new();
        for chunk in data.chunks(10) {
            let mut word = [false; 10];
            word[..chunk.len()].copy_from_slice(chunk);
            let parity = word
                .iter()
                .zip(FEC23_COLUMNS)
                .filter(|(bit, _)| **bit)
                .fold(0u8, |p, (_, column)| p ^ column);
            out.extend(word);
            out.extend((0..5).map(|j| (parity >> j) & 1 != 0));
        }
        out
    }

    /// **The table worked out from g(D) is `libbtbb`'s table.** Its
    /// `fec23_gen_matrix`, parity part (`>> 10`), as the port's source has it.
    #[test]
    fn the_fec_table_from_the_polynomial_is_the_ported_table() {
        const LIBBTBB_GEN_MATRIX: [u16; 10] = [
            0x2c01, 0x5802, 0x1c04, 0x3808, 0x7010, 0x4c20, 0x3440, 0x6880, 0x7d00, 0x5600,
        ];
        let from_libbtbb = LIBBTBB_GEN_MATRIX.map(|row| (row >> 10) as u8);
        assert_eq!(FEC23_COLUMNS, from_libbtbb);
        assert!(FEC23_COLUMNS.iter().all(|c| c.count_ones() % 2 == 1));
    }

    /// Every single error in a codeword is corrected, in the data or in the
    /// parity; every double error is caught and refused, never corrected
    /// into other data.
    #[test]
    fn one_error_a_codeword_is_corrected_and_two_are_refused() {
        let data: Vec<bool> = (0..30).map(|i| (i * 7) % 3 == 0).collect();
        let clean = fec23_encode(&data);
        assert_eq!(unfec23(&clean, 30).as_deref(), Some(&data[..]));
        for word in 0..3 {
            for a in 0..15 {
                let mut one = clean.clone();
                one[word * 15 + a] = !one[word * 15 + a];
                assert_eq!(
                    unfec23(&one, 30).as_deref(),
                    Some(&data[..]),
                    "one error at {a}"
                );
                for b in a + 1..15 {
                    let mut two = one.clone();
                    two[word * 15 + b] = !two[word * 15 + b];
                    assert_eq!(unfec23(&two, 30), None, "two errors at {a} and {b}");
                }
            }
        }
        assert_eq!(
            unfec23(&clean[..44], 30),
            None,
            "one bit short of three codewords"
        );
    }

    /// DM1, DM3 and DM5 verify through the FEC like their DH twins do bare,
    /// fail on the wrong UAP, and survive one error in every codeword.
    #[test]
    fn dm_payloads_verify_through_the_fec() {
        for (pt, len) in [
            (PacketType::Dm1, 17),
            (PacketType::Dm3, 121),
            (PacketType::Dm5, 224),
        ] {
            let body: Vec<u8> = (0..len).map(|i| (i * 37 + 11) as u8).collect();
            let (_, payload) = synthetic_packet(pt, 29, 0x6d, &body);
            assert_eq!(verify_crc(&payload, 29, pt, 0x6d), Some(true), "{pt:?}");
            assert_eq!(
                verify_crc(&payload, 29, pt, 0x6e),
                Some(false),
                "{pt:?} wrong UAP"
            );
            let mut hit = payload.clone();
            for word in 0..hit.len() / 15 {
                let at = word * 15 + word % 15;
                hit[at] = !hit[at];
            }
            assert_eq!(
                verify_crc(&hit, 29, pt, 0x6d),
                Some(true),
                "{pt:?} one error a word"
            );
        }
    }

    /// A damaged codeword is no verdict, and so is a capture that has not
    /// reached the CRC's codewords yet.
    #[test]
    fn a_dm_payload_the_fec_cannot_mend_or_has_not_reached_is_refused() {
        let (_, payload) = synthetic_packet(PacketType::Dm3, 5, 0x21, &[0x42; 40]);
        let mut two = payload.clone();
        two[31] = !two[31];
        two[33] = !two[33];
        assert_eq!(verify_crc(&two, 5, PacketType::Dm3, 0x21), None);
        let short = &payload[..payload.len() - 1];
        assert_eq!(verify_crc(short, 5, PacketType::Dm3, 0x21), None);
    }

    /// A payload left unchecked says why, because the reasons mean
    /// different things on the air: a codeword the FEC cannot place (a
    /// damaged capture, or a payload that is not basic rate at all), a
    /// capture that ended before the packet did, and a type with no reader.
    /// None of them is "PSK": that is a guess about the link, not a
    /// reading of the packet.
    #[test]
    fn an_unchecked_payload_says_why() {
        let (_, payload) = synthetic_packet(PacketType::Dm3, 5, 0x21, &[0x42; 40]);
        assert_eq!(check_crc(&payload, 5, PacketType::Dm3, 0x21), Ok(true));
        let mut two = payload.clone();
        two[31] = !two[31];
        two[33] = !two[33];
        assert_eq!(
            check_crc(&two, 5, PacketType::Dm3, 0x21),
            Err(Unchecked::FecFailed)
        );
        let short = &payload[..payload.len() - 1];
        assert_eq!(
            check_crc(short, 5, PacketType::Dm3, 0x21),
            Err(Unchecked::CutShort)
        );
        assert_eq!(
            check_crc(&payload, 5, PacketType::Hv3, 0x21),
            Err(Unchecked::NoReader)
        );
        assert_eq!(
            verify_crc(short, 5, PacketType::Dm3, 0x21),
            None,
            "the old form"
        );
    }

    /// The tie breaks on a DM packet as it does on a DH one.
    #[test]
    fn break_uap_tie_reads_a_dm_payload_too() {
        let true_uap = 0x3au8;
        let (header_whitened, payload) =
            synthetic_packet(PacketType::Dm3, 44, true_uap, &[0x5a; 50]);
        let decoy = header::candidate_uaps(&header_whitened)
            .iter()
            .copied()
            .find(|&u| u != true_uap)
            .unwrap();
        assert_eq!(
            break_uap_tie(
                &header::pairs_for(&header_whitened, &[decoy, true_uap]),
                &header_whitened,
                &payload
            ),
            Some((true_uap, 44)),
            "the UAP, and the clock it was sent at, not the other clock that fits"
        );
    }

    /// A passing payload hands back its LLID and body as sent, for DM1
    /// through the FEC and for DH1 bare.
    #[test]
    fn a_passing_payload_keeps_its_bytes() {
        let body = [0x4e, 0x10, 0x02];
        for pt in [PacketType::Dm1, PacketType::Dh1] {
            let (_, raw) = synthetic_with_llid(pt, 29, 0x6d, 0b11, &body);
            let got = read_payload(&raw, 29, pt, 0x6d).unwrap();
            assert!(got.crc_ok, "{pt:?}");
            assert_eq!(got.llid, 0b11, "{pt:?}");
            assert_eq!(got.body, body.to_vec(), "{pt:?}");
        }
    }

    /// A failed CRC still says so, with whatever the bytes were; the
    /// reasons a payload goes unchecked are the ones `check_crc` gives.
    #[test]
    fn a_failing_payload_is_not_ok_and_unchecked_ones_say_why() {
        let (_, raw) = synthetic_with_llid(PacketType::Dm1, 29, 0x6d, 0b11, &[1, 2]);
        assert!(
            !read_payload(&raw, 29, PacketType::Dm1, 0x6e)
                .unwrap()
                .crc_ok
        );
        assert_eq!(
            read_payload(&raw[..20], 29, PacketType::Dm1, 0x6d),
            Err(Unchecked::CutShort)
        );
        assert_eq!(
            read_payload(&raw, 29, PacketType::Hv1, 0x6d),
            Err(Unchecked::NoReader)
        );
    }

    #[test]
    fn a_genuine_payload_verifies_against_its_own_crc() {
        let (_, payload) = synthetic_packet(PacketType::Dh1, 17, 0x5c, &[0x11, 0x22, 0x33]);
        assert_eq!(verify_crc(&payload, 17, PacketType::Dh1, 0x5c), Some(true));
    }

    #[test]
    fn a_corrupted_body_bit_fails_the_crc() {
        let (_, mut payload) = synthetic_packet(PacketType::Dh1, 17, 0x5c, &[0x11, 0x22, 0x33]);
        // Flip a bit inside the body, well clear of the header and CRC.
        let flip = payload_header_bits(PacketType::Dh1).unwrap() + 4;
        payload[flip] = !payload[flip];
        assert_eq!(verify_crc(&payload, 17, PacketType::Dh1, 0x5c), Some(false));
    }

    #[test]
    fn dh3_and_dh5_use_the_two_byte_payload_header() {
        let (_, payload) = synthetic_packet(PacketType::Dh3, 40, 0x2a, &[0xaa; 20]);
        assert_eq!(verify_crc(&payload, 40, PacketType::Dh3, 0x2a), Some(true));

        let (_, payload) = synthetic_packet(PacketType::Dh5, 3, 0x81, &[0x00; 60]);
        assert_eq!(verify_crc(&payload, 3, PacketType::Dh5, 0x81), Some(true));
    }

    #[test]
    fn an_unsupported_packet_type_is_refused_not_guessed() {
        assert_eq!(verify_crc(&[false; 200], 0, PacketType::Fhs, 0), None);
    }

    #[test]
    fn a_short_capture_that_has_not_reached_the_crc_yet_is_refused() {
        let (_, payload) = synthetic_packet(PacketType::Dh1, 17, 0x5c, &[0x11, 0x22, 0x33]);
        // Long enough to decode the payload header's own LENGTH field, not
        // long enough to reach the trailing CRC it names.
        let short = &payload[..payload_header_bits(PacketType::Dh1).unwrap() + 8];
        assert_eq!(verify_crc(short, 17, PacketType::Dh1, 0x5c), None);
    }

    /// [`break_uap_tie`]: given a real header and
    /// payload, and a candidate list containing both the true UAP and a
    /// decoy this same header's own 64-candidate set genuinely produces
    /// under some other CLK1-6 (`PiconetClock`'s own measured floor - see
    /// `header.rs`'s doc), only the true UAP's own clock also makes the
    /// payload's CRC agree.
    #[test]
    fn break_uap_tie_picks_the_candidate_whose_payload_also_checks_out() {
        let true_uap = 0x5cu8;
        let clk6 = 17u8;
        let (header_whitened, payload) =
            synthetic_packet(PacketType::Dh1, clk6, true_uap, &[0x11, 0x22, 0x33]);

        let all_candidates = header::candidate_uaps(&header_whitened);
        let decoy_uap = all_candidates
            .iter()
            .copied()
            .find(|&u| u != true_uap)
            .expect("64 candidates for one header must include more than one distinct value");

        let tries = header::pairs_for(&header_whitened, &[true_uap, decoy_uap]);
        let winner = break_uap_tie(&tries, &header_whitened, &payload);
        assert_eq!(winner, Some((true_uap, clk6)));
    }

    #[test]
    fn break_uap_tie_finds_nothing_when_no_candidate_is_offered() {
        let (header_whitened, payload) =
            synthetic_packet(PacketType::Dh1, 17, 0x5c, &[0x11, 0x22, 0x33]);
        assert_eq!(break_uap_tie(&[], &header_whitened, &payload), None);
    }
}
