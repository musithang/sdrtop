// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Classic Bluetooth's receivers: how many the survey can afford, and the
//! fleet run side by side with the BLE receiver on one block.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::signal::ble::receive::Receiver as BleReceiver;
use crate::signal::bt::header::PiconetClock;
use crate::signal::bt::payload;
use crate::signal::bt::piconet::Inquiry;
use crate::signal::bt::receive::Receiver as BtReceiver;
use crate::state::{BtHop, SdrMetrics};

use super::Tuning;

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
pub(super) fn survey_budget(now: usize, reading: f64, cap: usize) -> usize {
    if reading > SURVEY_LOAD_HIGH {
        now.saturating_sub(1)
    } else if reading < SURVEY_LOAD_LOW {
        (now + 1).min(cap)
    } else {
        now
    }
}

/// What one classic receiver heard in a block: its hits and its headers.
pub(super) type Heard = (
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
pub(super) fn push_fleet(fleet: &mut [BtReceiver], iq: &[num_complex::Complex<f32>]) -> Vec<Heard> {
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
pub(super) fn push_all(
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

/// One header as [`Classic::read_header`] read it, for publishing.
struct ReadHeader {
    lap: u32,
    at_us: f64,
    /// How many clock hypotheses its piconet's clock still holds.
    hypotheses: u8,
    reading: crate::signal::bt::piconet::PacketReading,
    /// The UAPs its LAP is shown with: one once read or resolved.
    shown: Vec<u8>,
}

/// A piconet's slot grid, where it gets one, and its pace.
type Fit = (
    u32,
    Option<Result<crate::signal::bt::slots::SlotFit, crate::signal::bt::slots::SlotRefusal>>,
    crate::signal::bt::slots::Pace,
);

/// Everything classic Bluetooth carries from one block to the next: the
/// fleet of receivers, and what each piconet has taught the worker so far.
pub(super) struct Classic {
    /// One receiver per channel watched, in channel order.
    pub(super) fleet: Vec<BtReceiver>,
    /// Each LAP's clock, narrowing its UAP header by header.
    clocks: HashMap<u32, PiconetClock>,
    /// The live UAP tie-break, one LAP at a time: once resolved, a LAP's real
    /// UAP does not change (it comes from the piconet master's own fixed
    /// address), so this sticks the same way
    /// `signal::net::census::Device::first_seen` never moves on a repeat
    /// sighting - a later header from a packet type
    /// `payload::break_uap_tie` cannot read (POLL, FHS, ...) must not flip a
    /// resolved answer back to two candidates.
    resolved_uap: HashMap<u32, u8>,
    /// Each piconet's access-code times, us on the stream's clock, for its
    /// slot grid (`signal::bt::slots`), kept here rather than in the state
    /// because only the fit is shown. A new stream or a new rate restarts
    /// the logs: their times are then on another clock.
    arrivals: HashMap<u32, VecDeque<f64>>,
    /// The rate `arrivals` were dated at.
    arrivals_rate: f64,
    /// Which stream the times are on: bumped with every restart of the
    /// clock they count on, so a hop, a header and a grid are compared only
    /// within one.
    stream_id: u32,
    /// When each piconet's slot grid was last fitted.
    last_fit: HashMap<u32, Instant>,
    /// Piconets with hits their last fit has not seen: refitted once the
    /// interval allows, whether or not another hit comes, so a piconet that
    /// falls silent still has its last hits in its figure.
    unfitted: HashSet<u32>,
    /// How many classic channels the survey watches: grown and shrunk by the
    /// measured load ([`SURVEY_LOAD_HIGH`]).
    survey_channels: usize,
    /// What `survey_channels` starts from, and returns to on closing.
    survey_start: usize,
    /// Whether the classic account was last published load-limited; `None`
    /// until it has been published at all. The fleet alone cannot say: a
    /// survey with no room starts empty and stays empty, and an empty fleet
    /// that never changed would otherwise never say why.
    said: Option<bool>,
}

impl Classic {
    pub(super) fn new(survey_start: usize) -> Self {
        Self {
            fleet: Vec::new(),
            clocks: HashMap::new(),
            resolved_uap: HashMap::new(),
            arrivals: HashMap::new(),
            arrivals_rate: 0.0,
            stream_id: 0,
            last_fit: HashMap::new(),
            unfitted: HashSet::new(),
            survey_channels: survey_start,
            survey_start,
            said: None,
        }
    }

    /// A new stream: its clock starts again, so what each piconet's clock
    /// learned from the old one's timing no longer applies. A resolved UAP
    /// does - a piconet's address does not change - so it is kept.
    pub(super) fn new_stream(&mut self) {
        self.clocks.clear();
        self.arrivals.clear();
        self.unfitted.clear();
        self.stream_id = self.stream_id.wrapping_add(1);
    }

    /// The rate this block was captured at: a new one restarts the arrival
    /// logs, whose times were counted at the old one.
    pub(super) fn rate(&mut self, rate_hz: f64) {
        if rate_hz != self.arrivals_rate {
            self.arrivals.clear();
            self.unfitted.clear();
            self.arrivals_rate = rate_hz;
            self.stream_id = self.stream_id.wrapping_add(1);
        }
    }

    /// The fleet for this block: one receiver per channel the tuning and the
    /// cap together let it watch, closest to the tuned centre first,
    /// `capacity` of them on a classic view (`classic_view`) and on the
    /// survey only as many as its measured load leaves room for. Rebuilt,
    /// and published, when the channels, the tuning or the reason changes.
    pub(super) fn watch(
        &mut self,
        tuning: Tuning,
        classic_view: bool,
        capacity: usize,
        state: &Arc<Mutex<SdrMetrics>>,
    ) {
        let mut wanted =
            crate::signal::bt::channel::channels_in_span(tuning.centre_hz, tuning.span_hz);
        wanted.sort_by_key(|&ch| {
            let f = crate::signal::bt::channel::centre_hz(ch).unwrap_or(0) as f64;
            (f - tuning.centre_hz).abs() as u64
        });
        let could = wanted.len().min(capacity);
        let cap = if classic_view {
            capacity
        } else {
            self.survey_channels.min(capacity)
        };
        let load_limited = cap < could;
        wanted.truncate(cap);
        wanted.sort_unstable();

        let current: Vec<u8> = self.fleet.iter().map(|r| r.channel()).collect();
        let stale_tuning = self
            .fleet
            .first()
            .is_some_and(|r| !r.matches(r.channel(), tuning.rate_hz, tuning.centre_hz));
        if current != wanted || stale_tuning || self.said != Some(load_limited) {
            let mut fleet = Vec::with_capacity(wanted.len());
            let mut refusal = None;
            for &ch in &wanted {
                match BtReceiver::new(tuning.rate_hz, ch, tuning.centre_hz, tuning.first_pair) {
                    Ok(r) => fleet.push(r),
                    Err(e) => {
                        refusal.get_or_insert(e);
                    }
                }
            }
            self.fleet = fleet;
            self.said = Some(load_limited);
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            m.net.bt_load_limited = load_limited;
            m.net.bt_refused = if wanted.is_empty() && load_limited {
                Some("not running: the survey's load leaves no room".to_string())
            } else if wanted.is_empty() {
                Some(format!(
                    "no classic Bluetooth channel fits inside the current {:.1} MHz view",
                    tuning.span_hz / 1e6
                ))
            } else {
                refusal
            };
            m.net.bt_channels_watched = self.fleet.iter().map(|r| r.channel()).collect();
            m.net.bt_capacity = capacity;
        }
    }

    /// What the fleet heard in this block, `answers` in fleet order: each
    /// hit logged for its piconet's slot grid, each header read, the grids
    /// due a fit fitted, and all of it published in one lock block.
    pub(super) fn read(
        &mut self,
        answers: Vec<Heard>,
        window: &crate::signal::net::measure::Recent,
        tuning: Tuning,
        now: Instant,
        fit_every: std::time::Duration,
        state: &Arc<Mutex<SdrMetrics>>,
    ) {
        let mut hits = Vec::new();
        let mut header_hits = Vec::new();
        for (rx, (laps, headers)) in self.fleet.iter().zip(answers) {
            for hit in laps {
                hits.push((rx.channel(), hit.lap, hit.at_us));
                let log = self.arrivals.entry(hit.lap).or_default();
                if log.len() == crate::signal::bt::slots::KEPT {
                    log.pop_front();
                }
                log.push_back(hit.at_us);
                self.unfitted.insert(hit.lap);
            }
            header_hits.extend(headers.into_iter().filter(|h| Inquiry::of(h.lap).is_none()));
        }
        // Read outside the lock: narrowing does real work (64 dewhitenings
        // per header), the same reasoning every other float or device-free
        // computation in this worker stays outside the lock block for.
        let headers: Vec<_> = header_hits
            .iter()
            .map(|hit| self.read_header(hit, window, tuning))
            .collect();
        let fits = self.fit_due(now, fit_every);
        if !hits.is_empty() || !headers.is_empty() || !fits.is_empty() {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            self.publish(&mut m, hits, headers, fits, now);
        }
    }

    /// One header, fed to its LAP's own `PiconetClock`, read where the clock
    /// allows, and measured again from the raw samples in `window` through
    /// the tester's filter (`measure`).
    ///
    /// **The UAP tie-break rides alongside it.** A LAP already resolved
    /// shows its one confirmed UAP and does no further work at all -
    /// `resolved_uap`'s own doc says why a later, unresolvable header must
    /// not undo this. Otherwise, a header narrowed to more than one candidate
    /// gets one attempt at `payload::break_uap_tie` using this same hit's own
    /// captured payload; success resolves the LAP for good, failure (an
    /// unsupported packet type, or simply not enough real payload behind this
    /// particular header) falls back to showing the still-honest candidate
    /// set `PiconetClock` itself reports.
    fn read_header(
        &mut self,
        hit: &crate::signal::bt::receive::HeaderHit,
        window: &crate::signal::net::measure::Recent,
        tuning: Tuning,
    ) -> ReadHeader {
        let clock = self.clocks.entry(hit.lap).or_default();
        clock.observe(hit.at_us, &hit.whitened);
        // The UAPs still standing, each at the clocks the piconet's own clock
        // gives it for this header: never the first clock that happens to fit,
        // which about one header in five reads as another packet type.
        let standing = match self.resolved_uap.get(&hit.lap) {
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
        // One pair: the header is read there. More: this hit's own payload gets
        // one attempt at choosing, UAP and clock together; the clock it checks
        // out at pins the piconet's clock, so the headers after it have one.
        let (shown, read_at) = match pairs.as_slice() {
            [pair] => (vec![pair.0], Some(*pair)),
            _ => match payload::break_uap_tie(&pairs, &hit.whitened, &hit.payload_raw) {
                Some((uap, clk6)) => {
                    self.resolved_uap.insert(hit.lap, uap);
                    clock.pin(hit.at_us, clk6);
                    (vec![uap], Some((uap, clk6)))
                }
                None => (standing, None),
            },
        };
        let read = match (shown.as_slice(), read_at) {
            (_, Some((uap, clk6))) => {
                match crate::signal::bt::header::decode_at(&hit.whitened, uap, clk6) {
                    Some(h) => crate::signal::bt::piconet::HeaderRead::Decoded(h),
                    None => crate::signal::bt::piconet::HeaderRead::Undecoded,
                }
            }
            // One UAP, two clocks and no payload to choose: two different
            // headers, so neither is claimed.
            ([_], None) => crate::signal::bt::piconet::HeaderRead::Undecoded,
            _ => crate::signal::bt::piconet::HeaderRead::Unresolved,
        };
        // Who sent it: the slot parity of the clock it was read at, and only
        // once it was read there. And what its payload told: checked where
        // sdrtop can read the type, never guessed where it cannot.
        let direction = match read {
            crate::signal::bt::piconet::HeaderRead::Decoded(h) => {
                Some(crate::signal::bt::piconet::Direction::of_clk6(h.clk6))
            }
            _ => None,
        };
        // What it carries, read only from a payload whose CRC passed.
        let (payload, content) = {
            use crate::signal::bt::header::PacketType;
            use crate::signal::bt::piconet::{content_of, HeaderRead, PayloadVerdict};
            match (read, read_at) {
                (HeaderRead::Decoded(h), Some((uap, clk6))) => match h.packet_type {
                    PacketType::Null | PacketType::Poll => (PayloadVerdict::NoPayload, None),
                    t => match payload::read_payload(&hit.payload_raw, clk6, t, uap) {
                        Ok(p) => (PayloadVerdict::Crc(p.crc_ok), content_of(&p, t)),
                        // Why, in the list's words: never "PSK",
                        // which would be a guess about the link.
                        Err(why) => (PayloadVerdict::NotRead(why.words()), None),
                    },
                },
                _ => (PayloadVerdict::NotRead("clock not known"), None),
            }
        };
        // The header read again from the raw samples, as the test suites define
        // its readings (`measure::classic`).
        let channel_hz = crate::signal::bt::channel::centre_hz(hit.ch);
        let measured = channel_hz
            .and_then(|hz| {
                let r = crate::signal::net::measure::classic(
                    window,
                    tuning.rate_hz,
                    hz as f64 - tuning.centre_hz,
                    hit.lap,
                    hit.sync_end_pair,
                )?;
                let carrier = r
                    .carrier
                    .map(|(_, drift)| crate::signal::bt::piconet::Carrier::of(&drift, hz as f64))
                    .unwrap_or_default();
                let f0_ppm = r
                    .carrier
                    .map(|(_, drift)| drift.initial_hz.scale(1e6 / hz as f64));
                Some((r.deviation, carrier, f0_ppm))
            })
            .unwrap_or_default();
        ReadHeader {
            lap: hit.lap,
            at_us: hit.at_us,
            hypotheses: clock.hypotheses(),
            reading: crate::signal::bt::piconet::PacketReading {
                header: read,
                direction,
                deviation: measured.0,
                carrier: measured.1,
                f0_ppm: measured.2,
                payload,
                content,
            },
            shown,
        }
    }

    /// The slot fit of each piconet due one, at most once `fit_every` a
    /// piconet: a rate search over hundreds of hits is real work, and a
    /// jitter figure does not need refreshing faster.
    fn fit_due(&mut self, now: Instant, fit_every: std::time::Duration) -> Vec<Fit> {
        let due: Vec<u32> = self
            .unfitted
            .iter()
            .copied()
            .filter(|lap| {
                self.last_fit
                    .get(lap)
                    .is_none_or(|t| now.saturating_duration_since(*t) >= fit_every)
            })
            .collect();
        // An inquiry code is every searching device's at once, so it gets no
        // slot grid of one piconet. Every LAP gets its pace (`slots::pace`),
        // cheap and burst by burst: the timing half of telling inquiry and
        // paging from a piconet's traffic.
        let mut fits = Vec::with_capacity(due.len());
        for lap in due {
            use crate::signal::bt::slots;
            let times: Vec<f64> = self
                .arrivals
                .get(&lap)
                .map(|l| l.iter().copied().collect())
                .unwrap_or_default();
            let inquiry = Inquiry::of(lap).is_some();
            let whole = (!inquiry).then(|| slots::fit(&times));
            fits.push((lap, whole, slots::pace(&times)));
            self.last_fit.insert(lap, now);
            self.unfitted.remove(&lap);
        }
        fits
    }

    /// This block's hits, headers and fits into the state, inside the
    /// caller's lock.
    fn publish(
        &self,
        m: &mut SdrMetrics,
        hits: Vec<(u8, u32, f64)>,
        mut headers: Vec<ReadHeader>,
        fits: Vec<Fit>,
        now: Instant,
    ) {
        m.net.health.bt_hits += hits.len() as u64;
        for (channel, lap, at_us) in hits {
            crate::signal::bt::piconet::observe(&mut m.net.bt_piconets, lap, channel, now);
            m.net.bt_hops.push_front(BtHop {
                channel,
                lap,
                seen: now,
                at_us,
                stream: self.stream_id,
                header: None,
            });
            // Every hit is a packet, read or not: an ID row until a header is
            // joined to it.
            crate::signal::bt::piconet::observe_packet(
                &mut m.net.bt_piconets,
                lap,
                crate::signal::bt::piconet::BtPacket {
                    seen: now,
                    at_us,
                    stream: self.stream_id,
                    channel,
                    header: None,
                    direction: None,
                    deviation: Default::default(),
                    carrier: Default::default(),
                    f0_ppm: None,
                    payload: crate::signal::bt::piconet::PayloadVerdict::NoPayload,
                    content: None,
                },
            );
        }
        m.net.bt_hops.truncate(crate::state::BT_HOP_LIMIT);
        for h in &mut headers {
            m.net.bt_uap.insert(h.lap, std::mem::take(&mut h.shown));
        }
        for (lap, whole, pace) in fits {
            if let Some(p) = m.net.bt_piconets.iter_mut().find(|p| p.lap == lap) {
                if let Some(fit) = whole {
                    p.slots = Some(fit);
                    p.slots_stream = self.stream_id;
                }
                p.pace = pace;
            }
        }
        for ReadHeader {
            lap,
            at_us,
            hypotheses,
            reading,
            ..
        } in headers
        {
            // Joined to its hit by LAP and time: the header's capture starts on
            // the lane that found the access code, which may be a
            // quarter-symbol lane off the one the hit was dated by.
            if let Some(hop) = m.net.bt_hops.iter_mut().find(|h| {
                h.lap == lap && h.stream == self.stream_id && (h.at_us - at_us).abs() < 2.0
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
            number_lmp_address(&mut m.net.address_book, reading.content.as_ref());
            crate::signal::bt::piconet::read_packet(
                &mut m.net.bt_piconets,
                lap,
                self.stream_id,
                at_us,
                reading,
            );
        }
    }

    /// Not on a classic view: no receiver to run, and a refusal or a
    /// watched-channel list from a previous visit must not linger onto a
    /// screen that never claimed to be this one.
    pub(super) fn stand_down(&mut self, state: &Arc<Mutex<SdrMetrics>>) {
        self.fleet.clear();
        self.said = None;
        let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
        Self::unpublish(&mut m);
    }

    /// The section closed: the fleet goes, and the survey starts again from
    /// no channel, since its room is measured afresh.
    pub(super) fn close(&mut self) {
        self.fleet.clear();
        self.said = None;
        self.survey_channels = self.survey_start;
    }

    /// What the classic account showed, cleared, inside the caller's lock.
    pub(super) fn unpublish(m: &mut SdrMetrics) {
        m.net.bt_refused = None;
        m.net.bt_channels_watched.clear();
        m.net.bt_load_limited = false;
    }

    /// A survey's load reading: its classic channel count follows it, one
    /// fewer over the high mark, one more under the low one.
    pub(super) fn loaded(&mut self, reading: f64, capacity: usize) {
        self.survey_channels = survey_budget(self.survey_channels, reading, capacity);
    }
}

/// A BD_ADDR an LMP message carries gets its session number the moment it
/// reaches the state, as an advertiser's address does, so the masked mode
/// can name it by number on any panel.
fn number_lmp_address(
    book: &mut crate::state::AddressBook,
    content: Option<&crate::signal::bt::piconet::PayloadContent>,
) {
    use crate::signal::bt::lmp::Identifying;
    use crate::signal::bt::piconet::PayloadContent;
    if let Some(PayloadContent::Lmp(m)) = content {
        if let Some(Identifying::Address(a)) = m.identifying {
            book.number(a);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A BD_ADDR an LMP message carries gets its session number as an
    /// advertiser's address does, so a masked screen names it by number;
    /// nothing else is numbered.
    #[test]
    fn an_lmp_address_is_numbered_like_an_advertiser() {
        use crate::signal::bt::piconet::PayloadContent;
        let mut book = crate::state::AddressBook::default();
        book.number([9; 6]);
        let slot_offset = crate::signal::bt::lmp::parse(&[52 << 1, 0, 0, 6, 5, 4, 3, 2, 1])
            .map(PayloadContent::Lmp);
        number_lmp_address(&mut book, slot_offset.as_ref());
        assert_eq!(book.get([1, 2, 3, 4, 5, 6]), Some(2));

        let name =
            crate::signal::bt::lmp::parse(&[2 << 1, 0, 2, b'h', b'i']).map(PayloadContent::Lmp);
        number_lmp_address(&mut book, name.as_ref());
        number_lmp_address(&mut book, None);
        assert_eq!(book.number([7; 6]), 3, "nothing else was numbered");
    }

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
}
