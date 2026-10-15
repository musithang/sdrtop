// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! An instrument's ballistics for what is drawn, never for what is
//! measured: a level that rises at once and falls slowly, the true peak
//! held a while, and digits refreshed slowly enough to be read.
//!
//! **Fast up, slow down.** A level redrawn at the frame rate follows every
//! burst up and every gap down, a strobe nobody reads. Damped the same way
//! both ways it calms, but swallows the bursts it should show: a 10 ms
//! burst through a 300 ms one-pole reaches 1 - e^(-10/300) of its power,
//! 15 dB short. So the level rises with [`ATTACK`] and falls with
//! [`RELEASE`], as S-meters and programme meters do: a burst is shown
//! within 2 dB, and the fall is what is slowed.
//!
//! **The peak is the readings' own,** not the damped level's: the highest
//! reading, held for [`PEAK_HOLD`], then down to the level.
//!
//! **One quantity, one account.** The digits beside a bar show the bar's
//! own level, sampled every [`READOUT`] so they stand still long enough to
//! be read; they never disagree with the bar by more than a quarter second.
//!
//! Averaged in power, not in decibels, which would bias low. Display only:
//! a measurement, an export or a rule reads the raw reading, never this.

use std::time::{Duration, Instant};

/// How fast a level rises towards a stronger reading.
pub const ATTACK: Duration = Duration::from_millis(10);
/// How fast it falls towards a weaker one.
pub const RELEASE: Duration = Duration::from_millis(300);
/// How long the highest reading is held as the peak.
pub const PEAK_HOLD: Duration = Duration::from_secs(1);
/// How often the digits take the level: four times a second.
pub const READOUT: Duration = Duration::from_millis(250);
/// A pause longer than this starts the meter afresh, rather than swinging
/// from a level that belongs to before it.
pub const RESTART: Duration = Duration::from_secs(1);

/// What a meter shows, in dBFS: the level for its bar, the peak, and the
/// digits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shown {
    pub level: f32,
    pub peak: f32,
    pub readout: f32,
}

/// One meter's ballistics, fed every reading as it comes.
#[derive(Clone, Debug)]
pub struct Ballistic {
    floor: f32,
    power: Option<f64>,
    peak: f32,
    peak_until: Option<Instant>,
    readout: f32,
    readout_at: Option<Instant>,
    last: Option<Instant>,
}

impl Ballistic {
    /// A meter whose silence, a reading that is not finite, is `floor`.
    pub fn new(floor: f32) -> Self {
        Ballistic {
            floor,
            power: None,
            peak: floor,
            peak_until: None,
            readout: floor,
            readout_at: None,
            last: None,
        }
    }

    /// The reading `reading` taken at `now`; what to show, or `None` before
    /// the first finite reading.
    pub fn update(&mut self, reading: f32, now: Instant) -> Option<Shown> {
        let dt = self
            .last
            .map_or(Duration::ZERO, |l| now.saturating_duration_since(l));
        self.last = Some(now);
        if dt > RESTART {
            self.power = None;
        }
        let power = |db: f32| 10f64.powf(f64::from(db) / 10.0);
        let db = |p: f64| (10.0 * p.log10()) as f32;
        let Some(level) = self.power else {
            if !reading.is_finite() {
                return None;
            }
            self.power = Some(power(reading));
            self.peak = reading;
            self.peak_until = Some(now + PEAK_HOLD);
            self.readout = reading;
            self.readout_at = Some(now);
            return Some(Shown {
                level: reading,
                peak: reading,
                readout: reading,
            });
        };
        let target = power(if reading.is_finite() {
            reading
        } else {
            self.floor
        });
        let tau = if target > level { ATTACK } else { RELEASE };
        let alpha = 1.0 - (-dt.as_secs_f64() / tau.as_secs_f64()).exp();
        let level = level + alpha * (target - level);
        self.power = Some(level);
        let shown = db(level).max(self.floor);
        if reading.is_finite() && reading >= self.peak {
            self.peak = reading;
            self.peak_until = Some(now + PEAK_HOLD);
        } else if self.peak_until.is_none_or(|until| now >= until) {
            self.peak = shown;
        }
        if self
            .readout_at
            .is_none_or(|at| now.saturating_duration_since(at) >= READOUT)
        {
            self.readout = shown;
            self.readout_at = Some(now);
        }
        Some(Shown {
            level: shown,
            peak: self.peak.max(shown),
            readout: self.readout,
        })
    }
}

/// How long a new value must hold before it replaces the one shown.
pub const SETTLE: Duration = Duration::from_secs(1);

/// A class that changes on screen only once a new one has held for
/// [`SETTLE`]: a classification that flips on every burst and gap, held
/// still until the flip is real.
///
/// It reads only what is there. A frame with nothing in it (`None`: a gap
/// between bursts) neither changes the value nor restarts the hold of the
/// one waiting to replace it, so a station heard in bursts settles however
/// short they are; a whole [`SETTLE`] with nothing heard and the value is
/// given up.
#[derive(Clone, Debug)]
pub struct Settled<T> {
    shown: Option<T>,
    candidate: Option<(T, Instant)>,
    heard: Option<Instant>,
}

impl<T: Copy + PartialEq> Default for Settled<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Copy + PartialEq> Settled<T> {
    /// Showing nothing until something has held.
    pub fn new() -> Self {
        Settled {
            shown: None,
            candidate: None,
            heard: None,
        }
    }

    /// `value` seen at `now`, `None` for a frame with nothing in it; what
    /// to show.
    pub fn update(&mut self, value: Option<T>, now: Instant) -> Option<T> {
        let Some(value) = value else {
            if self
                .heard
                .is_none_or(|h| now.saturating_duration_since(h) > SETTLE)
            {
                self.shown = None;
                self.candidate = None;
            }
            return self.shown;
        };
        self.heard = Some(now);
        if self.shown == Some(value) {
            self.candidate = None;
            return self.shown;
        }
        match self.candidate {
            Some((c, since)) if c == value => {
                if now.saturating_duration_since(since) >= SETTLE {
                    self.shown = Some(value);
                    self.candidate = None;
                }
            }
            _ => self.candidate = Some((value, now)),
        }
        self.shown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **A value changes only once it has held for a second.** A flip that
    /// lasts 900 ms is never shown; one that lasts a second is, from then
    /// on; a flip back and forth starts the wait again each time.
    #[test]
    fn a_value_changes_only_once_it_has_held() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let mut s = Settled::new();
        assert_eq!(s.update(Some('a'), ms(0)), None, "nothing until it holds");
        assert_eq!(s.update(Some('a'), ms(1000)), Some('a'));
        assert_eq!(s.update(Some('b'), ms(1100)), Some('a'));
        assert_eq!(s.update(Some('b'), ms(1999)), Some('a'));
        assert_eq!(
            s.update(Some('a'), ms(2000)),
            Some('a'),
            "back before it held"
        );
        assert_eq!(s.update(Some('b'), ms(2100)), Some('a'));
        assert_eq!(
            s.update(Some('c'), ms(2500)),
            Some('a'),
            "another candidate starts afresh"
        );
        assert_eq!(s.update(Some('c'), ms(3499)), Some('a'));
        assert_eq!(s.update(Some('c'), ms(3500)), Some('c'));
    }

    /// **A gap is not a reading.** Heard in bursts of 300 ms every second,
    /// a class settles a second after it is first heard, and the gaps in
    /// between neither change it nor restart its hold.
    #[test]
    fn a_gap_neither_changes_nor_restarts_the_hold() {
        let t0 = Instant::now();
        let mut s = Settled::new();
        let mut first = None;
        for ms in (0..5_000u64).step_by(10) {
            let heard = (ms % 1_000 < 300).then_some('w');
            if let Some(w) = s.update(heard, t0 + Duration::from_millis(ms)) {
                assert_eq!(w, 'w');
                first.get_or_insert(ms);
            } else {
                assert!(first.is_none(), "given up at {ms} ms in a short gap");
            }
        }
        assert_eq!(first, Some(1_000));
    }

    /// A second with nothing heard and it is given up; heard again, it
    /// must hold afresh.
    #[test]
    fn a_second_of_nothing_gives_it_up() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let mut s = Settled::new();
        s.update(Some('w'), ms(0));
        assert_eq!(s.update(Some('w'), ms(1000)), Some('w'));
        assert_eq!(s.update(None, ms(2000)), Some('w'));
        assert_eq!(s.update(None, ms(2001)), None);
        assert_eq!(s.update(Some('w'), ms(2100)), None);
        assert_eq!(s.update(Some('w'), ms(3100)), Some('w'));
    }

    /// Readings every 5 ms: `floor` for `quiet` ms, then `level` for
    /// `burst` ms, then `floor` again for `after` ms. Every update's shown.
    fn run(level: f32, burst: u64, after: u64) -> Vec<(u64, Shown)> {
        let t0 = Instant::now();
        let mut b = Ballistic::new(-160.0);
        let mut out = Vec::new();
        for ms in (0..500 + burst + after).step_by(5) {
            let reading = if (500..500 + burst).contains(&ms) {
                level
            } else {
                -80.0
            };
            if let Some(s) = b.update(reading, t0 + Duration::from_millis(ms)) {
                out.push((ms, s));
            }
        }
        out
    }

    fn at(run: &[(u64, Shown)], ms: u64) -> Shown {
        run.iter().find(|(t, _)| *t == ms).unwrap().1
    }

    /// **A burst is shown at once.** A 10 ms burst 60 dB over the floor
    /// reaches within 2 dB of its level (the attack's 1 - e^(-1) in power),
    /// where a symmetric 300 ms one-pole would stop 15 dB short; 50 ms reach
    /// it within a tenth of a decibel.
    #[test]
    fn a_burst_is_shown_at_once() {
        let short = run(-20.0, 10, 0);
        let top = short.iter().map(|(_, s)| s.level).fold(f32::MIN, f32::max);
        assert!(top > -22.1 && top <= -20.0, "{top}");
        let long = run(-20.0, 50, 0);
        let top = long.iter().map(|(_, s)| s.level).fold(f32::MIN, f32::max);
        assert!(top > -20.1, "{top}");
    }

    /// **And falls slowly.** 300 ms after a long burst ends, its power has
    /// fallen to e^-1 of what it was: 4.3 dB down, not at the floor.
    #[test]
    fn it_falls_slowly() {
        let r = run(-20.0, 200, 400);
        let end = at(&r, 695).level;
        let later = at(&r, 995).level;
        assert!((end - later - 4.34).abs() < 0.3, "{end} then {later}");
    }

    /// **The peak is the true peak**, a reading's own level and not the
    /// damped one's, held for a second after it, then down to the level.
    #[test]
    fn the_peak_is_the_readings_own_held_for_a_second() {
        let r = run(-20.0, 10, 1500);
        assert_eq!(at(&r, 505).peak, -20.0);
        assert_eq!(at(&r, 1500).peak, -20.0);
        let released = at(&r, 1550);
        assert_eq!(released.peak, released.level);
    }

    /// **The digits are the level, four times a second.** Over a second of
    /// changing readings they change at most five times, each to the level
    /// at that moment.
    #[test]
    fn the_digits_take_the_level_four_times_a_second() {
        let t0 = Instant::now();
        let mut b = Ballistic::new(-160.0);
        let mut changes = 0;
        let mut last = None;
        for ms in (0..1000).step_by(5) {
            let s = b
                .update(-60.0 + (ms % 40) as f32, t0 + Duration::from_millis(ms))
                .unwrap();
            if last.is_some_and(|l| l != s.readout) {
                changes += 1;
            }
            last = Some(s.readout);
        }
        assert!((3..=5).contains(&changes), "{changes}");
    }

    /// A reading that is not finite is silence: the level falls towards
    /// the floor. Before any finite reading there is nothing to show; after
    /// a pause of more than a second the meter starts afresh.
    #[test]
    fn silence_falls_and_a_pause_starts_afresh() {
        let t0 = Instant::now();
        let mut b = Ballistic::new(-160.0);
        assert_eq!(b.update(f32::NEG_INFINITY, t0), None);
        b.update(-20.0, t0 + Duration::from_millis(5));
        let fell = b
            .update(f32::NEG_INFINITY, t0 + Duration::from_millis(305))
            .unwrap();
        assert!(fell.level < -20.0 && fell.level > -30.0, "{fell:?}");
        let fresh = b.update(-70.0, t0 + Duration::from_secs(3)).unwrap();
        assert_eq!(
            (fresh.level, fresh.peak, fresh.readout),
            (-70.0, -70.0, -70.0)
        );
    }
}
