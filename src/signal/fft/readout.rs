// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! What the Lab shows of a frame's reading, paced for the eye: display only,
//! computed before the lock like the S-meter's ballistics, never read by a
//! measurement.
//!
//! A bursty signal alternates a carrier and noise from one frame to the next,
//! and some figures mean nothing on noise: the adjacent-channel ratio of noise
//! is about 0 dB, its occupied bandwidth the whole span, and a verdict judged
//! frame by frame flips between a carrier and none at every burst. So the
//! carrier figures are taken only from frames with a carrier, held through a
//! gap of up to a second, and refused after that.

use std::time::Instant;

use crate::signal::ballistics::{Ballistic, Settled, Shown, READOUT, SETTLE};
use crate::state::{judge, Carrier, Verdict};

use super::analysis::Reading;
use super::DB_FLOOR;

/// The carrier figures' state, carried from frame to frame.
pub(super) struct CarrierPacing {
    acpr: [Ballistic; 2],
    shown: [Option<Shown>; 2],
    verdict: Settled<Verdict>,
    /// Peak over noise, occupied bandwidth and adjacent level at the last tick.
    figures: Option<(f32, u64, f32)>,
    tick: Option<Instant>,
    heard: Option<Instant>,
}

impl CarrierPacing {
    pub(super) fn new() -> Self {
        Self {
            acpr: [Ballistic::new(DB_FLOOR), Ballistic::new(DB_FLOOR)],
            shown: [None, None],
            verdict: Settled::new(),
            figures: None,
            tick: None,
            heard: None,
        }
    }

    /// One frame's reading at `now`; the carrier figures to show.
    pub(super) fn update(&mut self, r: &Reading, now: Instant) -> Option<Carrier> {
        let judged = judge(
            r.modulation,
            r.peak_to_nf_db,
            r.acpr_lower_db,
            r.acpr_upper_db,
        );
        let verdict = self.verdict.update(judged, now);
        if judged.is_some() {
            self.heard = Some(now);
            for (i, db) in [r.acpr_lower_db, r.acpr_upper_db].into_iter().enumerate() {
                // A side not measured is refused, never fed in as a reading.
                self.shown[i] = db
                    .is_finite()
                    .then(|| self.acpr[i].update(db, now))
                    .flatten();
            }
            if self
                .tick
                .is_none_or(|t| now.saturating_duration_since(t) >= READOUT)
            {
                self.figures = Some((r.peak_to_nf_db, r.occupied_bw_hz, r.adj_carrier_dbfs));
                self.tick = Some(now);
            }
        }
        let heard = self
            .heard
            .filter(|h| now.saturating_duration_since(*h) <= SETTLE)?;
        let (snr_db, occupied_bw_hz, adj_carrier_dbfs) = self.figures?;
        Some(Carrier {
            snr_db,
            occupied_bw_hz,
            adj_carrier_dbfs,
            acpr_lower: self.shown[0],
            acpr_upper: self.shown[1],
            verdict,
            heard,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Modulation;
    use std::time::Duration;

    /// A broadcast carrier: 40 dB over the noise, 180 kHz wide, its
    /// adjacent channels 40 dB down.
    fn carrier(snr: f32) -> Reading {
        Reading {
            noise_floor: -90.0,
            peak_to_nf_db: snr,
            occupied_bw_hz: 180_000,
            channel_power_dbfs: -40.0,
            modulation: crate::state::classify(snr, 180_000),
            acpr_offset_hz: 200_000.0,
            acpr_lower_db: -40.0,
            acpr_upper_db: -42.0,
            adj_carrier_dbfs: -80.0,
        }
    }

    /// The gap between bursts: noise against noise.
    fn gap() -> Reading {
        Reading {
            noise_floor: -90.0,
            peak_to_nf_db: 4.0,
            occupied_bw_hz: 1_900_000,
            channel_power_dbfs: -70.0,
            modulation: Modulation::Unknown,
            acpr_offset_hz: 200_000.0,
            acpr_lower_db: -0.4,
            acpr_upper_db: 0.3,
            adj_carrier_dbfs: -70.0,
        }
    }

    /// Frames every 33 ms over `ms`, the carrier on for the first `on` of
    /// every `period`; what each frame shows.
    fn keyed(on: u64, period: u64, ms: u64) -> Vec<(u64, Option<Carrier>)> {
        let t0 = Instant::now();
        let mut p = CarrierPacing::new();
        (0..ms)
            .step_by(33)
            .map(|t| {
                let r = if t % period < on {
                    carrier(40.0)
                } else {
                    gap()
                };
                (t, p.update(&r, t0 + Duration::from_millis(t)))
            })
            .collect()
    }

    /// **A gap is not read as the carrier.** Keyed 300 ms on, 700 ms off,
    /// the figures stay the carrier's through every gap.
    #[test]
    fn a_gap_is_not_read_as_the_carrier() {
        let frames = keyed(300, 1_000, 4_000);
        assert!(frames.iter().all(|(_, c)| c.is_some()), "dropped in a gap");
        for (t, c) in &frames {
            let c = c.unwrap();
            assert_eq!(c.occupied_bw_hz, 180_000, "at {t} ms");
            assert_eq!(c.snr_db, 40.0, "at {t} ms");
            for side in [c.acpr_lower, c.acpr_upper] {
                assert!(side.unwrap().readout < -35.0, "noise as ACPR at {t} ms");
            }
        }
        let late = frames.last().unwrap().1.unwrap();
        assert_eq!(late.verdict, Some(Verdict::Clean(Modulation::Wfm)));
    }

    /// A second without a carrier and the figures are refused.
    #[test]
    fn a_second_without_a_carrier_refuses_the_figures() {
        let frames = keyed(300, 10_000, 2_000);
        for (t, c) in frames {
            // The last carrier frame is at 297 ms.
            assert_eq!(c.is_some(), t <= 297 + 1_000, "at {t} ms");
        }
    }

    /// **The verdict holds a second.** A carrier that reads clean and weak
    /// on alternate frames keeps the verdict it had; one weak for over a
    /// second is called weak.
    #[test]
    fn the_verdict_holds_a_second() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut p = CarrierPacing::new();
        let mut last = None;
        for t in (0..1_500).step_by(33) {
            last = p.update(&carrier(30.0), at(t)).unwrap().verdict;
        }
        let clean = Some(Verdict::Clean(Modulation::Wfm));
        assert_eq!(last, clean);
        for (i, t) in (1_500..3_000).step_by(33).enumerate() {
            let snr = if i % 2 == 0 { 15.0 } else { 30.0 };
            assert_eq!(p.update(&carrier(snr), at(t)).unwrap().verdict, clean);
        }
        let weak = Some(Verdict::Weak(Modulation::Wfm));
        let called: Vec<_> = (3_000..4_200)
            .step_by(33)
            .map(|t| (t, p.update(&carrier(15.0), at(t)).unwrap().verdict))
            .collect();
        assert!(
            called.iter().all(|(t, v)| (*v == weak) == (*t >= 4_000)),
            "{called:?}"
        );
    }

    /// The digits change at most four times a second, however fast the
    /// frames come.
    #[test]
    fn the_digits_change_four_times_a_second() {
        let t0 = Instant::now();
        let mut p = CarrierPacing::new();
        let mut changes = 0;
        let mut last = None;
        for (i, t) in (0..2_000u64).step_by(10).enumerate() {
            let mut r = carrier(40.0);
            r.occupied_bw_hz = 180_000 + i as u64;
            let bw = p
                .update(&r, t0 + Duration::from_millis(t))
                .unwrap()
                .occupied_bw_hz;
            if last != Some(bw) {
                changes += 1;
                last = Some(bw);
            }
        }
        assert!(changes <= 9, "{changes} changes in two seconds");
    }
}
