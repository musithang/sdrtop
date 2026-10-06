// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! A followed connection's timing: how much stream an event needs and where
//! its window lies.

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
