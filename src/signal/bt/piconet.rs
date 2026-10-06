// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The piconet roster: one record per LAP heard. **Keyed by what a LAP really
//! is.** A classic access code carries the lower address part of the piconet's
//! *master*, so a LAP names a piconet, not a device: the slaves in it send the
//! master's access code too. This is a roster of piconets, and it does not
//! pretend to be a device census: what identifies a classic *device* to a
//! passive listener is an open question.
//!
//! **Counted over the session, where `NetState::bt_hops` keeps a window.**
//! The scatter needs the last few hundred hits with their times; a roster
//! needs every hit, but only as counts. So each hit updates one small record
//! here, inside the lock, with increments and a bit set (the `tasks/rx`
//! discipline), and the list of hops keeps its own cap.
//!
//! **A hit is an exact 64-bit access code** (`access_code::find_access_code`
//! corrects no bit errors), so a LAP in this roster was sent: a random
//! window reads as some valid code about once in 2^40 bit positions, which
//! at ten watched channels is one false row in a day and more of listening.

use std::time::Instant;

/// A LAP the specification keeps for inquiry, which names no piconet.
///
/// **Read from the primary sources**: Core 5.4 Vol 2 Part B 1.2.1 reserves
/// 0x9E8B00 to 0x9E8B3F, one of them (0x9E8B33) for general inquiry and the
/// other 63 for dedicated inquiry, and says none can be part of a device's
/// own address; the Assigned Numbers document (2.2, Special LAPs) names
/// 0x9E8B00 the Limited Inquiry Access Code. A device looking for others
/// sends one of these, and so does every other device looking, so a hit
/// on one is somebody searching, not a master's piconet: there is no one
/// clock to fit, no member's modulation, and no UAP to narrow, because the
/// same section fixes it at the DCI, 0x00.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Inquiry {
    /// GIAC, 0x9E8B33: any device inquiring for any other.
    General,
    /// LIAC, 0x9E8B00: inquiry for devices in limited discoverable mode.
    Limited,
    /// One of the other 62 dedicated codes.
    Dedicated,
}

/// The first and last reserved LAP (Core 5.4 Vol 2 Part B 1.2.1).
const RESERVED: std::ops::RangeInclusive<u32> = 0x9E_8B00..=0x9E_8B3F;

/// The UAP every reserved LAP is sent with, the default check
/// initialisation (Core 5.4 Vol 2 Part B 1.2.1).
pub const DCI: u8 = 0x00;

impl Inquiry {
    pub fn of(lap: u32) -> Option<Self> {
        match lap {
            0x9E_8B33 => Some(Self::General),
            0x9E_8B00 => Some(Self::Limited),
            l if RESERVED.contains(&l) => Some(Self::Dedicated),
            _ => None,
        }
    }

    /// The code's own abbreviation, as the specification writes it.
    pub fn short(self) -> &'static str {
        match self {
            Self::General => "GIAC",
            Self::Limited => "LIAC",
            Self::Dedicated => "DIAC",
        }
    }

    /// What a hit on it means, in a few words.
    pub fn meaning(self) -> &'static str {
        match self {
            Self::General => "general inquiry: a device looking for any other",
            Self::Limited => "limited inquiry: a device looking for ones briefly discoverable",
            Self::Dedicated => "dedicated inquiry: a device looking for one class of others",
        }
    }
}

use super::header::Header;
use crate::signal::dsp::carrier::Drift;
use crate::signal::dsp::deviation::Sums;
use crate::signal::dsp::uncertainty::Uncertain;

/// A piconet's deviation readings, gathered from
/// the access code, trailer and header of every header captured on its
/// LAP, as sums (`signal::dsp::deviation`): bits whose neighbours both
/// equal them give delta-f1, the modulation index's own reading, and bits
/// whose neighbours both differ give delta-f2.
///
/// **Every member's transmissions, not the master's alone**: a LAP names a
/// piconet, and a slave answering in it sends the master's access code
/// too, so this is the piconet's modulation, said so where it is shown.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Deviation {
    pub settled: Sums,
    pub alternating: Sums,
    /// Headers not read because the next channel was busy at the same
    /// moment (`signal::net::measure`).
    pub neighbour_busy: u64,
}

impl Deviation {
    /// One header's readings, delta-f1 and delta-f2 in Hz, as the test
    /// suites define them (`dsp::deviation::suite_readings`, read by
    /// `signal::net::measure`).
    pub fn from_readings(settled: &[f32], alternating: &[f32]) -> Self {
        Self {
            settled: Sums::of(settled),
            alternating: Sums::of(alternating),
            neighbour_busy: 0,
        }
    }

    /// One header not read, because a neighbour was too loud to read past.
    pub fn neighbour_busy() -> Self {
        Self {
            neighbour_busy: 1,
            ..Self::default()
        }
    }
}

/// What a piconet's headers have said: counted
/// from every header captured on its LAP, and read only once its UAP has
/// narrowed to one value. Before that a header's HEC cannot say which
/// dewhitening is right, so nothing about its content is counted, not even
/// a guess (rule 2).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Headers {
    /// Headers captured after this LAP's access codes.
    pub captured: u64,
    /// Of those, captured while the UAP was one value and read under it.
    pub decoded: u64,
    /// Captured while the UAP was one value and no CLK1-6 reproduced its
    /// HEC under it: a damaged capture, or a UAP that is wrong. Counted,
    /// since a rising count is how a wrong resolution would show.
    pub undecoded: u64,
    /// Decoded headers per 4-bit `TYPE` (`header::PacketType::code`).
    pub types: [u32; 16],
    /// Which LT_ADDRs decoded headers carried, one bit each (0 is the
    /// master's broadcast).
    pub lt_addrs: u8,
    /// The CLK1-6 hypotheses still standing after the latest header
    /// (`header::PiconetClock::hypotheses`).
    pub clock_hypotheses: u8,
    /// Deviation readings from every captured header's symbols, resolved
    /// or not: the modulation does not need the UAP.
    pub deviation: Deviation,
    /// The carrier under every measured header, likewise.
    pub carrier: Carrier,
    /// The same readings kept by who sent them, so each device of the
    /// piconet has its own figures; `deviation` and `carrier` above stay
    /// their sum.
    pub sides: Sides,
}

/// Who sent a packet, **read from the Core Specification 5.4, Vol 2, Part
/// B, 2.2.5** on the SIG's own site: "The Central transmission shall always
/// start at even numbered time slots (CLK1=0) and the Peripheral
/// transmission shall always start at odd numbered time slots (CLK1=1)."
/// Both on the master's clock, so the slot's parity is CLK1, the lowest bit
/// of the CLK1-6 a header is read at. A packet's start is what its access
/// code dates, so a multi-slot packet running on into the other parity
/// does not change who sent it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Master,
    Slave,
}

impl Direction {
    /// From the CLK1-6 a header was read at.
    pub fn of_clk6(clk6: u8) -> Self {
        if clk6 & 1 == 0 {
            Direction::Master
        } else {
            Direction::Slave
        }
    }
}

/// What a packet's payload told.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PayloadVerdict {
    /// POLL, NULL, or an access code with no header after it.
    NoPayload,
    /// Checked: the CRC passed (`true`) or failed. A failure says nothing
    /// about why: an encrypted payload and a damaged capture fail alike.
    Crc(bool),
    /// Not read, and why, in the list's words ("FEC failed", "cut short",
    /// "clock not known": `payload::Unchecked::words`).
    NotRead(&'static str),
}

/// One packet of a piconet, as the Piconet view lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct BtPacket {
    pub seen: Instant,
    /// When its access code ended, us on the stream's clock, and which
    /// stream (`state::BtHop` keeps the same pair, for the same reason).
    pub at_us: f64,
    pub stream: u32,
    pub channel: u8,
    /// `None`: an access code with no header after it.
    pub header: Option<HeaderRead>,
    /// `None` until the header is read at one clock.
    pub direction: Option<Direction>,
    /// This packet's own readings; empty when none were taken.
    pub deviation: Deviation,
    pub carrier: Carrier,
    /// Its f0 in ppm of its channel, with the uncertainty its preamble
    /// gives: `carrier` keeps sums for pooling, which one reading's spread
    /// cannot come back out of.
    pub f0_ppm: Option<Uncertain>,
    pub payload: PayloadVerdict,
}

/// One side's readings, and how many packets it has sent.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Side {
    pub deviation: Deviation,
    pub carrier: Carrier,
    pub packets: u64,
}

impl Side {
    fn add(&mut self, deviation: Deviation, carrier: Carrier) {
        self.deviation.settled.add(deviation.settled);
        self.deviation.alternating.add(deviation.alternating);
        self.deviation.neighbour_busy += deviation.neighbour_busy;
        self.carrier.add(carrier);
    }
}

/// A piconet's readings by who sent them. `unknown` holds the packets whose
/// direction is not known yet: never guessed onto a side.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sides {
    pub master: Side,
    pub slave: Side,
    pub unknown: Side,
}

impl Sides {
    pub fn of(&mut self, direction: Option<Direction>) -> &mut Side {
        match direction {
            Some(Direction::Master) => &mut self.master,
            Some(Direction::Slave) => &mut self.slave,
            None => &mut self.unknown,
        }
    }
}

/// How many packets a piconet keeps: its own ring, so a busy neighbour
/// cannot push out the one being watched.
pub const PACKETS_KEPT: usize = 1000;

/// A piconet's carrier as the test suites define it (`dsp::carrier`, read by
/// `signal::net::measure`), from every header measured on its LAP:
/// every member's, like its modulation.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Carrier {
    /// Each header's f0, in ppm of its channel's frequency: a piconet hops,
    /// and only a fraction of the carrier compares across channels (the
    /// census keeps BLE offsets the same way).
    pub f0_ppm: Sums,
    /// The channels those headers were on, in MHz, to turn the mean back
    /// into kHz.
    pub channel_mhz: Sums,
    /// The header whose drift reached furthest from its f0, `fk - f0` in
    /// Hz. The limit is on every packet, so the worst one is what is held
    /// to it.
    pub worst_drift_hz: Option<Uncertain>,
    /// The header with the steepest five-block step, in Hz/us.
    pub worst_rate_hz_per_us: Option<Uncertain>,
}

impl Carrier {
    /// One header's: its f0 and drift figures, on channel `channel_hz`.
    pub fn of(drift: &Drift, channel_hz: f64) -> Self {
        Self {
            f0_ppm: Sums::of(&[(drift.initial_hz.value() / channel_hz * 1e6) as f32]),
            channel_mhz: Sums::of(&[(channel_hz / 1e6) as f32]),
            worst_drift_hz: Some(drift.drift_hz),
            worst_rate_hz_per_us: Some(drift.drift_rate_hz_per_us),
        }
    }

    /// Pool `other` in: sums added, the worse of each worst kept.
    pub fn add(&mut self, other: Carrier) {
        self.f0_ppm.add(other.f0_ppm);
        self.channel_mhz.add(other.channel_mhz);
        let worse = |a: Option<Uncertain>, b: Option<Uncertain>| match (a, b) {
            (Some(a), Some(b)) => Some(if b.value().abs() > a.value().abs() {
                b
            } else {
                a
            }),
            (a, b) => a.or(b),
        };
        self.worst_drift_hz = worse(self.worst_drift_hz, other.worst_drift_hz);
        self.worst_rate_hz_per_us = worse(self.worst_rate_hz_per_us, other.worst_rate_hz_per_us);
    }
}

/// One header's outcome, as the worker hands it over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeaderRead {
    /// The UAP is not one value yet: the header is captured, not read.
    Unresolved,
    /// Read under the resolved UAP.
    Decoded(Header),
    /// The UAP is one value and no clock reproduced this header under it.
    Undecoded,
}

/// One piconet, as its hits describe it.
#[derive(Clone, Debug, PartialEq)]
pub struct Piconet {
    /// The master's lower address part, 24 bits.
    pub lap: u32,
    /// Access codes found carrying this LAP.
    pub hits: u64,
    /// Hits found on each of the 79 channels: where it hops, as the hop
    /// panel's WHERE bars draw it.
    pub per_channel: [u32; 79],
    pub first_seen: Instant,
    pub last_seen: Instant,
    pub headers: Headers,
    /// Its slot grid and jitter, or why there is none (`super::slots`),
    /// refitted by the worker at most once a second; `None` before its
    /// first hit was timed.
    pub slots: Option<Result<super::slots::SlotFit, super::slots::SlotRefusal>>,
    /// The stream `slots` was fitted on (`state::BtHop::stream`).
    pub slots_stream: u32,
    /// How its hits are spaced burst by burst (`slots::pace`): whole slots
    /// for a piconet, odd half slots too for inquiry and paging. Refreshed
    /// with `slots`.
    pub pace: super::slots::Pace,
    /// Its last [`PACKETS_KEPT`] packets, newest first.
    pub packets: std::collections::VecDeque<BtPacket>,
}

/// What a LAP's hits are, as far as they can say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A master's piconet: the default, and all that is claimed without
    /// evidence otherwise.
    Piconet,
    /// A reserved inquiry code (`Inquiry`), known from the LAP alone.
    Inquiry(Inquiry),
    /// Somebody paging the device this LAP belongs to: see [`Piconet::kind`].
    Paged,
}

impl Kind {
    /// One word for a table cell and the export.
    pub fn word(self) -> &'static str {
        match self {
            Kind::Piconet => "piconet",
            Kind::Inquiry(_) => "inquiry",
            Kind::Paged => "paged",
        }
    }
}

/// Hits with no header after any of them before that absence is taken as
/// ID packets: a header passes its FEC on noise about once in 7·10^10, so
/// the risk is a weak real piconet whose headers all failed, which this
/// many hits makes unlikely.
const PAGED_MIN_HITS: u64 = 16;

impl Piconet {
    /// What these hits are.
    ///
    /// **Paged only on both signs, measured.** Paging sends the paged
    /// device's own access code as ID packets, which carry no header (Core
    /// 5.4 Vol 2 Part B 8.3.2, 5.1), at inquiry's and paging's 3200-a-second
    /// pace, so their spacings include odd half slots, which a piconet's
    /// never do (`slots::pace`). Without a header after any of
    /// [`PAGED_MIN_HITS`] hits and with that pace beyond chance, the LAP is
    /// the *called* device's, not a master's; with either sign missing it
    /// stays a piconet, which is what every row was before. An answered
    /// page carries headers under the same code (the FHS) and so stays a
    /// piconet: missed, rather than named wrongly.
    pub fn kind(&self) -> Kind {
        if let Some(i) = Inquiry::of(self.lap) {
            return Kind::Inquiry(i);
        }
        if self.headers.captured == 0 && self.hits >= PAGED_MIN_HITS && self.pace.is_half_slot() {
            return Kind::Paged;
        }
        Kind::Piconet
    }

    /// How many different channels it has been heard on.
    pub fn channels_hit(&self) -> u32 {
        self.per_channel.iter().filter(|&&n| n > 0).count() as u32
    }

    /// The channels it has been heard on, one bit per channel.
    pub fn channel_mask(&self) -> u128 {
        self.per_channel
            .iter()
            .enumerate()
            .filter(|(_, &n)| n > 0)
            .fold(0, |m, (ch, _)| m | 1 << ch)
    }
}

/// Record one hit: a new row for a LAP heard for the first time, an update
/// otherwise. A channel beyond the 79 counts the hit and sets no bit.
pub fn observe(roster: &mut Vec<Piconet>, lap: u32, channel: u8, now: Instant) {
    let p = match roster.iter().position(|p| p.lap == lap) {
        Some(i) => &mut roster[i],
        None => {
            roster.push(Piconet {
                lap,
                hits: 0,
                per_channel: [0; 79],
                first_seen: now,
                last_seen: now,
                headers: Headers::default(),
                slots: None,
                slots_stream: 0,
                pace: Default::default(),
                packets: std::collections::VecDeque::new(),
            });
            roster.last_mut().expect("just pushed")
        }
    };
    p.hits += 1;
    p.last_seen = now;
    if let Some(n) = p.per_channel.get_mut(channel as usize) {
        *n = n.saturating_add(1);
    }
}

/// Record one captured header of `lap`, with the clock hypotheses left
/// standing after it. A LAP the roster has no row for is skipped: a header
/// always follows an access code, which made the row.
pub fn observe_header(
    roster: &mut [Piconet],
    lap: u32,
    read: HeaderRead,
    hypotheses: u8,
    deviation: Deviation,
    carrier: Carrier,
) {
    let Some(p) = roster.iter_mut().find(|p| p.lap == lap) else {
        return;
    };
    let h = &mut p.headers;
    h.captured += 1;
    h.clock_hypotheses = hypotheses;
    h.deviation.settled.add(deviation.settled);
    h.deviation.alternating.add(deviation.alternating);
    h.deviation.neighbour_busy += deviation.neighbour_busy;
    h.carrier.add(carrier);
    match read {
        HeaderRead::Unresolved => {}
        HeaderRead::Undecoded => h.undecoded += 1,
        HeaderRead::Decoded(header) => {
            h.decoded += 1;
            let t = &mut h.types[header.packet_type.code() as usize & 0x0f];
            *t = t.saturating_add(1);
            h.lt_addrs |= 1 << (header.lt_addr & 0x07);
        }
    }
}

/// Record one packet: into the piconet's ring, newest first, and its
/// readings onto its side. A LAP the roster has no row for is skipped: a
/// packet always follows the access code that made the row.
pub fn observe_packet(roster: &mut [Piconet], lap: u32, packet: BtPacket) {
    let Some(p) = roster.iter_mut().find(|p| p.lap == lap) else {
        return;
    };
    let side = p.headers.sides.of(packet.direction);
    side.add(packet.deviation, packet.carrier);
    side.packets += 1;
    p.packets.push_front(packet);
    p.packets.truncate(PACKETS_KEPT);
}

/// What reading a packet's header told, handed over together.
#[derive(Clone, Debug, PartialEq)]
pub struct PacketReading {
    pub header: HeaderRead,
    pub direction: Option<Direction>,
    pub deviation: Deviation,
    pub carrier: Carrier,
    pub f0_ppm: Option<Uncertain>,
    pub payload: PayloadVerdict,
}

/// A header read after its access code was recorded: fill in the record
/// made at the hit (same stream, within 2 us, the tolerance the hop join
/// uses) and move it from `unknown` to its side, with its readings. The
/// hit's record came with empty readings, so only the count moves; the
/// readings are added where the direction puts them. No record near that
/// time, or one already read: nothing changes.
pub fn read_packet(
    roster: &mut [Piconet],
    lap: u32,
    stream: u32,
    at_us: f64,
    reading: PacketReading,
) {
    let Some(p) = roster.iter_mut().find(|p| p.lap == lap) else {
        return;
    };
    let Some(packet) = p
        .packets
        .iter_mut()
        .find(|k| k.stream == stream && k.header.is_none() && (k.at_us - at_us).abs() < 2.0)
    else {
        return;
    };
    packet.header = Some(reading.header);
    packet.direction = reading.direction;
    packet.deviation = reading.deviation;
    packet.carrier = reading.carrier;
    packet.f0_ppm = reading.f0_ppm;
    packet.payload = reading.payload;
    let sides = &mut p.headers.sides;
    sides.unknown.packets = sides.unknown.packets.saturating_sub(1);
    let side = sides.of(reading.direction);
    side.packets += 1;
    side.add(reading.deviation, reading.carrier);
}

/// The LAPs in the order the roster is drawn: the most recently heard
/// first, ties by LAP so the order does not shuffle between frames.
pub fn ordered(roster: &[Piconet]) -> Vec<&Piconet> {
    let mut out: Vec<&Piconet> = roster.iter().collect();
    out.sort_by(|a, b| b.last_seen.cmp(&a.last_seen).then(a.lap.cmp(&b.lap)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Paged needs both signs and enough hits; either missing, a piconet.
    #[test]
    fn paged_needs_no_header_and_the_half_slot_pace() {
        use super::super::slots::Pace;
        let t0 = Instant::now();
        let mut roster = Vec::new();
        for _ in 0..20 {
            observe(&mut roster, 0x9a_0af4, 10, t0);
        }
        let half = Pace {
            close: 57,
            whole: 20,
            odd_half: 37,
        };
        roster[0].pace = half;
        assert_eq!(roster[0].kind(), Kind::Paged);
        // A header after one of them: a piconet after all.
        roster[0].headers.captured = 1;
        assert_eq!(roster[0].kind(), Kind::Piconet);
        roster[0].headers.captured = 0;
        // Whole slots only: a piconet whose headers were missed.
        roster[0].pace = Pace {
            close: 57,
            whole: 57,
            odd_half: 0,
        };
        assert_eq!(roster[0].kind(), Kind::Piconet);
        // Too few hits to trust the absence of headers.
        let mut few = Vec::new();
        for _ in 0..10 {
            observe(&mut few, 0x12_3456, 3, t0);
        }
        few[0].pace = half;
        assert_eq!(few[0].kind(), Kind::Piconet);
        assert_eq!(Kind::Paged.word(), "paged");
    }

    /// The reserved block, its two named codes, and its edges.
    #[test]
    fn inquiry_codes_are_the_reserved_block() {
        assert_eq!(Inquiry::of(0x9E_8B33), Some(Inquiry::General));
        assert_eq!(Inquiry::of(0x9E_8B00), Some(Inquiry::Limited));
        assert_eq!(Inquiry::of(0x9E_8B01), Some(Inquiry::Dedicated));
        assert_eq!(Inquiry::of(0x9E_8B3F), Some(Inquiry::Dedicated));
        assert_eq!(Inquiry::of(0x9E_8B40), None);
        assert_eq!(Inquiry::of(0x9E_8AFF), None);
        assert_eq!(Inquiry::of(0x12_3456), None);
    }
    use std::time::Duration;

    #[test]
    fn a_lap_gathers_its_hits_and_the_channels_they_were_on() {
        let t0 = Instant::now();
        let mut roster = Vec::new();
        observe(&mut roster, 0x5a3c71, 10, t0);
        observe(&mut roster, 0x5a3c71, 40, t0 + Duration::from_millis(5));
        observe(&mut roster, 0x5a3c71, 10, t0 + Duration::from_millis(9));
        observe(&mut roster, 0x123456, 78, t0 + Duration::from_millis(7));
        assert_eq!(roster.len(), 2);
        let p = &roster[0];
        assert_eq!((p.hits, p.channels_hit()), (3, 2));
        assert_eq!(p.first_seen, t0);
        assert_eq!(p.last_seen, t0 + Duration::from_millis(9));
        assert_eq!(roster[1].channel_mask(), 1 << 78);
        assert_eq!(p.per_channel[10], 2);

        // A header counts once, and is read only when resolved.
        use crate::signal::bt::header::PacketType;
        let poll = Header {
            lt_addr: 3,
            packet_type: PacketType::Poll,
            flags: 0,
            hec: 0,
            clk6: 0,
        };
        observe_header(
            &mut roster,
            0x5a3c71,
            HeaderRead::Unresolved,
            2,
            Default::default(),
            Default::default(),
        );
        observe_header(
            &mut roster,
            0x5a3c71,
            HeaderRead::Decoded(poll),
            2,
            Default::default(),
            Default::default(),
        );
        observe_header(
            &mut roster,
            0x5a3c71,
            HeaderRead::Undecoded,
            2,
            Default::default(),
            Default::default(),
        );
        observe_header(
            &mut roster,
            0xabcdef,
            HeaderRead::Undecoded,
            2,
            Default::default(),
            Default::default(),
        );
        let h = &roster[0].headers;
        assert_eq!((h.captured, h.decoded, h.undecoded), (3, 1, 1));
        assert_eq!(h.types[PacketType::Poll.code() as usize], 1);
        assert_eq!(h.lt_addrs, 1 << 3);

        // Most recently heard first.
        let order: Vec<u32> = ordered(&roster).iter().map(|p| p.lap).collect();
        assert_eq!(order, vec![0x5a3c71, 0x123456]);
    }

    fn packet(at_us: f64, direction: Option<Direction>, settled: f32) -> BtPacket {
        BtPacket {
            seen: Instant::now(),
            at_us,
            stream: 1,
            channel: 40,
            header: None,
            direction,
            deviation: Deviation::from_readings(&[settled], &[settled * 0.9]),
            carrier: Carrier::default(),
            f0_ppm: None,
            payload: PayloadVerdict::NoPayload,
        }
    }

    /// The master starts in even slots and the slave in odd ones, on the
    /// master's clock: the slot's parity is CLK1-6's lowest bit.
    #[test]
    fn direction_is_the_clocks_slot_parity() {
        assert_eq!(Direction::of_clk6(44), Direction::Master);
        assert_eq!(Direction::of_clk6(45), Direction::Slave);
        assert_eq!(Direction::of_clk6(0), Direction::Master);
        assert_eq!(Direction::of_clk6(63), Direction::Slave);
    }

    /// Each piconet keeps its own last thousand packets, newest first, so a
    /// busy neighbour cannot push out the one being watched.
    #[test]
    fn a_piconet_keeps_its_last_thousand_packets_newest_first() {
        let mut roster = Vec::new();
        observe(&mut roster, 0xc3_d318, 73, Instant::now());
        for k in 0..1005 {
            observe_packet(&mut roster, 0xc3_d318, packet(k as f64, None, 160e3));
        }
        let ring = &roster[0].packets;
        assert_eq!(ring.len(), PACKETS_KEPT);
        assert_eq!(ring[0].at_us, 1004.0);
        assert_eq!(ring[PACKETS_KEPT - 1].at_us, 5.0);
    }

    /// Each packet's readings land on its own side, and the three sides add
    /// up to the totals every header already feeds.
    #[test]
    fn each_side_adds_up_to_the_totals() {
        let mut roster = Vec::new();
        observe(&mut roster, 0xfe_17f1, 73, Instant::now());
        for (k, dir) in [Some(Direction::Master), Some(Direction::Slave), None]
            .into_iter()
            .enumerate()
        {
            let p = packet(k as f64, dir, 150e3 + k as f32 * 5e3);
            observe_header(
                &mut roster,
                0xfe_17f1,
                HeaderRead::Unresolved,
                2,
                p.deviation,
                p.carrier,
            );
            observe_packet(&mut roster, 0xfe_17f1, p);
        }
        let h = &roster[0].headers;
        assert_eq!(
            (
                h.sides.master.packets,
                h.sides.slave.packets,
                h.sides.unknown.packets
            ),
            (1, 1, 1)
        );
        let by_side = h.sides.master.deviation.settled.n
            + h.sides.slave.deviation.settled.n
            + h.sides.unknown.deviation.settled.n;
        assert_eq!(by_side, h.deviation.settled.n);
        assert_eq!(h.sides.master.deviation.settled.sum, 150e3);
        assert_eq!(h.sides.slave.deviation.settled.sum, 155e3);
    }

    /// A packet of a LAP with no row is dropped: a packet always follows the
    /// access code that made the row.
    #[test]
    fn a_packet_for_an_unknown_lap_is_dropped() {
        let mut roster: Vec<Piconet> = Vec::new();
        observe_packet(&mut roster, 0x12_3456, packet(0.0, None, 1.0));
        assert!(roster.is_empty());
    }

    /// Reading a packet fills in its record and moves it from `unknown` to
    /// its side, with its readings; one read with no known direction stays
    /// under `unknown`, its readings added there.
    #[test]
    fn reading_a_packet_moves_it_to_its_side() {
        let mut roster = Vec::new();
        observe(&mut roster, 0xc3_d318, 73, Instant::now());
        observe_packet(&mut roster, 0xc3_d318, packet(100.0, None, 0.0));
        observe_packet(&mut roster, 0xc3_d318, packet(900.0, None, 0.0));
        let reading = Deviation::from_readings(&[160e3], &[150e3]);
        read_packet(
            &mut roster,
            0xc3_d318,
            1,
            100.5,
            PacketReading {
                header: HeaderRead::Unresolved,
                direction: Some(Direction::Slave),
                deviation: reading,
                carrier: Carrier::default(),
                f0_ppm: None,
                payload: PayloadVerdict::NotRead("clock not known"),
            },
        );
        let p = &roster[0];
        let read = p.packets.iter().find(|k| k.at_us == 100.0).unwrap();
        assert_eq!(read.direction, Some(Direction::Slave));
        assert_eq!(read.header, Some(HeaderRead::Unresolved));
        assert_eq!(read.deviation.settled.n, 1);
        let sides = &p.headers.sides;
        assert_eq!((sides.slave.packets, sides.unknown.packets), (1, 1));
        assert_eq!(sides.slave.deviation.settled.n, 1);
        // No record near that time: nothing changes.
        read_packet(
            &mut roster,
            0xc3_d318,
            1,
            5_000.0,
            PacketReading {
                header: HeaderRead::Unresolved,
                direction: Some(Direction::Master),
                deviation: reading,
                carrier: Carrier::default(),
                f0_ppm: None,
                payload: PayloadVerdict::NoPayload,
            },
        );
        assert_eq!(roster[0].headers.sides.master.packets, 0);
    }
}
