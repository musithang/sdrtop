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

/// How long a station stays after it was last heard.
pub const STATION_SECONDS: f64 = 3600.0;

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

/// What the FT8 view shows, set by its keys; the worker never touches it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ft8View {
    pub range: MapRange,
    /// Only the decodes that name your call.
    pub only_yours: bool,
    /// The selected station, by call.
    pub selected: Option<String>,
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

    /// Move the selection to the station of the row before or after the
    /// selected one in the list; with none selected, the first row's.
    pub fn select_step(&mut self, down: bool) {
        let rows = self.listed();
        let calls: Vec<String> = rows
            .iter()
            .filter_map(|d| d.decode.message.sender().map(str::to_string))
            .collect();
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
    fn stations_leave_after_an_hour_unheard() {
        let mut s = Ft8State::default();
        let now = Instant::now();
        s.push_cycle(
            cycle(0.0),
            vec![decode("OK1HZD", Some("JO70"), 2.0)],
            None,
            now,
            None,
        );
        s.push_cycle(cycle(3600.0), vec![], None, now, None);
        assert!(s.stations.contains_key("OK1HZD"));
        s.push_cycle(cycle(3615.0), vec![], None, now, None);
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
    fn parity_by_the_quarter_minute() {
        assert!(cycle(1_800_000_000.0).even());
        assert!(!cycle(1_800_000_015.0).even());
    }
}
