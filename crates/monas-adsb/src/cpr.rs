// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

use std::time::Duration;

use crate::{AirbornePosition, AltitudeSource, CprFormat, Df17Frame, Message};

const CPR_SCALE: i32 = 1 << 17;
const MAX_PAIR_AGE: Duration = Duration::from_secs(10);

/// The position of a frame in a continuous receive epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CprTimestamp {
    pub epoch: u64,
    pub first_pair: u64,
    /// Time from the epoch's beginning, derived from the sample clock.
    pub elapsed: Duration,
}

/// One CPR half from a validated airborne DF17 frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CprObservation {
    icao_address: u32,
    position: AirbornePosition,
    timestamp: CprTimestamp,
}

impl CprObservation {
    /// Extract CPR fields from an airborne-position frame.
    pub fn from_frame(frame: &Df17Frame, timestamp: CprTimestamp) -> Option<Self> {
        if !matches!(frame.type_code(), 9..=18 | 20..=22) {
            return None;
        }
        let Message::AirbornePosition(position) = frame.message() else {
            return None;
        };
        Some(Self {
            icao_address: frame.icao_address(),
            position,
            timestamp,
        })
    }

    pub fn icao_address(&self) -> u32 {
        self.icao_address
    }

    pub fn position(&self) -> &AirbornePosition {
        &self.position
    }

    pub fn timestamp(&self) -> CprTimestamp {
        self.timestamp
    }
}

/// The horizontal position and the two receptions that support it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AirborneFix {
    pub icao_address: u32,
    pub altitude_source: AltitudeSource,
    pub latitude_deg: f64,
    /// Signed longitude in the range [-180, 180).
    pub longitude_deg: f64,
    pub even: CprTimestamp,
    pub odd: CprTimestamp,
    /// Grid of the newer contributing reception.
    pub format: CprFormat,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CprPairError {
    DifferentAircraft,
    DifferentPositionFamily,
    DifferentEpoch,
    SameFormat,
    CoincidentTimestamps,
    Stale,
    LatitudeOutOfRange,
    DifferentLongitudeZones,
}

/// Resolve one pair of airborne CPR observations, independent of input order.
///
/// The newer sample sets the grid. The halves may be up to 10 seconds apart.
/// No observer position is required for this calculation.
/// See the [airborne CPR equations and worked vector](https://mode-s.org/1090mhz/content/ads-b/3-airborne-position.html).
pub fn resolve_airborne_cpr(
    first: CprObservation,
    second: CprObservation,
) -> Result<AirborneFix, CprPairError> {
    if first.icao_address != second.icao_address {
        return Err(CprPairError::DifferentAircraft);
    }
    if first.position.altitude_source != second.position.altitude_source {
        return Err(CprPairError::DifferentPositionFamily);
    }
    if first.timestamp.epoch != second.timestamp.epoch {
        return Err(CprPairError::DifferentEpoch);
    }
    let (even, odd) = match (first.position.cpr_format, second.position.cpr_format) {
        (CprFormat::Even, CprFormat::Odd) => (first, second),
        (CprFormat::Odd, CprFormat::Even) => (second, first),
        _ => return Err(CprPairError::SameFormat),
    };
    let even_time = even.timestamp.elapsed;
    let odd_time = odd.timestamp.elapsed;
    if even_time == odd_time {
        return Err(CprPairError::CoincidentTimestamps);
    }
    if even_time.abs_diff(odd_time) > MAX_PAIR_AGE {
        return Err(CprPairError::Stale);
    }

    let even_y = even.position.cpr_latitude as i32;
    let odd_y = odd.position.cpr_latitude as i32;
    let latitude_index = (59 * even_y - 60 * odd_y + CPR_SCALE / 2).div_euclid(CPR_SCALE);
    let even_latitude = latitude(latitude_index, 60, even_y)?;
    let odd_latitude = latitude(latitude_index, 59, odd_y)?;
    let zones = longitude_zones(even_latitude);
    if zones != longitude_zones(odd_latitude) {
        return Err(CprPairError::DifferentLongitudeZones);
    }

    let even_x = even.position.cpr_longitude as i32;
    let odd_x = odd.position.cpr_longitude as i32;
    let longitude_index =
        ((zones - 1) * even_x - zones * odd_x + CPR_SCALE / 2).div_euclid(CPR_SCALE);
    let (latitude_deg, x, cells, format) = if even_time > odd_time {
        (even_latitude, even_x, zones, CprFormat::Even)
    } else {
        (odd_latitude, odd_x, (zones - 1).max(1), CprFormat::Odd)
    };
    let longitude = grid_degrees(longitude_index, cells, x);
    let longitude_deg = if longitude >= 180.0 {
        longitude - 360.0
    } else {
        longitude
    };
    Ok(AirborneFix {
        icao_address: even.icao_address,
        altitude_source: even.position.altitude_source,
        latitude_deg,
        longitude_deg,
        even: even.timestamp,
        odd: odd.timestamp,
        format,
    })
}

fn grid_degrees(index: i32, cells: i32, code: i32) -> f64 {
    let value = index.rem_euclid(cells) * CPR_SCALE + code;
    360.0 * f64::from(value) / f64::from(cells * CPR_SCALE)
}

fn latitude(index: i32, cells: i32, code: i32) -> Result<f64, CprPairError> {
    let angle = grid_degrees(index, cells, code);
    match angle {
        0.0..=90.0 => Ok(angle),
        270.0.. => Ok(angle - 360.0),
        _ => Err(CprPairError::LatitudeOutOfRange),
    }
}

fn longitude_zones(latitude: f64) -> i32 {
    use std::f64::consts::{PI, TAU};

    let latitude = latitude.abs();
    match latitude {
        0.0 => 59,
        87.0 => 2,
        87.0.. => 1,
        _ => {
            let cosine = latitude.to_radians().cos();
            let angle = (1.0 - (1.0 - (PI / 30.0).cos()) / (cosine * cosine)).acos();
            // The formula tends to 60 at the equator. NL there is 59.
            ((TAU / angle).floor() as i32).clamp(1, 59)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Altitude;

    // https://mode-s.org/1090mhz/content/ads-b/3-airborne-position.html
    const EVEN: [u8; 14] = [
        0x8D, 0x40, 0x62, 0x1D, 0x58, 0xC3, 0x82, 0xD6, 0x90, 0xC8, 0xAC, 0x28, 0x63, 0xA7,
    ];
    const ODD: [u8; 14] = [
        0x8D, 0x40, 0x62, 0x1D, 0x58, 0xC3, 0x86, 0x43, 0x5C, 0xC4, 0x12, 0x69, 0x2A, 0xD6,
    ];

    fn timestamp(seconds: u64) -> CprTimestamp {
        CprTimestamp {
            epoch: 3,
            first_pair: seconds * 10_000_000,
            elapsed: Duration::from_secs(seconds),
        }
    }

    fn published_pair(even_time: u64, odd_time: u64) -> (CprObservation, CprObservation) {
        let even = Df17Frame::try_from(EVEN.as_slice()).unwrap();
        let odd = Df17Frame::try_from(ODD.as_slice()).unwrap();
        (
            CprObservation::from_frame(&even, timestamp(even_time)).unwrap(),
            CprObservation::from_frame(&odd, timestamp(odd_time)).unwrap(),
        )
    }

    fn assert_location(fix: AirborneFix, latitude: f64, longitude: f64, tolerance: f64) {
        assert!((fix.latitude_deg - latitude).abs() <= tolerance, "{fix:?}");
        assert!(
            (fix.longitude_deg - longitude).abs() <= tolerance,
            "{fix:?}"
        );
    }

    #[test]
    fn resolves_published_coordinates_in_either_argument_order() {
        let (even, odd) = published_pair(3, 1);
        let fix = resolve_airborne_cpr(even, odd).unwrap();
        assert_eq!(resolve_airborne_cpr(odd, even), Ok(fix));
        assert_location(fix, 52.257_202_148_437_5, 3.919_372_558_593_75, 1e-10);
        assert_eq!(fix.icao_address, 0x40_62_1D);
        assert_eq!(fix.altitude_source, AltitudeSource::Barometric);
        assert_eq!(fix.format, CprFormat::Even);
        assert_eq!(fix.even, timestamp(3));
        assert_eq!(fix.odd, timestamp(1));
    }

    #[test]
    fn uses_the_odd_grid_when_odd_is_newer() {
        let (even, odd) = published_pair(1, 3);
        let fix = resolve_airborne_cpr(even, odd).unwrap();
        assert_location(fix, 52.265_780_174_126_06, 3.938_912_527_901_786, 1e-10);
        assert_eq!(fix.format, CprFormat::Odd);
    }

    #[test]
    fn includes_ten_seconds_and_rejects_a_later_sample() {
        let (even, mut odd) = published_pair(2, 12);
        assert!(resolve_airborne_cpr(even, odd).is_ok());
        odd.timestamp.first_pair += 1;
        odd.timestamp.elapsed += Duration::from_nanos(100);
        assert_eq!(resolve_airborne_cpr(even, odd), Err(CprPairError::Stale));
    }

    #[test]
    fn rejects_same_grid_and_simultaneous_frames() {
        let (even, odd) = published_pair(3, 3);
        assert_eq!(
            resolve_airborne_cpr(even, even),
            Err(CprPairError::SameFormat)
        );
        assert_eq!(
            resolve_airborne_cpr(even, odd),
            Err(CprPairError::CoincidentTimestamps)
        );
    }

    #[test]
    fn rejects_unrelated_receptions() {
        let (even, odd) = published_pair(2, 3);
        let mut different = odd;
        different.icao_address += 1;
        assert_eq!(
            resolve_airborne_cpr(even, different),
            Err(CprPairError::DifferentAircraft)
        );

        different = odd;
        different.position.altitude_source = AltitudeSource::Geometric;
        assert_eq!(
            resolve_airborne_cpr(even, different),
            Err(CprPairError::DifferentPositionFamily)
        );

        different = odd;
        different.timestamp.epoch += 1;
        assert_eq!(
            resolve_airborne_cpr(even, different),
            Err(CprPairError::DifferentEpoch)
        );
    }

    // Quantized with the CPR encoder's fractional grid equation.
    fn synthetic_pair(even_xy: (u32, u32), odd_xy: (u32, u32)) -> (CprObservation, CprObservation) {
        let observation = |cpr_format, (cpr_latitude, cpr_longitude), seconds| CprObservation {
            icao_address: 0x40_62_1D,
            position: AirbornePosition {
                altitude_source: AltitudeSource::Barometric,
                altitude: Altitude::Unavailable,
                cpr_format,
                cpr_latitude,
                cpr_longitude,
            },
            timestamp: timestamp(seconds),
        };
        (
            observation(CprFormat::Even, even_xy, 1),
            observation(CprFormat::Odd, odd_xy, 2),
        )
    }

    #[test]
    fn normalizes_the_southern_hemisphere_and_west_longitude() {
        let (even, odd) = synthetic_pair((0, 126_430), (10_923, 39_140));
        assert_location(
            resolve_airborne_cpr(even, odd).unwrap(),
            -30.0,
            -120.25,
            0.000_05,
        );
    }

    #[test]
    fn handles_the_polar_grid_and_wraps_the_antimeridian() {
        let (even, odd) = synthetic_pair((98_304, 65_500), (66_082, 65_500));
        assert_location(resolve_airborne_cpr(even, odd).unwrap(), 88.5, 179.9, 0.002);
        assert_eq!(longitude_zones(0.0), 59);
        assert_eq!(longitude_zones(-87.0), 2);
        assert_eq!(longitude_zones(87.0), 2);
        assert_eq!(longitude_zones(87.000_1), 1);

        let (even, odd) = synthetic_pair((0, 65_536), (0, 0));
        assert_location(resolve_airborne_cpr(even, odd).unwrap(), 0.0, -180.0, 0.0);
    }

    #[test]
    fn rejects_invalid_latitudes_and_a_longitude_zone_transition() {
        let (even, odd) = synthetic_pair((0, 0), (87_381, 0));
        assert_eq!(
            resolve_airborne_cpr(even, odd),
            Err(CprPairError::LatitudeOutOfRange)
        );

        // The two samples straddle the first NL transition near 10.47047°.
        let (even, odd) = synthetic_pair((97_655, 126_703), (93_852, 117_965));
        assert_eq!(
            resolve_airborne_cpr(even, odd),
            Err(CprPairError::DifferentLongitudeZones)
        );
    }

    #[test]
    fn does_not_extract_cpr_from_a_crc_valid_identification_frame() {
        let bytes = [
            0x8D, 0x48, 0x40, 0xD6, 0x20, 0x2C, 0xC3, 0x71, 0xC3, 0x2C, 0xE0, 0x57, 0x60, 0x98,
        ];
        let frame = Df17Frame::try_from(bytes.as_slice()).unwrap();
        assert_eq!(CprObservation::from_frame(&frame, timestamp(1)), None);
    }
}
