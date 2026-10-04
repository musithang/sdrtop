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
//! touch and read the same block, so [`push_all`] runs them on scoped
//! threads for that block and hands their answers back in the order one
//! thread would have produced them: the same results, in less time on a
//! machine with more than one core.
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

// The views the classic receiver runs for (the Classic and the Piconet
// view): a plain string comparison, because that is what the menu is keyed
// by, and the registry's structural tests hold the strings and the preset
// files' names to agreeing. A view left out would show an empty list forever.
use super::lock::CLASSIC_VIEWS;
use super::scan::Scan;

/// The survey watches one classic channel fewer once its measured load
/// passes this, and one more once it falls under [`SURVEY_LOAD_LOW`], up to
/// the Classic view's own cap. The survey is there to measure the band: a
/// classic receiver that pushed it past real time would cost it blocks, and
/// every duty cycle on screen with them. Measured on the i3 before this was
/// built: the survey alone 0.61x at 8 Msps and 1.00x at 20, a classic
/// channel about 0.18x and 0.28x more, so room for about one channel at 8
/// and none at 20; a faster machine gets more.
const SURVEY_LOAD_HIGH: f64 = 0.8;
const SURVEY_LOAD_LOW: f64 = 0.7;

/// The survey's classic channel count after a load `reading`: one fewer
/// over [`SURVEY_LOAD_HIGH`], one more under [`SURVEY_LOAD_LOW`] up to `cap`,
/// unchanged between them, so it settles rather than hunting.
fn survey_budget(now: usize, reading: f64, cap: usize) -> usize {
    if reading > SURVEY_LOAD_HIGH {
        now.saturating_sub(1)
    } else if reading < SURVEY_LOAD_LOW {
        (now + 1).min(cap)
    } else {
        now
    }
}

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

/// The blocks the measurement path may cut a burst from: those held from
/// before, and this one.
/// How much of the stream is held while a connection is followed, s: its
/// first event's transmit window (up to 10 ms, Core 5.4 Vol 6 Part B 4.5.3),
/// the widening either side, and an event's length after.
const FOLLOW_HELD_S: f64 = 0.02;

/// Listened to before an event's earliest start, us: the receiver's filters
/// and its detector settle on it, and the preamble's lead is in it.
const FOLLOW_WARMUP_US: f64 = 64.0;

/// The longest an event is listened to after its anchor, us: a Central's
/// longest LE 1M packet (2120 us), T_IFS and an answer as long. A longer
/// event's later packets are not heard; its first exchange is.
const EVENT_SPAN_US: f64 = 4_500.0;

/// At most this many events accounted a block per connection: catching up
/// after a gap, the rest wait for the next block rather than stall this one.
const FOLLOW_ROUNDS: usize = 32;

/// The stretch of stream an event is listened to over, in pairs: from its
/// earliest start less the warm-up to its latest anchor plus an event's
/// length, never into the next event.
fn event_window(
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
fn connect_end_pair(p: &crate::signal::ble::pdu::Packet, rate_hz: f64) -> Option<f64> {
    let bit = rate_hz / 1e6;
    p.pdu_pair
        .map(|centre| centre + (crate::signal::ble::pdu::used_bits(p.length) as f64 - 0.5) * bit)
}

/// An LE Coded packet measured as the test suite defines it, from the
/// symbols its decoded bits were sent as: `None` when its samples are not
/// held, or it is not a Coded packet.
fn measure_coded(
    p: &crate::signal::ble::pdu::Packet,
    channel: u8,
    window: &super::measure::Recent,
    rate_hz: f64,
    centre_hz: f64,
) -> Option<super::measure::CodedReading> {
    let coding = p.coding?;
    let symbols = crate::signal::ble::coded::symbols_of(
        crate::signal::ble::detect::ADVERTISING_ACCESS_ADDRESS,
        coding,
        &p.air,
    );
    let offset = crate::signal::ble::channel::centre_hz(channel)? as f64 - centre_hz;
    super::measure::le_coded(window, rate_hz, offset, p.at_pair? as f64, &symbols, coding)
}

/// An extended advertising PDU's header, read, in `role`: `None` for any
/// other type, a failed CRC (whose header bytes say nothing), or a payload
/// that is not the extended format.
fn extended(
    p: &crate::signal::ble::pdu::Packet,
    role: crate::state::ExtRole,
) -> Option<crate::state::ExtInfo> {
    if !p.crc_ok || p.pdu_type != crate::signal::ble::pdu::PduType::Other(0x07) {
        return None;
    }
    let header = crate::signal::ble::ext::parse(&p.payload).ok()?;
    Some(crate::state::ExtInfo {
        header,
        role,
        aux: crate::signal::ble::aux::AuxOutcome::NonePromised,
    })
}

/// An LE Coded packet into the LE Coded list, numbered as it arrives.
fn push_coded(
    m: &mut SdrMetrics,
    p: crate::signal::ble::pdu::Packet,
    channel: u8,
    reading: Option<super::measure::CodedReading>,
    ext: Option<crate::state::ExtInfo>,
    now: Instant,
) {
    let Some(coding) = p.coding else { return };
    m.net.coded_heard += 1;
    let seq = m.net.coded_heard;
    // An auxiliary packet names its advertiser in its extended header.
    let adv_addr = p
        .adv_addr
        .or_else(|| ext.as_ref().and_then(|e| e.header.adv_a));
    m.net.coded_packets.push_front(BlePacket {
        seq,
        phy: crate::signal::ble::Phy::Coded(coding),
        channel,
        pdu_type: p.pdu_type,
        ch_sel: p.ch_sel,
        tx_add_random: p.tx_add_random,
        rx_add_random: p.rx_add_random,
        length: p.length,
        adv_addr,
        payload: p.payload,
        crc_ok: p.crc_ok,
        snr_db: reading.and_then(|r| r.snr_db),
        // The preamble's f0, as BV-14-C reads it, is the carrier offset the
        // list's column shows.
        freq_offset_hz: reading.and_then(|r| r.drift).map(|d| d.initial_hz),
        modulation: None,
        drift: None,
        seen: now,
        coded: Some(crate::state::CodedFacts {
            fec_repairs: p.fec_repairs.unwrap_or(0),
            reading,
        }),
        ext,
    });
    m.net.coded_packets.truncate(crate::state::BLE_PACKET_LIMIT);
}

/// What became of the AuxPtr of the Coded packet numbered `seq`, where it
/// is still in the list.
fn set_aux_outcome(m: &mut SdrMetrics, seq: u64, outcome: crate::signal::ble::aux::AuxOutcome) {
    if let Some(e) = m
        .net
        .coded_packets
        .iter_mut()
        .find(|p| p.seq == seq)
        .and_then(|p| p.ext.as_mut())
    {
        e.aux = outcome;
    }
}

/// Put the LE Coded receiver down, if there is one: a capture it had under
/// way ends as given up in the funnel rather than with the receiver.
fn put_down(
    coded: &mut Option<crate::signal::ble::coded_rx::CodedReceiver>,
    state: &Arc<Mutex<SdrMetrics>>,
) {
    let Some(rx) = coded.take() else { return };
    let funnel = rx.finish();
    if funnel != crate::signal::ble::receive::Funnel::default() {
        let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
        m.net.health.coded.add(funnel);
    }
}

/// Promises that can no longer be kept, because the stream broke or the
/// view that listens for them closed: each ends as lost to the feed, and is
/// counted, rather than vanishing.
fn abandon(promises: &mut Vec<crate::signal::ble::aux::Promise>, state: &Arc<Mutex<SdrMetrics>>) {
    if promises.is_empty() {
        return;
    }
    let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
    for p in promises.drain(..) {
        let outcome = crate::signal::ble::aux::AuxOutcome::FeedLost;
        m.net.health.aux.count(&outcome);
        set_aux_outcome(&mut m, p.superior_seq, outcome);
    }
}

fn held<'a>(
    recent: &'a std::collections::VecDeque<(u64, Vec<num_complex::Complex<f32>>)>,
    current: Option<(u64, &'a [num_complex::Complex<f32>])>,
) -> super::measure::Recent<'a> {
    super::measure::Recent::new(
        recent
            .iter()
            .map(|(p, v)| (*p, v.as_slice()))
            .chain(current),
    )
}

/// What one classic receiver heard in a block: its hits and its headers.
type Heard = (
    Vec<crate::signal::bt::receive::AccessHit>,
    Vec<crate::signal::bt::receive::HeaderHit>,
);

/// Every classic receiver of the fleet fed `iq`, their answers in fleet
/// order, the receivers spread over the machine's cores.
///
/// **Parallel because nothing is shared, and so exact.** Each receiver owns
/// its mixer, filter, lanes and captures, and reads the same block; run on
/// several threads and put back in order, they give what one thread gives,
/// bit for bit. On the i3 the Classic view at 4 Msps was 1.4 times real time
/// on one core, most of it three receivers doing the same work side by side.
/// A block is milliseconds long, so starting the threads for each is lost in
/// it.
fn push_fleet(fleet: &mut [BtReceiver], iq: &[num_complex::Complex<f32>]) -> Vec<Heard> {
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let threads = cores.min(fleet.len());
    if threads <= 1 {
        return fleet.iter_mut().map(|rx| rx.push_iq(iq)).collect();
    }
    let per = fleet.len().div_ceil(threads);
    std::thread::scope(|scope| {
        let running: Vec<_> = fleet
            .chunks_mut(per)
            .map(|chunk| {
                scope.spawn(move || {
                    chunk
                        .iter_mut()
                        .map(|rx| rx.push_iq(iq))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        running
            .into_iter()
            .flat_map(|t| {
                t.join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
            })
            .collect()
    })
}

/// The BLE receiver, when there is one to feed, and the classic fleet, fed
/// the same block at once: the BLE receiver on a thread of its own beside
/// [`push_fleet`]'s. Each owns everything it touches, so running them side
/// by side gives what running them one after the other gave, and the BLE
/// receiver's packets are handled after the fleet's hits are in, both in
/// the order they always were.
fn push_all(
    ble: Option<&mut BleReceiver>,
    fleet: &mut [BtReceiver],
    iq: &[num_complex::Complex<f32>],
    first_pair: u64,
) -> (Option<Vec<crate::signal::ble::pdu::Packet>>, Vec<Heard>) {
    let Some(rx) = ble else {
        return (None, push_fleet(fleet, iq));
    };
    // Nothing to run beside it: a thread started and joined for one decoder
    // is a cost with no second decoder to pay for it.
    if fleet.is_empty() {
        return (Some(rx.push_iq_at(iq, first_pair)), Vec::new());
    }
    std::thread::scope(|scope| {
        let packets = scope.spawn(move || rx.push_iq_at(iq, first_pair));
        let answers = push_fleet(fleet, iq);
        let packets = packets
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
        (Some(packets), answers)
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
    /// How many classic channels the survey starts with, before its first
    /// load reading: none, because room for a receiver is measured, not
    /// assumed. A test sets it, since the reading follows the clock.
    pub survey_bt_start: usize,
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
        // How many classic channels the survey watches: grown and shrunk by
        // the measured load (`SURVEY_LOAD_HIGH`).
        let mut survey_bt = self.survey_bt_start;
        // Whether the classic account was last published load-limited; `None`
        // until it has been published at all. The fleet alone cannot say:
        // a survey with no room starts empty and stays empty, and an empty
        // fleet that never changed would otherwise never say why.
        let mut bt_said: Option<bool> = None;
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
        let mut promises: Vec<crate::signal::ble::aux::Promise> = Vec::new();

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
                put_down(&mut coded, &self.state);
                abandon(&mut promises, &self.state);
                bt.clear();
                recent.clear();
            }
            next_pair = Some(first_pair + pairs);
            // The tuning and rate these samples were captured at, from the
            // block rather than the state: see `StreamBlock::centre_hz`.
            let centre_hz = centre_hz as f64;

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
                ble = None;
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
                abandon(&mut promises, &self.state);
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
                    if ble.is_some() {
                        ble_on = Some(ch);
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
                let mut wanted = crate::signal::bt::channel::channels_in_span(centre_hz, span_hz);
                wanted.sort_by_key(|&ch| {
                    let f = crate::signal::bt::channel::centre_hz(ch).unwrap_or(0) as f64;
                    (f - centre_hz).abs() as u64
                });
                let could = wanted.len().min(self.bt_channels);
                let cap = if is_net_bt {
                    self.bt_channels
                } else {
                    survey_bt.min(self.bt_channels)
                };
                let load_limited = cap < could;
                wanted.truncate(cap);
                wanted.sort_unstable();

                let current: Vec<u8> = bt.iter().map(|r| r.channel()).collect();
                let stale_tuning = bt
                    .first()
                    .is_some_and(|r| !r.matches(r.channel(), rate_hz, centre_hz));
                if current != wanted || stale_tuning || bt_said != Some(load_limited) {
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
                    bt_said = Some(load_limited);
                    let mut m = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    m.net.bt_load_limited = load_limited;
                    m.net.bt_refused = if wanted.is_empty() && load_limited {
                        Some("not running: the survey's load leaves no room".to_string())
                    } else if wanted.is_empty() {
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
                let (ble_out, answers) =
                    push_all(ble_on.and(ble.as_mut()), &mut bt, block, first_pair);
                ble_packets = ble_out;
                for (rx, (laps, headers)) in bt.iter().zip(answers) {
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
                let window = held(&recent, iq.as_deref().map(|v| (first_pair, v)));
                for hit in &header_hits {
                    let clock = piconet_clocks.entry(hit.lap).or_default();
                    clock.observe(hit.at_us, &hit.whitened);
                    // The UAPs still standing, each at the clocks the
                    // piconet's own clock gives it for this header: never the
                    // first clock that happens to fit, which about one
                    // header in five reads as another packet type.
                    let standing = match resolved_bt_uap.get(&hit.lap) {
                        Some(&uap) => vec![uap],
                        None => clock.narrowed(),
                    };
                    let pairs: Vec<(u8, u8)> = standing
                        .iter()
                        .flat_map(|&uap| {
                            clock
                                .clocks_for(uap, hit.at_us)
                                .into_iter()
                                .map(move |clk6| (uap, clk6))
                        })
                        .collect();
                    // One pair: the header is read there. More: this hit's
                    // own payload gets one attempt at choosing, UAP and
                    // clock together; the clock it checks out at pins the
                    // piconet's clock, so the headers after it have one.
                    let (shown, read_at) = match pairs.as_slice() {
                        [pair] => (vec![pair.0], Some(*pair)),
                        _ => {
                            match payload::break_uap_tie(&pairs, &hit.whitened, &hit.payload_raw) {
                                Some((uap, clk6)) => {
                                    resolved_bt_uap.insert(hit.lap, uap);
                                    clock.pin(hit.at_us, clk6);
                                    (vec![uap], Some((uap, clk6)))
                                }
                                None => (standing, None),
                            }
                        }
                    };
                    let read = match (shown.as_slice(), read_at) {
                        (_, Some((uap, clk6))) => {
                            match crate::signal::bt::header::decode_at(&hit.whitened, uap, clk6) {
                                Some(h) => crate::signal::bt::piconet::HeaderRead::Decoded(h),
                                None => crate::signal::bt::piconet::HeaderRead::Undecoded,
                            }
                        }
                        // One UAP, two clocks and no payload to choose: two
                        // different headers, so neither is claimed.
                        ([_], None) => crate::signal::bt::piconet::HeaderRead::Undecoded,
                        _ => crate::signal::bt::piconet::HeaderRead::Unresolved,
                    };
                    // Who sent it: the slot parity of the clock it was read
                    // at, and only once it was read there. And what its
                    // payload told: checked where sdrtop can read the type,
                    // never guessed where it cannot.
                    let direction = match read {
                        crate::signal::bt::piconet::HeaderRead::Decoded(h) => {
                            Some(crate::signal::bt::piconet::Direction::of_clk6(h.clk6))
                        }
                        _ => None,
                    };
                    let payload = {
                        use crate::signal::bt::header::PacketType;
                        use crate::signal::bt::piconet::{HeaderRead, PayloadVerdict};
                        match (read, read_at) {
                            (HeaderRead::Decoded(h), Some((uap, clk6))) => match h.packet_type {
                                PacketType::Null | PacketType::Poll => PayloadVerdict::NoPayload,
                                t => match payload::check_crc(&hit.payload_raw, clk6, t, uap) {
                                    Ok(ok) => PayloadVerdict::Crc(ok),
                                    // Why, in the list's words: never "PSK",
                                    // which would be a guess about the link.
                                    Err(why) => PayloadVerdict::NotRead(why.words()),
                                },
                            },
                            _ => PayloadVerdict::NotRead("clock not known"),
                        }
                    };
                    // The header read again from the raw samples, as the
                    // test suites define its readings (`measure::classic`).
                    let channel_hz = crate::signal::bt::channel::centre_hz(hit.ch);
                    let measured = channel_hz
                        .and_then(|hz| {
                            let r = super::measure::classic(
                                &window,
                                rate_hz,
                                hz as f64 - centre_hz,
                                hit.lap,
                                hit.sync_end_pair,
                            )?;
                            let carrier = r
                                .carrier
                                .map(|(_, drift)| {
                                    crate::signal::bt::piconet::Carrier::of(&drift, hz as f64)
                                })
                                .unwrap_or_default();
                            let f0_ppm = r
                                .carrier
                                .map(|(_, drift)| drift.initial_hz.scale(1e6 / hz as f64));
                            Some((r.deviation, carrier, f0_ppm))
                        })
                        .unwrap_or_default();
                    headers_read.push((
                        hit.lap,
                        hit.at_us,
                        clock.hypotheses(),
                        crate::signal::bt::piconet::PacketReading {
                            header: read,
                            direction,
                            deviation: measured.0,
                            carrier: measured.1,
                            f0_ppm: measured.2,
                            payload,
                        },
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
                        // Every hit is a packet, read or not: an ID row
                        // until a header is joined to it.
                        crate::signal::bt::piconet::observe_packet(
                            &mut m.net.bt_piconets,
                            lap,
                            crate::signal::bt::piconet::BtPacket {
                                seen: now,
                                at_us,
                                stream: stream_id,
                                channel,
                                header: None,
                                direction: None,
                                deviation: Default::default(),
                                carrier: Default::default(),
                                f0_ppm: None,
                                payload: crate::signal::bt::piconet::PayloadVerdict::NoPayload,
                            },
                        );
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
                    for (lap, at_us, hypotheses, reading) in headers_read {
                        // Joined to its hit by LAP and time: the header's
                        // capture starts on the lane that found the access
                        // code, which may be a quarter-symbol lane off the
                        // one the hit was dated by.
                        if let Some(hop) = m.net.bt_hops.iter_mut().find(|h| {
                            h.lap == lap && h.stream == stream_id && (h.at_us - at_us).abs() < 2.0
                        }) {
                            hop.header = Some(reading.header);
                        }
                        crate::signal::bt::piconet::observe_header(
                            &mut m.net.bt_piconets,
                            lap,
                            reading.header,
                            hypotheses,
                            reading.deviation,
                            reading.carrier,
                        );
                        crate::signal::bt::piconet::read_packet(
                            &mut m.net.bt_piconets,
                            lap,
                            stream_id,
                            at_us,
                            reading,
                        );
                    }
                }
            } else {
                // Not on this preset: no receiver to run, and a refusal or a
                // watched-channel list from a previous visit must not linger
                // onto a screen that never claimed to be this one.
                bt.clear();
                bt_said = None;
                let mut m = self.state.lock().unwrap_or_else(|e| e.into_inner());
                m.net.bt_refused = None;
                m.net.bt_channels_watched.clear();
                m.net.bt_load_limited = false;
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
                            coded: None,
                            ext: None,
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
                            let (in_view, feed_lost, heard) =
                                (heard.in_view, heard.feed_lost, heard.data);
                            done.push((aa, due.counter, in_view, feed_lost, heard));
                        }
                        if done.is_empty() {
                            break;
                        }
                        let mut m = self.state.lock().unwrap_or_else(|e| e.into_inner());
                        for (aa, counter, in_view, feed_lost, heard) in done {
                            if let Some(f) = m
                                .net
                                .ble_connections
                                .iter_mut()
                                .find(|f| f.connection.access_address() == aa)
                            {
                                // Still the event this was for.
                                if f.connection.next_due().counter == counter {
                                    f.connection.account(in_view, feed_lost, heard);
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
            // decoded bits were sent as; then every AuxPtr whose window this
            // block completes, listened to where it promised.
            if let (Some(ch), Some(rx), Some(this)) = (coded_on, coded.as_mut(), iq.as_deref()) {
                let window = held(&recent, Some((first_pair, this)));
                let held_end = (first_pair + this.len() as u64) as f64;
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
                            e.aux = match start.map(|s| {
                                crate::signal::ble::aux::promise(seq, s, &e.header, 0, rate_hz)
                            }) {
                                Some(Ok(promise)) => {
                                    promises.push(promise);
                                    crate::signal::ble::aux::AuxOutcome::Pending
                                }
                                Some(Err(outcome)) => {
                                    m.net.health.aux.count(&outcome);
                                    outcome
                                }
                                None => crate::signal::ble::aux::AuxOutcome::NonePromised,
                            };
                            e
                        });
                        push_coded(&mut m, p, ch, reading, ext, now);
                    }
                }
                // Promises whose windows are now held, in the order made.
                let (due, waiting): (Vec<_>, Vec<_>) =
                    promises.drain(..).partition(|p| p.to_pair <= held_end);
                promises = waiting;
                for promise in due {
                    let ear = match promise.phy {
                        crate::signal::ble::aux::AuxPhy::Coded => super::listen::Ear::Coded,
                        crate::signal::ble::aux::AuxPhy::OneM => super::listen::Ear::Link(
                            crate::signal::ble::receive::Link::Advertising,
                            crate::signal::ble::Phy::OneM,
                        ),
                        crate::signal::ble::aux::AuxPhy::TwoM => super::listen::Ear::Link(
                            crate::signal::ble::receive::Link::Advertising,
                            crate::signal::ble::Phy::TwoM,
                        ),
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
                        let phy = crate::signal::ble::Phy::Coded(p.coding?);
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
                        crate::signal::ble::aux::keeps(&promise, phy, &ext.header)
                            .then_some((p, ext))
                    });
                    let mut m = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    let outcome = if !out.in_view {
                        crate::signal::ble::aux::AuxOutcome::NotInView
                    } else if out.feed_lost {
                        crate::signal::ble::aux::AuxOutcome::FeedLost
                    } else if let Some((p, mut ext)) = kept {
                        drop(m);
                        let reading =
                            measure_coded(&p, promise.channel, &window, rate_hz, centre_hz);
                        m = self.state.lock().unwrap_or_else(|e| e.into_inner());
                        let seq = m.net.coded_heard + 1;
                        let after_us = p.at_pair.map_or(0.0, |a| {
                            (a as f64 - promise.superior_start_pair) / rate_hz * 1e6
                        });
                        // Its own AuxPtr, if any, is the chain's next link.
                        ext.aux = match p.at_pair.map(|a| {
                            crate::signal::ble::aux::promise(
                                seq,
                                a as f64,
                                &ext.header,
                                promise.depth + 1,
                                rate_hz,
                            )
                        }) {
                            Some(Ok(next)) => {
                                promises.push(next);
                                crate::signal::ble::aux::AuxOutcome::Pending
                            }
                            Some(Err(o)) => {
                                m.net.health.aux.count(&o);
                                o
                            }
                            None => crate::signal::ble::aux::AuxOutcome::NonePromised,
                        };
                        push_coded(&mut m, p, promise.channel, reading, Some(ext), now);
                        crate::signal::ble::aux::AuxOutcome::Heard { seq, after_us }
                    } else {
                        crate::signal::ble::aux::AuxOutcome::Missed
                    };
                    m.net.health.aux.count(&outcome);
                    set_aux_outcome(&mut m, promise.superior_seq, outcome);
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
                    let held_s = if following || is_coded {
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
                ble = None;
                put_down(&mut coded, &self.state);
                bt.clear();
                load = Load::default();
                survey_bt = self.survey_bt_start;
                bt_said = None;
                next_pair = None;
                let mut m = self.state.lock().unwrap_or_else(|e| e.into_inner());
                m.net.bt_refused = None;
                m.net.bt_channels_watched.clear();
                m.net.bt_load_limited = false;
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
                    survey_bt = survey_budget(survey_bt, reading, self.bt_channels);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::SampleFormat;

    /// The survey's classic budget moves one channel at a time, down over
    /// the high mark, up under the low one to the cap, and holds between.
    #[test]
    fn the_survey_budget_follows_the_load_it_measured() {
        assert_eq!(survey_budget(0, 0.5, 8), 1);
        assert_eq!(survey_budget(8, 0.5, 8), 8, "capped");
        assert_eq!(survey_budget(3, 0.95, 8), 2);
        assert_eq!(survey_budget(0, 0.95, 8), 0);
        assert_eq!(survey_budget(3, 0.75, 8), 3, "held between the marks");
    }

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
        use crate::signal::ble::aux::AuxPhy;
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
        use crate::signal::ble::aux::AuxOutcome;
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
        use crate::signal::ble::aux::AuxOutcome;
        let m = scene_view(&[(38, 0x07, adv_ext_ind(30), 20_000)]);
        assert_eq!(m.net.coded_packets.len(), 1);
        let ext = m.net.coded_packets[0].ext.as_ref().unwrap();
        assert_eq!(ext.aux, AuxOutcome::NotInView);
        assert_eq!(m.net.health.aux.not_in_view, 1);
    }

    /// In view, listened to, and nothing there: missed.
    #[test]
    fn an_aux_not_sent_is_missed() {
        use crate::signal::ble::aux::AuxOutcome;
        let m = scene_view(&[(38, 0x07, adv_ext_ind(9), 20_000)]);
        let ext = m.net.coded_packets[0].ext.as_ref().unwrap();
        assert_eq!(ext.aux, AuxOutcome::Missed);
        assert_eq!(m.net.health.aux.missed, 1);
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
