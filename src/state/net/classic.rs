// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Classic Bluetooth's hops and the scatter that shows them, and the
//! Piconet view's packet list.

/// The hop scatter's zoom steps, in milliseconds: from half a second, where
/// single hops of one piconet separate, to a minute, where piconets come and
/// go.
pub const HOP_WINDOWS_MS: [u64; 7] = [500, 1_000, 2_000, 5_000, 10_000, 20_000, 60_000];

/// The Piconet view's packet list: held or live, and how far down.
///
/// **Held at a packet, not a copy of the list.** The list is newest first
/// and grows at the top, so holding it means drawing from one packet down
/// while newer ones arrive above: the ring already keeps them, so there is
/// nothing to copy, and how many arrived since is simply how far down the
/// held one now sits.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PacketsView {
    /// The newest packet shown while held, by its stream and its time on
    /// that stream (`piconet::BtPacket`); `None` while live.
    pub held: Option<(u32, f64)>,
    /// Rows scrolled past, from the held packet (or the newest, live).
    pub first_visible: usize,
    /// The piconet's LMP log in place of every packet: a mode, like the
    /// hold, kept until switched back, so a scroll or a step to another
    /// piconet does not drop it.
    pub lmp_only: bool,
}

/// What the piconet roster can be ordered by, in the order its columns are
/// drawn: the names are the column titles, so the header's mark, the title's
/// tag and the ordering cannot disagree about which column is which.
pub const ROSTER_SORT_KEYS: &[&str] = &["LAP", "KIND", "LAST", "HITS", "CH", "UAP", "FIRST"];

/// How the piconet roster is ordered: a column of [`ROSTER_SORT_KEYS`],
/// and which way.
///
/// **By LAP until asked otherwise.** Ordered by when each was last heard,
/// the rows changed places every time a piconet spoke, and the one a reader
/// was reaching for moved from under the cursor. A LAP does not change.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RosterSort {
    pub column: usize,
    pub descending: bool,
}

impl RosterSort {
    /// The column's title.
    pub fn key(&self) -> &'static str {
        ROSTER_SORT_KEYS
            .get(self.column)
            .copied()
            .unwrap_or(ROSTER_SORT_KEYS[0])
    }

    /// The next column along, wrapping.
    pub fn cycle(&mut self) {
        self.column = (self.column + 1) % ROSTER_SORT_KEYS.len();
    }

    pub fn reverse(&mut self) {
        self.descending = !self.descending;
    }
}

impl super::NetState {
    /// The classic piconets in the order the roster draws them.
    ///
    /// **The one ordering every reader of the roster takes**: the roster
    /// and its arrows, the hop scatter's lanes, the steps between piconets
    /// on the Piconet and Bench views and the menu's live line. A second
    /// ordering anywhere would have the cursor step through a list nobody
    /// can see.
    ///
    /// **Sorted as the column reads.** Masked, the LAP column is `#n`, the
    /// order first heard, and sorts that way: by the hidden LAP it would
    /// give away how the LAPs compare. Likewise a masked UAP sorts only by
    /// whether it is found and how many candidates are left. A UAP with no
    /// candidates at all, no header heard yet, goes last either way. Ties
    /// fall to the LAP column, so equal rows keep still too.
    pub fn bt_roster(&self) -> Vec<&crate::signal::bt::piconet::Piconet> {
        use crate::signal::bt::piconet::{Kind, Piconet};
        use std::cmp::Ordering;
        let masked = self.address_display == super::AddressDisplay::Masked;
        let place = |p: &Piconet| -> u64 {
            if masked {
                self.bt_piconets
                    .iter()
                    .position(|q| q.lap == p.lap)
                    .unwrap_or(usize::MAX) as u64
            } else {
                u64::from(p.lap)
            }
        };
        let uap = |p: &Piconet| -> Option<(usize, u8)> {
            let candidates = self.bt_uap.get(&p.lap).filter(|u| !u.is_empty())?;
            let value = match candidates.as_slice() {
                [one] if !masked => *one,
                _ => 0,
            };
            Some((candidates.len(), value))
        };
        let kind = |p: &Piconet| match p.kind() {
            Kind::Piconet => 0,
            Kind::Paged => 1,
            Kind::Inquiry(_) => 2,
        };
        let sort = self.bt_sort;
        let mut out: Vec<&Piconet> = self.bt_piconets.iter().collect();
        out.sort_by(|a, b| {
            let directed = |o: Ordering| if sort.descending { o.reverse() } else { o };
            let by = match sort.key() {
                "KIND" => directed(kind(a).cmp(&kind(b))),
                // Ages, so the youngest first, as the column reads.
                "LAST" => directed(b.last_seen.cmp(&a.last_seen)),
                "FIRST" => directed(b.first_seen.cmp(&a.first_seen)),
                "HITS" => directed(a.hits.cmp(&b.hits)),
                "CH" => directed(a.channels_hit().cmp(&b.channels_hit())),
                "UAP" => match (uap(a), uap(b)) {
                    (Some(x), Some(y)) => directed(x.cmp(&y)),
                    (Some(_), None) => Ordering::Less,
                    (None, Some(_)) => Ordering::Greater,
                    (None, None) => Ordering::Equal,
                },
                _ => directed(place(a).cmp(&place(b))),
            };
            by.then_with(|| place(a).cmp(&place(b)))
        });
        out
    }
}

/// Which stretch of time the classic hop scatter shows
/// a zoom step, and how far before now it ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HopView {
    /// Index into [`HOP_WINDOWS_MS`].
    pub zoom: usize,
    /// How far before now the view ends, ms; `0` is live.
    pub back_ms: u64,
}

impl Default for HopView {
    /// Twenty seconds ending now: what the scatter showed before it could
    /// zoom.
    fn default() -> Self {
        Self {
            zoom: 5,
            back_ms: 0,
        }
    }
}

impl HopView {
    pub fn span_ms(&self) -> u64 {
        HOP_WINDOWS_MS[self.zoom.min(HOP_WINDOWS_MS.len() - 1)]
    }

    /// A shorter window, keeping where it ends.
    pub fn zoom_in(&mut self) {
        self.zoom = self.zoom.saturating_sub(1);
    }

    /// A longer window, keeping where it ends.
    pub fn zoom_out(&mut self) {
        self.zoom = (self.zoom + 1).min(HOP_WINDOWS_MS.len() - 1);
    }

    /// Move a quarter of a window back in time, but not past `oldest_ms`,
    /// the age of the oldest hit kept: a view that ends before anything was
    /// kept would show an empty plot of nothing known.
    pub fn back(&mut self, oldest_ms: u64) {
        self.back_ms = (self.back_ms + self.span_ms() / 4).min(oldest_ms);
    }

    /// Move a quarter of a window toward now.
    pub fn forward(&mut self) {
        self.back_ms = self.back_ms.saturating_sub(self.span_ms() / 4);
    }
}

/// How many recent hits [`super::NetState::bt_hops`] keeps - a scatter plots a
/// recent time window, not a session's worth of hops, and old rows fall off
/// the end the same way [`super::BLE_PACKET_LIMIT`] already does for advertising
/// PDUs.
pub const BT_HOP_LIMIT: usize = 500;

/// One classic Bluetooth access-code hit, as [`ui::panels::net::bt_hops::
/// NetBtHopsPanel`] plots it: which channel found it, the LAP the access
/// code carries (free at detection time - see
/// `signal::bt::access_code::find_access_code`'s own doc), and when.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BtHop {
    pub channel: u8,
    /// The piconet it belongs to: what `net_bt_hops` colours it by
    /// free at detection time.
    pub lap: u32,
    pub seen: std::time::Instant,
    /// When its access code ended, µs on the stream's sample clock
    /// (`signal::bt::receive::AccessHit::at_us`), and which stream: a new
    /// stream or rate starts that clock again, so a time is only comparable
    /// to one of the same `stream`.
    pub at_us: f64,
    pub stream: u32,
    /// What its header said, once one was captured and joined to it: the
    /// export's header columns. `None` when no header followed.
    pub header: Option<crate::signal::bt::piconet::HeaderRead>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::bt::piconet::observe;
    use crate::state::{AddressDisplay, NetState};
    use std::time::{Duration, Instant};

    /// Three piconets, first heard in the order 0x30…, 0x10…, 0x20…, the
    /// last one most recently.
    fn three() -> NetState {
        let mut net = NetState::default();
        let t = Instant::now() - Duration::from_secs(30);
        observe(&mut net.bt_piconets, 0x30_0000, 3, t);
        observe(
            &mut net.bt_piconets,
            0x10_0000,
            5,
            t + Duration::from_secs(1),
        );
        observe(
            &mut net.bt_piconets,
            0x20_0000,
            7,
            t + Duration::from_secs(2),
        );
        net
    }

    fn laps(net: &NetState) -> Vec<u32> {
        net.bt_roster().iter().map(|p| p.lap).collect()
    }

    fn column(title: &str) -> usize {
        ROSTER_SORT_KEYS
            .iter()
            .position(|k| *k == title)
            .unwrap_or_else(|| panic!("no column {title}"))
    }

    /// By LAP until asked otherwise, so a piconet heard again stays where
    /// it is rather than jumping to the top under the cursor.
    #[test]
    fn the_roster_stands_still_by_lap_until_asked_otherwise() {
        let mut net = three();
        assert_eq!(laps(&net), [0x10_0000, 0x20_0000, 0x30_0000]);
        observe(&mut net.bt_piconets, 0x30_0000, 3, Instant::now());
        assert_eq!(laps(&net), [0x10_0000, 0x20_0000, 0x30_0000]);
        net.bt_sort.reverse();
        assert_eq!(laps(&net), [0x30_0000, 0x20_0000, 0x10_0000]);
    }

    #[test]
    fn the_sort_walks_the_columns_and_comes_back() {
        let mut s = RosterSort::default();
        for want in ["LAP", "KIND", "LAST", "HITS", "CH", "UAP", "FIRST", "LAP"] {
            assert_eq!(s.key(), want);
            s.cycle();
        }
        s.reverse();
        assert_eq!(
            (s.key(), s.descending),
            ("KIND", true),
            "reversing keeps the column"
        );
    }

    #[test]
    fn last_puts_the_most_recently_heard_first() {
        let mut net = three();
        net.bt_sort.column = column("LAST");
        assert_eq!(laps(&net), [0x20_0000, 0x10_0000, 0x30_0000]);
    }

    /// Masked, the LAP column reads `#n`, the order first heard, and sorts
    /// that way: by the hidden value it would give away how the LAPs
    /// compare.
    #[test]
    fn masked_the_lap_column_sorts_as_it_reads() {
        let mut net = three();
        net.address_display = AddressDisplay::Masked;
        assert_eq!(laps(&net), [0x30_0000, 0x10_0000, 0x20_0000]);
    }

    /// A resolved UAP first, then the fewest candidates; a piconet with no
    /// header yet last, whichever way the column runs.
    #[test]
    fn uap_sorts_resolved_then_fewest_candidates_then_none() {
        let mut net = three();
        net.bt_sort.column = column("UAP");
        net.bt_uap.insert(0x10_0000, vec![1, 2]);
        net.bt_uap.insert(0x30_0000, vec![0x67]);
        assert_eq!(laps(&net), [0x30_0000, 0x10_0000, 0x20_0000]);
        net.bt_sort.reverse();
        assert_eq!(laps(&net), [0x10_0000, 0x30_0000, 0x20_0000]);
    }
}
