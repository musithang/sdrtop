// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Where the sun is: the point it stands over at a moment, and its altitude
//! anywhere, so the map can show day, twilight and night.
//!
//! The NOAA Global Monitoring Division's "General Solar Position
//! Calculations": the fractional year, the declination and the equation of
//! time as short Fourier series in it, the hour angle from the equation of
//! time and the longitude. Good to a fraction of a degree, finer than a
//! terminal cell on any map this draws.
//!
//! **Twilight is civil twilight**: the sun's centre between the horizon and
//! 6 degrees below it, the band where the low HF bands open along the grey
//! line. The horizon is the geometric one; NOAA's refraction allowance for
//! sunrise (0.833 degrees) is below what a cell shows.

use crate::export::provenance::civil_from_days;

/// Civil twilight's lower edge, degrees of solar altitude.
pub const TWILIGHT_DEG: f64 = -6.0;

/// Day, twilight or night at a place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Light {
    Day,
    Twilight,
    Night,
}

impl Light {
    pub fn of(altitude_deg: f64) -> Self {
        if altitude_deg > 0.0 {
            Light::Day
        } else if altitude_deg > TWILIGHT_DEG {
            Light::Twilight
        } else {
            Light::Night
        }
    }
}

/// The day of the year (1 on 1 January) and the year's length, for a Unix
/// day number.
fn day_of_year(days: i64) -> (u32, u32) {
    // Days before each month in a common year.
    const BEFORE: [u32; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    let (y, m, d) = civil_from_days(days);
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let doy = BEFORE[(m - 1) as usize] + d + u32::from(leap && m > 2);
    (doy, if leap { 366 } else { 365 })
}

/// The sun at a moment: the declination (the latitude it stands over),
/// degrees, and the equation of time, minutes.
fn declination_and_eot(unix: f64) -> (f64, f64) {
    let days = (unix / 86_400.0).floor() as i64;
    let hour = (unix - days as f64 * 86_400.0) / 3600.0;
    let (doy, len) = day_of_year(days);
    // The fractional year, radians.
    let g = std::f64::consts::TAU / f64::from(len) * (f64::from(doy) - 1.0 + (hour - 12.0) / 24.0);
    let eot = 229.18
        * (0.000075 + 0.001868 * g.cos()
            - 0.032077 * g.sin()
            - 0.014615 * (2.0 * g).cos()
            - 0.040849 * (2.0 * g).sin());
    let decl = 0.006918 - 0.399912 * g.cos() + 0.070257 * g.sin() - 0.006758 * (2.0 * g).cos()
        + 0.000907 * (2.0 * g).sin()
        - 0.002697 * (3.0 * g).cos()
        + 0.00148 * (3.0 * g).sin();
    (decl.to_degrees(), eot)
}

/// The point the sun stands over at `unix`: latitude and longitude, degrees.
/// It is local solar noon there: NOAA's `snoon = 720 - 4 longitude - eqtime`
/// minutes after 00:00 UTC, solved for the longitude.
pub fn subsolar(unix: f64) -> (f64, f64) {
    let (decl, eot) = declination_and_eot(unix);
    let minutes = (unix.rem_euclid(86_400.0)) / 60.0;
    let lon = (720.0 - eot - minutes) / 4.0;
    (decl, (lon + 540.0).rem_euclid(360.0) - 180.0)
}

/// The sun's altitude at `place`, degrees, given where it stands over.
pub fn altitude_deg(subsolar: (f64, f64), place: (f64, f64)) -> f64 {
    let (d, s) = (subsolar.0.to_radians(), place.0.to_radians());
    let ha = (place.1 - subsolar.1).to_radians();
    let cz = s.sin() * d.sin() + s.cos() * d.cos() * ha.cos();
    90.0 - cz.clamp(-1.0, 1.0).acos().to_degrees()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::provenance::iso8601;

    // Unix times of the dates the tests name, each checked against the
    // export's own calendar below.
    const JUNE_21: f64 = 1_782_000_000.0;
    const DEC_21: f64 = 1_797_811_200.0;
    const MAR_20: f64 = 1_773_964_800.0;
    const SEP_23: f64 = 1_790_121_600.0;
    const NOV_03_NOON: f64 = 1_793_707_200.0;
    const FEB_11_NOON: f64 = 1_770_811_200.0;
    const LEAP_DEC_31_NOON: f64 = 1_861_876_800.0;

    #[test]
    fn the_dates_are_the_dates() {
        assert_eq!(iso8601(JUNE_21 as i64), "2026-06-21T00:00:00Z");
        assert_eq!(iso8601(DEC_21 as i64), "2026-12-21T00:00:00Z");
        assert_eq!(iso8601(MAR_20 as i64), "2026-03-20T00:00:00Z");
        assert_eq!(iso8601(SEP_23 as i64), "2026-09-23T00:00:00Z");
        assert_eq!(iso8601(LEAP_DEC_31_NOON as i64), "2028-12-31T12:00:00Z");
    }

    #[test]
    fn day_of_year_counts_the_leap_day() {
        assert_eq!(
            day_of_year((LEAP_DEC_31_NOON / 86_400.0) as i64),
            (366, 366)
        );
        assert_eq!(day_of_year((JUNE_21 / 86_400.0) as i64), (172, 365));
    }

    #[test]
    fn the_sun_stands_over_the_tropics_at_the_solstices_and_the_equator_at_the_equinoxes() {
        // The Earth's tilt is a little under 23.5 degrees.
        let at = |t: f64| subsolar(t + 12.0 * 3600.0).0;
        assert!((at(JUNE_21) - 23.44).abs() < 0.3, "{}", at(JUNE_21));
        assert!((at(DEC_21) + 23.44).abs() < 0.3, "{}", at(DEC_21));
        assert!(at(MAR_20).abs() < 0.6, "{}", at(MAR_20));
        assert!(at(SEP_23).abs() < 0.6, "{}", at(SEP_23));
    }

    #[test]
    fn the_equation_of_time_swings_about_a_quarter_hour_each_way() {
        // Its extremes: early November the sun is about 16 minutes ahead of
        // the clock, mid-February about 14 behind.
        let (_, nov) = declination_and_eot(NOV_03_NOON);
        let (_, feb) = declination_and_eot(FEB_11_NOON);
        assert!((15.5..17.0).contains(&nov), "{nov}");
        assert!((-15.0..-13.5).contains(&feb), "{feb}");
        // Ahead of the clock, it crossed Greenwich's meridian before 12:00
        // UTC, so at 12:00 it stands a quarter degree a minute to the west.
        let (_, lon) = subsolar(NOV_03_NOON);
        assert!((lon + nov / 4.0).abs() < 1e-9, "{lon}");
        assert!(lon < -3.5, "{lon}");
    }

    #[test]
    fn under_the_sun_it_is_overhead_and_opposite_it_is_midnight() {
        let t = 1_793_366_100.0; // 2026-10-30 13:15 UTC
        let s = subsolar(t);
        assert!((altitude_deg(s, s) - 90.0).abs() < 1e-6);
        let anti = (-s.0, s.1 + 180.0);
        assert!((altitude_deg(s, anti) + 90.0).abs() < 1e-6);
        // A quarter of the way round, on the terminator.
        assert!(altitude_deg(s, (0.0, s.1 + 90.0)).abs() < 15.0);
    }

    #[test]
    fn day_twilight_and_night() {
        assert_eq!(Light::of(10.0), Light::Day);
        assert_eq!(Light::of(-3.0), Light::Twilight);
        assert_eq!(Light::of(-6.0), Light::Night);
        // Budapest in the early afternoon of 30 October is in daylight,
        // Tokyo (eight hours east) in the night.
        let s = subsolar(1_793_366_100.0);
        assert_eq!(Light::of(altitude_deg(s, (47.5, 19.0))), Light::Day);
        assert_eq!(Light::of(altitude_deg(s, (35.7, 139.7))), Light::Night);
    }
}
