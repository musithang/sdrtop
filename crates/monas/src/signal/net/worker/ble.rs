// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! What a decoded LE packet becomes in the state: its list, the census, what
//! it advertised, and the tester's reading of LE Coded. And putting a
//! receiver down, so a capture it had under way still ends in the funnel.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::signal::ble::receive::Receiver as BleReceiver;
use crate::state::{BlePacket, SdrMetrics};

/// An LE Coded packet measured as the test suite defines it, from the
/// symbols its decoded bits were sent as: `None` when its samples are not
/// held, or it is not a Coded packet.
pub(super) fn measure_coded(
    p: &crate::signal::ble::pdu::Packet,
    channel: u8,
    window: &crate::signal::net::measure::Recent,
    rate_hz: f64,
    centre_hz: f64,
) -> Option<crate::signal::net::measure::CodedReading> {
    let coding = p.coding?;
    let symbols = crate::signal::ble::coded::symbols_of(
        crate::signal::ble::detect::ADVERTISING_ACCESS_ADDRESS,
        coding,
        &p.air,
    );
    let offset = crate::signal::ble::channel::centre_hz(channel)? as f64 - centre_hz;
    crate::signal::net::measure::le_coded(
        window,
        rate_hz,
        offset,
        p.at_pair? as f64,
        &symbols,
        coding,
    )
}

/// An extended advertising PDU's header, read, in `role`: `None` for any
/// other type, a failed CRC (whose header bytes say nothing), or a payload
/// that is not the extended format.
pub(super) fn extended(
    p: &crate::signal::ble::pdu::Packet,
    role: crate::state::ExtRole,
) -> Option<crate::state::ExtInfo> {
    if !p.crc_ok || p.pdu_type != crate::signal::ble::pdu::PduType::Other(0x07) {
        return None;
    }
    let header = crate::signal::ble::ext::parse(&p.payload).ok()?;
    Some(crate::state::ExtInfo {
        header,
        role,
        aux: crate::signal::ble::aux_ptr::AuxOutcome::NonePromised,
    })
}

/// An LE Coded packet into the LE Coded list, numbered as it arrives.
pub(super) fn push_coded(
    m: &mut SdrMetrics,
    p: crate::signal::ble::pdu::Packet,
    channel: u8,
    reading: Option<crate::signal::net::measure::CodedReading>,
    ext: Option<crate::state::ExtInfo>,
    now: Instant,
) {
    let Some(coding) = p.coding else { return };
    m.net.coded_heard += 1;
    let seq = m.net.coded_heard;
    // An auxiliary packet names its advertiser in its extended header.
    let adv_addr = p
        .adv_addr
        .or_else(|| ext.as_ref().and_then(|e| e.header.adv_a));
    m.net.coded_packets.push_front(BlePacket {
        seq,
        phy: crate::signal::ble::Phy::Coded(coding),
        channel,
        pdu_type: p.pdu_type,
        ch_sel: p.ch_sel,
        tx_add_random: p.tx_add_random,
        rx_add_random: p.rx_add_random,
        length: p.length,
        adv_addr,
        payload: p.payload,
        crc_ok: p.crc_ok,
        snr_db: reading.and_then(|r| r.snr_db),
        // The preamble's f0, as BV-14-C reads it, is the carrier offset the
        // list's column shows.
        freq_offset_hz: reading.and_then(|r| r.drift).map(|d| d.initial_hz),
        modulation: None,
        drift: None,
        seen: now,
        coded: Some(crate::state::CodedFacts {
            fec_repairs: p.fec_repairs.unwrap_or(0),
            reading,
        }),
        ext,
    });
    m.net.coded_packets.truncate(crate::state::BLE_PACKET_LIMIT);
}

/// An auxiliary packet heard for an LE advertisement, into the LE list,
/// numbered as it arrives: its readings the receiver's and the tester's on
/// LE 1M and LE 2M, LE Coded's own on LE Coded.
pub(super) fn push_le_aux(
    m: &mut SdrMetrics,
    p: crate::signal::ble::pdu::Packet,
    channel: u8,
    phy: crate::signal::ble::Phy,
    reading: Option<crate::signal::net::measure::CodedReading>,
    ext: crate::state::ExtInfo,
    now: Instant,
) {
    m.net.ble_heard += 1;
    let seq = m.net.ble_heard;
    let adv_addr = p.adv_addr.or(ext.header.adv_a);
    if let Some(addr) = adv_addr {
        m.net.address_book.number(addr);
    }
    let coded = p.coding.map(|_| crate::state::CodedFacts {
        fec_repairs: p.fec_repairs.unwrap_or(0),
        reading,
    });
    let (snr_db, freq_offset_hz) = match coded {
        Some(_) => (
            reading.and_then(|r| r.snr_db),
            reading.and_then(|r| r.drift).map(|d| d.initial_hz),
        ),
        None => (p.snr_db, p.freq_offset_hz),
    };
    m.net.ble_packets.push_front(BlePacket {
        seq,
        phy,
        channel,
        pdu_type: p.pdu_type,
        ch_sel: p.ch_sel,
        tx_add_random: p.tx_add_random,
        rx_add_random: p.rx_add_random,
        length: p.length,
        adv_addr,
        payload: p.payload,
        crc_ok: p.crc_ok,
        snr_db,
        freq_offset_hz,
        modulation: p.modulation,
        drift: p.drift,
        seen: now,
        coded,
        ext: Some(ext),
    });
    m.net.trim_ble_packets();
}

/// Put the BLE receiver down, if there is one: a capture it had under way
/// ends as given up in the funnel rather than with the receiver.
pub(super) fn put_down_ble(ble: &mut Option<BleReceiver>, state: &Arc<Mutex<SdrMetrics>>) {
    let Some(rx) = ble.take() else { return };
    let funnel = rx.finish();
    if !funnel.is_empty() {
        let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
        m.net.health.ble.add(funnel);
    }
}

/// Put the LE Coded receiver down, if there is one: a capture it had under
/// way ends as given up in the funnel rather than with the receiver.
pub(super) fn put_down(
    coded: &mut Option<crate::signal::ble::coded_rx::CodedReceiver>,
    state: &Arc<Mutex<SdrMetrics>>,
) {
    let Some(rx) = coded.take() else { return };
    let funnel = rx.finish();
    if funnel != crate::signal::ble::receive::Funnel::default() {
        let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
        m.net.health.coded.add(funnel);
    }
}

/// Fold one decoded BLE packet into the shared census, if it earns a place
/// there.
///
/// **The census counts confirmed transmitters, not decode attempts.** An
/// address from a packet whose CRC did not pass is not a device this receiver
/// has actually confirmed, and counting it would be exactly the invented
/// reading rule 2 refuses. "Every device in the room" means the CRC-clean ones,
/// the only kind this can honestly claim to have found. A failed packet can
/// still count *against* a device already confirmed, as a CRC failure on an
/// exact address match (`census::observe_crc_failure` says why that much is
/// safe); it never makes a row.
///
/// Pulled out of [`NetWorker::run`](super::NetWorker::run)'s own loop as a
/// plain function of a packet and a clock, rather than tested only by building
/// a real, noisy capture through the whole receive chain to get
/// one - `census::observe`'s own tests already hold the census half of this to
/// account; what only this function does is decide *whether*, and *which*, to
/// call.
///
/// Runs inside the state lock, once per packet: a lookup, a few additions
/// and an inverse-variance fold, and nothing that waits on a square root
/// (the census module doc).
///
/// **An arrival is kept only in LOCK, and only from periodic advertising.**
/// While surveying, the dwell schedule decides which packets are heard, so
/// gaps between them would time the survey (`signal::ble::interval`). And a
/// scan response is an answer to a scanner, sent whenever it asked, so it
/// times the scanner: only the four advertising PDUs, which the Link Layer
/// sends once an event, are timed.
pub(super) fn census_from_ble(
    devices: &mut Vec<crate::signal::net::census::Device>,
    p: &crate::signal::ble::pdu::Packet,
    channel: u8,
    locked: bool,
    rate_hz: f64,
    now: Instant,
) {
    use crate::signal::ble::pdu::PduType;
    let Some(address) = p.adv_addr else {
        return;
    };
    if !p.crc_ok {
        crate::signal::net::census::observe_crc_failure(devices, address, p.snr_db);
        return;
    }
    // In ppm of the channel it was heard on, so readings from all three
    // advertising channels can be combined: see `Device::crystal_offset_ppm`.
    let carrier = crate::signal::ble::channel::centre_hz(channel);
    let crystal_offset_ppm = p
        .freq_offset_hz
        .zip(carrier)
        .map(|(hz, c)| crate::state::offset_ppm(hz, c as f64));
    let sighting = crate::signal::net::census::Sighting {
        address,
        random: p.tx_add_random,
        snr_db: p.snr_db,
        crystal_offset_ppm,
        ble_pdu_code: Some(p.pdu_type.code()),
        modulation_index: p.modulation.map(|m| m.modulation_index),
        arrival: p
            .at_pair
            .filter(|_| {
                locked
                    && matches!(
                        p.pdu_type,
                        PduType::AdvInd
                            | PduType::AdvDirectInd
                            | PduType::AdvNonconnInd
                            | PduType::AdvScanInd
                    )
            })
            .map(|pair| crate::signal::net::census::Arrival {
                channel,
                rate_hz,
                pair,
            }),
    };
    crate::signal::net::census::observe(devices, &sighting, now);
}

/// An extended advertiser into the census, from the auxiliary packet its
/// AuxPtr was followed to: the packet that carries its address (taken into
/// `p`, which names none of its own) and its advertising data. Counted with
/// no arrival, as `census_from_ble` counts any type but legacy advertising:
/// an aux comes when its primary's offset says, not on an interval.
pub(super) fn census_from_aux(
    m: &mut SdrMetrics,
    p: &mut crate::signal::ble::pdu::Packet,
    ext: &crate::state::ExtInfo,
    channel: u8,
    rate_hz: f64,
    now: Instant,
) {
    p.adv_addr = p.adv_addr.or(ext.header.adv_a);
    let Some(address) = p.adv_addr else { return };
    let locked = m.net.mode == crate::state::NetMode::Lock;
    census_from_ble(&mut m.net.census.devices, p, channel, locked, rate_hz, now);
    let said = crate::signal::ble::ad::Advertised::from_structures(&crate::signal::ble::ad::parse(
        &ext.header.adv_data,
    ));
    if p.crc_ok && !said.is_empty() {
        m.net.advertised.entry(address).or_default().merge(said);
    }
}

/// The address a packet came from and what it advertised about itself
/// (`signal::ble::ad::Advertised`), for `NetState::advertised`; `None` where
/// it said nothing beyond its address.
///
/// **Only from a packet whose CRC passed**: a failed one could name a company
/// or a device nobody sent, and then name it for every packet that device
/// sends after.
pub(super) fn advertised_of(
    p: &crate::signal::ble::pdu::Packet,
) -> Option<([u8; 6], crate::signal::ble::ad::Advertised)> {
    use crate::signal::ble::ad;
    if !p.crc_ok {
        return None;
    }
    let address = p.adv_addr?;
    let data = ad::adv_data(p.pdu_type, &p.payload)?;
    let said = ad::Advertised::from_structures(&ad::parse(data));
    (!said.is_empty()).then_some((address, said))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A packet whose CRC did not pass is not a confirmed transmitter - rule
    /// 2 refuses to count an address this receiver has not actually
    /// verified. A plain function of a packet and a clock, tested directly
    /// as one rather than by building a real, noisy capture through the
    /// whole receive chain to get a CRC-failing decode out the other end -
    /// see [`census_from_ble`]'s own doc for why.
    #[test]
    fn a_failed_crc_does_not_reach_the_census() {
        let mut devices = Vec::new();
        let mut packet = crate::signal::ble::pdu::decode(&[false; 40]).unwrap();
        packet.crc_ok = false;
        packet.adv_addr = Some([1, 2, 3, 4, 5, 6]);
        census_from_ble(&mut devices, &packet, 37, false, 8e6, Instant::now());
        assert!(devices.is_empty(), "{devices:?}");
    }

    /// The other half of the same gate: a confirmed packet with an address
    /// does reach the census.
    #[test]
    fn a_passed_crc_with_an_address_reaches_the_census() {
        let mut devices = Vec::new();
        let mut packet = crate::signal::ble::pdu::decode(&[false; 40]).unwrap();
        packet.crc_ok = true;
        packet.adv_addr = Some([1, 2, 3, 4, 5, 6]);
        census_from_ble(&mut devices, &packet, 37, false, 8e6, Instant::now());
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].address, [1, 2, 3, 4, 5, 6]);
    }

    /// **The company comes from CRC-good manufacturer data only**: the real
    /// ADV_NONCONN_IND's 0x004C is taken, the same octets with a failed CRC
    /// are not, and a packet with no manufacturer data names nothing.
    #[test]
    fn what_is_advertised_is_read_only_from_a_packet_whose_crc_passed() {
        use crate::signal::ble::pdu::{air_octets, PduType};
        let addr = [0xd1, 0x9a, 0x7e, 0x91, 0x27, 0x9e];
        let mut packet = crate::signal::ble::pdu::decode(&[false; 40]).unwrap();
        packet.pdu_type = PduType::AdvNonconnInd;
        packet.adv_addr = Some(addr);
        packet.payload = air_octets(addr).to_vec();
        packet
            .payload
            .extend_from_slice(&[0x07, 0xff, 0x4c, 0x00, 0x12, 0x02, 0x00, 0x02]);
        packet.crc_ok = true;
        let company = |p| advertised_of(p).and_then(|(a, said)| said.company.map(|c| (a, c)));
        assert_eq!(company(&packet), Some((addr, 0x004C)));
        packet.crc_ok = false;
        assert_eq!(advertised_of(&packet), None);
        packet.crc_ok = true;
        packet.payload.truncate(6);
        assert_eq!(advertised_of(&packet), None);
    }

    /// **Every packet counts in its SNR bin, good or failed**: a good one in
    /// the device's curve and the section's, a failed one from the same
    /// address in both too (the exact-address rule).
    #[test]
    fn packets_fill_the_frame_error_curves_by_snr() {
        use crate::signal::ble::fer::bin_of;
        let mut devices = Vec::new();
        let mut packet = crate::signal::ble::pdu::decode(&[false; 40]).unwrap();
        packet.adv_addr = Some([1, 2, 3, 4, 5, 6]);
        packet.snr_db = Some(13.0);
        packet.crc_ok = true;
        census_from_ble(&mut devices, &packet, 37, false, 8e6, Instant::now());
        packet.crc_ok = false;
        census_from_ble(&mut devices, &packet, 37, false, 8e6, Instant::now());
        let fer = &devices[0].fer;
        assert_eq!((fer.good[bin_of(13.0)], fer.failed[bin_of(13.0)]), (1, 1));
    }

    /// **Only LOCK's periodic advertising is timed.** An ADV_IND in LOCK
    /// is kept with its stream position and channel; the same packet while
    /// surveying is not, and neither is a scan response in LOCK, which is
    /// timed by whoever scanned.
    #[test]
    fn only_periodic_advertising_in_lock_is_timed() {
        use crate::signal::ble::pdu::PduType;
        let mut packet = crate::signal::ble::pdu::decode(&[false; 40]).unwrap();
        packet.crc_ok = true;
        packet.adv_addr = Some([1, 2, 3, 4, 5, 6]);
        packet.at_pair = Some(123_456);
        let now = Instant::now();

        packet.pdu_type = PduType::AdvInd;
        let mut devices = Vec::new();
        census_from_ble(&mut devices, &packet, 38, true, 8e6, now);
        let log = devices[0].arrivals.clone().expect("timed in LOCK");
        assert_eq!((log.channel, log.pairs[0]), (38, 123_456));

        let mut surveying = Vec::new();
        census_from_ble(&mut surveying, &packet, 38, false, 8e6, now);
        assert!(surveying[0].arrivals.is_none());

        packet.pdu_type = PduType::ScanRsp;
        let mut answered = Vec::new();
        census_from_ble(&mut answered, &packet, 38, true, 8e6, now);
        assert!(answered[0].arrivals.is_none());
    }

    /// **What the packet measured reaches the record**: its PDU type and,
    /// where one could be taken, its modulation index; and a later packet
    /// from the same address whose CRC failed counts against it.
    #[test]
    fn a_packet_carries_its_type_and_modulation_and_a_failure_counts_against_it() {
        use crate::signal::dsp::uncertainty::Uncertain;
        let mut devices = Vec::new();
        let mut packet = crate::signal::ble::pdu::decode(&[false; 40]).unwrap();
        packet.crc_ok = true;
        packet.adv_addr = Some([1, 2, 3, 4, 5, 6]);
        packet.pdu_type = crate::signal::ble::pdu::PduType::ScanRsp;
        packet.modulation = Some(crate::signal::ble::measure::ModulationQuality {
            delta_f1_avg_hz: Uncertain::from_sigma(250e3, 5e3),
            delta_f2_avg_hz: crate::signal::dsp::uncertainty::Uncertain::from_sigma(230e3, 4e3),
            modulation_index: Uncertain::from_sigma(0.5, 0.01),
            ratio: Uncertain::from_sigma(0.9, 0.02),
        });
        census_from_ble(&mut devices, &packet, 37, false, 8e6, Instant::now());
        assert_eq!(devices[0].ble_pdu_codes().collect::<Vec<_>>(), vec![0x4]);
        assert_eq!(devices[0].modulation_index.unwrap().value(), 0.5);

        packet.crc_ok = false;
        census_from_ble(&mut devices, &packet, 37, false, 8e6, Instant::now());
        assert_eq!((devices[0].packets, devices[0].crc_failed), (1, 1));
    }

    /// **The census keeps ppm of the channel a packet was heard on**, so one
    /// crystal reads one number on all three advertising channels: 10 ppm is
    /// 24.02 kHz on channel 37 (2402 MHz) and 24.80 kHz on 39 (2480 MHz).
    /// Combined in Hz, the two would have disagreed by 3 % about one clock.
    #[test]
    fn one_crystal_reads_one_ppm_on_every_channel() {
        use crate::signal::dsp::uncertainty::Uncertain;
        let mut devices = Vec::new();
        let mut packet = crate::signal::ble::pdu::decode(&[false; 40]).unwrap();
        packet.crc_ok = true;
        packet.adv_addr = Some([1, 2, 3, 4, 5, 6]);
        packet.freq_offset_hz = Some(Uncertain::from_sigma(24_020.0, 100.0));
        census_from_ble(&mut devices, &packet, 37, false, 8e6, Instant::now());
        packet.freq_offset_hz = Some(Uncertain::from_sigma(24_800.0, 100.0));
        census_from_ble(&mut devices, &packet, 39, false, 8e6, Instant::now());
        let ppm = devices[0].crystal_offset_ppm.unwrap();
        assert!((ppm.value() - 10.0).abs() < 1e-9, "got {}", ppm.value());
    }

    /// A confirmed packet with no address at all - a PDU type that carries
    /// none, per `pdu::decode`'s own contract - has nothing to key a census
    /// row on, and is skipped rather than inventing an address.
    #[test]
    fn a_passed_crc_with_no_address_is_skipped() {
        let mut devices = Vec::new();
        let mut packet = crate::signal::ble::pdu::decode(&[false; 40]).unwrap();
        packet.crc_ok = true;
        packet.adv_addr = None;
        census_from_ble(&mut devices, &packet, 37, false, 8e6, Instant::now());
        assert!(devices.is_empty(), "{devices:?}");
    }
}
