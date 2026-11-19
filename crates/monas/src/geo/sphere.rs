// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Great circles on a sphere: the distance between two places, the bearing
//! to set off on, and where a bearing and a distance arrive.

use std::f64::consts::PI;

/// The sphere's radius, km: the Earth's mean radius, to the kilometre. The
/// ellipsoid differs from any sphere by up to about half a per cent; no
/// figure here is shown finer than that.
pub const EARTH_KM: f64 = 6371.0;

/// Half the circumference: how far the antipode is.
pub const ANTIPODE_KM: f64 = PI * EARTH_KM;

/// Great-circle distance, km (the haversine form, which keeps its precision
/// for places close together).
pub fn distance_km(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (p1, p2) = (a.0.to_radians(), b.0.to_radians());
    let dp = p2 - p1;
    let dl = (b.1 - a.1).to_radians();
    let h = (dp / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    2.0 * EARTH_KM * h.sqrt().min(1.0).asin()
}

/// The initial bearing from `a` towards `b` along the short path, degrees
/// clockwise from north, 0 to 360. Undefined (returned as 0) between a place
/// and itself.
pub fn bearing_deg(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (p1, p2) = (a.0.to_radians(), b.0.to_radians());
    let dl = (b.1 - a.1).to_radians();
    let y = dl.sin() * p2.cos();
    let x = p1.cos() * p2.sin() - p1.sin() * p2.cos() * dl.cos();
    if x == 0.0 && y == 0.0 {
        return 0.0;
    }
    y.atan2(x).to_degrees().rem_euclid(360.0)
}

/// Where setting off from `a` on `bearing` (degrees) for `km` arrives.
pub fn destination(a: (f64, f64), bearing: f64, km: f64) -> (f64, f64) {
    let (p1, l1) = (a.0.to_radians(), a.1.to_radians());
    let t = bearing.to_radians();
    let d = km / EARTH_KM;
    let p2 = (p1.sin() * d.cos() + p1.cos() * d.sin() * t.cos())
        .clamp(-1.0, 1.0)
        .asin();
    let l2 = l1 + (t.sin() * d.sin() * p1.cos()).atan2(d.cos() - p1.sin() * p2.sin());
    (
        p2.to_degrees(),
        (l2.to_degrees() + 540.0).rem_euclid(360.0) - 180.0,
    )
}

/// The eight compass points, for a bearing.
pub fn compass(bearing: f64) -> &'static str {
    ["N", "NE", "E", "SE", "S", "SW", "W", "NW"]
        [((bearing.rem_euclid(360.0) + 22.5) / 45.0) as usize % 8]
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEG_KM: f64 = PI * EARTH_KM / 180.0;

    #[test]
    fn along_a_meridian_and_the_equator_a_degree_is_a_degree() {
        assert!((distance_km((10.0, 20.0), (11.0, 20.0)) - DEG_KM).abs() < 1e-9);
        assert!((distance_km((0.0, 20.0), (0.0, 21.0)) - DEG_KM).abs() < 1e-9);
        assert!((distance_km((-45.0, 0.0), (45.0, 0.0)) - 90.0 * DEG_KM).abs() < 1e-6);
    }

    #[test]
    fn the_antipode_is_half_way_round_from_anywhere() {
        for (lat, lon) in [(47.5, 19.0), (-33.9, 151.2), (0.0, 179.9), (89.0, -10.0)] {
            let anti = (-lat, if lon > 0.0 { lon - 180.0 } else { lon + 180.0 });
            assert!((distance_km((lat, lon), anti) - ANTIPODE_KM).abs() < 1e-6);
        }
    }

    #[test]
    fn distance_is_symmetric_and_zero_to_itself() {
        let (a, b) = ((47.497, 19.04), (35.69, 139.69));
        assert_eq!(distance_km(a, b), distance_km(b, a));
        assert_eq!(distance_km(a, a), 0.0);
    }

    #[test]
    fn bearings_point_where_they_should() {
        assert!((bearing_deg((10.0, 20.0), (30.0, 20.0)) - 0.0).abs() < 1e-9);
        assert!((bearing_deg((10.0, 20.0), (-30.0, 20.0)) - 180.0).abs() < 1e-9);
        assert!((bearing_deg((0.0, 20.0), (0.0, 30.0)) - 90.0).abs() < 1e-9);
        assert!((bearing_deg((0.0, 20.0), (0.0, 10.0)) - 270.0).abs() < 1e-9);
        // Across the antimeridian the short way is east.
        assert!((bearing_deg((0.0, 179.0), (0.0, -179.0)) - 90.0).abs() < 1e-9);
    }

    #[test]
    fn a_bearing_and_a_distance_arrive_where_they_measured_from() {
        let home = (47.497, 19.04);
        for &(lat, lon) in &[
            (35.69, 139.69),
            (-37.8, 144.96),
            (40.7, -74.0),
            (-33.9, 18.4),
            (64.1, -21.9),
        ] {
            let d = distance_km(home, (lat, lon));
            let b = bearing_deg(home, (lat, lon));
            let (lat2, lon2) = destination(home, b, d);
            assert!(
                (lat2 - lat).abs() < 1e-6 && (lon2 - lon).abs() < 1e-6,
                "{lat},{lon} -> {lat2},{lon2}"
            );
        }
    }

    #[test]
    fn compass_points() {
        assert_eq!(compass(0.0), "N");
        assert_eq!(compass(46.0), "NE");
        assert_eq!(compass(359.0), "N");
        assert_eq!(compass(237.0), "SW");
    }
}
