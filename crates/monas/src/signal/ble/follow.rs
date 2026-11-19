// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Following a BLE connection from its CONNECT_IND: when and where each of
//! its events will be, in the radio's own stream.
//!
//! Plain data in and out: a CONNECT_IND's parameters and the stream
//! position its packet ended at go in, the next event's channel, expected
//! anchor and listening window come out, and what was heard there goes back
//! in. No radio, no clock but the stream's.
//!
//! **The timing, from Core 5.4 Vol 6 Part B**, read on the SIG's site:
//! - 4.5.3: "the start of the first packet will be no earlier than
//!   transmitWindowDelay + transmitWindowOffset and no later than
//!   transmitWindowDelay + transmitWindowOffset + transmitWindowSize after
//!   the end of the packet containing the CONNECT_IND PDU", the delay 1.25 ms
//!   for a CONNECT_IND, the offset and size in 1.25 ms units.
//! - 4.5.4: "The first packet sent in the Connection State by the Central
//!   determines the anchor point for the first connection event, and
//!   therefore the timings of all future connection events"; 4.5.1: "The
//!   start of connection events are spaced regularly with an interval of
//!   connInterval", in 1.25 ms units.
//! - 4.2.4, window widening: the listener widens its window by the
//!   transmitter's clock accuracy times the time since it last synchronised,
//!   plus a fixed allowance, "16 when the sleep clock applies" in us.
//!
//! **Whose clocks.** The Central's accuracy is the SCA its CONNECT_IND
//! declares, at the top of its range (Table 2.11). This radio's own clock is
//! [`OWN_CLOCK_PPM`]: an assumption, not a reading, stated where it is used.
//! The anchors heard will measure the Central's clock against this radio's,
//! which is the measurement, not this window.

use super::connect::{sca_ppm, ConnectIndData, Csa1, Csa2};
use super::data::DataPdu;
use super::llcp;
use super::receive::DataTiming;
use super::Phy;
use crate::signal::dsp::uncertainty::Uncertain;

/// The accuracy this radio's own sample clock is assumed to keep, in ppm,
/// for the listening window only: an assumption, neither measured nor read
/// from a radio's data sheet. Too narrow a window loses events, too wide
/// costs only samples, so it errs wide; a frequency reference does not
/// narrow it yet.
pub const OWN_CLOCK_PPM: f64 = 20.0;

/// 4.2.4's fixed allowance when the sleep clock applies, us: the larger of
/// its two, since which clock the Central runs is not known here.
const WIDENING_FIXED_US: f64 = 16.0;

/// The CONNECT_IND's transmitWindowDelay, us (4.5.3).
const TRANSMIT_WINDOW_DELAY_US: f64 = 1250.0;

/// The unit of WinOffset, WinSize and Interval, us.
const UNIT_US: f64 = 1250.0;

/// The next event a follower should listen for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Due {
    pub counter: u16,
    /// Its data channel index.
    pub channel: u8,
    /// Where its anchor is expected, stream pairs, fractional.
    pub anchor_pair: f64,
    /// How far either side of it to listen, pairs (4.2.4).
    pub widening_pairs: f64,
    /// While no anchor has been heard, the transmit window the first one
    /// fell in (4.5.3): listen from `anchor_pair - widening_pairs` to
    /// `anchor_pair + window_pairs + widening_pairs`. Zero after.
    pub window_pairs: f64,
}

/// Which channel selection algorithm the connection uses, with its state.
#[derive(Clone, Debug)]
enum Hops {
    One(Csa1),
    Two(Csa2),
}

/// Where the timing is anchored: an event's counter and where its anchor
/// was heard, in pairs.
#[derive(Clone, Copy, Debug)]
struct Anchor {
    counter: u16,
    pair: f64,
}

/// Before an anchor is heard, where the first one can be: the transmit
/// window after the CONNECT_IND (4.5.3), or after a connection update's
/// instant, opening at `open_pair` for event `counter`, `size_pairs` wide.
/// `sync_pair` is where the timing was last known, for the widening.
#[derive(Clone, Copy, Debug)]
struct Window {
    counter: u16,
    open_pair: f64,
    size_pairs: f64,
    sync_pair: f64,
}

/// How an event in a followed connection went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Account {
    /// In view, and a packet with the connection's access address heard.
    Followed,
    /// In view, nothing heard. The Peripheral may skip events (its
    /// latency) and the Central send nothing; neither is claimed.
    Missed,
    /// Its channel is outside the band the radio sees.
    NotInView,
    /// The feed lost samples inside its window: not listened to.
    FeedLost,
    /// Its samples were held, but no receiver for its PHY can run at this
    /// sample rate: not listened to.
    CannotReceive,
}

/// Whether an event's window was listened to, and if not, why.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Listened {
    Yes,
    NotInView,
    FeedLost,
    CannotReceive,
}

/// Who sent a packet: the Central opens each event at its anchor (4.5.1),
/// and the two take turns T_IFS apart (4.1.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sender {
    Central,
    Peripheral,
}

/// One packet heard in an event.
#[derive(Clone, Debug, PartialEq)]
pub struct HeardPdu {
    pub pdu: DataPdu,
    pub timing: DataTiming,
    /// `None` when neither the anchor nor a turn after a placed packet
    /// accounts for its time: never guessed.
    pub sender: Option<Sender>,
    /// An LL Control PDU, read when the link is not yet encrypted.
    pub control: Option<llcp::Control>,
    /// From the end of the packet before it to its start, us, when that
    /// packet was the other end's turn.
    pub t_ifs_us: Option<f64>,
}

/// One event, as it went.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub counter: u16,
    pub channel: u8,
    pub account: Account,
    pub pdus: Vec<HeardPdu>,
    /// The PHY the Central was to send it on.
    pub phy: Phy,
}

/// Where following stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    Following,
    /// The events in view went silent past the supervision timeout (4.5.2)
    /// or the widening's limit (4.2.4): after the last event heard, if
    /// any. Most often a change made outside the window, never seen.
    Lost {
        after: Option<u16>,
    },
    /// LL_TERMINATE_IND heard, with its reason.
    Terminated {
        reason: u8,
    },
    /// A change this follower cannot follow was seen.
    NotFollowed {
        why: &'static str,
    },
}

/// How many events a connection keeps, newest first.
pub const EVENTS_KEPT: usize = 500;

/// How many anchors heard the Central's clock is fitted to, the newest.
const ANCHORS_KEPT: usize = 2_000;

/// The Inter Frame Space's tolerance, us: "the start of a packet is
/// transmitted 150±2 µs after the end of the previous" (4.2.1).
pub const T_IFS_TOLERANCE_US: f64 = 2.0;

/// The turns heard, pooled: T_IFS's mean with its standard error, how many,
/// and how many fell outside 150 ± [`T_IFS_TOLERANCE_US`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TIfs {
    pub mean: Uncertain,
    pub count: usize,
    pub outside: usize,
}

/// T_IFS, us (4.1.1).
const T_IFS_US: f64 = 150.0;

/// How far from T_IFS a packet may start and still be taken as the other
/// end's turn, us: far wider than the Core's 2 us (4.2.1), so a late
/// answer is placed and its T_IFS judged against the limit, not dropped;
/// far narrower than any other gap an event has.
const TURN_SLACK_US: f64 = 25.0;

/// In view and missed, at least this many times, before a silence is taken
/// as loss: one or two misses are a weak packet or the Peripheral's latency.
const LOSS_MISSES: u32 = 3;

/// One connection, followed.
#[derive(Clone, Debug)]
pub struct Connection {
    params: ConnectIndData,
    raw_rate: f64,
    hops: Hops,
    /// The next event not yet accounted for, and its channel.
    counter: u16,
    channel: u8,
    window: Window,
    anchor: Option<Anchor>,
    /// Each direction's PHY.
    phy: Phy,
    phy_peripheral: Phy,
    /// Changes waiting for their instant.
    pending: Vec<llcp::Update>,
    events: std::collections::VecDeque<Event>,
    state: State,
    encrypted_from: Option<u16>,
    /// The last event a packet was heard in, and where its first packet
    /// started.
    last_heard: Option<(u16, f64)>,
    /// Events in view missed since then.
    misses: u32,
    /// Events accounted for since the CONNECT_IND, never wrapping: the
    /// x of the clock fit, where the 16-bit counter wraps at 65536.
    index: u64,
    /// Anchors heard, `(index, pair)`, for the Central's clock: since the
    /// last connection update, which changes the interval they are on.
    anchors: std::collections::VecDeque<(u64, f64)>,
    /// Packets heard and CRCs passed, per data channel.
    per_channel: [(u32, u32); 37],
}

impl Connection {
    /// From a CONNECT_IND whose packet ended at `end_pair` in a stream of
    /// `raw_rate` pairs a second, `ch_sel` its header's ChSel bit (set:
    /// Channel Selection Algorithm #2). `None` for a channel map with no
    /// used channel, which no connection can hop on.
    pub fn new(c: &ConnectIndData, ch_sel: bool, end_pair: f64, raw_rate: f64) -> Option<Self> {
        let hops = if ch_sel {
            Hops::Two(Csa2::new(c.access_address, c.channel_map)?)
        } else {
            Hops::One(Csa1::new(c.hop_increment, c.channel_map)?)
        };
        let unit = UNIT_US * 1e-6 * raw_rate;
        let mut this = Self {
            params: *c,
            raw_rate,
            hops,
            counter: 0,
            channel: 0,
            window: Window {
                counter: 0,
                open_pair: end_pair
                    + TRANSMIT_WINDOW_DELAY_US * 1e-6 * raw_rate
                    + c.win_offset as f64 * unit,
                size_pairs: c.win_size as f64 * unit,
                sync_pair: end_pair,
            },
            anchor: None,
            // A legacy CONNECT_IND is sent on LE 1M, and the connection
            // starts on the PHY it was made on.
            phy: Phy::OneM,
            phy_peripheral: Phy::OneM,
            pending: Vec::new(),
            events: std::collections::VecDeque::new(),
            state: State::Following,
            encrypted_from: None,
            last_heard: None,
            misses: 0,
            index: 0,
            anchors: std::collections::VecDeque::new(),
            per_channel: [(0, 0); 37],
        };
        this.channel = this.hop();
        Some(this)
    }

    pub fn access_address(&self) -> u32 {
        self.params.access_address
    }

    pub fn crc_init(&self) -> u32 {
        self.params.crc_init
    }

    /// The parameters now in force: the CONNECT_IND's, with every
    /// connection update and channel map applied since.
    pub fn params(&self) -> &ConnectIndData {
        &self.params
    }

    /// Follow no further, and say why: for a connection whose schedule
    /// cannot be known from what was heard.
    pub fn refuse(&mut self, why: &'static str) {
        self.state = State::NotFollowed { why };
    }

    /// Which Channel Selection Algorithm the connection hops by, 1 or 2,
    /// as its CONNECT_IND's ChSel named it.
    pub fn algorithm(&self) -> u8 {
        match self.hops {
            Hops::One(_) => 1,
            Hops::Two(_) => 2,
        }
    }

    /// The PHY the Central sends on.
    pub fn phy(&self) -> Phy {
        self.phy
    }

    /// The PHY the Peripheral sends on.
    pub fn phy_peripheral(&self) -> Phy {
        self.phy_peripheral
    }

    /// The events accounted for, newest first.
    pub fn events(&self) -> &std::collections::VecDeque<Event> {
        &self.events
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    /// The Central's clock against this radio's, ppm, positive when the
    /// Central's runs fast: a least-squares line through the anchors heard
    /// (the event's index against where its anchor was, in pairs), its slope
    /// against the interval the Central was to keep (4.5.1), with the
    /// slope's standard error from the anchors' scatter about the line. The
    /// sign is the classic slot clock's (`signal::bt::slots`, read through
    /// the Piconets panel): a radio reference corrects both the same way.
    /// `None` with fewer than three anchors.
    pub fn clock_ppm(&self) -> Option<Uncertain> {
        let n = self.anchors.len();
        if n < 3 {
            return None;
        }
        let nf = n as f64;
        let mx = self.anchors.iter().map(|a| a.0 as f64).sum::<f64>() / nf;
        let my = self.anchors.iter().map(|a| a.1).sum::<f64>() / nf;
        let sxx: f64 = self.anchors.iter().map(|a| (a.0 as f64 - mx).powi(2)).sum();
        if sxx <= 0.0 {
            return None;
        }
        let sxy: f64 = self
            .anchors
            .iter()
            .map(|a| (a.0 as f64 - mx) * (a.1 - my))
            .sum();
        let slope = sxy / sxx;
        let ssr: f64 = self
            .anchors
            .iter()
            .map(|a| (a.1 - my - slope * (a.0 as f64 - mx)).powi(2))
            .sum();
        let slope_sigma = (ssr / (nf - 2.0) / sxx).sqrt();
        let interval = self.interval_pairs();
        Some(Uncertain::from_sigma(
            (1.0 - slope / interval) * 1e6,
            slope_sigma / interval * 1e6,
        ))
    }

    /// How many anchors the clock is fitted to.
    pub fn anchors_heard(&self) -> usize {
        self.anchors.len()
    }

    /// The turns heard in the events kept, pooled (4.1.1).
    pub fn t_ifs(&self) -> Option<TIfs> {
        let gaps: Vec<f64> = self
            .events
            .iter()
            .flat_map(|e| e.pdus.iter().filter_map(|p| p.t_ifs_us))
            .collect();
        if gaps.is_empty() {
            return None;
        }
        let n = gaps.len() as f64;
        let mean = gaps.iter().sum::<f64>() / n;
        let sd = if gaps.len() > 1 {
            (gaps.iter().map(|g| (g - mean).powi(2)).sum::<f64>() / (n - 1.0)).sqrt()
        } else {
            0.0
        };
        Some(TIfs {
            mean: Uncertain::from_sigma(mean, sd / n.sqrt()),
            count: gaps.len(),
            outside: gaps
                .iter()
                .filter(|g| (*g - T_IFS_US).abs() > T_IFS_TOLERANCE_US)
                .count(),
        })
    }

    /// Packets heard and CRCs passed, per data channel, over the
    /// connection.
    pub fn per_channel(&self) -> &[(u32, u32); 37] {
        &self.per_channel
    }

    /// The event LL_START_ENC_REQ was heard in: every packet after it is
    /// encrypted (Vol 6 Part C 1 sends it in the clear and its answers
    /// encrypted).
    pub fn encrypted_from(&self) -> Option<u16> {
        self.encrypted_from
    }

    /// The event `self.counter`'s channel, advancing CSA #1's state.
    fn hop(&mut self) -> u8 {
        match &mut self.hops {
            Hops::One(csa) => csa.next(),
            Hops::Two(csa) => csa.channel(self.counter).0,
        }
    }

    fn pairs(&self, us: f64) -> f64 {
        us * 1e-6 * self.raw_rate
    }

    fn interval_pairs(&self) -> f64 {
        self.pairs(self.params.interval as f64 * UNIT_US)
    }

    /// The next event to listen for.
    pub fn next_due(&self) -> Due {
        let interval = self.interval_pairs();
        let (anchor_pair, window_pairs, since) = match self.anchor {
            Some(a) => {
                let at = a.pair + (self.counter.wrapping_sub(a.counter)) as f64 * interval;
                (at, 0.0, at - a.pair)
            }
            None => {
                let w = self.window;
                let at = w.open_pair + self.counter.wrapping_sub(w.counter) as f64 * interval;
                (at, w.size_pairs, at + w.size_pairs - w.sync_pair)
            }
        };
        let ppm = sca_ppm(self.params.sca).1 as f64 + OWN_CLOCK_PPM;
        Due {
            counter: self.counter,
            channel: self.channel,
            anchor_pair,
            widening_pairs: ppm * 1e-6 * since + self.pairs(WIDENING_FIXED_US),
            window_pairs,
        }
    }

    /// The event due was heard, its anchor at `anchor_pair`: the timing is
    /// fixed from it, and the next event is due.
    pub fn heard(&mut self, anchor_pair: f64) {
        self.anchor = Some(Anchor {
            counter: self.counter,
            pair: anchor_pair,
        });
        self.anchors.push_back((self.index, anchor_pair));
        if self.anchors.len() > ANCHORS_KEPT {
            self.anchors.pop_front();
        }
        self.advance();
    }

    /// The event due passed without its anchor heard: the next is due, on
    /// the timing as it was.
    pub fn missed(&mut self) {
        self.advance();
    }

    fn advance(&mut self) {
        self.counter = self.counter.wrapping_add(1);
        self.index += 1;
        self.apply_due_updates();
        self.channel = self.hop();
    }

    /// The changes whose instant is the event now due, before its channel
    /// is chosen: from the instant on, the new parameters are in force
    /// (5.1.1, 5.1.2, 5.1.10).
    fn apply_due_updates(&mut self) {
        let now = self.counter;
        let (due, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut self.pending)
            .into_iter()
            .partition(|u| instant_of(u) == Some(now));
        self.pending = waiting;
        for update in due {
            match update {
                llcp::Update::ChannelMap { map, .. } => {
                    let ok = match &mut self.hops {
                        Hops::One(csa) => csa.set_map(map),
                        Hops::Two(csa) => match Csa2::new(self.params.access_address, map) {
                            Some(new) => {
                                *csa = new;
                                true
                            }
                            None => false,
                        },
                    };
                    if ok {
                        self.params.channel_map = map;
                    }
                }
                llcp::Update::Phy { c_to_p, p_to_c, .. } => {
                    if let Some(phy) = phy_of(c_to_p) {
                        self.phy = phy;
                    }
                    if let Some(phy) = phy_of(p_to_c) {
                        self.phy_peripheral = phy;
                    }
                    if c_to_p & 0b100 != 0 || p_to_c & 0b100 != 0 {
                        self.state = State::NotFollowed {
                            why: "moved to LE Coded: not followed",
                        };
                    }
                }
                llcp::Update::Connection {
                    win_size,
                    win_offset,
                    interval,
                    latency,
                    timeout,
                    ..
                } => {
                    // The old timing's anchor at the instant, then the new
                    // transmit window after it, as at the start (reasoned
                    // from 5.1.1's procedure, not yet seen on the air).
                    let old = self.next_due();
                    let unit = self.pairs(UNIT_US);
                    self.window = Window {
                        counter: now,
                        open_pair: old.anchor_pair + win_offset as f64 * unit,
                        size_pairs: win_size as f64 * unit,
                        sync_pair: old.anchor_pair - old.widening_pairs,
                    };
                    self.anchor = None;
                    // The anchors before are on the old interval.
                    self.anchors.clear();
                    self.params.interval = interval;
                    self.params.latency = latency;
                    self.params.timeout = timeout;
                }
                llcp::Update::Terminate { .. } | llcp::Update::Subrate => {}
            }
        }
    }

    /// Account for the event due: whether it was in view, whether the feed
    /// lost samples in its window, and the packets heard there with the
    /// link's access address. Nothing is recorded once following has ended.
    pub fn account(&mut self, listened: Listened, mut heard: Vec<(DataPdu, DataTiming)>) {
        if self.state != State::Following {
            return;
        }
        let due = self.next_due();
        heard.sort_by(|a, b| a.1.start_pair.total_cmp(&b.1.start_pair));
        // A receiver that could run (one PHY of two, mid-update) may still
        // have heard the event.
        let account = match listened {
            Listened::NotInView => Account::NotInView,
            Listened::FeedLost => Account::FeedLost,
            Listened::CannotReceive if heard.is_empty() => Account::CannotReceive,
            _ if heard.is_empty() => Account::Missed,
            _ => Account::Followed,
        };
        let opens = due.anchor_pair - due.widening_pairs;
        let closes = due.anchor_pair + due.window_pairs + due.widening_pairs;
        let mut pdus = Vec::with_capacity(heard.len());
        let mut before: Option<(Sender, f64)> = None;
        let mut central_at = None;
        let mut ends = None;
        for (pdu, timing) in heard {
            if let Some(n) = self.per_channel.get_mut(due.channel as usize) {
                n.0 += 1;
                n.1 += pdu.crc_ok as u32;
            }
            let (sender, t_ifs_us) = match before {
                None if (opens..=closes).contains(&timing.start_pair) => {
                    (Some(Sender::Central), None)
                }
                Some((last, end)) => {
                    let gap_us = (timing.start_pair - end) / self.raw_rate * 1e6;
                    if (gap_us - T_IFS_US).abs() <= TURN_SLACK_US {
                        let next = match last {
                            Sender::Central => Sender::Peripheral,
                            Sender::Peripheral => Sender::Central,
                        };
                        (Some(next), Some(gap_us))
                    } else {
                        (None, None)
                    }
                }
                None => (None, None),
            };
            if sender == Some(Sender::Central) && central_at.is_none() {
                central_at = Some(timing.start_pair);
            }
            before = sender.map(|s| (s, timing.end_pair));
            let control = (pdu.llid == 3 && pdu.crc_ok && self.encrypted_from.is_none())
                .then(|| llcp::read(&pdu.payload));
            if let Some(k) = &control {
                if k.opcode == 0x05 && k.name.is_some() {
                    self.encrypted_from = Some(due.counter);
                }
                match llcp::update(&pdu.payload) {
                    Some(llcp::Update::Terminate { reason }) => {
                        ends = Some(State::Terminated { reason });
                    }
                    Some(llcp::Update::Subrate) => {
                        ends = Some(State::NotFollowed {
                            why: "subrate change: not followed",
                        });
                    }
                    Some(update) if instant_of(&update).is_some() => self.pending.push(update),
                    _ => {}
                }
            }
            pdus.push(HeardPdu {
                pdu,
                timing,
                sender,
                control,
                t_ifs_us,
            });
        }
        let first_heard = pdus.first().map(|p| p.timing.start_pair);
        self.events.push_front(Event {
            counter: due.counter,
            channel: due.channel,
            account,
            pdus,
            phy: self.phy,
        });
        self.events.truncate(EVENTS_KEPT);

        match account {
            Account::Followed => {
                self.last_heard = first_heard.map(|at| (due.counter, at));
                self.misses = 0;
            }
            Account::Missed => self.misses += 1,
            Account::NotInView | Account::FeedLost | Account::CannotReceive => {}
        }
        if let Some(end) = ends {
            self.state = end;
            return;
        }
        // Loss: the events in view silent past the supervision timeout
        // (4.5.2), or the window widened past (connInterval / 2 - T_IFS)
        // (4.2.4).
        let since = due.anchor_pair - self.last_heard.map_or(self.window.sync_pair, |h| h.1);
        let timeout = self.pairs(self.params.timeout as f64 * 10_000.0);
        let widest = self.interval_pairs() / 2.0 - self.pairs(T_IFS_US);
        if (self.misses >= LOSS_MISSES && since > timeout) || due.widening_pairs >= widest {
            self.state = State::Lost {
                after: self.last_heard.map(|h| h.0),
            };
            return;
        }
        match central_at {
            Some(at) => self.heard(at),
            None => self.missed(),
        }
    }
}

/// Whether a connection hops by Channel Selection Algorithm #2, from the
/// ChSel bits of its CONNECT_IND and of the advertising PDU it answered:
/// "If the initiator sent a CONNECT_IND PDU in response to an ADV_IND or
/// ADV_DIRECT_IND PDU and either or both devices' PDU had the ChSel field set
/// to 0, then Channel Selection Algorithm #1 ... shall be used on the
/// connection. Otherwise, Channel Selection Algorithm #2" (Core 5.4 Vol 6
/// Part B 4.5). The initiator may set its bit when the advertiser does not
/// support #2 (2.3.3.1), so the CONNECT_IND alone does not say. `None`: its
/// bit is set and the advertising PDU was not heard.
pub fn uses_csa2(connect_ch_sel: bool, advertised_ch_sel: Option<bool>) -> Option<bool> {
    if !connect_ch_sel {
        return Some(false);
    }
    advertised_ch_sel
}

/// Whether an advertising PDU is one a CONNECT_IND can answer: connectable
/// (ADV_IND or ADV_DIRECT_IND, the two 4.5 names) and from its AdvA.
pub fn answers(t: super::pdu::PduType, adv_addr: Option<[u8; 6]>, c: &ConnectIndData) -> bool {
    use super::pdu::PduType;
    matches!(t, PduType::AdvInd | PduType::AdvDirectInd) && adv_addr == Some(c.adv_a)
}

/// The instant an update takes effect at, for the ones that have one.
fn instant_of(update: &llcp::Update) -> Option<u16> {
    match *update {
        llcp::Update::Connection { instant, .. } | llcp::Update::ChannelMap { instant, .. } => {
            Some(instant)
        }
        // Both directions unchanged: "there is no Instant" (2.4.2.23).
        llcp::Update::Phy {
            c_to_p: 0,
            p_to_c: 0,
            ..
        } => None,
        llcp::Update::Phy { instant, .. } => Some(instant),
        llcp::Update::Terminate { .. } | llcp::Update::Subrate => None,
    }
}

/// A PHY field's one set bit as the PHY this follower can receive (Table
/// 2.21); `None` for unchanged (zero) and for LE Coded, which it cannot.
fn phy_of(bits: u8) -> Option<Phy> {
    match bits {
        0b001 => Some(Phy::OneM),
        0b010 => Some(Phy::TwoM),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::ble::connect::{ConnectIndData, Csa1, Csa2};
    use crate::signal::ble::data::DataPdu;
    use crate::signal::ble::receive::DataTiming;

    fn params(interval: u16, win_offset: u16, win_size: u8) -> ConnectIndData {
        ConnectIndData {
            init_a: [0; 6],
            adv_a: [0; 6],
            access_address: 0x5065_4b6a,
            crc_init: 0x3a_5b7c,
            win_size,
            win_offset,
            interval,
            latency: 0,
            timeout: 100,
            channel_map: (1u64 << 37) - 1,
            hop_increment: 7,
            sca: 0,
        }
    }

    /// The first event's window is the transmit window: 1.25 ms plus the
    /// offset after the CONNECT_IND ends, WinSize long (4.5.3).
    #[test]
    fn the_first_event_is_looked_for_across_the_transmit_window() {
        let rate = 20e6;
        let c = Connection::new(&params(6, 2, 3), false, 1_000_000.0, rate).unwrap();
        let d = c.next_due();
        assert_eq!(d.counter, 0);
        let expect = 1_000_000.0 + (1.25e-3 + 2.0 * 1.25e-3) * rate;
        assert!((d.anchor_pair - expect).abs() < 1.0, "{d:?}");
        assert!((d.window_pairs - 3.0 * 1.25e-3 * rate).abs() < 1.0, "{d:?}");
        assert_eq!(c.access_address(), 0x5065_4b6a);
        assert_eq!(c.crc_init(), 0x3a_5b7c);
        assert_eq!(c.phy(), Phy::OneM);
    }

    /// Channels follow the algorithm the CONNECT_IND names.
    #[test]
    fn channels_follow_the_named_algorithm() {
        let p = params(6, 0, 1);
        let mut c1 = Connection::new(&p, false, 0.0, 20e6).unwrap();
        let mut csa1 = Csa1::new(7, p.channel_map).unwrap();
        let mut c2 = Connection::new(&p, true, 0.0, 20e6).unwrap();
        let csa2 = Csa2::new(p.access_address, p.channel_map).unwrap();
        for k in 0..20u16 {
            assert_eq!(c1.next_due().counter, k);
            assert_eq!(c1.next_due().channel, csa1.next());
            assert_eq!(c2.next_due().channel, csa2.channel(k).0);
            c1.missed();
            c2.missed();
        }
    }

    /// An anchor heard fixes the next ones a whole number of intervals
    /// later; before any is heard, they are counted from the transmit
    /// window, which stays their uncertainty.
    #[test]
    fn anchors_follow_the_interval() {
        let rate = 20e6;
        let interval = 80.0 * 1.25e-3 * rate;
        let mut c = Connection::new(&params(80, 4, 2), false, 0.0, rate).unwrap();
        let first = c.next_due();
        c.missed();
        let second = c.next_due();
        assert!((second.anchor_pair - first.anchor_pair - interval).abs() < 1.0);
        assert!(
            (second.window_pairs - first.window_pairs).abs() < 1.0,
            "{second:?}"
        );
        let heard_at = second.anchor_pair + 1234.0;
        c.heard(heard_at);
        let third = c.next_due();
        assert_eq!(third.counter, 2);
        assert!(
            (third.anchor_pair - heard_at - interval).abs() < 1.0,
            "{third:?}"
        );
        assert_eq!(third.window_pairs, 0.0);
    }

    /// Unheard events widen the window by the clocks' accuracy over the
    /// time since the last anchor heard (4.2.4); a heard one resets it.
    #[test]
    fn the_window_widens_until_an_anchor_is_heard() {
        let rate = 20e6;
        let mut c = Connection::new(&params(80, 0, 1), false, 0.0, rate).unwrap();
        let first = c.next_due().anchor_pair;
        c.heard(first + 3.0);
        let w1 = c.next_due().widening_pairs;
        c.missed();
        let w2 = c.next_due().widening_pairs;
        assert!(w2 > w1, "{w1} then {w2}");
        let at = c.next_due().anchor_pair;
        c.heard(at);
        assert!((c.next_due().widening_pairs - w1).abs() < 1e-6);
        // SCA 0 is up to 500 ppm (Table 2.11), this radio's own 20: over
        // 100 ms that is 52 us, and 4.2.4's 16 us besides.
        assert!(
            (w1 / rate * 1e6 - (520e-6 * 0.1e6 + 16.0)).abs() < 0.01,
            "{w1}"
        );
    }

    const RATE: f64 = 20e6;

    fn pdu(llid: u8, payload: &[u8]) -> DataPdu {
        DataPdu {
            llid,
            nesn: false,
            sn: false,
            md: false,
            cte_info: None,
            payload: payload.to_vec(),
            crc_ok: true,
        }
    }

    /// A 1M packet starting at `start`: preamble 8 bits, access address 32,
    /// header 16, payload, CRC 24, at 1 us a bit.
    fn at(start: f64, payload_len: usize) -> DataTiming {
        let bits = 8 + 32 + 16 + payload_len * 8 + 24;
        DataTiming {
            start_pair: start,
            end_pair: start + bits as f64 * 1e-6 * RATE,
        }
    }

    /// The Central's empty PDU at the anchor and the Peripheral's T_IFS
    /// after it.
    fn exchange(c: &Connection, t_ifs_us: f64) -> Vec<(DataPdu, DataTiming)> {
        let m = at(c.next_due().anchor_pair, 0);
        let s = at(m.end_pair + t_ifs_us * 1e-6 * RATE, 0);
        vec![(pdu(1, &[]), m), (pdu(1, &[]), s)]
    }

    /// The first PDU at the anchor is the Central's, the next T_IFS after
    /// it the Peripheral's (4.5.1, 4.1.1); T_IFS is measured.
    #[test]
    fn an_event_places_its_packets() {
        let mut c = Connection::new(&params(80, 0, 1), false, 0.0, RATE).unwrap();
        let heard = exchange(&c, 150.0);
        c.account(Listened::Yes, heard);
        let e = &c.events()[0];
        assert_eq!(e.account, Account::Followed);
        assert_eq!(e.pdus[0].sender, Some(Sender::Central));
        assert_eq!(e.pdus[1].sender, Some(Sender::Peripheral));
        assert!((e.pdus[1].t_ifs_us.unwrap() - 150.0).abs() < 0.1);
        assert_eq!(e.pdus[0].t_ifs_us, None);
    }

    /// An event no receiver could be built for (its PHY at this sample
    /// rate) was not listened to, and says so: not a miss, which would count
    /// towards declaring the link lost, and not a feed loss, since its
    /// samples were held.
    #[test]
    fn an_event_no_receiver_could_hear_says_so() {
        let mut c = Connection::new(&params(80, 0, 1), false, 0.0, RATE).unwrap();
        for _ in 0..50 {
            c.account(Listened::CannotReceive, vec![]);
        }
        assert!(c
            .events()
            .iter()
            .all(|e| e.account == Account::CannotReceive));
        assert_eq!(*c.state(), State::Following);
    }

    /// A PDU heard far from the anchor, with nothing before it, is heard
    /// but not placed, and does not move the timing.
    #[test]
    fn a_lone_late_packet_is_not_placed() {
        let mut c = Connection::new(&params(80, 0, 1), false, 0.0, RATE).unwrap();
        let heard = exchange(&c, 150.0);
        c.account(Listened::Yes, heard);
        let due = c.next_due().anchor_pair;
        let late = at(due + 400e-6 * RATE, 0);
        c.account(Listened::Yes, vec![(pdu(1, &[]), late)]);
        let e = &c.events()[0];
        assert_eq!(e.account, Account::Followed);
        assert_eq!(e.pdus[0].sender, None);
        let interval = 80.0 * 1.25e-3 * RATE;
        assert!((c.next_due().anchor_pair - due - interval).abs() < 1.0);
    }

    /// A channel map update seen applies at its instant: from then on only
    /// the channels it leaves in use.
    #[test]
    fn a_channel_map_update_applies_at_its_instant() {
        let mut c = Connection::new(&params(80, 0, 1), false, 0.0, RATE).unwrap();
        let a = c.next_due().anchor_pair;
        // LL_CHANNEL_MAP_IND: channels 0-9 only, instant 6.
        let ind = pdu(3, &[0x01, 0xff, 0x03, 0x00, 0x00, 0x00, 0x06, 0x00]);
        c.account(Listened::Yes, vec![(ind, at(a, 8))]);
        assert_eq!(
            c.events()[0].pdus[0].control.as_ref().map(|k| k.name),
            Some(Some("LL_CHANNEL_MAP_IND"))
        );
        while c.next_due().counter < 6 {
            c.account(Listened::NotInView, vec![]);
        }
        for _ in 0..30 {
            assert!(c.next_due().channel < 10, "{:?}", c.next_due());
            c.account(Listened::NotInView, vec![]);
        }
    }

    /// A PHY update to 2M moves the events from its instant to 2M.
    #[test]
    fn a_phy_update_moves_the_link_to_2m() {
        let mut c = Connection::new(&params(80, 0, 1), false, 0.0, RATE).unwrap();
        let a = c.next_due().anchor_pair;
        // LL_PHY_UPDATE_IND: C->P 2M, P->C 2M, instant 4.
        let ind = pdu(3, &[0x18, 0x02, 0x02, 0x04, 0x00]);
        c.account(Listened::Yes, vec![(ind, at(a, 5))]);
        assert_eq!(c.phy(), Phy::OneM);
        while c.next_due().counter < 4 {
            c.account(Listened::NotInView, vec![]);
        }
        assert_eq!(c.phy(), Phy::TwoM);
        c.account(Listened::NotInView, vec![]);
        assert_eq!(c.events()[0].phy, Phy::TwoM);
    }

    /// A connection update seen: at its instant the old timing's anchor
    /// plus WinOffset opens a new transmit window WinSize wide, and the
    /// events after it are the new interval apart.
    #[test]
    fn a_connection_update_opens_a_new_window_at_its_instant() {
        let mut c = Connection::new(&params(80, 0, 1), false, 0.0, RATE).unwrap();
        let a = c.next_due().anchor_pair;
        let old_interval = 80.0 * 1.25e-3 * RATE;
        // LL_CONNECTION_UPDATE_IND: WinSize 2, WinOffset 4, Interval 160,
        // Latency 0, Timeout 300, Instant 3.
        let ind = pdu(3, &[0x00, 2, 4, 0, 160, 0, 0, 0, 0x2c, 0x01, 3, 0]);
        c.account(Listened::Yes, vec![(ind, at(a, 12))]);
        c.account(Listened::NotInView, vec![]);
        c.account(Listened::NotInView, vec![]);
        let d = c.next_due();
        assert_eq!(d.counter, 3);
        let expect = a + 3.0 * old_interval + 4.0 * 1.25e-3 * RATE;
        assert!((d.anchor_pair - expect).abs() < 1.0, "{d:?}");
        assert!((d.window_pairs - 2.0 * 1.25e-3 * RATE).abs() < 1.0, "{d:?}");
        assert_eq!((c.params().interval, c.params().timeout), (160, 300));
        let heard = exchange(&c, 150.0);
        let new_anchor = heard[0].1.start_pair;
        c.account(Listened::Yes, heard);
        let next = c.next_due();
        assert!((next.anchor_pair - new_anchor - 160.0 * 1.25e-3 * RATE).abs() < 1.0);
        assert_eq!(next.window_pairs, 0.0);
    }

    /// An update not seen: the events in view go silent, and once the
    /// silence passes the supervision timeout (100 x 10 ms here, with 100 ms
    /// events) the connection is lost after the last event heard.
    #[test]
    fn an_unseen_update_ends_in_lost() {
        let mut c = Connection::new(&params(80, 0, 1), false, 0.0, RATE).unwrap();
        for _ in 0..10 {
            let heard = exchange(&c, 150.0);
            c.account(Listened::Yes, heard);
        }
        assert_eq!(c.state(), &State::Following);
        for _ in 0..10 {
            c.account(Listened::Yes, vec![]);
        }
        assert_eq!(
            c.state(),
            &State::Following,
            "exactly the timeout is not past it"
        );
        c.account(Listened::Yes, vec![]);
        assert_eq!(c.state(), &State::Lost { after: Some(9) });
    }

    /// Out of view for longer than the timeout is not lost: nothing was
    /// there to hear.
    #[test]
    fn silence_out_of_view_is_not_loss() {
        let mut c = Connection::new(&params(80, 0, 1), false, 0.0, RATE).unwrap();
        let heard = exchange(&c, 150.0);
        c.account(Listened::Yes, heard);
        for _ in 0..30 {
            c.account(Listened::NotInView, vec![]);
        }
        assert_eq!(c.state(), &State::Following);
    }

    /// LL_TERMINATE_IND ends it with its reason, and nothing after counts.
    #[test]
    fn terminate_ends_it() {
        let mut c = Connection::new(&params(80, 0, 1), false, 0.0, RATE).unwrap();
        let a = c.next_due().anchor_pair;
        c.account(Listened::Yes, vec![(pdu(3, &[0x02, 0x13]), at(a, 2))]);
        assert_eq!(c.state(), &State::Terminated { reason: 0x13 });
        c.account(Listened::Yes, vec![]);
        assert_eq!(c.events().len(), 1);
    }

    /// After LL_START_ENC_REQ (sent in the clear, Vol 6 Part C 1) the
    /// link's PDUs are encrypted: their control PDUs are not read.
    #[test]
    fn encryption_stops_the_reading() {
        let mut c = Connection::new(&params(80, 0, 1), false, 0.0, RATE).unwrap();
        let a = c.next_due().anchor_pair;
        c.account(Listened::Yes, vec![(pdu(3, &[0x05]), at(a, 1))]);
        assert_eq!(c.encrypted_from(), Some(0));
        let a = c.next_due().anchor_pair;
        c.account(
            Listened::Yes,
            vec![(pdu(3, &[0x0c, 0x0c, 0x4c, 0x00, 0x34, 0x12]), at(a, 6))],
        );
        assert_eq!(c.events()[0].pdus[0].control, None);
    }

    /// Out of view, a lost feed and a miss are three accounts.
    #[test]
    fn accounts_are_kept_apart() {
        let mut c = Connection::new(&params(80, 0, 1), false, 0.0, RATE).unwrap();
        c.account(Listened::NotInView, vec![]);
        c.account(Listened::FeedLost, vec![]);
        c.account(Listened::Yes, vec![]);
        let accounts: Vec<Account> = c.events().iter().map(|e| e.account).collect();
        assert_eq!(
            accounts,
            [Account::Missed, Account::FeedLost, Account::NotInView]
        );
    }

    /// The newest events are kept, the oldest let go.
    #[test]
    fn the_newest_events_are_kept() {
        let mut c = Connection::new(&params(80, 0, 1), false, 0.0, RATE).unwrap();
        for _ in 0..EVENTS_KEPT + 20 {
            c.account(Listened::NotInView, vec![]);
        }
        assert_eq!(c.events().len(), EVENTS_KEPT);
        assert_eq!(c.events()[0].counter, (EVENTS_KEPT + 19) as u16);
    }

    /// The Central's clock from its anchors: a Central 12 ppm fast puts
    /// its anchors 12 ppm closer together on this radio's clock, and the
    /// fit through 200 of them says +12 to well within its uncertainty.
    #[test]
    fn the_centrals_clock_is_read_from_its_anchors() {
        let mut c = Connection::new(&params(80, 0, 1), false, 0.0, RATE).unwrap();
        assert_eq!(c.clock_ppm(), None, "nothing heard yet");
        let first = c.next_due().anchor_pair;
        let interval = 80.0 * 1.25e-3 * RATE * (1.0 - 12e-6);
        for k in 0..200 {
            // A little scatter, as a real anchor's timing has.
            let jitter = ((k * 37 % 11) as f64 - 5.0) * 0.5;
            let at_k = first + k as f64 * interval + jitter;
            c.account(Listened::Yes, vec![(pdu(1, &[]), at(at_k, 0))]);
        }
        let clock = c.clock_ppm().unwrap();
        assert!((clock.value() - 12.0).abs() < 0.05, "{clock:?}");
        assert!(clock.sigma() > 0.0 && clock.sigma() < 0.05, "{clock:?}");
        assert!(
            (clock.value() - 12.0).abs() < 4.0 * clock.sigma() + 0.001,
            "{clock:?}"
        );
    }

    /// T_IFS pooled over the answers heard, and how many fall outside
    /// 150 +-2 us (4.1.1, 4.2.1).
    #[test]
    fn t_ifs_is_pooled_and_judged() {
        let mut c = Connection::new(&params(80, 0, 1), false, 0.0, RATE).unwrap();
        assert!(c.t_ifs().is_none());
        for t_ifs in [150.4, 149.8, 150.1, 153.0] {
            let heard = exchange(&c, t_ifs);
            c.account(Listened::Yes, heard);
        }
        let t = c.t_ifs().unwrap();
        assert_eq!((t.count, t.outside), (4, 1));
        assert!((t.mean.value() - 150.825).abs() < 0.01, "{t:?}");
    }

    /// Packets and CRC passes, counted per data channel over the whole
    /// connection.
    #[test]
    fn crc_is_counted_per_channel() {
        let mut c = Connection::new(&params(80, 0, 1), false, 0.0, RATE).unwrap();
        let ch = c.next_due().channel;
        let mut heard = exchange(&c, 150.0);
        heard[1].0.crc_ok = false;
        c.account(Listened::Yes, heard);
        assert_eq!(c.per_channel()[ch as usize], (2, 1));
        assert_eq!(c.per_channel().iter().map(|p| p.0).sum::<u32>(), 2);
    }

    /// CSA #2 only when both PDUs set ChSel (Vol 6 Part B 4.5); the
    /// initiator may set it alone, as a TV box answering its remote did on
    /// the air, and then it is CSA #1. The advertising PDU not heard, with
    /// the CONNECT_IND's set, leaves it unknown.
    #[test]
    fn the_algorithm_needs_both_chsel_bits() {
        assert_eq!(uses_csa2(false, None), Some(false));
        assert_eq!(uses_csa2(false, Some(true)), Some(false));
        assert_eq!(uses_csa2(true, Some(false)), Some(false));
        assert_eq!(uses_csa2(true, Some(true)), Some(true));
        assert_eq!(uses_csa2(true, None), None);
    }

    /// A map with no used channel is no connection to follow.
    #[test]
    fn an_empty_channel_map_is_refused() {
        let mut p = params(6, 0, 1);
        p.channel_map = 0;
        assert!(Connection::new(&p, false, 0.0, 20e6).is_none());
    }
}
