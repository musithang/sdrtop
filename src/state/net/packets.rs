// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The BLE and LE Coded lists: their packets, what an extended advertising
//! PDU said, and how each list is held, filtered and read.

use super::NetState;

/// How many recent PDUs of each kind [`NetState::ble_packets`] keeps
/// ([`NetState::trim_ble_packets`]). A bench instrument is read a screenful
/// at a time, not scrolled back through a session's worth of advertising
/// traffic; old rows fall off the end rather than growing the list forever.
pub const BLE_PACKET_LIMIT: usize = 200;

/// One decoded advertising channel PDU, as a panel shows it.
///
/// `crate::signal::ble::pdu::Packet` is the decode itself, pure and knowing
/// nothing about a screen; this adds the two facts a panel needs that decode
/// alone does not carry - which channel it arrived on and when.
#[derive(Clone, Debug)]
pub struct BlePacket {
    /// Its place in the session's arrivals, from 1: what a selection holds on
    /// to, since the ring's positions shift with every packet
    /// (`NetState::ble_heard`).
    pub seq: u64,
    pub channel: u8,
    pub pdu_type: crate::signal::ble::pdu::PduType,
    /// The PHY it was received on: the packet's own, not whatever the
    /// decoder is set to now, since the list outlives a switch.
    pub phy: crate::signal::ble::Phy,
    /// ChSel, where the type defines it (`pdu::Packet::ch_sel`).
    pub ch_sel: bool,
    pub tx_add_random: bool,
    pub rx_add_random: bool,
    pub length: u8,
    pub adv_addr: Option<[u8; 6]>,
    /// The PDU's payload as decoded (`pdu::Packet::payload`): what the AD
    /// structures and a CONNECT_IND's parameters are read from. Bounded by
    /// the ring (`BLE_PACKET_LIMIT`) and by the length field's 6 bits.
    pub payload: Vec<u8>,
    pub crc_ok: bool,
    /// Read from the detector's own coherence at the moment this
    /// packet's sync word was found. `None` only at a coherence of one -
    /// noiseless, which does not happen on a radio - never because nothing
    /// was measured.
    pub snr_db: Option<f64>,
    /// The carrier offset as received, in Hz: estimated from the sync word
    /// against its known waveform (`signal::ble::receive`'s data-aided
    /// estimate), with its uncertainty. **Their crystal's error minus our
    /// oscillator's**, never corrected here: the panels take it through
    /// `RadioState::transmitter_offset`, which removes ours when a reference
    /// allows, and the chrome says which of the two the number on screen is.
    pub freq_offset_hz: Option<crate::signal::dsp::uncertainty::Uncertain>,
    /// Modulation index, delta-f1 average, delta-f2 average and their
    /// ratio, measured from this packet's own on-air symbols. `None` when
    /// the packet was too short, or too unlucky in its particular random
    /// content, to contain a settled run of either kind - see
    /// `signal::ble::measure`'s own doc for what "settled" means here.
    pub modulation: Option<crate::signal::ble::measure::ModulationQuality>,
    /// This packet's own frequency offset, read early and late, and the
    /// drift between them. `None` under the same conditions as
    /// `modulation` - too short a capture to give each half its own
    /// variance.
    pub drift: Option<crate::signal::ble::measure::Drift>,
    pub seen: std::time::Instant,
    /// What only an LE Coded packet has; `None` on the uncoded PHYs.
    pub coded: Option<CodedFacts>,
    /// An extended advertising PDU's header, its place in its advertising
    /// event, and what became of its AuxPtr; `None` on a legacy PDU, or an
    /// extended one not read as such.
    pub ext: Option<ExtInfo>,
}

impl BlePacket {
    /// The kind the list's filter and its ring count it as: an extended PDU
    /// read as one is advertising (its roles are all advertising PDUs), and
    /// every other packet is its type's kind.
    pub fn kind(&self) -> Option<PduKind> {
        match self.ext {
            Some(_) => Some(PduKind::Advertising),
            None => PduKind::of(self.pdu_type),
        }
    }
}

/// An extended advertising PDU, read (Core 5.4 Vol 6 Part B 2.3.4).
#[derive(Clone, Debug, PartialEq)]
pub struct ExtInfo {
    pub header: crate::signal::ble::ext::ExtHeader,
    pub role: ExtRole,
    /// What became of its AuxPtr's promise.
    pub aux: crate::signal::ble::aux_ptr::AuxOutcome,
}

/// Which extended PDU a type 7 is: the type code is one for all three, and
/// only where it was heard says which (2.3, Table 2.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExtRole {
    /// On a primary advertising channel.
    AdvExt,
    /// Where an `ADV_EXT_IND`'s AuxPtr promised it, that packet's `seq`; or
    /// heard in such a window without being the one promised (`None`).
    AuxAdv { superior_seq: Option<u64> },
    /// Where an `AUX_ADV_IND`'s or another `AUX_CHAIN_IND`'s AuxPtr
    /// promised it.
    AuxChain { superior_seq: u64 },
}

impl ExtRole {
    /// The PDU's name as the Core gives it.
    pub fn label(self) -> &'static str {
        match self {
            ExtRole::AdvExt => "ADV_EXT_IND",
            ExtRole::AuxAdv { .. } => "AUX_ADV_IND",
            ExtRole::AuxChain { .. } => "AUX_CHAIN_IND",
        }
    }
}

/// An LE Coded packet's facts beside the ones every BLE packet has.
#[derive(Clone, Debug, PartialEq)]
pub struct CodedFacts {
    /// How many symbols the FEC decoder overruled across both blocks: what it
    /// took to get the bits, beside the bits.
    pub fec_repairs: u32,
    /// What the measurement path read of it (`net::measure::le_coded`), when
    /// its samples were still held.
    pub reading: Option<crate::signal::net::measure::CodedReading>,
}

/// How the BLE packet list is being read.
///
/// The cursor holds a packet's [`BlePacket::seq`], not a row: the list is
/// newest first, so every arrival moves every row, and a cursor on a row
/// number would slide to a different packet each time one came in.
#[derive(Clone, Debug, Default)]
pub struct BlePacketView {
    pub selection: crate::state::Selection<u64>,
    /// Only packets from this advertiser address, when set.
    pub filter: Option<[u8; 6]>,
    /// Only this kind of PDU, when set. It narrows together with
    /// [`Self::filter`], and outlives leaving the view as that does.
    pub kind: Option<PduKind>,
    /// The list as it stood when it was held, and [`NetState::ble_heard`] at
    /// that moment: a copy, so holding the list stops nothing else. The
    /// coexistence marks and the census go on taking every packet.
    pub held: Option<(std::collections::VecDeque<BlePacket>, u64)>,
}

/// The three kinds of advertising channel PDU the BLE list can be narrowed
/// to, by what a reader is looking for: a connection being set up, a scanner
/// asking, or the advertising that makes up nearly all of the rest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PduKind {
    /// CONNECT_IND.
    Connect,
    /// SCAN_REQ and SCAN_RSP.
    Scan,
    /// ADV_IND, ADV_DIRECT_IND, ADV_NONCONN_IND and ADV_SCAN_IND, and the
    /// extended advertising PDUs read as such (`BlePacket::kind`).
    Advertising,
}

impl PduKind {
    /// The kind a PDU type belongs to; `None` for the types none of them
    /// names, which a kind filter therefore never shows.
    pub fn of(pdu_type: crate::signal::ble::pdu::PduType) -> Option<Self> {
        use crate::signal::ble::pdu::PduType;
        match pdu_type {
            PduType::ConnectInd => Some(Self::Connect),
            PduType::ScanReq | PduType::ScanRsp => Some(Self::Scan),
            PduType::AdvInd
            | PduType::AdvDirectInd
            | PduType::AdvNonconnInd
            | PduType::AdvScanInd => Some(Self::Advertising),
            PduType::Other(_) => None,
        }
    }

    /// The next step of the list's `t` key: every kind, then each in turn.
    pub fn step(current: Option<Self>) -> Option<Self> {
        match current {
            None => Some(Self::Connect),
            Some(Self::Connect) => Some(Self::Scan),
            Some(Self::Scan) => Some(Self::Advertising),
            Some(Self::Advertising) => None,
        }
    }

    /// How the frame and the list name it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Connect => "CONNECT",
            Self::Scan => "SCAN",
            Self::Advertising => "ADV",
        }
    }
}

/// Where a packet's kind is counted against [`BLE_PACKET_LIMIT`]: one slot per
/// [`PduKind`], and one for the types none of them names.
fn kind_slot(kind: Option<PduKind>) -> usize {
    match kind {
        Some(PduKind::Connect) => 0,
        Some(PduKind::Scan) => 1,
        Some(PduKind::Advertising) => 2,
        None => 3,
    }
}

impl NetState {
    /// The packets the BLE list shows, newest first: the held copy while the
    /// list is held, the live ring otherwise, narrowed to the filter's
    /// address and to the chosen kind of PDU when there are those.
    ///
    /// **The one account of the list**, read by the panel that draws it and
    /// the keys that move through it, so the arrows step through exactly the
    /// rows on screen.
    pub fn ble_shown(&self) -> Vec<&BlePacket> {
        let source = match &self.ble_view.held {
            Some((held, _)) => held,
            None => &self.ble_packets,
        };
        source
            .iter()
            .filter(|p| self.ble_view.filter.is_none_or(|a| p.adv_addr == Some(a)))
            .filter(|p| self.ble_view.kind.is_none_or(|k| p.kind() == Some(k)))
            .collect()
    }

    /// The moment back to which [`Self::ble_packets`] is every packet heard,
    /// or `None` when it still is all the way: the oldest kept packet of each
    /// kind at its limit, the latest of those. Before it, only the kinds not
    /// yet cut remain, so a reader of the ring as a record of the band (the
    /// coexistence marks) stops there.
    ///
    /// A kind exactly at the limit counts as cut: it may have been, and a
    /// stretch drawn too short is a smaller error than one drawn too quiet.
    pub fn ble_complete_since(&self) -> Option<std::time::Instant> {
        let mut kept = [0usize; 4];
        let mut oldest = [None; 4];
        for p in &self.ble_packets {
            let slot = kind_slot(p.kind());
            kept[slot] += 1;
            oldest[slot] = Some(p.seen);
        }
        (0..4)
            .filter(|&i| kept[i] >= BLE_PACKET_LIMIT)
            .filter_map(|i| oldest[i])
            .max()
    }

    /// Cut [`Self::ble_packets`] to the newest [`BLE_PACKET_LIMIT`] of each
    /// [`PduKind`] (and of the types none names), keeping arrival order.
    ///
    /// **Per kind, not overall**: advertising arrives by the hundred and a
    /// CONNECT_IND once, so one shared limit pushed the packet a reader was
    /// looking for out of the list within seconds of its arrival.
    pub fn trim_ble_packets(&mut self) {
        let mut kept = [0usize; 4];
        self.ble_packets.retain(|p| {
            let slot = kind_slot(p.kind());
            kept[slot] += 1;
            kept[slot] <= BLE_PACKET_LIMIT
        });
    }

    /// The LE list's packets as the screen has them, unfiltered: the held
    /// copy while it is held, the live ring otherwise. Where a packet's
    /// superior or its aux is looked up: the live ring keeps filling behind
    /// a hold, and can let go of what the held list still shows.
    pub fn ble_list(&self) -> &std::collections::VecDeque<BlePacket> {
        self.ble_view
            .held
            .as_ref()
            .map_or(&self.ble_packets, |(held, _)| held)
    }

    /// [`Self::ble_list`] for the LE Coded list.
    pub fn coded_list(&self) -> &std::collections::VecDeque<BlePacket> {
        self.coded_view
            .held
            .as_ref()
            .map_or(&self.coded_packets, |(held, _)| held)
    }

    /// The packets the LE Coded list shows, newest first: the held copy while
    /// the list is held, the live ring otherwise. The one account of that
    /// list, as [`Self::ble_shown`] is of LE 1M's.
    pub fn coded_shown(&self) -> Vec<&BlePacket> {
        match &self.coded_view.held {
            Some((held, _)) => held.iter().collect(),
            None => self.coded_packets.iter().collect(),
        }
    }

    /// LE Coded packets that have arrived since its list was held.
    pub fn coded_behind(&self) -> u64 {
        self.coded_view
            .held
            .as_ref()
            .map_or(0, |(_, at)| self.coded_heard.saturating_sub(*at))
    }

    /// Packets that have arrived since the list was held; zero when it is not.
    pub fn ble_behind(&self) -> u64 {
        self.ble_view
            .held
            .as_ref()
            .map_or(0, |(_, at)| self.ble_heard.saturating_sub(*at))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ble_packet(seq: u64, pdu_type: crate::signal::ble::pdu::PduType) -> BlePacket {
        BlePacket {
            phy: crate::signal::ble::Phy::OneM,
            seq,
            channel: 37,
            pdu_type,
            ch_sel: false,
            tx_add_random: false,
            rx_add_random: false,
            length: 6,
            adv_addr: Some([seq as u8; 6]),
            payload: Vec::new(),
            crc_ok: true,
            snr_db: None,
            freq_offset_hz: None,
            modulation: None,
            drift: None,
            seen: std::time::Instant::now(),
            coded: None,
            ext: None,
        }
    }

    /// **How far back the list is every packet**: as far as the oldest kept
    /// packet of a kind that has been cut; everything when none has.
    #[test]
    fn the_list_is_complete_back_to_the_oldest_cut_kind() {
        use crate::signal::ble::pdu::PduType;
        use std::time::{Duration, Instant};
        let now = Instant::now();
        let at = |seq, t, s: u64| BlePacket {
            seen: now - Duration::from_secs(s),
            ..ble_packet(seq, t)
        };
        let mut net = NetState::default();
        net.ble_packets.push_front(at(1, PduType::ConnectInd, 900));
        net.ble_packets.push_front(at(2, PduType::AdvInd, 800));
        assert_eq!(net.ble_complete_since(), None, "nothing cut yet");

        for seq in 0..BLE_PACKET_LIMIT as u64 {
            net.ble_packets
                .push_front(at(10 + seq, PduType::AdvInd, 500 - seq));
        }
        net.trim_ble_packets();
        let oldest_adv = now - Duration::from_secs(500);
        assert_eq!(net.ble_complete_since(), Some(oldest_adv));
    }

    /// **The flood does not push the rare kinds out.** The ring keeps the
    /// newest [`BLE_PACKET_LIMIT`] of each kind, so a CONNECT_IND heard
    /// before three hundred advertisements is still there to filter for.
    #[test]
    fn the_ring_keeps_each_kinds_newest() {
        use crate::signal::ble::pdu::PduType;
        let mut net = NetState::default();
        net.ble_packets
            .push_front(ble_packet(1, PduType::ConnectInd));
        net.ble_packets.push_front(ble_packet(2, PduType::ScanReq));
        for seq in 3..303 {
            net.ble_packets.push_front(ble_packet(seq, PduType::AdvInd));
        }
        net.trim_ble_packets();
        let advs = net
            .ble_packets
            .iter()
            .filter(|p| p.pdu_type == PduType::AdvInd)
            .count();
        assert_eq!(advs, BLE_PACKET_LIMIT);
        // The newest of them, in arrival order still.
        assert_eq!(net.ble_packets.front().map(|p| p.seq), Some(302));
        assert!(net.ble_packets.iter().any(|p| p.seq == 1));
        assert!(net.ble_packets.iter().any(|p| p.seq == 2));
        assert!(!net.ble_packets.iter().any(|p| p.seq == 102));
        assert!(net.ble_packets.iter().any(|p| p.seq == 103));
    }

    /// **Extended advertising is advertising.** An `ADV_EXT_IND` and its
    /// `AUX_ADV_IND`, read as extended PDUs, are shown by the ADV filter and
    /// counted with the advertising; a type 7 not read as one (its CRC
    /// failed) stays with the types no kind names.
    #[test]
    fn extended_advertising_is_advertising_to_the_filter() {
        use crate::signal::ble::aux_ptr::AuxOutcome;
        use crate::signal::ble::pdu::PduType;
        let extended = |seq, role| BlePacket {
            ext: Some(ExtInfo {
                header: crate::signal::ble::ext::parse(&[1, 0]).unwrap(),
                role,
                aux: AuxOutcome::NonePromised,
            }),
            ..ble_packet(seq, PduType::Other(0x07))
        };
        let mut net = NetState::default();
        net.ble_packets.push_front(extended(1, ExtRole::AdvExt));
        net.ble_packets.push_front(extended(
            2,
            ExtRole::AuxAdv {
                superior_seq: Some(1),
            },
        ));
        net.ble_packets
            .push_front(ble_packet(3, PduType::Other(0x07)));
        net.ble_view.kind = Some(PduKind::Advertising);
        let shown: Vec<u64> = net.ble_shown().iter().map(|p| p.seq).collect();
        assert_eq!(shown, [2, 1]);
        assert_eq!(net.ble_packets[0].kind(), None);
    }
}
