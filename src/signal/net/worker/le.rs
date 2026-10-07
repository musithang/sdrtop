// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The LE side of the worker: the BLE receiver (LE 1M or LE 2M), LE Coded's
//! own, and what hangs on them from one block to the next, the AuxPtr
//! promises their advertisements made and the receivers a scheduled listen
//! uses. A promise goes with the receiver whose packet made it, which is why
//! the two live together.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::signal::ble::coded_rx::CodedReceiver;
use crate::signal::ble::pdu::Packet;
use crate::signal::ble::receive::Receiver as BleReceiver;
use crate::signal::ble::Phy;
use crate::signal::net::listen::Listener;
use crate::signal::net::measure::Recent;
use crate::state::{BlePacket, SdrMetrics};

use super::ble::{
    advertised_of, census_from_ble, extended, measure_coded, push_coded, put_down, put_down_ble,
};
use super::follow::connect_end_pair;
use super::promises::{abandon, keep_promise, packet_start, AuxList, Pending};
use super::{Tuning, View};

/// The channels this block's LE receivers are fed on, each where it has a
/// receiver: the BLE receiver's with the PHY it runs at, and LE Coded's,
/// which runs in LE 1M's place on the LE Coded view, on the advertising
/// channel LE 1M would have had, so that neither pays for the other and
/// neither list shows the other's packets.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Feeds {
    pub(super) ble: Option<(u8, Phy)>,
    pub(super) coded: Option<u8>,
}

impl Feeds {
    pub(super) fn any(&self) -> bool {
        self.ble.is_some() || self.coded.is_some()
    }
}

/// Everything the LE side carries from one block to the next.
#[derive(Default)]
pub(super) struct Le {
    /// LE 1M or LE 2M, on the channel the tuning gives it.
    ble: Option<BleReceiver>,
    /// LE Coded's own chain, on the LE Coded view alone.
    coded: Option<CodedReceiver>,
    /// AuxPtr promises waiting for the stream to reach their windows.
    promises: Vec<Pending>,
    /// The receivers a scheduled listen uses (a followed connection's
    /// event, per connection, channel and PHY, and an AuxPtr's window), each
    /// reset before its window: built once, since a matched filter and its
    /// reference are the receiver's cost.
    listener: Listener,
}

impl Le {
    /// A break in the stream: no receiver is carried across it, and a
    /// promise whose window falls in what was lost cannot be kept.
    pub(super) fn interrupted(&mut self, state: &Arc<Mutex<SdrMetrics>>) {
        put_down_ble(&mut self.ble, state);
        put_down(&mut self.coded, state);
        abandon(
            &mut self.promises,
            None,
            crate::signal::ble::aux_ptr::AuxOutcome::FeedLost,
            state,
        );
    }

    /// The radio retuned: the samples held are another tuning's, so no
    /// promise can be listened for in them.
    pub(super) fn retuned(&mut self, state: &Arc<Mutex<SdrMetrics>>) {
        abandon(
            &mut self.promises,
            None,
            crate::signal::ble::aux_ptr::AuxOutcome::Refused("the radio retuned before its window"),
            state,
        );
    }

    /// The receivers this block is fed to, built or rebuilt for the tuning,
    /// the view and the PHY the user chose, or put down with the reason
    /// published where none can run. A receiver put down takes its
    /// advertisements' promises with it.
    ///
    /// BLE decode is only possible on one of the three fixed advertising
    /// frequencies, and only at a sample rate `receive::front_end` can reach
    /// the working rate from. Neither condition is `net_survey`'s to share,
    /// so this keeps its own refusal rather than reusing `survey_refused`.
    pub(super) fn choose(
        &mut self,
        tuning: Tuning,
        view: View,
        state: &Arc<Mutex<SdrMetrics>>,
    ) -> Feeds {
        // Surveying, the advertising channel in view; locked, the
        // tuning's own (`channel::to_decode`). LE 2M is never sent on the
        // advertising channels, so for it the tuning's own channel it is.
        let channel = if view.phy == crate::signal::ble::Phy::TwoM {
            crate::signal::ble::channel::channel_of(tuning.centre_hz as u64)
        } else {
            crate::signal::ble::channel::to_decode(
                tuning.centre_hz as u64,
                tuning.span_hz,
                view.locked,
            )
        };
        let mut feeds = Feeds::default();
        if view.coded && view.open {
            put_down_ble(&mut self.ble, state);
            let advertising = crate::signal::ble::channel::to_decode(
                tuning.centre_hz as u64,
                tuning.span_hz,
                view.locked,
            )
            .filter(|&ch| crate::signal::ble::channel::advertising_channel_index(ch).is_some());
            match advertising {
                Some(ch) => {
                    if !self
                        .coded
                        .as_ref()
                        .is_some_and(|r| r.matches(ch, tuning.rate_hz, tuning.centre_hz))
                    {
                        put_down(&mut self.coded, state);
                        let built = crate::signal::ble::coded_rx::CodedReceiver::new(
                            tuning.rate_hz,
                            ch,
                            tuning.centre_hz,
                        );
                        let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
                        match built {
                            Ok(r) => {
                                m.net.coded_refused = None;
                                m.net.coded_channel = Some(ch);
                                self.coded = Some(r);
                            }
                            Err(reason) => {
                                m.net.coded_refused = Some(reason);
                                m.net.coded_channel = None;
                            }
                        }
                    }
                    if self.coded.is_some() {
                        feeds.coded = Some(ch);
                    }
                }
                None => {
                    put_down(&mut self.coded, state);
                    let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
                    m.net.coded_refused = Some(
                        "not tuned to an advertising channel (2402, 2426 or 2480 MHz)".to_string(),
                    );
                    m.net.coded_channel = None;
                }
            }
        } else {
            put_down(&mut self.coded, state);
        }
        if self.coded.is_none() {
            abandon(
                &mut self.promises,
                Some(AuxList::Coded),
                crate::signal::ble::aux_ptr::AuxOutcome::FeedLost,
                state,
            );
        }
        match channel.filter(|_| !view.coded) {
            Some(ch)
                if view.open
                    && view.phy == crate::signal::ble::Phy::TwoM
                    && crate::signal::ble::channel::advertising_channel_index(ch).is_some() =>
            {
                // The primary advertising channels carry LE 1M and LE Coded
                // only (legacy advertising is always LE 1M, and extended
                // advertising's primary channel is 1M or Coded), so a 2M
                // decoder here would listen to nothing and an empty list would
                // read as a quiet room. Said, and not run.
                put_down_ble(&mut self.ble, state);
                let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
                m.net.ble_refused = Some(format!(
                    "LE 2M is not used on the primary advertising channels (ch {ch} is one); \
                     tune to a data or secondary channel, or switch back to LE 1M"
                ));
                m.net.ble_channel = None;
            }
            Some(ch) if view.open => {
                // The PHY the user chose (`NetState::ble_phy`): a switch
                // rebuilds the receiver, like a retune does.
                if !self
                    .ble
                    .as_ref()
                    .is_some_and(|r| r.matches(ch, tuning.rate_hz, view.phy, tuning.centre_hz))
                {
                    put_down_ble(&mut self.ble, state);
                    self.ble =
                        match BleReceiver::new(tuning.rate_hz, ch, view.phy, tuning.centre_hz) {
                            Ok(r) => {
                                let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
                                m.net.ble_refused = None;
                                m.net.ble_channel = Some(ch);
                                Some(r)
                            }
                            Err(reason) => {
                                let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
                                m.net.ble_refused = Some(reason);
                                m.net.ble_channel = None;
                                None
                            }
                        };
                }
                if self.ble.is_some() {
                    feeds.ble = Some((ch, view.phy));
                }
            }
            Some(_) => {
                // Section closed; nothing decodes while it is.
                put_down_ble(&mut self.ble, state);
            }
            None => {
                put_down_ble(&mut self.ble, state);
                let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
                m.net.ble_refused = Some(
                    "not tuned to an advertising channel (2402, 2426 or 2480 MHz)".to_string(),
                );
                m.net.ble_channel = None;
            }
        }

        // An LE advertisement's promises go with the receiver that heard it; LE
        // Coded's with its own, above.
        if self.ble.is_none() {
            abandon(
                &mut self.promises,
                Some(AuxList::Le),
                crate::signal::ble::aux_ptr::AuxOutcome::FeedLost,
                state,
            );
        }
        feeds
    }

    /// The BLE receiver, to run beside the classic fleet, where it is fed.
    pub(super) fn beside(&mut self, feeds: Feeds) -> Option<&mut BleReceiver> {
        feeds.ble.and(self.ble.as_mut())
    }

    /// The BLE receiver run alone on `block`, where it is fed.
    pub(super) fn push_ble(
        &mut self,
        feeds: Feeds,
        block: &[num_complex::Complex<f32>],
        first_pair: u64,
    ) -> Option<Vec<Packet>> {
        let (Some(_), Some(rx)) = (feeds.ble, self.ble.as_mut()) else {
            return None;
        };
        Some(rx.push_iq_at(block, first_pair))
    }

    /// What the BLE receiver found in this block: read again as the tester
    /// reads it, then into the funnel, the connections to follow, what was
    /// advertised, the census and the list, an `ADV_EXT_IND`'s AuxPtr kept
    /// as a promise.
    pub(super) fn ble_packets(
        &mut self,
        feeds: Feeds,
        packets: Option<Vec<Packet>>,
        window: &Recent,
        tuning: Tuning,
        now: Instant,
        state: &Arc<Mutex<SdrMetrics>>,
    ) {
        let (Some((ch, phy)), Some(mut packets), Some(rx)) =
            (feeds.ble, packets, self.ble.as_mut())
        else {
            return;
        };
        // LE 1M read again as a tester reads it (`measure::le_1m`), outside the
        // lock; a packet whose window is not held keeps no figure rather than
        // the receiver's own. LE 2M keeps the receiver's.
        if phy == crate::signal::ble::Phy::OneM && !packets.is_empty() {
            let offset =
                crate::signal::ble::channel::centre_hz(ch).map(|hz| hz as f64 - tuning.centre_hz);
            for p in packets.iter_mut() {
                let read = offset.zip(p.pdu_pair).and_then(|(o, at)| {
                    crate::signal::net::measure::le_1m(window, tuning.rate_hz, o, at, &p.air)
                });
                (p.modulation, p.drift) = read.unwrap_or((None, None));
            }
        }
        let funnel = rx.take_funnel();
        if !funnel.is_empty() {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            m.net.health.ble.add(funnel);
        }
        if !packets.is_empty() {
            // Read before the lock: parsing is work the UI thread should not
            // wait behind.
            let advertised: Vec<_> = packets.iter().filter_map(advertised_of).collect();
            // Each connection a passing CONNECT_IND set up, and where
            // its packet ended: the origin of its transmit window.
            // With the ChSel of the advertising PDU each answered, as
            // far as this block heard it: the algorithm needs both.
            use crate::signal::ble::pdu::PduType;
            let connects: Vec<_> = packets
                .iter()
                .enumerate()
                .filter(|(_, p)| p.crc_ok && p.pdu_type == PduType::ConnectInd)
                .filter_map(|(i, p)| {
                    let c = crate::signal::ble::connect::decode_octets(&p.payload)?;
                    let answered = packets[..i]
                        .iter()
                        .rev()
                        .find(|q| crate::signal::ble::follow::answers(q.pdu_type, q.adv_addr, &c))
                        .map(|q| q.ch_sel);
                    let flags = (p.ch_sel, p.tx_add_random, p.rx_add_random);
                    Some((c, flags, answered, connect_end_pair(p, tuning.rate_hz)?))
                })
                .collect();
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            for (c, (ch_sel, init_random, adv_random), answered, end) in &connects {
                // Not in this block: the newest kept, from the ring.
                let answered = answered.or_else(|| {
                    m.net
                        .ble_packets
                        .iter()
                        .find(|q| crate::signal::ble::follow::answers(q.pdu_type, q.adv_addr, c))
                        .map(|q| q.ch_sel)
                });
                let csa2 = crate::signal::ble::follow::uses_csa2(*ch_sel, answered);
                m.net.follow(
                    c,
                    (csa2, *init_random, *adv_random),
                    *end,
                    tuning.rate_hz,
                    now,
                );
            }
            for (address, said) in advertised {
                m.net.advertised.entry(address).or_default().merge(said);
            }
            if let Some(i) = crate::signal::ble::channel::advertising_channel_index(ch) {
                m.net.ble_channel_packets[i] += packets.len() as u64;
                m.net.ble_channel_crc_ok[i] += packets.iter().filter(|p| p.crc_ok).count() as u64;
            }
            for p in packets {
                // Numbered as it arrives, so `masked` counts in
                // the order devices were heard: see `AddressBook`.
                if let Some(addr) = p.adv_addr {
                    m.net.address_book.number(addr);
                }
                let locked = m.net.mode == crate::state::NetMode::Lock;
                if let Some(snr) = p.snr_db {
                    m.net.fer.record(snr, p.crc_ok);
                }
                census_from_ble(
                    &mut m.net.census.devices,
                    &p,
                    ch,
                    locked,
                    tuning.rate_hz,
                    now,
                );
                m.net.ble_heard += 1;
                let seq = m.net.ble_heard;
                // On a primary channel, a type 7 is an ADV_EXT_IND,
                // and its AuxPtr is followed as LE Coded's is.
                let ext = crate::signal::ble::channel::advertising_channel_index(ch)
                    .and_then(|_| extended(&p, crate::state::ExtRole::AdvExt))
                    .map(|mut e| {
                        let made = packet_start(&p, phy, tuning.rate_hz).map(|s| {
                            crate::signal::ble::aux_ptr::promise(
                                seq,
                                s,
                                &e.header,
                                0,
                                tuning.rate_hz,
                            )
                        });
                        e.aux = keep_promise(&mut m, &mut self.promises, AuxList::Le, made);
                        e
                    });
                let adv_addr = p
                    .adv_addr
                    .or_else(|| ext.as_ref().and_then(|e| e.header.adv_a));
                m.net.ble_packets.push_front(BlePacket {
                    seq,
                    phy,
                    channel: ch,
                    pdu_type: p.pdu_type,
                    ch_sel: p.ch_sel,
                    tx_add_random: p.tx_add_random,
                    rx_add_random: p.rx_add_random,
                    length: p.length,
                    adv_addr,
                    payload: p.payload,
                    crc_ok: p.crc_ok,
                    snr_db: p.snr_db,
                    freq_offset_hz: p.freq_offset_hz,
                    modulation: p.modulation,
                    drift: p.drift,
                    seen: now,
                    coded: None,
                    ext,
                });
            }
            m.net.trim_ble_packets();
        }
    }

    /// LE Coded's packets in `this` block, measured as the test suite
    /// defines them (`measure::le_coded`) outside the lock, from the symbols
    /// their decoded bits were sent as, an `ADV_EXT_IND`'s AuxPtr kept as a
    /// promise.
    pub(super) fn coded_packets(
        &mut self,
        feeds: Feeds,
        this: &[num_complex::Complex<f32>],
        window: &Recent,
        tuning: Tuning,
        now: Instant,
        state: &Arc<Mutex<SdrMetrics>>,
    ) {
        let (Some(ch), Some(rx)) = (feeds.coded, self.coded.as_mut()) else {
            return;
        };
        let primary: Vec<_> = rx
            .push_iq_at(this, tuning.first_pair)
            .into_iter()
            .map(|p| {
                let reading = measure_coded(&p, ch, window, tuning.rate_hz, tuning.centre_hz);
                (p, reading)
            })
            .collect();
        let funnel = rx.take_funnel();
        {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            m.net.health.coded.add(funnel);
            for (p, reading) in primary {
                let start = p.at_pair.map(|a| a as f64);
                let ext = extended(&p, crate::state::ExtRole::AdvExt);
                let seq = m.net.coded_heard + 1;
                let ext = ext.map(|mut e| {
                    e.aux = keep_promise(
                        &mut m,
                        &mut self.promises,
                        AuxList::Coded,
                        start.map(|s| {
                            crate::signal::ble::aux_ptr::promise(
                                seq,
                                s,
                                &e.header,
                                0,
                                tuning.rate_hz,
                            )
                        }),
                    );
                    e
                });
                push_coded(&mut m, p, ch, reading, ext, now);
            }
        }
    }

    /// Each followed connection's events the held stream, ending at
    /// `held_end`, now completes: see [`super::follow::events`].
    pub(super) fn follow_events(
        &mut self,
        window: &Recent,
        held_end: f64,
        tuning: Tuning,
        state: &Arc<Mutex<SdrMetrics>>,
    ) {
        super::follow::events(&mut self.listener, window, held_end, tuning, state);
    }

    /// Every promise whose window the held stream, ending at `held_end`,
    /// now completes: see [`super::promises::listen_due`].
    pub(super) fn keep_promises(
        &mut self,
        window: &Recent,
        held_end: f64,
        tuning: Tuning,
        now: Instant,
        state: &Arc<Mutex<SdrMetrics>>,
    ) {
        super::promises::listen_due(
            &mut self.promises,
            &mut self.listener,
            window,
            held_end,
            tuning,
            now,
            state,
        );
    }

    /// Promises are waiting: the stream must be held long enough for their
    /// windows.
    pub(super) fn waiting(&self) -> bool {
        !self.promises.is_empty()
    }

    /// The section closed: both receivers go, a capture under way ending in
    /// the funnel.
    pub(super) fn close(&mut self, state: &Arc<Mutex<SdrMetrics>>) {
        put_down_ble(&mut self.ble, state);
        put_down(&mut self.coded, state);
    }
}
