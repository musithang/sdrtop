// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! A followed connection's timing: how much stream an event needs and where
//! its window lies, and listening to the events the held stream completes.

use std::sync::{Arc, Mutex};

use crate::signal::net::listen::Listener;
use crate::signal::net::measure::Recent;
use crate::state::SdrMetrics;

use super::Tuning;

/// How much of the stream is held while a connection is followed, s: its
/// first event's transmit window (up to 10 ms, Core 5.4 Vol 6 Part B 4.5.3),
/// the widening either side, and an event's length after.
pub(super) const FOLLOW_HELD_S: f64 = 0.02;

/// Listened to before an event's earliest start, us: the receiver's filters
/// and its detector settle on it, and the preamble's lead is in it.
const FOLLOW_WARMUP_US: f64 = 64.0;

/// The longest an event is listened to after its anchor, us: a Central's
/// longest LE 1M packet (2120 us), T_IFS and an answer as long. A longer
/// event's later packets are not heard; its first exchange is.
const EVENT_SPAN_US: f64 = 4_500.0;

/// At most this many events accounted a block per connection: catching up
/// after a gap, the rest wait for the next block rather than stall this one.
pub(super) const FOLLOW_ROUNDS: usize = 32;

/// The stretch of stream an event is listened to over, in pairs: from its
/// earliest start less the warm-up to its latest anchor plus an event's
/// length, never into the next event.
pub(super) fn event_window(
    due: &crate::signal::ble::follow::Due,
    interval_pairs: f64,
    rate_hz: f64,
) -> (f64, f64) {
    let us = |x: f64| x * 1e-6 * rate_hz;
    let from = due.anchor_pair - due.widening_pairs - us(FOLLOW_WARMUP_US);
    let span = us(EVENT_SPAN_US).min(interval_pairs - us(150.0));
    let to = due.anchor_pair + due.window_pairs + due.widening_pairs + span;
    (from, to)
}

/// Where a CONNECT_IND's packet ended, pairs: the end of its CRC, from where
/// its PDU's first bit was centred and how many bits the PDU is, on LE 1M.
pub(super) fn connect_end_pair(p: &crate::signal::ble::pdu::Packet, rate_hz: f64) -> Option<f64> {
    let bit = rate_hz / 1e6;
    p.pdu_pair
        .map(|centre| centre + (crate::signal::ble::pdu::used_bits(p.length) as f64 - 0.5) * bit)
}

/// Each followed connection's events whose windows the held stream, ending
/// at `held_end`, now completes: demodulated outside the lock with the
/// link's own receiver from `listener`, accounted for inside it. A round
/// takes one event a connection, so an event's outcome is in its timing
/// before the next one is placed. The receivers of connections no longer
/// followed go.
pub(super) fn events(
    listener: &mut Listener,
    window: &Recent,
    held_end: f64,
    tuning: Tuning,
    state: &Arc<Mutex<SdrMetrics>>,
) {
    for _ in 0..FOLLOW_ROUNDS {
        let jobs: Vec<_> = {
            let m = state.lock().unwrap_or_else(|e| e.into_inner());
            m.net
                .ble_connections
                .iter()
                .map(|f| &f.connection)
                .filter(|c| *c.state() == crate::signal::ble::follow::State::Following)
                .map(|c| {
                    let interval = c.params().interval as f64 * 1.25e-3 * tuning.rate_hz;
                    (
                        c.access_address(),
                        c.crc_init(),
                        c.next_due(),
                        interval,
                        c.phy(),
                        c.phy_peripheral(),
                    )
                })
                .collect()
        };
        let mut done = Vec::new();
        for (aa, crc_init, due, interval, phy_c, phy_p) in jobs {
            let (from, to) = event_window(&due, interval, tuning.rate_hz);
            if to > held_end {
                continue;
            }
            let link = crate::signal::ble::receive::Link::Data {
                access_address: aa,
                crc_init,
            };
            let mut ears = vec![crate::signal::net::listen::Ear::Link(link, phy_c)];
            if phy_p != phy_c {
                ears.push(crate::signal::net::listen::Ear::Link(link, phy_p));
            }
            let job = crate::signal::net::listen::Job {
                ears,
                channel: due.channel,
                from_pair: from,
                to_pair: to,
            };
            let heard = listener.listen(
                &job,
                window,
                tuning.rate_hz,
                tuning.centre_hz,
                tuning.span_hz,
            );
            use crate::signal::ble::follow::Listened;
            let listened = if !heard.in_view {
                Listened::NotInView
            } else if heard.feed_lost {
                Listened::FeedLost
            } else if heard.refused.is_some() {
                Listened::CannotReceive
            } else {
                Listened::Yes
            };
            done.push((aa, due.counter, listened, heard.data));
        }
        if done.is_empty() {
            break;
        }
        let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
        for (aa, counter, listened, heard) in done {
            if let Some(f) = m
                .net
                .ble_connections
                .iter_mut()
                .find(|f| f.connection.access_address() == aa)
            {
                // Still the event this was for.
                if f.connection.next_due().counter == counter {
                    f.connection.account(listened, heard);
                }
            }
        }
    }
    // Receivers of connections no longer followed go.
    let m = state.lock().unwrap_or_else(|e| e.into_inner());
    let alive: std::collections::HashSet<u32> = m
        .net
        .ble_connections
        .iter()
        .filter(|f| *f.connection.state() == crate::signal::ble::follow::State::Following)
        .map(|f| f.connection.access_address())
        .collect();
    drop(m);
    listener.retain(|ear| match ear {
        crate::signal::net::listen::Ear::Link(
            crate::signal::ble::receive::Link::Data { access_address, .. },
            _,
        ) => alive.contains(access_address),
        _ => true,
    });
}
