// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The population, one transmitter to a row. The second body written, which is
//! the point: **one body proves nothing about a seam.** This and
//! [`super::occupancy`] share the provenance header and the destination
//! handling without either knowing about the other, which is the property a
//! later IQ-sample export needs to hold.
//!
//! **Every column the panel shows, and nothing it does not.** Each reading the
//! census panel prints with its uncertainty is written as two columns, the
//! value and its sigma, because a ppm figure without its uncertainty is the
//! number six months later nobody can judge. **Blank wherever the panel
//! dashes**, never a zero: a mean from one packet, an offset no packet
//! reported, an interval not yet read.
//! `the_export_is_blank_wherever_the_panel_ dashes` holds the two to one rule
//! on a populated census.
//!
//! An empty census exports its emptiness as what it is, with the reason in the
//! header: a quiet room, or nothing decoding addresses (`super::net_section`).

use crate::state::SdrMetrics;

/// The columns, in order. Offsets are corrected for our oscillator exactly as
/// the panel shows them, and the provenance header's `reference` line says
/// what they are worth; `addresses` says how the first column is shown.
pub const HEADER: &str = "address,kind,\
name,name_complete,tx_power_dbm,company_id,company,\
packets,crc_failed,crc_pass_pct,\
best_snr_db,mean_snr_db,mean_snr_sigma_db,\
crystal_offset_ppm,crystal_offset_sigma_ppm,\
modulation_index,modulation_index_sigma,pdu_types,\
adv_status,adv_interval_ms,adv_interval_sigma_ms,adv_events,adv_channel,\
adv_delay,adv_delay_width_ms,adv_grid,adv_grid_steps,adv_grid_off_ms,\
first_seen_s,last_seen_s";

/// A value and its sigma at `places`, or two blanks where the account has no
/// figure.
fn with_sigma(u: Option<crate::engine::view::net::Sigma>, places: usize) -> [String; 2] {
    match u {
        Some(u) => [
            format!("{:.places$}", u.value),
            format!("{:.places$}", u.sigma),
        ],
        None => [String::new(), String::new()],
    }
}

/// The census as CSV rows, in the order the panel is showing it: the
/// engine's account of each device
/// ([`crate::engine::view::net::census_records`]) written as text, blank
/// where the account has nothing.
pub fn rows(state: &SdrMetrics) -> Vec<String> {
    let opt = |v: Option<String>| v.unwrap_or_default();
    crate::engine::view::net::census_records(state)
        .into_iter()
        .map(|d| {
            let a = &d.advertising;
            let mut fields = vec![
                super::csv_field(&d.address).into_owned(),
                d.kind.to_string(),
                opt(d.name.as_deref().map(|n| super::csv_field(n).into_owned())),
                opt(d.name_complete.map(|c| c.to_string())),
                opt(d.tx_power_dbm.map(|dbm| dbm.to_string())),
                opt(d.company_id.map(|id| format!("0x{id:04X}"))),
                opt(d.company.map(|n| super::csv_field(n).into_owned())),
                d.packets.to_string(),
                d.crc_failed.to_string(),
                format!("{:.2}", d.crc_pass_pct),
                opt(d.best_snr_db.map(|db| format!("{db:.1}"))),
            ];
            fields.extend(with_sigma(d.mean_snr_db, 2));
            fields.extend(with_sigma(d.crystal_offset_ppm, 2));
            fields.extend(with_sigma(d.modulation_index, 4));
            fields.push(super::csv_field(&d.pdu_types.join(";")).into_owned());
            fields.push(a.status.clone());
            fields.extend(with_sigma(a.interval_ms, 3));
            fields.extend([
                opt(a.events.map(|e| e.to_string())),
                opt(a.channel.map(|c| c.to_string())),
                opt(a.delay.map(str::to_string)),
                opt(a.delay_width_ms.map(|w| format!("{w:.2}"))),
                opt(a.grid.map(str::to_string)),
                opt(a.grid_steps.clone()),
                opt(a.grid_off_ms.map(|o| format!("{o:.3}"))),
                d.first_seen_s.to_string(),
                d.last_seen_s.to_string(),
            ]);
            fields.join(",")
        })
        .collect()
}

/// A populated census for tests, here and the panel's.
#[cfg(any(test, feature = "testing"))]
pub mod sample {
    use crate::signal::dsp::uncertainty::Uncertain;
    use crate::signal::net::census::{column, observe, Arrival, Sighting};
    use crate::state::SdrMetrics;
    use std::time::Instant;

    /// A census as a room gives one: a device with every reading, one heard
    /// once, and one still being timed in LOCK. Sorted by address, so no
    /// title but ADDRESS carries the sort marker.
    pub fn room() -> SdrMetrics {
        let now = Instant::now();
        let mut m = SdrMetrics::fixture().streaming();
        m.net.mode = crate::state::NetMode::Lock;
        let sighting =
            |address, snr, ppm: Option<f64>, index: Option<f64>, pair: Option<u64>| Sighting {
                address,
                random: true,
                snr_db: snr,
                crystal_offset_ppm: ppm.map(|p| Uncertain::from_sigma(p, 0.4)),
                ble_pdu_code: Some(0x0),
                modulation_index: index.map(|i| Uncertain::from_sigma(i, 0.01)),
                arrival: pair.map(|pair| Arrival {
                    channel: 37,
                    rate_hz: 8e6,
                    pair,
                }),
            };
        let full = [0x4a, 0, 0, 0, 0, 1];
        let once = [0x4a, 0, 0, 0, 0, 2];
        let timing = [0x4a, 0, 0, 0, 0, 3];
        let devices = &mut m.net.census.devices;
        let mut t = 1.0;
        for k in 0..300u64 {
            t += 0.1 + (k * 7919 % 1000) as f64 * 1e-5;
            let snr = Some(10.0 + (k % 5) as f64);
            observe(
                devices,
                &sighting(full, snr, Some(-9.3), Some(0.5), Some((t * 8e6) as u64)),
                now,
            );
        }
        observe(devices, &sighting(once, Some(4.0), None, None, None), now);
        for k in 0..3u64 {
            let pair = Some(((1.0 + k as f64 * 0.2) * 8e6) as u64);
            observe(devices, &sighting(timing, Some(7.0), None, None, pair), now);
        }
        m.net.census.sort = column("ADDRESS");
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::dsp::uncertainty::Uncertain;
    use crate::signal::net::census::{column, Device};
    use std::time::{Duration, Instant};

    /// The index of `name` in [`HEADER`].
    fn at(name: &str) -> usize {
        HEADER
            .split(',')
            .position(|h| h == name)
            .unwrap_or_else(|| panic!("no column {name}"))
    }

    fn field<'a>(row: &'a str, name: &str) -> &'a str {
        row.split(',')
            .nth(at(name))
            .unwrap_or_else(|| panic!("{row}"))
    }

    /// **What the device advertised, under the BLE export's own column
    /// names**, so the two files join on them; blank where it sent nothing,
    /// and a name with a comma quoted whole.
    #[test]
    fn the_advertised_columns_carry_what_the_device_said() {
        let now = Instant::now();
        let mut m = SdrMetrics::fixture().streaming();
        let (said, quiet) = ([0xa4, 0x83, 0xe7, 0x1c, 9, 0xbe], [1, 2, 3, 4, 5, 6]);
        m.net.census.devices = vec![
            Device {
                packets: 2,
                ..Device::heard(said, false, now)
            },
            Device {
                packets: 1,
                ..Device::heard(quiet, false, now)
            },
        ];
        m.net.census.sort = column("PKTS");
        m.net.census.descending = true;
        m.net.advertised.insert(
            said,
            crate::signal::ble::ad::Advertised {
                company: Some(0x004C),
                name: Some(("Kitchen, left".to_string(), true)),
                tx_power_dbm: Some(-8),
            },
        );
        let out = rows(&m);
        assert!(
            out[0].contains(",\"Kitchen, left\",true,-8,0x004C,\"Apple, Inc.\","),
            "{}",
            out[0]
        );
        assert!(out[1].contains(",public,,,,,,1,"), "{}", out[1]);
    }

    #[test]
    fn an_empty_census_exports_a_header_and_no_rows() {
        let m = SdrMetrics::fixture().streaming();
        assert!(rows(&m).is_empty());
        // The header names every column: the shape of the answer is visible
        // even when there is no answer yet.
        assert_eq!(HEADER.split(',').count(), 30);
    }

    /// A reading's sigma travels with it, and the timing columns say what
    /// was read or why not.
    #[test]
    fn every_reading_carries_its_sigma_and_the_timing_its_status() {
        let rows = rows(&sample::room());
        let full = &rows[0];
        assert_eq!(field(full, "crystal_offset_ppm"), "-9.30");
        assert_eq!(field(full, "crystal_offset_sigma_ppm"), "0.02");
        assert_eq!(field(full, "modulation_index"), "0.5000");
        assert_eq!(field(full, "adv_status"), "estimated");
        assert_eq!(field(full, "adv_channel"), "37");
        assert_eq!(field(full, "adv_grid"), "on");
        assert_eq!(field(full, "adv_grid_steps"), "160");
        assert_eq!(field(full, "kind"), "RPA");
        assert_eq!(field(full, "pdu_types"), "ADV_IND");

        assert_eq!(field(&rows[1], "adv_status"), "not timed in LOCK");
        assert_eq!(field(&rows[1], "mean_snr_sigma_db"), "", "one packet");
        assert_eq!(field(&rows[2], "adv_status"), "collecting 2 of 8");
        for row in &rows {
            assert_eq!(HEADER.split(',').count(), row.split(',').count(), "{row}");
        }
    }

    #[test]
    fn the_rows_come_out_in_the_order_the_panel_is_showing() {
        let now = Instant::now();
        let mut m = SdrMetrics::fixture().streaming();
        m.net.census.devices = vec![
            Device {
                packets: 7,
                best_snr_db: Some(-88.0),
                last_seen: now - Duration::from_secs(240),
                ..Device::heard(
                    [0xf0, 0x18, 0x98, 0, 0x11, 0x22],
                    false,
                    now - Duration::from_secs(300),
                )
            },
            Device {
                packets: 1_204,
                best_snr_db: Some(-41.2),
                last_seen: now - Duration::from_secs(2),
                crystal_offset_ppm: Some(Uncertain::exact(15.0)),
                ..Device::heard(
                    [0xa4, 0x83, 0xe7, 0x1c, 9, 0xbe],
                    false,
                    now - Duration::from_secs(600),
                )
            },
        ];
        m.net.census.sort = column("PKTS");
        m.net.census.descending = true;

        let by_packets = rows(&m);
        assert_eq!(by_packets.len(), 2);
        assert!(
            by_packets[0].starts_with("a4:83:e7:1c:09:be,public,,,,,,1204,"),
            "{}",
            by_packets[0]
        );
        assert_eq!(field(&by_packets[0], "best_snr_db"), "-41.2");
        assert_eq!(field(&by_packets[0], "crystal_offset_ppm"), "15.00");
        assert_eq!(
            field(&by_packets[1], "crystal_offset_ppm"),
            "",
            "no CFO measured: a blank field, not a zero"
        );

        // Ordered by address instead, and the file follows.
        m.net.census.sort = column("ADDRESS");
        m.net.census.descending = false;
        let by_address = rows(&m);
        assert!(by_address[0].starts_with("a4:"), "{:?}", by_address[0]);

        // The file shows addresses the way the screen does, so an export taken
        // with the switch away from `full` leaks no more than the screen did.
        m.net.address_display = crate::state::AddressDisplay::Oui;
        let shown = rows(&m);
        assert!(shown[0].starts_with("Apple ..09:be,"), "{:?}", shown[0]);

        // Masked, the file carries the session's number and nothing more.
        m.net.address_display = crate::state::AddressDisplay::Masked;
        m.net.address_book.number([0xa4, 0x83, 0xe7, 0x1c, 9, 0xbe]);
        let masked = rows(&m);
        assert!(masked[0].starts_with("Apple #1,"), "{:?}", masked[0]);
        assert!(!masked.join("\n").contains("09:be"), "{masked:?}");
    }
}
