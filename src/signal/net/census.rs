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
    /// The MAC or BD_ADDR, in the written octet order (most significant
    /// first; see `signal::ble::pdu::air_octets`).
    pub address: [u8; 6],
    /// Whether the address was sent as a random one (BLE's TxAdd = 1), which
    /// decides what kind of address it is (`signal::ble::address::kind`).
    pub random: bool,
    /// Packets attributed to it.
    pub packets: u64,
    /// The strongest it has been heard. See the module doc for why this is
    /// an SNR, not an RSSI.
    pub best_snr_db: f32,
    /// When this address was first heard this session. B13's own reason to
    /// exist: address-rotation observation needs to know when a row's
    /// address *appeared*, not only that it exists, to say how many new
    /// ones are showing up per unit time.
    pub first_seen: Instant,
    /// When it was last heard.
    pub last_seen: Instant,
    /// This device's own crystal error in ppm, refined
    /// ([`Uncertain::combine`]) across every packet that reported one -
    /// design section 2.5's "crystal-error histogram" measurement, per
    /// device. `None` until at least one packet from this device has
    /// reported a frequency offset.
    ///
    /// **As the air delivered it: their error minus ours**, never corrected
    /// here. A reference can be captured, or expire, while the row lives;
    /// storing the raw figure and correcting on the way to the screen
    /// (`RadioState::corrected_ppm`) keeps every packet already folded in
    /// right either way. **ppm, not Hz**, because a device advertises on
    /// three channels 78 MHz apart and the same crystal reads 3 % more Hz
    /// on the highest than the lowest; Hz from different channels cannot be
    /// combined, ppm can.
    pub crystal_offset_ppm: Option<Uncertain>,
}

impl Device {
    /// The address as the section's display mode shows it, with this
    /// device's own kind and masked number, in a column `width` wide (`None`:
    /// uncut, for an export). See `state::AddressDisplay::show`.
    pub fn address_text(&self, net: &crate::state::NetState, width: Option<usize>) -> String {
        net.show_address(self.address, self.random, width)
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
/// as if it were a good one. The magnitude is of the offset as the panel
/// shows it, corrected through `radio` when a reference allows: our own
/// error shifts every device the same way, so ranking the raw figures
/// would put a clock that is dead on below one that happens to cancel ours.
pub fn order(
    devices: &mut [Device],
    sort: usize,
    descending: bool,
    now: Instant,
    radio: &crate::state::RadioState,
) {
    devices.sort_by(|a, b| {
        // CFO's "absent sorts last" is not reversed by `descending` - only
        // the ordering *between two measured* devices is - so it is kept
        // out of the generic reversal below rather than folded into it.
        let key = if sort == 4 {
            cfo_key(a, b, descending, |u| radio.corrected_ppm(u, now).0.value())
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
fn cfo_key(
    a: &Device,
    b: &Device,
    descending: bool,
    corrected: impl Fn(Uncertain) -> f64,
) -> std::cmp::Ordering {
    let mag = |d: &Device| d.crystal_offset_ppm.map(|u| corrected(u).abs());
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
/// recent. **`crystal_offset_ppm` is refined, not replaced** -
/// [`Uncertain::combine`] folds a new packet's own CFO reading into
/// whatever this device's estimate already was, the same way more samples
/// tighten any other measurement in this app, rather than keeping only the
/// latest packet's own noisy single reading.
pub fn observe(
    devices: &mut Vec<Device>,
    address: [u8; 6],
    random: bool,
    snr_db: Option<f64>,
    crystal_offset_ppm: Option<Uncertain>,
    now: Instant,
) {
    let device = match devices.iter_mut().find(|d| d.address == address) {
        Some(d) => d,
        None => {
            devices.push(Device {
                address,
                random,
                packets: 0,
                best_snr_db: f32::NEG_INFINITY,
                first_seen: now,
                last_seen: now,
                crystal_offset_ppm: None,
            });
            devices.last_mut().expect("just pushed")
        }
    };
    device.packets += 1;
    device.last_seen = now;
    if let Some(snr) = snr_db {
        device.best_snr_db = device.best_snr_db.max(snr as f32);
    }
    if let Some(offset) = crystal_offset_ppm {
        device.crystal_offset_ppm = Some(match device.crystal_offset_ppm {
            Some(existing) => existing.combine(&offset),
            None => offset,
        });
    }
}

/// How many *new* addresses have appeared in the last `window` - design
/// section 2.5's measurement 19, address rotation observation.
///
/// **A measurement about the protocol, not about a person.** BLE privacy
/// rotates a device's own advertising address every so often; this counts
/// how many distinct addresses are showing up, and how fast, without ever
/// claiming two of them are the same device wearing a new one - that would
/// need the resolving key a passive receiver does not have, and rule 1
/// refuses to reason past what was actually measured. "Rotations per
/// device" is not answerable from this vantage point; "how many distinct
/// addresses appear per unit time" - the design document's own, more
/// careful phrasing of the same measurement - is, and is what this computes.
///
/// A rate, not a running total: `new in the last window / window`, per
/// minute. A device rotating on the specification's own cadence (roughly
/// every 15 minutes for a resolvable private address) shows up as a small,
/// intermittent bump in this number rather than a step in an ever-climbing
/// total that never says whether the room emptied or just went quiet.
pub fn turnover_per_minute(devices: &[Device], window: std::time::Duration, now: Instant) -> f64 {
    if window.is_zero() {
        return 0.0;
    }
    let new_count = devices
        .iter()
        .filter(|d| now.saturating_duration_since(d.first_seen) <= window)
        .count();
    new_count as f64 / (window.as_secs_f64() / 60.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn radio() -> crate::state::RadioState {
        crate::state::SdrMetrics::fixture().radio
    }

    fn device(last: u8, packets: u64, snr: f32, ago_s: u64, now: Instant) -> Device {
        Device {
            address: [0xa4, 0x83, 0xe7, 0x1c, 0x09, last],
            random: false,
            packets,
            best_snr_db: snr,
            first_seen: now - Duration::from_secs(ago_s),
            last_seen: now - Duration::from_secs(ago_s),
            crystal_offset_ppm: None,
        }
    }

    #[test]
    fn an_address_reads_as_an_address() {
        let now = Instant::now();
        assert_eq!(
            device(0xbe, 0, 0.0, 0, now).address_text(&crate::state::NetState::default(), Some(17)),
            "a4:83:e7:1c:09:be"
        );
        assert_eq!(
            Device {
                address: [0, 0, 0, 0, 0, 0],
                ..device(0, 0, 0.0, 0, now)
            }
            .address_text(&crate::state::NetState::default(), Some(17)),
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
        order(&mut d, 0, false, now, &radio());
        assert_eq!(tails(&d), vec![1, 2, 3], "by address, ascending");

        let mut d = make();
        order(&mut d, 1, false, now, &radio());
        assert_eq!(tails(&d), vec![1, 3, 2], "most recently seen first");

        let mut d = make();
        order(&mut d, 2, true, now, &radio());
        assert_eq!(tails(&d), vec![1, 3, 2], "busiest first");

        let mut d = make();
        order(&mut d, 3, true, now, &radio());
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
        order(&mut a, 2, true, now, &radio());
        order(&mut b, 2, true, now, &radio());
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
        let with = |tail: u8, ppm: f64| Device {
            crystal_offset_ppm: Some(Uncertain::exact(ppm)),
            ..device(tail, 0, 0.0, 0, now)
        };
        let mut d = vec![
            with(0x01, -30.0),            // worst clock, negative
            device(0x02, 0, 0.0, 0, now), // unmeasured
            with(0x03, 5.0),              // best clock
        ];
        let tails = |d: &[Device]| d.iter().map(|x| x.address[5]).collect::<Vec<_>>();

        order(&mut d, 4, true, now, &radio()); // worst first
        assert_eq!(tails(&d), vec![1, 3, 2]);

        order(&mut d, 4, false, now, &radio()); // best first
        assert_eq!(tails(&d), vec![3, 1, 2]);
    }

    /// **With a reference, the ranking is of the clocks, not of the
    /// readings.** Our oscillator 10 ppm fast shifts every reading down by
    /// ten: a device reading -10 is dead on, one reading +8 is 18 out.
    /// Ranked raw, the perfect clock would come out worse.
    #[test]
    fn cfo_ranks_the_corrected_clocks_when_a_reference_allows() {
        let now = Instant::now();
        let with = |tail: u8, ppm: f64| Device {
            crystal_offset_ppm: Some(Uncertain::exact(ppm)),
            ..device(tail, 0, 0.0, 0, now)
        };
        let mut d = vec![with(0x01, -10.0), with(0x02, 8.0)];
        let tails = |d: &[Device]| d.iter().map(|x| x.address[5]).collect::<Vec<_>>();

        order(&mut d, 4, true, now, &radio()); // worst first, raw
        assert_eq!(tails(&d), vec![1, 2]);

        let mut referenced = radio();
        referenced.reference = Some(crate::state::FrequencyReference {
            ppm: 10.0,
            sigma_ppm: 0.1,
            provenance: crate::state::Provenance::Traceable,
            source: "WWV 10 MHz".to_string(),
            at: now,
            efficiency: None,
        });
        order(&mut d, 4, true, now, &referenced);
        assert_eq!(tails(&d), vec![2, 1], "the dead-on clock is the best one");
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
            false,
            Some(4.0),
            Some(Uncertain::from_sigma(120.0, 20.0)),
            now,
        );
        assert_eq!(devices.len(), 1);
        let d = &devices[0];
        assert_eq!(d.packets, 1);
        assert_eq!(d.best_snr_db, 4.0);
        assert_eq!(d.crystal_offset_ppm.unwrap().value(), 120.0);
        assert_eq!(d.first_seen, now);
    }

    /// `first_seen` is set once, at the row's own birth, and does not move
    /// on later sightings - B13's own turnover measurement needs to know
    /// when an address *appeared*, which a `first_seen` that kept sliding
    /// forward with every packet could never answer.
    #[test]
    fn first_seen_does_not_move_on_a_repeat_sighting() {
        let born = Instant::now();
        let later = born + Duration::from_secs(90);
        let mut devices = Vec::new();
        observe(&mut devices, [1, 2, 3, 4, 5, 6], false, None, None, born);
        observe(&mut devices, [1, 2, 3, 4, 5, 6], false, None, None, later);
        assert_eq!(devices[0].first_seen, born);
        assert_eq!(devices[0].last_seen, later);
    }

    /// A second sighting of the same address updates the one row rather than
    /// adding a second - a census counts transmitters, not packets.
    #[test]
    fn a_repeat_sighting_updates_the_same_row() {
        let now = Instant::now();
        let mut devices = Vec::new();
        let addr = [1, 2, 3, 4, 5, 6];
        observe(&mut devices, addr, false, Some(4.0), None, now);
        observe(&mut devices, addr, false, Some(9.0), None, now);
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].packets, 2);
        // Best-ever, not most-recent: 4.0 dB does not overwrite 9.0 dB.
        assert_eq!(devices[0].best_snr_db, 9.0);

        let mut devices2 = Vec::new();
        observe(&mut devices2, addr, false, Some(9.0), None, now);
        observe(&mut devices2, addr, false, Some(4.0), None, now);
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
        observe(&mut devices, addr, false, None, Some(a), now);
        observe(&mut devices, addr, false, None, Some(b), now);
        let combined = devices[0].crystal_offset_ppm.unwrap();
        let direct = a.combine(&b);
        assert_eq!(combined.value(), direct.value());
        assert_eq!(combined.sigma(), direct.sigma());
        // Two equal-variance readings: tighter than either alone.
        assert!(combined.sigma() < a.sigma());
    }

    /// B13's own exit condition: a rate, not a running total. Three
    /// addresses appeared inside the window, one appeared before it, so the
    /// count is three, turned into a per-minute rate by the window's own
    /// length.
    #[test]
    fn turnover_counts_only_what_appeared_inside_the_window() {
        let now = Instant::now();
        let window = Duration::from_secs(120);
        let devices = vec![
            device(0x01, 1, 0.0, 10, now),  // 10 s ago: inside
            device(0x02, 1, 0.0, 60, now),  // 60 s ago: inside
            device(0x03, 1, 0.0, 119, now), // 119 s ago: inside
            device(0x04, 1, 0.0, 200, now), // 200 s ago: outside
        ];
        // Three new addresses in a two-minute window is 1.5 a minute.
        assert!((turnover_per_minute(&devices, window, now) - 1.5).abs() < 1e-9);
    }

    /// No devices, or a window of zero, is a rate of zero - not a division
    /// by zero and not an invented figure.
    #[test]
    fn turnover_is_zero_with_nothing_to_count() {
        let now = Instant::now();
        assert_eq!(turnover_per_minute(&[], Duration::from_secs(60), now), 0.0);
        let devices = vec![device(0x01, 1, 0.0, 5, now)];
        assert_eq!(turnover_per_minute(&devices, Duration::ZERO, now), 0.0);
    }
}
