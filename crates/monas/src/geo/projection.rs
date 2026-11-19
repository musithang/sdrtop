// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The globe laid flat around the listener: azimuthal equidistant, so every
//! place lies in the true direction of the great circle that reaches it, and
//! every great circle through the listener is a straight line through the
//! middle. Distance from the middle is a chosen scale of the true distance:
//!
//! - **logarithmic** ([`Scale::Log`]): the whole world in the disc, its edge
//!   the antipode, the near ground opened out. Linear up to about
//!   [`LOG_KNEE_KM`], logarithmic beyond, the way a dB scale treats power:
//!   a neighbour country and the far side of the world both readable at once.
//! - **linear** ([`Scale::Linear`]): true to scale out to a range, which sits
//!   at the edge; beyond it, off the map.
//!
//! Positions are in units of the edge: `(0, 0)` the listener, a radius of 1
//! the edge, `x` east and `y` north.

use super::sphere::{bearing_deg, destination, distance_km, ANTIPODE_KM};

/// Where the logarithmic scale turns from nearly linear to logarithmic, km.
pub const LOG_KNEE_KM: f64 = 250.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Scale {
    Log,
    Linear { range_km: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Azimuthal {
    pub centre: (f64, f64),
    pub scale: Scale,
}

impl Azimuthal {
    pub fn new(centre: (f64, f64), scale: Scale) -> Self {
        Self { centre, scale }
    }

    /// A distance from the listener, km, as a radius (1 at the edge).
    pub fn radius(&self, km: f64) -> f64 {
        match self.scale {
            Scale::Log => (km / LOG_KNEE_KM).ln_1p() / (ANTIPODE_KM / LOG_KNEE_KM).ln_1p(),
            Scale::Linear { range_km } => km / range_km,
        }
    }

    /// A radius back to the distance it stands for, km.
    pub fn distance(&self, r: f64) -> f64 {
        match self.scale {
            Scale::Log => LOG_KNEE_KM * (r * (ANTIPODE_KM / LOG_KNEE_KM).ln_1p()).exp_m1(),
            Scale::Linear { range_km } => r * range_km,
        }
    }

    /// A place on the map.
    pub fn forward(&self, place: (f64, f64)) -> (f64, f64) {
        let r = self.radius(distance_km(self.centre, place));
        let b = bearing_deg(self.centre, place).to_radians();
        (r * b.sin(), r * b.cos())
    }

    /// The place at a point of the map; `None` beyond the antipode, where
    /// there is no Earth left.
    pub fn inverse(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        let km = self.distance(x.hypot(y));
        if km > ANTIPODE_KM {
            return None;
        }
        let b = x.atan2(y).to_degrees();
        Some(destination(self.centre, b, km))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: (f64, f64) = (47.497, 19.04);

    #[test]
    fn the_listener_is_the_middle_and_the_antipode_the_edge() {
        let map = Azimuthal::new(HOME, Scale::Log);
        assert_eq!(map.forward(HOME), (0.0, 0.0));
        assert!((map.radius(ANTIPODE_KM) - 1.0).abs() < 1e-12);
        assert!(map.inverse(0.0, 1.001).is_none());
        let lin = Azimuthal::new(HOME, Scale::Linear { range_km: 3000.0 });
        assert!((lin.radius(3000.0) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn the_log_scale_opens_the_near_ground_and_keeps_the_far() {
        let map = Azimuthal::new(HOME, Scale::Log);
        let r = |km| map.radius(km);
        // Monotonic, and the scale and its inverse agree.
        let mut last = -1.0;
        for km in [0.0, 50.0, 250.0, 1000.0, 2000.0, 5000.0, 10_000.0, 20_000.0] {
            assert!(r(km) > last);
            last = r(km);
            assert!((map.distance(r(km)) - km).abs() < 1e-6, "{km}");
        }
        // 2000 km sits near half the radius: the near world gets half the map.
        assert!((0.45..0.55).contains(&r(2000.0)), "{}", r(2000.0));
    }

    #[test]
    fn a_place_comes_back_from_where_it_was_drawn() {
        for scale in [Scale::Log, Scale::Linear { range_km: 9000.0 }] {
            let map = Azimuthal::new(HOME, scale);
            for p in [
                (35.69, 139.69),
                (-37.8, 144.96),
                (40.7, -74.0),
                (-89.0, 0.0),
                (0.0, -179.9),
            ] {
                let (x, y) = map.forward(p);
                let (lat, lon) = map.inverse(x, y).unwrap();
                assert!(
                    (lat - p.0).abs() < 1e-6 && (lon - p.1).abs() < 1e-6,
                    "{p:?} -> {lat},{lon}"
                );
            }
        }
    }

    #[test]
    fn a_great_circle_through_the_listener_is_a_straight_line() {
        // Points along one bearing land on one ray from the middle.
        let map = Azimuthal::new(HOME, Scale::Log);
        let angle = |p| {
            let (x, y): (f64, f64) = map.forward(p);
            x.atan2(y)
        };
        let first = angle(destination(HOME, 46.0, 1000.0));
        for km in [3000.0, 6000.0, 9000.0, 15_000.0] {
            assert!(
                (angle(destination(HOME, 46.0, km)) - first).abs() < 1e-9,
                "{km}"
            );
        }
    }

    #[test]
    fn north_is_up_and_east_is_right() {
        let map = Azimuthal::new(HOME, Scale::Linear { range_km: 5000.0 });
        let (x, y) = map.forward((HOME.0 + 10.0, HOME.1));
        assert!(x.abs() < 1e-9 && y > 0.0);
        let (x, _) = map.forward(destination(HOME, 90.0, 1000.0));
        assert!(x > 0.0);
    }
}
