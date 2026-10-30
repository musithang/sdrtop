// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Where you listen from, and your call: `[station]` in the config, checked.
//!
//! Held apart from any radio's state because it is not a fact about the
//! radio: it follows you to every one.

use crate::config::StationSettings;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Home {
    /// Degrees north and east, both or neither.
    pub position: Option<(f64, f64)>,
    /// Upper case, of the characters a call is made of.
    pub callsign: Option<String>,
}

/// The characters a call may hold (the FT8 call alphabet, which every
/// amateur call fits).
fn is_call(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 11
        && s.bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'/')
}

impl Home {
    /// A latitude, if it is one.
    pub fn latitude(v: f64) -> Option<f64> {
        (v.is_finite() && (-90.0..=90.0).contains(&v)).then_some(v)
    }

    /// A longitude, if it is one.
    pub fn longitude(v: f64) -> Option<f64> {
        (v.is_finite() && (-180.0..=180.0).contains(&v)).then_some(v)
    }

    /// A call, upper-cased and trimmed, if it is one.
    pub fn call(s: &str) -> Option<String> {
        let c = s.trim().to_ascii_uppercase();
        is_call(&c).then_some(c)
    }

    /// The settings checked: what passes, and a sentence for each that does
    /// not.
    pub fn from_settings(s: &StationSettings) -> (Self, Vec<String>) {
        let mut said = Vec::new();
        let lat = s.latitude.and_then(|v| {
            let ok = Self::latitude(v);
            if ok.is_none() {
                said.push(format!(
                    "[station] latitude {v} is not one (-90 to 90): no position"
                ));
            }
            ok
        });
        let lon = s.longitude.and_then(|v| {
            let ok = Self::longitude(v);
            if ok.is_none() {
                said.push(format!(
                    "[station] longitude {v} is not one (-180 to 180): no position"
                ));
            }
            ok
        });
        if lat.is_some() != lon.is_some() && s.latitude.is_some() != s.longitude.is_some() {
            said.push("[station] needs both latitude and longitude: no position".to_string());
        }
        let callsign = s.callsign.as_deref().and_then(|c| {
            let ok = Self::call(c);
            if ok.is_none() {
                said.push(format!("[station] callsign {c:?} is not a call: none"));
            }
            ok
        });
        let home = Self {
            position: lat.zip(lon),
            callsign,
        };
        (home, said)
    }

    /// Back to the settings, for the save.
    pub fn settings(&self) -> StationSettings {
        StationSettings {
            latitude: self.position.map(|p| p.0),
            longitude: self.position.map(|p| p.1),
            callsign: self.callsign.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(lat: Option<f64>, lon: Option<f64>, call: Option<&str>) -> StationSettings {
        StationSettings {
            latitude: lat,
            longitude: lon,
            callsign: call.map(str::to_string),
        }
    }

    #[test]
    fn a_good_station_passes_and_round_trips() {
        let s = settings(Some(47.497), Some(19.040), Some(" ha5zzt "));
        let (home, said) = Home::from_settings(&s);
        assert!(said.is_empty());
        assert_eq!(home.position, Some((47.497, 19.040)));
        assert_eq!(home.callsign.as_deref(), Some("HA5ZZT"));
        assert_eq!(
            home.settings(),
            settings(Some(47.497), Some(19.040), Some("HA5ZZT"))
        );
    }

    #[test]
    fn out_of_range_or_half_a_position_is_no_position_and_is_said() {
        for s in [
            settings(Some(91.0), Some(19.0), None),
            settings(Some(47.0), Some(-181.0), None),
            settings(Some(f64::NAN), Some(19.0), None),
            settings(Some(47.0), None, None),
        ] {
            let (home, said) = Home::from_settings(&s);
            assert_eq!(home.position, None, "{s:?}");
            assert_eq!(said.len(), 1, "{s:?}: {said:?}");
        }
        // The poles and the antimeridian are places.
        let (home, _) = Home::from_settings(&settings(Some(-90.0), Some(180.0), None));
        assert_eq!(home.position, Some((-90.0, 180.0)));
    }

    #[test]
    fn a_call_is_made_of_call_characters() {
        assert_eq!(Home::call("pj4/k1abc").as_deref(), Some("PJ4/K1ABC"));
        for bad in ["", "HA5 ZZT", "HA5ZZT!", "ABCDEFGHIJKL"] {
            assert_eq!(Home::call(bad), None, "{bad}");
        }
        let (home, said) = Home::from_settings(&settings(None, None, Some("HÁ5ZZT")));
        assert_eq!(home.callsign, None);
        assert_eq!(said.len(), 1);
    }
}
