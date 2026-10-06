// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The 2.4 GHz worker: one thread, one block at a time.
//!
//! Same shape as [`crate::signal::DemodWorker`], for the same reason: a thread
//! that owns the state carried between blocks, so that everything below it can
//! be a pure function of its arguments and be tested with no radio anywhere.
//!
//! **One thread owns it; the decoders of one block run side by side.** The
//! BLE receiver and each classic channel's receiver own everything they
//! touch and read the same block, so [`classic::push_all`] runs them on scoped
//! threads for that block and hands their answers back in the order one
//! thread would have produced them: the same results, in less time on a
//! machine with more than one core.
//!
//! **It counts what arrived and measures what was in the band.** What the
//! receiver missed is a first-class displayed number rather than an
//! inference, and a feed whose losses are only visible once
//! there is something to lose is a feed nobody will trust when the losses
//! matter - so the counting was built and shown before any measurement sat on
//! top of it. The band measurement is [`super::scan`] over
//! [`super::occupancy`].
//!
//! **The decoders run here**, rather than each in its own task, because they
//! need the same per-block bytes the occupancy scan already has: a second
//! worker reading the same channel would need its own copy of the geometry
//! and the retune detection this one carries.
//!
//! - **BLE**, on the advertising channel in view: its packets, the census,
//!   the connections a CONNECT_IND sets up (followed event by event), and
//!   the auxiliary packets an `ADV_EXT_IND`'s AuxPtr promises.
//! - **LE Coded**, on its own view, in LE 1M's place, with its AuxPtrs
//!   followed the same way.
//! - **Classic Bluetooth**: one `signal::bt::receive::Receiver` per channel
//!   `signal::bt::channel::channels_in_span` and the configured
//!   [`SAFE_BT_CHANNELS`]-guarded cap together let it watch, closest to the
//!   tuned centre first, rebuilt whenever the tuning or the wanted channel
//!   list changes. One `signal::bt::header::PiconetClock` per LAP narrows
//!   each piconet's UAP as far as a header alone can (two candidates, not
//!   one: `PiconetClock`'s doc has why), and `signal::bt::payload::
//!   break_uap_tie` breaks the tie from a captured payload.
//!   [`classic::Classic`] remembers a LAP resolved this way for the rest of
//!   the session: a piconet's real UAP does not change, so a later header whose
//!   payload cannot be read (a POLL or an FHS, say) must not undo an answer
//!   already earned.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use crossbeam_channel::Receiver as SampleReceiver;

use crate::hardware::{SampleGeometry, StreamBlock};
use crate::signal::ble::receive::Receiver as BleReceiver;
use crate::signal::stream::plan_block;
use crate::state::{BlePacket, SdrMetrics};

// The views the classic receiver runs for (the Classic and the Piconet
// view): a plain string comparison, because that is what the menu is keyed
// by, and the registry's structural tests hold the strings and the preset
// files' names to agreeing. A view left out would show an empty list forever.
use super::lock::CLASSIC_VIEWS;
use super::scan::Scan;

mod ble;
mod classic;
mod feed;
mod follow;
mod promises;

use ble::{
    advertised_of, census_from_aux, census_from_ble, extended, measure_coded, push_coded,
    push_le_aux, put_down, put_down_ble,
};
use classic::{push_all, Classic};
use feed::{decoded_block, held, Load, Run};
use follow::{connect_end_pair, event_window, FOLLOW_HELD_S, FOLLOW_ROUNDS};
use promises::{abandon, keep_promise, packet_start, set_aux_outcome, AuxList, Pending};

/// Above this many simultaneous classic BT channels, `NetWorker::new` logs a
/// warning naming the cost rather than staying quiet about it -
/// `signal::bt::receive`'s own doc has the measured tap counts this is
/// guarding against. Matches `config::default_bt_channels`, so a default
/// config never warns; only a config that deliberately asks for more does.
pub const SAFE_BT_CHANNELS: usize = 8;

/// How much *observation* a dwell is, before it is published and started again.
///
/// **Not a wall-clock interval, which is what this was first written as.** The
/// feed is lossy and the section can be closed and reopened, so wall time and
/// time spent looking at the band are different quantities, and a duty cycle is
/// a fraction of the second one. Counting windows means a dwell interrupted by
/// dropped blocks is a shorter dwell rather than a diluted one - and it means
/// the measurement can be tested without a clock, which is how the end-to-end
/// test below exists at all.
///
/// Fifty milliseconds is about eight thousand windows. The duty cycle that
/// supports is good to a quarter of a percent, which is finer than the whole
/// percent it is shown to and not by much - see
/// [`super::occupancy::DUTY_RESOLUTION`], where the two were made to agree. It
/// publishes at most twenty times a second against a screen that redraws thirty.
const DWELL_S: f64 = 0.05;

/// Where one block sits in the stream and what it was captured at: what
/// every receiver is built for and every window is cut against.
#[derive(Clone, Copy, Debug)]
struct Tuning {
    first_pair: u64,
    centre_hz: f64,
    rate_hz: f64,
    /// The usable span: the baseband filter's where the radio has one.
    span_hz: f64,
}

pub struct NetWorker {
    pub sample_rx: SampleReceiver<StreamBlock>,
    pub state: Arc<Mutex<SdrMetrics>>,
    pub geometry: SampleGeometry,
    /// How many classic BT channels to give a live receiver at once - see
    /// [`SAFE_BT_CHANNELS`] and `signal::bt::receive`'s own doc for why this
    /// is capped rather than left to follow the full view.
    pub bt_channels: usize,
    /// How often one piconet's slot grid may be refitted
    /// (`signal::bt::slots`): the rate search is real work, and a jitter
    /// figure does not need refreshing faster. A test sets it to zero.
    pub slot_fit_every: std::time::Duration,
    /// How many classic channels the survey starts with, before its first
    /// load reading: none, because room for a receiver is measured, not
    /// assumed. A test sets it, since the reading follows the clock.
    pub survey_bt_start: usize,
}

impl NetWorker {
    pub fn new(
        sample_rx: SampleReceiver<StreamBlock>,
        state: Arc<Mutex<SdrMetrics>>,
        geometry: SampleGeometry,
        bt_channels: usize,
    ) -> Self {
        if bt_channels > SAFE_BT_CHANNELS {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            m.push_log(format!(
                "NET: [net].bt_channels = {bt_channels} asks for real capacity - \
                 classic Bluetooth's own 1 MHz channel spacing makes each watched \
                 channel expensive (see signal::bt::receive's own doc); the \
                 default of {SAFE_BT_CHANNELS} is the reasoned-safe figure"
            ));
        }
        Self {
            slot_fit_every: std::time::Duration::from_secs(1),
            survey_bt_start: 0,
            sample_rx,
            state,
            geometry,
            bt_channels,
        }
    }

    pub fn run(self) {
        let mut run = Run::default();
        let pair_bytes = self.geometry.bytes_per_pair() as u64;
        let mut scan: Option<Scan> = None;
        let mut ble: Option<BleReceiver> = None;
        // LE Coded's own chain, on the LE Coded view alone.
        let mut coded: Option<crate::signal::ble::coded_rx::CodedReceiver> = None;
        let mut classic = Classic::new(self.survey_bt_start);
        let mut load = Load::default();
        // Where the next block must start for the stream to be unbroken. `None`
        // until a block has been seen, and again after the section closes.
        let mut next_pair: Option<u64> = None;
        // The last few decoded blocks, with their stream positions: what the
        // measurement path cuts a burst's raw samples from
        // (`measure::Recent`), moved here as each block finishes rather than
        // copied, and dropped at any break.
        let mut recent: std::collections::VecDeque<(u64, Vec<num_complex::Complex<f32>>)> =
            std::collections::VecDeque::new();
        // The receivers a scheduled listen uses (a followed connection's event,
        // per connection, channel and PHY), each reset before its window:
        // built once, since a matched filter and its reference are the
        // receiver's cost.
        let mut listener = super::listen::Listener::default();
        // AuxPtr promises waiting for the stream to reach their windows.
        let mut promises: Vec<Pending> = Vec::new();
        // The tuning and rate of the samples held in `recent`: a retune
        // keeps the stream's positions running, so only this says the held
        // samples are another tuning's.
        let mut held_tuning: Option<(f64, f64)> = None;

        while let Ok(StreamBlock {
            seq,
            gap_before,
            bytes,
            first_pair,
            centre_hz,
            rate_hz,
        }) = self.sample_rx.recv()
        {
            let started = run.drop_ref.is_some();
            let plan = plan_block(seq, gap_before, run.last_seq, run.drop_ref);
            // The clock is read outside the lock, because the lock block below
            // does integer work only and a float or a syscall inside one is a
            // dropped frame on the UI thread.
            let now = Instant::now();
            run.last_seq = seq;
            run.drop_ref = Some(seq);

            let pairs = bytes.len() as u64 / pair_bytes.max(1);
            // **A run has to have started before it can be interrupted.**
            // `plan_block` guards its `dropped` count with `drop_ref` and does
            // not guard `contiguous` with anything, because for the demod the
            // difference is invisible: a first block declared discontiguous just
            // resets session state that is already empty. Here the same flag is
            // about to become a number on a panel, and the section is normally
            // opened on a radio that has been streaming for a minute - so the
            // first block through carries a sequence number thousands past
            // whatever this worker last saw, and would report an interruption
            // that never happened, once per visit.
            let broke = started && !plan.contiguous;
            run.blocks = if broke || !started { 1 } else { run.blocks + 1 };

            // **No receiver is ever carried across a break in the samples.** A
            // decimator's filter state, a capture half-filled with the start of
            // a packet, a classic receiver's symbol count - all of them assume
            // the next sample follows the last one. Across a refused block, a
            // driver drop or a restarted stream it does not, and carrying on
            // joins two moments milliseconds apart into one signal that never
            // existed. The position the block carries says whether it follows;
            // when it does not, the receivers start again from this block.
            let continuous = next_pair == Some(first_pair);
            // A position *behind* the expected one is a new stream (RX was
            // restarted, `RxContext::begin_stream`), whose clock starts again.
            if next_pair.is_some_and(|n| first_pair < n) {
                classic.new_stream();
            }
            classic.rate(rate_hz);
            if !continuous {
                put_down_ble(&mut ble, &self.state);
                put_down(&mut coded, &self.state);
                abandon(
                    &mut promises,
                    None,
                    crate::signal::ble::aux_ptr::AuxOutcome::FeedLost,
                    &self.state,
                );
                classic.fleet.clear();
                recent.clear();
            }
            next_pair = Some(first_pair + pairs);
            // The tuning and rate these samples were captured at, from the
            // block rather than the state: see `StreamBlock::centre_hz`.
            let centre_hz = centre_hz as f64;
            // Samples held from another tuning are not this one's: a window
            // or a measurement cut from them would be mixed to the wrong
            // channel. Dropped, and the promises waiting on them said.
            if held_tuning.is_some_and(|t| t != (centre_hz, rate_hz)) {
                recent.clear();
                abandon(
                    &mut promises,
                    None,
                    crate::signal::ble::aux_ptr::AuxOutcome::Refused(
                        "the radio retuned before its window",
                    ),
                    &self.state,
                );
            }
            held_tuning = Some((centre_hz, rate_hz));

            let (still_open, span_hz, is_net_bt, is_survey, is_coded, phy, locked, following) = {
                let mut m = self.state.lock().unwrap_or_else(|e| e.into_inner());
                let h = &mut m.net.health;
                h.blocks_in = h.blocks_in.saturating_add(1);
                h.pairs_in = h.pairs_in.saturating_add(pairs);
                h.gaps = h.gaps.saturating_add(u64::from(broke));
                h.blocks_lost = h.blocks_lost.saturating_add(plan.dropped);
                h.run_blocks = run.blocks;
                h.last_block = Some(now);
                if broke || plan.dropped > 0 {
                    h.last_loss = Some(now);
                }
                // The usable span is the baseband filter's where the radio has
                // one, because the bins the front end rolled off carry no
                // measurement and averaging them in would drag every cell at the
                // edges of the view down towards a floor that is not the band's.
                // Where there is no filter, the rate is all we know.
                let span = if m.radio.bb_filter_hz > 0 {
                    m.radio.bb_filter_hz as f64
                } else {
                    rate_hz
                };
                (
                    m.ui.is_net_section(),
                    span.min(rate_hz),
                    CLASSIC_VIEWS.contains(&m.ui.active_preset.as_str()),
                    m.ui.active_preset == super::lock::SURVEY_VIEW,
                    super::lock::CODED_VIEWS.contains(&m.ui.active_preset.as_str()),
                    m.net.ble_phy,
                    m.net.mode == crate::state::NetMode::Lock,
                    m.net.ble_connections.iter().any(|f| {
                        *f.connection.state() == crate::signal::ble::follow::State::Following
                    }),
                )
            };
            let tuning = Tuning {
                first_pair,
                centre_hz,
                rate_hz,
                span_hz,
            };

            // Decoded once, by the first receiver that needs it, and shared by
            // the BLE receiver and every classic channel: each used to turn the
            // same bytes into the same samples for itself.
            let mut iq: Option<Vec<num_complex::Complex<f32>>> = None;

            // BLE decode: only possible on one of the three fixed advertising
            // frequencies, and only at a sample rate `receive::front_end` can
            // reach the working rate from. Neither condition is `net_survey`'s
            // to share, so this keeps its own refusal rather than reusing
            // `survey_refused`.
            // Surveying, the advertising channel in view; locked, the
            // tuning's own (`channel::to_decode`). LE 2M is never sent on the
            // advertising channels, so for it the tuning's own channel it is.
            let channel = if phy == crate::signal::ble::Phy::TwoM {
                crate::signal::ble::channel::channel_of(centre_hz as u64)
            } else {
                crate::signal::ble::channel::to_decode(centre_hz as u64, span_hz, locked)
            };
            // The advertising channel the BLE receiver is to be fed this
            // block, if any: fed below, alongside the classic fleet.
            let mut ble_on: Option<u8> = None;
            // On the LE Coded view LE Coded's chain runs in LE 1M's place, on
            // the advertising channel LE 1M would have: neither pays for the
            // other, and neither list shows the other's packets.
            let mut coded_on: Option<u8> = None;
            if is_coded && still_open {
                put_down_ble(&mut ble, &self.state);
                let advertising =
                    crate::signal::ble::channel::to_decode(centre_hz as u64, span_hz, locked)
                        .filter(|&ch| {
                            crate::signal::ble::channel::advertising_channel_index(ch).is_some()
                        });
                match advertising {
                    Some(ch) => {
                        if !coded
                            .as_ref()
                            .is_some_and(|r| r.matches(ch, rate_hz, centre_hz))
                        {
                            put_down(&mut coded, &self.state);
                            let built = crate::signal::ble::coded_rx::CodedReceiver::new(
                                rate_hz, ch, centre_hz,
                            );
                            let mut m = self.state.lock().unwrap_or_else(|e| e.into_inner());
                            match built {
                                Ok(r) => {
                                    m.net.coded_refused = None;
                                    m.net.coded_channel = Some(ch);
                                    coded = Some(r);
                                }
                                Err(reason) => {
                                    m.net.coded_refused = Some(reason);
                                    m.net.coded_channel = None;
                                }
                            }
                        }
                        if coded.is_some() {
                            coded_on = Some(ch);
                        }
                    }
                    None => {
                        put_down(&mut coded, &self.state);
                        let mut m = self.state.lock().unwrap_or_else(|e| e.into_inner());
                        m.net.coded_refused = Some(
                            "not tuned to an advertising channel (2402, 2426 or 2480 MHz)"
                                .to_string(),
                        );
                        m.net.coded_channel = None;
                    }
                }
            } else {
                put_down(&mut coded, &self.state);
            }
            if coded.is_none() {
                abandon(
                    &mut promises,
                    Some(AuxList::Coded),
                    crate::signal::ble::aux_ptr::AuxOutcome::FeedLost,
                    &self.state,
                );
            }
            match channel.filter(|_| !is_coded) {
                Some(ch)
                    if still_open
                        && phy == crate::signal::ble::Phy::TwoM
                        && crate::signal::ble::channel::advertising_channel_index(ch).is_some() =>
                {
                    // The primary advertising channels carry LE 1M and LE
                    // Coded only (legacy advertising is always LE 1M, and
                    // extended advertising's primary channel is 1M or
                    // Coded), so a 2M decoder here would listen to nothing
                    // and an empty list would read as a quiet room. Said,
                    // and not run.
                    put_down_ble(&mut ble, &self.state);
                    let mut m = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    m.net.ble_refused = Some(format!(
                        "LE 2M is not used on the primary advertising channels (ch {ch} is one); \
                         tune to a data or secondary channel, or switch back to LE 1M"
                    ));
                    m.net.ble_channel = None;
                }
                Some(ch) if still_open => {
                    // The PHY the user chose (`NetState::ble_phy`): a switch
                    // rebuilds the receiver, like a retune does.
                    if !ble
                        .as_ref()
                        .is_some_and(|r| r.matches(ch, rate_hz, phy, centre_hz))
                    {
                        put_down_ble(&mut ble, &self.state);
                        ble = match BleReceiver::new(rate_hz, ch, phy, centre_hz) {
                            Ok(r) => {
                                let mut m = self.state.lock().unwrap_or_else(|e| e.into_inner());
                                m.net.ble_refused = None;
                                m.net.ble_channel = Some(ch);
                                Some(r)
                            }
                            Err(reason) => {
                                let mut m = self.state.lock().unwrap_or_else(|e| e.into_inner());
                                m.net.ble_refused = Some(reason);
                                m.net.ble_channel = None;
                                None
                            }
                        };
                    }
                    if ble.is_some() {
                        ble_on = Some(ch);
                    }
                }
                Some(_) => {
                    // Section closed; nothing decodes while it is.
                    put_down_ble(&mut ble, &self.state);
                }
                None => {
                    put_down_ble(&mut ble, &self.state);
                    let mut m = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    m.net.ble_refused = Some(
                        "not tuned to an advertising channel (2402, 2426 or 2480 MHz)".to_string(),
                    );
                    m.net.ble_channel = None;
                }
            }

            // An LE advertisement's promises go with the receiver that heard
            // it; LE Coded's with its own, above.
            if ble.is_none() {
                abandon(
                    &mut promises,
                    Some(AuxList::Le),
                    crate::signal::ble::aux_ptr::AuxOutcome::FeedLost,
                    &self.state,
                );
            }

            // Decoded once, before either decoder runs, so both read it at once.
            let classic_here = still_open && (is_net_bt || is_survey);
            let follow_here = still_open && following;
            // The band is measured on the survey, the one view that shows
            // it; elsewhere its cost would buy nothing on screen, and its
            // time axis is kept moving with "nobody looked" instead.
            let scan_here = still_open && is_survey;
            if ble_on.is_some() || coded_on.is_some() || classic_here || follow_here || scan_here {
                decoded_block(&mut iq, &bytes, self.geometry);
            }
            if scan_here {
                // Retuning invalidates every cell mapping, so the scan is
                // rebuilt and whatever it had accumulated goes with it: half a
                // dwell at one frequency and half at another is a measurement
                // of neither.
                if !scan
                    .as_ref()
                    .is_some_and(|s| s.matches(centre_hz, rate_hz, span_hz))
                {
                    scan = Some(Scan::new(centre_hz, rate_hz, span_hz));
                }
                if let (Some(scan), Some(block)) = (scan.as_mut(), iq.as_deref()) {
                    scan.push_iq(block);
                    if scan.observed_s() >= DWELL_S {
                        let band = scan.take();
                        let mut m = self.state.lock().unwrap_or_else(|e| e.into_inner());
                        m.net.band.absorb(band, now);
                    }
                }
            } else {
                scan = None;
                if still_open {
                    let mut m = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    m.net.band.mark_unobserved(now);
                }
            }
            let block: &[num_complex::Complex<f32>] = iq.as_deref().unwrap_or(&[]);
            let mut ble_packets: Option<Vec<crate::signal::ble::pdu::Packet>> = None;

            // The classic receiver, one per channel the current tuning and
            // the cap together let it watch: `self.bt_channels` on the
            // Classic view, and on the survey only as many as its measured
            // load leaves room for (`SURVEY_LOAD_HIGH`), so the coexistence
            // history can mark classic hits without starving the band
            // measurement the survey is for. Classic BT has no fixed channel
            // set to gate on, so the preset's name is the gate.
            if classic_here {
                classic.watch(tuning, is_net_bt, self.bt_channels, &self.state);
                let (ble_out, answers) = push_all(
                    ble_on.and(ble.as_mut()),
                    &mut classic.fleet,
                    block,
                    first_pair,
                );
                ble_packets = ble_out;
                let window = held(&recent, iq.as_deref().map(|v| (first_pair, v)));
                classic.read(
                    answers,
                    &window,
                    tuning,
                    now,
                    self.slot_fit_every,
                    &self.state,
                );
            } else {
                classic.stand_down(&self.state);
            }

            // The BLE receiver alone, when no classic fleet ran beside it,
            // then what it found, as it always was.
            if ble_packets.is_none() {
                if let (Some(_), Some(rx)) = (ble_on, ble.as_mut()) {
                    ble_packets = Some(rx.push_iq_at(block, first_pair));
                }
            }
            if let (Some(ch), Some(mut packets), Some(rx)) = (ble_on, ble_packets, ble.as_mut()) {
                // LE 1M read again as a tester reads it
                // (`measure::le_1m`), outside the lock; a packet whose
                // window is not held keeps no figure rather than the
                // receiver's own. LE 2M keeps the receiver's.
                if phy == crate::signal::ble::Phy::OneM && !packets.is_empty() {
                    let window = held(&recent, iq.as_deref().map(|v| (first_pair, v)));
                    let offset =
                        crate::signal::ble::channel::centre_hz(ch).map(|hz| hz as f64 - centre_hz);
                    for p in packets.iter_mut() {
                        let read = offset.zip(p.pdu_pair).and_then(|(o, at)| {
                            super::measure::le_1m(&window, rate_hz, o, at, &p.air)
                        });
                        (p.modulation, p.drift) = read.unwrap_or((None, None));
                    }
                }
                let funnel = rx.take_funnel();
                if !funnel.is_empty() {
                    let mut m = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    m.net.health.ble.add(funnel);
                }
                if !packets.is_empty() {
                    // Read before the lock: parsing is work the UI
                    // thread should not wait behind.
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
                                .find(|q| {
                                    crate::signal::ble::follow::answers(q.pdu_type, q.adv_addr, &c)
                                })
                                .map(|q| q.ch_sel);
                            let flags = (p.ch_sel, p.tx_add_random, p.rx_add_random);
                            Some((c, flags, answered, connect_end_pair(p, rate_hz)?))
                        })
                        .collect();
                    let mut m = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    for (c, (ch_sel, init_random, adv_random), answered, end) in &connects {
                        // Not in this block: the newest kept, from the ring.
                        let answered = answered.or_else(|| {
                            m.net
                                .ble_packets
                                .iter()
                                .find(|q| {
                                    crate::signal::ble::follow::answers(q.pdu_type, q.adv_addr, c)
                                })
                                .map(|q| q.ch_sel)
                        });
                        let csa2 = crate::signal::ble::follow::uses_csa2(*ch_sel, answered);
                        m.net
                            .follow(c, (csa2, *init_random, *adv_random), *end, rate_hz, now);
                    }
                    for (address, said) in advertised {
                        m.net.advertised.entry(address).or_default().merge(said);
                    }
                    if let Some(i) = crate::signal::ble::channel::advertising_channel_index(ch) {
                        m.net.ble_channel_packets[i] += packets.len() as u64;
                        m.net.ble_channel_crc_ok[i] +=
                            packets.iter().filter(|p| p.crc_ok).count() as u64;
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
                        census_from_ble(&mut m.net.census.devices, &p, ch, locked, rate_hz, now);
                        m.net.ble_heard += 1;
                        let seq = m.net.ble_heard;
                        // On a primary channel, a type 7 is an ADV_EXT_IND,
                        // and its AuxPtr is followed as LE Coded's is.
                        let ext = crate::signal::ble::channel::advertising_channel_index(ch)
                            .and_then(|_| extended(&p, crate::state::ExtRole::AdvExt))
                            .map(|mut e| {
                                let made = packet_start(&p, phy, rate_hz).map(|s| {
                                    crate::signal::ble::aux_ptr::promise(
                                        seq, s, &e.header, 0, rate_hz,
                                    )
                                });
                                e.aux = keep_promise(&mut m, &mut promises, AuxList::Le, made);
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

            // Each followed connection's events whose windows this block
            // completes: demodulated outside the lock with the link's own
            // receiver, accounted for inside it. A round takes one event a
            // connection, so an event's outcome is in its timing before the
            // next one is placed.
            if follow_here {
                if let Some(this) = iq.as_deref() {
                    let window = held(&recent, Some((first_pair, this)));
                    let held_end = (first_pair + this.len() as u64) as f64;
                    for _ in 0..FOLLOW_ROUNDS {
                        let jobs: Vec<_> = {
                            let m = self.state.lock().unwrap_or_else(|e| e.into_inner());
                            m.net
                                .ble_connections
                                .iter()
                                .map(|f| &f.connection)
                                .filter(|c| {
                                    *c.state() == crate::signal::ble::follow::State::Following
                                })
                                .map(|c| {
                                    let interval = c.params().interval as f64 * 1.25e-3 * rate_hz;
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
                            let (from, to) = event_window(&due, interval, rate_hz);
                            if to > held_end {
                                continue;
                            }
                            let link = crate::signal::ble::receive::Link::Data {
                                access_address: aa,
                                crc_init,
                            };
                            let mut ears = vec![super::listen::Ear::Link(link, phy_c)];
                            if phy_p != phy_c {
                                ears.push(super::listen::Ear::Link(link, phy_p));
                            }
                            let job = super::listen::Job {
                                ears,
                                channel: due.channel,
                                from_pair: from,
                                to_pair: to,
                            };
                            let heard = listener.listen(&job, &window, rate_hz, centre_hz, span_hz);
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
                        let mut m = self.state.lock().unwrap_or_else(|e| e.into_inner());
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
                    let m = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    let alive: std::collections::HashSet<u32> = m
                        .net
                        .ble_connections
                        .iter()
                        .filter(|f| {
                            *f.connection.state() == crate::signal::ble::follow::State::Following
                        })
                        .map(|f| f.connection.access_address())
                        .collect();
                    drop(m);
                    listener.retain(|ear| match ear {
                        super::listen::Ear::Link(
                            crate::signal::ble::receive::Link::Data { access_address, .. },
                            _,
                        ) => alive.contains(access_address),
                        _ => true,
                    });
                }
            }

            // LE Coded's packets, measured as the test suite defines them
            // (`measure::le_coded`) outside the lock, from the symbols their
            // decoded bits were sent as.
            if let (Some(ch), Some(rx), Some(this)) = (coded_on, coded.as_mut(), iq.as_deref()) {
                let window = held(&recent, Some((first_pair, this)));
                let primary: Vec<_> = rx
                    .push_iq_at(this, first_pair)
                    .into_iter()
                    .map(|p| {
                        let reading = measure_coded(&p, ch, &window, rate_hz, centre_hz);
                        (p, reading)
                    })
                    .collect();
                let funnel = rx.take_funnel();
                {
                    let mut m = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    m.net.health.coded.add(funnel);
                    for (p, reading) in primary {
                        let start = p.at_pair.map(|a| a as f64);
                        let ext = extended(&p, crate::state::ExtRole::AdvExt);
                        let seq = m.net.coded_heard + 1;
                        let ext = ext.map(|mut e| {
                            e.aux = keep_promise(
                                &mut m,
                                &mut promises,
                                AuxList::Coded,
                                start.map(|s| {
                                    crate::signal::ble::aux_ptr::promise(
                                        seq, s, &e.header, 0, rate_hz,
                                    )
                                }),
                            );
                            e
                        });
                        push_coded(&mut m, p, ch, reading, ext, now);
                    }
                }
            }

            // Every AuxPtr whose window this block completes, listened to
            // where it promised, its packet joining its advertisement's list.
            if let Some(this) = iq.as_deref().filter(|_| !promises.is_empty()) {
                use crate::signal::ble::aux_ptr::{AuxOutcome, AuxPhy};
                use crate::signal::ble::Phy;
                let window = held(&recent, Some((first_pair, this)));
                let held_end = (first_pair + this.len() as u64) as f64;
                // Promises whose windows are now held, in the order made.
                let (due, waiting): (Vec<_>, Vec<_>) = promises
                    .drain(..)
                    .partition(|p| p.promise.to_pair <= held_end);
                promises = waiting;
                for Pending { promise, list } in due {
                    let link = crate::signal::ble::receive::Link::Auxiliary;
                    let ear = match promise.phy {
                        AuxPhy::Coded => super::listen::Ear::Coded,
                        AuxPhy::OneM => super::listen::Ear::Link(link, Phy::OneM),
                        AuxPhy::TwoM => super::listen::Ear::Link(link, Phy::TwoM),
                    };
                    let job = super::listen::Job {
                        ears: vec![ear],
                        channel: promise.channel,
                        from_pair: promise.from_pair,
                        to_pair: promise.to_pair,
                    };
                    let out = listener.listen(&job, &window, rate_hz, centre_hz, span_hz);
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
                        crate::signal::ble::aux_ptr::keeps(&promise, phy, &ext.header)
                            .then_some((p, phy, ext))
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
                            Phy::Coded(_) => {
                                measure_coded(&p, promise.channel, &window, rate_hz, centre_hz)
                            }
                            Phy::OneM => {
                                let offset =
                                    crate::signal::ble::channel::centre_hz(promise.channel)
                                        .map(|hz| hz as f64 - centre_hz);
                                let read = offset.zip(p.pdu_pair).and_then(|(o, at)| {
                                    super::measure::le_1m(&window, rate_hz, o, at, &p.air)
                                });
                                (p.modulation, p.drift) = read.unwrap_or((None, None));
                                None
                            }
                            Phy::TwoM => None,
                        };
                        let start = packet_start(&p, phy, rate_hz);
                        let after_us = start
                            .map_or(0.0, |s| (s - promise.superior_start_pair) / rate_hz * 1e6);
                        let mut m = self.state.lock().unwrap_or_else(|e| e.into_inner());
                        let seq = match list {
                            AuxList::Le => m.net.ble_heard,
                            AuxList::Coded => m.net.coded_heard,
                        } + 1;
                        // Its own AuxPtr, if any, is the chain's next link.
                        ext.aux = keep_promise(
                            &mut m,
                            &mut promises,
                            list,
                            start.map(|s| {
                                crate::signal::ble::aux_ptr::promise(
                                    seq,
                                    s,
                                    &ext.header,
                                    promise.depth + 1,
                                    rate_hz,
                                )
                            }),
                        );
                        match list {
                            AuxList::Le => {
                                census_from_aux(
                                    &mut m,
                                    &mut p,
                                    &ext,
                                    promise.channel,
                                    rate_hz,
                                    now,
                                );
                                push_le_aux(&mut m, p, promise.channel, phy, reading, ext, now)
                            }
                            AuxList::Coded => {
                                push_coded(&mut m, p, promise.channel, reading, Some(ext), now)
                            }
                        }
                        AuxOutcome::Heard { seq, after_us }
                    } else {
                        AuxOutcome::Missed
                    };
                    let mut m = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    m.net.health.aux.count(&outcome);
                    set_aux_outcome(&mut m, list, promise.superior_seq, outcome);
                }
            }

            // Held for the measurement path, as much as `measure::HELD_S`
            // asks and no more, or a followed connection's event needs; a
            // block nothing decoded leaves a hole, so what was held before it
            // can no longer be joined to what comes after.
            match iq.take() {
                Some(block) if still_open => {
                    recent.push_back((first_pair, block));
                    // An LE Coded packet at S=8 lasts up to 17 ms, and is
                    // measured whole once it ends.
                    let held_s = if following || is_coded || !promises.is_empty() {
                        FOLLOW_HELD_S.max(super::measure::HELD_S)
                    } else {
                        super::measure::HELD_S
                    };
                    let keep = (held_s * rate_hz) as usize;
                    while recent.len() > 1
                        && recent.iter().skip(1).map(|(_, b)| b.len()).sum::<usize>() >= keep
                    {
                        recent.pop_front();
                    }
                }
                _ => recent.clear(),
            }

            // Closing the section stops `process_block` forwarding, but blocks
            // already in the channel still arrive - and the run they belong to
            // is over whether or not they are the last of it.
            if !still_open {
                run.suspend();
                // The band measurement stops with it. Nothing has been observed
                // since the section closed, and a panel reopened an hour later
                // showing the last dwell as if it were current is exactly what
                // rule 4 exists to prevent; the chrome's staleness marks it, and
                // dropping the scan means the next dwell starts clean.
                scan = None;
                put_down_ble(&mut ble, &self.state);
                put_down(&mut coded, &self.state);
                classic.close();
                load = Load::default();
                next_pair = None;
                let mut m = self.state.lock().unwrap_or_else(|e| e.into_inner());
                Classic::unpublish(&mut m);
                m.net.ble_channel = None;
                // Nothing is being decoded, so there is no load to report -
                // and a figure from before the section closed must not be
                // shown on reopening as if it were current.
                m.net.health.decode_load = None;
            } else if let Some(reading) = load.add(now.elapsed(), pairs, rate_hz) {
                // The clock read and the division both happened above, outside
                // the lock; only the finished figure goes in.
                let mut m = self.state.lock().unwrap_or_else(|e| e.into_inner());
                m.net.health.decode_load = Some(reading);
                // The survey's classic channels follow the load it measured:
                // one fewer over the high mark, one more under the low one.
                if is_survey {
                    classic.loaded(reading, self.bt_channels);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::SampleFormat;

    /// The survey starts with no classic channel, and says it is the load:
    /// its first reading has not come yet, and a receiver it has not measured
    /// room for is not run on the band measurement's time.
    #[test]
    fn the_survey_runs_no_classic_channel_until_its_load_leaves_room() {
        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = crate::signal::net::SECTION.to_string();
        m.ui.active_preset = super::super::lock::SURVEY_VIEW.to_string();
        m.radio.frequency = 2_441_000_000;
        m.radio.config_sample_rate = 8e6;
        m.radio.bb_filter_hz = 0;
        let state = Arc::new(Mutex::new(m));
        let (tx, rx) = crossbeam_channel::unbounded();
        tx.send(stamped(&state, 1, false, vec![0u8; 2 * 4096]))
            .unwrap();
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), eight_bit(), SAFE_BT_CHANNELS).run();
        let m = state.lock().unwrap();
        assert!(m.net.bt_channels_watched.is_empty());
        assert!(m.net.bt_load_limited);
        assert_eq!(
            m.net.bt_refused.as_deref(),
            Some("not running: the survey's load leaves no room")
        );
    }

    /// A block as `hardware::process::process_block` would stamp it: its
    /// position is the `seq`-th block of this size, so a jump in `seq` is a
    /// jump in the stream, and its tuning is whatever the test put in the
    /// state - read now, at capture, the way the real stamp is.
    fn stamped(
        state: &Arc<Mutex<SdrMetrics>>,
        seq: u64,
        gap_before: bool,
        bytes: Vec<u8>,
    ) -> StreamBlock {
        let m = state.lock().unwrap();
        let pairs = bytes.len() as u64 / 2;
        StreamBlock {
            seq,
            gap_before,
            first_pair: seq.saturating_sub(1) * pairs,
            centre_hz: m.radio.frequency,
            rate_hz: m.radio.config_sample_rate,
            bytes,
        }
    }

    /// **What the receive chain costs, measured.** Not a check: run it by
    /// hand, in release, on the machine the question is about
    /// (`cargo test --release measure_the_receive_chain -- --ignored
    /// --nocapture`), and it prints each view's wall time over the stream
    /// time it covered, the figure the header calls the decode load. Fed
    /// noise, so it measures the chain's floor, not a busy room's.
    #[test]
    #[ignore]
    fn measure_the_receive_chain() {
        use crate::signal::dsp::testkit::Rng;
        const SECONDS: f64 = 4.0;
        const BLOCK_PAIRS: usize = 131_072;
        let cases: [(&str, f64, u64); 7] = [
            ("net_survey", 8e6, 2_426_500_000),
            ("net_ble", 8e6, 2_426_000_000),
            ("net_ble", 20e6, 2_426_000_000),
            ("net_bt", 4e6, 2_440_000_000),
            ("net_bt", 8e6, 2_440_000_000),
            ("net_bt", 20e6, 2_440_000_000),
            ("net_survey", 20e6, 2_426_500_000),
        ];
        // Noise at a level an ADC sees in a quiet band: well clear of clipping.
        let block: Vec<u8> = Rng::new(7)
            .noise(BLOCK_PAIRS, 0.02)
            .iter()
            .flat_map(|z| {
                let q = |v: f32| (v * 128.0).clamp(-127.0, 127.0) as i8 as u8;
                [q(z.re), q(z.im)]
            })
            .collect();
        // `SDRTOP_MEASURE=net_bt@8` runs that one case alone, for a profiler.
        let only = std::env::var("SDRTOP_MEASURE").ok();
        for (preset, rate, tuned) in cases {
            let name = format!("{preset}@{}", rate / 1e6);
            if only.as_ref().is_some_and(|o| *o != name) {
                continue;
            }
            let mut m = SdrMetrics::fixture().streaming();
            m.ui.section = crate::signal::net::SECTION.to_string();
            m.ui.active_preset = preset.to_string();
            m.radio.frequency = tuned;
            m.radio.config_sample_rate = rate;
            m.radio.bb_filter_hz = 0;
            let state = Arc::new(Mutex::new(m));
            let blocks = (SECONDS * rate / BLOCK_PAIRS as f64).ceil() as u64;
            let (tx, rx) = crossbeam_channel::unbounded();
            for seq in 1..=blocks {
                tx.send(stamped(&state, seq, false, block.clone())).unwrap();
            }
            drop(tx);
            let start = std::time::Instant::now();
            NetWorker::new(rx, Arc::clone(&state), eight_bit(), SAFE_BT_CHANNELS).run();
            let wall = start.elapsed().as_secs_f64();
            let stream = blocks as f64 * BLOCK_PAIRS as f64 / rate;
            let m = state.lock().unwrap();
            eprintln!(
                "{preset:<11} {:>5.1} Msps  BLE ch {:?}  BT {} ch  {:.2}x real time",
                rate / 1e6,
                m.net.ble_channel,
                m.net.bt_channels_watched.len(),
                wall / stream
            );
        }
    }

    fn eight_bit() -> SampleGeometry {
        SampleGeometry {
            format: SampleFormat::Int8,
            full_scale: 128.0,
        }
    }

    /// Run the worker over a scripted feed and read back what it recorded.
    ///
    /// The channel is closed before the worker starts, so `run` drains it and
    /// returns rather than blocking: the loop under test is the same one either
    /// way, and a test that has to join a live thread is a test that can hang
    /// CI.
    fn feed(blocks: &[(u64, bool, usize)], open: bool) -> crate::state::NetDecodeHealth {
        let mut m = SdrMetrics::fixture();
        m.ui.section = if open {
            crate::signal::net::SECTION.to_string()
        } else {
            String::new()
        };
        let state = Arc::new(Mutex::new(m));
        let (tx, rx) = crossbeam_channel::unbounded();
        for &(seq, gap_before, pairs) in blocks {
            tx.send(stamped(&state, seq, gap_before, vec![0u8; pairs * 2]))
                .unwrap();
        }
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), eight_bit(), SAFE_BT_CHANNELS).run();
        let m = state.lock().unwrap();
        m.net.health.clone()
    }

    #[test]
    fn an_unbroken_feed_reports_no_gaps_and_one_growing_run() {
        let h = feed(&[(1, false, 64), (2, false, 64), (3, false, 64)], true);
        assert_eq!(h.blocks_in, 3);
        assert_eq!(h.pairs_in, 192, "eight-bit pairs are two bytes each");
        assert_eq!((h.gaps, h.blocks_lost), (0, 0));
        assert_eq!(h.run_blocks, 3);
        assert!(h.last_block.is_some());
        assert!(
            h.last_loss.is_none(),
            "nothing was lost, so there is no when"
        );
    }

    /// Opening the section on a radio that is already streaming is the normal
    /// case, and it must not read as a loss.
    ///
    /// The device stamps every callback whether or not this feed is being
    /// forwarded to, so the first block through carries a sequence number
    /// thousands past whatever the worker last saw. Counting that as an
    /// interruption put a phantom gap on the panel once per visit, and a panel
    /// whose job is to say what the receiver missed is the last place that can
    /// afford one.
    #[test]
    fn opening_the_section_mid_stream_is_not_an_interruption() {
        let h = feed(&[(9_412, false, 64), (9_413, false, 64)], true);
        assert_eq!(h.gaps, 0, "nothing was interrupted; nothing had started");
        assert_eq!(h.blocks_lost, 0);
        assert!(h.last_loss.is_none(), "and no loss is dated either");
        assert_eq!(
            h.run_blocks, 2,
            "and the run is the two blocks that arrived"
        );
    }

    #[test]
    fn a_gap_breaks_the_run_and_starts_a_new_one() {
        // Three good blocks, then the driver says samples went missing.
        let h = feed(
            &[
                (1, false, 64),
                (2, false, 64),
                (3, false, 64),
                (4, true, 64),
            ],
            true,
        );
        assert_eq!(h.gaps, 1);
        assert_eq!(h.blocks_lost, 1, "the floor: samples went, count unknown");
        assert_eq!(h.run_blocks, 1, "and the new run is one block long");
        assert!(h.last_loss.is_some(), "a loss says when it happened");
    }

    #[test]
    fn blocks_lost_by_the_channel_are_counted_too() {
        // 1, then 5: three blocks the bounded feed refused, plus a driver gap.
        let h = feed(&[(1, false, 64), (5, true, 64)], true);
        assert_eq!(h.blocks_lost, 4);
        assert_eq!(h.gaps, 1);
    }

    /// End to end, with no radio: bytes in, a duty cycle out, on the cell the
    /// transmitter was actually on.
    ///
    /// The synthetic transmitter is a carrier three megahertz above the tuning,
    /// keyed on for a quarter of the time. Everything between those bytes and
    /// the number on the panel runs here: the transform, the bin-to-cell
    /// mapping, the noise floor and its correction, and the threshold.
    #[test]
    fn a_transmitter_in_the_band_lands_on_its_own_cell() {
        use crate::signal::dsp::testkit::Rng;
        use std::f64::consts::TAU;

        const RATE: f64 = 20_000_000.0;
        const CENTRE: u64 = 2_437_000_000;
        const OFFSET: f64 = 3_000_000.0;
        const DUTY: f64 = 0.25;
        // Eight microseconds at 20 Msps is 160 samples, so the transform is 128
        // and one block holds a whole number of windows.
        const PAIRS: usize = 128 * 400;

        let mut rng = Rng::new(0xB0_1234);
        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = crate::signal::net::SECTION.to_string();
        // The band is measured on the survey, the view that shows it.
        m.ui.active_preset = super::super::lock::SURVEY_VIEW.to_string();
        m.radio.frequency = CENTRE;
        m.radio.config_sample_rate = RATE;
        m.radio.bb_filter_hz = 18_000_000;
        let state = Arc::new(Mutex::new(m));

        let (tx, rx) = crossbeam_channel::unbounded();
        let mut phase = 0.0f64;
        // Enough blocks to complete one dwell, so the real publish path runs.
        for seq in 1..=20u64 {
            let mut bytes = Vec::with_capacity(PAIRS * 2);
            for i in 0..PAIRS {
                // Keyed in whole windows, so a window is either on or off and
                // the duty cycle the measurement recovers is the one keyed.
                let window = i / 128;
                let on = (window % 100) < (100.0 * DUTY) as usize;
                phase += TAU * OFFSET / RATE;
                let (mut re, mut im) = (0.0f64, 0.0f64);
                if on {
                    re = 40.0 * phase.cos();
                    im = 40.0 * phase.sin();
                }
                let (a, b) = rng.normal_pair();
                bytes.push((re + a).clamp(-127.0, 127.0) as i8 as u8);
                bytes.push((im + b).clamp(-127.0, 127.0) as i8 as u8);
            }
            tx.send(stamped(&state, seq, false, bytes)).unwrap();
        }
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), eight_bit(), SAFE_BT_CHANNELS).run();

        let m = state.lock().unwrap();
        let band = &m.net.band;
        assert!(band.trusted, "tail {} spread {}", band.tail, band.spread);
        assert_eq!(band.cells.len(), crate::signal::net::occupancy::CELLS);

        // The carrier is at 2440 MHz, which is cell 40.
        let cell = crate::signal::net::occupancy::cell_of(2_440_000_000.0).unwrap();
        assert_eq!(cell, 40);
        let hit = &band.cells[cell];
        assert!(hit.observed());
        assert!(
            (hit.duty - DUTY).abs() < 0.02,
            "cell {cell} read {} for a keyed {DUTY}",
            hit.duty
        );

        // A cell nobody transmitted in reads empty, which is a measurement.
        let quiet = &band.cells[30];
        assert!(quiet.observed());
        assert_eq!(quiet.duty, 0.0);
        assert!(quiet.mean_dbfs < hit.mean_dbfs - 10.0);

        // A cell outside the eighteen megahertz in view was not looked at, and
        // that is a different answer from an empty one.
        assert!(!band.cells[0].observed());
        assert!(!band.cells[60].observed());
    }

    /// A dwell is published when it is a dwell, and not before.
    ///
    /// A fraction measured over four hundred windows can only take values a
    /// quarter of a percent apart, and carries a standard error four times
    /// coarser than that. Publishing whatever has arrived so far would put a
    /// number on screen finer than the observation behind it, which is the same
    /// mistake `Uncertain::decimals` exists to prevent one layer up.
    #[test]
    fn a_partial_dwell_is_not_published() {
        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = crate::signal::net::SECTION.to_string();
        m.radio.frequency = 2_437_000_000;
        m.radio.config_sample_rate = 20_000_000.0;
        m.radio.bb_filter_hz = 18_000_000;
        let state = Arc::new(Mutex::new(m));

        let (tx, rx) = crossbeam_channel::unbounded();
        // One block: four hundred windows of six and a half microseconds, which
        // is under three milliseconds of the fifty a dwell is.
        tx.send(stamped(&state, 1, false, vec![0x05u8; 128 * 400 * 2]))
            .unwrap();
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), eight_bit(), SAFE_BT_CHANNELS).run();

        let m = state.lock().unwrap();
        assert_eq!(m.net.health.blocks_in, 1, "the block arrived");
        assert!(
            m.net.band.cells.is_empty(),
            "and nothing was published from it"
        );
    }

    #[test]
    fn closing_the_section_ends_the_run_rather_than_losing_it() {
        // The section is closed, so every block in the channel is one already in
        // flight when it closed. The jump between them must not be reported as a
        // loss, because the device counts callbacks whether or not we take them.
        let h = feed(&[(1, false, 64), (9_000, false, 64)], false);
        assert_eq!(h.blocks_in, 2, "blocks in flight are still counted");
        assert_eq!(
            (h.gaps, h.blocks_lost),
            (0, 0),
            "and the pause is not a loss"
        );
    }

    /// Through the actual worker rather than
    /// `Receiver` directly: tuned to an advertising channel at a rate the
    /// decoder can reach, a synthetic packet in the block stream ends up in
    /// `net.ble_packets`, CRC-checked.
    #[test]
    fn a_synthetic_advertising_packet_reaches_ble_packets() {
        use crate::signal::ble::detect::{
            access_address_bits, preamble_bits, ADVERTISING_ACCESS_ADDRESS,
        };
        use crate::signal::ble::gfsk::modulate;
        use crate::signal::ble::pdu::encode;
        use crate::signal::ble::Phy;
        use crate::signal::dsp::testkit::{at_snr, Rng};
        use num_complex::Complex;

        const SPS: usize = 4;
        const SAMPLE_RATE: f64 = 4_000_000.0;
        const CHANNEL: u8 = 37; // 2402 MHz

        let addr = [0x11u8, 0x22, 0x33, 0x44, 0x55, 0x66];
        let mut bits = preamble_bits(ADVERTISING_ACCESS_ADDRESS, Phy::OneM);
        bits.extend_from_slice(&access_address_bits(ADVERTISING_ACCESS_ADDRESS));
        bits.extend_from_slice(&encode(
            CHANNEL,
            0x00,
            &crate::signal::ble::pdu::air_octets(addr),
        ));
        let mut rng = Rng::new(1);
        bits.extend((0..16).map(|_| rng.next_u64() & 1 == 1));
        let clean = modulate(&bits, SPS, 250_000.0, SAMPLE_RATE, 0.5);
        let noisy = at_snr(&clean, 20.0, &mut Rng::new(2));

        let geometry = eight_bit();
        let bytes: Vec<u8> = noisy
            .iter()
            .flat_map(|s: &Complex<f32>| {
                let re = (s.re * geometry.full_scale).clamp(-127.0, 127.0) as i8;
                let im = (s.im * geometry.full_scale).clamp(-127.0, 127.0) as i8;
                [re as u8, im as u8]
            })
            .collect();

        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = crate::signal::net::SECTION.to_string();
        m.radio.frequency = 2_402_000_000;
        m.radio.config_sample_rate = SAMPLE_RATE;
        m.radio.bb_filter_hz = 0;
        let state = Arc::new(Mutex::new(m));

        let (tx, rx) = crossbeam_channel::unbounded();
        tx.send(stamped(&state, 1, false, bytes)).unwrap();
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), geometry, SAFE_BT_CHANNELS).run();

        let m = state.lock().unwrap();
        assert!(m.net.ble_refused.is_none(), "{:?}", m.net.ble_refused);
        assert_eq!(
            m.net.ble_channel,
            Some(CHANNEL),
            "the receiver says where it is running"
        );
        assert_eq!(m.net.ble_packets.len(), 1, "{:?}", m.net.ble_packets);
        let p = &m.net.ble_packets[0];
        assert_eq!(p.channel, CHANNEL);
        assert_eq!(p.adv_addr, Some(addr));
        assert!(p.crc_ok);
        // Numbered on arrival, so `masked` can show it.
        assert_eq!(m.net.address_book.get(addr), Some(1));

        // What was decoded reaches the state whole: the payload the AD
        // structures will be read from, the address in air order inside it.
        assert_eq!(
            p.payload,
            crate::signal::ble::pdu::air_octets(addr).to_vec(),
            "{p:?}"
        );
        assert!(!p.ch_sel && !p.rx_add_random);
        // The section's frame error curve took it, in its SNR bin.
        assert_eq!(m.net.fer.total(), 1, "{:?}", m.net.fer);
        // A confirmed device reaches the shared census too, keyed by the same
        // address `net_ble_packets` shows.
        assert_eq!(m.net.census.devices.len(), 1, "{:?}", m.net.census.devices);
        assert_eq!(m.net.census.devices[0].address, addr);
        assert_eq!(m.net.census.devices[0].packets, 1);
    }

    /// A synthetic ADV_IND on LE 2M, as raw 8 Msps bytes on data channel
    /// `ch`: the advertising access address and the channel's own
    /// whitening, as a secondary advertising channel carries them.
    fn ble_2m_bytes(ch: u8) -> (Vec<u8>, [u8; 6]) {
        use crate::signal::ble::detect::{
            access_address_bits, preamble_bits, ADVERTISING_ACCESS_ADDRESS,
        };
        use crate::signal::ble::gfsk::modulate;
        use crate::signal::ble::pdu::encode;
        use crate::signal::ble::Phy;
        use crate::signal::dsp::testkit::{at_snr, Rng};
        use num_complex::Complex;

        let addr = [0x11u8, 0x22, 0x33, 0x44, 0x55, 0x66];
        let mut bits: Vec<bool> = (0..16).map(|i| i % 3 == 0).collect();
        bits.extend(preamble_bits(ADVERTISING_ACCESS_ADDRESS, Phy::TwoM));
        bits.extend_from_slice(&access_address_bits(ADVERTISING_ACCESS_ADDRESS));
        bits.extend_from_slice(&encode(
            ch,
            0x00,
            &crate::signal::ble::pdu::air_octets(addr),
        ));
        let mut rng = Rng::new(1);
        bits.extend((0..32).map(|_| rng.next_u64() & 1 == 1));
        let clean = modulate(&bits, 4, Phy::TwoM.deviation_hz(), 8_000_000.0, 0.5);
        let noisy = at_snr(&clean, 20.0, &mut Rng::new(2));
        let geometry = eight_bit();
        let bytes = noisy
            .iter()
            .flat_map(|s: &Complex<f32>| {
                let re = (s.re * geometry.full_scale).clamp(-127.0, 127.0) as i8;
                let im = (s.im * geometry.full_scale).clamp(-127.0, 127.0) as i8;
                [re as u8, im as u8]
            })
            .collect();
        (bytes, addr)
    }

    /// A connection set up and run on the air, as a 20 Msps stream tuned to
    /// 2426 MHz: a CONNECT_IND on advertising channel 38, then `events`
    /// events on the channels CSA #1 gives, each in view one an empty PDU
    /// from the Central at its anchor and one from the Peripheral T_IFS
    /// after. Returns the stream's eight-bit bytes, block by block, and the
    /// access address.
    fn a_connection_on_the_air(events: u16) -> (Vec<Vec<u8>>, u32) {
        a_connection_with_chsel(events, false, None)
    }

    /// [`a_connection_on_the_air`] with the CONNECT_IND's ChSel bit, and,
    /// when given, an ADV_DIRECT_IND from its AdvA with that ChSel 150 us
    /// before it, the PDU it answers.
    fn a_connection_with_chsel(
        events: u16,
        connect_ch_sel: bool,
        advertised: Option<bool>,
    ) -> (Vec<Vec<u8>>, u32) {
        use crate::signal::ble::connect::Csa1;
        use crate::signal::ble::data::{encode as encode_data, DataPdu};
        use crate::signal::ble::detect::{
            access_address_bits, preamble_bits, ADVERTISING_ACCESS_ADDRESS,
        };
        use crate::signal::ble::gfsk::modulate;
        use crate::signal::ble::pdu::encode;
        use crate::signal::ble::Phy;
        use crate::signal::dsp::testkit::Rng;
        use num_complex::Complex;

        const RATE: f64 = 20e6;
        const SPS: usize = 20;
        const TUNED: f64 = 2_426e6;
        const BLOCK: usize = 131_072;
        const AA: u32 = 0x5065_4b6a;
        const CRC: u32 = 0x3a_5b7c;
        let us = |x: f64| x * 1e-6 * RATE;
        let total = (us(2_000.0 + 1_250.0 + events as f64 * 7_500.0) as usize / BLOCK + 2) * BLOCK;
        let mut iq = vec![Complex::new(0.0f32, 0.0); total];
        let mut rng = Rng::new(17);
        // A burst whose preamble starts at `at`, on `ch`.
        let mut put = |at: usize, ch: u8, aa: u32, pdu_bits: Vec<bool>| {
            let mut bits: Vec<bool> = (0..16).map(|_| rng.next_u64() & 1 == 1).collect();
            bits.extend(preamble_bits(aa, Phy::OneM));
            bits.extend_from_slice(&access_address_bits(aa));
            bits.extend(pdu_bits);
            let wave = modulate(&bits, SPS, Phy::OneM.deviation_hz(), RATE, 0.5);
            let offset = crate::signal::ble::channel::centre_hz(ch).unwrap() as f64 - TUNED;
            let from = at - 16 * SPS;
            for (k, s) in wave.iter().enumerate() {
                let n = from + k;
                let ph = std::f64::consts::TAU * offset * n as f64 / RATE;
                iq[n] += s * Complex::new(ph.cos() as f32, ph.sin() as f32) * 0.6;
            }
        };
        // CONNECT_IND: InitA, AdvA, then LLData: AA, CRCInit, WinSize 1,
        // WinOffset 0, Interval 6 (7.5 ms), Latency 0, Timeout 100, every
        // channel, Hop 7, SCA 0.
        let mut payload = vec![
            0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0xa1, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6,
        ];
        payload.extend(AA.to_le_bytes());
        payload.extend(&CRC.to_le_bytes()[..3]);
        payload.extend([1, 0, 0, 6, 0, 0, 0, 100, 0, 0xff, 0xff, 0xff, 0xff, 0x1f, 7]);
        let connect_at = us(2_000.0) as usize;
        if let Some(adv_ch_sel) = advertised {
            // ADV_DIRECT_IND: AdvA then TargetA (the initiator), air order.
            let direct: Vec<u8> = payload[6..12]
                .iter()
                .chain(&payload[..6])
                .copied()
                .collect();
            let header = 0x01 | (adv_ch_sel as u8) << 5;
            // 8 + 32 + 16 + 12 * 8 + 24 bits, then T_IFS.
            let before = (8 + 32 + 16 + 12 * 8 + 24) * SPS + us(150.0) as usize;
            put(
                connect_at - before,
                38,
                ADVERTISING_ACCESS_ADDRESS,
                encode(38, header, &direct),
            );
        }
        let header = 0x05 | (connect_ch_sel as u8) << 5;
        put(
            connect_at,
            38,
            ADVERTISING_ACCESS_ADDRESS,
            encode(38, header, &payload),
        );
        // Preamble 8, address 32, header 16, 34 octets, CRC 24, 20 pairs a bit.
        let connect_end = connect_at + (8 + 32 + 16 + 34 * 8 + 24) * SPS;
        let empty = |sn: bool| DataPdu {
            llid: 1,
            nesn: !sn,
            sn,
            md: false,
            cte_info: None,
            payload: Vec::new(),
            crc_ok: true,
        };
        let mut csa = Csa1::new(7, (1u64 << 37) - 1).unwrap();
        for k in 0..events {
            let ch = csa.next();
            if !(7..=14).contains(&ch) {
                continue;
            }
            let anchor = connect_end + us(1_250.0 + k as f64 * 7_500.0) as usize;
            put(anchor, ch, AA, encode_data(&empty(false), CRC, ch));
            let central_end = anchor + (8 + 32 + 16 + 24) * SPS;
            put(
                central_end + us(150.0) as usize,
                ch,
                AA,
                encode_data(&empty(true), CRC, ch),
            );
        }
        let blocks = iq
            .chunks(BLOCK)
            .map(|c| {
                c.iter()
                    .flat_map(|s| {
                        let re = (s.re * 128.0).clamp(-127.0, 127.0) as i8;
                        let im = (s.im * 128.0).clamp(-127.0, 127.0) as i8;
                        [re as u8, im as u8]
                    })
                    .collect()
            })
            .collect();
        (blocks, AA)
    }

    /// Runs the worker on the LE Advertising view over `blocks`, leaving out
    /// the ones `skip` names (a lost block), and returns the state.
    fn follow_over(blocks: &[Vec<u8>], skip: &[usize]) -> SdrMetrics {
        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = "le".to_string();
        m.ui.active_preset = "net_ble".to_string();
        m.radio.frequency = 2_426_000_000;
        m.radio.config_sample_rate = 20e6;
        m.radio.bb_filter_hz = 0;
        let state = Arc::new(Mutex::new(m));
        let (tx, rx) = crossbeam_channel::unbounded();
        for (i, bytes) in blocks.iter().enumerate() {
            if !skip.contains(&i) {
                tx.send(stamped(&state, i as u64 + 1, false, bytes.clone()))
                    .unwrap();
            }
        }
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), eight_bit(), SAFE_BT_CHANNELS).run();
        let m = state.lock().unwrap().clone();
        m
    }

    /// **What following a connection costs, measured**, as
    /// `measure_the_receive_chain` measures the views: run by hand, in
    /// release, on the machine the question is about (`cargo test --release
    /// measure_following_a_connection -- --ignored --nocapture`). One
    /// connection at the shortest interval the Core allows (7.5 ms), a
    /// second of it, against `net_ble@20` in that probe for the same view
    /// with nothing to follow.
    #[test]
    #[ignore]
    fn measure_following_a_connection() {
        let (blocks, _) = a_connection_on_the_air(133);
        let start = std::time::Instant::now();
        let m = follow_over(&blocks, &[]);
        let wall = start.elapsed().as_secs_f64();
        let stream = blocks.iter().map(|b| b.len() / 2).sum::<usize>() as f64 / 20e6;
        let c = &m.net.ble_connections[0].connection;
        let followed = c
            .events()
            .iter()
            .filter(|e| e.account == crate::signal::ble::follow::Account::Followed)
            .count();
        eprintln!(
            "following at 20 Msps: {} events, {followed} followed, {:.2}x real time",
            c.events().len(),
            wall / stream
        );
    }

    /// A recording at `path` (ci8 at 20 Msps tuned to 2426 MHz) through the
    /// worker on `section`'s `preset`, locked, every block taken: the feed
    /// waits for the worker, so nothing is lost to load. The state it left,
    /// and how many blocks went in.
    fn replay(path: &str, section: &str, preset: &str) -> (SdrMetrics, u64) {
        use std::io::Read;
        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = section.to_string();
        m.ui.active_preset = preset.to_string();
        m.net.mode = crate::state::NetMode::Lock;
        m.radio.frequency = 2_426_000_000;
        m.radio.config_sample_rate = 20e6;
        m.radio.bb_filter_hz = 0;
        let state = Arc::new(Mutex::new(m));
        let (tx, rx) = crossbeam_channel::bounded(4);
        let st = Arc::clone(&state);
        let worker = std::thread::spawn(move || {
            NetWorker::new(rx, st, eight_bit(), SAFE_BT_CHANNELS).run();
        });
        let mut f = std::fs::File::open(path).unwrap();
        let mut seq = 1u64;
        loop {
            let mut buf = vec![0u8; 131_072 * 2];
            let mut got = 0;
            while got < buf.len() {
                let n = f.read(&mut buf[got..]).unwrap();
                if n == 0 {
                    break;
                }
                got += n;
            }
            if got < buf.len() {
                break;
            }
            tx.send(stamped(&state, seq, false, buf)).unwrap();
            seq += 1;
        }
        drop(tx);
        worker.join().unwrap();
        let m = state.lock().unwrap().clone();
        (m, seq - 1)
    }

    /// Counts the LE 1M `ADV_EXT_IND`s on channel 38 in a recording
    /// (`SDRTOP_REPLAY=path.sigmf-data`, ci8 at 20 Msps tuned to 2426 MHz)
    /// and where their AuxPtrs point: how much following them would have to
    /// listen to. By hand, in release.
    #[test]
    #[ignore]
    fn count_1m_extended_advertising() {
        use crate::signal::ble::aux_ptr::AuxPhy;
        use std::io::Read;
        let Ok(path) = std::env::var("SDRTOP_REPLAY") else {
            return;
        };
        let mut rx = crate::signal::ble::receive::Receiver::new(
            20e6,
            38,
            crate::signal::ble::Phy::OneM,
            2_426e6,
        )
        .unwrap();
        let mut f = std::fs::File::open(&path).unwrap();
        let (mut at, mut all, mut ext, mut ok) = (0u64, 0u64, 0u64, 0u64);
        let mut by_phy = std::collections::BTreeMap::new();
        let (mut in_view, mut out_of_view, mut none) = (0u64, 0u64, 0u64);
        let mut senders = std::collections::BTreeSet::new();
        let mut buf = vec![0u8; 131_072 * 2];
        let mut iq = Vec::new();
        let started = std::time::Instant::now();
        while f.read_exact(&mut buf).is_ok() {
            crate::signal::demod::decode(&buf, eight_bit(), usize::MAX, &mut iq);
            for p in rx.push_iq_at(&iq, at) {
                all += 1;
                if p.pdu_type != crate::signal::ble::pdu::PduType::Other(0x07) {
                    continue;
                }
                ext += 1;
                if !p.crc_ok {
                    continue;
                }
                ok += 1;
                let Ok(h) = crate::signal::ble::ext::parse(&p.payload) else {
                    continue;
                };
                if let Some(a) = h.adi {
                    senders.insert((a.sid, a.did));
                }
                match h.aux_ptr.filter(|a| !a.promises_nothing()) {
                    None => none += 1,
                    Some(a) => {
                        let phy = match a.phy {
                            Some(AuxPhy::OneM) => "1M",
                            Some(AuxPhy::TwoM) => "2M",
                            Some(AuxPhy::Coded) => "Coded",
                            None => "reserved",
                        };
                        *by_phy.entry(phy).or_insert(0u64) += 1;
                        if (7..=14).contains(&a.channel) {
                            in_view += 1;
                        } else {
                            out_of_view += 1;
                        }
                    }
                }
            }
            at += 131_072;
        }
        let secs = at as f64 / 20e6;
        eprintln!(
            "{secs:.1} s · {all} packets on 38 · {ext} ADV_EXT_IND ({ok} CRC good, {:.2}/s) · \
             AuxPtr by PHY {by_phy:?} · to 7..=14 {in_view}, elsewhere {out_of_view}, none {none} · \
             {} sets · {:.1} s to read",
            ok as f64 / secs,
            senders.len(),
            started.elapsed().as_secs_f64()
        );
    }

    /// Replays a recording (`SDRTOP_REPLAY=path.sigmf-data`, ci8 at 20 Msps
    /// tuned to 2426 MHz) of a phone advertising on LE Coded through the
    /// worker on the LE Coded view, locked on channel 38, and prints what it
    /// read. By hand, in release: the phone's `ADV_EXT_IND`s are decoded,
    /// at least one AuxPtr is followed to its `AUX_ADV_IND`, the phone's
    /// name is read from it, and every trigger ends once.
    #[test]
    #[ignore]
    fn replay_a_coded_recording() {
        let Ok(path) = std::env::var("SDRTOP_REPLAY") else {
            return;
        };
        let (m, blocks) = replay(&path, "coded", "net_coded");
        let f = m.net.health.coded;
        let a = m.net.health.aux;
        let kept: Vec<_> = m.net.coded_packets.iter().collect();
        let role = |p: &&BlePacket| p.ext.as_ref().map(|e| e.role.label());
        let primaries = kept
            .iter()
            .filter(|p| role(p) == Some("ADV_EXT_IND"))
            .count();
        let auxes: Vec<_> = kept
            .iter()
            .filter(|p| role(p) == Some("AUX_ADV_IND"))
            .collect();
        let names: std::collections::BTreeSet<String> = auxes
            .iter()
            .filter_map(|p| {
                let e = p.ext.as_ref()?;
                let structures = crate::signal::ble::ad::parse(&e.header.adv_data);
                crate::signal::ble::ad::name(&structures).map(|(n, _)| n.to_string())
            })
            .collect();
        let snr = |ps: &[&&BlePacket]| {
            let v: Vec<f64> = ps.iter().filter_map(|p| p.snr_db).collect();
            let lo = v.iter().cloned().fold(f64::INFINITY, f64::min);
            let hi = v.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            format!("{lo:.1} to {hi:.1} dB")
        };
        let prim: Vec<_> = kept
            .iter()
            .filter(|p| role(p) == Some("ADV_EXT_IND"))
            .collect();
        let repairs: Vec<u32> = kept
            .iter()
            .filter_map(|p| p.coded.as_ref().map(|c| c.fec_repairs))
            .collect();
        eprintln!(
            "blocks {blocks} · heard {} · kept {} ({primaries} ADV_EXT_IND, {} AUX_ADV_IND)",
            m.net.coded_heard,
            kept.len(),
            auxes.len()
        );
        eprintln!(
            "funnel: {} triggers · {} CRC ok · {} CRC failed · {} gave up",
            f.triggered, f.decoded, f.crc_failed, f.gave_up
        );
        eprintln!(
            "aux: {} heard · {} missed · {} not in view · {} feed lost · {} none promised · {} refused",
            a.heard, a.missed, a.not_in_view, a.feed_lost, a.none_promised, a.refused
        );
        eprintln!(
            "SNR primary {} · aux {} · FEC repairs max {} mean {:.2}",
            snr(&prim),
            snr(&auxes),
            repairs.iter().max().unwrap_or(&0),
            repairs.iter().sum::<u32>() as f64 / repairs.len().max(1) as f64
        );
        for p in auxes.iter().take(3) {
            eprintln!(
                "aux ch {} · {} · SNR {:?} · CFO {:?}",
                p.channel,
                p.phy.label(),
                p.snr_db,
                p.freq_offset_hz
            );
        }
        eprintln!("names {names:?}");
        assert!(primaries > 0, "no ADV_EXT_IND");
        assert!(a.heard > 0 && !auxes.is_empty(), "no AuxPtr followed");
        assert!(!names.is_empty(), "no name read");
        assert_eq!(f.triggered, f.decoded + f.crc_failed + f.gave_up, "{f:?}");
    }

    /// Replays a recording (`SDRTOP_REPLAY=path.sigmf-data`, ci8 at 20 Msps
    /// tuned to 2426 MHz) through the worker on LE 2, locked, and prints the
    /// connections followed. By hand, in release.
    #[test]
    #[ignore]
    fn replay_a_recording() {
        use std::io::Read;
        let Ok(path) = std::env::var("SDRTOP_REPLAY") else {
            return;
        };
        let (m, blocks) = replay(&path, "le", "net_ble");
        let connects = m
            .net
            .ble_packets
            .iter()
            .filter(|p| p.pdu_type == crate::signal::ble::pdu::PduType::ConnectInd)
            .count();
        eprintln!(
            "blocks {blocks} · packets kept {} · CONNECT_IND kept {connects}",
            m.net.ble_packets.len()
        );
        // A second pass, with no follower: every packet with the link's
        // access address on the eight data channels in view, and the
        // CONNECT_IND on 38, from `SDRTOP_REPLAY_FROM` pairs on.
        if let Some(f) = m.net.ble_connections.first() {
            use crate::signal::ble::receive::{Link, Receiver};
            let (aa, crc) = (f.connection.access_address(), f.connection.crc_init());
            let from: u64 = std::env::var("SDRTOP_REPLAY_FROM")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            let mut adv = Receiver::new(20e6, 38, crate::signal::ble::Phy::OneM, 2_426e6).unwrap();
            let mut links: Vec<(u8, Receiver)> = (7..=14u8)
                .map(|ch| {
                    (
                        ch,
                        Receiver::for_link(
                            20e6,
                            ch,
                            crate::signal::ble::Phy::OneM,
                            2_426e6,
                            Link::Data {
                                access_address: aa,
                                crc_init: crc,
                            },
                        )
                        .unwrap(),
                    )
                })
                .collect();
            let mut f = std::fs::File::open(&path).unwrap();
            use std::io::Seek;
            f.seek(std::io::SeekFrom::Start(from * 2)).unwrap();
            let mut at = from;
            let mut heard = Vec::new();
            loop {
                let mut buf = vec![0u8; 131_072 * 2];
                let mut got = 0;
                while got < buf.len() {
                    let n = f.read(&mut buf[got..]).unwrap();
                    if n == 0 {
                        break;
                    }
                    got += n;
                }
                if got < buf.len() {
                    break;
                }
                let mut iq = Vec::new();
                crate::signal::demod::decode(&buf, eight_bit(), usize::MAX, &mut iq);
                for p in adv.push_iq_at(&iq, at) {
                    if p.pdu_type == crate::signal::ble::pdu::PduType::ConnectInd {
                        let end = p.pdu_pair.map(|c| {
                            c + (crate::signal::ble::pdu::used_bits(p.length) as f64 - 0.5) * 20.0
                        });
                        eprintln!(
                            "CONNECT_IND crc {} pdu_pair {:?} end {:?}",
                            p.crc_ok, p.pdu_pair, end
                        );
                    }
                }
                for (ch, rx) in links.iter_mut() {
                    rx.push_iq_at(&iq, at);
                    for (d, timing) in rx.take_data() {
                        heard.push((timing.start_pair, *ch, d.llid, d.payload.len(), d.crc_ok));
                    }
                }
                at += 131_072;
                if at > from + 200_000_000 {
                    break;
                }
            }
            heard.sort_by(|a, b| a.0.total_cmp(&b.0));
            eprintln!("heard with AA {aa:08x}: {}", heard.len());
            for h in heard.iter().take(60) {
                eprintln!(
                    "  start {:.1} ch{} llid {} len {} crc {}",
                    h.0, h.1, h.2, h.3, h.4
                );
            }
        }
        for f in &m.net.ble_connections {
            let c = &f.connection;
            let p = c.params();
            eprintln!(
                "AA {:08x} CSA#{} interval {} win {}+{} sca {} state {:?}",
                c.access_address(),
                c.algorithm(),
                p.interval,
                p.win_offset,
                p.win_size,
                p.sca,
                c.state()
            );
            for e in c.events().iter().rev() {
                let ctl: Vec<String> = e
                    .pdus
                    .iter()
                    .filter_map(|h| {
                        h.control
                            .as_ref()
                            .map(|k| format!("{:?} {}", k.name, k.words))
                    })
                    .collect();
                if !ctl.is_empty() || (125..=150).contains(&e.counter) {
                    eprintln!(
                        "  {:>4} ch{:>2} {:?} {:?} {:?}",
                        e.counter,
                        e.channel,
                        e.account,
                        e.pdus
                            .iter()
                            .map(|h| (h.sender, h.pdu.llid, h.pdu.payload.len(), h.pdu.crc_ok))
                            .collect::<Vec<_>>(),
                        ctl
                    );
                }
            }
            eprintln!(
                "encrypted from {:?} · clock {:?} · t_ifs {:?}",
                c.encrypted_from(),
                c.clock_ppm(),
                c.t_ifs()
            );
        }
    }

    /// **The band is measured where it is shown.** On the survey a dwell of
    /// noise fills the band; on the Advertising view the same samples leave
    /// it unmeasured, and the receiver it was costing time runs alone.
    #[test]
    fn the_band_is_measured_on_the_survey_only() {
        use crate::signal::dsp::testkit::Rng;
        let block: Vec<u8> = Rng::new(7)
            .noise(131_072, 0.02)
            .iter()
            .flat_map(|z| {
                let q = |v: f32| (v * 128.0).clamp(-127.0, 127.0) as i8 as u8;
                [q(z.re), q(z.im)]
            })
            .collect();
        let run = |section: &str, preset: &str| {
            let mut m = SdrMetrics::fixture().streaming();
            m.ui.section = section.to_string();
            m.ui.active_preset = preset.to_string();
            m.radio.frequency = 2_426_000_000;
            m.radio.config_sample_rate = 20e6;
            m.radio.bb_filter_hz = 0;
            let state = Arc::new(Mutex::new(m));
            let (tx, rx) = crossbeam_channel::unbounded();
            // A dwell is 50 ms of windows, a block 6.5 ms: a dozen is enough.
            for seq in 1..=12 {
                tx.send(stamped(&state, seq, false, block.clone())).unwrap();
            }
            drop(tx);
            NetWorker::new(rx, Arc::clone(&state), eight_bit(), SAFE_BT_CHANNELS).run();
            let m = state.lock().unwrap().clone();
            m
        };
        let survey = run("net", super::super::lock::SURVEY_VIEW);
        assert!(
            !survey.net.band.cells.is_empty(),
            "the survey measures the band"
        );
        let ble = run("le", "net_ble");
        assert!(ble.net.band.cells.is_empty(), "LE 2 does not");
        assert_eq!(ble.net.ble_channel, Some(38), "and its receiver still runs");
    }

    /// An LE Coded S=8 `ADV_EXT_IND` on channel 38 at 20 Msps, the radio at
    /// 2426 MHz, starting at pair `start` of `blocks` blocks of 131 072 pairs,
    /// as the radio's eight-bit bytes.
    fn coded_on_the_air(start: usize, blocks: usize) -> Vec<Vec<u8>> {
        use crate::signal::ble::coded::{self, Coding};
        use crate::signal::ble::detect::ADVERTISING_ACCESS_ADDRESS;
        let symbols = coded::transmit(
            ADVERTISING_ACCESS_ADDRESS,
            Coding::S8,
            38,
            0x07,
            &[6, 0b0001_1000, 0x23, 0x31, 0x09, 0x64, 0x40],
        );
        let wave = crate::signal::ble::gfsk::modulate(&symbols, 20, 250e3, 20e6, 0.5);
        let len = 131_072;
        let mut iq = vec![num_complex::Complex::new(0.0f32, 0.0); len * blocks];
        let mut rng = crate::signal::dsp::testkit::Rng::new(5);
        for (z, n) in iq.iter_mut().zip(rng.noise(len * blocks, 0.0005)) {
            *z = n;
        }
        for (k, w) in wave.iter().enumerate() {
            iq[start + k] += w * 0.5;
        }
        let q = |v: f32| (v * 128.0).clamp(-127.0, 127.0) as i8 as u8;
        iq.chunks(len)
            .map(|c| c.iter().flat_map(|z| [q(z.re), q(z.im)]).collect())
            .collect()
    }

    fn run_view(section: &str, preset: &str, blocks: &[(u64, Vec<u8>)]) -> SdrMetrics {
        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = section.to_string();
        m.ui.active_preset = preset.to_string();
        m.radio.frequency = 2_426_000_000;
        m.radio.config_sample_rate = 20e6;
        m.radio.bb_filter_hz = 0;
        let state = Arc::new(Mutex::new(m));
        let (tx, rx) = crossbeam_channel::unbounded();
        for (seq, bytes) in blocks {
            tx.send(stamped(&state, *seq, false, bytes.clone()))
                .unwrap();
        }
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), eight_bit(), SAFE_BT_CHANNELS).run();
        let m = state.lock().unwrap().clone();
        m
    }

    /// Coded S=8 packets on the air at 20 Msps, the radio at 2426 MHz:
    /// `(channel, header octet 0, payload, first pair)` each, in `blocks`
    /// blocks of 131 072 pairs, as the radio's eight-bit bytes.
    fn coded_scene(packets: &[(u8, u8, Vec<u8>, usize)], blocks: usize) -> Vec<Vec<u8>> {
        use crate::signal::ble::coded::{self, Coding};
        use crate::signal::ble::detect::ADVERTISING_ACCESS_ADDRESS;
        let len = 131_072;
        let mut rng = crate::signal::dsp::testkit::Rng::new(6);
        let mut iq = rng.noise(len * blocks, 0.0005);
        for (ch, byte0, payload, start) in packets {
            let symbols =
                coded::transmit(ADVERTISING_ACCESS_ADDRESS, Coding::S8, *ch, *byte0, payload);
            let wave = crate::signal::ble::gfsk::modulate(&symbols, 20, 250e3, 20e6, 0.5);
            let shift = crate::signal::ble::channel::centre_hz(*ch).unwrap() as f64 - 2.426e9;
            let step = std::f64::consts::TAU * shift / 20e6;
            for (k, w) in wave.iter().enumerate() {
                let rot = num_complex::Complex::from_polar(0.5, (step * (start + k) as f64) as f32);
                iq[start + k] += w * rot;
            }
        }
        let q = |v: f32| (v * 128.0).clamp(-127.0, 127.0) as i8 as u8;
        iq.chunks(len)
            .map(|c| c.iter().flat_map(|z| [q(z.re), q(z.im)]).collect())
            .collect()
    }

    /// An ADV_EXT_IND's payload: ADI 0x3123 (SID 3, DID 0x123) and an
    /// AuxPtr to `channel`, 100 units of 30 us on, LE Coded, CA 1.
    fn adv_ext_ind(channel: u8) -> Vec<u8> {
        let v: u32 = channel as u32 | 1 << 6 | 100 << 8 | 0b010 << 21;
        vec![
            6,
            0b0001_1000,
            0x23,
            0x31,
            v as u8,
            (v >> 8) as u8,
            (v >> 16) as u8,
        ]
    }

    /// Its AUX_ADV_IND's: AdvA 66:55:44:33:22:11, the same ADI, the name.
    fn aux_adv_ind() -> Vec<u8> {
        let mut payload = vec![
            9,
            0b0000_1001,
            0x11,
            0x22,
            0x33,
            0x44,
            0x55,
            0x66,
            0x23,
            0x31,
        ];
        payload.extend([6, 0x09, b'P', b'i', b'x', b'e', b'l']);
        payload
    }

    fn scene_view(packets: &[(u8, u8, Vec<u8>, usize)]) -> SdrMetrics {
        let blocks: Vec<(u64, Vec<u8>)> = coded_scene(packets, 4)
            .into_iter()
            .enumerate()
            .map(|(i, b)| (i as u64 + 1, b))
            .collect();
        run_view("coded", "net_coded", &blocks)
    }

    /// **An LE Coded advertisement is followed to its auxiliary packet.**
    /// The ADV_EXT_IND on 38 points 3000 us on to data channel 9, in view;
    /// the AUX_ADV_IND there is heard, bound to it, and names the advertiser.
    #[test]
    fn a_coded_advertisement_is_followed_to_its_aux() {
        use crate::signal::ble::aux_ptr::AuxOutcome;
        let m = scene_view(&[
            (38, 0x07, adv_ext_ind(9), 20_000),
            (9, 0x47, aux_adv_ind(), 20_000 + 60_000),
        ]);
        assert_eq!(m.net.coded_packets.len(), 2, "{:?}", m.net.coded_packets);
        let aux = &m.net.coded_packets[0];
        let superior = &m.net.coded_packets[1];
        let aux_ext = aux.ext.as_ref().expect("read as extended");
        assert_eq!(
            aux_ext.role,
            crate::state::ExtRole::AuxAdv {
                superior_seq: Some(superior.seq)
            }
        );
        assert_eq!(aux.channel, 9);
        assert_eq!(aux.adv_addr, Some([0x66, 0x55, 0x44, 0x33, 0x22, 0x11]));
        match superior.ext.as_ref().expect("read as extended").aux {
            AuxOutcome::Heard { seq, after_us } => {
                assert_eq!(seq, aux.seq);
                assert!((after_us - 3000.0).abs() < 31.0, "{after_us}");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(m.net.health.aux.heard, 1);
    }

    /// An AuxPtr to a channel the radio does not see is said to be one.
    #[test]
    fn an_aux_out_of_view_is_said_to_be() {
        use crate::signal::ble::aux_ptr::AuxOutcome;
        let m = scene_view(&[(38, 0x07, adv_ext_ind(30), 20_000)]);
        assert_eq!(m.net.coded_packets.len(), 1);
        let ext = m.net.coded_packets[0].ext.as_ref().unwrap();
        assert_eq!(ext.aux, AuxOutcome::NotInView);
        assert_eq!(m.net.health.aux.not_in_view, 1);
    }

    /// An AuxPtr to LE 2M at 20 Msps, where no LE 2M receiver can be
    /// built: not followed, and why, rather than "its samples were not held".
    #[test]
    fn an_aux_on_a_phy_this_rate_cannot_receive_is_refused() {
        use crate::signal::ble::aux_ptr::AuxOutcome;
        let mut payload = adv_ext_ind(9);
        // The AuxPtr's PHY bits, 21 to 23 of its three octets: LE 2M.
        payload[6] = (payload[6] & 0b0001_1111) | 0b001 << 5;
        let m = scene_view(&[(38, 0x07, payload, 20_000)]);
        let ext = m.net.coded_packets[0].ext.as_ref().unwrap();
        assert!(matches!(ext.aux, AuxOutcome::Refused(_)), "{:?}", ext.aux);
        assert_eq!(m.net.health.aux.feed_lost, 0);
        assert_eq!(m.net.health.aux.refused, 1);
    }

    /// In view, listened to, and nothing there: missed.
    #[test]
    fn an_aux_not_sent_is_missed() {
        use crate::signal::ble::aux_ptr::AuxOutcome;
        let m = scene_view(&[(38, 0x07, adv_ext_ind(9), 20_000)]);
        let ext = m.net.coded_packets[0].ext.as_ref().unwrap();
        assert_eq!(ext.aux, AuxOutcome::Missed);
        assert_eq!(m.net.health.aux.missed, 1);
    }

    /// LE 1M advertising packets on the air at 20 Msps, the radio at 2426
    /// MHz: `(channel, header octet 0, payload, first pair)` each, in
    /// `blocks` blocks, as the radio's eight-bit bytes.
    fn le_scene(packets: &[(u8, u8, Vec<u8>, usize)], blocks: usize) -> Vec<Vec<u8>> {
        use crate::signal::ble::detect::{
            access_address_bits, preamble_bits, ADVERTISING_ACCESS_ADDRESS,
        };
        let len = 131_072;
        let mut iq = crate::signal::dsp::testkit::Rng::new(8).noise(len * blocks, 0.0005);
        for (ch, byte0, payload, start) in packets {
            let mut bits = preamble_bits(ADVERTISING_ACCESS_ADDRESS, crate::signal::ble::Phy::OneM);
            bits.extend_from_slice(&access_address_bits(ADVERTISING_ACCESS_ADDRESS));
            bits.extend(crate::signal::ble::pdu::encode(*ch, *byte0, payload));
            let wave = crate::signal::ble::gfsk::modulate(&bits, 20, 250e3, 20e6, 0.5);
            let shift = crate::signal::ble::channel::centre_hz(*ch).unwrap() as f64 - 2.426e9;
            let step = std::f64::consts::TAU * shift / 20e6;
            for (k, w) in wave.iter().enumerate() {
                let rot = num_complex::Complex::from_polar(0.5, (step * (start + k) as f64) as f32);
                iq[start + k] += w * rot;
            }
        }
        let q = |v: f32| (v * 128.0).clamp(-127.0, 127.0) as i8 as u8;
        iq.chunks(len)
            .map(|c| c.iter().flat_map(|z| [q(z.re), q(z.im)]).collect())
            .collect()
    }

    /// The LE Advertising view over `le_scene`'s packets, four blocks.
    fn le_view(packets: &[(u8, u8, Vec<u8>, usize)]) -> SdrMetrics {
        let blocks: Vec<(u64, Vec<u8>)> = le_scene(packets, 4)
            .into_iter()
            .enumerate()
            .map(|(i, b)| (i as u64 + 1, b))
            .collect();
        run_view("le", "net_ble", &blocks)
    }

    /// `adv_ext_ind(channel)` with its AuxPtr's PHY set to `phy` (Table
    /// 2.14's three bits: 0 LE 1M, 1 LE 2M, 2 LE Coded).
    fn adv_ext_ind_on(channel: u8, phy: u8) -> Vec<u8> {
        let mut payload = adv_ext_ind(channel);
        payload[6] = (payload[6] & 0b0001_1111) | phy << 5;
        payload
    }

    /// **An LE 1M advertisement is followed to its auxiliary packet**, as an
    /// LE Coded one is: the `ADV_EXT_IND` on 38 points 3000 us on to data
    /// channel 9 on LE 1M; the `AUX_ADV_IND` there is heard, joins the LE
    /// list bound to it, and names the advertiser.
    #[test]
    fn a_1m_adv_ext_ind_is_followed_to_its_aux() {
        use crate::signal::ble::aux_ptr::AuxOutcome;
        let m = le_view(&[
            (38, 0x07, adv_ext_ind_on(9, 0), 20_000),
            (9, 0x47, aux_adv_ind(), 20_000 + 60_000),
        ]);
        let list: Vec<_> = m.net.ble_packets.iter().collect();
        assert_eq!(list.len(), 2, "{list:?}");
        let (aux, superior) = (list[0], list[1]);
        assert_eq!(superior.channel, 38);
        let aux_ext = aux.ext.as_ref().expect("read as extended");
        assert_eq!(
            aux_ext.role,
            crate::state::ExtRole::AuxAdv {
                superior_seq: Some(superior.seq)
            }
        );
        assert_eq!(aux.channel, 9);
        assert_eq!(aux.phy, crate::signal::ble::Phy::OneM);
        assert_eq!(aux.adv_addr, Some([0x66, 0x55, 0x44, 0x33, 0x22, 0x11]));
        let sup_ext = superior.ext.as_ref().expect("read as extended");
        assert!(
            matches!(
                sup_ext.aux,
                AuxOutcome::Heard { seq, after_us } if seq == aux.seq && (after_us - 3000.0).abs() < 5.0
            ),
            "{:?}",
            sup_ext.aux
        );
        assert_eq!(m.net.health.aux.heard, 1);
        // Nothing on the LE Coded list: one advertisement, one list.
        assert!(m.net.coded_packets.is_empty());
    }

    /// An LE 1M capture the worker cuts short at a break still ends in the
    /// funnel, as LE Coded's does: every trigger decoded, failed or given up.
    #[test]
    fn a_ble_capture_cut_by_a_break_still_ends_in_the_funnel() {
        let payload: Vec<u8> = (0..31u8).collect();
        // The packet (about 6 600 pairs) straddles the first block's end.
        let blocks = le_scene(&[(38, 0x02, payload, 131_072 - 2_000)], 3);
        let sent = vec![(1, blocks[0].clone()), (3, blocks[2].clone())];
        let m = run_view("le", "net_ble", &sent);
        let f = m.net.health.ble;
        assert!(f.gave_up >= 1, "{f:?}");
        assert_eq!(f.triggered, f.decoded + f.crc_failed + f.gave_up, "{f:?}");
    }

    /// **An extended advertiser is counted in the census.** Its address and
    /// name are in the auxiliary packet, not the `ADV_EXT_IND`, so the
    /// followed aux is what counts it: by its AdvA, with what its AdvData
    /// says, and with no arrival, since an aux's timing is its primary's
    /// offset, not an advertising interval.
    #[test]
    fn an_extended_advertiser_is_counted_in_the_census() {
        let m = le_view(&[
            (38, 0x07, adv_ext_ind_on(9, 0), 20_000),
            (9, 0x47, aux_adv_ind(), 20_000 + 60_000),
        ]);
        let addr = [0x66, 0x55, 0x44, 0x33, 0x22, 0x11];
        let devices = &m.net.census.devices;
        assert_eq!(devices.len(), 1, "{devices:?}");
        let d = &devices[0];
        assert_eq!(d.address, addr);
        assert_eq!(d.ble_pdu_codes().collect::<Vec<_>>(), [0x07]);
        let said = m.net.advertised.get(&addr).expect("its AdvData read");
        assert_eq!(said.name.as_ref().map(|n| n.0.as_str()), Some("Pixel"));
    }

    /// An LE 1M advertisement whose AuxPtr is out of view says so, on the
    /// LE list, and an LE 2M one at 20 Msps is not followed, and why.
    #[test]
    fn a_1m_adv_ext_ind_says_what_became_of_its_aux() {
        use crate::signal::ble::aux_ptr::AuxOutcome;
        let m = le_view(&[(38, 0x07, adv_ext_ind_on(30, 0), 20_000)]);
        let ext = m.net.ble_packets[0].ext.as_ref().expect("read as extended");
        assert_eq!(ext.aux, AuxOutcome::NotInView);
        let m = le_view(&[(38, 0x07, adv_ext_ind_on(9, 1), 20_000)]);
        let ext = m.net.ble_packets[0].ext.as_ref().expect("read as extended");
        assert!(matches!(ext.aux, AuxOutcome::Refused(_)), "{:?}", ext.aux);
    }

    /// **Samples from another tuning are not listened to.** A retune keeps
    /// the stream's positions running, so a promise whose window comes after
    /// it would be cut from samples mixed for the old centre. The ADV_EXT_IND
    /// points 6 ms on (20 units of 300 us), past its block; the next block
    /// arrives retuned by 2 MHz, and the promise ends not followed, and why.
    #[test]
    fn a_promise_across_a_retune_is_not_followed() {
        use crate::signal::ble::aux_ptr::AuxOutcome;
        let v: u32 = 9 | 1 << 6 | 1 << 7 | 20 << 8;
        let payload = vec![
            6,
            0b0001_1000,
            0x23,
            0x31,
            v as u8,
            (v >> 8) as u8,
            (v >> 16) as u8,
        ];
        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = "le".to_string();
        m.ui.active_preset = "net_ble".to_string();
        m.radio.frequency = 2_426_000_000;
        m.radio.config_sample_rate = 20e6;
        m.radio.bb_filter_hz = 0;
        let state = Arc::new(Mutex::new(m));
        let (tx, rx) = crossbeam_channel::unbounded();
        for (i, bytes) in le_scene(&[(38, 0x07, payload, 20_000)], 3)
            .into_iter()
            .enumerate()
        {
            let mut block = stamped(&state, i as u64 + 1, false, bytes);
            if i > 0 {
                block.centre_hz = 2_428_000_000;
            }
            tx.send(block).unwrap();
        }
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), eight_bit(), SAFE_BT_CHANNELS).run();
        let m = state.lock().unwrap();
        let ext = m.net.ble_packets[0].ext.as_ref().expect("read as extended");
        assert_eq!(
            ext.aux,
            AuxOutcome::Refused("the radio retuned before its window")
        );
        assert_eq!(m.net.health.aux.refused, 1);
    }

    /// **The LE Coded view runs LE Coded's receiver, and only it.** A Coded
    /// advertisement on channel 38 is one row in the Coded list, its scheme,
    /// repairs and measurement with it; the LE 1M list stays empty and its
    /// receiver gets no channel. On LE 2 the same samples give no Coded row.
    #[test]
    fn the_coded_view_runs_the_coded_receiver_alone() {
        let blocks: Vec<(u64, Vec<u8>)> = coded_on_the_air(20_000, 2)
            .into_iter()
            .enumerate()
            .map(|(i, b)| (i as u64 + 1, b))
            .collect();
        let m = run_view("coded", "net_coded", &blocks);
        assert_eq!(m.net.coded_channel, Some(38));
        assert_eq!(m.net.coded_packets.len(), 1, "{:?}", m.net.coded_refused);
        let p = &m.net.coded_packets[0];
        assert_eq!(
            p.phy,
            crate::signal::ble::Phy::Coded(crate::signal::ble::coded::Coding::S8)
        );
        assert!(p.crc_ok);
        let facts = p.coded.as_ref().expect("an LE Coded packet's facts");
        assert!(facts.reading.and_then(|r| r.snr_db).is_some(), "measured");
        assert!(m.net.ble_packets.is_empty());
        assert_eq!(m.net.ble_channel, None);
        assert_eq!(m.net.health.coded.decoded, 1);

        let le2 = run_view("le", "net_ble", &blocks);
        assert!(le2.net.coded_packets.is_empty());
    }

    /// **A gap inside a Coded packet is not stitched.** The block that held
    /// its middle never arrives; what is left on either side is no packet.
    #[test]
    fn a_gap_inside_a_coded_capture_is_not_stitched() {
        let blocks = coded_on_the_air(120_000, 3);
        let sent = vec![(1, blocks[0].clone()), (3, blocks[2].clone())];
        let m = run_view("coded", "net_coded", &sent);
        assert!(m.net.coded_packets.is_empty(), "{:?}", m.net.coded_packets);
    }

    /// A capture the worker cuts short at a break still ends in the funnel:
    /// every trigger is decoded, failed or given up, and the counts add up.
    #[test]
    fn a_capture_cut_by_a_break_still_ends_in_the_funnel() {
        let blocks = coded_on_the_air(120_000, 3);
        let sent = vec![(1, blocks[0].clone()), (3, blocks[2].clone())];
        let m = run_view("coded", "net_coded", &sent);
        let f = m.net.health.coded;
        assert!(f.gave_up >= 1, "{f:?}");
        assert_eq!(f.triggered, f.decoded + f.crc_failed + f.gave_up, "{f:?}");
    }

    /// **A connection heard set up is followed.** Its CONNECT_IND on
    /// channel 38 starts it; every event on a channel the 20 MHz window
    /// holds (data channels 7 to 14) is followed, its Central placed at the
    /// anchor and its Peripheral T_IFS after; the rest are out of view.
    #[test]
    fn a_connection_heard_set_up_is_followed() {
        use crate::signal::ble::follow::{Account, Sender};
        let (blocks, aa) = a_connection_on_the_air(24);
        let m = follow_over(&blocks, &[]);
        assert_eq!(m.net.ble_connections.len(), 1);
        let c = &m.net.ble_connections[0].connection;
        assert_eq!(c.access_address(), aa);
        let events = c.events();
        assert!(events.len() >= 20, "{} accounted", events.len());
        for e in events {
            let in_view = (7..=14).contains(&e.channel);
            assert_eq!(e.account == Account::NotInView, !in_view, "{e:?}");
            if in_view {
                assert_eq!(e.account, Account::Followed, "{e:?}");
                let senders: Vec<_> = e.pdus.iter().map(|p| p.sender).collect();
                assert_eq!(
                    senders,
                    [Some(Sender::Central), Some(Sender::Peripheral)],
                    "{e:?}"
                );
                let t_ifs = e.pdus[1].t_ifs_us.unwrap();
                assert!((t_ifs - 150.0).abs() < 1.0, "{t_ifs}");
            }
        }
        assert!(
            events
                .iter()
                .filter(|e| e.account == Account::Followed)
                .count()
                >= 5
        );
    }

    /// **The advertising PDU's ChSel counts too.** A CONNECT_IND with ChSel
    /// set answering an ADV_DIRECT_IND without it is a CSA #1 connection,
    /// and is followed as one; with the advertising PDU never heard, which
    /// algorithm is unknown and the connection says so.
    #[test]
    fn the_answered_advertisings_chsel_picks_the_algorithm() {
        use crate::signal::ble::follow::{Account, State};
        let (blocks, _) = a_connection_with_chsel(24, true, Some(false));
        let m = follow_over(&blocks, &[]);
        let c = &m.net.ble_connections[0].connection;
        assert_eq!(c.algorithm(), 1);
        assert!(c.events().iter().any(|e| e.account == Account::Followed));

        let (blocks, _) = a_connection_with_chsel(24, true, None);
        let m = follow_over(&blocks, &[]);
        let c = &m.net.ble_connections[0].connection;
        assert!(
            matches!(c.state(), State::NotFollowed { .. }),
            "{:?}",
            c.state()
        );
    }

    /// A block lost inside an event's window: that event is not listened
    /// to, and says so; the ones after are followed again.
    #[test]
    fn an_event_in_a_lost_block_is_feed_lost() {
        use crate::signal::ble::follow::Account;
        let (blocks, _) = a_connection_on_the_air(24);
        // Event 6, on channel 12, has its anchor in the eighth block.
        let m = follow_over(&blocks, &[7]);
        let c = &m.net.ble_connections[0].connection;
        let six = c.events().iter().find(|e| e.counter == 6).unwrap();
        assert_eq!(six.account, Account::FeedLost, "{six:?}");
        let later = c.events().iter().find(|e| e.counter == 11).unwrap();
        assert_eq!(later.account, Account::Followed, "{later:?}");
    }

    /// Surveying, a position whose centre is not a BLE channel still decodes
    /// the advertising channel it holds: tuned to 2403.5 MHz, the first
    /// position of an 8 Msps pass, a channel-37 packet 1.5 MHz below the
    /// centre is heard as channel 37. The tuning alone gave data channel 0
    /// there, and 37 was never decoded in a survey.
    #[test]
    fn a_survey_position_decodes_the_advertising_channel_it_holds() {
        use crate::signal::ble::detect::{
            access_address_bits, preamble_bits, ADVERTISING_ACCESS_ADDRESS,
        };
        use crate::signal::ble::gfsk::modulate;
        use crate::signal::ble::pdu::encode;
        use crate::signal::ble::Phy;
        use crate::signal::dsp::nco::Nco;
        use crate::signal::dsp::testkit::{at_snr, Rng};
        use num_complex::Complex;

        const RATE: f64 = 8_000_000.0;
        let tuned = 2_403_500_000u64;
        let addr = [0x21u8, 0x32, 0x43, 0x54, 0x65, 0x76];
        let mut bits: Vec<bool> = (0..32).map(|i| i % 3 == 0).collect();
        bits.extend(preamble_bits(ADVERTISING_ACCESS_ADDRESS, Phy::OneM));
        bits.extend_from_slice(&access_address_bits(ADVERTISING_ACCESS_ADDRESS));
        bits.extend_from_slice(&encode(
            37,
            0x00,
            &crate::signal::ble::pdu::air_octets(addr),
        ));
        let mut rng = Rng::new(3);
        bits.extend((0..64).map(|_| rng.next_u64() & 1 == 1));
        let mut iq = modulate(&bits, 8, Phy::OneM.deviation_hz(), RATE, 0.5);
        let offset = crate::signal::ble::channel::centre_hz(37).unwrap() as f64 - tuned as f64;
        Nco::new(offset, RATE).mix(&mut iq);
        let noisy = at_snr(&iq, 25.0, &mut Rng::new(4));
        let geometry = eight_bit();
        let bytes: Vec<u8> = noisy
            .iter()
            .flat_map(|s: &Complex<f32>| {
                let re = (s.re * geometry.full_scale).clamp(-127.0, 127.0) as i8;
                let im = (s.im * geometry.full_scale).clamp(-127.0, 127.0) as i8;
                [re as u8, im as u8]
            })
            .collect();

        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = crate::signal::net::SECTION.to_string();
        m.ui.active_preset = "net_survey".to_string();
        m.radio.frequency = tuned;
        m.radio.config_sample_rate = RATE;
        m.radio.bb_filter_hz = 0;
        let state = Arc::new(Mutex::new(m));
        let (tx, rx) = crossbeam_channel::unbounded();
        tx.send(stamped(&state, 1, false, bytes)).unwrap();
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), geometry, SAFE_BT_CHANNELS).run();

        let m = state.lock().unwrap();
        assert_eq!(m.net.ble_channel, Some(37));
        assert_eq!(m.net.ble_packets.len(), 1, "{:?}", m.net.health.ble);
        let p = &m.net.ble_packets[0];
        assert_eq!(p.channel, 37);
        assert!(p.crc_ok);
        assert_eq!(p.adv_addr, Some(addr));
    }

    /// **LE 2M, run through the worker**: on a data channel a 2M packet is
    /// decoded, stamped as LE 2M, and measured like any other; on an
    /// advertising channel the decoder does not run at all and says why.
    #[test]
    fn le_2m_is_decoded_off_the_advertising_channels_and_refused_on_them() {
        let data_ch = 10u8;
        let (bytes, addr) = ble_2m_bytes(data_ch);
        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = crate::signal::net::SECTION.to_string();
        m.radio.frequency = crate::signal::ble::channel::centre_hz(data_ch).unwrap();
        m.radio.config_sample_rate = 8_000_000.0;
        m.radio.bb_filter_hz = 0;
        m.net.ble_phy = crate::signal::ble::Phy::TwoM;
        let state = Arc::new(Mutex::new(m));
        let (tx, rx) = crossbeam_channel::unbounded();
        tx.send(stamped(&state, 1, false, bytes)).unwrap();
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), eight_bit(), SAFE_BT_CHANNELS).run();
        {
            let m = state.lock().unwrap();
            assert!(m.net.ble_refused.is_none(), "{:?}", m.net.ble_refused);
            assert_eq!(m.net.ble_packets.len(), 1, "{:?}", m.net.health.ble);
            let p = &m.net.ble_packets[0];
            assert_eq!(p.phy, crate::signal::ble::Phy::TwoM);
            assert_eq!(p.adv_addr, Some(addr));
            assert!(p.crc_ok);
            // Measured on LE 2M's own clock.
            assert!(p.drift.is_some(), "{p:?}");
        }

        let (bytes, _) = ble_2m_bytes(37);
        {
            let mut m = state.lock().unwrap();
            m.radio.frequency = crate::signal::ble::channel::centre_hz(37).unwrap();
            m.net.ble_packets.clear();
        }
        let (tx, rx) = crossbeam_channel::unbounded();
        tx.send(stamped(&state, 1, false, bytes)).unwrap();
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), eight_bit(), SAFE_BT_CHANNELS).run();
        let m = state.lock().unwrap();
        let why = m.net.ble_refused.clone().unwrap_or_default();
        assert!(
            why.contains("LE 2M is not used on the primary advertising channels"),
            "{why}"
        );
        assert!(m.net.ble_packets.is_empty());
        assert_eq!(m.net.ble_channel, None);
    }

    /// The same synthetic ADV_IND the test above sends, as raw 4 Msps
    /// channel-37 bytes, and the state that goes with it.
    fn ble_packet_bytes() -> (Vec<u8>, [u8; 6], Arc<Mutex<SdrMetrics>>) {
        use crate::signal::ble::detect::{
            access_address_bits, preamble_bits, ADVERTISING_ACCESS_ADDRESS,
        };
        use crate::signal::ble::gfsk::modulate;
        use crate::signal::ble::pdu::encode;
        use crate::signal::ble::Phy;
        use crate::signal::dsp::testkit::{at_snr, Rng};
        use num_complex::Complex;

        let addr = [0x11u8, 0x22, 0x33, 0x44, 0x55, 0x66];
        let mut bits = preamble_bits(ADVERTISING_ACCESS_ADDRESS, Phy::OneM);
        bits.extend_from_slice(&access_address_bits(ADVERTISING_ACCESS_ADDRESS));
        bits.extend_from_slice(&encode(
            37,
            0x00,
            &crate::signal::ble::pdu::air_octets(addr),
        ));
        let mut rng = Rng::new(1);
        bits.extend((0..16).map(|_| rng.next_u64() & 1 == 1));
        let clean = modulate(&bits, 4, 250_000.0, 4_000_000.0, 0.5);
        let noisy = at_snr(&clean, 20.0, &mut Rng::new(2));
        let geometry = eight_bit();
        let bytes: Vec<u8> = noisy
            .iter()
            .flat_map(|s: &Complex<f32>| {
                let re = (s.re * geometry.full_scale).clamp(-127.0, 127.0) as i8;
                let im = (s.im * geometry.full_scale).clamp(-127.0, 127.0) as i8;
                [re as u8, im as u8]
            })
            .collect();

        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = crate::signal::net::SECTION.to_string();
        m.radio.frequency = 2_402_000_000;
        m.radio.config_sample_rate = 4_000_000.0;
        m.radio.bb_filter_hz = 0;
        (bytes, addr, Arc::new(Mutex::new(m)))
    }

    /// Run the worker over the packet cut in two, the second half placed at
    /// `second_at` in the stream, and count the packets that came out.
    fn split_packet(second_at_offset: u64) -> usize {
        let (bytes, _, state) = ble_packet_bytes();
        // Cut inside the header: the preamble and access address are in the
        // first half, the rest of the packet in the second.
        let cut = (8 + 32 + 8) * 4 * 2;
        let first_pairs = cut as u64 / 2;
        let (tx, rx) = crossbeam_channel::unbounded();
        for (seq, first_pair, part) in [
            (1, 0, bytes[..cut].to_vec()),
            (2, first_pairs + second_at_offset, bytes[cut..].to_vec()),
        ] {
            tx.send(StreamBlock {
                seq,
                gap_before: false,
                bytes: part,
                first_pair,
                centre_hz: 2_402_000_000,
                rate_hz: 4_000_000.0,
            })
            .unwrap();
        }
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), eight_bit(), SAFE_BT_CHANNELS).run();
        let n = state.lock().unwrap().net.ble_packets.len();
        n
    }

    /// Control: a packet cut across two *contiguous* blocks is still one
    /// packet - the receiver carries its state across a block boundary, as it
    /// must, since packets straddle boundaries all the time.
    #[test]
    fn a_packet_across_two_contiguous_blocks_is_still_decoded() {
        assert_eq!(split_packet(0), 1);
    }

    /// **The regression.** The same bytes, but the second block sits a
    /// thousand pairs further on in the stream - a refused block in between.
    /// The worker used to carry the capture straight across, and here that
    /// even yields a clean CRC, because this test's second half happens to be
    /// the true continuation; on a real radio it never is, and the splice was
    /// a packet made of two moments. A capture must never span a break.
    #[test]
    fn a_capture_never_spans_a_break_in_the_stream() {
        assert_eq!(split_packet(1_000), 0);
    }

    /// A block is decoded at the tuning it was captured at, not at wherever
    /// the radio has moved to by the time the worker reaches it.
    #[test]
    fn a_block_is_decoded_at_the_tuning_it_was_captured_at() {
        let (bytes, addr, state) = ble_packet_bytes();
        let block = stamped(&state, 1, false, bytes);
        // The survey moves on before the worker gets to the block.
        state.lock().unwrap().radio.frequency = 2_480_000_000;
        let (tx, rx) = crossbeam_channel::unbounded();
        tx.send(block).unwrap();
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), eight_bit(), SAFE_BT_CHANNELS).run();
        let m = state.lock().unwrap();
        assert_eq!(m.net.ble_packets.len(), 1, "{:?}", m.net.ble_packets);
        assert_eq!(m.net.ble_packets[0].channel, 37, "the capture's channel");
        assert_eq!(m.net.ble_packets[0].adv_addr, Some(addr));
        assert!(m.net.ble_packets[0].crc_ok);
    }

    /// Tuned off any advertising frequency, the worker says so rather than
    /// silently decoding nothing.
    #[test]
    fn an_off_channel_tuning_is_refused_with_a_reason() {
        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = crate::signal::net::SECTION.to_string();
        m.radio.frequency = 2_437_000_000; // Wi-Fi channel 6, not a BLE one
        m.radio.config_sample_rate = 4_000_000.0;
        let state = Arc::new(Mutex::new(m));
        let (tx, rx) = crossbeam_channel::unbounded();
        tx.send(stamped(&state, 1, false, vec![0u8; 256])).unwrap();
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), eight_bit(), SAFE_BT_CHANNELS).run();
        let m = state.lock().unwrap();
        assert!(m.net.ble_refused.is_some());
        assert_eq!(m.net.ble_channel, None, "no receiver, no channel");
        assert!(m.net.ble_packets.is_empty());
    }

    /// Closing the section drops the receiver and the load figure with it,
    /// so neither is shown on reopening as if it were still true.
    #[test]
    fn a_closed_section_leaves_no_receiver_or_load_behind() {
        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = "lab".to_string();
        m.radio.frequency = 2_402_000_000;
        m.radio.config_sample_rate = 4_000_000.0;
        m.net.ble_channel = Some(37);
        m.net.health.decode_load = Some(0.4);
        let state = Arc::new(Mutex::new(m));
        let (tx, rx) = crossbeam_channel::unbounded();
        tx.send(stamped(&state, 1, false, vec![0u8; 256])).unwrap();
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), eight_bit(), SAFE_BT_CHANNELS).run();
        let m = state.lock().unwrap();
        assert_eq!(m.net.ble_channel, None);
        assert_eq!(m.net.health.decode_load, None);
    }

    /// End to end through the worker: enough stream to cover a load window
    /// publishes a load. `feed`'s unit tests hold the arithmetic; this holds
    /// the wiring, which they cannot see.
    #[test]
    fn the_worker_publishes_a_load_once_a_window_of_stream_has_passed() {
        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = crate::signal::net::SECTION.to_string();
        m.radio.frequency = 2_437_000_000;
        m.radio.config_sample_rate = 1_000_000.0;
        let state = Arc::new(Mutex::new(m));
        let (tx, rx) = crossbeam_channel::unbounded();
        // Eight 100 000-pair blocks at 1 Msps: 0.8 s of stream.
        for seq in 1..=8 {
            tx.send(stamped(&state, seq, false, vec![0u8; 200_000]))
                .unwrap();
        }
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), eight_bit(), SAFE_BT_CHANNELS).run();
        let load = state.lock().unwrap().net.health.decode_load;
        assert!(load.is_some_and(|l| l > 0.0), "{load:?}");
    }

    /// A view too narrow for `signal::bt::receive::front_end`'s own working
    /// rate is refused, with a reason - the same "refused, not silent"
    /// discipline `ble_refused` already follows.
    #[test]
    fn a_view_too_narrow_for_the_working_rate_is_refused() {
        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = crate::signal::net::SECTION.to_string();
        m.ui.active_preset = "net_bt".to_string();
        m.radio.frequency = 2_437_000_000;
        m.radio.config_sample_rate = 2_000_000.0; // below the 4 Msps working rate
        let state = Arc::new(Mutex::new(m));
        let (tx, rx) = crossbeam_channel::unbounded();
        tx.send(stamped(&state, 1, false, vec![0u8; 256])).unwrap();
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), eight_bit(), SAFE_BT_CHANNELS).run();
        let m = state.lock().unwrap();
        assert!(m.net.bt_refused.is_some(), "{:?}", m.net.bt_refused);
        assert!(m.net.bt_hops.is_empty());
        assert!(m.net.bt_channels_watched.is_empty());
    }

    /// A view wide enough to hold real classic BT channels clears the
    /// refusal and says which channels it is actually watching - capped at
    /// [`SAFE_BT_CHANNELS`] here, not the full count a 20 MHz view could
    /// otherwise see, because the default config asks for no more.
    #[test]
    fn a_wide_enough_view_clears_the_refusal_and_names_the_watched_channels() {
        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = crate::signal::net::SECTION.to_string();
        m.ui.active_preset = "net_bt".to_string();
        m.radio.frequency = 2_441_000_000;
        m.radio.config_sample_rate = 20_000_000.0;
        m.radio.bb_filter_hz = 0;
        let state = Arc::new(Mutex::new(m));
        let (tx, rx) = crossbeam_channel::unbounded();
        tx.send(stamped(&state, 1, false, vec![0u8; 256])).unwrap();
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), eight_bit(), SAFE_BT_CHANNELS).run();
        let m = state.lock().unwrap();
        assert!(m.net.bt_refused.is_none(), "{:?}", m.net.bt_refused);
        assert_eq!(m.net.bt_channels_watched.len(), SAFE_BT_CHANNELS);
    }

    /// Through the actual worker rather than
    /// `signal::bt::receive::Receiver` directly: a synthetic classic BT
    /// access code sitting on one channel of a wideband capture reaches
    /// `net.bt_hops`, tagged with the channel it was found on.
    #[test]
    fn a_synthetic_classic_bt_access_code_reaches_bt_hops() {
        use crate::signal::ble::gfsk::modulate;
        use crate::signal::bt::access_code::access_code_bits;
        use crate::signal::dsp::testkit::{at_snr, Rng};
        use num_complex::Complex;

        const RAW_RATE: f64 = 20_000_000.0;
        // The channel IS the tuned centre - zero offset, so it lands inside
        // the watched cap regardless of how the nearest-first sort breaks
        // ties, the same worked-example shape
        // `signal::bt::receive::the_channel_and_the_tuning_can_both_vary`
        // already exercises directly.
        let ch = 45u8;
        let channel_hz = crate::signal::bt::channel::centre_hz(ch).unwrap();
        let lap = 0x0044_5566;

        // Forty settling symbols either side - see
        // `signal::bt::receive`'s own test-module doc (`SETTLE_SYMBOLS`)
        // for why a bare few bits of margin let the modulator's own pulse
        // shaping and this receiver's own channel-select filter corrupt the
        // access code under test rather than merely surrounding it.
        let mut bits: Vec<bool> = (0..40).map(|i| i % 2 == 0).collect();
        bits.extend(access_code_bits(lap));
        bits.extend((0..40).map(|i| i % 2 == 1));
        let sps = (RAW_RATE / 1_000_000.0) as usize;
        let clean = modulate(&bits, sps, 160_000.0, RAW_RATE, 0.5);
        let placed = at_snr(&clean, 40.0, &mut Rng::new(7));

        let geometry = eight_bit();
        let bytes: Vec<u8> = placed
            .iter()
            .flat_map(|s: &Complex<f32>| {
                let re = (s.re * geometry.full_scale).clamp(-127.0, 127.0) as i8;
                let im = (s.im * geometry.full_scale).clamp(-127.0, 127.0) as i8;
                [re as u8, im as u8]
            })
            .collect();

        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = crate::signal::net::SECTION.to_string();
        m.ui.active_preset = "net_bt".to_string();
        m.radio.frequency = channel_hz;
        m.radio.config_sample_rate = RAW_RATE;
        m.radio.bb_filter_hz = 0;
        let state = Arc::new(Mutex::new(m));

        let (tx, rx) = crossbeam_channel::unbounded();
        tx.send(stamped(&state, 1, false, bytes)).unwrap();
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), geometry, SAFE_BT_CHANNELS).run();

        let m = state.lock().unwrap();
        assert!(m.net.bt_refused.is_none(), "{:?}", m.net.bt_refused);
        assert_eq!(m.net.bt_hops.len(), 1, "{:?}", m.net.bt_hops);
        let hop = &m.net.bt_hops[0];
        assert_eq!(hop.channel, ch);
        assert_eq!(hop.lap, lap);
    }

    /// The same access code on the survey, once its load has left room for
    /// one channel: the nearest classic channel runs, its hit reaches
    /// `net.bt_hops` for the coexistence history, and the account says the
    /// load holds it to that one.
    #[test]
    fn the_survey_marks_classic_hits_once_its_load_leaves_room() {
        use crate::signal::ble::gfsk::modulate;
        use crate::signal::bt::access_code::access_code_bits;
        use crate::signal::dsp::testkit::{at_snr, Rng};
        use num_complex::Complex;

        const RAW_RATE: f64 = 20_000_000.0;
        let ch = 45u8;
        let channel_hz = crate::signal::bt::channel::centre_hz(ch).unwrap();
        let lap = 0x0044_5566;
        let mut bits: Vec<bool> = (0..40).map(|i| i % 2 == 0).collect();
        bits.extend(access_code_bits(lap));
        bits.extend((0..40).map(|i| i % 2 == 1));
        let sps = (RAW_RATE / 1_000_000.0) as usize;
        let clean = modulate(&bits, sps, 160_000.0, RAW_RATE, 0.5);
        let placed = at_snr(&clean, 40.0, &mut Rng::new(7));
        let geometry = eight_bit();
        let bytes: Vec<u8> = placed
            .iter()
            .flat_map(|s: &Complex<f32>| {
                let re = (s.re * geometry.full_scale).clamp(-127.0, 127.0) as i8;
                let im = (s.im * geometry.full_scale).clamp(-127.0, 127.0) as i8;
                [re as u8, im as u8]
            })
            .collect();

        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = crate::signal::net::SECTION.to_string();
        m.ui.active_preset = super::super::lock::SURVEY_VIEW.to_string();
        m.radio.frequency = channel_hz;
        m.radio.config_sample_rate = RAW_RATE;
        m.radio.bb_filter_hz = 0;
        let state = Arc::new(Mutex::new(m));
        let (tx, rx) = crossbeam_channel::unbounded();
        tx.send(stamped(&state, 1, false, bytes)).unwrap();
        drop(tx);
        let mut worker = NetWorker::new(rx, Arc::clone(&state), geometry, SAFE_BT_CHANNELS);
        worker.survey_bt_start = 1;
        worker.run();

        let m = state.lock().unwrap();
        assert!(m.net.bt_refused.is_none(), "{:?}", m.net.bt_refused);
        assert_eq!(m.net.bt_channels_watched, vec![ch]);
        assert!(m.net.bt_load_limited);
        assert_eq!(m.net.bt_hops.len(), 1, "{:?}", m.net.bt_hops);
        assert_eq!(m.net.bt_hops[0].lap, lap);
    }

    /// An access code with nothing after it on channel 45 of a 20 Msps
    /// capture tuned to it: the device bytes and the tuning.
    fn access_code_only(lap: u32) -> (Vec<u8>, u64) {
        use crate::signal::ble::gfsk::modulate;
        use crate::signal::bt::access_code::access_code_bits;
        use crate::signal::dsp::testkit::{at_snr, Rng};
        use num_complex::Complex;
        const RAW_RATE: f64 = 20_000_000.0;
        let channel_hz = crate::signal::bt::channel::centre_hz(45).unwrap();
        let mut bits: Vec<bool> = (0..40).map(|i| i % 2 == 0).collect();
        bits.extend(access_code_bits(lap));
        bits.extend((0..40).map(|i| i % 2 == 1));
        let clean = modulate(&bits, 20, 160_000.0, RAW_RATE, 0.5);
        let placed = at_snr(&clean, 40.0, &mut Rng::new(7));
        let geometry = eight_bit();
        let bytes = placed
            .iter()
            .flat_map(|s: &Complex<f32>| {
                let re = (s.re * geometry.full_scale).clamp(-127.0, 127.0) as i8;
                let im = (s.im * geometry.full_scale).clamp(-127.0, 127.0) as i8;
                [re as u8, im as u8]
            })
            .collect();
        (bytes, channel_hz)
    }

    /// Run one capture through the worker on `preset`, tuned to `tuned`.
    fn run_classic(preset: &str, tuned: u64, bytes: Vec<u8>) -> Arc<Mutex<SdrMetrics>> {
        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = crate::signal::net::SECTION.to_string();
        m.ui.active_preset = preset.to_string();
        m.radio.frequency = tuned;
        m.radio.config_sample_rate = 20_000_000.0;
        m.radio.bb_filter_hz = 0;
        let state = Arc::new(Mutex::new(m));
        let (tx, rx) = crossbeam_channel::unbounded();
        tx.send(stamped(&state, 1, false, bytes)).unwrap();
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), eight_bit(), SAFE_BT_CHANNELS).run();
        state
    }

    /// The Piconet view listens as the Classic view does: without it in the
    /// gate, the view would be empty forever.
    #[test]
    fn the_piconet_view_runs_the_classic_receiver() {
        let (bytes, tuned) = access_code_only(0x0044_5566);
        let state = run_classic("net_piconet", tuned, bytes);
        let m = state.lock().unwrap();
        assert!(m.net.bt_refused.is_none(), "{:?}", m.net.bt_refused);
        assert_eq!(m.net.bt_channels_watched.len(), SAFE_BT_CHANNELS);
        assert_eq!(m.net.bt_hops.len(), 1);
    }

    /// The Bench view listens too: it reads the same receiver's packets.
    #[test]
    fn the_bench_view_runs_the_classic_receiver() {
        let (bytes, tuned) = access_code_only(0x0044_5566);
        let state = run_classic("net_bench", tuned, bytes);
        let m = state.lock().unwrap();
        assert!(m.net.bt_refused.is_none(), "{:?}", m.net.bt_refused);
        assert_eq!(m.net.bt_hops.len(), 1);
    }

    /// A header read at one clock is a packet with its direction, which is
    /// that clock's slot parity, and its payload's verdict: this DH1's CRC
    /// checks out.
    #[test]
    fn a_header_read_at_one_clock_is_a_packet_with_its_direction() {
        use crate::signal::bt::piconet::{Direction, HeaderRead, PayloadVerdict};
        let lap = 0x0033_2211u32;
        let (bytes, tuned) = dh1_capture(lap, 0x7b);
        let state = run_classic("net_bt", tuned, bytes);
        let m = state.lock().unwrap();
        let p = m.net.bt_piconets.iter().find(|p| p.lap == lap).unwrap();
        assert_eq!(p.packets.len(), 1, "{:?}", p.packets);
        let packet = &p.packets[0];
        let Some(HeaderRead::Decoded(h)) = packet.header else {
            panic!("{:?}", packet.header);
        };
        assert_eq!(packet.direction, Some(Direction::of_clk6(h.clk6)));
        assert_eq!(packet.payload, PayloadVerdict::Crc(true));
        assert!(packet.deviation.settled.n > 0, "its own readings kept");
        // Its own f0, with the spread the pooled sums cannot give back, and
        // the same figure the sums hold.
        assert_eq!(packet.carrier.f0_ppm.n, 1);
        let f0 = packet.f0_ppm.expect("its own f0");
        assert!(
            (f0.value() - packet.carrier.f0_ppm.sum).abs() < 1e-3,
            "{f0:?}"
        );
        assert!(f0.sigma() > 0.0 && f0.sigma().is_finite(), "{f0:?}");
        let side = p.headers.sides.clone().of(packet.direction).packets;
        assert_eq!(side, 1, "counted on its side, not under unknown");
        assert_eq!(p.headers.sides.unknown.packets, 0);
    }

    /// An access code with no header after it is an ID row: no header, no
    /// payload, no direction.
    #[test]
    fn a_hit_with_no_header_is_an_id_row() {
        use crate::signal::bt::piconet::PayloadVerdict;
        let lap = 0x0044_5566;
        let (bytes, tuned) = access_code_only(lap);
        let state = run_classic("net_bt", tuned, bytes);
        let m = state.lock().unwrap();
        let p = m.net.bt_piconets.iter().find(|p| p.lap == lap).unwrap();
        assert_eq!(p.packets.len(), 1);
        let packet = &p.packets[0];
        assert_eq!(packet.header, None);
        assert_eq!(packet.payload, PayloadVerdict::NoPayload);
        assert_eq!(packet.direction, None);
        assert_eq!(p.headers.sides.unknown.packets, 1);
    }

    /// A classic packet on channel 45 of a 20 Msps capture tuned to it:
    /// `lap`'s access code, a real DH1 header whose HEC is `true_uap`'s, and
    /// a real DH1 payload with its own CRC-16. Returns the device bytes and
    /// the tuning.
    fn dh1_capture(lap: u32, true_uap: u8) -> (Vec<u8>, u64) {
        use crate::signal::ble::gfsk::modulate;
        use crate::signal::bt::access_code::access_code_bits;
        use crate::signal::bt::header;
        use crate::signal::bt::payload;
        use crate::signal::dsp::testkit::{at_snr, Rng};
        use num_complex::Complex;

        const RAW_RATE: f64 = 20_000_000.0;
        let ch = 45u8;
        let channel_hz = crate::signal::bt::channel::centre_hz(ch).unwrap();
        let clk6 = 22u8;

        let lt_addr = 0b010u8;
        let flags = 0b110u8;
        let type_bits = 0b0100u8; // DH1
        let data10 = (lt_addr as u16) | ((type_bits as u16) << 3) | ((flags as u16) << 7);
        let hec = (0u16..256)
            .map(|h| h as u8)
            .find(|&h| header::uap_from_hec(data10, h) == true_uap)
            .expect("uap_from_hec is a bijection in hec for a fixed data10");
        let mut header_host = [false; header::HEADER_BITS];
        for (i, slot) in header_host[0..10].iter_mut().enumerate() {
            *slot = (data10 >> i) & 1 != 0;
        }
        for (i, slot) in header_host[10..18].iter_mut().enumerate() {
            *slot = (hec >> i) & 1 != 0;
        }
        let whitened_header = header::unwhiten_header(&header_host, clk6);

        let body = [0x44u8, 0x55, 0x66];
        let mut payload_host = vec![false, true, false]; // LLID, FLOW
        for i in 0..5 {
            payload_host.push((body.len() as u8 >> i) & 1 != 0); // LENGTH
        }
        for &byte in &body {
            for i in 0..8 {
                payload_host.push((byte >> i) & 1 != 0);
            }
        }
        let crc = payload::crcgen(&payload_host, true_uap);
        for i in 0..16 {
            payload_host.push((crc >> i) & 1 != 0);
        }
        let payload_whitened = header::unwhiten_at(&payload_host, clk6, header::HEADER_BITS);
        // A generous, arbitrary window past the real payload's own end -
        // this worker's own receiver always captures a fixed maximum
        // regardless of the real payload's shorter length
        // (`signal::bt::receive::PAYLOAD_CAPTURE_BITS`'s own doc).
        const PAYLOAD_CAPTURE_BITS: usize = 343 * 8;

        let mut bits: Vec<bool> = (0..40).map(|i| i % 2 == 0).collect();
        bits.extend(access_code_bits(lap));
        bits.extend([true, false, true, false]); // 4-bit trailer, content unread
        for &b in &whitened_header {
            bits.extend([b, b, b]); // FEC(1/3): each bit sent three times
        }
        bits.extend(payload_whitened.iter().copied());
        bits.extend((0..(PAYLOAD_CAPTURE_BITS - payload_whitened.len())).map(|i| i % 2 == 1));
        bits.extend((0..40).map(|i| i % 2 == 0));

        let sps = (RAW_RATE / 1_000_000.0) as usize;
        let clean = modulate(&bits, sps, 160_000.0, RAW_RATE, 0.5);
        let placed = at_snr(&clean, 40.0, &mut Rng::new(11));

        let geometry = eight_bit();
        let bytes: Vec<u8> = placed
            .iter()
            .flat_map(|s: &Complex<f32>| {
                let re = (s.re * geometry.full_scale).clamp(-127.0, 127.0) as i8;
                let im = (s.im * geometry.full_scale).clamp(-127.0, 127.0) as i8;
                [re as u8, im as u8]
            })
            .collect();

        (bytes, channel_hz)
    }

    /// Through the actual worker: a synthetic
    /// classic BT packet carrying a real DH1 header and a real DH1
    /// payload (its own genuine CRC-16) resolves `net.bt_uap` to exactly
    /// one confirmed UAP - not the two-candidate floor a header alone
    /// reaches - the same day `signal::bt::payload::break_uap_tie` proved
    /// it could, wired here instead of left standing untested.
    #[test]
    fn a_real_dh1_payload_resolves_bt_uap_to_one_confirmed_value() {
        const RAW_RATE: f64 = 20_000_000.0;
        let lap = 0x0033_2211u32;
        let true_uap = 0x7bu8;
        use crate::signal::bt::header;
        let (bytes, channel_hz) = dh1_capture(lap, true_uap);
        let geometry = eight_bit();
        // What `dh1_capture` puts in the header.
        let lt_addr = 0b010u8;

        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = crate::signal::net::SECTION.to_string();
        m.ui.active_preset = "net_bt".to_string();
        m.radio.frequency = channel_hz;
        m.radio.config_sample_rate = RAW_RATE;
        m.radio.bb_filter_hz = 0;
        let state = Arc::new(Mutex::new(m));

        let (tx, rx) = crossbeam_channel::unbounded();
        tx.send(stamped(&state, 1, false, bytes)).unwrap();
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), geometry, SAFE_BT_CHANNELS).run();

        let m = state.lock().unwrap();
        let narrowed = m
            .net
            .bt_uap
            .get(&lap)
            .expect("this LAP's header should have arrived");
        assert_eq!(narrowed, &vec![true_uap], "{narrowed:?}");

        // Resolved by this very payload, the same header is then read under the
        // UAP: a DH1 from LT_ADDR 2, counted on the roster.
        let p = m
            .net
            .bt_piconets
            .iter()
            .find(|p| p.lap == lap)
            .expect("the access code made a row");
        let h = &p.headers;
        assert_eq!((h.captured, h.decoded, h.undecoded), (1, 1, 0), "{h:?}");
        assert_eq!(h.types[header::PacketType::Dh1.code() as usize], 1);
        assert_eq!(h.lt_addrs, 1 << lt_addr);

        // The header's own symbols give the modulation index the
        // modulator was set to: 160 kHz at 1 Msym/s is h = 0.32. Read
        // against the slicer's fast tracker it came out 0.297 (149 kHz); from
        // the header's own settled centre, 0.322 (160.9 +/- 0.8 kHz).
        let df1 = h
            .deviation
            .settled
            .mean()
            .expect("settled runs in a header");
        let index = df1.value() * 2.0 / 1e6;
        assert!((index - 0.32).abs() < 0.01, "h = {index} from {df1:?}");

        // The header joins its own hit, so the export can say what the hit
        // carried.
        let hop = m
            .net
            .bt_hops
            .iter()
            .find(|h| h.lap == lap)
            .expect("the hit");
        assert!(
            matches!(
                hop.header,
                Some(crate::signal::bt::piconet::HeaderRead::Decoded(h))
                    if h.packet_type == header::PacketType::Dh1
            ),
            "{hop:?}"
        );
    }

    /// An inquiry code is counted and placed, and nothing more: the same
    /// capture that resolves an ordinary LAP's UAP and reads its header
    /// leaves the GIAC with no UAP, no header read and no slot fit, since
    /// every searching device sends it and none of those would be one
    /// device's.
    #[test]
    fn an_inquiry_code_is_counted_but_never_narrowed_or_fitted() {
        const RAW_RATE: f64 = 20_000_000.0;
        let giac = 0x9E_8B33;
        let (bytes, channel_hz) = dh1_capture(giac, 0x7b);
        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = crate::signal::net::SECTION.to_string();
        m.ui.active_preset = "net_bt".to_string();
        m.radio.frequency = channel_hz;
        m.radio.config_sample_rate = RAW_RATE;
        m.radio.bb_filter_hz = 0;
        let state = Arc::new(Mutex::new(m));
        let (tx, rx) = crossbeam_channel::unbounded();
        tx.send(stamped(&state, 1, false, bytes)).unwrap();
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), eight_bit(), SAFE_BT_CHANNELS).run();

        let m = state.lock().unwrap();
        let p = m
            .net
            .bt_piconets
            .iter()
            .find(|p| p.lap == giac)
            .expect("the hit is still counted");
        assert_eq!(p.hits, 1);
        assert!(!m.net.bt_uap.contains_key(&giac), "{:?}", m.net.bt_uap);
        assert_eq!(p.headers.captured, 0);
        assert!(p.slots.is_none());
        // Its pace is still read: one hit, no spacing yet.
        assert_eq!(p.pace, crate::signal::bt::slots::Pace::default());
        assert!(m.net.bt_hops.iter().all(|h| h.header.is_none()));
    }

    /// **Slot timing end to end** (6.5): twelve access codes of one LAP,
    /// each on a slot of a piconet whose slots run 30 ppm long on our clock,
    /// across equal blocks of one continuous stream. The worker dates them
    /// on the stream's clock and fits a grid whose leftover is well inside
    /// the specification's 1 µs.
    #[test]
    fn a_piconets_access_codes_give_its_slot_grid() {
        use crate::signal::ble::gfsk::modulate;
        use crate::signal::bt::access_code::access_code_bits;
        use crate::signal::dsp::testkit::Rng;
        use num_complex::Complex;

        const RAW_RATE: f64 = 4_000_000.0;
        const BLOCK: usize = 10_000; // 2.5 ms: one access code a block at most
        let ch = 45u8;
        let channel_hz = crate::signal::bt::channel::centre_hz(ch).unwrap();
        let lap = 0x0012_3456u32;
        let slot_samples = 625e-6 * (1.0 + 30e-6) * RAW_RATE;
        let slots = [0u32, 5, 11, 16, 22, 28, 33, 40, 46, 51, 57, 63];

        let mut bits: Vec<bool> = (0..40).map(|i| i % 2 == 0).collect();
        bits.extend(access_code_bits(lap));
        bits.extend((0..40).map(|i| i % 2 == 1));
        let burst = modulate(&bits, 4, 160_000.0, RAW_RATE, 0.5);
        let total = ((*slots.last().unwrap() as f64 + 4.0) * slot_samples) as usize;
        let total = total.div_ceil(BLOCK) * BLOCK;
        let mut rng = Rng::new(9);
        let mut iq: Vec<Complex<f32>> = rng.noise(total, 1e-4);
        for &k in &slots {
            let at = (5_000.0 + k as f64 * slot_samples).round() as usize;
            for (i, s) in burst.iter().enumerate() {
                iq[at + i] += s;
            }
        }
        let geometry = eight_bit();
        let bytes: Vec<u8> = iq
            .iter()
            .flat_map(|s| {
                let re = (s.re * geometry.full_scale).clamp(-127.0, 127.0) as i8;
                let im = (s.im * geometry.full_scale).clamp(-127.0, 127.0) as i8;
                [re as u8, im as u8]
            })
            .collect();

        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = crate::signal::net::SECTION.to_string();
        m.ui.active_preset = "net_bt".to_string();
        m.radio.frequency = channel_hz;
        m.radio.config_sample_rate = RAW_RATE;
        m.radio.bb_filter_hz = 0;
        let state = Arc::new(Mutex::new(m));
        let (tx, rx) = crossbeam_channel::unbounded();
        for (i, chunk) in bytes.chunks(BLOCK * 2).enumerate() {
            tx.send(stamped(&state, i as u64 + 1, false, chunk.to_vec()))
                .unwrap();
        }
        drop(tx);
        let mut worker = NetWorker::new(rx, Arc::clone(&state), geometry, SAFE_BT_CHANNELS);
        worker.slot_fit_every = std::time::Duration::ZERO;
        worker.run();

        let m = state.lock().unwrap();
        let p = m
            .net
            .bt_piconets
            .iter()
            .find(|p| p.lap == lap)
            .expect("a row");
        assert_eq!(p.hits, slots.len() as u64);
        let fit = p.slots.as_ref().expect("fitted").as_ref().expect("a grid");
        assert_eq!(fit.hits, slots.len());
        // Measured when written: 27.1 ppm (40 ms is short to resolve a
        // rate), rms 0.15 +/- 0.03 us, max 0.21 us - our own quarter-symbol
        // dating and the rounding of each burst to a sample, nothing more.
        assert!((fit.rate_ppm - 30.0).abs() < 5.0, "{fit:?}");
        assert!(fit.rms_us.value() < 0.3, "{fit:?}");
        assert!(fit.max_us < 0.5, "{fit:?}");

        // Every one of those hits exports its residual from that grid.
        let rows = crate::export::bt::rows(&m);
        let col = crate::export::bt::HEADER
            .split(',')
            .position(|h| h == "slot_residual_us")
            .unwrap();
        assert_eq!(rows.len(), slots.len());
        for row in &rows {
            let r: f64 = row.split(',').nth(col).unwrap().parse().expect(row);
            assert!(r.abs() < 0.5, "{row}");
        }
    }

    /// Any other preset's blocks leave `bt_refused` unset, so a stale
    /// refusal from a previous `net_bt` visit does not linger onto a screen
    /// that never claimed to be that preset.
    #[test]
    fn a_different_preset_leaves_bt_refused_unset() {
        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = crate::signal::net::SECTION.to_string();
        m.ui.active_preset = "net_ble".to_string();
        m.radio.frequency = 2_402_000_000;
        m.radio.config_sample_rate = 4_000_000.0;
        let state = Arc::new(Mutex::new(m));
        let (tx, rx) = crossbeam_channel::unbounded();
        tx.send(stamped(&state, 1, false, vec![0u8; 256])).unwrap();
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), eight_bit(), SAFE_BT_CHANNELS).run();
        let m = state.lock().unwrap();
        assert!(m.net.bt_refused.is_none(), "{:?}", m.net.bt_refused);
    }
}
