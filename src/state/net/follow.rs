// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The connections a CONNECT_IND set up, followed event by event.

use super::NetState;

/// How many connections [`NetState::ble_connections`] keeps: enough for a
/// room's phones and their watches, few enough that each is worth a look.
pub const CONNECTIONS_KEPT: usize = 16;

/// One connection followed, when its CONNECT_IND was heard, and whether
/// its two addresses are random (the packet's TxAdd for InitA, RxAdd for
/// AdvA).
#[derive(Clone, Debug)]
pub struct FollowedConnection {
    pub seen: std::time::Instant,
    pub init_random: bool,
    pub adv_random: bool,
    pub connection: crate::signal::ble::follow::Connection,
}

/// How the Connection view is being read: which connection, by its access
/// address (`None`: the newest), and how far its rows are scrolled.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ConnectionView {
    pub selected: Option<u32>,
    pub first_visible: usize,
}

impl NetState {
    /// Start following the connection a CONNECT_IND set up, unless one with
    /// its access address is followed already: `true` when it is new. Past
    /// [`CONNECTIONS_KEPT`] the oldest ended one is let go, or the oldest
    /// when none has ended.
    pub fn follow(
        &mut self,
        c: &crate::signal::ble::connect::ConnectIndData,
        (csa2, init_random, adv_random): (Option<bool>, bool, bool),
        end_pair: f64,
        raw_rate: f64,
        now: std::time::Instant,
    ) -> bool {
        use crate::signal::ble::follow::{Connection, State};
        if self
            .ble_connections
            .iter()
            .any(|f| f.connection.access_address() == c.access_address)
        {
            return false;
        }
        let Some(mut connection) = Connection::new(c, csa2.unwrap_or(false), end_pair, raw_rate)
        else {
            return false;
        };
        if csa2.is_none() {
            connection.refuse(
                "the advertising PDU it answered was not heard: which channel selection algorithm is not known",
            );
        }
        self.ble_connections.insert(
            0,
            FollowedConnection {
                seen: now,
                init_random,
                adv_random,
                connection,
            },
        );
        if self.ble_connections.len() > CONNECTIONS_KEPT {
            let ended = self
                .ble_connections
                .iter()
                .rposition(|f| *f.connection.state() != State::Following);
            let at = ended.unwrap_or(self.ble_connections.len() - 1);
            self.ble_connections.remove(at);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connect_ind(aa: u32) -> crate::signal::ble::connect::ConnectIndData {
        crate::signal::ble::connect::ConnectIndData {
            init_a: [0; 6],
            adv_a: [0; 6],
            access_address: aa,
            crc_init: 0x12_3456,
            win_size: 1,
            win_offset: 0,
            interval: 6,
            latency: 0,
            timeout: 100,
            channel_map: (1u64 << 37) - 1,
            hop_increment: 7,
            sca: 0,
        }
    }

    /// One connection an access address; the newest first; at most
    /// [`CONNECTIONS_KEPT`], the ended ones let go before the followed.
    #[test]
    fn connections_are_followed_once_each_and_kept_to_a_number() {
        let now = std::time::Instant::now();
        let mut net = NetState::default();
        assert!(net.follow(&connect_ind(1), (Some(false), false, false), 0.0, 20e6, now));
        assert!(
            !net.follow(&connect_ind(1), (Some(false), false, false), 9.0, 20e6, now),
            "heard twice"
        );
        assert_eq!(net.ble_connections.len(), 1);
        // The first one ends: LL_TERMINATE_IND at its first anchor.
        let mut c = net.ble_connections[0].connection.clone();
        let at = c.next_due().anchor_pair;
        let terminate = crate::signal::ble::data::DataPdu {
            llid: 3,
            nesn: false,
            sn: false,
            md: false,
            cte_info: None,
            payload: vec![0x02, 0x13],
            crc_ok: true,
        };
        let timing = crate::signal::ble::receive::DataTiming {
            start_pair: at,
            end_pair: at + 1_000.0,
        };
        c.account(
            crate::signal::ble::follow::Listened::Yes,
            vec![(terminate, timing)],
        );
        net.ble_connections[0].connection = c;
        for aa in 2..=(CONNECTIONS_KEPT as u32 + 1) {
            assert!(net.follow(
                &connect_ind(aa),
                (Some(false), false, false),
                0.0,
                20e6,
                now
            ));
        }
        assert_eq!(net.ble_connections.len(), CONNECTIONS_KEPT);
        assert_eq!(
            net.ble_connections[0].connection.access_address(),
            CONNECTIONS_KEPT as u32 + 1
        );
        assert!(net
            .ble_connections
            .iter()
            .all(|c| c.connection.access_address() != 1));
    }
}
