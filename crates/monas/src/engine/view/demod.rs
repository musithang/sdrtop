// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The demodulator's readings, as the Lab's demodulator panel shows them:
//! FM deviation and the carrier's offset, AM depth, the stereo pilot, the
//! CTCSS tone and RDS, each only while it is being renewed.
//!
//! Each figure is the demodulator's own "live" reading, the same call the
//! panel makes, so neither can show an expired one. RDS is the one reading
//! that accumulates, and so the one that can outlive its station: past a
//! few seconds its age is said, past half a minute it is not shown at all.

use serde::Serialize;

use super::{Clock, Group, Reading};
use crate::engine::testimony::Testimony;
use crate::state::{SdrMetrics, RDS_AGED_AFTER, RDS_DROPPED_AFTER};

/// The demodulator's group's readings.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Demod {
    /// Whether blocks are being given to the demodulator at all.
    pub running: bool,
    /// The channel's centre, Hz from the tuned frequency.
    pub offset_hz: i64,
    /// The rate the channel lands on after decimation.
    pub channel_rate_hz: Reading<f64>,
    /// The demodulator forced in place of the classifier's choice, if one is.
    pub forced: Option<&'static str>,
    pub fm: Reading<Fm>,
    pub am: Reading<Am>,
    pub pilot: Reading<Pilot>,
    pub ctcss: Reading<Ctcss>,
    pub rds: Reading<Rds>,
    /// Audio blocks missing from the channel's run this session.
    pub blocks_dropped: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Fm {
    pub peak_dev_hz: f32,
    pub rms_dev_hz: f32,
    pub carrier_offset_hz: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Am {
    pub depth_pct: f32,
    pub positive_pct: f32,
    pub negative_pct: f32,
    pub carrier_dbfs: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Pilot {
    /// `absent`, `marginal` (detectable, below a trustworthy injection) or
    /// `locked`.
    pub state: &'static str,
    pub deviation_hz: f32,
    pub injection_pct: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Ctcss {
    pub tone_hz: f32,
    pub deviation_hz: f32,
    pub margin_db: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Rds {
    pub pi: Option<u16>,
    pub pty: Option<u8>,
    /// The station's name, as broadcast.
    pub ps: Option<String>,
    /// RadioText, as broadcast.
    pub rt: Option<String>,
    pub groups_ok: u32,
    pub groups_session: u32,
    /// Whether the block synchroniser is tracking the groups now.
    pub sync: bool,
    /// Seconds since the last whole group.
    pub age_s: f64,
    /// Past the few seconds after which the name is said with its age.
    pub aged: bool,
}

/// The demodulator's group. Stale once its readings stop being renewed;
/// dated by the last renewal; `feed_loss` while its channel has lately lost
/// audio, when RDS and CTCSS cannot be blamed on the station.
pub fn demod(m: &SdrMetrics, clock: Clock) -> Group<Demod> {
    let d = &m.demod;
    Group {
        testimony: Testimony {
            at_unix_ms: d.last_update.map(|t| clock.unix_ms_of(t)),
            stale: d.is_stale(),
            feed_loss: d.dropping().is_some(),
            absent: None,
        },
        readings: demod_readings(m),
    }
}

/// The demodulator's readings alone.
pub fn demod_readings(m: &SdrMetrics) -> Demod {
    let d = &m.demod;
    let quiet = || {
        if !d.enabled {
            "the demodulator is not running".to_string()
        } else if d.is_stale() {
            "no measurement renewed lately".to_string()
        } else {
            String::new()
        }
    };
    // The reason a reading is missing: the demodulator's state first, then
    // the reading's own.
    let why = |own: &str| {
        let q = quiet();
        if q.is_empty() {
            own.to_string()
        } else {
            q
        }
    };
    let rds = d.live_rds().zip(d.rds_age()).and_then(|(r, age)| {
        (age <= RDS_DROPPED_AFTER).then(|| Rds {
            pi: r.pi,
            pty: r.pty,
            ps: r.ps.clone(),
            rt: r.rt.clone(),
            groups_ok: r.groups_ok,
            groups_session: r.groups_session,
            sync: d.rds_sync,
            age_s: age.as_secs_f64(),
            aged: age > RDS_AGED_AFTER,
        })
    });
    Demod {
        running: d.enabled,
        offset_hz: d.offset_hz,
        channel_rate_hz: Reading::of((d.decimation > 0).then_some(d.channel_rate_hz), || {
            why("no channel built yet")
        }),
        forced: d.mode_override.map(|m| m.label()),
        fm: Reading::of(
            d.live().map(|f| Fm {
                peak_dev_hz: f.peak_dev_hz,
                rms_dev_hz: f.rms_dev_hz,
                carrier_offset_hz: f.carrier_offset_hz,
            }),
            || why("no FM reading: no carrier, too weak, or not FM"),
        ),
        am: Reading::of(
            d.live_am().map(|a| Am {
                depth_pct: a.depth_pct,
                positive_pct: a.positive_pct,
                negative_pct: a.negative_pct,
                carrier_dbfs: a.carrier_dbfs,
            }),
            || why("no AM reading"),
        ),
        pilot: Reading::of(
            d.live_pilot().map(|p| Pilot {
                state: match p.state {
                    crate::signal::demod::PilotState::Absent => "absent",
                    crate::signal::demod::PilotState::Marginal => "marginal",
                    crate::signal::demod::PilotState::Locked => "locked",
                },
                deviation_hz: p.deviation_hz,
                injection_pct: p.injection_pct,
            }),
            || why("no pilot reading: not wide FM"),
        ),
        ctcss: Reading::of(
            d.live_ctcss().map(|c| Ctcss {
                tone_hz: c.tone_hz,
                deviation_hz: c.deviation_hz,
                margin_db: c.margin_db,
            }),
            || {
                if d.ctcss_searching() {
                    format!(
                        "searching: {:.0} % of the window heard unbroken",
                        d.ctcss_fill * 100.0
                    )
                } else {
                    why("no tone")
                }
            },
        ),
        rds: Reading::of(rds, || match d.rds_age() {
            Some(age) if quiet().is_empty() && age > RDS_DROPPED_AFTER => {
                format!("no RDS for {} s", age.as_secs())
            }
            _ => why("no RDS group yet"),
        }),
        blocks_dropped: d.blocks_dropped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// With the demodulator off every reading says so, and the group is
    /// stale.
    #[test]
    fn a_demodulator_not_running_says_so() {
        let m = SdrMetrics::fixture().streaming();
        let v = demod_readings(&m);
        assert!(!v.running);
        for why in [
            v.fm.absent(),
            v.am.absent(),
            v.pilot.absent(),
            v.rds.absent(),
        ] {
            assert_eq!(why, Some("the demodulator is not running"));
        }
        assert!(demod(&m, Clock::now()).testimony.stale);
    }

    /// A live FM reading is served; once it stops being renewed it is not.
    #[test]
    fn an_fm_reading_is_served_only_while_renewed() {
        let mut m = SdrMetrics::fixture().streaming();
        m.demod.enabled = true;
        m.demod.fm = Some(crate::signal::demod::FmMeasure {
            peak_dev_hz: 75_000.0,
            rms_dev_hz: 22_000.0,
            carrier_offset_hz: 120.0,
        });
        m.demod.last_update = Some(std::time::Instant::now());
        let v = demod_readings(&m);
        assert_eq!(v.fm.value().map(|f| f.peak_dev_hz), Some(75_000.0));

        m.demod.last_update = Some(std::time::Instant::now() - std::time::Duration::from_secs(60));
        assert_eq!(
            demod_readings(&m).fm.absent(),
            Some("no measurement renewed lately")
        );
    }
}
