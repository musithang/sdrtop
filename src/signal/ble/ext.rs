// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The Common Extended Advertising Payload Format (Core 5.4 Vol 6 Part B
//! 2.3.4): the header every extended advertising PDU starts its payload
//! with, `ADV_EXT_IND` and the auxiliary packets alike.
//!
//! A six-bit length and the two-bit AdvMode, then, if the length is not zero,
//! a flags octet and the fields it names, in the flags' order; whatever is
//! left of the extended header is ACAD, and what follows it is AdvData.
//! Read from the text and its figures (2.15, 2.18, 2.19), whose fields are
//! drawn least significant first (1.2), and nothing else: a field this
//! module does not read the inside of (SyncInfo) is said to be present and
//! its octets stepped over, not guessed at.

// Nothing reads an extended header until the LE Coded worker does; the
// attribute goes when it does.

/// The advertising event an extended PDU belongs to (Table 2.13).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdvMode {
    NonConnectableNonScannable,
    Connectable,
    Scannable,
}

/// The AdvDataInfo field (2.3.4.4, Figure 2.18): which advertising set,
/// and which version of its data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Adi {
    /// The Advertising Data ID, bits 0-11.
    pub did: u16,
    /// The Advertising Set ID, bits 12-15.
    pub sid: u8,
}

/// The PHY an auxiliary packet is sent on (Table 2.16).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuxPhy {
    OneM,
    TwoM,
    Coded,
}

/// The AuxPtr field (2.3.4.5, Figure 2.19): where and when the auxiliary
/// packet comes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuxPtr {
    /// The general-purpose channel index the auxiliary packet is sent on.
    pub channel: u8,
    /// CA (Table 2.17): `true` is 0 to 50 ppm, `false` 51 to 500 ppm.
    pub accurate_clock: bool,
    /// The Offset Units (Table 2.15): 30 or 300 us.
    pub unit_us: u32,
    /// The Aux Offset times the unit: from the start of the packet carrying
    /// this field to no earlier than the auxiliary packet's start, which
    /// comes no later than one unit after that.
    pub offset_us: u32,
    /// `None` for a reserved value.
    pub phy: Option<AuxPhy>,
}

impl AuxPtr {
    /// The advertiser's clock bound between the two packets, ppm.
    pub fn clock_ppm(&self) -> u32 {
        if self.accurate_clock {
            50
        } else {
            500
        }
    }

    /// An Aux Offset of zero: no auxiliary packet will be sent, and the data
    /// is incomplete (2.3.4.5).
    pub fn promises_nothing(&self) -> bool {
        self.offset_us == 0
    }
}

/// An extended PDU's header and data, as 2.3.4 lays them out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtHeader {
    /// `None` for the reserved `0b11`.
    pub mode: Option<AdvMode>,
    /// The advertiser's address, in written order (`pdu::air_octets`).
    pub adv_a: Option<[u8; 6]>,
    /// The address a directed event is for, in written order.
    pub target_a: Option<[u8; 6]>,
    /// The CTEInfo octet (2.5.2), as sent.
    pub cte_info: Option<u8>,
    pub adi: Option<Adi>,
    pub aux_ptr: Option<AuxPtr>,
    /// Whether a SyncInfo field is present; its 18 octets are not read here.
    pub sync_info: bool,
    /// The advertiser's transmit power, dBm, signed (2.3.4.7).
    pub tx_power_dbm: Option<i8>,
    /// The Additional Controller Advertising Data: what is left of the
    /// extended header after the fields its flags name.
    pub acad: Vec<u8>,
    /// The host's advertising data, after the extended header.
    pub adv_data: Vec<u8>,
}

/// Why a payload is not a Common Extended Advertising Payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Malformed {
    /// Not even the length octet.
    Empty,
    /// The extended header claims more octets than the payload holds.
    HeaderPastPayload,
    /// The fields the flags name do not fit in the extended header.
    FieldsPastHeader,
}

/// Field sizes in octets, in flag order (Table 2.14; 2.3.4.1 to 2.3.4.7).
const ADV_A: usize = 6;
const TARGET_A: usize = 6;
const CTE_INFO: usize = 1;
const ADI: usize = 2;
const AUX_PTR: usize = 3;
const SYNC_INFO: usize = 18;
const TX_POWER: usize = 1;

/// The AuxPtr field's three octets, least significant first (Figure 2.19):
/// Channel Index bits 0-5, CA bit 6, Offset Units bit 7, Aux Offset bits
/// 8-20, Aux PHY bits 21-23.
pub(crate) fn aux_ptr(octets: [u8; 3]) -> AuxPtr {
    let v = u32::from(octets[0]) | u32::from(octets[1]) << 8 | u32::from(octets[2]) << 16;
    let unit_us = if v >> 7 & 1 == 1 { 300 } else { 30 };
    AuxPtr {
        channel: (v & 0x3F) as u8,
        accurate_clock: v >> 6 & 1 == 1,
        unit_us,
        offset_us: (v >> 8 & 0x1FFF) * unit_us,
        phy: match v >> 21 & 0b111 {
            0b000 => Some(AuxPhy::OneM),
            0b001 => Some(AuxPhy::TwoM),
            0b010 => Some(AuxPhy::Coded),
            _ => None,
        },
    }
}

/// The ADI field's two octets, least significant first (Figure 2.18).
pub(crate) fn adi(octets: [u8; 2]) -> Adi {
    let v = u16::from_le_bytes(octets);
    Adi {
        did: v & 0x0FFF,
        sid: (v >> 12) as u8,
    }
}

/// A PDU's payload read as the Common Extended Advertising Payload Format.
pub fn parse(payload: &[u8]) -> Result<ExtHeader, Malformed> {
    let (&first, rest) = payload.split_first().ok_or(Malformed::Empty)?;
    let length = (first & 0x3F) as usize;
    let mode = match first >> 6 {
        0b00 => Some(AdvMode::NonConnectableNonScannable),
        0b01 => Some(AdvMode::Connectable),
        0b10 => Some(AdvMode::Scannable),
        _ => None,
    };
    if rest.len() < length {
        return Err(Malformed::HeaderPastPayload);
    }
    let (header, adv_data) = rest.split_at(length);
    let mut out = ExtHeader {
        mode,
        adv_a: None,
        target_a: None,
        cte_info: None,
        adi: None,
        aux_ptr: None,
        sync_info: false,
        tx_power_dbm: None,
        acad: Vec::new(),
        adv_data: adv_data.to_vec(),
    };
    // A length of zero: no flags octet and no fields (2.3.4).
    let Some((&flags, mut fields)) = header.split_first() else {
        return Ok(out);
    };
    let mut take = |n: usize| -> Result<&[u8], Malformed> {
        if fields.len() < n {
            return Err(Malformed::FieldsPastHeader);
        }
        let (field, after) = fields.split_at(n);
        fields = after;
        Ok(field)
    };
    let address = |octets: &[u8]| -> [u8; 6] {
        let mut a = [0u8; 6];
        a.copy_from_slice(octets);
        super::pdu::air_octets(a)
    };
    if flags & 1 << 0 != 0 {
        out.adv_a = Some(address(take(ADV_A)?));
    }
    if flags & 1 << 1 != 0 {
        out.target_a = Some(address(take(TARGET_A)?));
    }
    if flags & 1 << 2 != 0 {
        out.cte_info = Some(take(CTE_INFO)?[0]);
    }
    if flags & 1 << 3 != 0 {
        let f = take(ADI)?;
        out.adi = Some(adi([f[0], f[1]]));
    }
    if flags & 1 << 4 != 0 {
        let f = take(AUX_PTR)?;
        out.aux_ptr = Some(aux_ptr([f[0], f[1], f[2]]));
    }
    if flags & 1 << 5 != 0 {
        take(SYNC_INFO)?;
        out.sync_info = true;
    }
    if flags & 1 << 6 != 0 {
        out.tx_power_dbm = Some(take(TX_POWER)?[0] as i8);
    }
    // Bit 7 is reserved and names no field; what is left is ACAD.
    out.acad = fields.to_vec();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Figure 2.19, least significant first: channel 9, CA 1, 30 us units,
    /// offset 100 (3000 us), LE Coded.
    #[test]
    fn auxptr_reads_figure_2_19() {
        let v: u32 = 9 | 1 << 6 | 100 << 8 | 0b010 << 21;
        let p = aux_ptr([v as u8, (v >> 8) as u8, (v >> 16) as u8]);
        assert_eq!(
            p,
            AuxPtr {
                channel: 9,
                accurate_clock: true,
                unit_us: 30,
                offset_us: 3000,
                phy: Some(AuxPhy::Coded)
            }
        );
        assert_eq!(p.clock_ppm(), 50);
        assert!(!p.promises_nothing());
    }

    #[test]
    fn three_hundred_microsecond_units_and_a_reserved_phy() {
        let v: u32 = 20 | 1 << 7 | 1000 << 8 | 0b101 << 21;
        let p = aux_ptr([v as u8, (v >> 8) as u8, (v >> 16) as u8]);
        assert_eq!(
            (p.unit_us, p.offset_us, p.phy, p.clock_ppm()),
            (300, 300_000, None, 500)
        );
    }

    /// An Aux Offset of zero: no auxiliary packet will come (2.3.4.5).
    #[test]
    fn a_zero_offset_promises_nothing() {
        let v: u32 = 9 | 0b010 << 21;
        assert!(aux_ptr([v as u8, (v >> 8) as u8, (v >> 16) as u8]).promises_nothing());
    }

    /// Figure 2.18: DID in bits 0-11, SID in 12-15.
    #[test]
    fn adi_reads_figure_2_18() {
        assert_eq!(adi([0x23, 0x31]), Adi { did: 0x123, sid: 3 });
    }

    /// An ADV_EXT_IND as LE Coded sends it: AdvMode 0, ADI and AuxPtr only.
    #[test]
    fn a_coded_adv_ext_ind_has_adi_and_auxptr() {
        let aux: u32 = 9 | 100 << 8 | 0b010 << 21;
        let payload = [
            6,           // ext header length 6, AdvMode 0b00
            0b0001_1000, // flags: ADI, AuxPtr
            0x23,
            0x31, // ADI
            aux as u8,
            (aux >> 8) as u8,
            (aux >> 16) as u8,
        ];
        let h = parse(&payload).unwrap();
        assert_eq!(h.mode, Some(AdvMode::NonConnectableNonScannable));
        assert_eq!(h.adi, Some(Adi { did: 0x123, sid: 3 }));
        assert_eq!(h.aux_ptr.map(|a| a.channel), Some(9));
        assert!(h.adv_a.is_none() && h.adv_data.is_empty() && h.acad.is_empty());
    }

    /// Fields in flag order (Table 2.14), the rest of the header as ACAD,
    /// then AdvData.
    #[test]
    fn fields_follow_their_flags_then_acad_then_data() {
        let payload = [
            9 | 0b01 << 6, // length 9, connectable
            0b0100_0001,   // AdvA, TxPower
            1,
            2,
            3,
            4,
            5,
            6,    // AdvA, air order
            0xF6, // TxPower -10 dBm
            0xAA, // one octet of ACAD
            0x02,
            0x01,
            0x06, // AdvData: Flags
        ];
        let h = parse(&payload).unwrap();
        assert_eq!(h.mode, Some(AdvMode::Connectable));
        assert_eq!(h.adv_a, Some([6, 5, 4, 3, 2, 1]));
        assert_eq!(h.tx_power_dbm, Some(-10));
        assert_eq!(h.acad, vec![0xAA]);
        assert_eq!(h.adv_data, vec![0x02, 0x01, 0x06]);
    }

    /// Every field the flags can name, SyncInfo's 18 octets stepped over.
    #[test]
    fn every_field_in_order() {
        let mut payload = vec![0, 0b0111_1111];
        payload.extend([1, 2, 3, 4, 5, 6]); // AdvA
        payload.extend([7, 8, 9, 10, 11, 12]); // TargetA
        payload.push(0x15); // CTEInfo
        payload.extend([0x01, 0x20]); // ADI: DID 1, SID 2
        payload.extend([5, 0x0A, 0x00]); // AuxPtr: ch 5, offset 10 x 30 us, LE 1M
        payload.extend([0u8; 18]); // SyncInfo
        payload.push(4); // TxPower
        payload[0] = (payload.len() - 1) as u8;
        payload.extend([0x03, 0x09, b'h', b'i']);
        let h = parse(&payload).unwrap();
        assert_eq!(h.target_a, Some([12, 11, 10, 9, 8, 7]));
        assert_eq!(h.cte_info, Some(0x15));
        assert_eq!(h.adi, Some(Adi { did: 1, sid: 2 }));
        assert_eq!(
            h.aux_ptr.map(|a| (a.channel, a.offset_us, a.phy)),
            Some((5, 300, Some(AuxPhy::OneM)))
        );
        assert!(h.sync_info);
        assert_eq!(h.tx_power_dbm, Some(4));
        assert!(h.acad.is_empty());
        assert_eq!(h.adv_data, vec![0x03, 0x09, b'h', b'i']);
    }

    #[test]
    fn a_header_longer_than_its_payload_is_malformed() {
        assert_eq!(parse(&[]), Err(Malformed::Empty));
        assert_eq!(parse(&[10, 0x01, 1, 2]), Err(Malformed::HeaderPastPayload));
        assert_eq!(
            parse(&[2, 0b0001_0000, 0x00]),
            Err(Malformed::FieldsPastHeader)
        );
    }

    #[test]
    fn adv_mode_three_is_reserved() {
        assert_eq!(parse(&[0b11 << 6]).unwrap().mode, None);
    }
}
