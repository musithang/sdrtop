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

#[derive(Clone, Debug, Default)]
pub struct Ft8State {
    pub cycles: VecDeque<CycleEntry>,
    pub decodes: VecDeque<Arc<DecodeEntry>>,
    /// Decodes this session, including any let go.
    pub decoded: u64,
    pub stations: BTreeMap<String, Station>,
    pub clock: Option<ClockVerdict>,
    /// The cycle being heard: its start, once the stream's time is known.
    pub hearing: Option<f64>,
    /// A decode is running.
    pub decoding: bool,
    /// The radio's rate offers no route to FT8's 12 000 samples a second.
    pub unreachable_rate: Option<f64>,
    pub feed: Ft8Feed,
}

impl Ft8State {
    /// A finished cycle and its decodes, received at `at` / `unix`.
    pub fn push_cycle(
        &mut self,
        cycle: CycleEntry,
        decodes: Vec<Decode>,
        clock: Option<ClockVerdict>,
        at: Instant,
    ) {
        for d in decodes {
            self.decoded += 1;
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
                    });
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
        self.cycles.push_back(cycle);
        while self.cycles.len() > CYCLES_MAX {
            self.cycles.pop_front();
        }
    }

    /// Forget what was heard (the session counter stays).
    // Unread until the view's `c` key calls it; `expect` fails the build
    // once something does.
    #[expect(dead_code)]
    pub fn clear(&mut self) {
        self.cycles.clear();
        self.decodes.clear();
        self.stations.clear();
        self.clock = None;
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
        );
        s.push_cycle(cycle(15.0), vec![decode("JA1VQD", None, -18.0)], None, now);
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
    fn a_station_never_heard_with_a_locator_has_none() {
        let mut s = Ft8State::default();
        s.push_cycle(
            cycle(0.0),
            vec![decode("YO3GCI", None, -1.0)],
            None,
            Instant::now(),
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
        );
        s.push_cycle(cycle(3600.0), vec![], None, now);
        assert!(s.stations.contains_key("OK1HZD"));
        s.push_cycle(cycle(3615.0), vec![], None, now);
        assert!(!s.stations.contains_key("OK1HZD"));
    }

    #[test]
    fn lists_are_bounded() {
        let mut s = Ft8State::default();
        let now = Instant::now();
        for k in 0..60 {
            let ds = (0..10).map(|_| decode("K1ABC", None, -5.0)).collect();
            s.push_cycle(cycle(k as f64 * 15.0), ds, None, now);
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
    fn parity_by_the_quarter_minute() {
        assert!(cycle(1_800_000_000.0).even());
        assert!(!cycle(1_800_000_015.0).even());
    }
}
