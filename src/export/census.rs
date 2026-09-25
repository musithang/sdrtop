// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The population, one transmitter to a row.
//!
//! The body design section 15 names first, and the second one written, which is
//! the point: **one body proves nothing about a seam.** This and
//! [`super::occupancy`] share the provenance header and the destination handling
//! without either knowing about the other, which is the property a later
//! IQ-sample export needs to hold.
//!
//! **Every column the panel shows, and nothing it does not.** Each reading
//! the census panel prints with its uncertainty is written as two columns, the
//! value and its sigma, because a ppm figure without its uncertainty is the
//! number six months later nobody can judge (design 15.1). **Blank wherever the
//! panel dashes**, never a zero: a mean from one packet, an offset no packet
//! reported, an interval not yet read. `the_export_is_blank_wherever_the_panel_
//! dashes` holds the two to one rule on a populated census, which N20 could
//! only promise while the census was empty.
//!
//! An empty census exports its emptiness as what it is, with the reason in the
//! header: a quiet room, or nothing decoding addresses (`super::net_section`).

use crate::signal::ble::interval::{Delay, Grid, Refusal};
use crate::signal::net::census::Device;
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

/// What the device advertised about itself (`NetState::advertised`), in
/// the BLE export's own column names so the two files join on them: blank
/// where it has not sent that field.
fn advertised(state: &SdrMetrics, d: &Device) -> [String; 5] {
    let said = state.net.advertised.get(&d.address);
    let (name, complete) = said
        .and_then(|a| a.name.as_ref())
        .map(|(text, complete)| {
            (
                super::csv_field(&state.net.show_name(text)).into_owned(),
                complete.to_string(),
            )
        })
        .unwrap_or_default();
    let company = said.and_then(|a| a.company);
    [
        name,
        complete,
        said.and_then(|a| a.tx_power_dbm)
            .map(|dbm| dbm.to_string())
            .unwrap_or_default(),
        company.map(|id| format!("0x{id:04X}")).unwrap_or_default(),
        company
            .and_then(crate::signal::ble::assigned::company)
            .map(|n| super::csv_field(n).into_owned())
            .unwrap_or_default(),
    ]
}

/// A value and its sigma at `places`, or two blanks where the panel dashes:
/// no reading, or one whose uncertainty cannot be stated (a mean from one
/// packet, whose sigma is infinite).
fn with_sigma(u: Option<crate::signal::dsp::uncertainty::Uncertain>, places: usize) -> [String; 2] {
    match u {
        Some(u) if u.value().is_finite() && u.sigma().is_finite() => [
            format!("{:.places$}", u.value()),
            format!("{:.places$}", u.sigma()),
        ],
        _ => [String::new(), String::new()],
    }
}

/// Columns the advertising timing takes, `adv_status` through
/// `adv_grid_off_ms`.
const ADVERTISING_COLUMNS: usize = 10;

/// The advertising timing's columns (`signal::ble::interval`), blank where
/// there is no reading, with the reason in `adv_status`. Always exactly
/// [`ADVERTISING_COLUMNS`] of them: the first version padded a refusal with
/// one blank too many, which shifted every later column of every device
/// not timed in LOCK, and a test counting fields per row is what caught it.
fn advertising(d: &Device) -> Vec<String> {
    let blank = || vec![String::new(); ADVERTISING_COLUMNS - 1];
    let status = |text: String| {
        let mut v = vec![text];
        v.extend(blank());
        v
    };
    let e = match d.advertising() {
        None => return status("not timed in LOCK".to_string()),
        Some(Err(Refusal::Collecting { have, need })) => {
            return status(format!("collecting {have} of {need}"))
        }
        Some(Err(Refusal::BelowMinimum(_))) => {
            return status("faster than any legacy interval".to_string())
        }
        Some(Err(Refusal::NoSingleEvents(_))) => {
            return status("no two consecutive events".to_string())
        }
        Some(Ok(e)) => e,
    };
    let [interval, sigma] = with_sigma(Some(e.interval_s.scale(1e3)), 3);
    let (shape, width) = match e.delay {
        Delay::Absent { width_s } => ("none", width_s),
        Delay::Spread {
            width_s,
            uniform: true,
            ..
        } => ("uniform", width_s),
        Delay::Spread { width_s, .. } => ("not uniform", width_s),
    };
    let (grid, steps, off) = match e.grid {
        Grid::On(n) => ("on", n.to_string(), String::new()),
        Grid::Off { n, by_s } => ("off", n.to_string(), format!("{:.3}", by_s * 1e3)),
        Grid::CannotTell => ("cannot tell", String::new(), String::new()),
    };
    let out = vec![
        "estimated".to_string(),
        interval,
        sigma,
        e.events.to_string(),
        d.arrivals
            .as_ref()
            .map(|a| a.channel.to_string())
            .unwrap_or_default(),
        shape.to_string(),
        format!("{:.2}", width * 1e3),
        grid.to_string(),
        steps,
        off,
    ];
    debug_assert_eq!(out.len(), ADVERTISING_COLUMNS);
    out
}

/// The census as CSV rows, in the order the panel is showing it.
///
/// The panel's order, not an arbitrary one, so a file and the screen it was
/// taken from can be read side by side.
pub fn rows(state: &SdrMetrics) -> Vec<String> {
    let now = std::time::Instant::now();
    state
        .net
        .census
        .ordered(now, &state.radio)
        .iter()
        .map(|d| {
            let offset = d
                .crystal_offset_ppm
                .map(|u| state.radio.corrected_ppm(u, now).0);
            let pdu_types = d
                .ble_pdu_codes()
                .map(|c| crate::signal::ble::pdu::PduType::from_bits(c).label())
                .collect::<Vec<_>>()
                .join(";");
            let mut fields = vec![
                super::csv_field(&d.address_text(&state.net, None)).into_owned(),
                d.kind().label().to_string(),
            ];
            fields.extend(advertised(state, d));
            fields.extend([
                d.packets.to_string(),
                d.crc_failed.to_string(),
                format!("{:.2}", d.crc_pass_rate() * 100.0),
                d.best_snr_db
                    .map(|db| format!("{db:.1}"))
                    .unwrap_or_default(),
            ]);
            fields.extend(with_sigma(d.mean_snr_db(), 2));
            fields.extend(with_sigma(offset, 2));
            fields.extend(with_sigma(d.modulation_index, 4));
            fields.push(super::csv_field(&pdu_types).into_owned());
            fields.extend(advertising(d));
            fields.push(
                now.saturating_duration_since(d.first_seen)
                    .as_secs()
                    .to_string(),
            );
            fields.push(
                now.saturating_duration_since(d.last_seen)
                    .as_secs()
                    .to_string(),
            );
            fields.join(",")
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::dsp::uncertainty::Uncertain;
    use crate::signal::net::census::{column, observe, Arrival, Device, Sighting};
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

    /// A census as a room gives one: a device with every reading, one heard
    /// once, and one still being timed in LOCK. Sorted by address, so no
    /// title but ADDRESS carries the sort marker.
    fn room() -> SdrMetrics {
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

    /// The cell under `title`, where the column before it is `prev`: both
    /// right-aligned, so a title's last character marks its column's end.
    fn cell(header: &str, row: &str, prev: &str, title: &str) -> String {
        let h: Vec<char> = header.chars().collect();
        let end = |t: &str| {
            let t: Vec<char> = t.chars().collect();
            h.windows(t.len()).position(|w| w == t.as_slice()).unwrap() + t.len()
        };
        row.chars()
            .skip(end(prev))
            .take(end(title) - end(prev))
            .collect::<String>()
            .trim()
            .to_string()
    }

    /// **N20's rule, on a populated census at last**: every reading the
    /// panel dashes is blank in the file, and every one it prints is there,
    /// across the columns that can be absent.
    #[test]
    fn the_export_is_blank_wherever_the_panel_dashes() {
        let m = room();
        let panel = crate::state::fixture::draw(crate::ui::NetCensusPanel, 170, 12, &m);
        let header = &panel[1];
        let exported = rows(&m);
        assert_eq!(exported.len(), 3);

        let pairs = [
            ("PKTS", "BEST SNR", "best_snr_db"),
            ("CRC", "CFO", "crystal_offset_ppm"),
            ("CFO", "MEAN SNR", "mean_snr_db"),
            ("TYPES", "MOD", "modulation_index"),
            ("MOD", "INTERVAL", "adv_interval_ms"),
        ];
        let mut dashed = 0;
        let mut shown = 0;
        for (tail, row) in [("00:01", 0), ("00:02", 1), ("00:03", 2)] {
            let line = panel.iter().find(|l| l.contains(tail)).unwrap();
            for (prev, title, name) in pairs {
                let on_screen = cell(header, line, prev, title);
                let in_file = field(&exported[row], name);
                let dash = on_screen == "-" || on_screen.starts_with('\u{2014}');
                assert_eq!(
                    dash,
                    in_file.is_empty(),
                    "{title} for {tail}: screen {on_screen:?}, file {in_file:?}"
                );
                if dash {
                    dashed += 1;
                } else {
                    shown += 1;
                }
            }
        }
        // Both halves of the rule were exercised, not one of them vacuously.
        assert!(dashed >= 5 && shown >= 5, "{dashed} dashed, {shown} shown");
    }

    /// A reading's sigma travels with it, and the timing columns say what
    /// was read or why not.
    #[test]
    fn every_reading_carries_its_sigma_and_the_timing_its_status() {
        let rows = rows(&room());
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
