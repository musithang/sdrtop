// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The Bluetooth LE packets, as the packet lists show them: what each packet
//! said and what was measured of it, every uncertain figure with its sigma.
//!
//! The BLE export's own account: built here once, written as text by the
//! export and served as JSON by the API, so the two cannot differ. What the
//! panels refuse is absent here too: the advertised structures of a packet
//! whose CRC failed, its modulation and drift, a start and end frequency where
//! no drift was measured, ChSel on a type that reserves it, RxAdd on a type
//! with no target address. Names stand beside readings, never in place of
//! them: a company is its identifier with the SIG snapshot's name beside it.

use serde::Serialize;

use super::net::Sigma;
use super::{Clock, Group};
use crate::engine::testimony::{FeedSpan, Staleness, Testimony};
use crate::signal::ble::ad::{self, Ad, Structure};
use crate::signal::ble::pdu::PduType;
use crate::signal::dsp::uncertainty::Uncertain;
use crate::state::{BlePacket, SdrMetrics};

/// What a packet advertised about its sender: the AD structures, flattened.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Advertised {
    /// As the address mode allows it, as on screen.
    pub name: Option<String>,
    pub name_complete: Option<bool>,
    pub flags: Vec<String>,
    pub tx_power_dbm: Option<i8>,
    pub services: Vec<String>,
    /// `uuid=bytes`, the bytes as the address mode allows them.
    pub service_data: Vec<String>,
    pub company_id: Option<u16>,
    pub company: Option<&'static str>,
    pub mfr_data: Vec<String>,
    /// `0xCODE=bytes` for structures with no column of their own.
    pub other_ad: Vec<String>,
    /// Where the structures stopped making sense, and why.
    pub malformed: Option<String>,
}

/// One packet: what it said, and what was measured of it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Packet {
    pub seq: u64,
    pub age_s: u64,
    pub channel: u8,
    pub phy: &'static str,
    /// The PDU's type, or an extended PDU's role.
    pub pdu_type: String,
    pub crc_ok: bool,
    pub length: u8,
    /// As the address mode shows it.
    pub address: Option<String>,
    pub address_kind: Option<&'static str>,
    /// `public` or `random`.
    pub tx_add: &'static str,
    /// Only on a type with a target address.
    pub rx_add: Option<&'static str>,
    /// Only on a type that defines it.
    pub ch_sel: Option<bool>,
    pub advertised: Advertised,
    pub snr_db: Option<f64>,
    /// The transmitter's offset, corrected for our oscillator as on screen.
    pub cfo_khz: Option<Sigma>,
    pub cfo_ppm: Option<Sigma>,
    pub start_khz: Option<Sigma>,
    pub end_khz: Option<Sigma>,
    pub mod_index: Option<Sigma>,
    pub df1_avg_khz: Option<Sigma>,
    pub df2_avg_khz: Option<Sigma>,
    pub df2_df1_ratio: Option<Sigma>,
    pub drift_khz: Option<Sigma>,
    pub drift_rate_hz_per_us: Option<Sigma>,
    /// LE Coded: bits the FEC repaired.
    pub fec_repairs: Option<u32>,
    pub sid: Option<u8>,
    /// What became of an AuxPtr: `pending`, `heard`, `missed`, ...
    pub aux: Option<String>,
}

/// A packet list, for its route.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Packets {
    /// Why the list is not every packet, when it is not: held, filtered,
    /// narrowed to a kind; for LE Coded, which tests its columns are.
    pub note: Option<String>,
    /// Newest first, as on screen; `since` leaves out those numbered up to it.
    pub packets: Vec<Packet>,
}

/// What became of an AuxPtr, one word.
pub fn aux_word(aux: crate::signal::ble::aux_ptr::AuxOutcome) -> String {
    use crate::signal::ble::aux_ptr::AuxOutcome;
    match aux {
        AuxOutcome::Pending => "pending".to_string(),
        AuxOutcome::Heard { .. } => "heard".to_string(),
        AuxOutcome::Missed => "missed".to_string(),
        AuxOutcome::NotInView => "not_in_view".to_string(),
        AuxOutcome::FeedLost => "feed_lost".to_string(),
        AuxOutcome::NonePromised => "none_promised".to_string(),
        AuxOutcome::Refused(why) => format!("refused: {why}"),
    }
}

/// The packet's AD structures, flattened; none where it carries none, or
/// where its CRC failed.
fn advertised(p: &BlePacket, net: &crate::state::NetState) -> Advertised {
    let masked = net.address_display == crate::state::AddressDisplay::Masked;
    let bytes = |data: &[u8]| {
        if masked {
            net.show_bytes(data)
        } else {
            ad::hex(data).replace(' ', "")
        }
    };
    let mut a = Advertised::default();
    // An extended PDU's advertising data follows its extended header; a type
    // 7 not read as one carries none to find.
    let data = match &p.ext {
        Some(ext) => Some(ext.header.adv_data.as_slice()),
        None => ad::adv_data(p.pdu_type, &p.payload).filter(|_| p.pdu_type != PduType::Other(0x07)),
    }
    .filter(|_| p.crc_ok);
    for structure in data.map(ad::parse).unwrap_or_default() {
        match structure {
            Structure::Malformed { offset, why } => {
                a.malformed = Some(format!("octet {offset}: {why}"))
            }
            Structure::Ad { ad, .. } => match ad {
                Ad::Flags(bits) => a
                    .flags
                    .extend(ad::flag_names(bits).into_iter().map(|f| f.to_string())),
                Ad::Name { complete, text } => {
                    // The complete name wins, as on screen.
                    if a.name.is_none() || complete {
                        a.name = Some(net.show_name(&text));
                        a.name_complete = Some(complete);
                    }
                }
                Ad::TxPower(dbm) => a.tx_power_dbm = Some(dbm),
                Ad::Uuids { uuids, .. } => {
                    a.services.extend(uuids.iter().map(|u| ad::uuid_text(u)));
                }
                Ad::ServiceData { uuid, data } => {
                    a.service_data
                        .push(format!("{}={}", ad::uuid_text(&uuid), bytes(&data)))
                }
                Ad::Manufacturer { company: id, data } => {
                    a.company_id = Some(id);
                    a.company = crate::signal::assigned::company(id);
                    a.mfr_data.push(bytes(&data));
                }
                Ad::Other { code, data } => {
                    a.other_ad.push(format!("0x{code:02X}={}", bytes(&data)))
                }
            },
        }
    }
    a
}

/// The packets given, in their order, each as the lists show it.
pub fn records(state: &SdrMetrics, packets: Vec<&BlePacket>) -> Vec<Packet> {
    let now = std::time::Instant::now();
    let s = |u: Option<Uncertain>| Sigma::of(u);
    packets
        .into_iter()
        .map(|p| {
            let carrier = crate::signal::ble::channel::centre_hz(p.channel);
            let offset =
                |hz: Uncertain| carrier.map(|c| state.radio.transmitter_offset(hz, c as f64, now));
            let cfo = p.freq_offset_hz.and_then(offset);
            let measured = p.crc_ok;
            // An LE Coded packet's readings are its own suite's; each packet
            // carries one kind or the other, never both.
            let coded = p
                .coded
                .as_ref()
                .and_then(|c| c.reading)
                .filter(|_| measured);
            let drift = p.drift.or(coded.and_then(|r| r.drift)).filter(|_| measured);
            let quality = p.modulation.filter(|_| measured);
            let df1 = quality
                .map(|q| q.delta_f1_avg_hz)
                .or(coded.and_then(|r| r.modulation).map(|m| m.delta_f1_avg_hz));
            let targeted = matches!(
                p.pdu_type,
                PduType::AdvDirectInd | PduType::ScanReq | PduType::ConnectInd
            );
            let ch_sel_defined = matches!(
                p.pdu_type,
                PduType::AdvInd | PduType::AdvDirectInd | PduType::ConnectInd
            );
            let address_kind = |a| crate::signal::ble::address::kind(a, p.tx_add_random).label();
            Packet {
                seq: p.seq,
                age_s: now.saturating_duration_since(p.seen).as_secs(),
                channel: p.channel,
                phy: p.phy.label(),
                pdu_type: p
                    .ext
                    .as_ref()
                    .map_or(p.pdu_type.label(), |e| e.role.label().to_string()),
                crc_ok: p.crc_ok,
                length: p.length,
                address: p
                    .adv_addr
                    .map(|a| state.net.show_address(a, p.tx_add_random, None)),
                address_kind: p.adv_addr.map(address_kind),
                tx_add: if p.tx_add_random { "random" } else { "public" },
                rx_add: targeted.then_some(if p.rx_add_random { "random" } else { "public" }),
                ch_sel: ch_sel_defined.then_some(p.ch_sel),
                advertised: advertised(p, &state.net),
                snr_db: p.snr_db,
                cfo_khz: s(cfo.map(|t| t.khz)),
                cfo_ppm: s(cfo.map(|t| t.ppm)),
                start_khz: s(drift.map(|d| d.initial_hz).and_then(offset).map(|t| t.khz)),
                end_khz: s(drift.map(|d| d.final_hz).and_then(offset).map(|t| t.khz)),
                mod_index: s(quality.map(|q| q.modulation_index)),
                df1_avg_khz: s(df1.map(|d| d.scale(1e-3))),
                df2_avg_khz: s(quality.map(|q| q.delta_f2_avg_hz.scale(1e-3))),
                df2_df1_ratio: s(quality.map(|q| q.ratio)),
                drift_khz: s(drift.map(|d| d.drift_hz.scale(1e-3))),
                drift_rate_hz_per_us: s(drift.map(|d| d.drift_rate_hz_per_us)),
                fec_repairs: p.coded.as_ref().map(|c| c.fec_repairs),
                sid: p.ext.as_ref().and_then(|e| e.header.adi).map(|a| a.sid),
                aux: p.ext.as_ref().map(|e| aux_word(e.aux)),
            }
        })
        .collect()
}

fn group(
    m: &SdrMetrics,
    clock: Clock,
    note: Option<String>,
    packets: Vec<Packet>,
    since: Option<u64>,
) -> Group<Packets> {
    Group {
        testimony: Testimony::resolve(
            m,
            Staleness::NotStreaming,
            Some(FeedSpan::Session),
            m.net.health.last_block.map(|t| clock.unix_ms_of(t)),
        ),
        readings: Packets {
            note,
            packets: packets
                .into_iter()
                .filter(|p| since.is_none_or(|s| p.seq > s))
                .collect(),
        },
    }
}

/// The LE 1M and 2M list, as it is showing.
pub fn ble(m: &SdrMetrics, clock: Clock, since: Option<u64>) -> Group<Packets> {
    let note = crate::export::ble::note(m);
    group(m, clock, note, records(m, m.net.ble_shown()), since)
}

/// The LE Coded list, as it is showing.
pub fn coded(m: &SdrMetrics, clock: Clock, since: Option<u64>) -> Group<Packets> {
    let note = Some(crate::export::ble::coded_note(m));
    group(m, clock, note, records(m, m.net.coded_shown()), since)
}
