// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

use crate::Df17Frame;

/// The interpretation of a DF17 message field.
#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    Identification(Identification),
    AirbornePosition(AirbornePosition),
    AirborneVelocity(AirborneVelocity),
    UnsupportedTypeCode(u8),
    UnsupportedVelocitySubtype(u8),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identification {
    pub callsign: Result<String, CallsignError>,
    /// Interpret this category in the frame's type-code group.
    pub category_code: Option<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallsignError {
    Blank,
    InvalidCharacter { code: u8 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AltitudeSource {
    Barometric,
    Geometric,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Altitude {
    Feet(i32),
    Unavailable,
    /// The 100-foot Gillham code is retained for inspection.
    Gillham {
        code: u16,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CprFormat {
    Even,
    Odd,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AirbornePosition {
    pub altitude_source: AltitudeSource,
    pub altitude: Altitude,
    pub cpr_format: CprFormat,
    /// Encoded CPR latitude, in units of 1/131072 of its grid cell.
    pub cpr_latitude: u32,
    /// Encoded CPR longitude, in units of 1/131072 of its grid cell.
    pub cpr_longitude: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AirborneVelocity {
    pub subtype: u8,
    pub horizontal: HorizontalVelocity,
    pub vertical_rate: Option<VerticalRate>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HorizontalVelocity {
    Ground {
        /// Positive east, in knots.
        east_knots: VelocityValue,
        /// Positive north, in knots.
        north_knots: VelocityValue,
    },
    Air {
        kind: AirspeedKind,
        speed_knots: VelocityValue,
        heading_deg: Option<f64>,
    },
}

impl HorizontalVelocity {
    /// Ground speed in knots and ground track clockwise from true north.
    /// A stationary ground vector has a speed of zero and no track.
    pub fn ground_speed_and_track(self) -> Option<(f64, Option<f64>)> {
        let Self::Ground {
            east_knots: VelocityValue::Nominal(east),
            north_knots: VelocityValue::Nominal(north),
        } = self
        else {
            return None;
        };
        let (east, north) = (f64::from(east), f64::from(north));
        let speed = east.hypot(north);
        let track = (speed > 0.0).then(|| east.atan2(north).to_degrees().rem_euclid(360.0));
        Some((speed, track))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AirspeedKind {
    Indicated,
    True,
}

/// A signed component or speed as encoded by TC19.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VelocityValue {
    Unavailable,
    Nominal(i32),
    /// The all-ones magnitude reports that the value has exceeded the range.
    Saturated {
        negative: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VerticalRate {
    pub source: VerticalRateSource,
    pub feet_per_minute: VelocityValue,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerticalRateSource {
    NonBarometric,
    Barometric,
}

/// The 56 bits of ME, in on-air order. Fields use one-based ME bit numbers.
struct Me(u64);

impl Me {
    fn bits(&self, first: u32, width: u32) -> u32 {
        ((self.0 >> (57 - first - width)) & ((1 << width) - 1)) as u32
    }

    fn flag(&self, bit: u32) -> bool {
        self.bits(bit, 1) != 0
    }
}

pub(crate) fn decode(frame: &Df17Frame) -> Message {
    let mut bytes = [0; 8];
    bytes[1..].copy_from_slice(&frame.bytes()[4..11]);
    let me = Me(u64::from_be_bytes(bytes));
    match frame.type_code() {
        1..=4 => {
            let category_code = me.bits(6, 3) as u8;
            Message::Identification(Identification {
                callsign: callsign(&me),
                category_code: (category_code != 0).then_some(category_code),
            })
        }
        9..=18 => airborne_position(&me, AltitudeSource::Barometric),
        19 => airborne_velocity(&me),
        20..=22 => airborne_position(&me, AltitudeSource::Geometric),
        code => Message::UnsupportedTypeCode(code),
    }
}

fn callsign(me: &Me) -> Result<String, CallsignError> {
    let mut callsign = String::with_capacity(8);
    for first in (9..=51).step_by(6) {
        let code = me.bits(first, 6) as u8;
        let byte = match code {
            1..=26 => b'A' + code - 1,
            32 => b' ',
            48..=57 => b'0' + code - 48,
            _ => return Err(CallsignError::InvalidCharacter { code }),
        };
        callsign.push(char::from(byte));
    }
    callsign.truncate(callsign.trim_end().len());
    if callsign.is_empty() {
        return Err(CallsignError::Blank);
    }
    Ok(callsign)
}

fn airborne_position(me: &Me, altitude_source: AltitudeSource) -> Message {
    // DO-260C §2.2.3.2.3.4.3 applies the same Q-bit coding to both sources.
    let code = me.bits(9, 12) as u16;
    let altitude = match code {
        0 => Altitude::Unavailable,
        code if code & 0x10 == 0 => Altitude::Gillham { code },
        code => {
            let value = ((code & 0x0FE0) >> 1) | (code & 0x0F);
            Altitude::Feet(i32::from(value) * 25 - 1000)
        }
    };
    Message::AirbornePosition(AirbornePosition {
        altitude_source,
        altitude,
        cpr_format: if me.flag(22) {
            CprFormat::Odd
        } else {
            CprFormat::Even
        },
        cpr_latitude: me.bits(23, 17),
        cpr_longitude: me.bits(40, 17),
    })
}

fn airborne_velocity(me: &Me) -> Message {
    let subtype = me.bits(6, 3) as u8;
    let scale = if matches!(subtype, 2 | 4) { 4 } else { 1 };
    let horizontal = match subtype {
        1 | 2 => HorizontalVelocity::Ground {
            east_knots: velocity_value(me.bits(15, 10), 1023, me.flag(14), scale),
            north_knots: velocity_value(me.bits(26, 10), 1023, me.flag(25), scale),
        },
        3 | 4 => HorizontalVelocity::Air {
            kind: if me.flag(25) {
                AirspeedKind::True
            } else {
                AirspeedKind::Indicated
            },
            speed_knots: velocity_value(me.bits(26, 10), 1023, false, scale),
            heading_deg: me
                .flag(14)
                .then(|| f64::from(me.bits(15, 10)) * 360.0 / 1024.0),
        },
        _ => return Message::UnsupportedVelocitySubtype(subtype),
    };
    let vertical_magnitude = me.bits(38, 9);
    let vertical_rate = (vertical_magnitude != 0).then(|| VerticalRate {
        source: if me.flag(36) {
            VerticalRateSource::Barometric
        } else {
            VerticalRateSource::NonBarometric
        },
        feet_per_minute: velocity_value(vertical_magnitude, 511, me.flag(37), 64),
    });
    Message::AirborneVelocity(AirborneVelocity {
        subtype,
        horizontal,
        vertical_rate,
    })
}

fn velocity_value(code: u32, maximum: u32, negative: bool, scale: i32) -> VelocityValue {
    match code {
        0 => VelocityValue::Unavailable,
        code if code == maximum => VelocityValue::Saturated { negative },
        code => {
            let sign = if negative { -1 } else { 1 };
            VelocityValue::Nominal((code as i32 - 1) * scale * sign)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crc_remainder;

    fn published(hex: &str) -> Message {
        assert_eq!(hex.len(), 28);
        let mut bytes = [0; 14];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&hex[2 * index..2 * index + 2], 16).unwrap();
        }
        Df17Frame::try_from(bytes.as_slice()).unwrap().message()
    }

    // Write test fields one bit at a time, then attach parity for the public parser.
    fn synthetic(fields: &[(usize, usize, u32)]) -> Message {
        let mut bytes = [0; 14];
        bytes[0] = 0x8D;
        for &(first, width, value) in fields {
            assert!(value < 1 << width);
            for offset in 0..width {
                let index = 32 + first - 1 + offset;
                let bit = ((value >> (width - 1 - offset)) & 1) as u8;
                bytes[index / 8] |= bit << (7 - index % 8);
            }
        }
        let parity = crc_remainder(&bytes).to_be_bytes();
        bytes[11..].copy_from_slice(&parity[1..]);
        Df17Frame::try_from(bytes.as_slice()).unwrap().message()
    }

    #[test]
    fn reads_published_callsign_and_zero_category() {
        // https://mode-s.org/1090mhz/content/ads-b/2-identification.html
        assert_eq!(
            published("8D4840D6202CC371C32CE0576098"),
            Message::Identification(Identification {
                callsign: Ok("KLM1023".into()),
                category_code: None,
            })
        );
    }

    #[test]
    fn distinguishes_blank_from_invalid_callsigns() {
        let mut fields = vec![(1, 5, 4), (6, 3, 5)];
        fields.extend((9..=51).step_by(6).map(|start| (start, 6, 32)));
        assert_eq!(
            synthetic(&fields),
            Message::Identification(Identification {
                callsign: Err(CallsignError::Blank),
                category_code: Some(5),
            })
        );
        fields[2].2 = 27;
        assert_eq!(
            synthetic(&fields),
            Message::Identification(Identification {
                callsign: Err(CallsignError::InvalidCharacter { code: 27 }),
                category_code: Some(5),
            })
        );
    }

    #[test]
    fn reads_both_halves_of_the_published_position_pair() {
        // https://mode-s.org/1090mhz/content/ads-b/3-airborne-position.html
        for (hex, cpr_format, cpr_latitude, cpr_longitude) in [
            (
                "8D40621D58C382D690C8AC2863A7",
                CprFormat::Even,
                93000,
                51372,
            ),
            ("8D40621D58C386435CC412692AD6", CprFormat::Odd, 74158, 50194),
        ] {
            assert_eq!(
                published(hex),
                Message::AirbornePosition(AirbornePosition {
                    altitude_source: AltitudeSource::Barometric,
                    altitude: Altitude::Feet(38_000),
                    cpr_format,
                    cpr_latitude,
                    cpr_longitude,
                })
            );
        }
    }

    #[test]
    fn keeps_altitude_sources_zero_and_unsupported_coding_distinct() {
        for (tc, altitude_source) in [
            (9, AltitudeSource::Barometric),
            (20, AltitudeSource::Geometric),
        ] {
            for (code, expected) in [
                (0, Altitude::Unavailable),
                (0x20, Altitude::Gillham { code: 0x20 }),
                (0x10, Altitude::Feet(-1000)),
                (0x58, Altitude::Feet(0)),
                (0xFFF, Altitude::Feet(50_175)),
            ] {
                let Message::AirbornePosition(position) = synthetic(&[(1, 5, tc), (9, 12, code)])
                else {
                    panic!("expected an airborne position");
                };
                assert_eq!(position.altitude_source, altitude_source);
                assert_eq!(position.altitude, expected);
            }
        }
    }

    #[test]
    fn reads_published_ground_velocity_and_true_airspeed() {
        // https://mode-s.org/1090mhz/content/ads-b/5-airborne-velocity.html
        let Message::AirborneVelocity(ground) = published("8D485020994409940838175B284F") else {
            panic!("expected ground velocity");
        };
        assert_eq!(ground.subtype, 1);
        assert_eq!(
            ground.horizontal,
            HorizontalVelocity::Ground {
                east_knots: VelocityValue::Nominal(-8),
                north_knots: VelocityValue::Nominal(-159),
            }
        );
        let (speed, track) = ground.horizontal.ground_speed_and_track().unwrap();
        assert!((speed - 159.201).abs() < 0.001);
        assert!((track.unwrap() - 182.88).abs() < 0.01);
        assert_eq!(
            ground.vertical_rate,
            Some(VerticalRate {
                source: VerticalRateSource::NonBarometric,
                feet_per_minute: VelocityValue::Nominal(-832),
            })
        );

        let Message::AirborneVelocity(air) = published("8DA05F219B06B6AF189400CBC33F") else {
            panic!("expected airspeed");
        };
        assert_eq!(air.subtype, 3);
        assert_eq!(
            air.horizontal,
            HorizontalVelocity::Air {
                kind: AirspeedKind::True,
                speed_knots: VelocityValue::Nominal(375),
                heading_deg: Some(243.984375),
            }
        );
        assert_eq!(
            air.vertical_rate,
            Some(VerticalRate {
                source: VerticalRateSource::Barometric,
                feet_per_minute: VelocityValue::Nominal(-2304),
            })
        );
    }

    #[test]
    fn reads_supersonic_and_saturated_magnitudes() {
        assert_eq!(
            synthetic(&[
                (1, 5, 19),
                (6, 3, 2),
                (15, 10, 3),
                (25, 1, 1),
                (26, 10, 1023)
            ]),
            Message::AirborneVelocity(AirborneVelocity {
                subtype: 2,
                horizontal: HorizontalVelocity::Ground {
                    east_knots: VelocityValue::Nominal(8),
                    north_knots: VelocityValue::Saturated { negative: true },
                },
                vertical_rate: None,
            })
        );
        assert_eq!(
            synthetic(&[
                (1, 5, 19),
                (6, 3, 4),
                (15, 10, 512),
                (26, 10, 4),
                (37, 1, 1),
                (38, 9, 511)
            ]),
            Message::AirborneVelocity(AirborneVelocity {
                subtype: 4,
                horizontal: HorizontalVelocity::Air {
                    kind: AirspeedKind::Indicated,
                    speed_knots: VelocityValue::Nominal(12),
                    heading_deg: None,
                },
                vertical_rate: Some(VerticalRate {
                    source: VerticalRateSource::NonBarometric,
                    feet_per_minute: VelocityValue::Saturated { negative: true },
                }),
            })
        );
    }

    #[test]
    fn distinguishes_zero_speed_from_unavailable_speed() {
        let Message::AirborneVelocity(stationary) =
            synthetic(&[(1, 5, 19), (6, 3, 1), (15, 10, 1), (26, 10, 1), (38, 9, 1)])
        else {
            panic!("expected ground velocity");
        };
        assert_eq!(
            stationary.horizontal.ground_speed_and_track(),
            Some((0.0, None))
        );
        assert_eq!(
            stationary.vertical_rate.unwrap().feet_per_minute,
            VelocityValue::Nominal(0)
        );

        let Message::AirborneVelocity(missing) = synthetic(&[(1, 5, 19), (6, 3, 1), (26, 10, 1)])
        else {
            panic!("expected ground velocity");
        };
        assert_eq!(
            missing.horizontal,
            HorizontalVelocity::Ground {
                east_knots: VelocityValue::Unavailable,
                north_knots: VelocityValue::Nominal(0),
            }
        );
        assert_eq!(missing.horizontal.ground_speed_and_track(), None);
        assert_eq!(missing.vertical_rate, None);
    }

    #[test]
    fn preserves_unsupported_payload_discriminators() {
        assert_eq!(synthetic(&[(1, 5, 5)]), Message::UnsupportedTypeCode(5));
        assert_eq!(
            synthetic(&[(1, 5, 19), (6, 3, 7)]),
            Message::UnsupportedVelocitySubtype(7)
        );
    }
}
