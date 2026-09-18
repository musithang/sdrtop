// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Classic Bluetooth's payload CRC-16 - B17, the task [`super::header`]'s
//! own doc named and deferred: `PiconetClock` narrows a LAP's UAP to
//! exactly two candidates, measured as a genuine floor a header alone
//! cannot break. `libbtbb`'s own `crc_check` breaks that same tie using the
//! *payload's* CRC, a register run long enough that the header's own
//! algebra no longer leaves a twin standing. [`break_uap_tie`] is this
//! module's own version of that.
//!
//! **Scope, decided rather than assumed: FEC-off packet types only.**
//! DH1/DH3/DH5 carry their payload with no 2/3-rate FEC, so decoding one is
//! whitening plus a CRC-16 - the same shape of work [`super::header`]
//! already did for the header. DM1/DM3/DM5 (2/3-rate FEC on the payload)
//! and the voice-carrying/control types (FHS, HV1-3, DV, EV4-5) are a
//! separate scope, deliberately left for a later checkpoint.
//!
//! **Not read from the Bluetooth Core Specification itself this session** -
//! the same standing every fact in [`super::header`] has. Ported instead
//! from `libbtbb` (<https://github.com/greatscottgadgets/libbtbb>,
//! `lib/src/bluetooth_packet.c`: `crcgen`, `decode_payload_header`, `DH`,
//! `payload_crc`), GPL-2.0-or-later, the same standing [`super::header`]'s
//! own port already has.
//!
//! **Not yet wired to a live receiver.** Nothing in `signal::bt::receive`
//! yet captures the bits after a header hit far enough to reach a payload's
//! own CRC, or knows how many bits that even is before the payload header
//! (captured first, one byte or two, packet-type dependent) says so -
//! [`super::header`]'s own "not yet wired" gap, one level up again.

use super::header::{self, HEADER_BITS};

/// A payload's own header: two or three fields packed into the first byte
/// (single-slot packets) or two bytes (multi-slot) of the payload region -
/// `libbtbb`'s own `decode_payload_header`, but only as much of it as
/// [`verify_crc`] actually needs (LLID and FLOW are carried through for a
/// future consumer; nothing here reads them yet).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
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
/// this session's own scope decision, not a limit of the algorithm itself.
#[allow(dead_code)]
pub fn payload_header_bits(packet_type: header::PacketType) -> Option<usize> {
    match packet_type {
        header::PacketType::Dh1 => Some(8),
        header::PacketType::Dh3 | header::PacketType::Dh5 => Some(16),
        _ => None,
    }
}

/// The largest legal total payload length (header + body + CRC, in bytes)
/// for each supported type - `libbtbb`'s own per-type `max_length`, guarding
/// against a corrupted LENGTH field claiming an impossible size rather than
/// trusting it outright.
#[allow(dead_code)]
pub fn max_payload_length(packet_type: header::PacketType) -> Option<usize> {
    match packet_type {
        header::PacketType::Dh1 => Some(30),
        header::PacketType::Dh3 => Some(187),
        header::PacketType::Dh5 => Some(343),
        _ => None,
    }
}

/// Decode a payload header already dewhitened - `bits.len()` must be
/// exactly 8 or 16, [`payload_header_bits`]'s own two legal answers.
#[allow(dead_code)]
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
/// function's.
#[allow(dead_code)]
fn crcgen(bits: &[bool], uap: u8) -> u16 {
    let mut reg: u16 = (header::reverse_bits(uap) as u16) << 8;
    for &bit in bits {
        let fed_bit = (reg & 0x0001) ^ (bit as u16);
        reg = (reg >> 1) | (fed_bit << 15);
        reg ^= (reg & 0x8000) >> 5;
        reg ^= (reg & 0x8000) >> 12;
    }
    reg
}

/// Check a captured payload region's own CRC-16 against a specific
/// CLK1-6/UAP guess, dewhitening as it goes - the header/payload's shared
/// whitening stream, continued from bit [`HEADER_BITS`] rather than
/// restarted (see [`header::unwhiten_at`]'s own doc).
///
/// `raw` is the still-whitened payload region, starting from its own first
/// bit (the payload header's own first bit), in whatever length the caller
/// happened to capture - this function reads only as much of it as the
/// payload header itself says the packet actually is.
///
/// Returns `None` when nothing here can render a verdict at all: an
/// unsupported packet type, or a `raw` shorter than the length the
/// payload's own header claims (not enough was captured yet to check).
/// `Some(true)`/`Some(false)` is the actual CRC verdict once one is
/// possible - never invented when the data to compute it is simply
/// missing, `POLICY.md` rule 2's own "what cannot be asked is refused, not
/// answered".
#[allow(dead_code)]
pub fn verify_crc(
    raw: &[bool],
    clk6: u8,
    packet_type: header::PacketType,
    uap: u8,
) -> Option<bool> {
    let header_bits_len = payload_header_bits(packet_type)?;
    let max_len = max_payload_length(packet_type)?;
    if raw.len() < header_bits_len {
        return None;
    }
    let dewhitened_header = header::unwhiten_at(&raw[..header_bits_len], clk6, HEADER_BITS);
    let payload_header = decode_payload_header(&dewhitened_header)?;
    let payload_length = payload_header.payload_length.min(max_len);
    let total_bits = payload_length * 8;
    if total_bits < 16 || raw.len() < total_bits {
        return None;
    }
    let dewhitened = header::unwhiten_at(&raw[..total_bits], clk6, HEADER_BITS);
    let received = header::pack_bits(&dewhitened[total_bits - 16..total_bits]);
    let computed = crcgen(&dewhitened[..total_bits - 16], uap);
    Some(received == computed)
}

/// Break [`super::header::PiconetClock`]'s own measured two-candidate
/// floor, using one packet's payload rather than another header.
///
/// For each candidate UAP, finds the CLK1-6 that reproduces this same
/// packet's header under it ([`header::decode_with_uap`]'s own search,
/// which also names the resulting packet type), then checks whether that
/// clock's own dewhitening of the payload also carries a correct CRC.
/// Exactly one candidate should pass on a genuine packet; returns it.
///
/// `None` when no candidate's own header decodes at all, none of the ones
/// that do name a packet type this module reads, or (vanishingly likely
/// with only two real candidates surviving `PiconetClock` in the first
/// place) neither one's own payload actually checks out - every one of
/// those is "still tied, honestly", never a guessed answer.
#[allow(dead_code)]
pub fn break_uap_tie(
    candidates: &[u8],
    header_whitened: &[bool; HEADER_BITS],
    payload_raw: &[bool],
) -> Option<u8> {
    let mut winner = None;
    for &uap in candidates {
        let Some(decoded) = header::decode_with_uap(header_whitened, uap) else {
            continue;
        };
        if verify_crc(payload_raw, decoded.clk6, decoded.packet_type, uap) == Some(true) {
            winner = Some(uap);
        }
    }
    winner
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
        let type_bits = match packet_type {
            PacketType::Dh1 => 4u8,
            PacketType::Dh3 => 11,
            PacketType::Dh5 => 15,
            _ => panic!("test helper only knows DH1/DH3/DH5"),
        };
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
        let llid = 0b10u8;
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
        (header_whitened, payload_whitened)
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

    /// [`break_uap_tie`]'s own exit condition: given a real header and
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

        let winner = break_uap_tie(&[true_uap, decoy_uap], &header_whitened, &payload);
        assert_eq!(winner, Some(true_uap));
    }

    #[test]
    fn break_uap_tie_finds_nothing_when_no_candidate_is_offered() {
        let (header_whitened, payload) =
            synthetic_packet(PacketType::Dh1, 17, 0x5c, &[0x11, 0x22, 0x33]);
        assert_eq!(break_uap_tie(&[], &header_whitened, &payload), None);
    }
}
