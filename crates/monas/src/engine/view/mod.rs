// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The engine's readings as data: each a value or the reason there is none,
//! each group with its testimony.
//!
//! Whether a number can be shown is decided here, once, and every client
//! reads the decision: the terminal's panels draw from these readings, and
//! the API serialises them. A panel that worked out on its own whether to
//! dash a number would be a second witness to the same quantity, and the
//! day the two disagreed a script would be told a number the screen
//! refused (POLICY rules 2 and 5).
//!
//! Testimony is per group, not per number, because that is what the state
//! knows: one time, one staleness and one feed account for the radio, one
//! for the spectrum's figures, and so on. Claiming a separate age for each
//! scalar would be inventing it. What each reading carries alone is whether
//! it is there, and if not, why.

pub mod demod;
pub mod ft8;
pub mod iq;
pub mod pager;
pub mod record;
pub mod spectrum;
pub mod vitals;

use std::time::Instant;

use serde::Serialize;

use crate::engine::testimony::{Staleness, Testimony};
use crate::state::SdrMetrics;

/// One reading: its value, or why there is none.
///
/// Serialised as `{"value": 12.5}` or `{"absent": "no spectrum frame yet"}`,
/// so a client cannot read an absent number as a zero.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reading<T> {
    Value(T),
    Absent(String),
}

impl<T> Reading<T> {
    /// `value`, or absent for the reason `why` gives.
    pub fn of(value: Option<T>, why: impl FnOnce() -> String) -> Self {
        match value {
            Some(v) => Reading::Value(v),
            None => Reading::Absent(why()),
        }
    }

    /// The value, when there is one.
    pub fn value(&self) -> Option<&T> {
        match self {
            Reading::Value(v) => Some(v),
            Reading::Absent(_) => None,
        }
    }

    /// Why there is no value, when there is none.
    pub fn absent(&self) -> Option<&str> {
        match self {
            Reading::Value(_) => None,
            Reading::Absent(why) => Some(why),
        }
    }
}

/// A group of readings and what the group says about itself.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Group<T> {
    pub testimony: Testimony,
    #[serde(flatten)]
    pub readings: T,
}

/// The two clocks a view is dated by: the monotonic one the state's
/// instants are on, and the wall clock a reader on another machine has.
#[derive(Clone, Copy, Debug)]
pub struct Clock {
    pub now: Instant,
    pub unix_ms: u64,
}

impl Clock {
    /// Now, on both clocks.
    pub fn now() -> Clock {
        let unix_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
        Clock {
            now: Instant::now(),
            unix_ms,
        }
    }

    /// When `at` was, on the wall clock.
    pub fn unix_ms_of(self, at: Instant) -> u64 {
        let ago = self.now.saturating_duration_since(at).as_millis();
        self.unix_ms
            .saturating_sub(u64::try_from(ago).unwrap_or(u64::MAX))
    }
}

/// Everything the engine reads, as of `clock`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Snapshot {
    /// When the snapshot was taken, milliseconds since the Unix epoch.
    pub at_unix_ms: u64,
    /// The engine's version.
    pub monas: &'static str,
    /// What the engine computes: the name of the view whose work it does.
    pub mode: String,
    pub radio: Group<Radio>,
    pub signal: Group<Signal>,
    pub iq: Group<iq::Iq>,
    pub vitals: Group<vitals::Vitals>,
    pub record: Group<Reading<record::Recording>>,
    pub pager: Group<pager::Pager>,
    pub ft8: Group<ft8::Ft8>,
    pub demod: Group<demod::Demod>,
}

/// The snapshot of `m` as of `clock`.
pub fn snapshot(m: &SdrMetrics, clock: Clock) -> Snapshot {
    Snapshot {
        at_unix_ms: clock.unix_ms,
        monas: env!("CARGO_PKG_VERSION"),
        mode: m.mode.view.clone(),
        radio: radio(m, clock),
        signal: signal(m, clock),
        iq: iq::iq(m, clock),
        vitals: vitals::vitals(m, clock),
        record: record::record(m),
        pager: pager::pager(m, clock),
        ft8: ft8::ft8(m),
        demod: demod::demod(m, clock),
    }
}

/// The radio: which, where it is tuned, the gain chain, the boost, and the
/// frequency reference its offsets are corrected by.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Radio {
    pub name: String,
    pub serial: String,
    pub streaming: bool,
    pub frequency_hz: u64,
    pub sample_rate_hz: f64,
    /// Front to back, by the radio's own names.
    pub stages: Vec<Stage>,
    /// The whole chain in dB, where every stage reads in dB here.
    pub chain_db: Reading<f64>,
    pub boost: Reading<Boost>,
    pub reference: Reading<Reference>,
}

/// One gain stage: what it is set to, and what that gives in dB.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Stage {
    pub name: String,
    /// The stage's setting as stored: dB, or the radio's own step.
    pub setting: f64,
    pub db: Reading<f64>,
}

/// The front end's boost (an amplifier, or the tuner's AGC) and whether it
/// is on.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Boost {
    pub name: String,
    pub on: bool,
}

/// Our oscillator's error, what it was measured against, and how long ago.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Reference {
    /// `relative`, `referenced` or `traceable`.
    pub provenance: String,
    pub ppm: f64,
    pub sigma_ppm: f64,
    pub source: String,
    pub age_s: u64,
}

/// The radio's group. Stale when it is not streaming; dated now, since
/// what it holds is the radio's settings as they stand.
pub fn radio(m: &SdrMetrics, clock: Clock) -> Group<Radio> {
    let stages = m
        .caps
        .gain
        .stages()
        .into_iter()
        .enumerate()
        .map(|(i, spec)| Stage {
            name: spec.name.clone(),
            setting: m.radio.stage_gain(i),
            db: Reading::of(m.stage_db(i), || {
                m.stage_db_refusal(i)
                    .unwrap_or_else(|| format!("{} reads in no dB here", spec.name))
            }),
        })
        .collect();
    let readings = Radio {
        name: m.system.board_name.to_string(),
        serial: m.system.serial.to_string(),
        streaming: m.radio.hw_streaming,
        frequency_hz: m.radio.frequency,
        sample_rate_hz: m.radio.config_sample_rate,
        stages,
        chain_db: Reading::of(m.chain_total_db(), || {
            m.chain_db_refusal()
                .unwrap_or_else(|| "not every stage reads in dB here".to_string())
        }),
        boost: Reading::of(
            m.caps.gain.boost().map(|_| Boost {
                name: m.caps.gain.boost_label().to_string(),
                on: m.radio.amp_enabled,
            }),
            || "this radio has no boost".to_string(),
        ),
        reference: reference(m, clock.now),
    };
    Group {
        testimony: Testimony::resolve(m, Staleness::NotStreaming, None, Some(clock.unix_ms)),
        readings,
    }
}

/// The frequency reference as it is worth now: a reference past its age is
/// no reference, and says which one expired rather than giving its figure,
/// because a number outlives the sentence beside it.
pub fn reference(m: &SdrMetrics, now: Instant) -> Reading<Reference> {
    let Some(r) = m.radio.reference.as_ref() else {
        return Reading::Absent("no reference established".to_string());
    };
    if r.is_stale(now) {
        return Reading::Absent(format!(
            "a {} reference against {} expired after {}",
            r.provenance.label().to_lowercase(),
            r.source,
            crate::export::provenance::clock(crate::state::REFERENCE_STALE_S)
        ));
    }
    Reading::Value(Reference {
        provenance: r.provenance.label().to_lowercase(),
        ppm: r.ppm,
        sigma_ppm: r.sigma_ppm,
        source: r.source.clone(),
        age_s: r.age(now).as_secs(),
    })
}

/// The signal's figures as the Lab benches print them: the frame taken at
/// the last quarter-second tick, and channel power as the S-meter reads it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Signal {
    pub peak_to_nf_db: Reading<f32>,
    /// Approximate: the S-meter's smoothed readout.
    pub channel_power_dbfs: Reading<f32>,
    /// The carrier's, so a gap between bursts does not occupy the span.
    pub occupied_bw_hz: Reading<u64>,
    pub noise_floor_dbfs: Reading<f32>,
}

/// The signal's group. Stale by the spectrum's age; dated by the frame its
/// figures come from.
pub fn signal(m: &SdrMetrics, clock: Clock) -> Group<Signal> {
    Group {
        testimony: Testimony::resolve(
            m,
            Staleness::FftAge,
            None,
            m.signal
                .readout
                .as_ref()
                .map(|frame| clock.unix_ms_of(frame.timestamp)),
        ),
        readings: signal_readings(m),
    }
}

/// The signal's readings alone, for a panel that needs no dates.
pub fn signal_readings(m: &SdrMetrics) -> Signal {
    let stale = Staleness::FftAge.resolve(m);
    let why_frame = || {
        if m.waterfall.last_fft.is_none() || m.signal.readout.is_none() {
            "no spectrum frame yet".to_string()
        } else {
            "the spectrum is stale".to_string()
        }
    };
    let frame = m.signal.readout.as_ref().filter(|_| !stale);
    let finite = |x: f32| x.is_finite().then_some(x);
    Signal {
        peak_to_nf_db: Reading::of(frame.and_then(|fr| finite(fr.peak_to_nf_db)), why_frame),
        channel_power_dbfs: Reading::of(
            m.signal
                .s_meter
                .filter(|_| !stale)
                .and_then(|s| finite(s.readout)),
            || {
                if stale {
                    why_frame()
                } else {
                    "no channel power read yet".to_string()
                }
            },
        ),
        occupied_bw_hz: Reading::of(
            m.signal
                .carrier
                .map(|c| c.occupied_bw_hz)
                .filter(|&bw| bw > 0),
            || "no carrier found".to_string(),
        ),
        noise_floor_dbfs: Reading::of(frame.and_then(|fr| finite(fr.noise_floor)), why_frame),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn clock() -> Clock {
        Clock {
            now: Instant::now(),
            unix_ms: 1_788_632_561_000,
        }
    }

    /// An absent reading serialises as its reason, never as a number a
    /// client could read as zero.
    #[test]
    fn an_absent_reading_is_its_reason_on_the_wire() {
        let absent: Reading<f32> = Reading::Absent("no carrier found".into());
        assert_eq!(
            serde_json::to_value(&absent).unwrap(),
            serde_json::json!({ "absent": "no carrier found" })
        );
        assert_eq!(
            serde_json::to_value(Reading::Value(1.5_f32)).unwrap(),
            serde_json::json!({ "value": 1.5 })
        );
    }

    /// Before the first frame, every figure from the spectrum is absent and
    /// says so, and the group is stale.
    #[test]
    fn with_no_frame_the_signal_figures_are_absent_and_stale() {
        let m = SdrMetrics::fixture();
        let g = signal(&m, clock());
        assert!(g.testimony.stale);
        assert_eq!(
            g.readings.peak_to_nf_db.absent(),
            Some("no spectrum frame yet")
        );
        assert_eq!(
            g.readings.noise_floor_dbfs.absent(),
            Some("no spectrum frame yet")
        );
        assert!(g.readings.channel_power_dbfs.absent().is_some());
    }

    /// A live frame gives the figures and dates the group by that frame.
    #[test]
    fn a_live_frame_gives_the_figures_dated_by_it() {
        let m = SdrMetrics::fixture().streaming().with_carrier(0.0, 40.0);
        let c = clock();
        let g = signal(&m, c);
        assert!(!g.testimony.stale);
        assert!(g.readings.peak_to_nf_db.value().is_some());
        assert!(g.readings.occupied_bw_hz.value().is_some());
        let at = g.testimony.at_unix_ms.expect("dated by its frame");
        assert!(at <= c.unix_ms && c.unix_ms - at < 60_000);
    }

    /// A frame past its age gives no figures: a frozen number never reads
    /// as live.
    #[test]
    fn a_stale_frame_gives_no_figures() {
        let m = SdrMetrics::fixture()
            .streaming()
            .with_carrier(0.0, 40.0)
            .with_stale_fft();
        let g = signal(&m, clock());
        assert!(g.testimony.stale);
        assert_eq!(
            g.readings.peak_to_nf_db.absent(),
            Some("the spectrum is stale")
        );
        assert_eq!(
            g.readings.channel_power_dbfs.absent(),
            Some("the spectrum is stale")
        );
    }

    /// Each stage reads in dB where it was measured, and the chain sums
    /// them; under the AGC neither has a figure, and each says why.
    #[test]
    fn the_chain_reads_in_db_or_says_why_not() {
        let m = SdrMetrics::fixture().streaming().named_chain();
        let g = radio(&m, clock());
        assert!(!g.readings.stages.is_empty());
        assert!(g.readings.stages.iter().all(|s| s.db.value().is_some()));
        assert!(g.readings.chain_db.value().is_some());

        let mut agc = SdrMetrics::fixture().streaming().rtlsdr();
        agc.radio.amp_enabled = true;
        let g = radio(&agc, clock());
        if (0..g.readings.stages.len()).any(|i| agc.agc_sets(i)) {
            assert!(g
                .readings
                .chain_db
                .absent()
                .is_some_and(|w| w.contains("AGC")));
        }
    }

    /// A radio that is not streaming has a stale group, dated now.
    #[test]
    fn a_radio_not_streaming_is_stale() {
        let c = clock();
        let g = radio(&SdrMetrics::fixture(), c);
        assert!(g.testimony.stale);
        assert_eq!(g.testimony.at_unix_ms, Some(c.unix_ms));
    }

    /// A reference past its age is no reference, and gives no figure.
    #[test]
    fn an_expired_reference_gives_no_figure() {
        let mut m = SdrMetrics::fixture();
        assert_eq!(
            reference(&m, Instant::now()).absent(),
            Some("no reference established")
        );
        let taken = Instant::now();
        m.radio.reference = Some(crate::state::FrequencyReference {
            ppm: 1.25,
            sigma_ppm: 0.1,
            provenance: crate::state::Provenance::Referenced,
            source: "WWV 10 MHz".into(),
            at: taken,
            efficiency: None,
            trusted: None,
        });
        let fresh = reference(&m, taken + Duration::from_secs(60));
        assert_eq!(fresh.value().map(|r| r.ppm), Some(1.25));
        let late = taken + Duration::from_secs(crate::state::REFERENCE_STALE_S + 1);
        let expired = reference(&m, late);
        assert!(expired.absent().is_some_and(|w| w.contains("expired")));
    }

    /// The wall-clock date of an instant is now's, less how long ago it was.
    #[test]
    fn an_instant_is_dated_on_the_wall_clock() {
        let c = clock();
        assert_eq!(c.unix_ms_of(c.now), c.unix_ms);
        assert_eq!(
            c.unix_ms_of(c.now - Duration::from_millis(250)),
            c.unix_ms - 250
        );
        // An instant after now (a clock read in the other order) is now.
        assert_eq!(c.unix_ms_of(c.now + Duration::from_secs(1)), c.unix_ms);
    }

    /// The snapshot has the engine's state and nothing of a screen's.
    #[test]
    fn the_snapshot_carries_no_screen_state() {
        let m = SdrMetrics::fixture().streaming().with_carrier(0.0, 40.0);
        let text = serde_json::to_string(&snapshot(&m, clock())).unwrap();
        for screen in ["rail_mode", "menu", "focused", "preset", "theme"] {
            assert!(!text.contains(screen), "{screen} in {text}");
        }
    }
}
