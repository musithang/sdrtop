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
//! **The same reasoning names two of the fields net-ux-polish-plan 4.2.b
//! added.** Bluetooth design measurement 17 asks for "PDU types used" and
//! "measured modulation index"; both are BLE's own quantities, so the fields
//! say so (`ble_pdu_types`, `modulation_index` from B8's LE 1M measurement)
//! rather than taking general names a second arc's different type space would
//! then have to squeeze into. The census stays protocol-neutral in the sense
//! that matters: it keeps codes and numbers, and naming them is the panel's
//! job. The one thing it asks of `ble` is the address kind (4.4), which is not
//! a new fact about a device but a reading of two it already holds: the
//! address and the TxAdd bit it was sent with (`random`).
//!
//! **Sums in the record, statistics on the screen.** The mean SNR is kept as
//! a count and two running sums, and turned into a mean and its uncertainty
//! only when something reads it ([`Device::mean_snr_db`]). A record is
//! updated inside the state lock, once per packet; a square root there is
//! paid on every packet for a figure the panel reads thirty times a second at
//! most, and the `tasks/rx` discipline keeps that kind of work outside the
//! lock for a reason that holds here too.

use std::time::Instant;

use crate::signal::ble::address::AddressKind;
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
    /// Packets attributed to it: every one of them passed its CRC, so this is
    /// also the pass count [`Self::crc_pass_rate`] divides.
    pub packets: u64,
    /// The strongest it has been heard. See the module doc for why this is
    /// an SNR, not an RSSI. `None` until a packet has reported one: an absent
    /// reading, never a sentinel that prints as `-inf dB`.
    pub best_snr_db: Option<f32>,
    /// Packets that reported an SNR, and the sum and sum of squares of those
    /// readings in dB: what [`Self::mean_snr_db`] is computed from. See the
    /// module doc for why sums rather than the mean.
    pub snr_count: u64,
    pub snr_sum: f64,
    pub snr_sum_sq: f64,
    /// Which BLE PDU types this address has been heard sending, one bit per
    /// four-bit type code (`signal::ble::pdu::PduType::code`). Sixteen codes,
    /// sixteen bits: the whole of the header's type space, the extended
    /// advertising codes included, so nothing heard is dropped for not being
    /// on a list.
    pub ble_pdu_types: u16,
    /// This device's modulation index, refined ([`Uncertain::combine`]) across
    /// every packet B8 could measure one from. `None` until one could: B8
    /// needs settled runs a short packet does not always contain.
    pub modulation_index: Option<Uncertain>,
    /// Packets whose CRC failed and whose address field read exactly this
    /// address. See [`observe_crc_failure`] for what that can claim and what
    /// it cannot.
    pub crc_failed: u64,
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
    /// A row for an address heard for the first time at `now`, with nothing
    /// counted yet: what [`observe`] starts from, and what a test builds a
    /// device on with `..Device::heard(..)`.
    pub fn heard(address: [u8; 6], random: bool, now: Instant) -> Self {
        Self {
            address,
            random,
            packets: 0,
            best_snr_db: None,
            snr_count: 0,
            snr_sum: 0.0,
            snr_sum_sq: 0.0,
            ble_pdu_types: 0,
            modulation_index: None,
            crc_failed: 0,
            first_seen: now,
            last_seen: now,
            crystal_offset_ppm: None,
        }
    }

    /// The address as the section's display mode shows it, with this
    /// device's own kind and masked number, in a column `width` wide (`None`:
    /// uncut, for an export). See `state::AddressDisplay::show`.
    pub fn address_text(&self, net: &crate::state::NetState, width: Option<usize>) -> String {
        net.show_address(self.address, self.random, width)
    }

    /// The mean SNR over every packet that reported one, with the standard
    /// uncertainty of that mean. `None` before any did.
    ///
    /// **The same estimator as `dsp::uncertainty::mean_with_uncertainty`,
    /// from sums instead of a slice**, because a census cannot keep every
    /// packet's reading for a session; a test holds the two to the same
    /// answer. And the same answer for one reading: a mean, with an infinite
    /// uncertainty, because one reading has no spread to estimate one from,
    /// and a zero there would read as a perfect measurement.
    ///
    /// The mean of dB values, not the dB of a mean power: the figure
    /// measurement 17 asks for sits beside the best SNR in the same unit, and
    /// averaging in dB is what makes a device that fades in and out read as
    /// fading rather than as its loudest moments.
    pub fn mean_snr_db(&self) -> Option<Uncertain> {
        if self.snr_count == 0 {
            return None;
        }
        let n = self.snr_count as f64;
        let mean = self.snr_sum / n;
        if self.snr_count < 2 {
            return Some(Uncertain::from_variance(mean, f64::INFINITY));
        }
        // Clamped at zero: identical readings cancel the two sums to a hair
        // below it in floating point, and a negative variance is not a figure.
        let sample_variance = ((self.snr_sum_sq - self.snr_sum * mean) / (n - 1.0)).max(0.0);
        Some(Uncertain::from_variance(mean, sample_variance / n))
    }

    /// The fraction of this address's packets whose CRC passed, `0.0` to
    /// `1.0`, over the packets it could be credited with either way.
    ///
    /// **An upper bound, and the panel says so.** A packet whose CRC failed
    /// is credited here only when its address field survived intact
    /// ([`observe_crc_failure`]); one whose address took the bit errors is
    /// no device's failure, so every device's failures are a floor and its
    /// pass rate a ceiling.
    pub fn crc_pass_rate(&self) -> f64 {
        let total = self.packets + self.crc_failed;
        if total == 0 {
            return 0.0;
        }
        self.packets as f64 / total as f64
    }

    /// What kind of address this is (`signal::ble::address::kind`): public,
    /// static, or one of the private kinds, read from the address and the
    /// TxAdd bit it came with.
    pub fn kind(&self) -> AddressKind {
        crate::signal::ble::address::kind(self.address, self.random)
    }

    /// How many distinct PDU types it has been heard sending.
    pub fn ble_pdu_type_count(&self) -> u32 {
        self.ble_pdu_types.count_ones()
    }

    /// The codes of the PDU types it has sent, lowest first.
    pub fn ble_pdu_codes(&self) -> impl Iterator<Item = u8> + '_ {
        (0..16u8).filter(|c| self.ble_pdu_types & (1 << c) != 0)
    }
}

/// What the table can be ordered by, in the order the columns are drawn.
///
/// The names are the column titles, so the chrome tag and the header cannot
/// disagree about what the table is sorted by. **The order is also the order
/// the columns give way in** on a narrow terminal (`ui::widgets::table::
/// columns_that_fit` drops from the right), so the measurements a reader is
/// most likely to need come first and the ones that need the most room last.
pub const SORT_KEYS: &[&str] = &[
    "ADDRESS", "KIND", "SEEN", "PKTS", "SNR", "CRC", "CFO", "MEAN SNR", "TYPES", "MOD",
];

/// The index of the column titled `title` in [`SORT_KEYS`], for the tests
/// that set a sort: by name, so a column added in the middle does not
/// silently turn every one of them into a test of the column beside it.
#[cfg(test)]
pub fn column(title: &str) -> usize {
    SORT_KEYS
        .iter()
        .position(|k| *k == title)
        .unwrap_or_else(|| panic!("no column {title}"))
}

/// One entry of [`SORT_KEYS`], by what it orders on rather than where it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Key {
    Address,
    Kind,
    Seen,
    Packets,
    BestSnr,
    Crc,
    Cfo,
    MeanSnr,
    Types,
    Modulation,
}

impl Key {
    /// Every key, in [`SORT_KEYS`] order: a test holds the two lists to one
    /// length, and one title each.
    const ALL: [Key; 10] = [
        Key::Address,
        Key::Kind,
        Key::Seen,
        Key::Packets,
        Key::BestSnr,
        Key::Crc,
        Key::Cfo,
        Key::MeanSnr,
        Key::Types,
        Key::Modulation,
    ];
}

/// Put `devices` in the order the panel asked for.
///
/// **Total, and deterministic where the key ties.** Two devices heard the same
/// number of times must not swap places between frames, so the address breaks
/// every tie: it is the one field that is unique by definition.
///
/// **A device with no reading sorts last, in either direction.** The best
/// SNR, the mean SNR, the modulation index and the CFO are all absent until
/// a packet supplies one, and rule 2 refuses to rank an absent reading as if
/// it were a good one or a bad one; only the order *between* two measured
/// devices is reversed.
///
/// **Sorting by CFO ranks by how bad the clock is, not by its sign** - design
/// section 2.5's own "sorted by how bad its clock is" - so the key is the
/// *magnitude* of the offset, as the panel shows it, corrected through
/// `radio` when a reference allows: our own error shifts every device the same
/// way, so ranking the raw figures would put a clock that is dead on below one
/// that happens to cancel ours.
pub fn order(
    devices: &mut [Device],
    sort: usize,
    descending: bool,
    now: Instant,
    radio: &crate::state::RadioState,
) {
    let key = Key::ALL.get(sort).copied().unwrap_or(Key::Address);
    let measured = |d: &Device| -> Option<f64> {
        match key {
            Key::BestSnr => d.best_snr_db.map(f64::from),
            Key::MeanSnr => d.mean_snr_db().map(|u| u.value()),
            Key::Modulation => d.modulation_index.map(|u| u.value()),
            Key::Cfo => d
                .crystal_offset_ppm
                .map(|u| radio.corrected_ppm(u, now).0.value().abs()),
            _ => None,
        }
    };
    devices.sort_by(|a, b| {
        let ordering = match key {
            Key::BestSnr | Key::MeanSnr | Key::Modulation | Key::Cfo => {
                absent_last(measured(a), measured(b), descending)
            }
            _ => {
                let plain = match key {
                    Key::Seen => now
                        .saturating_duration_since(a.last_seen)
                        .cmp(&now.saturating_duration_since(b.last_seen)),
                    Key::Kind => kind_rank(a.kind()).cmp(&kind_rank(b.kind())),
                    Key::Packets => a.packets.cmp(&b.packets),
                    Key::Crc => a.crc_pass_rate().total_cmp(&b.crc_pass_rate()),
                    Key::Types => a.ble_pdu_type_count().cmp(&b.ble_pdu_type_count()),
                    _ => std::cmp::Ordering::Equal,
                };
                if descending {
                    plain.reverse()
                } else {
                    plain
                }
            }
        };
        ordering.then_with(|| a.address.cmp(&b.address))
    });
}

/// The order KIND sorts in: from the address that says most about who sent
/// it to the one that says least - public, static, resolvable, non-resolvable
/// - with the reserved kind, which no compliant device sends, last.
fn kind_rank(k: AddressKind) -> u8 {
    match k {
        AddressKind::Public => 0,
        AddressKind::Static => 1,
        AddressKind::ResolvablePrivate => 2,
        AddressKind::NonResolvablePrivate => 3,
        AddressKind::Reserved => 4,
    }
}

/// Two optional readings compared: by value when both exist, reversed when
/// `descending` asks, and a missing one after a present one either way. See
/// [`order`]'s own doc for why.
fn absent_last(a: Option<f64>, b: Option<f64>, descending: bool) -> std::cmp::Ordering {
    match (a, b) {
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

/// One packet's worth of facts about the device that sent it, as [`observe`]
/// takes them. Everything but the address may be missing: each is a
/// measurement a given packet may not have supported.
#[derive(Clone, Copy, Debug)]
pub struct Sighting {
    pub address: [u8; 6],
    pub random: bool,
    pub snr_db: Option<f64>,
    /// In ppm, as the air delivered it: see [`Device::crystal_offset_ppm`].
    pub crystal_offset_ppm: Option<Uncertain>,
    /// The BLE PDU type's four-bit code (`signal::ble::pdu::PduType::code`).
    pub ble_pdu_code: Option<u8>,
    pub modulation_index: Option<Uncertain>,
}

/// Record one packet whose CRC passed: a new row if this is the first time
/// its address has been heard, otherwise an update to the existing one.
///
/// **`snr_db` replaces the best reading only when it is stronger** - the
/// "best it has been heard" the field's own name promises, not the most
/// recent - and is added to the sums the mean comes from either way.
/// **The crystal offset and the modulation index are refined, not replaced**:
/// [`Uncertain::combine`] folds a new packet's reading into the device's
/// estimate, the same way more samples tighten any other measurement in this
/// app, rather than keeping only the latest packet's own noisy reading.
pub fn observe(devices: &mut Vec<Device>, s: &Sighting, now: Instant) {
    let device = match devices.iter_mut().position(|d| d.address == s.address) {
        Some(i) => &mut devices[i],
        None => {
            devices.push(Device::heard(s.address, s.random, now));
            devices.last_mut().expect("just pushed")
        }
    };
    device.packets += 1;
    device.last_seen = now;
    if let Some(snr) = s.snr_db {
        let best = device.best_snr_db.map_or(snr as f32, |b| b.max(snr as f32));
        device.best_snr_db = Some(best);
        device.snr_count += 1;
        device.snr_sum += snr;
        device.snr_sum_sq += snr * snr;
    }
    if let Some(code) = s.ble_pdu_code {
        device.ble_pdu_types |= 1 << (code & 0x0F);
    }
    refine(&mut device.crystal_offset_ppm, s.crystal_offset_ppm);
    refine(&mut device.modulation_index, s.modulation_index);
}

/// Fold `reading` into `estimate`, or start it.
fn refine(estimate: &mut Option<Uncertain>, reading: Option<Uncertain>) {
    if let Some(r) = reading {
        *estimate = Some(match *estimate {
            Some(e) => e.combine(&r),
            None => r,
        });
    }
}

/// Credit a packet whose CRC failed to the device whose address its address
/// field reads, if the census already holds one. Returns whether it did.
///
/// **Only an exact match on a device already confirmed, and never a new
/// row.** A failed CRC says some bit between the header and the CRC is
/// wrong and not which, so the address itself is under suspicion; what makes
/// an exact match worth crediting is that corruption landing on one of the
/// few dozen 48-bit addresses a room holds is not a coincidence worth
/// designing for, while corruption landing anywhere else simply leaves the
/// packet uncredited. So every device's failures are a floor and its pass rate
/// a ceiling (`Device::crc_pass_rate`), which is the honest side to err on:
/// the rate can flatter a device and cannot slander one.
///
/// **It does not touch `last_seen`.** The sightings the census dates are the
/// ones it confirmed; a packet it cannot vouch for has no business moving the
/// SEEN column, or keeping a device that went quiet looking present.
pub fn observe_crc_failure(devices: &mut [Device], address: [u8; 6]) -> bool {
    match devices.iter_mut().find(|d| d.address == address) {
        Some(d) => {
            d.crc_failed += 1;
            true
        }
        None => false,
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
///
/// **Split by address kind (net-ux-polish-plan 4.4), busiest kind first**,
/// the kind breaking a tie in [`kind_rank`]'s order. Only the private kinds
/// are regenerated while a device runs (the Generic Access Profile's
/// private-address timer, Core Vol 3 Part C 10.7, fifteen minutes
/// recommended; a static address changes only across a power cycle, Vol 6
/// Part B 1.3.2.1; cited from the specification's structure, not quoted from
/// a copy read this session). So a total that mixes kinds is not the rotation
/// rate B13 set out to show: a new public address is a device walking in, a
/// new resolvable one may be a device already here wearing a new address.
/// Split, the line can say which. Empty when nothing new appeared, or the
/// window is zero.
pub fn turnover_by_kind(
    devices: &[Device],
    window: std::time::Duration,
    now: Instant,
) -> Vec<(AddressKind, f64)> {
    if window.is_zero() {
        return Vec::new();
    }
    let minutes = window.as_secs_f64() / 60.0;
    let mut counts: Vec<(AddressKind, usize)> = Vec::new();
    for d in devices
        .iter()
        .filter(|d| now.saturating_duration_since(d.first_seen) <= window)
    {
        let k = d.kind();
        match counts.iter_mut().find(|(c, _)| *c == k) {
            Some((_, n)) => *n += 1,
            None => counts.push((k, 1)),
        }
    }
    counts.sort_by(|a, b| b.1.cmp(&a.1).then(kind_rank(a.0).cmp(&kind_rank(b.0))));
    counts
        .into_iter()
        .map(|(k, n)| (k, n as f64 / minutes))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// All the new addresses a minute, every kind together.
    fn turnover_per_minute(devices: &[Device], window: Duration, now: Instant) -> f64 {
        turnover_by_kind(devices, window, now)
            .iter()
            .map(|(_, r)| r)
            .sum()
    }

    fn radio() -> crate::state::RadioState {
        crate::state::SdrMetrics::fixture().radio
    }

    fn device(last: u8, packets: u64, snr: f32, ago_s: u64, now: Instant) -> Device {
        Device {
            packets,
            best_snr_db: Some(snr),
            ..Device::heard(
                [0xa4, 0x83, 0xe7, 0x1c, 0x09, last],
                false,
                now - Duration::from_secs(ago_s),
            )
        }
    }

    /// A packet with only an SNR, for the tests that are about nothing else.
    fn heard(address: [u8; 6], snr_db: Option<f64>) -> Sighting {
        Sighting {
            address,
            random: false,
            snr_db,
            crystal_offset_ppm: None,
            ble_pdu_code: None,
            modulation_index: None,
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
        order(&mut d, column("ADDRESS"), false, now, &radio());
        assert_eq!(tails(&d), vec![1, 2, 3], "by address, ascending");

        let mut d = make();
        order(&mut d, column("SEEN"), false, now, &radio());
        assert_eq!(tails(&d), vec![1, 3, 2], "most recently seen first");

        let mut d = make();
        order(&mut d, column("PKTS"), true, now, &radio());
        assert_eq!(tails(&d), vec![1, 3, 2], "busiest first");

        let mut d = make();
        order(&mut d, column("SNR"), true, now, &radio());
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
        order(&mut a, column("PKTS"), true, now, &radio());
        order(&mut b, column("PKTS"), true, now, &radio());
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

        order(&mut d, column("CFO"), true, now, &radio()); // worst first
        assert_eq!(tails(&d), vec![1, 3, 2]);

        order(&mut d, column("CFO"), false, now, &radio()); // best first
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

        order(&mut d, column("CFO"), true, now, &radio()); // worst first, raw
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
        order(&mut d, column("CFO"), true, now, &referenced);
        assert_eq!(tails(&d), vec![2, 1], "the dead-on clock is the best one");
    }

    /// The titles and the keys `order` switches on are one list each, of one
    /// length, so a new column cannot be drawn without something to sort it
    /// by, and the index the state keeps names the same thing in both.
    #[test]
    fn the_sort_keys_are_the_column_titles() {
        assert_eq!(SORT_KEYS.len(), Key::ALL.len());
        for (title, key) in [
            ("KIND", Key::Kind),
            ("PKTS", Key::Packets),
            ("CFO", Key::Cfo),
            ("MOD", Key::Modulation),
        ] {
            assert_eq!(Key::ALL[column(title)], key, "{title}");
        }
    }

    #[test]
    fn a_first_sighting_creates_a_row() {
        let now = Instant::now();
        let mut devices = Vec::new();
        observe(
            &mut devices,
            &Sighting {
                crystal_offset_ppm: Some(Uncertain::from_sigma(120.0, 20.0)),
                ..heard([1, 2, 3, 4, 5, 6], Some(4.0))
            },
            now,
        );
        assert_eq!(devices.len(), 1);
        let d = &devices[0];
        assert_eq!(d.packets, 1);
        assert_eq!(d.best_snr_db, Some(4.0));
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
        observe(&mut devices, &heard([1, 2, 3, 4, 5, 6], None), born);
        observe(&mut devices, &heard([1, 2, 3, 4, 5, 6], None), later);
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
        observe(&mut devices, &heard(addr, Some(4.0)), now);
        observe(&mut devices, &heard(addr, Some(9.0)), now);
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].packets, 2);
        // Best-ever, not most-recent: 4.0 dB does not overwrite 9.0 dB.
        assert_eq!(devices[0].best_snr_db, Some(9.0));

        let mut devices2 = Vec::new();
        observe(&mut devices2, &heard(addr, Some(9.0)), now);
        observe(&mut devices2, &heard(addr, Some(4.0)), now);
        assert_eq!(devices2[0].best_snr_db, Some(9.0), "order must not matter");
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
        for ppm in [a, b] {
            let s = Sighting {
                crystal_offset_ppm: Some(ppm),
                ..heard(addr, None)
            };
            observe(&mut devices, &s, now);
        }
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

    /// **The streaming mean is the house mean.** The record keeps sums
    /// because it cannot keep every reading; this holds the sums to the
    /// estimator that takes the readings, value and uncertainty both.
    #[test]
    fn the_mean_snr_from_sums_is_the_mean_from_the_readings() {
        let now = Instant::now();
        let readings = [12.0f32, 9.5, 14.25, 11.0, 7.75, 13.5];
        let mut devices = Vec::new();
        for r in readings {
            observe(&mut devices, &heard([1; 6], Some(r as f64)), now);
        }
        let got = devices[0].mean_snr_db().unwrap();
        let want = crate::signal::dsp::uncertainty::mean_with_uncertainty(&readings);
        assert!(
            (got.value() - want.value()).abs() < 1e-9,
            "{got:?} {want:?}"
        );
        assert!(
            (got.sigma() - want.sigma()).abs() < 1e-9,
            "{got:?} {want:?}"
        );
        assert_eq!(devices[0].best_snr_db, Some(14.25));
    }

    /// One reading is a mean with no spread to estimate from: an infinite
    /// uncertainty, which the reading cell dashes, never a zero that would
    /// read as a perfect measurement. None at all is no mean.
    #[test]
    fn one_reading_is_a_mean_nobody_can_vouch_for_and_none_is_no_mean() {
        let now = Instant::now();
        let mut devices = Vec::new();
        observe(&mut devices, &heard([1; 6], None), now);
        assert!(devices[0].mean_snr_db().is_none());
        assert_eq!(devices[0].best_snr_db, None);

        observe(&mut devices, &heard([1; 6], Some(8.0)), now);
        let one = devices[0].mean_snr_db().unwrap();
        assert_eq!(one.value(), 8.0);
        assert!(one.sigma().is_infinite());

        // Identical readings: a zero spread, not a negative one from rounding.
        observe(&mut devices, &heard([1; 6], Some(8.0)), now);
        observe(&mut devices, &heard([1; 6], Some(8.0)), now);
        assert_eq!(devices[0].mean_snr_db().unwrap().sigma(), 0.0);
    }

    /// **A failed CRC is credited only to a device already confirmed, on an
    /// exact match.** It never makes a row, and never moves the SEEN column:
    /// the census dates what it confirmed.
    #[test]
    fn a_failed_crc_is_credited_only_to_an_address_already_confirmed() {
        let born = Instant::now();
        let mut devices = Vec::new();
        observe(&mut devices, &heard([1; 6], None), born);
        observe(&mut devices, &heard([1; 6], None), born);
        observe(&mut devices, &heard([1; 6], None), born);

        assert!(observe_crc_failure(&mut devices, [1; 6]));
        assert!(
            !observe_crc_failure(&mut devices, [2; 6]),
            "an unknown address"
        );
        assert_eq!(devices.len(), 1, "no row for an address nobody confirmed");
        assert_eq!(devices[0].crc_failed, 1);
        assert_eq!(devices[0].last_seen, born);
        // Three good, one failed.
        assert!((devices[0].crc_pass_rate() - 0.75).abs() < 1e-12);
    }

    /// The PDU types are kept as the set heard, whatever order and however
    /// often, the extended-advertising codes included.
    #[test]
    fn the_pdu_types_are_the_set_heard() {
        let now = Instant::now();
        let mut devices = Vec::new();
        for code in [0x0, 0x4, 0x0, 0x0, 0x7] {
            let s = Sighting {
                ble_pdu_code: Some(code),
                ..heard([1; 6], None)
            };
            observe(&mut devices, &s, now);
        }
        assert_eq!(devices[0].ble_pdu_type_count(), 3);
        assert_eq!(
            devices[0].ble_pdu_codes().collect::<Vec<_>>(),
            vec![0, 4, 7]
        );
    }

    /// The modulation index tightens with packets the way the crystal offset
    /// does, and a packet B8 could not measure leaves it alone.
    #[test]
    fn the_modulation_index_is_refined_and_an_unmeasured_packet_leaves_it() {
        let now = Instant::now();
        let a = Uncertain::from_sigma(0.49, 0.02);
        let b = Uncertain::from_sigma(0.51, 0.02);
        let mut devices = Vec::new();
        for m in [Some(a), None, Some(b)] {
            let s = Sighting {
                modulation_index: m,
                ..heard([1; 6], None)
            };
            observe(&mut devices, &s, now);
        }
        let got = devices[0].modulation_index.unwrap();
        let want = a.combine(&b);
        assert_eq!((got.value(), got.sigma()), (want.value(), want.sigma()));
    }

    /// Every column that can be absent sorts its absent rows last, ascending
    /// and descending: best SNR, mean SNR and modulation index as well as the
    /// CFO the rule was first written for.
    #[test]
    fn every_optional_column_puts_the_unmeasured_last_both_ways() {
        let now = Instant::now();
        let measured = |tail: u8, v: f64| {
            let mut d = Device {
                best_snr_db: Some(v as f32),
                modulation_index: Some(Uncertain::exact(v)),
                ..Device::heard([0, 0, 0, 0, 0, tail], false, now)
            };
            for _ in 0..2 {
                d.snr_count += 1;
                d.snr_sum += v;
                d.snr_sum_sq += v * v;
            }
            d
        };
        let tails = |d: &[Device]| d.iter().map(|x| x.address[5]).collect::<Vec<_>>();
        for sort in ["SNR", "MEAN SNR", "MOD"].map(column) {
            let mut d = vec![
                Device::heard([0, 0, 0, 0, 0, 1], false, now),
                measured(2, 1.0),
                measured(3, 5.0),
            ];
            order(&mut d, sort, true, now, &radio());
            assert_eq!(tails(&d), vec![3, 2, 1], "{} descending", SORT_KEYS[sort]);
            order(&mut d, sort, false, now, &radio());
            assert_eq!(tails(&d), vec![2, 3, 1], "{} ascending", SORT_KEYS[sort]);
        }
    }

    /// CRC orders by pass rate and TYPES by how many were heard: worst link
    /// first when ascending, the chattiest device first when descending.
    #[test]
    fn crc_and_types_order_by_what_they_name() {
        let now = Instant::now();
        let d = |tail: u8, good: u64, bad: u64, types: u16| Device {
            packets: good,
            crc_failed: bad,
            ble_pdu_types: types,
            ..Device::heard([0, 0, 0, 0, 0, tail], false, now)
        };
        let tails = |d: &[Device]| d.iter().map(|x| x.address[5]).collect::<Vec<_>>();
        let make = || vec![d(1, 10, 0, 0b1), d(2, 5, 5, 0b10011), d(3, 9, 1, 0b11)];

        let mut v = make();
        order(&mut v, column("CRC"), false, now, &radio());
        assert_eq!(tails(&v), vec![2, 3, 1], "50 %, 90 %, 100 %");

        let mut v = make();
        order(&mut v, column("TYPES"), true, now, &radio());
        assert_eq!(tails(&v), vec![2, 3, 1], "3 types, 2, 1");
    }

    /// KIND orders from the address that says most about its sender to the
    /// one that says least, and the reserved kind last.
    #[test]
    fn kind_orders_from_public_to_private() {
        let now = Instant::now();
        let with = |top: u8, random: bool| Device::heard([top, 0, 0, 0, 0, top], random, now);
        let mut d = vec![
            with(0x00, true),  // non-resolvable
            with(0x80, true),  // reserved
            with(0x40, true),  // resolvable
            with(0xc0, true),  // static
            with(0x11, false), // public
        ];
        order(&mut d, column("KIND"), false, now, &radio());
        let kinds: Vec<&str> = d.iter().map(|x| x.kind().label()).collect();
        assert_eq!(kinds, ["public", "static", "RPA", "NRPA", "reserved"]);
    }

    /// **The turnover split by kind**: three resolvable addresses and one
    /// public appeared in two minutes, one static before the window; the
    /// rates are per kind, busiest first, and still sum to the total.
    #[test]
    fn turnover_splits_by_kind_and_sums_to_the_total() {
        let now = Instant::now();
        let at = |top: u8, tail: u8, random: bool, ago: u64| {
            Device::heard(
                [top, 0, 0, 0, 0, tail],
                random,
                now - Duration::from_secs(ago),
            )
        };
        let devices = vec![
            at(0x40, 1, true, 10),
            at(0x41, 2, true, 50),
            at(0x42, 3, true, 100),
            at(0x11, 4, false, 30),
            at(0xc0, 5, true, 500),
        ];
        let window = Duration::from_secs(120);
        let split = turnover_by_kind(&devices, window, now);
        assert_eq!(split.len(), 2, "{split:?}");
        assert_eq!(split[0].0, AddressKind::ResolvablePrivate);
        assert!((split[0].1 - 1.5).abs() < 1e-9, "{split:?}");
        assert_eq!(split[1].0, AddressKind::Public);
        assert!((split[1].1 - 0.5).abs() < 1e-9, "{split:?}");
        assert!((turnover_per_minute(&devices, window, now) - 2.0).abs() < 1e-9);
        assert!(turnover_by_kind(&devices, Duration::ZERO, now).is_empty());
    }
}
