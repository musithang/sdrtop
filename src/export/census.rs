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
//! It is empty until an arc decodes an address, and it exports that emptiness as
//! what it is. A census file with a header and no rows says "nobody was
//! counted"; the sentence that says *why* nobody was counted belongs in the
//! provenance header, where a reader six months later will look.

use crate::state::SdrMetrics;

/// `crystal_offset_ppm` is corrected for our oscillator exactly as the panel
/// shows it, and the provenance header's `reference` line says what it is
/// worth: relative without a reference, absolute with one.
pub const HEADER: &str = "address,packets,best_snr_db,crystal_offset_ppm,first_seen_s,last_seen_s";

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
            // A blank field, not a zero: a device with no CFO measurement
            // yet has not had one refused so much as never asked, but a
            // literal `0` in a CSV column reads as "measured, and exactly
            // zero" to anything that parses this file later - the same
            // reason [`crate::export::occupancy`] never writes a bare
            // number for a cell nothing has covered.
            let cfo = d
                .crystal_offset_ppm
                .map(|u| format!("{:.2}", state.radio.corrected_ppm(u, now).0.value()))
                .unwrap_or_default();
            // Blank for the same reason: no packet from it reported an SNR.
            let snr = d
                .best_snr_db
                .map(|db| format!("{db:.1}"))
                .unwrap_or_default();
            format!(
                "{},{},{},{},{},{}",
                super::csv_field(&d.address_text(&state.net, None)),
                d.packets,
                snr,
                cfo,
                now.saturating_duration_since(d.first_seen).as_secs(),
                now.saturating_duration_since(d.last_seen).as_secs()
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::net::census::Device;
    use std::time::{Duration, Instant};

    #[test]
    fn an_empty_census_exports_a_header_and_no_rows() {
        let m = SdrMetrics::fixture().streaming();
        assert!(rows(&m).is_empty());
        // The header still names six columns: the shape of the answer is
        // visible even when there is no answer yet.
        assert_eq!(HEADER.split(',').count(), 6);
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
                crystal_offset_ppm: Some(crate::signal::dsp::uncertainty::Uncertain::exact(15.0)),
                ..Device::heard(
                    [0xa4, 0x83, 0xe7, 0x1c, 9, 0xbe],
                    false,
                    now - Duration::from_secs(600),
                )
            },
        ];
        m.net.census.sort = 2;
        m.net.census.descending = true;

        let by_packets = rows(&m);
        assert_eq!(by_packets.len(), 2);
        assert!(
            by_packets[0].starts_with("a4:83:e7:1c:09:be,1204,-41.2,15.00,"),
            "{}",
            by_packets[0]
        );
        assert!(
            by_packets[1].starts_with("f0:18:98:00:11:22,7,-88.0,,"),
            "no CFO measured: a blank field, not a zero - {}",
            by_packets[1]
        );
        assert_eq!(HEADER.split(',').count(), by_packets[0].split(',').count());

        // Ordered by address instead, and the file follows.
        m.net.census.sort = 0;
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
