// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The BLE packets, one to a row (net-ux-polish-plan 5.6).
//!
//! The third body behind the section's one export key, beside the band and the
//! census, sharing their provenance header and destination handling; N20 built
//! the seam so a new body is one more file, not a new key.
//!
//! **What the list shows, in its order.** The rows are `NetState::ble_shown`,
//! the account the packet list draws from, so a file taken while the list was
//! held or filtered is that list, and its note says so ([`note`]). Newest
//! first, as on screen.
//!
//! **Every reading with its uncertainty, and blank wherever the screen would
//! not state it.** Each uncertain figure is a value column and a sigma column.
//! What the panels refuse, the file leaves blank: the advertised structures
//! of a packet whose CRC failed (the list's NAME column and the detail's
//! ADVERTISED section read nothing from one), its modulation and drift (5.4.c),
//! a start and end frequency where no drift was measured, ChSel on a type
//! that reserves it, RxAdd on a type with no target address.
//!
//! **Names are the readings' neighbours, never their replacements.** A
//! company is its identifier with the SIG snapshot's name in the next column;
//! services are their UUIDs as written; a local name went through
//! `ad::printable`, so a control character a device put in it cannot break a
//! row or a terminal that shows the file.

use crate::signal::ble::ad::{self, Ad, Structure};
use crate::signal::ble::pdu::PduType;
use crate::signal::dsp::uncertainty::Uncertain;
use crate::state::{BlePacket, SdrMetrics};

/// The columns, in order. Offsets are corrected for our oscillator exactly as
/// the panels show them; the provenance header's `reference` line says what
/// they are worth and its `addresses` line how the address column is shown.
pub const HEADER: &str = "seq,age_s,channel,phy,pdu_type,crc_ok,length,\
address,address_kind,tx_add,rx_add,ch_sel,\
name,name_complete,flags,tx_power_dbm,services,service_data,\
company_id,company,mfr_data,other_ad,malformed,\
snr_db,cfo_khz,cfo_khz_sigma,cfo_ppm,cfo_ppm_sigma,\
start_khz,start_khz_sigma,end_khz,end_khz_sigma,\
mod_index,mod_index_sigma,df1_avg_khz,df1_avg_khz_sigma,df2_max_khz,\
df2_df1_ratio,df2_df1_ratio_sigma,drift_khz,drift_khz_sigma,\
drift_rate_hz_per_us,drift_rate_hz_per_us_sigma";

/// Why the list the file was taken from is not every packet, if it is not.
pub fn note(state: &SdrMetrics) -> Option<String> {
    let view = &state.net.ble_view;
    let mut parts = Vec::new();
    if view.held.is_some() {
        parts.push(format!(
            "the list was held; {} packets arrived after it and are not in this file",
            state.net.ble_behind()
        ));
    }
    if view.filter.is_some() {
        parts.push("the list was filtered to one advertiser address".to_string());
    }
    (!parts.is_empty()).then(|| parts.join("; "))
}

/// A value and its sigma to `places`, or two blanks.
fn with_sigma(u: Option<Uncertain>, places: usize) -> [String; 2] {
    match u {
        Some(u) if u.value().is_finite() && u.sigma().is_finite() => [
            format!("{:.places$}", u.value()),
            format!("{:.places$}", u.sigma()),
        ],
        _ => [String::new(), String::new()],
    }
}

/// The eleven advertising columns, `name` through `malformed`, flattened from
/// the packet's AD structures; blank where the packet carries none, or where
/// its CRC failed.
fn advertised(p: &BlePacket) -> Vec<String> {
    let mut name = String::new();
    let mut complete = String::new();
    let mut flags = Vec::new();
    let mut tx_power = String::new();
    let mut services = Vec::new();
    let mut service_data = Vec::new();
    let mut company_id = String::new();
    let mut company = String::new();
    let mut mfr_data = Vec::new();
    let mut other = Vec::new();
    let mut malformed = String::new();
    let readable = p.crc_ok && p.pdu_type != PduType::Other(0x07);
    let data = ad::adv_data(p.pdu_type, &p.payload).filter(|_| readable);
    for structure in data.map(ad::parse).unwrap_or_default() {
        match structure {
            Structure::Malformed { offset, why } => malformed = format!("octet {offset}: {why}"),
            Structure::Ad { ad, .. } => match ad {
                Ad::Flags(bits) => flags.extend(ad::flag_names(bits)),
                Ad::Name { complete: c, text } => {
                    // The complete name wins, as on screen (`ad::name`).
                    if name.is_empty() || c {
                        name = ad::printable(&text);
                        complete = c.to_string();
                    }
                }
                Ad::TxPower(dbm) => tx_power = dbm.to_string(),
                Ad::Uuids { uuids, .. } => {
                    services.extend(uuids.iter().map(|u| ad::uuid_text(u)));
                }
                Ad::ServiceData { uuid, data } => service_data.push(format!(
                    "{}={}",
                    ad::uuid_text(&uuid),
                    ad::hex(&data).replace(' ', "")
                )),
                Ad::Manufacturer { company: id, data } => {
                    company_id = format!("0x{id:04X}");
                    company = crate::signal::ble::assigned::company(id)
                        .unwrap_or_default()
                        .to_string();
                    mfr_data.push(ad::hex(&data).replace(' ', ""));
                }
                Ad::Other { code, data } => {
                    other.push(format!("0x{code:02X}={}", ad::hex(&data).replace(' ', "")))
                }
            },
        }
    }
    vec![
        name,
        complete,
        flags.join(";"),
        tx_power,
        services.join(";"),
        service_data.join(";"),
        company_id,
        company,
        mfr_data.join(";"),
        other.join(";"),
        malformed,
    ]
}

/// The packets as CSV rows, in the order the list is showing them.
pub fn rows(state: &SdrMetrics) -> Vec<String> {
    let now = std::time::Instant::now();
    state
        .net
        .ble_shown()
        .into_iter()
        .map(|p| {
            let carrier = crate::signal::ble::channel::centre_hz(p.channel);
            let offset =
                |hz: Uncertain| carrier.map(|c| state.radio.transmitter_offset(hz, c as f64, now));
            let cfo = p.freq_offset_hz.and_then(offset);
            let measured = p.crc_ok;
            let drift = p.drift.filter(|_| measured);
            let quality = p.modulation.filter(|_| measured);
            let targeted = matches!(
                p.pdu_type,
                PduType::AdvDirectInd | PduType::ScanReq | PduType::ConnectInd
            );
            let ch_sel_defined = matches!(
                p.pdu_type,
                PduType::AdvInd | PduType::AdvDirectInd | PduType::ConnectInd
            );

            let mut f = vec![
                p.seq.to_string(),
                now.saturating_duration_since(p.seen).as_secs().to_string(),
                p.channel.to_string(),
                p.phy.label().to_string(),
                p.pdu_type.label(),
                p.crc_ok.to_string(),
                p.length.to_string(),
                p.adv_addr
                    .map(|a| state.net.show_address(a, p.tx_add_random, None))
                    .unwrap_or_default(),
                p.adv_addr
                    .map(|a| {
                        crate::signal::ble::address::kind(a, p.tx_add_random)
                            .label()
                            .to_string()
                    })
                    .unwrap_or_default(),
                if p.tx_add_random { "random" } else { "public" }.to_string(),
                if targeted {
                    if p.rx_add_random { "random" } else { "public" }.to_string()
                } else {
                    String::new()
                },
                if ch_sel_defined {
                    p.ch_sel.to_string()
                } else {
                    String::new()
                },
            ];
            f.extend(advertised(p));
            f.push(p.snr_db.map(|db| format!("{db:.1}")).unwrap_or_default());
            f.extend(with_sigma(cfo.map(|t| t.khz), 2));
            f.extend(with_sigma(cfo.map(|t| t.ppm), 2));
            for end in [drift.map(|d| d.initial_hz), drift.map(|d| d.final_hz)] {
                f.extend(with_sigma(end.and_then(offset).map(|t| t.khz), 2));
            }
            f.extend(with_sigma(quality.map(|q| q.modulation_index), 4));
            f.extend(with_sigma(
                quality.map(|q| q.delta_f1_avg_hz.scale(1e-3)),
                2,
            ));
            f.push(
                quality
                    .map(|q| format!("{:.2}", q.delta_f2_max_hz * 1e-3))
                    .unwrap_or_default(),
            );
            f.extend(with_sigma(quality.map(|q| q.ratio), 3));
            f.extend(with_sigma(drift.map(|d| d.drift_hz.scale(1e-3)), 2));
            f.extend(with_sigma(drift.map(|d| d.drift_rate_hz_per_us), 2));
            f.iter()
                .map(|field| super::csv_field(field).into_owned())
                .collect::<Vec<_>>()
                .join(",")
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::ble::measure::{Drift, ModulationQuality};
    use crate::signal::ble::Phy;
    use std::time::Instant;

    /// One CSV row into its fields, honouring RFC 4180 quoting.
    fn fields(row: &str) -> Vec<String> {
        let mut out = vec![String::new()];
        let mut quoted = false;
        let mut chars = row.chars().peekable();
        while let Some(c) = chars.next() {
            match (c, quoted) {
                ('"', true) if chars.peek() == Some(&'"') => {
                    chars.next();
                    out.last_mut().unwrap().push('"');
                }
                ('"', _) => quoted = !quoted,
                (',', false) => out.push(String::new()),
                (c, _) => out.last_mut().unwrap().push(c),
            }
        }
        out
    }

    fn column(name: &str) -> usize {
        HEADER
            .split(',')
            .position(|h| h == name)
            .unwrap_or_else(|| panic!("no column {name}"))
    }

    fn get(row: &[String], name: &str) -> String {
        row[column(name)].clone()
    }

    /// A CRC-good ADV_NONCONN_IND carrying `ad`, with every physical reading
    /// the receiver can give.
    fn packet(seq: u64, ad: &[u8]) -> BlePacket {
        let addr = [0x4a, 0x11, 0x22, 0x33, 0x09, 0xbe];
        let mut payload = crate::signal::ble::pdu::air_octets(addr).to_vec();
        payload.extend_from_slice(ad);
        let initial = Uncertain::from_sigma(-22_000.0, 400.0);
        let fin = Uncertain::from_sigma(-19_000.0, 400.0);
        let drift = fin.difference(&initial);
        BlePacket {
            seq,
            phy: Phy::OneM,
            channel: 37,
            pdu_type: PduType::AdvNonconnInd,
            ch_sel: false,
            tx_add_random: true,
            rx_add_random: false,
            length: payload.len() as u8,
            adv_addr: Some(addr),
            payload,
            crc_ok: true,
            snr_db: Some(14.2),
            freq_offset_hz: Some(Uncertain::from_sigma(-21_000.0, 500.0)),
            modulation: Some(ModulationQuality {
                delta_f1_avg_hz: Uncertain::from_sigma(248_000.0, 1_500.0),
                delta_f2_max_hz: 231_000.0,
                modulation_index: Uncertain::from_sigma(0.496, 0.003),
                ratio: Uncertain::from_sigma(0.91, 0.02),
            }),
            drift: Some(Drift {
                initial_hz: initial,
                final_hz: fin,
                drift_hz: drift,
                drift_rate_hz_per_us: drift.scale(0.02),
            }),
            seen: Instant::now(),
        }
    }

    fn state_with(packets: Vec<BlePacket>) -> SdrMetrics {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.ble_heard = packets.len() as u64;
        for p in packets {
            m.net.ble_packets.push_front(p);
        }
        m
    }

    /// **The real packet from the air**: company 0x004C named Apple, Inc.
    /// (quoted, since the name holds a comma), its octets, and every row
    /// the width of the header.
    #[test]
    fn the_real_packet_exports_its_company_and_its_physics() {
        let m = state_with(vec![packet(
            1,
            &[0x07, 0xff, 0x4c, 0x00, 0x12, 0x02, 0x00, 0x02],
        )]);
        let rows = rows(&m);
        assert!(rows[0].contains("\"Apple, Inc.\""), "{}", rows[0]);
        let row = fields(&rows[0]);
        assert_eq!(row.len(), HEADER.split(',').count(), "{row:?}");
        assert_eq!(get(&row, "company_id"), "0x004C");
        assert_eq!(get(&row, "company"), "Apple, Inc.");
        assert_eq!(get(&row, "mfr_data"), "12020002");
        assert_eq!(get(&row, "address_kind"), "RPA");
        assert_eq!(get(&row, "phy"), "LE 1M");
        assert_eq!(get(&row, "cfo_khz"), "-21.00");
        assert_eq!(get(&row, "cfo_khz_sigma"), "0.50");
        assert_eq!(get(&row, "mod_index"), "0.4960");
        assert_eq!(get(&row, "mod_index_sigma"), "0.0030");
        assert_eq!(get(&row, "start_khz"), "-22.00");
        assert_eq!(get(&row, "end_khz"), "-19.00");
        // ChSel is reserved on ADV_NONCONN_IND, RxAdd too: blank, not false.
        assert_eq!(get(&row, "ch_sel"), "");
        assert_eq!(get(&row, "rx_add"), "");
    }

    /// **Blank wherever the screen would not state it**: a failed CRC keeps
    /// its SNR and CFO (the list shows them) and loses everything read from
    /// its bits: the advertised structures, the modulation, the drift, the
    /// start and end.
    #[test]
    fn a_failed_crc_exports_its_arrival_and_nothing_read_from_its_bits() {
        let mut p = packet(1, &[0x05, 0x09, b'S', b'e', b'n', b's']);
        p.crc_ok = false;
        let row = fields(&rows(&state_with(vec![p]))[0]);
        assert_eq!(get(&row, "snr_db"), "14.2");
        assert_eq!(get(&row, "cfo_khz"), "-21.00");
        for blank in [
            "name",
            "company_id",
            "mod_index",
            "df1_avg_khz",
            "df2_max_khz",
            "drift_khz",
            "start_khz",
            "end_khz",
        ] {
            assert_eq!(get(&row, blank), "", "{blank}: {row:?}");
        }
    }

    /// Every structure kind flattened, and a name's control characters never
    /// reach the file.
    #[test]
    fn the_advertised_structures_flatten_into_their_columns() {
        let m = state_with(vec![packet(
            1,
            &[
                0x02, 0x01, 0x06, // flags
                0x05, 0x03, 0x0f, 0x18, 0x0a, 0x18, // services
                0x05, 0x09, b'A', 0x1b, b'B', b'C', // name with an ESC
                0x02, 0x0a, 0xf4, // TX power
                0x05, 0x16, 0x0f, 0x18, 0x55, 0x66, // service data
                0x03, 0x19, 0x41, 0x03, // appearance, kept as other
            ],
        )]);
        let row = fields(&rows(&m)[0]);
        assert_eq!(
            get(&row, "flags"),
            "LE General Discoverable;BR/EDR Not Supported"
        );
        assert_eq!(get(&row, "services"), "0x180F;0x180A");
        assert_eq!(get(&row, "name"), "A\u{fffd}BC");
        assert_eq!(get(&row, "name_complete"), "true");
        assert_eq!(get(&row, "tx_power_dbm"), "-12");
        assert_eq!(get(&row, "service_data"), "0x180F=5566");
        assert_eq!(get(&row, "other_ad"), "0x19=4103");
        assert_eq!(get(&row, "malformed"), "");
    }

    /// A file taken from a held or filtered list says what it is short of.
    #[test]
    fn a_held_or_filtered_list_says_so_in_its_note() {
        let mut m = state_with(vec![packet(1, &[]), packet(2, &[])]);
        assert_eq!(note(&m), None);
        m.net.ble_view.held = Some((m.net.ble_packets.clone(), 1));
        m.net.ble_view.filter = Some([0x4a, 0x11, 0x22, 0x33, 0x09, 0xbe]);
        let said = note(&m).unwrap();
        assert!(said.contains("held; 1 packets arrived after it"), "{said}");
        assert!(
            said.contains("filtered to one advertiser address"),
            "{said}"
        );
    }
}
