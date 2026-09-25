// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The piconet roster: one record per LAP heard (net-ux-polish-plan 6.1).
//!
//! **Keyed by what a LAP really is.** A classic access code carries the
//! lower address part of the piconet's *master*, so a LAP names a piconet,
//! not a device: the slaves in it send the master's access code too. This is
//! a roster of piconets, and it does not pretend to be a device census,
//! which `bluetooth-bench-plan-2.md` Tier 4 keeps as its own open question.
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
use crate::signal::dsp::deviation::Sums;

/// A piconet's deviation readings (net-ux-polish-plan 6.4), gathered from
/// the trailer and header symbols of every header captured on its LAP, as
/// sums (`signal::dsp::deviation`): the settled-run ends are delta-f1, the
/// modulation index's own reading, and the alternating-run ends delta-f2.
///
/// **Every member's transmissions, not the master's alone**: a LAP names a
/// piconet, and a slave answering in it sends the master's access code
/// too, so this is the piconet's modulation, said so where it is shown.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Deviation {
    pub settled: Sums,
    pub alternating: Sums,
}

impl Deviation {
    /// One header's readings: `air` its sliced symbols, `hz` the raw
    /// discriminator at each. Measured from the centre the header's own
    /// settled runs give (`dsp::deviation::settled_centre`); a header whose
    /// runs are all of one polarity gives no reading at all rather than
    /// one against a centre it cannot state.
    pub fn of(air: &[bool], hz: &[f32]) -> Self {
        use crate::signal::dsp::deviation::{run_ends, settled_centre};
        let Some(centre) = settled_centre(air, hz) else {
            return Self::default();
        };
        let from_centre: Vec<f32> = hz.iter().map(|f| f - centre).collect();
        let (settled, alternating) = run_ends(air, &from_centre);
        Self {
            settled: Sums::of(&settled),
            alternating: Sums::of(&alternating),
        }
    }
}

/// What a piconet's headers have said (net-ux-polish-plan 6.3): counted
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
}

impl Piconet {
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
) {
    let Some(p) = roster.iter_mut().find(|p| p.lap == lap) else {
        return;
    };
    let h = &mut p.headers;
    h.captured += 1;
    h.clock_hypotheses = hypotheses;
    h.deviation.settled.add(deviation.settled);
    h.deviation.alternating.add(deviation.alternating);
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
        );
        observe_header(
            &mut roster,
            0x5a3c71,
            HeaderRead::Decoded(poll),
            2,
            Default::default(),
        );
        observe_header(
            &mut roster,
            0x5a3c71,
            HeaderRead::Undecoded,
            2,
            Default::default(),
        );
        observe_header(
            &mut roster,
            0xabcdef,
            HeaderRead::Undecoded,
            2,
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
}
