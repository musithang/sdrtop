// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The 2.4 GHz worker: one thread, one block at a time.
//!
//! Same shape as [`crate::signal::DemodWorker`], for the same reason: a thread
//! that owns the state carried between blocks, so that everything below it can
//! be a pure function of its arguments and be tested with no radio anywhere.
//!
//! **It counts what arrived and measures what was in the band.** Design
//! section 13.2 makes what the receiver missed a first-class displayed number
//! rather than an inference, and a feed whose losses are only visible once
//! there is something to lose is a feed nobody will trust when the losses
//! matter - so the counting was built and shown before any measurement sat on
//! top of it. The band measurement is [`super::scan`] over
//! [`super::occupancy`].
//!
//! **B6 added the first decoder: BLE, on whichever of the three advertising
//! frequencies the radio is tuned to.** It runs here rather than in its own
//! task because it needs the same per-block bytes the occupancy scan already
//! has - a second worker reading the same channel would need its own copy of
//! the geometry and the retune-detection logic this one already carries.
//!
//! **B15 added the second: classic Bluetooth, on the `net_bt` preset.**
//! Unlike BLE, there is no fixed set of channels to gate on - every one of
//! the 79 is valid - so this worker builds one `signal::bt::receive::
//! Receiver` per channel `signal::bt::channel::channels_in_span` and the
//! configured [`SAFE_BT_CHANNELS`]-guarded cap together let it watch, closest
//! to the tuned centre first, and rebuilds the fleet whenever the tuning or
//! the wanted channel list changes.
//!
//! **B16 added the third: one `signal::bt::header::PiconetClock` per LAP**,
//! fed every `HeaderHit` the fleet's own receivers capture, narrowing each
//! piconet's own UAP as far as a header alone ever can - `PiconetClock`'s
//! own doc has the measured floor (two candidates, not one) and why. Wi-Fi
//! arrives the same way when that arc reaches this point.
//!
//! **B17 breaks that floor, live.** Each `HeaderHit` now carries a captured
//! payload region alongside its header; whenever a LAP's own `PiconetClock`
//! has not settled on one UAP, this worker tries `signal::bt::payload::
//! break_uap_tie` against it. `resolved_bt_uap` remembers a LAP that
//! resolves this way for the rest of the session - a piconet's real UAP does
//! not change, so a later header this arc cannot read the payload of (a
//! POLL or an FHS, say) must not undo an answer already earned.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crossbeam_channel::Receiver as SampleReceiver;

use crate::hardware::{SampleGeometry, StreamBlock};
use crate::signal::ble::receive::Receiver as BleReceiver;
use crate::signal::bt::header::PiconetClock;
use crate::signal::bt::payload;
use crate::signal::bt::piconet::Inquiry;
use crate::signal::bt::receive::Receiver as BtReceiver;
use crate::signal::stream::plan_block;
use crate::state::{BlePacket, BtHop, SdrMetrics};

use super::scan::Scan;

/// The name `net_bt`'s own preset carries in the menu, gating this worker's
/// classic Bluetooth branch the same way `signal::net::gate` gates whole
/// presets elsewhere - a plain string comparison because that is what the
/// menu itself is keyed by (`app/builder/registry.rs`'s own structural
/// tests hold this string and the preset file's name to agreeing).
const NET_BT_PRESET: &str = "net_bt";

/// `bytes` as samples, decoded into `slot` the first time a receiver asks
/// and handed out as they are after that.
fn decoded_block<'a>(
    slot: &'a mut Option<Vec<num_complex::Complex<f32>>>,
    bytes: &[u8],
    geometry: SampleGeometry,
) -> &'a [num_complex::Complex<f32>] {
    slot.get_or_insert_with(|| {
        let mut out = Vec::new();
        crate::signal::demod::decode(bytes, geometry, usize::MAX, &mut out);
        out
    })
}

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
}

/// What the worker carries from one block to the next.
///
/// Three fields, and two of them exist only so that a gap can be told from a
/// pause. See [`Run::suspend`].
#[derive(Default)]
struct Run {
    last_seq: u64,
    /// The sequence of the previous block *that reached us*, or `None` when the
    /// run has not started.
    drop_ref: Option<u64>,
    /// Unbroken blocks since the last gap.
    blocks: u64,
}

impl Run {
    /// The section was closed, so the feed stopped forwarding.
    ///
    /// The device goes on counting every callback, so the next block to arrive
    /// will be thousands of sequence numbers away without a single one having
    /// been lost. Clearing `drop_ref` is what stops that jump being reported as
    /// the worst loss event of the session. The run length goes with it: there
    /// is no run any more.
    fn suspend(&mut self) {
        self.drop_ref = None;
        self.blocks = 0;
    }
}

/// How long one decode-load reading averages over: half a second of stream,
/// or half a second of work, whichever comes first.
///
/// Half a second: at the block sizes the native radios deliver that is dozens
/// of blocks, enough that one slow block (a page fault, a scheduler hiccup)
/// does not read as a worker in trouble, and short enough that the figure
/// follows the user opening a heavier preset within a glance.
///
/// **Either clock closes the window, and the second one is not optional.** A
/// window counted in stream time alone takes longer to fill the slower the
/// worker is, because a worker that cannot keep up only ever sees the blocks
/// the bounded feed had room for. The first live run showed exactly that: a
/// worker in trouble whose load never appeared at all, twenty seconds in. So
/// the figure that matters most arrived last, or not at all. Closing the
/// window on work time too means an overloaded worker reports within half a
/// second, at whatever it really is.
const LOAD_WINDOW_S: f64 = 0.5;

/// The decode-load accumulator: wall time spent against stream time covered.
///
/// Pure, with the clock read by the caller, so the arithmetic is testable
/// without a radio or a real stopwatch.
#[derive(Default)]
struct Load {
    busy: std::time::Duration,
    stream_s: f64,
}

impl Load {
    /// Add one block: `spent` handling it, `pairs` I/Q pairs of it at
    /// `rate_hz`. Returns a reading once a whole window has been covered, and
    /// starts the next one.
    fn add(&mut self, spent: std::time::Duration, pairs: u64, rate_hz: f64) -> Option<f64> {
        // A rate that is not a positive number covers no stream time, and
        // dividing by it would invent a load.
        if rate_hz.is_nan() || rate_hz <= 0.0 {
            return None;
        }
        self.busy += spent;
        self.stream_s += pairs as f64 / rate_hz;
        if self.stream_s < LOAD_WINDOW_S && self.busy.as_secs_f64() < LOAD_WINDOW_S {
            return None;
        }
        let load = self.busy.as_secs_f64() / self.stream_s;
        *self = Load::default();
        Some(load)
    }
}

/// Fold one decoded BLE packet into the shared census, if it earns a place
/// there.
///
/// **The census counts confirmed transmitters, not decode attempts.** An
/// address from a packet whose CRC did not pass is not a device this
/// receiver has actually confirmed, and counting it would be exactly the
/// invented reading rule 2 refuses. B10's own exit condition is "every
/// device in the room" - CRC-clean ones, which is the only kind this can
/// honestly claim to have found. A failed packet can still count *against*
/// a device already confirmed, as a CRC failure on an exact address match
/// (`census::observe_crc_failure` says why that much is safe); it never
/// makes a row.
///
/// Pulled out of [`NetWorker::run`]'s own loop as a plain function of a
/// packet and a clock, rather than tested only by building a real, noisy
/// capture through the whole receive chain to get one - `census::observe`'s
/// own tests already hold the census half of this to account; what only this
/// function does is decide *whether*, and *which*, to call.
///
/// Runs inside the state lock, once per packet: a lookup, a few additions
/// and an inverse-variance fold, and nothing that waits on a square root
/// (the census module doc).
///
/// **An arrival is kept only in LOCK, and only from periodic advertising.**
/// While surveying, the dwell schedule decides which packets are heard, so
/// gaps between them would time the survey (`signal::ble::interval`). And a
/// scan response is an answer to a scanner, sent whenever it asked, so it
/// times the scanner: only the four advertising PDUs, which the Link Layer
/// sends once an event, are timed.
fn census_from_ble(
    devices: &mut Vec<crate::signal::net::census::Device>,
    p: &crate::signal::ble::pdu::Packet,
    channel: u8,
    locked: bool,
    rate_hz: f64,
    now: Instant,
) {
    use crate::signal::ble::pdu::PduType;
    let Some(address) = p.adv_addr else {
        return;
    };
    if !p.crc_ok {
        crate::signal::net::census::observe_crc_failure(devices, address, p.snr_db);
        return;
    }
    // In ppm of the channel it was heard on, so readings from all three
    // advertising channels can be combined: see `Device::crystal_offset_ppm`.
    let carrier = crate::signal::ble::channel::centre_hz(channel);
    let crystal_offset_ppm = p
        .freq_offset_hz
        .zip(carrier)
        .map(|(hz, c)| crate::state::offset_ppm(hz, c as f64));
    let sighting = crate::signal::net::census::Sighting {
        address,
        random: p.tx_add_random,
        snr_db: p.snr_db,
        crystal_offset_ppm,
        ble_pdu_code: Some(p.pdu_type.code()),
        modulation_index: p.modulation.map(|m| m.modulation_index),
        arrival: p
            .at_pair
            .filter(|_| {
                locked
                    && matches!(
                        p.pdu_type,
                        PduType::AdvInd
                            | PduType::AdvDirectInd
                            | PduType::AdvNonconnInd
                            | PduType::AdvScanInd
                    )
            })
            .map(|pair| crate::signal::net::census::Arrival {
                channel,
                rate_hz,
                pair,
            }),
    };
    crate::signal::net::census::observe(devices, &sighting, now);
}

/// The address a packet came from and what it advertised about itself
/// (`signal::ble::ad::Advertised`), for `NetState::advertised`; `None` where
/// it said nothing beyond its address.
///
/// **Only from a packet whose CRC passed**: a failed one could name a company
/// or a device nobody sent, and then name it for every packet that device
/// sends after.
fn advertised_of(
    p: &crate::signal::ble::pdu::Packet,
) -> Option<([u8; 6], crate::signal::ble::ad::Advertised)> {
    use crate::signal::ble::ad;
    if !p.crc_ok {
        return None;
    }
    let address = p.adv_addr?;
    let data = ad::adv_data(p.pdu_type, &p.payload)?;
    let said = ad::Advertised::from_structures(&ad::parse(data));
    (!said.is_empty()).then_some((address, said))
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
        let mut bt: Vec<BtReceiver> = Vec::new();
        let mut piconet_clocks: HashMap<u32, PiconetClock> = HashMap::new();
        // B17's own live tie-break, one LAP at a time: once resolved, a
        // LAP's real UAP does not change (it comes from the piconet
        // master's own fixed address), so this sticks the same way
        // `signal::net::census::Device::first_seen` never moves on a
        // repeat sighting - a later header from a packet type `payload::
        // break_uap_tie` cannot read (POLL, FHS, ...) must not flip a
        // resolved answer back to two candidates.
        let mut resolved_bt_uap: HashMap<u32, u8> = HashMap::new();
        // 6.5: each piconet's access-code times, µs on the stream's clock,
        // for its slot grid (`signal::bt::slots`), kept here rather than in
        // the state because only the fit is shown; with the rate they were
        // dated at, and when each was last fitted. A new stream or a new
        // rate restarts the logs: their times are then on another clock.
        let mut bt_arrivals: HashMap<u32, std::collections::VecDeque<f64>> = HashMap::new();
        let mut arrivals_rate = 0.0f64;
        // Which stream the times are on: bumped with every restart of the
        // clock they count on, so a hop, a header and a grid are compared
        // only within one (net-ux-polish-plan 6.6).
        let mut stream_id = 0u32;
        let mut last_fit: HashMap<u32, Instant> = HashMap::new();
        // Piconets with hits their last fit has not seen: refitted once the
        // interval allows, whether or not another hit comes, so a piconet
        // that falls silent still has its last hits in its figure.
        let mut unfitted: std::collections::HashSet<u32> = std::collections::HashSet::new();
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
            // restarted, `RxContext::begin_stream`): its clock starts again,
            // so what each piconet's clock learned from the old one's timing
            // no longer applies. A resolved UAP does - a piconet's address does
            // not change - so `resolved_bt_uap` is kept.
            if next_pair.is_some_and(|n| first_pair < n) {
                piconet_clocks.clear();
                bt_arrivals.clear();
                unfitted.clear();
                stream_id = stream_id.wrapping_add(1);
            }
            if rate_hz != arrivals_rate {
                bt_arrivals.clear();
                unfitted.clear();
                arrivals_rate = rate_hz;
                stream_id = stream_id.wrapping_add(1);
            }
            if !continuous {
                ble = None;
                bt.clear();
                recent.clear();
            }
            next_pair = Some(first_pair + pairs);
            // The tuning and rate these samples were captured at, from the
            // block rather than the state: see `StreamBlock::centre_hz`.
            let centre_hz = centre_hz as f64;

            let (still_open, span_hz, is_net_bt, phy, locked) = {
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
                    m.ui.active_preset == NET_BT_PRESET,
                    m.net.ble_phy,
                    m.net.mode == crate::state::NetMode::Lock,
                )
            };

            // Retuning invalidates every cell mapping, so the scan is rebuilt
            // and whatever it had accumulated goes with it: half a dwell at one
            // frequency and half at another is a measurement of neither.
            if !scan
                .as_ref()
                .is_some_and(|s| s.matches(centre_hz, rate_hz, span_hz))
            {
                scan = Some(Scan::new(centre_hz, rate_hz, span_hz));
            }
            if let Some(scan) = scan.as_mut() {
                scan.push(&bytes, self.geometry);
                if scan.observed_s() >= DWELL_S {
                    let band = scan.take();
                    let mut m = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    m.net.band.absorb(band, now);
                }
            }

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
            match channel {
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
                    // and not run (net-ux-polish-plan 5.5).
                    ble = None;
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
                    if let Some(rx) = ble.as_mut() {
                        let packets = rx
                            .push_iq_at(decoded_block(&mut iq, &bytes, self.geometry), first_pair);
                        let funnel = rx.take_funnel();
                        if !funnel.is_empty() {
                            let mut m = self.state.lock().unwrap_or_else(|e| e.into_inner());
                            m.net.health.ble.add(funnel);
                        }
                        if !packets.is_empty() {
                            // Read before the lock: parsing is work the UI
                            // thread should not wait behind.
                            let advertised: Vec<_> =
                                packets.iter().filter_map(advertised_of).collect();
                            let mut m = self.state.lock().unwrap_or_else(|e| e.into_inner());
                            for (address, said) in advertised {
                                m.net.advertised.entry(address).or_default().merge(said);
                            }
                            if let Some(i) =
                                crate::signal::ble::channel::advertising_channel_index(ch)
                            {
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
                                census_from_ble(
                                    &mut m.net.census.devices,
                                    &p,
                                    ch,
                                    locked,
                                    rate_hz,
                                    now,
                                );
                                m.net.ble_heard += 1;
                                let seq = m.net.ble_heard;
                                m.net.ble_packets.push_front(BlePacket {
                                    seq,
                                    phy,
                                    channel: ch,
                                    pdu_type: p.pdu_type,
                                    ch_sel: p.ch_sel,
                                    tx_add_random: p.tx_add_random,
                                    rx_add_random: p.rx_add_random,
                                    length: p.length,
                                    adv_addr: p.adv_addr,
                                    payload: p.payload,
                                    crc_ok: p.crc_ok,
                                    snr_db: p.snr_db,
                                    freq_offset_hz: p.freq_offset_hz,
                                    modulation: p.modulation,
                                    drift: p.drift,
                                    seen: now,
                                });
                            }
                            m.net.ble_packets.truncate(crate::state::BLE_PACKET_LIMIT);
                        }
                    }
                }
                Some(_) => {
                    // Section closed; nothing decodes while it is.
                    ble = None;
                }
                None => {
                    ble = None;
                    let mut m = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    m.net.ble_refused = Some(
                        "not tuned to an advertising channel (2402, 2426 or 2480 MHz)".to_string(),
                    );
                    m.net.ble_channel = None;
                }
            }

            // `net_bt`: B15's own live receiver, one per channel the current
            // tuning and `self.bt_channels` together let it watch. Not
            // gated on `still_open` the way BLE's block above is guarded
            // twice over (once by `channel_of` returning `None`, once by
            // this preset's own name) - classic BT has no fixed channel set
            // to fall back on, so the preset name is the only gate there is.
            if is_net_bt && still_open {
                let mut wanted = crate::signal::bt::channel::channels_in_span(centre_hz, span_hz);
                wanted.sort_by_key(|&ch| {
                    let f = crate::signal::bt::channel::centre_hz(ch).unwrap_or(0) as f64;
                    (f - centre_hz).abs() as u64
                });
                wanted.truncate(self.bt_channels);
                wanted.sort_unstable();

                let current: Vec<u8> = bt.iter().map(|r| r.channel()).collect();
                let stale_tuning = bt
                    .first()
                    .is_some_and(|r| !r.matches(r.channel(), rate_hz, centre_hz));
                if current != wanted || stale_tuning {
                    let mut fleet = Vec::with_capacity(wanted.len());
                    let mut refusal = None;
                    for &ch in &wanted {
                        match BtReceiver::new(rate_hz, ch, centre_hz, first_pair) {
                            Ok(r) => fleet.push(r),
                            Err(e) => {
                                refusal.get_or_insert(e);
                            }
                        }
                    }
                    bt = fleet;
                    let mut m = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    m.net.bt_refused = if wanted.is_empty() {
                        Some(format!(
                            "no classic Bluetooth channel fits inside the current {:.1} MHz view",
                            span_hz / 1e6
                        ))
                    } else {
                        refusal
                    };
                    m.net.bt_channels_watched = bt.iter().map(|r| r.channel()).collect();
                    m.net.bt_capacity = self.bt_channels;
                }

                let mut hits = Vec::new();
                let mut header_hits = Vec::new();
                for rx in bt.iter_mut() {
                    let (laps, headers) = rx.push_iq(decoded_block(&mut iq, &bytes, self.geometry));
                    for hit in laps {
                        hits.push((rx.channel(), hit.lap, hit.at_us));
                        let log = bt_arrivals.entry(hit.lap).or_default();
                        if log.len() == crate::signal::bt::slots::KEPT {
                            log.pop_front();
                        }
                        log.push_back(hit.at_us);
                        unfitted.insert(hit.lap);
                    }
                    header_hits
                        .extend(headers.into_iter().filter(|h| Inquiry::of(h.lap).is_none()));
                }
                // Fed to each LAP's own `PiconetClock` outside the lock -
                // narrowing does real work (64 dewhitenings per header),
                // the same reasoning every other float or device-free
                // computation in this worker stays outside the lock block
                // for.
                //
                // **B17's own tie-break rides alongside it.** A LAP
                // already resolved shows its one confirmed UAP and does
                // no further work at all - `resolved_bt_uap`'s own doc
                // says why a later, unresolvable header must not undo
                // this. Otherwise, a header narrowed to more than one
                // candidate gets one attempt at `payload::break_uap_tie`
                // using this same hit's own captured payload; success
                // resolves the LAP for good, failure (an unsupported
                // packet type, or simply not enough real payload behind
                // this particular header) falls back to showing the
                // still-honest candidate set `PiconetClock` itself
                // reports, exactly as before this step.
                let mut narrowed_by_lap = Vec::new();
                let mut headers_read = Vec::new();
                // Each header measured again from the raw samples, through
                // the tester's filter (`measure`), here outside the lock.
                let window = super::measure::Recent::new(
                    recent
                        .iter()
                        .map(|(p, v)| (*p, v.as_slice()))
                        .chain(iq.as_deref().map(|v| (first_pair, v))),
                );
                for hit in &header_hits {
                    let clock = piconet_clocks.entry(hit.lap).or_default();
                    clock.observe(hit.tick, &hit.whitened);
                    let shown = if let Some(&uap) = resolved_bt_uap.get(&hit.lap) {
                        vec![uap]
                    } else {
                        let narrowed = clock.narrowed();
                        if narrowed.len() > 1 {
                            match payload::break_uap_tie(&narrowed, &hit.whitened, &hit.payload_raw)
                            {
                                Some(uap) => {
                                    resolved_bt_uap.insert(hit.lap, uap);
                                    vec![uap]
                                }
                                None => narrowed,
                            }
                        } else {
                            narrowed
                        }
                    };
                    // 6.3: a header is read once its piconet's UAP is one
                    // value, and only then; the decode tries all 64 clocks,
                    // so it too stays out of the lock.
                    let read = match shown.as_slice() {
                        [uap] => {
                            match crate::signal::bt::header::decode_with_uap(&hit.whitened, *uap) {
                                Some(h) => crate::signal::bt::piconet::HeaderRead::Decoded(h),
                                None => crate::signal::bt::piconet::HeaderRead::Undecoded,
                            }
                        }
                        _ => crate::signal::bt::piconet::HeaderRead::Unresolved,
                    };
                    headers_read.push((
                        hit.lap,
                        hit.at_us,
                        read,
                        clock.hypotheses(),
                        crate::signal::bt::channel::centre_hz(hit.ch)
                            .and_then(|hz| {
                                super::measure::classic(
                                    &window,
                                    rate_hz,
                                    hz as f64 - centre_hz,
                                    hit.lap,
                                    hit.sync_end_pair,
                                    &hit.air,
                                )
                            })
                            .unwrap_or_default(),
                    ));
                    narrowed_by_lap.push((hit.lap, shown));
                }
                // The slot fit, outside the lock and at most once a second a
                // piconet: a rate search over hundreds of hits is real work,
                // and a jitter figure does not need refreshing faster.
                let due: Vec<u32> = unfitted
                    .iter()
                    .copied()
                    .filter(|lap| {
                        last_fit.get(lap).is_none_or(|t| {
                            now.saturating_duration_since(*t) >= self.slot_fit_every
                        })
                    })
                    .collect();
                // An inquiry code is every searching device's at once, so it
                // gets no slot grid of one piconet. Every LAP gets its pace
                // (`slots::pace`), cheap and burst by burst: the timing half
                // of telling inquiry and paging from a piconet's traffic.
                let mut fits = Vec::with_capacity(due.len());
                for lap in due {
                    use crate::signal::bt::slots;
                    let times: Vec<f64> = bt_arrivals
                        .get(&lap)
                        .map(|l| l.iter().copied().collect())
                        .unwrap_or_default();
                    let inquiry = Inquiry::of(lap).is_some();
                    let whole = (!inquiry).then(|| slots::fit(&times));
                    fits.push((lap, whole, slots::pace(&times)));
                    last_fit.insert(lap, now);
                    unfitted.remove(&lap);
                }
                if !hits.is_empty() || !narrowed_by_lap.is_empty() || !fits.is_empty() {
                    let mut m = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    m.net.health.bt_hits += hits.len() as u64;
                    for (channel, lap, at_us) in hits {
                        crate::signal::bt::piconet::observe(
                            &mut m.net.bt_piconets,
                            lap,
                            channel,
                            now,
                        );
                        m.net.bt_hops.push_front(BtHop {
                            channel,
                            lap,
                            seen: now,
                            at_us,
                            stream: stream_id,
                            header: None,
                        });
                    }
                    m.net.bt_hops.truncate(crate::state::BT_HOP_LIMIT);
                    for (lap, narrowed) in narrowed_by_lap {
                        m.net.bt_uap.insert(lap, narrowed);
                    }
                    for (lap, whole, pace) in fits {
                        if let Some(p) = m.net.bt_piconets.iter_mut().find(|p| p.lap == lap) {
                            if let Some(fit) = whole {
                                p.slots = Some(fit);
                                p.slots_stream = stream_id;
                            }
                            p.pace = pace;
                        }
                    }
                    for (lap, at_us, read, hypotheses, deviation) in headers_read {
                        // Joined to its hit by LAP and time: the header's
                        // capture starts on the lane that found the access
                        // code, which may be a quarter-symbol lane off the
                        // one the hit was dated by.
                        if let Some(hop) = m.net.bt_hops.iter_mut().find(|h| {
                            h.lap == lap && h.stream == stream_id && (h.at_us - at_us).abs() < 2.0
                        }) {
                            hop.header = Some(read);
                        }
                        crate::signal::bt::piconet::observe_header(
                            &mut m.net.bt_piconets,
                            lap,
                            read,
                            hypotheses,
                            deviation,
                        );
                    }
                }
            } else {
                // Not on this preset: no receiver to run, and a refusal or a
                // watched-channel list from a previous visit must not linger
                // onto a screen that never claimed to be this one.
                bt.clear();
                let mut m = self.state.lock().unwrap_or_else(|e| e.into_inner());
                m.net.bt_refused = None;
                m.net.bt_channels_watched.clear();
            }

            // Held for the measurement path, as much as `measure::HELD_S`
            // asks and no more; a block nothing decoded leaves a hole, so what
            // was held before it can no longer be joined to what comes after.
            match iq.take() {
                Some(block) if still_open => {
                    recent.push_back((first_pair, block));
                    let keep = (super::measure::HELD_S * rate_hz) as usize;
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
                ble = None;
                bt.clear();
                load = Load::default();
                next_pair = None;
                let mut m = self.state.lock().unwrap_or_else(|e| e.into_inner());
                m.net.bt_refused = None;
                m.net.bt_channels_watched.clear();
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
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::SampleFormat;

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
        let cases: [(&str, f64, u64); 6] = [
            ("net_survey", 8e6, 2_426_500_000),
            ("net_ble", 8e6, 2_426_000_000),
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

    /// B6's exit condition, run through the actual worker rather than
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
        // Numbered on arrival, so `masked` can show it (net-ux-polish-plan 1.6.b).
        assert_eq!(m.net.address_book.get(addr), Some(1));

        // B10's own exit condition: a confirmed device reaches the shared
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
        // census too, keyed by the same address `net_ble_packets` shows.
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
            // Measured on LE 2M's own clock (net-ux-polish-plan 5.5).
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

    /// A packet whose CRC did not pass is not a confirmed transmitter - rule
    /// 2 refuses to count an address this receiver has not actually
    /// verified. A plain function of a packet and a clock, tested directly
    /// as one rather than by building a real, noisy capture through the
    /// whole receive chain to get a CRC-failing decode out the other end -
    /// see [`census_from_ble`]'s own doc for why.
    #[test]
    fn a_failed_crc_does_not_reach_the_census() {
        let mut devices = Vec::new();
        let mut packet = crate::signal::ble::pdu::decode(&[false; 40]).unwrap();
        packet.crc_ok = false;
        packet.adv_addr = Some([1, 2, 3, 4, 5, 6]);
        census_from_ble(&mut devices, &packet, 37, false, 8e6, Instant::now());
        assert!(devices.is_empty(), "{devices:?}");
    }

    /// The other half of the same gate: a confirmed packet with an address
    /// does reach the census.
    #[test]
    fn a_passed_crc_with_an_address_reaches_the_census() {
        let mut devices = Vec::new();
        let mut packet = crate::signal::ble::pdu::decode(&[false; 40]).unwrap();
        packet.crc_ok = true;
        packet.adv_addr = Some([1, 2, 3, 4, 5, 6]);
        census_from_ble(&mut devices, &packet, 37, false, 8e6, Instant::now());
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].address, [1, 2, 3, 4, 5, 6]);
    }

    /// **The company comes from CRC-good manufacturer data only**: the real
    /// ADV_NONCONN_IND's 0x004C is taken, the same octets with a failed CRC
    /// are not, and a packet with no manufacturer data names nothing.
    #[test]
    fn what_is_advertised_is_read_only_from_a_packet_whose_crc_passed() {
        use crate::signal::ble::pdu::{air_octets, PduType};
        let addr = [0xd1, 0x9a, 0x7e, 0x91, 0x27, 0x9e];
        let mut packet = crate::signal::ble::pdu::decode(&[false; 40]).unwrap();
        packet.pdu_type = PduType::AdvNonconnInd;
        packet.adv_addr = Some(addr);
        packet.payload = air_octets(addr).to_vec();
        packet
            .payload
            .extend_from_slice(&[0x07, 0xff, 0x4c, 0x00, 0x12, 0x02, 0x00, 0x02]);
        packet.crc_ok = true;
        let company = |p| advertised_of(p).and_then(|(a, said)| said.company.map(|c| (a, c)));
        assert_eq!(company(&packet), Some((addr, 0x004C)));
        packet.crc_ok = false;
        assert_eq!(advertised_of(&packet), None);
        packet.crc_ok = true;
        packet.payload.truncate(6);
        assert_eq!(advertised_of(&packet), None);
    }

    /// **Every packet counts in its SNR bin, good or failed**: a good one in
    /// the device's curve and the section's, a failed one from the same
    /// address in both too (the exact-address rule).
    #[test]
    fn packets_fill_the_frame_error_curves_by_snr() {
        use crate::signal::ble::fer::bin_of;
        let mut devices = Vec::new();
        let mut packet = crate::signal::ble::pdu::decode(&[false; 40]).unwrap();
        packet.adv_addr = Some([1, 2, 3, 4, 5, 6]);
        packet.snr_db = Some(13.0);
        packet.crc_ok = true;
        census_from_ble(&mut devices, &packet, 37, false, 8e6, Instant::now());
        packet.crc_ok = false;
        census_from_ble(&mut devices, &packet, 37, false, 8e6, Instant::now());
        let fer = &devices[0].fer;
        assert_eq!((fer.good[bin_of(13.0)], fer.failed[bin_of(13.0)]), (1, 1));
    }

    /// **Only LOCK's periodic advertising is timed.** An ADV_IND in LOCK
    /// is kept with its stream position and channel; the same packet while
    /// surveying is not, and neither is a scan response in LOCK, which is
    /// timed by whoever scanned.
    #[test]
    fn only_periodic_advertising_in_lock_is_timed() {
        use crate::signal::ble::pdu::PduType;
        let mut packet = crate::signal::ble::pdu::decode(&[false; 40]).unwrap();
        packet.crc_ok = true;
        packet.adv_addr = Some([1, 2, 3, 4, 5, 6]);
        packet.at_pair = Some(123_456);
        let now = Instant::now();

        packet.pdu_type = PduType::AdvInd;
        let mut devices = Vec::new();
        census_from_ble(&mut devices, &packet, 38, true, 8e6, now);
        let log = devices[0].arrivals.clone().expect("timed in LOCK");
        assert_eq!((log.channel, log.pairs[0]), (38, 123_456));

        let mut surveying = Vec::new();
        census_from_ble(&mut surveying, &packet, 38, false, 8e6, now);
        assert!(surveying[0].arrivals.is_none());

        packet.pdu_type = PduType::ScanRsp;
        let mut answered = Vec::new();
        census_from_ble(&mut answered, &packet, 38, true, 8e6, now);
        assert!(answered[0].arrivals.is_none());
    }

    /// **What the packet measured reaches the record**: its PDU type and,
    /// where B8 could take one, its modulation index; and a later packet
    /// from the same address whose CRC failed counts against it.
    #[test]
    fn a_packet_carries_its_type_and_modulation_and_a_failure_counts_against_it() {
        use crate::signal::dsp::uncertainty::Uncertain;
        let mut devices = Vec::new();
        let mut packet = crate::signal::ble::pdu::decode(&[false; 40]).unwrap();
        packet.crc_ok = true;
        packet.adv_addr = Some([1, 2, 3, 4, 5, 6]);
        packet.pdu_type = crate::signal::ble::pdu::PduType::ScanRsp;
        packet.modulation = Some(crate::signal::ble::measure::ModulationQuality {
            delta_f1_avg_hz: Uncertain::from_sigma(250e3, 5e3),
            delta_f2_avg_hz: crate::signal::dsp::uncertainty::Uncertain::from_sigma(230e3, 4e3),
            modulation_index: Uncertain::from_sigma(0.5, 0.01),
            ratio: Uncertain::from_sigma(0.9, 0.02),
        });
        census_from_ble(&mut devices, &packet, 37, false, 8e6, Instant::now());
        assert_eq!(devices[0].ble_pdu_codes().collect::<Vec<_>>(), vec![0x4]);
        assert_eq!(devices[0].modulation_index.unwrap().value(), 0.5);

        packet.crc_ok = false;
        census_from_ble(&mut devices, &packet, 37, false, 8e6, Instant::now());
        assert_eq!((devices[0].packets, devices[0].crc_failed), (1, 1));
    }

    /// **The census keeps ppm of the channel a packet was heard on**, so one
    /// crystal reads one number on all three advertising channels: 10 ppm is
    /// 24.02 kHz on channel 37 (2402 MHz) and 24.80 kHz on 39 (2480 MHz).
    /// Combined in Hz, the two would have disagreed by 3 % about one clock.
    #[test]
    fn one_crystal_reads_one_ppm_on_every_channel() {
        use crate::signal::dsp::uncertainty::Uncertain;
        let mut devices = Vec::new();
        let mut packet = crate::signal::ble::pdu::decode(&[false; 40]).unwrap();
        packet.crc_ok = true;
        packet.adv_addr = Some([1, 2, 3, 4, 5, 6]);
        packet.freq_offset_hz = Some(Uncertain::from_sigma(24_020.0, 100.0));
        census_from_ble(&mut devices, &packet, 37, false, 8e6, Instant::now());
        packet.freq_offset_hz = Some(Uncertain::from_sigma(24_800.0, 100.0));
        census_from_ble(&mut devices, &packet, 39, false, 8e6, Instant::now());
        let ppm = devices[0].crystal_offset_ppm.unwrap();
        assert!((ppm.value() - 10.0).abs() < 1e-9, "got {}", ppm.value());
    }

    /// A confirmed packet with no address at all - a PDU type that carries
    /// none, per `pdu::decode`'s own contract - has nothing to key a census
    /// row on, and is skipped rather than inventing an address.
    #[test]
    fn a_passed_crc_with_no_address_is_skipped() {
        let mut devices = Vec::new();
        let mut packet = crate::signal::ble::pdu::decode(&[false; 40]).unwrap();
        packet.crc_ok = true;
        packet.adv_addr = None;
        census_from_ble(&mut devices, &packet, 37, false, 8e6, Instant::now());
        assert!(devices.is_empty(), "{devices:?}");
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
    /// publishes a load. The unit tests above hold the arithmetic; this holds
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

    /// Wall time over stream time, reported once a window is covered.
    #[test]
    fn the_load_is_wall_time_over_the_stream_time_it_covered() {
        use std::time::Duration;
        let mut load = Load::default();
        // 1 MHz, 100 000 pairs a block: 0.1 s of stream each, 25 ms to handle.
        for _ in 0..4 {
            assert_eq!(
                load.add(Duration::from_millis(25), 100_000, 1e6),
                None,
                "under half a second of stream, no reading yet"
            );
        }
        let reading = load
            .add(Duration::from_millis(25), 100_000, 1e6)
            .expect("five blocks cover the window");
        assert!((reading - 0.25).abs() < 1e-9, "{reading}");
        // And the next window starts from nothing.
        assert_eq!(load.add(Duration::from_millis(25), 100_000, 1e6), None);
    }

    /// Above one means the worker cannot keep up with the stream, and the
    /// figure must be free to say so rather than be clamped.
    #[test]
    fn a_load_above_one_is_reported_not_clamped() {
        use std::time::Duration;
        let mut load = Load::default();
        let reading = load
            .add(Duration::from_millis(900), 600_000, 1e6)
            .expect("0.6 s of stream covers the window");
        assert!((reading - 1.5).abs() < 1e-9, "{reading}");
    }

    /// A worker too slow to cover a window of stream still reports within
    /// half a second of work, at the figure it really is - the case a
    /// stream-time window alone would hide longest.
    #[test]
    fn an_overloaded_worker_reports_on_work_time_alone() {
        use std::time::Duration;
        let mut load = Load::default();
        // 10 000 pairs at 1 Msps is 10 ms of stream; each takes 200 ms.
        assert_eq!(load.add(Duration::from_millis(200), 10_000, 1e6), None);
        assert_eq!(load.add(Duration::from_millis(200), 10_000, 1e6), None);
        let reading = load
            .add(Duration::from_millis(200), 10_000, 1e6)
            .expect("0.6 s of work closes the window");
        assert!((reading - 20.0).abs() < 1e-9, "{reading}");
    }

    /// A rate that is not a positive number covers no stream time: no
    /// reading, rather than a division that invents one.
    #[test]
    fn no_rate_means_no_load() {
        use std::time::Duration;
        let mut load = Load::default();
        for rate in [0.0, -1.0, f64::NAN] {
            assert_eq!(load.add(Duration::from_secs(1), 1_000_000, rate), None);
        }
    }

    /// B15's own exit condition: a view too narrow for
    /// `signal::bt::receive::front_end`'s own working rate is refused, with
    /// a reason - the same "refused, not silent" discipline `ble_refused`
    /// already follows, now genuinely exercised by classic Bluetooth rather
    /// than being B14's unconditional placeholder.
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

    /// B15's own exit condition, run through the actual worker rather than
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

    /// B17's own exit condition, run through the actual worker: a synthetic
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

        // 6.3: resolved by this very payload, the same header is then read
        // under the UAP: a DH1 from LT_ADDR 2, counted on the roster.
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

        // 6.4: the header's own symbols give the modulation index the
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

        // 6.6: the header joins its own hit, so the export can say what the
        // hit carried.
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

        // 6.6: every one of those hits exports its residual from that grid.
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
