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
use crate::state::{acpr_offset_hz, judge, Carrier, Modulation, Verdict, ACPR_FLOOR_DB};

use super::analysis::Reading;

/// The carrier figures' state, carried from frame to frame.
pub(super) struct CarrierPacing {
    /// The adjacent-channel needles, lower and upper.
    acpr: [Ballistic; 2],
    shown: [Option<Shown>; 2],
    /// Each side's last carrier reading, which a needle holds through a gap.
    held: [Option<f32>; 2],
    /// The offset those readings were measured at.
    offset_hz: Option<f64>,
    /// When a frame last fed the needles.
    acpr_heard: Option<Instant>,
    verdict: Settled<Verdict>,
    /// Peak over noise, occupied bandwidth and adjacent level at the last tick.
    figures: Option<(f32, u64, f32)>,
    /// The needles' levels at the last tick: their digits. Taken four times
    /// a second while the carrier is heard or held, the first on its first
    /// frame, rather than on the meter's own readout, which a needle at rest
    /// last took of the floor.
    digits: [Option<f32>; 2],
    digits_at: Option<Instant>,
    tick: Option<Instant>,
    heard: Option<Instant>,
}

impl CarrierPacing {
    pub(super) fn new() -> Self {
        Self {
            acpr: [
                Ballistic::averaging(ACPR_FLOOR_DB),
                Ballistic::averaging(ACPR_FLOOR_DB),
            ],
            shown: [None, None],
            held: [None, None],
            offset_hz: None,
            acpr_heard: None,
            verdict: Settled::new(),
            figures: None,
            digits: [None, None],
            digits_at: None,
            tick: None,
            heard: None,
        }
    }

    /// The adjacent-channel bars' needles after the last update, lower and
    /// upper; `None` at rest.
    pub(super) fn needle(&self) -> [Option<f32>; 2] {
        self.shown
            .map(|s| s.map(|s| s.level).filter(|l| *l > ACPR_FLOOR_DB + 0.5))
    }

    /// The adjacent-channel offset the needles were measured at, kept through
    /// gaps and through frames measured at another spacing.
    pub(super) fn offset_hz(&self) -> Option<f64> {
        self.offset_hz
    }

    /// One frame's reading at `now`, with the class held for the badge; the
    /// carrier figures to show.
    pub(super) fn update(
        &mut self,
        r: &Reading,
        modulation: Modulation,
        now: Instant,
    ) -> Option<Carrier> {
        let judged = judge(
            r.modulation,
            r.peak_to_nf_db,
            r.acpr_lower_db,
            r.acpr_upper_db,
        );
        let verdict = self.verdict.update(judged, now);
        if judged.is_some() {
            self.heard = Some(now);
        }
        // One bar, one quantity: the needles read at one spacing. The held
        // class's when there is one; with none, the one they started at for
        // as long as they are off rest; at rest, any. A carrier frame measured
        // at another spacing does not move them, and a held class that names
        // a new spacing starts them again from rest.
        let at_rest = self.needle() == [None, None];
        let want = if modulation.is_known() {
            Some(acpr_offset_hz(modulation))
        } else if at_rest {
            None
        } else {
            self.offset_hz
        };
        let same = |a: f64, b: f64| (a - b).abs() < 0.5;
        let feeds = judged.is_some()
            && (r.acpr_lower_db.is_finite() || r.acpr_upper_db.is_finite())
            && want.is_none_or(|w| same(w, r.acpr_offset_hz));
        if feeds {
            if self.offset_hz.is_some_and(|o| !same(o, r.acpr_offset_hz)) {
                self.acpr.iter_mut().for_each(Ballistic::rest);
                self.held = [None, None];
                self.digits_at = None;
            }
            self.offset_hz = Some(r.acpr_offset_hz);
            self.acpr_heard = Some(now);
        }
        let holding = self
            .acpr_heard
            .is_some_and(|h| now.saturating_duration_since(h) <= SETTLE);
        // The needles move every frame. A frame that feeds them gives its
        // ratio (a side it did not measure is refused, never fed in as a
        // reading); any other frame within the hold gives the last one, so the
        // needle stands; after the hold the needle is fed its rest, the floor,
        // and falls back to it at the meter's own pace rather than vanishing.
        // It starts at rest, so a carrier's first frame springs up from there.
        for (i, db) in [r.acpr_lower_db, r.acpr_upper_db].into_iter().enumerate() {
            let feed = if feeds {
                self.held[i] = db.is_finite().then_some(db);
                self.held[i]
            } else if holding {
                self.held[i]
            } else {
                None
            };
            self.shown[i] = self.acpr[i].update(feed.unwrap_or(ACPR_FLOOR_DB), now);
        }
        // Digits four times a second while held, per side and only for a side
        // measured, so a side measured a frame late is never shown a level
        // taken while it was not.
        if !holding {
            self.digits_at = None;
        } else if self
            .digits_at
            .is_none_or(|t| now.saturating_duration_since(t) >= READOUT)
        {
            self.digits = [0, 1].map(|i| self.held[i].and(self.shown[i]).map(|s| s.level));
            self.digits_at = Some(now);
        }
        if judged.is_some()
            && self
                .tick
                .is_none_or(|t| now.saturating_duration_since(t) >= READOUT)
        {
            self.figures = Some((r.peak_to_nf_db, r.occupied_bw_hz, r.adj_carrier_dbfs));
            self.tick = Some(now);
        }
        let heard = self
            .heard
            .filter(|h| now.saturating_duration_since(*h) <= SETTLE)?;
        let (snr_db, occupied_bw_hz, adj_carrier_dbfs) = self.figures?;
        // Digits only for a side measured and held, read at the tick.
        let digits = |i: usize| {
            let shown = self.shown[i].filter(|_| holding && self.held[i].is_some())?;
            Some(Shown {
                readout: self.digits[i].unwrap_or(shown.level),
                ..shown
            })
        };
        Some(Carrier {
            snr_db,
            occupied_bw_hz,
            adj_carrier_dbfs,
            acpr_lower: digits(0),
            acpr_upper: digits(1),
            verdict,
            modulation,
            heard,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
                (
                    t,
                    p.update(&r, Modulation::Wfm, t0 + Duration::from_millis(t)),
                )
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
        assert_eq!(late.verdict, Some(Verdict::Clean));
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
            last = p
                .update(&carrier(30.0), Modulation::Wfm, at(t))
                .unwrap()
                .verdict;
        }
        let clean = Some(Verdict::Clean);
        assert_eq!(last, clean);
        for (i, t) in (1_500..3_000).step_by(33).enumerate() {
            let snr = if i % 2 == 0 { 15.0 } else { 30.0 };
            assert_eq!(
                p.update(&carrier(snr), Modulation::Wfm, at(t))
                    .unwrap()
                    .verdict,
                clean
            );
        }
        let weak = Some(Verdict::Weak);
        let called: Vec<_> = (3_000..4_200)
            .step_by(33)
            .map(|t| {
                (
                    t,
                    p.update(&carrier(15.0), Modulation::Wfm, at(t))
                        .unwrap()
                        .verdict,
                )
            })
            .collect();
        assert!(
            called.iter().all(|(t, v)| (*v == weak) == (*t >= 4_000)),
            "{called:?}"
        );
    }

    /// **The adjacent-channel bars are averaged, not peak-caught.** A ratio
    /// measured on noisy adjacent bands scatters a few dB from frame to frame
    /// on a perfectly steady carrier; the bar must not jump up with every
    /// scatter and fall back.
    #[test]
    fn the_acpr_bars_are_averaged() {
        let t0 = Instant::now();
        let mut p = CarrierPacing::new();
        // A steady carrier whose lower ratio scatters evenly over -45..-35 dB
        // from frame to frame.
        let mut seed = 7u32;
        let mut scatter = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 24) as f32 * 10.0 - 45.0
        };
        let (mut levels, mut powers) = (Vec::new(), Vec::new());
        for t in (0..6_000u64).step_by(33) {
            let mut r = carrier(40.0);
            r.acpr_lower_db = scatter();
            let c = p
                .update(&r, Modulation::Wfm, t0 + Duration::from_millis(t))
                .unwrap();
            if t >= 1_500 {
                levels.push(c.acpr_lower.unwrap().level);
                powers.push(10f64.powf(f64::from(r.acpr_lower_db) / 10.0));
            }
        }
        let lo = levels.iter().copied().fold(f32::INFINITY, f32::min);
        let hi = levels.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        assert!(hi - lo < 2.5, "the bar swings {lo:.1} to {hi:.1} dB");
        // And it reads the ratio's power mean, not its frame-to-frame peaks.
        let mean = (10.0 * (powers.iter().sum::<f64>() / powers.len() as f64).log10()) as f32;
        let shown = levels.iter().sum::<f32>() / levels.len() as f32;
        assert!(
            (shown - mean).abs() < 0.5,
            "shows {shown:.1} dB for a mean of {mean:.1}"
        );
    }

    /// **The needle springs up.** From rest, a carrier's ratio shows within
    /// a frame or two and climbs to it over most of a second, rather than
    /// appearing at once at full height.
    #[test]
    fn the_acpr_needle_springs_up() {
        let t0 = Instant::now();
        let mut p = CarrierPacing::new();
        for t in (0..1_000u64).step_by(33) {
            p.update(&gap(), Modulation::Wfm, t0 + Duration::from_millis(t));
        }
        assert_eq!(p.needle(), [None, None], "at rest before any carrier");
        let mut lifted = Vec::new();
        for t in (1_000..2_500u64).step_by(33) {
            let mut r = carrier(40.0);
            r.acpr_lower_db = -30.0;
            p.update(&r, Modulation::Wfm, t0 + Duration::from_millis(t));
            lifted.push((t, p.needle()[0].unwrap()));
        }
        assert!(
            lifted[0].1 < -40.0,
            "not at full height at once: {lifted:?}"
        );
        assert!(lifted.windows(2).all(|w| w[1].1 >= w[0].1), "{lifted:?}");
        assert!(lifted.last().unwrap().1 > -31.0, "{lifted:?}");
    }

    /// **The digits never read the rest.** A carrier arriving at a needle
    /// at rest shows its own ratio from its first frame, never the floor the
    /// needle was resting on.
    #[test]
    fn the_digits_never_read_the_rest() {
        let t0 = Instant::now();
        let mut p = CarrierPacing::new();
        for t in (0..2_000u64).step_by(33) {
            p.update(&gap(), Modulation::Wfm, t0 + Duration::from_millis(t));
        }
        for t in (2_000..2_600u64).step_by(33) {
            let mut r = carrier(40.0);
            r.acpr_lower_db = -30.0;
            let c = p
                .update(&r, Modulation::Wfm, t0 + Duration::from_millis(t))
                .unwrap();
            let d = c.acpr_lower.unwrap();
            assert!(d.readout > -60.0, "digits {:.1} at {t} ms", d.readout);
        }
    }

    /// **The digits follow their needle through the hold**, four times a
    /// second, rather than stopping at the last carrier frame while the
    /// needle goes on settling.
    #[test]
    fn the_digits_follow_the_needle_through_the_hold() {
        let t0 = Instant::now();
        let mut p = CarrierPacing::new();
        let mut r = carrier(40.0);
        r.acpr_lower_db = -30.0;
        for t in (0..2_000u64).step_by(33) {
            p.update(&gap(), Modulation::Wfm, t0 + Duration::from_millis(t));
        }
        for t in (2_000..2_300u64).step_by(33) {
            p.update(&r, Modulation::Wfm, t0 + Duration::from_millis(t));
        }
        let mut last = None;
        for t in (2_300..3_200u64).step_by(33) {
            last = p.update(&gap(), Modulation::Wfm, t0 + Duration::from_millis(t));
        }
        let d = last.unwrap().acpr_lower.unwrap();
        assert!(
            (d.readout - d.level).abs() < 1.0,
            "digits {:.1}, needle {:.1}",
            d.readout,
            d.level
        );
    }

    /// The offset is the one a ratio was measured at: a carrier frame whose
    /// adjacent bands could not be measured does not move it.
    #[test]
    fn an_unmeasured_frame_does_not_move_the_offset() {
        let t0 = Instant::now();
        let mut p = CarrierPacing::new();
        let mut r = carrier(40.0);
        r.acpr_offset_hz = 9_000.0;
        p.update(&r, Modulation::Am, t0);
        let mut wide = carrier(40.0);
        wide.acpr_offset_hz = 200_000.0;
        wide.acpr_lower_db = f32::NEG_INFINITY;
        wide.acpr_upper_db = f32::NEG_INFINITY;
        p.update(&wide, Modulation::Am, t0 + Duration::from_millis(33));
        assert_eq!(p.offset_hz(), Some(9_000.0));
    }

    /// **One bar, one spacing.** Bursts read as AM in one frame and WFM in
    /// the next are measured at ±9 kHz and ±200 kHz in turn; with no class
    /// held, the needle keeps the spacing it started at and the other frames
    /// do not move it.
    #[test]
    fn a_needle_reads_one_spacing() {
        let t0 = Instant::now();
        let mut p = CarrierPacing::new();
        let mut last = None;
        for (i, t) in (0..2_000u64).step_by(33).enumerate() {
            let mut r = carrier(40.0);
            (r.acpr_offset_hz, r.acpr_lower_db) = if i % 2 == 0 {
                (9_000.0, -45.0)
            } else {
                (200_000.0, -26.0)
            };
            last = p.update(&r, Modulation::Unknown, t0 + Duration::from_millis(t));
        }
        assert_eq!(p.offset_hz(), Some(9_000.0));
        let d = last.unwrap().acpr_lower.unwrap();
        assert!(d.level < -43.0 && d.readout < -43.0, "{d:?}");
    }

    /// The held class picks the spacing: frames measured at another one do
    /// not move the needle, and a needle started at another one starts again
    /// from rest.
    #[test]
    fn the_held_class_picks_the_spacing() {
        let t0 = Instant::now();
        let mut p = CarrierPacing::new();
        let at = |r: f32, off: f64| {
            let mut c = carrier(40.0);
            (c.acpr_offset_hz, c.acpr_lower_db) = (off, r);
            c
        };
        for t in (0..1_000u64).step_by(33) {
            p.update(
                &at(-45.0, 9_000.0),
                Modulation::Unknown,
                t0 + Duration::from_millis(t),
            );
        }
        assert_eq!(p.offset_hz(), Some(9_000.0));
        let c = p
            .update(
                &at(-26.0, 200_000.0),
                Modulation::Wfm,
                t0 + Duration::from_millis(1_033),
            )
            .unwrap();
        assert_eq!(p.offset_hz(), Some(200_000.0));
        assert!(
            c.acpr_lower.unwrap().level < -33.0,
            "afresh from rest: {:?}",
            c.acpr_lower
        );
        for t in (1_066..2_000u64).step_by(33) {
            p.update(
                &at(-45.0, 9_000.0),
                Modulation::Wfm,
                t0 + Duration::from_millis(t),
            );
        }
        assert_eq!(
            p.offset_hz(),
            Some(200_000.0),
            "a 9 kHz frame does not move it"
        );
    }

    /// Digits are taken per side and only for a side measured: a side first
    /// measured a frame after its carrier shows its own level, not one taken
    /// while it was unmeasured.
    #[test]
    fn a_side_measured_late_shows_its_own_digits() {
        let t0 = Instant::now();
        let mut p = CarrierPacing::new();
        for t in (0..1_000u64).step_by(33) {
            p.update(&gap(), Modulation::Wfm, t0 + Duration::from_millis(t));
        }
        let mut first = carrier(40.0);
        first.acpr_lower_db = f32::NEG_INFINITY;
        p.update(&first, Modulation::Wfm, t0 + Duration::from_millis(1_000));
        let mut r = carrier(40.0);
        r.acpr_lower_db = -30.0;
        for t in (1_033..1_200u64).step_by(33) {
            let d = p
                .update(&r, Modulation::Wfm, t0 + Duration::from_millis(t))
                .unwrap()
                .acpr_lower
                .unwrap();
            assert!((d.readout - d.level).abs() < 0.1, "at {t}: {d:?}");
        }
    }

    /// **Released, not vanished.** Through the hold the needle stands; after
    /// it the digits are refused and the needle falls back to rest, every
    /// frame a little, never in a jump.
    #[test]
    fn the_acpr_needle_is_released_not_vanished() {
        let t0 = Instant::now();
        let mut p = CarrierPacing::new();
        for t in (0..2_000u64).step_by(33) {
            let mut r = carrier(40.0);
            r.acpr_lower_db = -30.0;
            p.update(&r, Modulation::Wfm, t0 + Duration::from_millis(t));
        }
        let mut trace = Vec::new();
        for t in (2_000..12_000u64).step_by(33) {
            let c = p.update(&gap(), Modulation::Wfm, t0 + Duration::from_millis(t));
            trace.push((t, c.is_some(), p.needle()[0]));
        }
        for (t, held, needle) in &trace {
            if *t <= 2_970 {
                assert!(*held && needle.unwrap() > -31.0, "held at {t} ms");
            }
        }
        let fall: Vec<f32> = trace.iter().filter_map(|(_, _, n)| *n).collect();
        assert!(
            fall.windows(2).all(|w| w[1] <= w[0] && w[0] - w[1] < 1.5),
            "{fall:?}"
        );
        assert!(
            trace
                .iter()
                .any(|(t, held, n)| !held && n.is_some() && *t > 3_100),
            "released, not gone"
        );
        assert_eq!(trace.last().unwrap().2, None, "at rest in the end");
    }

    /// The offset the needles were measured at is the last carrier's: in a
    /// gap the classifier has no class and the spacing it would use is the
    /// broadcast one, which is not where the needle was measured.
    #[test]
    fn the_needles_offset_is_the_carriers() {
        let t0 = Instant::now();
        let mut p = CarrierPacing::new();
        assert_eq!(p.offset_hz(), None, "nothing measured yet");
        let mut r = carrier(40.0);
        r.acpr_offset_hz = 9_000.0;
        p.update(&r, Modulation::Am, t0);
        for t in (33..3_000u64).step_by(33) {
            p.update(&gap(), Modulation::Am, t0 + Duration::from_millis(t));
            assert_eq!(p.offset_hz(), Some(9_000.0), "at {t} ms");
        }
    }

    /// **A class that flips does not tear the verdict down.** Bursts whose
    /// bandwidth scatters classify as WFM in one frame and AM in the next;
    /// the verdict is a kind (clean, weak, splatter) and settles on it, its
    /// class being the one held for the badge.
    #[test]
    fn a_flipping_class_does_not_unsettle_the_verdict() {
        let t0 = Instant::now();
        let mut p = CarrierPacing::new();
        let mut last = None;
        for (i, t) in (0..1_500u64).step_by(33).enumerate() {
            let mut r = carrier(30.0);
            r.modulation = if i % 2 == 0 {
                Modulation::Wfm
            } else {
                Modulation::Am
            };
            last = p.update(&r, Modulation::Wfm, t0 + Duration::from_millis(t));
        }
        let c = last.unwrap();
        assert_eq!(c.verdict, Some(Verdict::Clean));
        assert_eq!(c.modulation, Modulation::Wfm);
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
                .update(&r, Modulation::Wfm, t0 + Duration::from_millis(t))
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
