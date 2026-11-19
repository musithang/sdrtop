// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The data channel PDU: what a connection's packets carry, from their
//! de-whitened bits.
//!
//! **Read from Core 5.4 Vol 6 Part B 2.4**, the SIG's HTML copy: "The Data
//! Physical Channel PDU has a 16 or 24 bit header, a variable size payload,
//! and may include a Message Integrity Check (MIC) field." Table 2.19 names
//! the header's fields in order, LLID, NESN, SN, MD, CP, RFU, Length and,
//! when CP is set, CTEInfo; "The Length field indicates the size, in octets,
//! of the Payload and MIC, if included", from 0 to 255. The field widths
//! are Figure 2.25's, an image, so they are reasoned rather than quoted:
//! LLID 2 bits, the four flags one each, RFU 2, Length 8, which fill the 16
//! bits the text gives, and CTEInfo the one octet that makes 24.
//!
//! **The CRC covers the whole PDU, CTEInfo included** (3.1.1: "The CRC
//! shall be calculated on the PDU of all Link Layer packets"), preset with
//! the connection's own initial value rather than the advertising one.
//! Whitening and the CRC's octet order are the advertising PDU's
//! ([`super::pdu::decode`] says why the lowest CRC octet comes first).
//!
//! The MIC, on an encrypted link, is the last four octets of what Length
//! counts; this module does not tell it from the payload, because nothing
//! in the header says whether a link is encrypted.

use crate::signal::dsp::code::crc::crc24;

/// One data channel PDU, as sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataPdu {
    /// 1: an L2CAP continuation or the Empty PDU; 2: an L2CAP start or a
    /// whole message; 3: an LL Control PDU; 0: reserved (Table 2.19).
    pub llid: u8,
    pub nesn: bool,
    pub sn: bool,
    pub md: bool,
    /// The CTEInfo octet, when the header's CP bit says one is there. The
    /// Constant Tone Extension it announces, after the CRC, is not read.
    pub cte_info: Option<u8>,
    /// The payload and, on an encrypted link, the MIC: Length octets.
    pub payload: Vec<u8>,
    pub crc_ok: bool,
}

/// The header without CTEInfo, in bits: as much as a receiver needs to know
/// how long the rest is.
pub const HEADER_BITS: usize = 16;

/// The CRC, in bits.
pub const CRC_BITS: usize = 24;

/// Octet `at` of `bits`, least significant bit first, as sent.
fn octet(bits: &[bool], at: usize) -> u8 {
    (0..8).fold(0u8, |b, i| b | (bits[at * 8 + i] as u8) << i)
}

/// The whole PDU's length in bits, header through CRC, from its de-whitened
/// header alone: `None` with fewer than [`HEADER_BITS`] bits.
pub fn used_bits(header: &[bool]) -> Option<usize> {
    if header.len() < HEADER_BITS {
        return None;
    }
    let cp = header[5];
    let length = octet(header, 1) as usize;
    Some(HEADER_BITS + 8 * cp as usize + length * 8 + CRC_BITS)
}

/// Decode one data channel PDU from its de-whitened bits, header first,
/// under the connection's CRC initial value. `None` until every bit the
/// header asks for is there: an incomplete PDU is not a wrong one.
pub fn decode(bits: &[bool], crc_init: u32) -> Option<DataPdu> {
    let total = used_bits(bits)?;
    if bits.len() < total {
        return None;
    }
    let octets: Vec<u8> = (0..(total - CRC_BITS) / 8)
        .map(|i| octet(bits, i))
        .collect();
    let crc_at = total / 8 - 3;
    let received = octet(bits, crc_at) as u32
        | (octet(bits, crc_at + 1) as u32) << 8
        | (octet(bits, crc_at + 2) as u32) << 16;
    let byte0 = octets[0];
    let cp = byte0 >> 5 & 1 != 0;
    let payload_at = 2 + cp as usize;
    Some(DataPdu {
        llid: byte0 & 0b11,
        nesn: byte0 >> 2 & 1 != 0,
        sn: byte0 >> 3 & 1 != 0,
        md: byte0 >> 4 & 1 != 0,
        cte_info: cp.then(|| octets[2]),
        payload: octets[payload_at..].to_vec(),
        crc_ok: crc24(&octets, crc_init) == received,
    })
}

/// A data channel PDU's bits, CRC'd under `crc_init` and whitened for
/// `channel` as a transmitter sends them: for this module's tests and the
/// receiver's.
#[cfg(test)]
pub fn encode(pdu: &DataPdu, crc_init: u32, channel: u8) -> Vec<bool> {
    let byte0 = pdu.llid & 0b11
        | (pdu.nesn as u8) << 2
        | (pdu.sn as u8) << 3
        | (pdu.md as u8) << 4
        | (pdu.cte_info.is_some() as u8) << 5;
    let mut octets = vec![byte0, pdu.payload.len() as u8];
    octets.extend(pdu.cte_info);
    octets.extend_from_slice(&pdu.payload);
    let crc = crc24(&octets, crc_init);
    octets.extend([crc as u8, (crc >> 8) as u8, (crc >> 16) as u8]);
    let mut bits: Vec<bool> = octets
        .iter()
        .flat_map(|&b| (0..8).map(move |i| b >> i & 1 != 0))
        .collect();
    crate::signal::dsp::code::lfsr::whiten(&mut bits, channel);
    bits
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::dsp::code::lfsr::whiten;

    fn pdu(llid: u8, flags: (bool, bool, bool), payload: &[u8]) -> DataPdu {
        DataPdu {
            llid,
            nesn: flags.0,
            sn: flags.1,
            md: flags.2,
            cte_info: None,
            payload: payload.to_vec(),
            crc_ok: true,
        }
    }

    /// The header's fields in Table 2.19's order, the CRC from the link's
    /// own init: a round trip, and a wrong init fails the CRC.
    #[test]
    fn a_data_pdu_round_trips_under_its_crc_init() {
        let p = pdu(
            3,
            (true, false, true),
            &[0x0c, 0x0c, 0x0f, 0x00, 0x05, 0x01],
        );
        let mut bits = encode(&p, 0x3a_5b7c, 12);
        whiten(&mut bits, 12);
        assert_eq!(decode(&bits, 0x3a_5b7c), Some(p));
        assert!(!decode(&bits, 0x55_5555).unwrap().crc_ok);
    }

    /// The empty PDU: LLID 1, length 0, the header and the CRC alone.
    #[test]
    fn the_empty_pdu_is_a_header_and_a_crc() {
        let p = pdu(1, (false, true, false), &[]);
        let mut bits = encode(&p, 0x12_3456, 3);
        whiten(&mut bits, 3);
        assert_eq!(bits.len(), 16 + 24);
        assert_eq!(used_bits(&bits[..HEADER_BITS]), Some(bits.len()));
        assert_eq!(decode(&bits, 0x12_3456), Some(p));
    }

    /// A header with CP set carries one more octet, CTEInfo, inside the
    /// CRC; the length still counts the payload alone.
    #[test]
    fn a_cte_info_octet_is_read_and_covered_by_the_crc() {
        let mut p = pdu(2, (false, false, false), &[1, 2, 3]);
        p.cte_info = Some(0x14);
        let mut bits = encode(&p, 0x12_3456, 20);
        whiten(&mut bits, 20);
        assert_eq!(bits.len(), 24 + 3 * 8 + 24);
        assert_eq!(used_bits(&bits[..HEADER_BITS]), Some(bits.len()));
        assert_eq!(decode(&bits, 0x12_3456), Some(p));
    }

    /// Too few bits for what the header claims is no packet yet; too few
    /// for a header, no length.
    #[test]
    fn a_short_capture_is_not_a_packet() {
        let p = pdu(2, (false, false, false), &[7; 20]);
        let mut bits = encode(&p, 0x12_3456, 3);
        whiten(&mut bits, 3);
        assert_eq!(decode(&bits[..bits.len() - 1], 0x12_3456), None);
        assert_eq!(used_bits(&bits[..HEADER_BITS - 1]), None);
    }

    /// The Core's own encryption sample data (Vol 6 Part C 1) gives real
    /// data channel PDUs, header first, with what each one is: read here,
    /// they are what the Core says they are. The sample shows no CRC ("they
    /// depend on a random CRC init value") and no whitening, so a CRC is
    /// appended under an arbitrary init and the bits go in as they are: what
    /// this holds is the header's layout, which `encode` cannot check
    /// against itself.
    #[test]
    fn the_cores_own_sample_pdus_read_as_labelled() {
        let read = |octets: &[u8]| {
            let crc = crc24(octets, 0x12_3456);
            let mut all = octets.to_vec();
            all.extend([crc as u8, (crc >> 8) as u8, (crc >> 16) as u8]);
            let bits: Vec<bool> = all
                .iter()
                .flat_map(|&b| (0..8).map(move |i| b >> i & 1 != 0))
                .collect();
            decode(&bits, 0x12_3456).unwrap()
        };
        // LL_ENC_REQ: "Length 0x17 Control Type 0x03".
        let enc_req = read(&[
            0x03, 0x17, 0x03, 0x90, 0x78, 0x56, 0x34, 0x12, 0xef, 0xcd, 0xab, 0x74, 0x24, 0x13,
            0x02, 0xf1, 0xe0, 0xdf, 0xce, 0xbd, 0xac, 0x24, 0xab, 0xdc, 0xba,
        ]);
        assert_eq!((enc_req.llid, enc_req.payload.len()), (3, 0x17));
        assert_eq!(enc_req.payload[0], 0x03);
        // LL_START_ENC_REQ: "Length 0x01 Control Type 0x05".
        let start = read(&[0x07, 0x01, 0x05]);
        assert_eq!((start.llid, start.payload.as_slice()), (3, &[0x05][..]));
        // LL_DATA1: an L2CAP start, "Length 0x1F (i.e. 27 + 4 = 31 dec)",
        // the four being the MIC.
        let mut data1 = vec![0x0e, 0x1f];
        data1.extend([0u8; 31]);
        let d = read(&data1);
        assert_eq!((d.llid, d.payload.len(), d.cte_info), (2, 31, None));
        assert!(d.crc_ok);
    }

    /// One flipped payload bit fails the CRC and nothing else.
    #[test]
    fn a_flipped_bit_fails_the_crc() {
        let p = pdu(2, (false, false, false), &[0xaa; 8]);
        let mut bits = encode(&p, 0x12_3456, 7);
        whiten(&mut bits, 7);
        bits[HEADER_BITS + 5] = !bits[HEADER_BITS + 5];
        let got = decode(&bits, 0x12_3456).unwrap();
        assert!(!got.crc_ok);
        assert_eq!(got.llid, 2);
    }
}
