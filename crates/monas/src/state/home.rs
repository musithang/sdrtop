// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Where you listen from, and your call: `[station]` in the config, checked.
//!
//! Held apart from any radio's state because it is not a fact about the
//! radio: it follows you to every one.

use crate::config::StationSettings;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Home {
    /// Degrees north. Kept on its own so the two can be entered one at a
    /// time; a position needs both ([`Home::position`]).
    pub latitude: Option<f64>,
    /// Degrees east.
    pub longitude: Option<f64>,
    /// Upper case, of the characters a call is made of.
    pub callsign: Option<String>,
}

/// The settings the Options pane edits, in the order it lists them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StationField {
    Latitude,
    Longitude,
    Callsign,
}

impl StationField {
    pub const ALL: [StationField; 3] = [
        StationField::Latitude,
        StationField::Longitude,
        StationField::Callsign,
    ];

    pub fn label(self) -> &'static str {
        match self {
            StationField::Latitude => "Latitude",
            StationField::Longitude => "Longitude",
            StationField::Callsign => "Callsign",
        }
    }

    /// What may be typed into it.
    pub fn accepts(self, c: char) -> bool {
        match self {
            StationField::Latitude | StationField::Longitude => {
                c.is_ascii_digit() || c == '.' || c == '-'
            }
            StationField::Callsign => c.is_ascii_alphanumeric() || c == '/',
        }
    }
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
    /// Where you listen from: both coordinates, or none.
    pub fn position(&self) -> Option<(f64, f64)> {
        self.latitude.zip(self.longitude)
    }

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
        if s.latitude.is_some() != s.longitude.is_some() {
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
            latitude: lat,
            longitude: lon,
            callsign,
        };
        (home, said)
    }

    /// A field's value as the pane shows it, or `None` when unset.
    pub fn shown(&self, field: StationField) -> Option<String> {
        match field {
            StationField::Latitude => self.latitude.map(|v| format!("{v:.3}")),
            StationField::Longitude => self.longitude.map(|v| format!("{v:.3}")),
            StationField::Callsign => self.callsign.clone(),
        }
    }

    /// Set a field from what was typed: empty clears it. On a value that is
    /// not one, the old stays and the reason comes back; on success, a line
    /// for the log.
    pub fn set(&mut self, field: StationField, typed: &str) -> Result<String, String> {
        let typed = typed.trim();
        let label = field.label();
        if typed.is_empty() {
            match field {
                StationField::Latitude => self.latitude = None,
                StationField::Longitude => self.longitude = None,
                StationField::Callsign => self.callsign = None,
            }
            return Ok(format!("Station {}: cleared", label.to_lowercase()));
        }
        match field {
            StationField::Latitude | StationField::Longitude => {
                let v: f64 = typed
                    .parse()
                    .map_err(|_| format!("{typed:?} is not a number"))?;
                let (ok, range) = if field == StationField::Latitude {
                    (Self::latitude(v), "-90 to 90")
                } else {
                    (Self::longitude(v), "-180 to 180")
                };
                let v = ok.ok_or_else(|| format!("{label} runs {range}"))?;
                if field == StationField::Latitude {
                    self.latitude = Some(v);
                } else {
                    self.longitude = Some(v);
                }
                Ok(format!("Station {} set to {v}", label.to_lowercase()))
            }
            StationField::Callsign => {
                let c = Self::call(typed)
                    .ok_or_else(|| "a call is letters, digits and /, at most 11".to_string())?;
                let line = format!("Station callsign set to {c}");
                self.callsign = Some(c);
                Ok(line)
            }
        }
    }

    /// Back to the settings, for the save.
    pub fn settings(&self) -> StationSettings {
        StationSettings {
            latitude: self.latitude,
            longitude: self.longitude,
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
        assert_eq!(home.position(), Some((47.497, 19.040)));
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
            assert_eq!(home.position(), None, "{s:?}");
            assert_eq!(said.len(), 1, "{s:?}: {said:?}");
        }
        // The poles and the antimeridian are places.
        let (home, _) = Home::from_settings(&settings(Some(-90.0), Some(180.0), None));
        assert_eq!(home.position(), Some((-90.0, 180.0)));
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

    #[test]
    fn typing_sets_a_field_and_a_bad_value_keeps_the_old() {
        let mut home = Home::default();
        assert!(home.set(StationField::Latitude, "47.497").is_ok());
        assert_eq!(home.position(), None, "half a position is no position");
        assert!(home.set(StationField::Longitude, " 19.04 ").is_ok());
        assert_eq!(home.position(), Some((47.497, 19.04)));
        assert_eq!(
            home.set(StationField::Latitude, "95"),
            Err("Latitude runs -90 to 90".into())
        );
        assert!(home.set(StationField::Longitude, "1.2.3").is_err());
        assert_eq!(home.position(), Some((47.497, 19.04)));
        assert!(home.set(StationField::Callsign, "ha5zzt").is_ok());
        assert_eq!(
            home.shown(StationField::Callsign).as_deref(),
            Some("HA5ZZT")
        );
        assert!(home.set(StationField::Callsign, "HA5/ZZT/QRP/X").is_err());
        assert_eq!(home.callsign.as_deref(), Some("HA5ZZT"));
        assert!(home.set(StationField::Callsign, "").is_ok());
        assert_eq!(home.callsign, None);
        assert_eq!(
            home.shown(StationField::Latitude).as_deref(),
            Some("47.497")
        );
    }

    #[test]
    fn each_field_takes_only_its_characters() {
        assert!(StationField::Latitude.accepts('-') && StationField::Latitude.accepts('.'));
        assert!(!StationField::Latitude.accepts('N'));
        assert!(StationField::Callsign.accepts('/') && StationField::Callsign.accepts('k'));
        assert!(!StationField::Callsign.accepts('.'));
    }
}
