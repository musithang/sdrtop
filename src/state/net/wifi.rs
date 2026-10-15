// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Wi-Fi's receiver, as the worker last left it: where it runs, or why not.

/// A frame's symbol clock, beside its carrier over the same DATA field: two
/// readings of what the standard requires to be one oscillator; and the
/// preamble's carrier, which lies apart from them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WifiClock {
    /// Its sample clock against ours, in ppm, positive when fast.
    pub clock_ppm: crate::signal::dsp::uncertainty::Uncertain,
    /// Its carrier over the DATA field against our tuning, in ppm of the
    /// channel centre.
    pub carrier_ppm: crate::signal::dsp::uncertainty::Uncertain,
    /// Its carrier over the preamble's long training field, likewise.
    pub preamble_ppm: crate::signal::dsp::uncertainty::Uncertain,
    /// The DATA symbols the clock was read over.
    pub symbols: usize,
    pub when: std::time::Instant,
}

/// Every clock read this session held against its carrier: the
/// inverse-variance sums of clock minus carrier, so the mean, its error
/// and how well the frames agree with one another can be said.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ClockCheck {
    pub frames: u64,
    sum_w: f64,
    sum_wd: f64,
    sum_wdd: f64,
}

impl ClockCheck {
    /// Count one frame's clock against its carrier.
    pub fn add(&mut self, clock: &WifiClock) {
        let d = clock.clock_ppm.value() - clock.carrier_ppm.value();
        let var = clock.clock_ppm.sigma().powi(2) + clock.carrier_ppm.sigma().powi(2);
        if !(var > 0.0 && var.is_finite() && d.is_finite()) {
            return;
        }
        let w = 1.0 / var;
        self.frames += 1;
        self.sum_w += w;
        self.sum_wd += w * d;
        self.sum_wdd += w * d * d;
    }

    /// The weighted mean of clock minus carrier, in ppm, with its error.
    pub fn mean(&self) -> Option<crate::signal::dsp::uncertainty::Uncertain> {
        (self.frames > 0).then(|| {
            crate::signal::dsp::uncertainty::Uncertain::from_variance(
                self.sum_wd / self.sum_w,
                1.0 / self.sum_w,
            )
        })
    }

    /// Chi-square per degree of freedom of the differences about their
    /// mean: near one when every frame agrees within its own uncertainty,
    /// well above when some transmitter's clocks are not one.
    pub fn chi2_per_dof(&self) -> Option<f64> {
        (self.frames > 1).then(|| {
            (self.sum_wdd - self.sum_wd * self.sum_wd / self.sum_w) / (self.frames - 1) as f64
        })
    }
}

/// What a frame of this channel is, by the format its signalling named:
/// the classes every Wi-Fi view colours alike. A frame of another channel
/// is none of them; it is counted, never given airtime here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    NonHt,
    Ht,
    Vht,
    He,
    Eht,
    Dsss,
    /// Signalling that did not read, or a PHY past EHT, whose airtime its
    /// L-SIG still stated.
    Undecoded,
}

impl Family {
    /// Every family, in the order a column stacks them.
    pub const ALL: [Family; 7] = [
        Family::NonHt,
        Family::Ht,
        Family::Vht,
        Family::He,
        Family::Eht,
        Family::Dsss,
        Family::Undecoded,
    ];

    /// The name the views give it.
    pub fn label(self) -> &'static str {
        match self {
            Family::NonHt => "non-HT",
            Family::Ht => "HT",
            Family::Vht => "VHT",
            Family::He => "HE",
            Family::Eht => "EHT",
            Family::Dsss => "DSSS",
            Family::Undecoded => "not decoded",
        }
    }

    /// The family a frame of this channel is, and the airtime its
    /// signalling stated, in microseconds, when it stated one.
    pub fn of(heard: &crate::signal::wifi::format::Heard) -> (Family, Option<f64>) {
        use crate::signal::wifi::format::Format;
        // The L-SIG's airtime is the frame's in every format but greenfield,
        // which has none and states its own in its HT-SIG.
        let lsig = heard
            .lsig
            .and_then(|l| l.ok())
            .map(|l| f64::from(l.airtime_us()));
        match heard.format {
            None => (Family::Undecoded, None),
            Some(Format::NonHt) => (Family::NonHt, lsig),
            Some(Format::HtMixed(_)) => (Family::Ht, lsig),
            Some(Format::Greenfield(Ok(sig))) => (
                Family::Ht,
                sig.greenfield_airtime_ns()
                    .ok()
                    .map(|ns| f64::from(ns) / 1e3),
            ),
            Some(Format::Greenfield(Err(_))) => (Family::Undecoded, None),
            Some(Format::Vht(_)) => (Family::Vht, lsig),
            Some(Format::He { .. }) => (Family::He, lsig),
            Some(Format::USig { sig: Ok(u), .. }) if u.version == 0 => (Family::Eht, lsig),
            Some(Format::USig { .. }) => (Family::Undecoded, lsig),
        }
    }
}

/// One visit the survey paid a channel: how long it listened, how busy its
/// busiest megahertz was, the share of the visit the frames read on its
/// centre stated as airtime (by family, in [`Family::ALL`]'s order), and how
/// many frames were read there and heard there from other channels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Visit {
    pub seconds: f64,
    pub busy: f64,
    pub airtime: [f64; 7],
    pub read: u32,
    pub other: u32,
}

/// The fewest visits whose spread is a spread: with one or two, the
/// standard error of their mean says nothing anyone should lean on.
pub const MIN_VISITS: usize = 3;

/// The channels the survey visits: 1 to 13.
pub const SURVEYED: usize = 13;

/// A channel's busy share over its visits: their mean, and the standard
/// error of that mean once there are [`MIN_VISITS`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Busy {
    pub mean: f64,
    pub sigma: Option<f64>,
    pub visits: usize,
    pub seconds: f64,
}

/// Which channel is least busy, or why none can be named yet.
#[derive(Clone, Debug, PartialEq)]
pub enum LeastBusy {
    /// Not every channel has been visited.
    FirstPass { visited: usize },
    /// Every channel visited, but some fewer than [`MIN_VISITS`] times, so
    /// no spread to compare against; `passes` more give every one its three.
    NoSpreadYet { passes: usize },
    /// One channel, apart from every other by more than 2 sigma.
    Alone(u8),
    /// The quietest and those within 2 sigma of it, in channel order.
    Few(Vec<u8>),
    /// Every channel within 2 sigma of the quietest. `passes`: how many more
    /// would set the first one apart, at the spread a visit has now; `None`
    /// when all read the same and no number of passes would.
    NotApart { passes: Option<usize> },
}

/// The survey's visits, channel by channel, since the view opened or was
/// started again.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WifiSurvey {
    /// Channel n's visits at index n - 1.
    pub visits: [Vec<Visit>; SURVEYED],
    /// The channel the view has selected, when focused.
    pub selected: Option<u8>,
}

impl WifiSurvey {
    fn of(&self, ch: u8) -> &[Visit] {
        usize::from(ch)
            .checked_sub(1)
            .and_then(|i| self.visits.get(i))
            .map_or(&[], |v| v.as_slice())
    }

    /// Channel `ch`'s busy share, `None` before its first visit.
    pub fn busy(&self, ch: u8) -> Option<Busy> {
        let v = self.of(ch);
        if v.is_empty() {
            return None;
        }
        let n = v.len() as f64;
        let mean = v.iter().map(|x| x.busy).sum::<f64>() / n;
        let sigma = (v.len() >= MIN_VISITS).then(|| {
            let var = v.iter().map(|x| (x.busy - mean).powi(2)).sum::<f64>() / (n - 1.0);
            (var / n).sqrt()
        });
        Some(Busy {
            mean,
            sigma,
            visits: v.len(),
            seconds: v.iter().map(|x| x.seconds).sum(),
        })
    }

    /// The share of channel `ch`'s time its frames stated, by family, over
    /// its visits.
    pub fn airtime(&self, ch: u8) -> [f64; 7] {
        let v = self.of(ch);
        let mut out = [0.0; 7];
        for x in v {
            for (o, a) in out.iter_mut().zip(x.airtime) {
                *o += a / v.len() as f64;
            }
        }
        out
    }

    /// The least busy channel by the plain 2 sigma rule: the quietest, and
    /// every channel whose mean is within twice their combined standard
    /// error of it. No other threshold.
    pub fn least_busy(&self) -> LeastBusy {
        let channels = 1..=SURVEYED as u8;
        let busy: Vec<Option<Busy>> = channels.clone().map(|c| self.busy(c)).collect();
        let visited = busy.iter().flatten().count();
        if visited < SURVEYED {
            return LeastBusy::FirstPass { visited };
        }
        let busy: Vec<Busy> = busy.into_iter().flatten().collect();
        let fewest = busy.iter().map(|b| b.visits).min().unwrap_or(0);
        if fewest < MIN_VISITS {
            return LeastBusy::NoSpreadYet {
                passes: MIN_VISITS - fewest,
            };
        }
        let sigma = |b: &Busy| b.sigma.unwrap_or(0.0);
        let q = busy
            .iter()
            .min_by(|a, b| a.mean.total_cmp(&b.mean))
            .copied()
            .unwrap_or(busy[0]);
        let tied: Vec<u8> = channels
            .zip(&busy)
            .filter(|(_, b)| b.mean - q.mean <= 2.0 * sigma(b).hypot(sigma(&q)))
            .map(|(c, _)| c)
            .collect();
        match tied.len() {
            1 => LeastBusy::Alone(tied[0]),
            n if n < SURVEYED => LeastBusy::Few(tied),
            _ => {
                // The spread a visit has stays; the standard error shrinks
                // as one over the root of the visits. Apart once
                // 2 sqrt((s_c^2 + s_q^2) / n) falls below the difference.
                let per_visit = |b: &Busy| sigma(b) * (b.visits as f64).sqrt();
                let passes = busy
                    .iter()
                    .filter(|b| b.mean > q.mean)
                    .map(|b| {
                        let d = b.mean - q.mean;
                        let need = (4.0 * (per_visit(b).powi(2) + per_visit(&q).powi(2)) / (d * d))
                            .floor() as usize
                            + 1;
                        need.saturating_sub(b.visits.min(q.visits)).max(1)
                    })
                    .min();
                LeastBusy::NotApart { passes }
            }
        }
    }
}

/// Where Wi-Fi's receiver runs, or why it does not, published by the
/// worker's Wi-Fi part (`signal::net::worker`'s `wifi`).
/// How many frames the Frames list keeps: about seven seconds of a busy
/// channel (300 frames a second), a minute and a half of a quiet one.
pub const WIFI_ROWS_KEPT: usize = 2_048;

/// What a frame in the Frames list is, as far as it was read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RowKind {
    /// A frame of this channel, OFDM: what its signalling said.
    Ofdm(crate::signal::wifi::format::Heard),
    /// A frame of this channel, 802.11b: its preamble, and its header or
    /// why it was refused.
    Dsss {
        preamble: crate::signal::wifi::dsss::header::Preamble,
        header: Result<
            crate::signal::wifi::dsss::header::Header,
            crate::signal::wifi::dsss::header::Fault,
        >,
    },
    /// Another channel's OFDM frame, `shift_mhz` from ours: its start, and
    /// nothing more, since nothing it said was read.
    OtherOfdm { shift_mhz: i32 },
    /// Another channel's 802.11b frame, its header read at its own centre.
    OtherDsss {
        shift_mhz: i32,
        preamble: crate::signal::wifi::dsss::header::Preamble,
        header: Result<
            crate::signal::wifi::dsss::header::Header,
            crate::signal::wifi::dsss::header::Fault,
        >,
    },
}

/// One frame in the Frames list.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WifiRow {
    /// Its place in the session's arrivals, from 1: what a selection holds
    /// on to, since the ring's positions shift with every frame.
    pub seq: u64,
    /// The stream pair its first sample arrived at.
    pub start: u64,
    /// The run of the stream it was heard in: within one run, starts are
    /// one clock; across a break or a retune they are not.
    pub run: u64,
    pub when: std::time::Instant,
    /// The channel centre it was heard on.
    pub carrier_hz: f64,
    pub kind: RowKind,
    /// The airtime it stated, in microseconds: `None` where nothing it
    /// said was read.
    pub airtime_us: Option<u32>,
    /// From the end of the frame before it to its start, in microseconds:
    /// `None` across a break, and after a frame that stated no length.
    pub gap_us: Option<f64>,
    /// Its carrier against the centre of the channel it was sent on
    /// ([`WifiRow::centre_hz`]): ours, or another channel's for its frame.
    pub offset_hz: Option<crate::signal::dsp::uncertainty::Uncertain>,
    /// Its SNR a sample, in dB.
    pub snr_db: Option<f64>,
    /// Its channel's magnitude on each of the 52 tones, as hundredths of a
    /// decibel against the tones' mean power; a frame of this channel's
    /// OFDM only.
    pub channel: Option<[i16; 52]>,
    /// Its symbol clock and its carrier over the DATA field, when the frame
    /// was long enough to read them (`WifiState::clock_row`).
    pub clock: Option<WifiClock>,
    /// The stream's clock it was heard on: one per stream and tuning. A
    /// feed gap keeps it, since the stream's position counts the pairs a
    /// dropped block held; a new stream or a retune starts another.
    pub epoch: u64,
}

impl WifiRow {
    /// The centre of the channel it was sent on: ours, or for another
    /// channel's frame, that channel's, which its offset is against.
    pub fn centre_hz(&self) -> f64 {
        match self.kind {
            RowKind::OtherOfdm { shift_mhz } | RowKind::OtherDsss { shift_mhz, .. } => {
                self.carrier_hz + f64::from(shift_mhz) * 1e6
            }
            RowKind::Ofdm(_) | RowKind::Dsss { .. } => self.carrier_hz,
        }
    }

    /// Its family, for a frame of this channel; another channel's frame is
    /// none of them.
    pub fn family(&self) -> Option<Family> {
        match self.kind {
            RowKind::Ofdm(heard) => Some(Family::of(&heard).0),
            RowKind::Dsss { .. } => Some(Family::Dsss),
            RowKind::OtherOfdm { .. } | RowKind::OtherDsss { .. } => None,
        }
    }
}

/// How much time one cell of the Frames river holds. Set by a key, never by
/// the traffic: a scale that followed the bursts would change every frame's
/// width from one second to the next.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FrameZoom {
    /// 1 ms a cell: the channel's load, which reads at any traffic.
    #[default]
    Load,
    /// 40 us a cell: the exchanges, a frame and its acknowledgement.
    Rhythm,
    /// 4 us a cell: one frame's anatomy, its preamble and the SIFS after.
    Anatomy,
}

impl FrameZoom {
    /// Stream pairs a cell, at the 20 Msps every Wi-Fi view runs at.
    pub fn cell_pairs(self) -> u64 {
        match self {
            FrameZoom::Load => 20_000,
            FrameZoom::Rhythm => 800,
            FrameZoom::Anatomy => 80,
        }
    }

    /// Microseconds a cell.
    pub fn cell_us(self) -> f64 {
        match self {
            FrameZoom::Load => 1_000.0,
            FrameZoom::Rhythm => 40.0,
            FrameZoom::Anatomy => 4.0,
        }
    }

    /// What the river's title says of it.
    pub fn label(self) -> &'static str {
        match self {
            FrameZoom::Load => "1 ms a cell",
            FrameZoom::Rhythm => "40 \u{b5}s a cell",
            FrameZoom::Anatomy => "4 \u{b5}s a cell",
        }
    }

    /// The next step in, wrapping from the finest back to the load.
    pub fn next(self) -> Self {
        match self {
            FrameZoom::Load => FrameZoom::Rhythm,
            FrameZoom::Rhythm => FrameZoom::Anatomy,
            FrameZoom::Anatomy => FrameZoom::Load,
        }
    }
}

/// The Frames view: live or held, the frame selected, the river's zoom and
/// what the card shows.
///
/// **Held at a frame, not a copy of the ring**, as the Classic list is: the
/// list is newest first, so holding it means drawing from one frame down
/// while newer ones arrive above.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FramesView {
    /// The newest frame shown while held, by its seq; `None` while live.
    pub held: Option<u64>,
    /// The selected frame, in rows down from the newest shown. Live, it is
    /// always the newest: a cursor on a list that moves would slide.
    pub cursor: usize,
    pub zoom: FrameZoom,
    /// The card shows the decode account in place of the selected frame.
    pub decode: bool,
    /// Where the card has no room, the selected frame (or the decode
    /// account) in place of the list.
    pub open: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct WifiState {
    /// The Wi-Fi channel the receiver runs on now, when it runs.
    pub channel: Option<u8>,
    /// Why it does not run, when a Wi-Fi view is open and it does not: the
    /// rate, or a tuning off every channel centre. `None` off the Wi-Fi
    /// views, where not running is not a refusal.
    pub refused: Option<String>,
    /// Whether it has run at all this session. Decode health names Wi-Fi
    /// only then, as it does LE Coded: a line for a receiver this session
    /// never started would offer one the view on screen does not start.
    pub ran: bool,
    /// Every clock read this session against its carrier.
    pub clock_check: ClockCheck,
    /// The survey's visits, channel by channel.
    pub survey: WifiSurvey,
    /// The frames heard, oldest first, in the order they were sent: at most
    /// [`WIFI_ROWS_KEPT`].
    pub rows: std::collections::VecDeque<WifiRow>,
    /// Frames heard this session, the last row's seq.
    pub rows_heard: u64,
    /// The Frames view's hold, selection, zoom and card.
    pub frames_view: FramesView,
    /// The spans of the stream a feed gap took, oldest first: at most
    /// [`LOST_KEPT`].
    pub lost: std::collections::VecDeque<Lost>,
    /// The Frames view's triggered capture.
    pub scope: Scope,
}

/// How many lost spans are kept: far more than a capture can hold.
pub const LOST_KEPT: usize = 256;

/// A span of the stream a feed gap took: never heard, though the stream's
/// clock ran through it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lost {
    pub epoch: u64,
    /// The pairs from `from` to `to` (not included).
    pub from: u64,
    pub to: u64,
}

/// What starts a capture.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Trigger {
    /// Whatever frame comes first.
    #[default]
    Any,
    /// A frame of this channel of this family.
    Family(Family),
    /// Another channel's frame.
    Other,
}

impl Trigger {
    /// The order `t` steps through them.
    const ORDER: [Trigger; 9] = [
        Trigger::Any,
        Trigger::Family(Family::NonHt),
        Trigger::Family(Family::Ht),
        Trigger::Family(Family::Vht),
        Trigger::Family(Family::He),
        Trigger::Family(Family::Eht),
        Trigger::Family(Family::Dsss),
        Trigger::Other,
        Trigger::Family(Family::Undecoded),
    ];

    /// The next one along, wrapping.
    pub fn next(self) -> Self {
        let at = Self::ORDER.iter().position(|t| *t == self).unwrap_or(0);
        Self::ORDER[(at + 1) % Self::ORDER.len()]
    }

    /// Whether `row` sets it off.
    pub fn accepts(self, row: &WifiRow) -> bool {
        match self {
            Trigger::Any => true,
            Trigger::Family(f) => row.family() == Some(f),
            Trigger::Other => row.family().is_none(),
        }
    }

    /// What the titles call it.
    pub fn label(self) -> &'static str {
        match self {
            Trigger::Any => "any frame",
            Trigger::Family(Family::Dsss) => "802.11b",
            Trigger::Family(Family::Undecoded) => "unreadable",
            Trigger::Family(f) => f.label(),
            Trigger::Other => "another channel",
        }
    }
}

/// How long after a capture the next trigger is not taken.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Holdoff {
    Half,
    #[default]
    One,
    Two,
    Five,
}

impl Holdoff {
    pub fn millis(self) -> u64 {
        match self {
            Holdoff::Half => 500,
            Holdoff::One => 1_000,
            Holdoff::Two => 2_000,
            Holdoff::Five => 5_000,
        }
    }

    /// One step longer (`+`), stopping at the longest.
    pub fn longer(self) -> Self {
        match self {
            Holdoff::Half => Holdoff::One,
            Holdoff::One => Holdoff::Two,
            Holdoff::Two | Holdoff::Five => Holdoff::Five,
        }
    }

    /// One step shorter (`-`), stopping at the shortest.
    pub fn shorter(self) -> Self {
        match self {
            Holdoff::Five => Holdoff::Two,
            Holdoff::Two => Holdoff::One,
            Holdoff::One | Holdoff::Half => Holdoff::Half,
        }
    }
}

/// One capture: a window of one clock's air, and a copy of what was heard
/// in it, so it outlives the ring.
#[derive(Clone, Debug, PartialEq)]
pub struct Capture {
    /// Captures taken this session, this one included.
    pub number: u64,
    pub zoom: FrameZoom,
    pub trigger: Trigger,
    pub epoch: u64,
    /// The window, in stream pairs: `from` up to `to`.
    pub from: u64,
    pub to: u64,
    /// The frame that set it off.
    pub trigger_seq: u64,
    /// The frames heard in the window, in the order they were sent.
    pub rows: Vec<WifiRow>,
    /// The spans of the window a feed gap took.
    pub lost: Vec<Lost>,
}

/// A capture waiting for the rest of its window to be heard.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Pending {
    epoch: u64,
    from: u64,
    to: u64,
    trigger_seq: u64,
    zoom: FrameZoom,
    trigger: Trigger,
}

/// The Frames view as a triggered capture: armed after a holdoff, set off
/// by the first frame the trigger accepts, shown once its window has been
/// heard, and held still until the next.
#[derive(Clone, Debug, PartialEq)]
pub struct Scope {
    pub trigger: Trigger,
    pub holdoff: Holdoff,
    /// Single: the capture shown stays, and no other is taken.
    pub stopped: bool,
    /// The window in cells of the river's scale: its lanes times their
    /// cells, written by the app from where the river is drawn; zero takes
    /// no capture, since none would be seen.
    pub window_cells: u64,
    /// The capture shown.
    pub capture: Option<Capture>,
    /// Captures taken this session.
    pub taken: u64,
    /// Armed: frames heard after this seq may set it off.
    armed_after: Option<u64>,
    /// When it arms again.
    rearm_at: Option<std::time::Instant>,
    pending: Option<Pending>,
    /// The holdoff's jitter, a xorshift state.
    seed: u64,
}

impl Default for Scope {
    fn default() -> Self {
        Scope {
            trigger: Trigger::Any,
            holdoff: Holdoff::One,
            stopped: false,
            window_cells: 0,
            capture: None,
            taken: 0,
            armed_after: None,
            rearm_at: None,
            pending: None,
            seed: 0x2545_f491_4f6c_dd1d,
        }
    }
}

/// How far past a window's end the stream must be heard before its capture
/// is whole: an 802.11b frame that began inside it is judged once its
/// header is held, 192 us and more after its start (300 us of pairs).
const SETTLE_PAIRS: u64 = 6_000;

impl Scope {
    /// Arm again now: a new trigger, a new holdoff, or `End`.
    pub fn rearm(&mut self) {
        self.pending = None;
        self.armed_after = None;
        self.rearm_at = None;
    }

    /// One step: take, finish or arm a capture from what has been heard,
    /// up to stream pair `heard_to` on clock `epoch`, at `now`.
    pub fn tick(
        &mut self,
        rows: &std::collections::VecDeque<WifiRow>,
        lost: &std::collections::VecDeque<Lost>,
        rows_heard: u64,
        zoom: FrameZoom,
        heard: (u64, u64),
        now: std::time::Instant,
    ) {
        let (epoch, heard_to) = heard;
        if self.stopped || self.window_cells == 0 {
            return;
        }
        if let Some(p) = self.pending {
            // Another clock or another scale: the window can no longer be
            // heard whole, or is another length. Arm again at once.
            if p.epoch != epoch || p.zoom != zoom {
                self.pending = None;
                self.armed_after = Some(rows_heard);
            } else if heard_to >= p.to + SETTLE_PAIRS {
                self.show(p, rows, lost, now);
                return;
            } else {
                return;
            }
        }
        let Some(after) = self.armed_after else {
            if self.rearm_at.is_some_and(|at| now < at) {
                return;
            }
            // Armed: what is heard from here on can set it off.
            self.rearm_at = None;
            self.armed_after = Some(rows_heard);
            return;
        };
        let Some(first) = rows
            .iter()
            .filter(|r| r.seq > after && r.epoch == epoch && self.trigger.accepts(r))
            .min_by_key(|r| r.start)
        else {
            return;
        };
        let window = self.window_cells * zoom.cell_pairs();
        let from = first.start.saturating_sub(window / 10);
        self.pending = Some(Pending {
            epoch,
            from,
            to: from + window,
            trigger_seq: first.seq,
            zoom,
            trigger: self.trigger,
        });
        // Perhaps already heard whole.
        self.tick(rows, lost, rows_heard, zoom, heard, now);
    }

    /// Show the capture `p` was waiting for, and hold off the next.
    fn show(
        &mut self,
        p: Pending,
        rows: &std::collections::VecDeque<WifiRow>,
        lost: &std::collections::VecDeque<Lost>,
        now: std::time::Instant,
    ) {
        let end = |r: &WifiRow| r.start + 20 * u64::from(r.airtime_us.unwrap_or(0));
        self.taken += 1;
        self.capture = Some(Capture {
            number: self.taken,
            zoom: p.zoom,
            trigger: p.trigger,
            epoch: p.epoch,
            from: p.from,
            to: p.to,
            trigger_seq: p.trigger_seq,
            rows: rows
                .iter()
                .filter(|r| {
                    r.epoch == p.epoch && r.start < p.to && (r.start >= p.from || end(r) > p.from)
                })
                .copied()
                .collect(),
            lost: lost
                .iter()
                .filter(|l| l.epoch == p.epoch && l.from < p.to && l.to > p.from)
                .copied()
                .collect(),
        });
        self.pending = None;
        self.armed_after = None;
        // Up to 10 % either way, in whole milliseconds: a holdoff of exactly
        // ten beacon intervals (102.4 ms) would land every capture on a
        // beacon, as a scope's trigger locks onto a periodic signal.
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 7;
        self.seed ^= self.seed << 17;
        let ms = self.holdoff.millis() * (900 + self.seed % 201) / 1_000;
        self.rearm_at = Some(now + std::time::Duration::from_millis(ms));
    }
}

impl WifiState {
    /// Keep `row`, in its place by start within its run (a frame can be
    /// judged after one that followed it), with the gap before it and the
    /// gap after it worked out again.
    pub fn push_row(&mut self, mut row: WifiRow) {
        self.rows_heard += 1;
        row.seq = self.rows_heard;
        let at = self
            .rows
            .iter()
            .rposition(|r| (r.run, r.start) <= (row.run, row.start))
            .map_or(0, |i| i + 1);
        self.rows.insert(at, row);
        for i in [at, at + 1] {
            if i < self.rows.len() {
                let gap = i
                    .checked_sub(1)
                    .and_then(|p| gap_us(&self.rows[p], &self.rows[i]));
                self.rows[i].gap_us = gap;
            }
        }
        while self.rows.len() > WIFI_ROWS_KEPT {
            self.rows.pop_front();
        }
    }
}

impl WifiState {
    /// Keep a span a feed gap took.
    pub fn lose(&mut self, lost: Lost) {
        self.lost.push_back(lost);
        while self.lost.len() > LOST_KEPT {
            self.lost.pop_front();
        }
    }

    /// Step the capture, the stream heard up to pair `heard_to` on clock
    /// `epoch`.
    pub fn tick_scope(&mut self, epoch: u64, heard_to: u64, now: std::time::Instant) {
        self.scope.tick(
            &self.rows,
            &self.lost,
            self.rows_heard,
            self.frames_view.zoom,
            (epoch, heard_to),
            now,
        );
    }

    /// Give the row of the frame that began at stream pair `start` in run
    /// `run` its clock: read once the frame's DATA field was held, after the
    /// row was kept. A row gone from the ring has nothing to carry it.
    pub fn clock_row(&mut self, run: u64, start: u64, clock: WifiClock) {
        if let Some(r) = self
            .rows
            .iter_mut()
            .rev()
            .find(|r| r.run == run && r.start == start)
        {
            r.clock = Some(clock);
        }
    }
}

/// From the end of `before` to the start of `row`, in microseconds, when
/// both are of one run and `before` stated its length. A Wi-Fi view runs at
/// exactly 20 Msps (`signal::net::wifi::rate_refusal`): 20 pairs a
/// microsecond.
fn gap_us(before: &WifiRow, row: &WifiRow) -> Option<f64> {
    let air = before.airtime_us?;
    (before.run == row.run)
        .then(|| (row.start as f64 - before.start as f64) / 20.0 - f64::from(air))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::dsp::uncertainty::Uncertain;

    /// A row of this channel's OFDM, `start_us` into run `run`, stating
    /// `air_us` of airtime.
    fn row(run: u64, start_us: f64, air_us: Option<u32>) -> WifiRow {
        WifiRow {
            seq: 0,
            start: (start_us * 20.0) as u64,
            run,
            when: std::time::Instant::now(),
            carrier_hz: 2_462e6,
            kind: RowKind::OtherOfdm { shift_mhz: 5 },
            airtime_us: air_us,
            gap_us: None,
            offset_hz: None,
            snr_db: None,
            channel: None,
            clock: None,
            epoch: 0,
        }
    }

    /// Each frame is the family its signalling named, with the airtime its
    /// L-SIG stated; a frame whose L-SIG was refused is undecoded and states
    /// nothing.
    #[test]
    fn a_frame_is_the_family_its_signalling_named() {
        use crate::signal::wifi::format::{Format, Heard};
        use crate::signal::wifi::lsig::LSig;
        use crate::signal::wifi::signal_field::Rate;
        let lsig = LSig {
            rate: Rate::R6,
            length: 100,
        };
        let heard = |format| Heard {
            lsig: Some(Ok(lsig)),
            format,
        };
        let air = lsig.airtime_us() as f64;
        assert_eq!(
            Family::of(&heard(Some(Format::NonHt))),
            (Family::NonHt, Some(air))
        );
        assert_eq!(
            Family::of(&heard(Some(Format::Vht(Ok(()))))),
            (Family::Vht, Some(air))
        );
        assert_eq!(
            Family::of(&Heard {
                lsig: Some(Err(crate::signal::wifi::lsig::Refused::Parity)),
                format: None
            }),
            (Family::Undecoded, None)
        );
    }

    /// **Rows are kept in the order they were sent, with the gap before
    /// each.** A data frame and its acknowledgement a SIFS apart read 16 us
    /// apart; a frame judged late (an 802.11b header waits longer than an
    /// OFDM one) goes in its place, and the gaps either side of it follow.
    #[test]
    fn rows_are_kept_in_order_with_their_gaps() {
        let mut w = WifiState::default();
        w.push_row(row(0, 0.0, Some(48)));
        w.push_row(row(0, 64.0, Some(44)));
        assert_eq!(w.rows[1].gap_us, Some(16.0));
        w.push_row(row(0, 2_000.0, Some(28)));
        w.push_row(row(0, 500.0, Some(100)));
        let starts: Vec<u64> = w.rows.iter().map(|r| r.start / 20).collect();
        assert_eq!(starts, [0, 64, 500, 2_000]);
        assert_eq!(w.rows[2].gap_us, Some(500.0 - 108.0));
        assert_eq!(w.rows[3].gap_us, Some(2_000.0 - 600.0));
        let seqs: Vec<u64> = w.rows.iter().map(|r| r.seq).collect();
        assert_eq!(seqs, [1, 2, 4, 3], "each keeps the seq it arrived with");
    }

    /// Across a break or a retune the stream's clock starts again: the gap
    /// is unknown, never a number. So is the gap after a frame whose length
    /// nothing stated.
    #[test]
    fn a_gap_is_known_only_within_a_run_after_a_stated_length() {
        let mut w = WifiState::default();
        w.push_row(row(0, 0.0, Some(48)));
        w.push_row(row(1, 30.0, Some(48)));
        assert_eq!(w.rows[1].gap_us, None, "across a break");
        w.push_row(row(1, 200.0, None));
        w.push_row(row(1, 400.0, Some(48)));
        assert_eq!(
            w.rows[3].gap_us, None,
            "after a frame with no stated length"
        );
    }

    /// The ring keeps its newest `WIFI_ROWS_KEPT`, oldest first.
    #[test]
    fn the_ring_keeps_its_newest() {
        let mut w = WifiState::default();
        for i in 0..(WIFI_ROWS_KEPT + 52) {
            w.push_row(row(0, i as f64 * 100.0, Some(48)));
        }
        assert_eq!(w.rows.len(), WIFI_ROWS_KEPT);
        assert_eq!(w.rows.front().unwrap().seq, 53);
        assert_eq!(w.rows_heard, (WIFI_ROWS_KEPT + 52) as u64);
    }

    /// The busiest megahertz of each channel at each of its six visits, as
    /// the survey recorded on 2026-10-14 heard them (channel 1 first).
    const RECORDED: [[f64; 6]; 13] = [
        [0.0005, 0.0, 0.0, 0.0, 0.0, 0.0],
        [0.0005, 0.0, 0.0010, 0.0, 0.0, 0.0],
        [0.0, 0.0004, 0.0005, 0.0, 0.0, 0.0],
        [0.0013, 0.0115, 0.0008, 0.0, 0.0, 0.0],
        [0.0004, 0.0664, 0.0, 0.0, 0.0, 0.0],
        [0.0, 0.0648, 0.0, 0.0, 0.0, 0.0],
        [0.0, 0.0993, 0.0, 0.0, 0.0, 0.0],
        [0.0, 0.1051, 0.0, 0.0, 0.0002, 0.0],
        [0.0196, 0.6634, 0.0195, 0.0284, 0.0209, 0.0267],
        [0.0191, 0.7678, 0.0197, 0.0379, 0.0203, 0.6827],
        [0.0196, 0.5527, 0.0247, 0.0291, 0.0242, 0.5197],
        [0.0396, 0.2029, 0.0265, 0.0191, 0.0253, 0.5636],
        [0.0242, 0.0016, 0.0245, 0.0196, 0.0395, 0.7191],
    ];

    /// The first `n` visits of every channel, with `busy` from `table`.
    fn survey(table: &[[f64; 6]; 13], n: usize) -> WifiSurvey {
        let mut s = WifiSurvey::default();
        for (ch, row) in table.iter().enumerate() {
            s.visits[ch] = row[..n]
                .iter()
                .map(|b| Visit {
                    seconds: 0.475,
                    busy: *b,
                    ..Default::default()
                })
                .collect();
        }
        s
    }

    /// A channel's busy is its visits' mean; its spread, the standard error
    /// of the mean, only from the third visit on.
    #[test]
    fn busy_is_the_mean_of_the_visits_and_its_spread_comes_at_three() {
        let six = survey(&RECORDED, 6).busy(10).unwrap();
        assert!((six.mean - 0.2579).abs() < 1e-4, "{six:?}");
        assert!((six.sigma.unwrap() - 0.1482).abs() < 1e-4, "{six:?}");
        assert_eq!((six.visits, (six.seconds * 1e3).round()), (6, 2850.0));
        assert_eq!(survey(&RECORDED, 2).busy(10).unwrap().sigma, None);
        assert!(survey(&RECORDED, 3).busy(10).unwrap().sigma.is_some());
        assert_eq!(WifiSurvey::default().busy(10), None);
        // Six visits of nothing: a mean of nought and no spread about it.
        let quiet = survey(&[[0.0; 6]; 13], 6).busy(3).unwrap();
        assert_eq!((quiet.mean, quiet.sigma), (0.0, Some(0.0)));
    }

    /// **The recorded survey names no channel, and says when one would
    /// be.** Six visits of bursty traffic: even channel 10, 25.8 % plus or
    /// minus 14.8, is within 2 sigma of the quietest, and at the spread a
    /// visit has, eight visits would set it apart: two more passes.
    #[test]
    fn the_recorded_survey_tells_no_channel_apart_yet() {
        assert_eq!(
            survey(&RECORDED, 6).least_busy(),
            LeastBusy::NotApart { passes: Some(2) }
        );
    }

    /// The verdicts in turn: a channel not yet visited, a spread not yet
    /// had, one channel apart, a few together, all the same.
    #[test]
    fn the_least_busy_is_named_only_as_far_as_the_visits_carry() {
        let mut first = survey(&RECORDED, 1);
        first.visits[12].clear();
        assert_eq!(first.least_busy(), LeastBusy::FirstPass { visited: 12 });
        assert_eq!(
            survey(&RECORDED, 2).least_busy(),
            LeastBusy::NoSpreadYet { passes: 1 }
        );
        // Steady channels: 6 quiet and alone, then 1 and 11 as quiet.
        let mut steady = [[0.3, 0.32, 0.28, 0.3, 0.31, 0.29]; 13];
        steady[5] = [0.01, 0.012, 0.008, 0.01, 0.011, 0.009];
        assert_eq!(survey(&steady, 6).least_busy(), LeastBusy::Alone(6));
        steady[0] = steady[5];
        steady[10] = [0.012, 0.01, 0.009, 0.011, 0.01, 0.008];
        assert_eq!(
            survey(&steady, 6).least_busy(),
            LeastBusy::Few(vec![1, 6, 11])
        );
        assert_eq!(
            survey(&[[0.0; 6]; 13], 6).least_busy(),
            LeastBusy::NotApart { passes: None }
        );
    }

    /// The airtime by family is the visits' mean share.
    #[test]
    fn airtime_is_the_mean_share_of_the_visits() {
        let mut s = WifiSurvey::default();
        let mut a = Visit::default();
        a.airtime[5] = 0.2;
        let mut b = Visit::default();
        b.airtime[5] = 0.1;
        b.airtime[1] = 0.04;
        s.visits[10] = vec![a, b];
        let at = s.airtime(11);
        assert!((at[5] - 0.15).abs() < 1e-12 && (at[1] - 0.02).abs() < 1e-12);
        assert_eq!(s.airtime(1), [0.0; 7]);
    }

    fn reading(clock: f64, carrier: f64) -> WifiClock {
        WifiClock {
            clock_ppm: Uncertain::from_sigma(clock, 1.0),
            carrier_ppm: Uncertain::from_sigma(carrier, 1.0),
            preamble_ppm: Uncertain::from_sigma(carrier + 2.0, 0.1),
            symbols: 100,
            when: std::time::Instant::now(),
        }
    }

    /// A clock is read after its frame's row was kept, and finds the row
    /// again by its run and start; a row of another run with the same
    /// start is another frame and is left alone.
    #[test]
    fn a_clock_finds_its_frames_row() {
        let mut w = WifiState::default();
        w.push_row(row(0, 100.0, Some(300)));
        w.push_row(row(1, 100.0, Some(300)));
        w.push_row(row(1, 900.0, Some(40)));
        w.clock_row(1, 2_000, reading(3.0, 2.0));
        assert_eq!(w.rows[1].clock.map(|c| c.symbols), Some(100));
        assert!(w.rows[0].clock.is_none() && w.rows[2].clock.is_none());
        // A row gone from the ring has nothing to carry it.
        w.clock_row(7, 2_000, reading(3.0, 2.0));
        assert_eq!(w.rows.iter().filter(|r| r.clock.is_some()).count(), 1);
    }

    /// The capture's tests: channel 11 at 40 us a cell, a window of 100
    /// cells (4 ms, its first 400 us before the trigger).
    fn scoped() -> WifiState {
        let mut w = WifiState::default();
        w.scope.window_cells = 100;
        w.frames_view.zoom = FrameZoom::Rhythm;
        w
    }

    /// A frame of this channel's, non-HT, `start_us` into clock `epoch`.
    fn ofdm(epoch: u64, start_us: f64) -> WifiRow {
        use crate::signal::wifi::format::{Format, Heard};
        use crate::signal::wifi::lsig::LSig;
        use crate::signal::wifi::signal_field::Rate;
        WifiRow {
            epoch,
            kind: RowKind::Ofdm(Heard {
                lsig: Some(Ok(LSig {
                    rate: Rate::R6,
                    length: 14,
                })),
                format: Some(Format::NonHt),
            }),
            ..row(epoch, start_us, Some(44))
        }
    }

    fn dsss(start_us: f64) -> WifiRow {
        use crate::signal::wifi::dsss::header::{Header, Preamble, Rate};
        WifiRow {
            kind: RowKind::Dsss {
                preamble: Preamble::Long,
                header: Ok(Header {
                    rate: Rate::M1,
                    locked: true,
                    extension: false,
                    length_us: 300,
                }),
            },
            ..row(0, start_us, Some(492))
        }
    }

    fn pairs(us: f64) -> u64 {
        (us * 20.0) as u64
    }

    fn starts(c: &Capture) -> Vec<u64> {
        c.rows.iter().map(|r| r.start / 20).collect()
    }

    /// **A capture is shown once its window has been heard**, and not
    /// before: the first frame after arming sets it off, 10 % into the
    /// window, and the window takes what falls in it.
    #[test]
    fn a_capture_is_shown_once_its_window_is_heard() {
        let t = std::time::Instant::now();
        let mut w = scoped();
        w.tick_scope(0, 0, t);
        for us in [1_000.0, 1_100.0, 4_000.0, 5_000.0] {
            w.push_row(ofdm(0, us));
        }
        w.tick_scope(0, pairs(3_000.0), t);
        assert!(w.scope.capture.is_none(), "the window not heard yet");
        // The window ends at 4 600 us; 300 us more for a late 802.11b header.
        w.tick_scope(0, pairs(4_899.0), t);
        assert!(w.scope.capture.is_none());
        w.tick_scope(0, pairs(4_900.0), t);
        let c = w.scope.capture.as_ref().expect("a capture");
        assert_eq!((c.number, c.trigger_seq), (1, 1));
        assert_eq!((c.from, c.to), (pairs(600.0), pairs(4_600.0)));
        assert_eq!(starts(c), [1_000, 1_100, 4_000]);
        assert_eq!(w.scope.taken, 1);
    }

    /// **The holdoff paces the captures**, jittered by up to 10 %: nothing
    /// is armed 0.899 s after a capture, always by 1.101 s, and only a
    /// frame heard after arming sets the next one off.
    #[test]
    fn the_holdoff_paces_the_captures_within_its_jitter() {
        for round in 0..20u64 {
            let t = std::time::Instant::now();
            let ms = |n: u64| t + std::time::Duration::from_millis(n);
            let mut w = scoped();
            w.scope.seed ^= round.wrapping_mul(0x9e37_79b9_7f4a_7c15);
            w.tick_scope(0, 0, t);
            w.push_row(ofdm(0, 1_000.0));
            w.tick_scope(0, pairs(5_000.0), t);
            assert_eq!(w.scope.taken, 1);
            w.push_row(ofdm(0, 10_000.0));
            w.tick_scope(0, pairs(20_000.0), ms(899));
            assert_eq!(w.scope.taken, 1, "armed too soon");
            w.tick_scope(0, pairs(20_000.0), ms(1_101));
            w.push_row(ofdm(0, 30_000.0));
            w.tick_scope(0, pairs(40_000.0), ms(1_102));
            let c = w.scope.capture.as_ref().unwrap();
            assert_eq!((c.number, starts(c)[0]), (2, 30_000), "round {round}");
        }
    }

    /// The trigger takes only its own kind.
    #[test]
    fn the_trigger_takes_only_its_kind() {
        let t = std::time::Instant::now();
        let mut w = scoped();
        w.scope.trigger = Trigger::Family(Family::Dsss);
        w.tick_scope(0, 0, t);
        w.push_row(ofdm(0, 1_000.0));
        w.push_row(dsss(2_000.0));
        w.tick_scope(0, pairs(9_000.0), t);
        let c = w.scope.capture.as_ref().unwrap();
        assert_eq!((c.trigger_seq, c.from), (2, pairs(1_600.0)));
        assert_eq!(c.trigger, Trigger::Family(Family::Dsss));
        assert!(Trigger::Other.accepts(&row(0, 0.0, None)));
        assert!(!Trigger::Other.accepts(&ofdm(0, 0.0)));
    }

    /// **Single keeps its capture**, and the capture is a copy: the ring
    /// moving on under it changes nothing.
    #[test]
    fn single_keeps_its_capture_and_the_copy_outlives_the_ring() {
        let t = std::time::Instant::now();
        let mut w = scoped();
        w.tick_scope(0, 0, t);
        w.push_row(ofdm(0, 1_000.0));
        w.tick_scope(0, pairs(5_000.0), t);
        let kept = w.scope.capture.clone();
        w.scope.stopped = true;
        for i in 0..3_000 {
            w.push_row(ofdm(0, 10_000.0 + f64::from(i) * 100.0));
        }
        // Long past the holdoff: armed, set off and heard, were it running.
        let later = t + std::time::Duration::from_secs(10);
        w.tick_scope(0, pairs(1e7), later);
        w.push_row(ofdm(0, 1e7));
        w.tick_scope(0, pairs(2e7), later);
        assert_eq!(w.scope.capture, kept);
        assert_eq!(w.rows.len(), WIFI_ROWS_KEPT);
    }

    /// A new clock (a retune, a new stream) before the window was heard
    /// drops the capture waiting for it, and the next is taken on the new
    /// clock; so does another zoom, whose window is another length.
    #[test]
    fn a_new_clock_or_another_zoom_drops_a_capture_still_waiting() {
        let t = std::time::Instant::now();
        let mut w = scoped();
        w.tick_scope(0, 0, t);
        w.push_row(ofdm(0, 1_000.0));
        w.tick_scope(0, pairs(2_000.0), t);
        w.tick_scope(1, pairs(100.0), t);
        w.push_row(ofdm(1, 500.0));
        w.tick_scope(1, pairs(9_000.0), t);
        let c = w.scope.capture.as_ref().unwrap();
        assert_eq!((c.epoch, starts(c)), (1, vec![500]));

        let mut w = scoped();
        w.tick_scope(0, 0, t);
        w.push_row(ofdm(0, 1_000.0));
        w.tick_scope(0, pairs(2_000.0), t);
        w.frames_view.zoom = FrameZoom::Anatomy;
        w.tick_scope(0, pairs(9_000.0), t);
        assert!(w.scope.capture.is_none(), "taken at the old zoom");
        w.push_row(ofdm(0, 10_000.0));
        w.tick_scope(0, pairs(20_000.0), t);
        let c = w.scope.capture.as_ref().unwrap();
        assert_eq!((c.zoom, c.to - c.from), (FrameZoom::Anatomy, 8_000));
    }

    /// The spans a feed gap took inside the window go with the capture.
    #[test]
    fn a_lost_span_in_the_window_goes_with_it() {
        let t = std::time::Instant::now();
        let mut w = scoped();
        w.tick_scope(0, 0, t);
        w.push_row(ofdm(0, 1_000.0));
        let inside = Lost {
            epoch: 0,
            from: pairs(2_000.0),
            to: pairs(2_500.0),
        };
        w.lose(inside);
        w.lose(Lost {
            epoch: 0,
            from: pairs(9_000.0),
            to: pairs(9_100.0),
        });
        w.tick_scope(0, pairs(10_000.0), t);
        assert_eq!(w.scope.capture.as_ref().unwrap().lost, vec![inside]);
    }

    /// With no river on screen no capture is taken: nobody would see it.
    #[test]
    fn no_river_takes_no_capture() {
        let t = std::time::Instant::now();
        let mut w = scoped();
        w.scope.window_cells = 0;
        w.tick_scope(0, 0, t);
        w.push_row(ofdm(0, 1_000.0));
        w.tick_scope(0, pairs(9_000.0), t);
        assert!(w.scope.capture.is_none());
    }

    /// Frames that agree give a mean near their difference and a
    /// chi-square near or under one; one frame 20 sigma off lifts the
    /// chi-square far above, though the mean alone moves little.
    #[test]
    fn the_check_says_whether_the_frames_agree() {
        let mut c = ClockCheck::default();
        assert_eq!((c.mean(), c.chi2_per_dof()), (None, None));
        for d in [0.5, -0.5, 0.3, -0.3] {
            c.add(&reading(10.0 + d, 10.0));
        }
        let m = c.mean().unwrap();
        assert!(m.value().abs() < 1e-9 && (m.sigma() - (2.0f64 / 4.0).sqrt()).abs() < 1e-9);
        assert!(c.chi2_per_dof().unwrap() < 1.0);
        for _ in 0..20 {
            c.add(&reading(0.2, 0.0));
        }
        c.add(&reading(28.0, 0.0));
        assert!(c.chi2_per_dof().unwrap() > 10.0);
        assert_eq!(c.frames, 25);
    }
}
