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
