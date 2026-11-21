// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The I/Q front end's condition: the DC offset and the LO leak it makes,
//! the gain and phase imbalance and the image rejection they allow, as the
//! IQ Diagnostics bench reads them.
//!
//! These are the **residual** after any correction that is running, so a
//! correction that is on and a reading that is still bad means the
//! correction is not keeping up.

use serde::Serialize;

use super::{Clock, Group, Reading};
use crate::engine::testimony::{Staleness, Testimony};
use crate::state::SdrMetrics;

/// The image rejection the imbalance allows, capped where the formula runs
/// away: as the imbalance approaches zero it gives hundreds of dB, and no
/// front end measures that.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Irr {
    pub db: f64,
    /// `db` is a floor: the formula said more, and more is arithmetic, not
    /// a radio.
    pub at_least: bool,
}

/// Above this the image rejection is reported as "at least" it.
pub const IRR_CAP_DB: f64 = 60.0;

/// The I/Q group's readings.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Iq {
    pub dc_i: Reading<f32>,
    pub dc_q: Reading<f32>,
    /// How tall the spike the DC makes in the spectrum is.
    pub dc_spike_dbfs: Reading<f64>,
    /// How far the DC stands over (positive) or under the signal's rms.
    pub dc_over_signal_db: Reading<f64>,
    pub gain_imbalance_db: Reading<f32>,
    pub phase_imbalance_deg: Reading<f32>,
    pub image_rejection: Reading<Irr>,
    /// Where the LO sits, Hz from the centre: zero on a radio that mixes
    /// straight to 0 Hz, where the DC above is the LO's leak.
    pub lo_offset_hz: i64,
    /// The spectrum's level at an LO off the centre.
    pub lo_level_dbfs: Reading<f64>,
    /// Who corrected DC and balance before the engine saw the samples, when
    /// someone did: the readings are what is left.
    pub corrected_by: Option<&'static str>,
    pub dc_block_on: bool,
    pub correction_applied: bool,
}

/// What the spectrum says at an LO off the centre.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LoLine {
    /// The LO is at the centre: the DC reading is its leak.
    AtTheCentre,
    /// No spectrum frame to read it from yet.
    NoSpectrum,
    /// Off the edge of the frame.
    OutOfSpan,
    /// The highest of the five bins round it, dBFS per bin.
    Level(f64),
}

impl LoLine {
    /// The spectrum's level at the LO, or why there is none.
    pub fn reading(self) -> Reading<f64> {
        match self {
            LoLine::Level(db) => Reading::Value(db),
            LoLine::AtTheCentre => {
                Reading::Absent("the LO is at the centre, where the DC is its leak".into())
            }
            LoLine::NoSpectrum => Reading::Absent("no spectrum frame yet".into()),
            LoLine::OutOfSpan => Reading::Absent("the LO is outside the spectrum's span".into()),
        }
    }
}

/// The spectrum's level at the radio's LO, where the LO is off the centre.
pub fn lo_line(state: &SdrMetrics) -> LoLine {
    let offset = state.radio.lo_offset_hz;
    if offset == 0 {
        return LoLine::AtTheCentre;
    }
    let Some(frame) = state.waterfall.last_fft.as_ref() else {
        return LoLine::NoSpectrum;
    };
    let n = frame.bins_dbfs.len();
    if n == 0 || frame.sample_rate <= 0.0 {
        return LoLine::NoSpectrum;
    }
    let at = (n as f64 / 2.0 + offset as f64 * n as f64 / frame.sample_rate).round();
    if at < 0.0 || at >= n as f64 {
        return LoLine::OutOfSpan;
    }
    let at = at as usize;
    let peak = frame.bins_dbfs[at.saturating_sub(2)..=(at + 2).min(n - 1)]
        .iter()
        .copied()
        .fold(f32::NEG_INFINITY, f32::max);
    LoLine::Level(f64::from(peak))
}

/// How tall the spike a DC offset of magnitude `dc_mag` makes, dBFS:
/// `20 log10(dc_mag)`. `None` with no offset at all.
pub fn dc_spike_dbfs(dc_mag: f64) -> Option<f64> {
    (dc_mag > 0.0).then(|| 20.0 * dc_mag.log10())
}

/// The image rejection the imbalance allows, capped at [`IRR_CAP_DB`].
pub fn image_rejection(amp_db: f32, phase_deg: f32) -> Irr {
    let db = crate::signal::image_rejection_db(amp_db, phase_deg);
    if db >= IRR_CAP_DB {
        Irr {
            db: IRR_CAP_DB,
            at_least: true,
        }
    } else {
        Irr {
            db,
            at_least: false,
        }
    }
}

/// The I/Q group. Stale when the radio is not streaming, and then without
/// figures: the bench dashes a front end it cannot see. Dated by the last
/// poll of the counters it is read from.
pub fn iq(m: &SdrMetrics, clock: Clock) -> Group<Iq> {
    Group {
        testimony: Testimony::resolve(
            m,
            Staleness::NotStreaming,
            None,
            Some(clock.unix_ms_of(m.radio.last_poll_time)),
        ),
        readings: iq_readings(m),
    }
}

/// The I/Q readings alone.
pub fn iq_readings(m: &SdrMetrics) -> Iq {
    let streaming = m.radio.hw_streaming;
    let not_streaming = || "the radio is not streaming".to_string();
    let live = |x: f32| streaming.then_some(x);
    let dc_i = m.iq.dc_offset_i;
    let dc_q = m.iq.dc_offset_q;
    let dc_mag = f64::from(dc_i).hypot(f64::from(dc_q));
    let spike = dc_spike_dbfs(dc_mag);
    let why_spike = || {
        if streaming {
            "no DC offset at all".to_string()
        } else {
            not_streaming()
        }
    };
    let refusal = m.iq_balance_refusal();
    let why_balance = || refusal.clone().unwrap_or_else(not_streaming);
    let balance = |x: f32| (streaming && refusal.is_none()).then_some(x);
    let amp = m.iq.iq_imbalance_db;
    let phase = m.iq.phase_imbalance_deg;
    Iq {
        dc_i: Reading::of(live(dc_i), not_streaming),
        dc_q: Reading::of(live(dc_q), not_streaming),
        dc_spike_dbfs: Reading::of(spike.filter(|_| streaming), why_spike),
        dc_over_signal_db: Reading::of(
            spike
                .filter(|_| streaming)
                .map(|s| s - f64::from(m.signal.adc_rms_dbfs)),
            why_spike,
        ),
        gain_imbalance_db: Reading::of(balance(amp), why_balance),
        phase_imbalance_deg: Reading::of(balance(phase), why_balance),
        image_rejection: Reading::of(
            balance(amp).map(|_| image_rejection(amp, phase)),
            why_balance,
        ),
        lo_offset_hz: m.radio.lo_offset_hz,
        lo_level_dbfs: if streaming {
            lo_line(m).reading()
        } else {
            Reading::Absent(not_streaming())
        },
        corrected_by: m.radio.upstream_correction,
        dc_block_on: m.iq.cal.dc_block_on,
        correction_applied: m.iq.cal.cal_applied,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dc_spike_typical_values() {
        // dc_mag = 0.005: 20 log10(0.005), about -46 dBFS.
        let s = dc_spike_dbfs(0.005).unwrap();
        assert!((s - (-46.0)).abs() < 0.2, "got {s:.1}");
        assert!(dc_spike_dbfs(0.0).is_none());
    }

    /// The cap exists because the formula diverges as the imbalance goes to
    /// zero: "312.4 dB" would be arithmetic, not a radio.
    #[test]
    fn a_perfect_front_end_reads_as_at_least_the_cap() {
        let irr = image_rejection(0.0, 0.0);
        assert_eq!(
            irr,
            Irr {
                db: IRR_CAP_DB,
                at_least: true
            }
        );
        let real = image_rejection(-4.2, 7.5);
        assert!(!real.at_least && real.db > 0.0 && real.db < IRR_CAP_DB);
    }

    /// A radio not streaming has no I/Q figures, each saying why.
    #[test]
    fn a_radio_not_streaming_gives_no_figures() {
        let v = iq_readings(&SdrMetrics::fixture());
        assert_eq!(v.dc_i.absent(), Some("the radio is not streaming"));
        assert_eq!(
            v.gain_imbalance_db.absent(),
            Some("the radio is not streaming")
        );
        assert_eq!(v.lo_level_dbfs.absent(), Some("the radio is not streaming"));
    }

    /// Samples turned off the LO have a balance that is not the front end's,
    /// and the reason is the engine's one account of it.
    #[test]
    fn a_balance_that_is_not_the_front_ends_is_refused_with_the_reason() {
        let mut m = SdrMetrics::fixture().streaming();
        m.radio.lo_offset_hz = 250_000;
        let v = iq_readings(&m);
        let why = m.iq_balance_refusal().unwrap();
        assert_eq!(v.gain_imbalance_db.absent(), Some(why.as_str()));
        assert_eq!(v.image_rejection.absent(), Some(why.as_str()));
        assert!(v.dc_i.value().is_some(), "the DC is still read");
    }

    /// The magnitude is the distance from zero, so the spike is that
    /// distance's level.
    #[test]
    fn the_spike_is_the_distance_from_centre() {
        let mut m = SdrMetrics::fixture().streaming();
        m.iq.dc_offset_i = 0.03;
        m.iq.dc_offset_q = 0.04;
        let v = iq_readings(&m);
        let want = 20.0 * 0.05_f64.log10();
        assert!((v.dc_spike_dbfs.value().unwrap() - want).abs() < 1e-6);
    }
}
