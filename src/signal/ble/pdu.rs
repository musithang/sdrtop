// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The advertising channel PDU: header, address and CRC, from a de-whitened
//! bit stream. `decode` and `measure`, in design section 10's split - this
//! reads what is there and checks it against the CRC it carries; nothing
//! here decides whether to trust a radio or corrects a frequency.
//!
//! Source: Bluetooth Core Specification, Vol 6, Part B - byte 0's 4-bit PDU
//! type, RFU, ChSel, TxAdd and RxAdd, byte 1's 6-bit Length, and the seven
//! legacy advertising PDU types. Cross-checked against public documentation
//! of the same layout; not read from a licensed copy of the specification
//! this session, the same standing B1's channel table has.

use crate::signal::dsp::code::crc::crc24_ble;
#[cfg(test)]
use crate::signal::dsp::code::lfsr::whiten;

/// The seven PDU types a legacy advertising channel packet can carry.
/// `Other` is not a defect in this list - PDU types 7 and up are extended
/// advertising (AUX_*, introduced after legacy advertising), out of scope
/// for this arc until a step says otherwise, and reporting the raw value
/// rather than refusing the packet is what rule 2 asks for: this is a real
/// four-bit field that was really read, not a case this decoder cannot see.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PduType {
    AdvInd,
    AdvDirectInd,
    AdvNonconnInd,
    ScanReq,
    ScanRsp,
    ConnectInd,
    AdvScanInd,
    Other(u8),
}

impl PduType {
    fn from_bits(bits: u8) -> Self {
        match bits & 0x0F {
            0x0 => Self::AdvInd,
            0x1 => Self::AdvDirectInd,
            0x2 => Self::AdvNonconnInd,
            0x3 => Self::ScanReq,
            0x4 => Self::ScanRsp,
            0x5 => Self::ConnectInd,
            0x6 => Self::AdvScanInd,
            other => Self::Other(other),
        }
    }

    /// The label a panel shows.
    pub fn label(self) -> String {
        match self {
            Self::AdvInd => "ADV_IND".to_string(),
            Self::AdvDirectInd => "ADV_DIRECT_IND".to_string(),
            Self::AdvNonconnInd => "ADV_NONCONN_IND".to_string(),
            Self::ScanReq => "SCAN_REQ".to_string(),
            Self::ScanRsp => "SCAN_RSP".to_string(),
            Self::ConnectInd => "CONNECT_IND".to_string(),
            Self::AdvScanInd => "ADV_SCAN_IND".to_string(),
            Self::Other(b) => format!("TYPE {b:#04x}"),
        }
    }

    /// Whether this PDU's first six payload bytes are `AdvA`, the
    /// advertiser's own address - true for every legacy type except
    /// `SCAN_REQ` and `CONNECT_IND`, whose first address is the scanner's or
    /// initiator's, not the advertiser's.
    fn carries_adv_addr_first(self) -> bool {
        matches!(
            self,
            Self::AdvInd
                | Self::AdvDirectInd
                | Self::AdvNonconnInd
                | Self::ScanRsp
                | Self::AdvScanInd
        )
    }
}

/// A decoded advertising channel PDU: the header, `AdvA` where the PDU type
/// says the payload starts with one, and whether the CRC that followed it
/// over the air actually checked out.
///
/// `snr_db`, `freq_offset_hz`, `modulation` and `drift` are `None` here
/// always - `decode` sees only bits, never the discriminator samples or the
/// detector's own coherence B7, B8 and B9's measurements are taken from -
/// and are filled in by `signal::ble::receive::Receiver::try_decode`, the
/// caller that has both.
#[derive(Clone, Debug, PartialEq)]
pub struct Packet {
    pub pdu_type: PduType,
    pub tx_add_random: bool,
    pub rx_add_random: bool,
    pub length: u8,
    pub adv_addr: Option<[u8; 6]>,
    pub crc_ok: bool,
    pub snr_db: Option<f64>,
    pub freq_offset_hz: Option<crate::signal::dsp::uncertainty::Uncertain>,
    pub drift: Option<super::measure::Drift>,
    pub modulation: Option<super::measure::ModulationQuality>,
}

/// How many trailing bits `decode` needs beyond the header to have a whole
/// PDU plus its CRC, once `length` is known.
fn body_bits(length: u8) -> usize {
    (length as usize + 3) * 8
}

/// How many bits, past the access address, `decode` needs to see before it
/// can be called at all: the 16-bit header alone.
pub const HEADER_BITS: usize = 16;

/// The whole PDU's own length in bits, header through CRC, once `length` is
/// known - the same figure `decode` requires before it returns `Some`.
///
/// `signal::ble::receive::Receiver` uses this to trim its own capture to
/// exactly the packet before measuring B8's modulation quality from it,
/// rather than re-deriving [`body_bits`]'s arithmetic a second time and
/// risking the two silently disagreeing.
pub fn used_bits(length: u8) -> usize {
    HEADER_BITS + body_bits(length)
}

/// Decode one advertising channel PDU from its de-whitened bits, in
/// transmission order, starting at the header's own first bit and running
/// through the header, the payload and the CRC.
///
/// `bits.len()` must be at least `HEADER_BITS + body_bits(length)` for the
/// `length` the header itself states - the caller reads the header's own six
/// length bits first (bits 8 to 13) to know how much more to wait for, which
/// is what a live receiver does and what this function's own tests do too.
/// Fewer bits than that is `None`: an incomplete PDU is not a wrong one, and
/// inventing a partial answer would be exactly the thing rule 2 refuses.
pub fn decode(bits: &[bool]) -> Option<Packet> {
    if bits.len() < HEADER_BITS {
        return None;
    }
    let byte = |bit_offset: usize| -> u8 {
        let mut b = 0u8;
        for i in 0..8 {
            if bits[bit_offset + i] {
                b |= 1 << i;
            }
        }
        b
    };
    let byte0 = byte(0);
    let byte1 = byte(8);
    let pdu_type = PduType::from_bits(byte0);
    let tx_add_random = (byte0 >> 6) & 1 != 0;
    let rx_add_random = (byte0 >> 7) & 1 != 0;
    let length = byte1 & 0x3F;

    let needed = used_bits(length);
    if bits.len() < needed {
        return None;
    }

    let pdu_bytes: Vec<u8> = (0..2 + length as usize).map(|i| byte(i * 8)).collect();
    let crc_bytes: Vec<u8> = (0..3)
        .map(|i| byte(HEADER_BITS + (length as usize) * 8 + i * 8))
        .collect();
    // The CRC is the one multi-octet field this specification sends most-
    // significant-octet-first - the stated exception to the rule every other
    // field here follows (see the module doc and `access_address_bits`).
    // `encode`'s own tests caught nothing because it wrote the same wrong
    // order it read back; only a real transmitter, which correctly follows
    // the exception, exposed it - every CRC failed on real hardware until
    // this matched.
    let received_crc =
        (crc_bytes[0] as u32) << 16 | (crc_bytes[1] as u32) << 8 | crc_bytes[2] as u32;
    let crc_ok = crc24_ble(&pdu_bytes) == received_crc;

    let adv_addr = if pdu_type.carries_adv_addr_first() && length >= 6 {
        let mut addr = [0u8; 6];
        for (i, a) in addr.iter_mut().enumerate() {
            *a = byte(HEADER_BITS + i * 8);
        }
        Some(addr)
    } else {
        None
    };

    Some(Packet {
        pdu_type,
        tx_add_random,
        rx_add_random,
        length,
        adv_addr,
        crc_ok,
        snr_db: None,
        freq_offset_hz: None,
        modulation: None,
        drift: None,
    })
}

/// A synthetic advertising channel PDU's bit stream, whitened and CRC'd
/// exactly as the air interface would send it - for this module's own tests
/// and for `signal::ble::receive`'s.
///
/// `channel` seeds the whitening the same way a real transmitter's would;
/// `header_byte0` and `payload` are given already assembled so a test can
/// build any PDU type or a deliberately corrupt one without this function
/// making decisions on its behalf.
///
/// `cfg(test)` rather than `#[allow(dead_code)]`: unlike `gfsk::modulate`,
/// which B3 promoted to production because a real detector needs to build a
/// reference from it, nothing on the receive side ever needs to construct a
/// packet - only decode one.
#[cfg(test)]
pub fn encode(channel: u8, header_byte0: u8, payload: &[u8]) -> Vec<bool> {
    let mut pdu_bytes = vec![header_byte0, payload.len() as u8];
    pdu_bytes.extend_from_slice(payload);
    let crc = crc24_ble(&pdu_bytes);
    let mut all_bytes = pdu_bytes;
    // Most-significant-octet-first: see `decode`'s own comment on the same
    // exception.
    all_bytes.push(((crc >> 16) & 0xFF) as u8);
    all_bytes.push(((crc >> 8) & 0xFF) as u8);
    all_bytes.push((crc & 0xFF) as u8);

    let mut bits = Vec::with_capacity(all_bytes.len() * 8);
    for byte in all_bytes {
        for i in 0..8 {
            bits.push((byte >> i) & 1 != 0);
        }
    }
    whiten(&mut bits, channel);
    bits
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A round trip through `encode` and `decode` recovers every field, on
    /// the PDU type that carries an address.
    #[test]
    fn a_synthetic_adv_ind_round_trips() {
        let addr = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66];
        let mut payload = addr.to_vec();
        payload.extend_from_slice(&[0x02, 0x01, 0x06]); // a plausible AD structure
        let mut bits = encode(37, 0x00, &payload); // 0x00 = ADV_IND, both address bits clear
        whiten(&mut bits, 37); // the air interface's own bits, de-whitened
        let packet = decode(&bits).expect("a full PDU was given");
        assert_eq!(packet.pdu_type, PduType::AdvInd);
        assert!(!packet.tx_add_random);
        assert!(!packet.rx_add_random);
        assert_eq!(packet.length, payload.len() as u8);
        assert_eq!(packet.adv_addr, Some(addr));
        assert!(packet.crc_ok);
    }

    /// `TxAdd` set is a random address, and the type without an `AdvA` at
    /// all - `SCAN_REQ` - reports no address even though it has payload.
    #[test]
    fn tx_add_and_addressless_types_are_read_correctly() {
        let mut bits = encode(0, 0x40 | 0x03, &[0u8; 12]); // bit6=TxAdd, type=SCAN_REQ
        whiten(&mut bits, 0);
        let packet = decode(&bits).unwrap();
        assert_eq!(packet.pdu_type, PduType::ScanReq);
        assert!(packet.tx_add_random);
        assert_eq!(packet.adv_addr, None);
    }

    /// A whitened bit flipped in the payload breaks the CRC, and only the
    /// CRC - the header still reads correctly, which is what lets a panel
    /// show a bad packet rather than discard it silently.
    #[test]
    fn a_corrupted_payload_fails_only_the_crc() {
        let mut bits = encode(10, 0x02, &[0xAA; 8]); // ADV_NONCONN_IND
        whiten(&mut bits, 10);
        let flip = HEADER_BITS + 20;
        bits[flip] = !bits[flip];
        let packet = decode(&bits).unwrap();
        assert_eq!(packet.pdu_type, PduType::AdvNonconnInd);
        assert_eq!(packet.length, 8);
        assert!(!packet.crc_ok);
    }

    /// Fewer bits than the header refuses rather than guessing.
    #[test]
    fn fewer_bits_than_the_header_decodes_to_nothing() {
        assert_eq!(decode(&[false; HEADER_BITS - 1]), None);
    }

    /// Enough for the header but not for the length it declares refuses too.
    #[test]
    fn a_header_promising_more_than_is_present_decodes_to_nothing() {
        let mut bits = encode(5, 0x00, &[0u8; 20]);
        whiten(&mut bits, 5);
        assert_eq!(decode(&bits[..HEADER_BITS + 8]), None);
    }

    /// An out-of-range PDU type is reported plainly, not refused: it is a
    /// real four-bit field that was really read.
    #[test]
    fn an_extended_advertising_type_is_named_rather_than_refused() {
        let mut bits = encode(0, 0x07, &[0u8; 4]);
        whiten(&mut bits, 0);
        let packet = decode(&bits).unwrap();
        assert_eq!(packet.pdu_type, PduType::Other(7));
    }
}
