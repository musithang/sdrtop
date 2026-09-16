// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Who is here: the population of the band, keyed by address.
//!
//! Design section 10 puts this in `net` rather than in either arc, and the
//! reason is the one that shapes the whole module: **a device is a device
//! whichever protocol found it.** A Wi-Fi station and a Bluetooth peripheral are
//! the same kind of row - an address, when it was last heard, how much it has
//! said, how strongly - and the panel that ranks them must not care which arc
//! filled it in.
//!
//! **B10 is the first arc to fill this in**, and BLE's own honesty about what
//! it measures shaped one decision here: this struct was drafted with a
//! `best_rssi_dbm` field, for whichever arc filled it in first, but BLE has
//! no calibrated absolute power reference, only the matched filter's own
//! SNR (design section 7's own reasoning - see `signal::ble::receive`).
//! Calling that number an RSSI would be exactly the mislabelling POLICY.md's
//! rule 5 exists to prevent, one scale misnamed as another. The field is
//! `best_snr_db` instead - honest for what fills it today. A future arc with
//! a real calibrated RSSI will need to settle how the two coexist in one
//! column; that is its own question, not answered here ahead of having it.
//!
//! Design section 1.1's address display switch (`full`, `oui`, `masked`)
//! arrives with whichever arc needs it; none does yet.

use std::time::Instant;

use crate::signal::dsp::uncertainty::Uncertain;

/// One transmitter, as the census knows it.
#[derive(Clone, Debug)]
pub struct Device {
    /// The MAC or BD_ADDR, as transmitted.
    pub address: [u8; 6],
    /// Packets attributed to it.
    pub packets: u64,
    /// The strongest it has been heard. See the module doc for why this is
    /// an SNR, not an RSSI.
    pub best_snr_db: f32,
    /// When it was last heard.
    pub last_seen: Instant,
    /// This device's own crystal error, refined ([`Uncertain::combine`])
    /// across every packet that reported one - design section 2.5's
    /// "crystal-error histogram" measurement, per device. `None` until at
    /// least one packet from this device has reported a frequency offset.
    pub crystal_offset_hz: Option<Uncertain>,
}

impl Device {
    /// `a4:83:e7:1c:09:be`, the form design section 1.1 makes the default.
    pub fn address_text(&self) -> String {
        self.address
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join(":")
    }
}

/// What the table can be ordered by, in the order the columns are drawn.
///
/// The names are the column titles, so the chrome tag and the header cannot
/// disagree about what the table is sorted by.
pub const SORT_KEYS: &[&str] = &["ADDRESS", "SEEN", "PKTS", "SNR", "CFO"];

/// Put `devices` in the order the panel asked for.
///
/// **Total, and deterministic where the key ties.** Two devices heard the same
/// number of times must not swap places between frames, so the address breaks
/// every tie: it is the one field that is unique by definition.
///
/// **Sorting by CFO ranks by how bad the clock is, not by its sign** - design
/// section 2.5's own "sorted by how bad its clock is" - so the key is the
/// *magnitude* of the offset. A device with no CFO measurement yet sorts
/// last regardless of direction: rule 2 refuses to rank an absent reading
/// as if it were a good one.
pub fn order(devices: &mut [Device], sort: usize, descending: bool, now: Instant) {
    devices.sort_by(|a, b| {
        // CFO's "absent sorts last" is not reversed by `descending` - only
        // the ordering *between two measured* devices is - so it is kept
        // out of the generic reversal below rather than folded into it.
        let key = if sort == 4 {
            cfo_key(a, b, descending)
        } else {
            let key = match sort {
                1 => now
                    .saturating_duration_since(a.last_seen)
                    .cmp(&now.saturating_duration_since(b.last_seen)),
                2 => a.packets.cmp(&b.packets),
                3 => a.best_snr_db.total_cmp(&b.best_snr_db),
                _ => std::cmp::Ordering::Equal,
            };
            if descending {
                key.reverse()
            } else {
                key
            }
        };
        key.then_with(|| a.address.cmp(&b.address))
    });
}

/// The CFO sort key: by magnitude between two measured devices, reversed
/// when `descending` asks for worst-first, but a device with no measurement
/// yet sorts last either way - see [`order`]'s own doc for why.
fn cfo_key(a: &Device, b: &Device, descending: bool) -> std::cmp::Ordering {
    let mag = |d: &Device| d.crystal_offset_hz.map(|u| u.value().abs());
    match (mag(a), mag(b)) {
        (Some(x), Some(y)) => {
            let cmp = x.total_cmp(&y);
            if descending {
                cmp.reverse()
            } else {
                cmp
            }
        }
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}

/// Record one packet from `address`: a new row if this is the first time
/// it has been heard, otherwise an update to the existing one.
///
/// **`snr_db` replaces the stored reading only when it is stronger** - the
/// "best it has been heard" the field's own name promises, not the most
/// recent. **`crystal_offset_hz` is refined, not replaced** -
/// [`Uncertain::combine`] folds a new packet's own CFO reading into
/// whatever this device's estimate already was, the same way more samples
/// tighten any other measurement in this app, rather than keeping only the
/// latest packet's own noisy single reading.
pub fn observe(
    devices: &mut Vec<Device>,
    address: [u8; 6],
    snr_db: Option<f64>,
    crystal_offset_hz: Option<Uncertain>,
    now: Instant,
) {
    let device = match devices.iter_mut().find(|d| d.address == address) {
        Some(d) => d,
        None => {
            devices.push(Device {
                address,
                packets: 0,
                best_snr_db: f32::NEG_INFINITY,
                last_seen: now,
                crystal_offset_hz: None,
            });
            devices.last_mut().expect("just pushed")
        }
    };
    device.packets += 1;
    device.last_seen = now;
    if let Some(snr) = snr_db {
        device.best_snr_db = device.best_snr_db.max(snr as f32);
    }
    if let Some(offset) = crystal_offset_hz {
        device.crystal_offset_hz = Some(match device.crystal_offset_hz {
            Some(existing) => existing.combine(&offset),
            None => offset,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn device(last: u8, packets: u64, snr: f32, ago_s: u64, now: Instant) -> Device {
        Device {
            address: [0xa4, 0x83, 0xe7, 0x1c, 0x09, last],
            packets,
            best_snr_db: snr,
            last_seen: now - Duration::from_secs(ago_s),
            crystal_offset_hz: None,
        }
    }

    #[test]
    fn an_address_reads_as_an_address() {
        let now = Instant::now();
        assert_eq!(
            device(0xbe, 0, 0.0, 0, now).address_text(),
            "a4:83:e7:1c:09:be"
        );
        assert_eq!(
            Device {
                address: [0, 0, 0, 0, 0, 0],
                ..device(0, 0, 0.0, 0, now)
            }
            .address_text(),
            "00:00:00:00:00:00"
        );
    }

    #[test]
    fn every_column_orders_by_the_thing_it_names() {
        let now = Instant::now();
        // (address tail, packets, snr, seconds ago)
        let make = || {
            vec![
                device(0x03, 10, 2.0, 30, now),
                device(0x01, 500, 18.0, 2, now),
                device(0x02, 7, 9.0, 90, now),
            ]
        };
        let tails = |d: &[Device]| d.iter().map(|x| x.address[5]).collect::<Vec<_>>();

        let mut d = make();
        order(&mut d, 0, false, now);
        assert_eq!(tails(&d), vec![1, 2, 3], "by address, ascending");

        let mut d = make();
        order(&mut d, 1, false, now);
        assert_eq!(tails(&d), vec![1, 3, 2], "most recently seen first");

        let mut d = make();
        order(&mut d, 2, true, now);
        assert_eq!(tails(&d), vec![1, 3, 2], "busiest first");

        let mut d = make();
        order(&mut d, 3, true, now);
        assert_eq!(tails(&d), vec![1, 2, 3], "strongest first");
    }

    /// **A tie must not shuffle.** Two devices with the same count would
    /// otherwise swap places between frames, and a table whose rows move under
    /// the cursor for no reason is one nobody can use.
    #[test]
    fn a_tie_is_broken_by_the_one_field_that_cannot_tie() {
        let now = Instant::now();
        let mut a = vec![
            device(0x09, 42, -50.0, 5, now),
            device(0x02, 42, -50.0, 5, now),
            device(0x07, 42, -50.0, 5, now),
        ];
        let mut b = a.clone();
        b.reverse();
        order(&mut a, 2, true, now);
        order(&mut b, 2, true, now);
        let tails = |d: &[Device]| d.iter().map(|x| x.address[5]).collect::<Vec<_>>();
        assert_eq!(
            tails(&a),
            tails(&b),
            "the same rows in a different order in"
        );
        assert_eq!(tails(&a), vec![2, 7, 9]);
    }

    /// CFO ranks by magnitude, and a device with none measured yet sorts
    /// last regardless of direction, ascending or descending - rule 2's
    /// refusal to rank an absent reading as a good one.
    #[test]
    fn cfo_orders_by_magnitude_and_puts_the_unmeasured_last() {
        let now = Instant::now();
        let with = |tail: u8, hz: f64| Device {
            crystal_offset_hz: Some(Uncertain::exact(hz)),
            ..device(tail, 0, 0.0, 0, now)
        };
        let mut d = vec![
            with(0x01, -300.0),           // worst clock, negative
            device(0x02, 0, 0.0, 0, now), // unmeasured
            with(0x03, 50.0),             // best clock
        ];
        let tails = |d: &[Device]| d.iter().map(|x| x.address[5]).collect::<Vec<_>>();

        order(&mut d, 4, true, now); // worst first
        assert_eq!(tails(&d), vec![1, 3, 2]);

        order(&mut d, 4, false, now); // best first
        assert_eq!(tails(&d), vec![3, 1, 2]);
    }

    /// The keys and the columns are one list, so the chrome tag and the header
    /// cannot name different things.
    #[test]
    fn the_sort_keys_are_the_column_titles() {
        assert_eq!(SORT_KEYS.len(), 5);
        assert!(SORT_KEYS.contains(&"PKTS"));
        assert!(SORT_KEYS.contains(&"CFO"));
    }

    #[test]
    fn a_first_sighting_creates_a_row() {
        let now = Instant::now();
        let mut devices = Vec::new();
        observe(
            &mut devices,
            [1, 2, 3, 4, 5, 6],
            Some(4.0),
            Some(Uncertain::from_sigma(120.0, 20.0)),
            now,
        );
        assert_eq!(devices.len(), 1);
        let d = &devices[0];
        assert_eq!(d.packets, 1);
        assert_eq!(d.best_snr_db, 4.0);
        assert_eq!(d.crystal_offset_hz.unwrap().value(), 120.0);
    }

    /// A second sighting of the same address updates the one row rather than
    /// adding a second - a census counts transmitters, not packets.
    #[test]
    fn a_repeat_sighting_updates_the_same_row() {
        let now = Instant::now();
        let mut devices = Vec::new();
        let addr = [1, 2, 3, 4, 5, 6];
        observe(&mut devices, addr, Some(4.0), None, now);
        observe(&mut devices, addr, Some(9.0), None, now);
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].packets, 2);
        // Best-ever, not most-recent: 4.0 dB does not overwrite 9.0 dB.
        assert_eq!(devices[0].best_snr_db, 9.0);

        let mut devices2 = Vec::new();
        observe(&mut devices2, addr, Some(9.0), None, now);
        observe(&mut devices2, addr, Some(4.0), None, now);
        assert_eq!(devices2[0].best_snr_db, 9.0, "order must not matter");
    }

    /// A device's own CFO estimate tightens as more packets report one,
    /// rather than jumping to whatever the latest single packet happened to
    /// read.
    #[test]
    fn repeated_cfo_readings_refine_rather_than_replace() {
        let now = Instant::now();
        let mut devices = Vec::new();
        let addr = [1, 2, 3, 4, 5, 6];
        let a = Uncertain::from_sigma(100.0, 20.0);
        let b = Uncertain::from_sigma(140.0, 20.0);
        observe(&mut devices, addr, None, Some(a), now);
        observe(&mut devices, addr, None, Some(b), now);
        let combined = devices[0].crystal_offset_hz.unwrap();
        let direct = a.combine(&b);
        assert_eq!(combined.value(), direct.value());
        assert_eq!(combined.sigma(), direct.sigma());
        // Two equal-variance readings: tighter than either alone.
        assert!(combined.sigma() < a.sigma());
    }
}
