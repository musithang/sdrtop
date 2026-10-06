// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! AuxPtr promises: what an extended advertisement says is coming, kept
//! until the stream reaches its window, and counted when it cannot be.

use std::sync::{Arc, Mutex};

use crate::state::SdrMetrics;

/// Which list an advertisement and its auxiliary packets join: the one its
/// primary packet was heard for. One advertisement, one list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AuxList {
    Le,
    Coded,
}

/// A promise waiting for the stream to reach its window, and the list its
/// packet will join.
#[derive(Clone, Copy, Debug)]
pub(super) struct Pending {
    pub(super) promise: crate::signal::ble::aux_ptr::Promise,
    pub(super) list: AuxList,
}

/// Where a packet started, its preamble's first symbol, in stream pairs:
/// what an AuxPtr's offset is counted from. LE Coded's receiver stamps it;
/// on LE 1M and LE 2M it is the PDU's first bit less the preamble and the
/// access address (8 or 16 bits, and 32), and half a bit to its start.
pub(super) fn packet_start(
    p: &crate::signal::ble::pdu::Packet,
    phy: crate::signal::ble::Phy,
    rate_hz: f64,
) -> Option<f64> {
    use crate::signal::ble::Phy;
    let bit = rate_hz / phy.symbol_rate_hz();
    match phy {
        Phy::Coded(_) => p.at_pair.map(|a| a as f64),
        Phy::OneM => p.pdu_pair.map(|c| c - (8.0 + 32.0 + 0.5) * bit),
        Phy::TwoM => p.pdu_pair.map(|c| c - (16.0 + 32.0 + 0.5) * bit),
    }
}

/// What became of an AuxPtr as it was read: `made`, the promise
/// `aux_ptr::promise` made of it (`None` where the packet's start is unknown),
/// waits in `list`; the outcome where it made none is counted.
pub(super) fn keep_promise(
    m: &mut SdrMetrics,
    promises: &mut Vec<Pending>,
    list: AuxList,
    made: Option<
        Result<crate::signal::ble::aux_ptr::Promise, crate::signal::ble::aux_ptr::AuxOutcome>,
    >,
) -> crate::signal::ble::aux_ptr::AuxOutcome {
    use crate::signal::ble::aux_ptr::AuxOutcome;
    match made {
        Some(Ok(promise)) => {
            promises.push(Pending { promise, list });
            AuxOutcome::Pending
        }
        Some(Err(outcome)) => {
            m.net.health.aux.count(&outcome);
            outcome
        }
        None => AuxOutcome::NonePromised,
    }
}

/// What became of the AuxPtr of the packet numbered `seq` in `list`, where
/// it is still there.
pub(super) fn set_aux_outcome(
    m: &mut SdrMetrics,
    list: AuxList,
    seq: u64,
    outcome: crate::signal::ble::aux_ptr::AuxOutcome,
) {
    let packets = match list {
        AuxList::Le => &mut m.net.ble_packets,
        AuxList::Coded => &mut m.net.coded_packets,
    };
    if let Some(e) = packets
        .iter_mut()
        .find(|p| p.seq == seq)
        .and_then(|p| p.ext.as_mut())
    {
        e.aux = outcome;
    }
}

/// Promises that can no longer be kept, those of `list` or every one: each
/// ends as `outcome`, and is counted, rather than vanishing. A break in the
/// stream or a closed view loses their samples; a retune means the samples
/// held are another tuning's.
pub(super) fn abandon(
    promises: &mut Vec<Pending>,
    list: Option<AuxList>,
    outcome: crate::signal::ble::aux_ptr::AuxOutcome,
    state: &Arc<Mutex<SdrMetrics>>,
) {
    if !promises.iter().any(|p| list.is_none_or(|l| l == p.list)) {
        return;
    }
    let (gone, kept): (Vec<_>, Vec<_>) = promises
        .drain(..)
        .partition(|p| list.is_none_or(|l| l == p.list));
    *promises = kept;
    let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
    for p in gone {
        m.net.health.aux.count(&outcome);
        set_aux_outcome(&mut m, p.list, p.promise.superior_seq, outcome);
    }
}
