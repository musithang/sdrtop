// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The session's frame error rate against SNR (net-ux-polish-plan 5.7), the
//! "session's frame-error curve" foundation design 15 names among the first
//! things anyone would export: one SNR bin a row, all BLE traffic first, then
//! each census device's own curve.
//!
//! **Counts always, a rate only where the panel draws one.** A bin with
//! packets is a row; its failure rate and the rate's uncertainty are blank
//! below `fer::MIN_PACKETS`, the dash the detail view shows there. The counts
//! stay, so a reader can pool thin bins with their own rule. A device's
//! failures are the exact-address ones (`census::observe_crc_failure`), a
//! ceiling on its link's error rate, which the note line says.

use crate::signal::ble::fer::{edges, FerCurve};
use crate::state::SdrMetrics;

pub const HEADER: &str = "scope,snr_from_db,snr_to_db,packets,crc_failed,fer_pct,fer_pct_sigma";

/// What the file always needs said beside it.
pub const NOTE: &str = "CRC failures among packets whose length matched; a \
device's failures are those whose address survived, so its rate is a ceiling";

/// One curve's bins with packets, as rows under `scope`.
fn curve_rows(scope: &str, curve: &FerCurve) -> Vec<String> {
    let Some(span) = curve.span() else {
        return Vec::new();
    };
    span.filter(|&b| curve.packets(b) > 0)
        .map(|b| {
            let (from, to) = edges(b);
            let (rate, sigma) = curve
                .rate(b)
                .map(|r| {
                    (
                        format!("{:.2}", r.value() * 100.0),
                        format!("{:.2}", r.sigma() * 100.0),
                    )
                })
                .unwrap_or_default();
            [
                super::csv_field(scope).into_owned(),
                from.map(|v| format!("{v:.0}")).unwrap_or_default(),
                to.map(|v| format!("{v:.0}")).unwrap_or_default(),
                curve.packets(b).to_string(),
                curve.failed[b].to_string(),
                rate,
                sigma,
            ]
            .join(",")
        })
        .collect()
}

/// The section's curve, then each device's, in the census's order.
pub fn rows(state: &SdrMetrics) -> Vec<String> {
    let now = std::time::Instant::now();
    let mut out = curve_rows("all", &state.net.fer);
    for d in state.net.census.ordered(now, &state.radio) {
        out.extend(curve_rows(&d.address_text(&state.net, None), &d.fer));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// All traffic first, then a device; a thin bin keeps its counts and
    /// leaves its rate blank, and every row is the header's width.
    #[test]
    fn the_curve_exports_its_counts_and_its_rates_where_they_stand() {
        let mut m = SdrMetrics::fixture().streaming();
        for i in 0..30 {
            m.net.fer.record(5.0, i >= 3);
        }
        for _ in 0..4 {
            m.net.fer.record(20.0, true);
        }
        let mut d = crate::signal::net::census::Device::heard(
            [0xaa, 0xbb, 0xcc, 0x11, 0x22, 0x33],
            false,
            std::time::Instant::now(),
        );
        for _ in 0..12 {
            d.fer.record(30.0, true);
        }
        m.net.census.devices.push(d);

        let rows = rows(&m);
        assert_eq!(
            rows,
            [
                "all,4,6,30,3,10.00,5.68",
                "all,20,22,4,0,,",
                "aa:bb:cc:11:22:33,30,32,12,0,0.00,5.33",
            ]
        );
        for row in &rows {
            assert_eq!(row.split(',').count(), HEADER.split(',').count(), "{row}");
        }
    }

    #[test]
    fn no_packets_is_no_rows() {
        assert!(rows(&SdrMetrics::fixture().streaming()).is_empty());
    }
}
