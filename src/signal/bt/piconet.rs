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

/// One piconet, as its hits describe it.
#[derive(Clone, Debug, PartialEq)]
pub struct Piconet {
    /// The master's lower address part, 24 bits.
    pub lap: u32,
    /// Access codes found carrying this LAP.
    pub hits: u64,
    /// Which of the 79 channels a hit was found on, one bit per channel.
    pub channels: u128,
    pub first_seen: Instant,
    pub last_seen: Instant,
}

impl Piconet {
    /// How many different channels it has been heard on.
    pub fn channels_hit(&self) -> u32 {
        self.channels.count_ones()
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
                channels: 0,
                first_seen: now,
                last_seen: now,
            });
            roster.last_mut().expect("just pushed")
        }
    };
    p.hits += 1;
    p.last_seen = now;
    if channel < 79 {
        p.channels |= 1 << channel;
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
    use std::time::Duration;

    #[test]
    fn a_lap_gathers_its_hits_and_the_channels_they_were_on() {
        let t0 = Instant::now();
        let mut roster = Vec::new();
        observe(&mut roster, 0x9e8b33, 10, t0);
        observe(&mut roster, 0x9e8b33, 40, t0 + Duration::from_millis(5));
        observe(&mut roster, 0x9e8b33, 10, t0 + Duration::from_millis(9));
        observe(&mut roster, 0x123456, 78, t0 + Duration::from_millis(7));
        assert_eq!(roster.len(), 2);
        let p = &roster[0];
        assert_eq!((p.hits, p.channels_hit()), (3, 2));
        assert_eq!(p.first_seen, t0);
        assert_eq!(p.last_seen, t0 + Duration::from_millis(9));
        assert_eq!(roster[1].channels, 1 << 78);

        // Most recently heard first.
        let order: Vec<u32> = ordered(&roster).iter().map(|p| p.lap).collect();
        assert_eq!(order, vec![0x9e8b33, 0x123456]);
    }
}
