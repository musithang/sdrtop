// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! AuxPtr promises: what an extended advertisement says is coming, kept
//! until the stream reaches its window, listened for there, and counted when
//! it cannot be.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::signal::net::listen::Listener;
use crate::signal::net::measure::Recent;
use crate::state::SdrMetrics;

use super::ble::{census_from_aux, extended, measure_coded, push_coded, push_le_aux};
use super::Tuning;

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

/// Every promise whose window the held stream, ending at `held_end`, now
/// completes, listened to where it promised with a receiver from
/// `listener`: its packet joins its advertisement's list, its own AuxPtr
/// becomes the chain's next promise, and what became of it is counted and
/// marked on the packet that made it.
pub(super) fn listen_due(
    promises: &mut Vec<Pending>,
    listener: &mut Listener,
    window: &Recent,
    held_end: f64,
    tuning: Tuning,
    now: Instant,
    state: &Arc<Mutex<SdrMetrics>>,
) {
    if promises.is_empty() {
        return;
    }
    use crate::signal::ble::aux_ptr::{AuxOutcome, AuxPhy};
    use crate::signal::ble::Phy;
    // Promises whose windows are now held, in the order made.
    let (due, waiting): (Vec<_>, Vec<_>) = promises
        .drain(..)
        .partition(|p| p.promise.to_pair <= held_end);
    *promises = waiting;
    for Pending { promise, list } in due {
        let link = crate::signal::ble::receive::Link::Auxiliary;
        let ear = match promise.phy {
            AuxPhy::Coded => crate::signal::net::listen::Ear::Coded,
            AuxPhy::OneM => crate::signal::net::listen::Ear::Link(link, Phy::OneM),
            AuxPhy::TwoM => crate::signal::net::listen::Ear::Link(link, Phy::TwoM),
        };
        let job = crate::signal::net::listen::Job {
            ears: vec![ear],
            channel: promise.channel,
            from_pair: promise.from_pair,
            to_pair: promise.to_pair,
        };
        let out = listener.listen(
            &job,
            window,
            tuning.rate_hz,
            tuning.centre_hz,
            tuning.span_hz,
        );
        // The promised packet: CRC passing, an extended PDU whose
        // header keeps the promise. Another set's packet in the
        // same window is not listed here: its own promise lists
        // it, and a window it merely fell in would list it twice.
        let kept = out.packets.into_iter().find_map(|p| {
            let phy = match (p.coding, promise.phy) {
                (Some(c), _) => Phy::Coded(c),
                (None, AuxPhy::TwoM) => Phy::TwoM,
                (None, _) => Phy::OneM,
            };
            let role = if promise.depth == 0 {
                crate::state::ExtRole::AuxAdv {
                    superior_seq: Some(promise.superior_seq),
                }
            } else {
                crate::state::ExtRole::AuxChain {
                    superior_seq: promise.superior_seq,
                }
            };
            let ext = extended(&p, role)?;
            crate::signal::ble::aux_ptr::keeps(&promise, phy, &ext.header).then_some((p, phy, ext))
        });
        let outcome = if !out.in_view {
            AuxOutcome::NotInView
        } else if out.feed_lost {
            AuxOutcome::FeedLost
        } else if out.refused.is_some() {
            AuxOutcome::Refused("its PHY cannot be received at this sample rate")
        } else if let Some((mut p, phy, mut ext)) = kept {
            // Read as the tester reads its PHY, outside the lock.
            let reading = match phy {
                Phy::Coded(_) => measure_coded(
                    &p,
                    promise.channel,
                    window,
                    tuning.rate_hz,
                    tuning.centre_hz,
                ),
                Phy::OneM => {
                    let offset = crate::signal::ble::channel::centre_hz(promise.channel)
                        .map(|hz| hz as f64 - tuning.centre_hz);
                    let read = offset
                        .zip(p.pdu_pair)
                        .map(|(o, at)| {
                            crate::signal::net::measure::le_1m(
                                window,
                                tuning.rate_hz,
                                o,
                                at,
                                &p.air,
                            )
                        })
                        .unwrap_or_default();
                    (p.snr_db, p.modulation, p.drift) = (read.snr_db, read.modulation, read.drift);
                    None
                }
                Phy::TwoM => None,
            };
            let start = packet_start(&p, phy, tuning.rate_hz);
            let after_us = start.map_or(0.0, |s| {
                (s - promise.superior_start_pair) / tuning.rate_hz * 1e6
            });
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            let seq = match list {
                AuxList::Le => m.net.ble_heard,
                AuxList::Coded => m.net.coded_heard,
            } + 1;
            // Its own AuxPtr, if any, is the chain's next link.
            ext.aux = keep_promise(
                &mut m,
                promises,
                list,
                start.map(|s| {
                    crate::signal::ble::aux_ptr::promise(
                        seq,
                        s,
                        &ext.header,
                        promise.depth + 1,
                        tuning.rate_hz,
                    )
                }),
            );
            match list {
                AuxList::Le => {
                    census_from_aux(&mut m, &mut p, &ext, promise.channel, tuning.rate_hz, now);
                    push_le_aux(&mut m, p, promise.channel, phy, reading, ext, now)
                }
                AuxList::Coded => push_coded(&mut m, p, promise.channel, reading, Some(ext), now),
            }
            AuxOutcome::Heard { seq, after_us }
        } else {
            AuxOutcome::Missed
        };
        let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
        m.net.health.aux.count(&outcome);
        set_aux_outcome(&mut m, list, promise.superior_seq, outcome);
    }
}
