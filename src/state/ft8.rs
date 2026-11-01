// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! What the FT8 receiver has heard: cycle by cycle, decode by decode, and
//! station by station.
//!
//! Written by `signal::ft8::worker`, read by the FT8 view. A decode is held
//! behind an `Arc`, because the whole state is cloned for every frame.
//!
//! **A station is where it said it was.** The table keeps, per call, the last
//! locator that call sent itself; a call never heard with a locator has
//! none, and nothing here fills it in (POLICY.md, rule 2). A station not heard
//! for [`STATION_SECONDS`] leaves the table.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use std::time::Instant;

use crate::signal::ft8::cycle::Decode;

pub use super::pager::PagerFeed as Ft8Feed;

/// Decodes kept, newest last.
pub const DECODES_MAX: usize = 500;

/// Cycles kept, newest last: the river's rows and the cycle strip.
pub const CYCLES_MAX: usize = 40;

/// How long a station stays after it was last heard: the longest time
/// window the map offers ([`Window::Session`]) reaches back this far.
pub const STATION_SECONDS: f64 = 86_400.0;

/// SNR readings kept per station, newest last.
pub const SNR_HISTORY: usize = 20;

/// What became of a cycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CycleOutcome {
    Decoded,
    /// The stream broke or was retuned inside it: not decoded, and not empty.
    Cut,
    /// The decoder was still busy with the cycle before: not decoded.
    Skipped,
}

/// One 15 s cycle.
#[derive(Clone, Debug, PartialEq)]
pub struct CycleEntry {
    /// Its start, Unix seconds (a multiple of 15).
    pub start_unix: f64,
    pub dial_hz: u64,
    pub outcome: CycleOutcome,
    pub decodes: usize,
    pub candidates: usize,
    pub malformed: usize,
    /// Wall time the decode took, seconds.
    pub seconds: f64,
    /// Power across the band, dB over the cycle's own median, in 25 Hz bins
    /// from 0 Hz: where there was energy, decoded or not.
    pub band: Vec<f32>,
}

impl CycleEntry {
    /// Even cycles start on the minute and the half minute.
    // Read by tests until the FT8 view draws it; `expect` fails the build
    // once something does.
    #[cfg_attr(not(test), expect(dead_code))]
    pub fn even(&self) -> bool {
        (self.start_unix / 15.0).round() as i64 % 2 == 0
    }
}

/// One decode as received.
#[derive(Clone, Debug, PartialEq)]
pub struct DecodeEntry {
    /// Its number in this session, from 1.
    pub n: u64,
    pub cycle_unix: f64,
    pub dial_hz: u64,
    pub at: Instant,
    pub decode: Decode,
    /// It names your call (`[station] callsign`) as a station addressed.
    pub for_you: bool,
}

/// A transmitting station.
#[derive(Clone, Debug, PartialEq)]
pub struct Station {
    pub call: String,
    /// The last locator it sent itself.
    pub locator: Option<String>,
    pub last_unix: f64,
    pub last_at: Instant,
    /// The dial it was last heard on.
    pub dial_hz: u64,
    pub heard: u32,
    /// (cycle start, SNR dB), newest last.
    pub snr: VecDeque<(f64, f64)>,
    /// It has called you this session.
    pub called_you: bool,
}

/// The receive path's time, judged by the stations heard: the median DT of
/// recent decodes. Positive: our samples are stamped late against the
/// crowd's transmissions (this computer's clock and the receive path
/// together).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClockVerdict {
    pub median_dt: f64,
    /// 1.4826 times the median absolute deviation: a standard deviation that
    /// one far-off station does not move.
    pub spread: f64,
    pub decodes: usize,
}

/// Fewer decodes than this and the verdict is not given.
pub const VERDICT_MIN: usize = 5;

/// The verdict from a set of DTs, or `None` with fewer than [`VERDICT_MIN`].
pub fn verdict(dts: &[f64]) -> Option<ClockVerdict> {
    if dts.len() < VERDICT_MIN {
        return None;
    }
    let median = |v: &mut Vec<f64>| {
        v.sort_by(f64::total_cmp);
        let n = v.len();
        if n % 2 == 1 {
            v[n / 2]
        } else {
            (v[n / 2 - 1] + v[n / 2]) / 2.0
        }
    };
    let m = median(&mut dts.to_vec());
    let mad = median(&mut dts.iter().map(|d| (d - m).abs()).collect());
    Some(ClockVerdict {
        median_dt: m,
        spread: 1.4826 * mad,
        decodes: dts.len(),
    })
}

/// How far the map reaches: the whole world on a logarithmic scale, or a
/// linear range to its edge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MapRange {
    #[default]
    Log,
    Km1500,
    Km3000,
    Km9000,
}

impl MapRange {
    /// The next range the `z` key steps to.
    pub fn next(self) -> Self {
        match self {
            MapRange::Log => MapRange::Km1500,
            MapRange::Km1500 => MapRange::Km3000,
            MapRange::Km3000 => MapRange::Km9000,
            MapRange::Km9000 => MapRange::Log,
        }
    }

    pub fn scale(self) -> crate::geo::projection::Scale {
        use crate::geo::projection::Scale;
        match self {
            MapRange::Log => Scale::Log,
            MapRange::Km1500 => Scale::Linear { range_km: 1500.0 },
            MapRange::Km3000 => Scale::Linear { range_km: 3000.0 },
            MapRange::Km9000 => Scale::Linear { range_km: 9000.0 },
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            MapRange::Log => "log distance",
            MapRange::Km1500 => "1 500 km",
            MapRange::Km3000 => "3 000 km",
            MapRange::Km9000 => "9 000 km",
        }
    }
}

impl MapRange {
    /// The word `[ft8] range` is written as.
    pub fn word(self) -> &'static str {
        match self {
            MapRange::Log => "log",
            MapRange::Km1500 => "1500",
            MapRange::Km3000 => "3000",
            MapRange::Km9000 => "9000",
        }
    }

    fn from_word(w: &str) -> Option<Self> {
        [
            MapRange::Log,
            MapRange::Km1500,
            MapRange::Km3000,
            MapRange::Km9000,
        ]
        .into_iter()
        .find(|r| r.word() == w)
    }
}

/// An arrow on the map: north is up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapDir {
    Up,
    Down,
    Left,
    Right,
}

impl MapDir {
    /// The unit step on the map, x east and y north.
    fn axis(self) -> (f64, f64) {
        match self {
            MapDir::Up => (0.0, 1.0),
            MapDir::Down => (0.0, -1.0),
            MapDir::Left => (-1.0, 0.0),
            MapDir::Right => (1.0, 0.0),
        }
    }
}

/// Whose calls the map writes beside their dots. The chosen station is named
/// whatever this says: a choice nobody can see is not one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Names {
    #[default]
    All,
    ThisCycle,
    None,
}

impl Names {
    pub fn next(self) -> Self {
        match self {
            Names::All => Names::ThisCycle,
            Names::ThisCycle => Names::None,
            Names::None => Names::All,
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Names::All => "all",
            Names::ThisCycle => "this cycle",
            Names::None => "none",
        }
    }

    fn from_word(w: &str) -> Option<Self> {
        [Names::All, Names::ThisCycle, Names::None]
            .into_iter()
            .find(|n| n.word() == w)
    }
}

/// How far back the map reaches: a station heard longer ago than this,
/// counted from the newest decoded cycle, is not drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Window {
    Cycle,
    Min15,
    #[default]
    Hour,
    Session,
}

impl Window {
    pub fn next(self) -> Self {
        match self {
            Window::Cycle => Window::Min15,
            Window::Min15 => Window::Hour,
            Window::Hour => Window::Session,
            Window::Session => Window::Cycle,
        }
    }

    /// The reach, seconds before the newest cycle's start.
    pub fn seconds(self) -> f64 {
        match self {
            Window::Cycle => 0.0,
            Window::Min15 => 900.0,
            Window::Hour => 3600.0,
            Window::Session => STATION_SECONDS,
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Window::Cycle => "this cycle",
            Window::Min15 => "15 min",
            Window::Hour => "1 h",
            Window::Session => "session",
        }
    }

    fn from_word(w: &str) -> Option<Self> {
        [Window::Cycle, Window::Min15, Window::Hour, Window::Session]
            .into_iter()
            .find(|x| x.word() == w)
    }
}

/// What the map draws, switched in its focus mode and kept in `[ft8]`.
#[derive(Clone, Debug, PartialEq)]
pub struct MapLayers {
    pub rays: bool,
    pub grid: bool,
    pub rings: bool,
    pub sun: bool,
    pub names: Names,
    pub window: Window,
    /// Only the stations last heard on the dial being listened to.
    pub this_band: bool,
}

impl Default for MapLayers {
    fn default() -> Self {
        Self {
            rays: true,
            grid: true,
            rings: true,
            sun: true,
            names: Names::All,
            window: Window::Hour,
            this_band: false,
        }
    }
}

/// What the FT8 view shows, set by its keys; the worker never touches it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ft8View {
    pub range: MapRange,
    pub map: MapLayers,
    /// The map zoomed on the chosen station: centred between you and it, on
    /// a linear range just wide enough for both. Follows the choice.
    pub zoomed: bool,
    /// Only the decodes that name your call.
    pub only_yours: bool,
    /// The selected station, by call.
    pub selected: Option<String>,
}

impl Ft8View {
    /// The view as `[ft8]` left it, and a line for each value that was not
    /// one this version knows (left at its default, never guessed).
    pub fn from_settings(s: &crate::config::Ft8Settings) -> (Self, Vec<String>) {
        let mut said = Vec::new();
        let mut read =
            |field: &str, v: &Option<String>, known: &dyn Fn(&str) -> bool| match v.as_deref() {
                Some(w) if !known(w) => {
                    said.push(format!(
                        "[ft8] {field} {w:?} is not one this version knows: the default"
                    ));
                    None
                }
                other => other.map(str::to_string),
            };
        let range = read("range", &s.range, &|w| MapRange::from_word(w).is_some())
            .and_then(|w| MapRange::from_word(&w));
        let names = read("names", &s.names, &|w| Names::from_word(w).is_some())
            .and_then(|w| Names::from_word(&w));
        let window = read("window", &s.window, &|w| Window::from_word(w).is_some())
            .and_then(|w| Window::from_word(&w));
        let d = MapLayers::default();
        let view = Ft8View {
            range: range.unwrap_or_default(),
            map: MapLayers {
                rays: s.rays.unwrap_or(d.rays),
                grid: s.grid.unwrap_or(d.grid),
                rings: s.rings.unwrap_or(d.rings),
                sun: s.sun.unwrap_or(d.sun),
                names: names.unwrap_or_default(),
                window: window.unwrap_or_default(),
                this_band: s.this_band.unwrap_or(d.this_band),
            },
            zoomed: false,
            only_yours: false,
            selected: None,
        };
        (view, said)
    }

    /// What `[ft8]` keeps: only what differs from the default is written.
    pub fn settings(&self) -> crate::config::Ft8Settings {
        let d = MapLayers::default();
        let m = &self.map;
        let differs = |a: bool, b: bool| (a != b).then_some(a);
        crate::config::Ft8Settings {
            range: (self.range != MapRange::default()).then(|| self.range.word().to_string()),
            rays: differs(m.rays, d.rays),
            grid: differs(m.grid, d.grid),
            rings: differs(m.rings, d.rings),
            sun: differs(m.sun, d.sun),
            names: (m.names != d.names).then(|| m.names.word().to_string()),
            window: (m.window != d.window).then(|| m.window.word().to_string()),
            this_band: differs(m.this_band, d.this_band),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Ft8State {
    pub view: Ft8View,
    pub cycles: VecDeque<CycleEntry>,
    pub decodes: VecDeque<Arc<DecodeEntry>>,
    /// Decodes this session, including any let go.
    pub decoded: u64,
    pub stations: BTreeMap<String, Station>,
    pub clock: Option<ClockVerdict>,
    /// The cycle being heard: its start, once the stream's time is known.
    pub hearing: Option<f64>,
    /// How far the receiver has heard, UTC of the latest sample: how far
    /// into [`Ft8State::hearing`] the stream has come.
    pub heard_until: Option<f64>,
    /// A decode is running.
    pub decoding: bool,
    /// The radio's rate offers no route to FT8's 12 000 samples a second.
    pub unreachable_rate: Option<f64>,
    pub feed: Ft8Feed,
}

impl Ft8State {
    /// A finished cycle and its decodes, received at `at`; `my_call` marks
    /// the ones that name it.
    pub fn push_cycle(
        &mut self,
        cycle: CycleEntry,
        decodes: Vec<Decode>,
        clock: Option<ClockVerdict>,
        at: Instant,
        my_call: Option<&str>,
    ) {
        for d in decodes {
            self.decoded += 1;
            let for_you = my_call.is_some_and(|me| d.message.addressees().contains(&me));
            if let Some(call) = d.message.sender() {
                let s = self
                    .stations
                    .entry(call.to_string())
                    .or_insert_with(|| Station {
                        call: call.to_string(),
                        locator: None,
                        last_unix: cycle.start_unix,
                        last_at: at,
                        dial_hz: cycle.dial_hz,
                        heard: 0,
                        snr: VecDeque::new(),
                        called_you: false,
                    });
                s.called_you |= for_you;
                if let Some(loc) = d.message.locator() {
                    s.locator = Some(loc.to_string());
                }
                s.last_unix = cycle.start_unix;
                s.last_at = at;
                s.dial_hz = cycle.dial_hz;
                s.heard += 1;
                s.snr.push_back((cycle.start_unix, d.physics.snr_db));
                if s.snr.len() > SNR_HISTORY {
                    s.snr.pop_front();
                }
            }
            self.decodes.push_back(Arc::new(DecodeEntry {
                n: self.decoded,
                cycle_unix: cycle.start_unix,
                dial_hz: cycle.dial_hz,
                at,
                decode: d,
                for_you,
            }));
        }
        while self.decodes.len() > DECODES_MAX {
            self.decodes.pop_front();
        }
        let now = cycle.start_unix;
        self.stations
            .retain(|_, s| now - s.last_unix <= STATION_SECONDS);
        if clock.is_some() || cycle.outcome == CycleOutcome::Decoded {
            self.clock = clock;
        }
        // In time order, whatever order they were finished in: a cycle the
        // decoder had no room for is published before the one it was busy
        // with.
        let at = self
            .cycles
            .iter()
            .position(|c| c.start_unix > cycle.start_unix)
            .unwrap_or(self.cycles.len());
        self.cycles.insert(at, cycle);
        while self.cycles.len() > CYCLES_MAX {
            self.cycles.pop_front();
        }
    }

    /// The dial being listened to, as the newest cycle was taken at.
    pub fn dial(&self) -> Option<u64> {
        self.cycles.back().map(|c| c.dial_hz)
    }

    /// Whether the map's window and band switch let `s` be drawn, counted
    /// from `newest`, the newest decoded cycle's start.
    pub fn on_map(&self, s: &Station, newest: Option<f64>) -> bool {
        let Some(newest) = newest else {
            return false;
        };
        let m = &self.view.map;
        newest - s.last_unix <= m.window.seconds()
            && (!m.this_band || self.dial() == Some(s.dial_hz))
    }

    /// Where the map puts `s`, in the projection's own units about you (x
    /// east, y north), as the map is drawn at `scale`; `None` without a
    /// locator.
    fn map_point(s: &Station, proj: &crate::geo::projection::Azimuthal) -> Option<(f64, f64)> {
        let place = crate::geo::maidenhead::centre(s.locator.as_deref()?)?;
        Some(proj.forward(place))
    }

    /// Move the choice to the nearest station drawn the way of `dir` on the
    /// map, from the chosen one (or from you): within 60 degrees of the
    /// arrow, the nearest, the ones off the arrow's line counted farther.
    /// Returns whether it moved. A station at the very place of the chosen
    /// one cannot be told apart by direction; the list's arrows reach it.
    pub fn step_on_map(&mut self, home: (f64, f64), dir: MapDir) -> bool {
        use crate::geo::projection::{Azimuthal, Scale};
        // Zoomed, the map is linear whatever the range says, and a linear
        // map's directions do not depend on its range.
        let scale = if self.view.zoomed {
            Scale::Linear { range_km: 1000.0 }
        } else {
            self.view.range.scale()
        };
        let proj = Azimuthal::new(home, scale);
        let newest = self
            .cycles
            .iter()
            .rev()
            .find(|c| c.outcome == CycleOutcome::Decoded)
            .map(|c| c.start_unix);
        let selected = self.view.selected.clone();
        let from = selected
            .as_ref()
            .and_then(|c| self.stations.get(c))
            .and_then(|s| Self::map_point(s, &proj))
            .unwrap_or((0.0, 0.0));
        let (ax, ay) = dir.axis();
        let best = self
            .stations
            .values()
            .filter(|s| Some(&s.call) != selected.as_ref() && self.on_map(s, newest))
            .filter_map(|s| {
                let (x, y) = Self::map_point(s, &proj)?;
                let (dx, dy) = (x - from.0, y - from.1);
                let d = dx.hypot(dy);
                if d < 1e-9 {
                    return None;
                }
                let cos = (dx * ax + dy * ay) / d;
                // Within 60 degrees of the arrow.
                (cos >= 0.5).then(|| (d * (1.0 + 2.0 * (1.0 - cos)), s.call.clone()))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0));
        match best {
            Some((_, call)) => {
                self.view.selected = Some(call);
                true
            }
            None => false,
        }
    }

    /// The selected station's newest decode.
    pub fn selected_decode(&self) -> Option<&Arc<DecodeEntry>> {
        let call = self.view.selected.as_deref()?;
        self.decodes
            .iter()
            .rev()
            .find(|d| d.decode.message.sender() == Some(call))
    }

    /// Forget what was heard (the session counter stays).
    pub fn clear(&mut self) {
        self.cycles.clear();
        self.decodes.clear();
        self.stations.clear();
        self.clock = None;
        self.view.selected = None;
    }

    /// The decodes in the order the list shows them: cycles newest first,
    /// each by tone; only yours when that filter is on.
    pub fn listed(&self) -> Vec<&Arc<DecodeEntry>> {
        let mut cycles: Vec<f64> = self.decodes.iter().map(|d| d.cycle_unix).collect();
        cycles.dedup();
        let mut out = Vec::new();
        for c in cycles.iter().rev() {
            let mut rows: Vec<&Arc<DecodeEntry>> = self
                .decodes
                .iter()
                .filter(|d| d.cycle_unix == *c && (!self.view.only_yours || d.for_you))
                .collect();
            rows.sort_by(|a, b| a.decode.physics.f0.total_cmp(&b.decode.physics.f0));
            out.extend(rows);
        }
        out
    }

    /// Move the selection to the station before or after the selected one,
    /// in the order the list first shows each; with none selected, the first
    /// row's. A station heard in several cycles is one step, not one per
    /// row: stepping by rows found the selected call's first row and went
    /// back and forth between it and the next one forever.
    pub fn select_step(&mut self, down: bool) {
        let rows = self.listed();
        let mut calls: Vec<String> = Vec::new();
        for d in &rows {
            if let Some(c) = d.decode.message.sender() {
                if !calls.iter().any(|k| k == c) {
                    calls.push(c.to_string());
                }
            }
        }
        if calls.is_empty() {
            return;
        }
        let at = self
            .view
            .selected
            .as_ref()
            .and_then(|s| calls.iter().position(|c| c == s));
        let next = match at {
            None => 0,
            Some(i) if down => (i + 1).min(calls.len() - 1),
            Some(i) => i.saturating_sub(1),
        };
        self.view.selected = Some(calls[next].clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::ft8::demod::Physics;
    use crate::signal::ft8::fields::G15;
    use crate::signal::ft8::message::{Call, Message};

    fn decode(from: &str, grid: Option<&str>, snr: f64) -> Decode {
        Decode {
            message: Message::Standard {
                to: Call::Cq(None),
                from: Call::Std {
                    call: from.into(),
                    suffix: None,
                },
                ack: false,
                word: grid.map_or(G15::Report(-10), |g| G15::Grid(g.into())),
            },
            payload: 0,
            dt: 0.2,
            physics: Physics {
                snr_db: snr,
                start: 0.7,
                f0: 1000.0,
                f0_sigma: 0.01,
                drift: 0.0,
                amplitude: 1.0,
                phase: 0.0,
            },
            costas: 21,
            block: 1,
            iterations: 0,
            repaired: 0,
            osd: None,
            distance: 0.0,
            pass: 1,
        }
    }

    fn cycle(start: f64) -> CycleEntry {
        CycleEntry {
            start_unix: start,
            dial_hz: 14_074_000,
            outcome: CycleOutcome::Decoded,
            decodes: 0,
            candidates: 0,
            malformed: 0,
            seconds: 0.0,
            band: Vec::new(),
        }
    }

    #[test]
    fn a_station_keeps_the_locator_it_sent_and_its_history() {
        let mut s = Ft8State::default();
        let now = Instant::now();
        s.push_cycle(
            cycle(0.0),
            vec![decode("JA1VQD", Some("PM95"), -21.0)],
            None,
            now,
            None,
        );
        s.push_cycle(
            cycle(15.0),
            vec![decode("JA1VQD", None, -18.0)],
            None,
            now,
            None,
        );
        let st = &s.stations["JA1VQD"];
        assert_eq!(st.locator.as_deref(), Some("PM95"));
        assert_eq!(st.heard, 2);
        assert_eq!(st.last_unix, 15.0);
        assert_eq!(
            st.snr.iter().map(|x| x.1).collect::<Vec<_>>(),
            vec![-21.0, -18.0]
        );
        assert_eq!(s.decodes.len(), 2);
        assert_eq!(s.decodes.back().unwrap().n, 2);
    }

    #[test]
    fn a_message_naming_your_call_is_yours() {
        let mut s = Ft8State::default();
        let mut d = decode("JA1VQD", Some("PM95"), -21.0);
        d.message = Message::Standard {
            to: Call::Std {
                call: "HA5ZZT".into(),
                suffix: None,
            },
            from: Call::Std {
                call: "JA1VQD".into(),
                suffix: None,
            },
            ack: false,
            word: G15::Grid("PM95".into()),
        };
        let other = decode("EA8ZQ", Some("IL18"), -9.0);
        s.push_cycle(
            cycle(0.0),
            vec![d.clone(), other.clone()],
            None,
            Instant::now(),
            Some("HA5ZZT"),
        );
        assert!(s.decodes[0].for_you);
        assert!(!s.decodes[1].for_you);
        assert!(s.stations["JA1VQD"].called_you);
        assert!(!s.stations["EA8ZQ"].called_you);
        // With no call set, nothing is anyone's.
        let mut t = Ft8State::default();
        t.push_cycle(cycle(0.0), vec![d], None, Instant::now(), None);
        assert!(!t.decodes[0].for_you);
    }

    #[test]
    fn a_station_never_heard_with_a_locator_has_none() {
        let mut s = Ft8State::default();
        s.push_cycle(
            cycle(0.0),
            vec![decode("YO3GCI", None, -1.0)],
            None,
            Instant::now(),
            None,
        );
        assert_eq!(s.stations["YO3GCI"].locator, None);
    }

    #[test]
    fn stations_leave_after_a_day_unheard() {
        // A day, not the hour the map shows by default: the map's session
        // window reaches back as far as the table does.
        let mut s = Ft8State::default();
        let now = Instant::now();
        s.push_cycle(
            cycle(0.0),
            vec![decode("OK1HZD", Some("JO70"), 2.0)],
            None,
            now,
            None,
        );
        s.push_cycle(cycle(STATION_SECONDS), vec![], None, now, None);
        assert!(s.stations.contains_key("OK1HZD"));
        s.push_cycle(cycle(STATION_SECONDS + 15.0), vec![], None, now, None);
        assert!(!s.stations.contains_key("OK1HZD"));
    }

    #[test]
    fn lists_are_bounded() {
        let mut s = Ft8State::default();
        let now = Instant::now();
        for k in 0..60 {
            let ds = (0..10).map(|_| decode("K1ABC", None, -5.0)).collect();
            s.push_cycle(cycle(k as f64 * 15.0), ds, None, now, None);
        }
        assert_eq!(s.cycles.len(), CYCLES_MAX);
        assert_eq!(s.decodes.len(), DECODES_MAX);
        assert_eq!(s.decoded, 600);
    }

    #[test]
    fn the_verdict_needs_five_and_ignores_one_far_off() {
        assert_eq!(verdict(&[0.1, 0.2, 0.2, 0.3]), None);
        let v = verdict(&[0.2, 0.21, 0.19, 0.22, 0.2, 1.9]).unwrap();
        assert!((v.median_dt - 0.205).abs() < 1e-9);
        assert!(v.spread < 0.05, "{}", v.spread);
        assert_eq!(v.decodes, 6);
    }

    #[test]
    fn cycles_keep_time_order_whatever_order_they_finish_in() {
        let mut s = Ft8State::default();
        let now = Instant::now();
        s.push_cycle(cycle(30.0), vec![], None, now, None);
        s.push_cycle(cycle(0.0), vec![], None, now, None);
        s.push_cycle(cycle(15.0), vec![], None, now, None);
        let starts: Vec<f64> = s.cycles.iter().map(|c| c.start_unix).collect();
        assert_eq!(starts, vec![0.0, 15.0, 30.0]);
    }

    #[test]
    fn stepping_walks_every_station_once_even_one_heard_twice() {
        // The live list: DK3JW in the newest cycle and again further down.
        let mut s = Ft8State::default();
        let now = Instant::now();
        let ds = |calls: &[&str]| calls.iter().map(|c| decode(c, None, -10.0)).collect();
        s.push_cycle(cycle(0.0), ds(&["HB9ELE", "DK3JW"]), None, now, None);
        s.push_cycle(cycle(15.0), ds(&["M0BEW"]), None, now, None);
        s.push_cycle(cycle(30.0), ds(&["DK3JW"]), None, now, None);
        s.push_cycle(cycle(45.0), ds(&["F4HBV", "RZ5D"]), None, now, None);
        let mut seen = Vec::new();
        for _ in 0..8 {
            s.select_step(true);
            seen.push(s.view.selected.clone().unwrap());
        }
        assert_eq!(
            seen,
            ["F4HBV", "RZ5D", "DK3JW", "M0BEW", "HB9ELE", "HB9ELE", "HB9ELE", "HB9ELE"]
        );
        for want in ["M0BEW", "DK3JW", "RZ5D", "F4HBV", "F4HBV"] {
            s.select_step(false);
            assert_eq!(s.view.selected.as_deref(), Some(want));
        }
    }

    #[test]
    fn the_map_switches_survive_a_quit_and_only_what_differs_is_written() {
        let mut v = Ft8View::default();
        assert!(v.settings().is_default(), "a default view writes no [ft8]");
        v.range = MapRange::Km3000;
        v.map.rays = false;
        v.map.names = Names::None;
        v.map.window = Window::Min15;
        v.map.this_band = true;
        let s = v.settings();
        assert_eq!(s.grid, None, "an unchanged switch is not written");
        let (back, said) = Ft8View::from_settings(&s);
        assert!(said.is_empty(), "{said:?}");
        assert_eq!(back.range, v.range);
        assert_eq!(back.map, v.map);
    }

    #[test]
    fn a_word_this_version_does_not_know_is_said_and_left_at_the_default() {
        let s = crate::config::Ft8Settings {
            window: Some("fortnight".into()),
            rays: Some(false),
            ..Default::default()
        };
        let (v, said) = Ft8View::from_settings(&s);
        assert_eq!(v.map.window, Window::default());
        assert!(!v.map.rays);
        assert_eq!(said.len(), 1);
        assert!(said[0].contains("window") && said[0].contains("fortnight"));
    }

    #[test]
    fn the_window_and_the_band_decide_who_is_on_the_map() {
        let mut s = Ft8State::default();
        let now = Instant::now();
        s.push_cycle(
            cycle(0.0),
            vec![decode("OLD1", None, -5.0)],
            None,
            now,
            None,
        );
        let mut on40 = cycle(1200.0);
        on40.dial_hz = 7_074_000;
        s.push_cycle(on40, vec![decode("NEW1", None, -5.0)], None, now, None);
        let newest = Some(1200.0);
        let st = |s: &Ft8State, c: &str| s.stations[c].clone();
        // An hour reaches back 20 minutes; 15 minutes does not.
        assert!(s.on_map(&st(&s, "OLD1"), newest));
        s.view.map.window = Window::Min15;
        assert!(!s.on_map(&st(&s, "OLD1"), newest));
        assert!(s.on_map(&st(&s, "NEW1"), newest));
        // This band only: OLD1 was heard on 20 m, the dial is now 40 m.
        s.view.map.window = Window::Session;
        s.view.map.this_band = true;
        assert!(!s.on_map(&st(&s, "OLD1"), newest));
        assert!(s.on_map(&st(&s, "NEW1"), newest));
        // This cycle: only the newest cycle's.
        s.view.map.this_band = false;
        s.view.map.window = Window::Cycle;
        assert!(!s.on_map(&st(&s, "OLD1"), newest));
        assert!(s.on_map(&st(&s, "NEW1"), newest));
    }

    /// Budapest, and a station due north, east, west and south of it, and
    /// one farther east.
    fn compass() -> Ft8State {
        let mut s = Ft8State::default();
        let ds = [
            ("NORTH", "JO99"),
            ("EAST", "KN47"),
            ("FAREAST", "LN47"),
            ("WEST", "JN47"),
            ("SOUTH", "JM99"),
        ]
        .iter()
        .map(|(c, g)| decode(c, Some(g), -10.0))
        .collect();
        s.push_cycle(cycle(0.0), ds, None, Instant::now(), None);
        s
    }

    const BUDAPEST: (f64, f64) = (47.5, 19.0);

    #[test]
    fn arrows_on_the_map_go_the_way_they_point() {
        for (dir, want) in [
            (MapDir::Up, "NORTH"),
            (MapDir::Right, "EAST"),
            (MapDir::Left, "WEST"),
            (MapDir::Down, "SOUTH"),
        ] {
            let mut s = compass();
            assert!(s.step_on_map(BUDAPEST, dir), "{dir:?}");
            assert_eq!(s.view.selected.as_deref(), Some(want), "{dir:?}");
        }
        // From the chosen one, not from you: on east, then farther east,
        // then back.
        let mut s = compass();
        s.step_on_map(BUDAPEST, MapDir::Right);
        s.step_on_map(BUDAPEST, MapDir::Right);
        assert_eq!(s.view.selected.as_deref(), Some("FAREAST"));
        s.step_on_map(BUDAPEST, MapDir::Left);
        assert_eq!(s.view.selected.as_deref(), Some("EAST"));
        // Nothing beyond the northernmost: nothing moves.
        let mut s = compass();
        s.step_on_map(BUDAPEST, MapDir::Up);
        assert!(!s.step_on_map(BUDAPEST, MapDir::Up));
        assert_eq!(s.view.selected.as_deref(), Some("NORTH"));
    }

    #[test]
    fn arrows_on_the_map_skip_what_the_map_does_not_draw() {
        let mut s = compass();
        s.view.map.this_band = true;
        let mut far = cycle(15.0);
        far.dial_hz = 7_074_000;
        s.push_cycle(
            far,
            vec![decode("ON40", Some("JN98"), -10.0)],
            None,
            Instant::now(),
            None,
        );
        // EAST and the rest were heard on 20 m: only the 40 m one is drawn.
        assert!(s.step_on_map(BUDAPEST, MapDir::Up));
        assert_eq!(s.view.selected.as_deref(), Some("ON40"));
        assert!(!s.step_on_map(BUDAPEST, MapDir::Right));
    }

    #[test]
    fn parity_by_the_quarter_minute() {
        assert!(cycle(1_800_000_000.0).even());
        assert!(!cycle(1_800_000_015.0).even());
    }
}
