// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! A theme as it is *written down*: hex strings and gradient stops.
//!
//! This is the shape of `theme/palettes/*.toml` and of anything a user drops in
//! `~/.config/sdrtop/themes/`. [`Theme`] is the shape the renderer
//! wants - parsed `Color`s - and [`ThemeFile::into_theme`] is the one crossing
//! between them.
//!
//! Every colour is required. A theme missing `status_crit` would render a fault
//! in whatever colour happened to be left over, which is worse than refusing to
//! load, so the deserializer rejects it and the caller falls back to a theme that
//! is complete.

use serde::Deserialize;

use super::Theme;

/// One stop of the spectrum / waterfall gradient.
#[derive(Deserialize, Clone, Debug)]
pub struct Stop {
    /// Position in the gradient, 0.0 (weakest) to 1.0 (strongest).
    pub at: f32,
    pub color: String,
}

/// A whole theme, straight out of TOML.
#[derive(Deserialize, Clone, Debug)]
pub struct ThemeFile {
    pub name: String,

    pub border_dim: String,
    pub border_default: String,
    pub border_accent: String,
    pub border_focused: String,

    pub label: String,
    pub value: String,
    pub value_hi: String,

    pub status_ok: String,
    pub status_warn: String,
    pub status_crit: String,

    pub peak_hold: String,
    pub noise_floor: String,
    pub stale: String,
    pub observer: String,

    /// The decoded-protocol marks. Optional, because a user theme written
    /// before they existed must still load: absent, they are drawn in the
    /// theme's own `border_focused` and `value_hi`, which every theme already
    /// sets to stand out.
    #[serde(default)]
    pub net_ble: Option<String>,
    #[serde(default)]
    pub net_bt: Option<String>,
    /// Wi-Fi's colour, optional for the same reason: absent, it takes the
    /// theme's own `border_accent`.
    #[serde(default)]
    pub net_wifi: Option<String>,
    /// The Wi-Fi formats' colours. Optional for the same reason: absent,
    /// the six formats take the series in order, a frame of another channel
    /// the theme's `label` and an undecoded one its `value`.
    #[serde(default)]
    pub wifi_non_ht: Option<String>,
    #[serde(default)]
    pub wifi_ht: Option<String>,
    #[serde(default)]
    pub wifi_vht: Option<String>,
    #[serde(default)]
    pub wifi_he: Option<String>,
    #[serde(default)]
    pub wifi_eht: Option<String>,
    #[serde(default)]
    pub wifi_dsss: Option<String>,
    #[serde(default)]
    pub wifi_other_channel: Option<String>,
    #[serde(default)]
    pub wifi_undecoded: Option<String>,
    /// The pager's colours. Optional for the same reason: absent, they are
    /// the theme's own (the case and bezel its `stale`, the edge its
    /// `border_dim`; the day display its `value` glass and `stale` ink, the
    /// night its `stale` glass and `value_hi` ink; no ghost dots, each ghost
    /// its glass).
    #[serde(default)]
    pub pager_case: Option<String>,
    #[serde(default)]
    pub pager_edge: Option<String>,
    #[serde(default)]
    pub pager_bezel: Option<String>,
    #[serde(default)]
    pub lcd_day_glass: Option<String>,
    #[serde(default)]
    pub lcd_day_ink: Option<String>,
    #[serde(default)]
    pub lcd_day_ghost: Option<String>,
    #[serde(default)]
    pub lcd_night_glass: Option<String>,
    #[serde(default)]
    pub lcd_night_ink: Option<String>,
    #[serde(default)]
    pub lcd_night_ghost: Option<String>,
    /// The FT8 map's colours. Optional for the same reason: absent, the
    /// land by day is the theme's `border_default`, by night its `stale`,
    /// the grey line its `border_accent`, the rings and grid its `border_dim`.
    #[serde(default)]
    pub map_land: Option<String>,
    #[serde(default)]
    pub map_night: Option<String>,
    #[serde(default)]
    pub map_greyline: Option<String>,
    #[serde(default)]
    pub map_grid: Option<String>,
    /// You, at the map's middle; absent, the theme's `status_ok`.
    #[serde(default)]
    pub map_home: Option<String>,
    /// The series colours. Optional for the same reason: absent, a theme's
    /// own accents stand in, in an order that keeps neighbours apart.
    #[serde(default)]
    pub series: Option<Vec<String>>,

    pub palette: Vec<Stop>,
}

impl ThemeFile {
    /// Parse TOML into a theme file, naming what went wrong.
    pub fn parse(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }

    /// Turn the written form into the rendered one.
    ///
    /// `Err` names the first field whose hex would not parse. A theme with one
    /// bad colour is not usable at 13/14 strength: the missing one would fall
    /// back to some other theme's value and the result would be a blend nobody
    /// chose.
    pub fn into_theme(self) -> Result<Theme, String> {
        macro_rules! hex {
            ($field:ident) => {
                Theme::parse_hex(&self.$field).ok_or_else(|| {
                    format!(
                        "{}: '{}' is not a #rrggbb colour",
                        stringify!($field),
                        self.$field
                    )
                })?
            };
        }
        let series = match &self.series {
            Some(list) if !list.is_empty() => list
                .iter()
                .enumerate()
                .map(|(i, hex)| {
                    Theme::parse_hex(hex)
                        .ok_or_else(|| format!("series {i}: '{hex}' is not a #rrggbb colour"))
                })
                .collect::<Result<Vec<_>, _>>()?,
            Some(_) => return Err("series: the list is empty".to_string()),
            None => vec![
                hex!(border_accent),
                hex!(value_hi),
                hex!(status_ok),
                hex!(observer),
                hex!(status_crit),
            ],
        };
        // A Wi-Fi format's colour: the theme's own, else the fallback named.
        let ink = |field: &str, hex: &Option<String>, fallback: ratatui::style::Color| match hex {
            Some(hex) => Theme::parse_hex(hex)
                .ok_or_else(|| format!("{field}: '{hex}' is not a #rrggbb colour")),
            None => Ok(fallback),
        };
        let nth = |i: usize| series[i % series.len()];
        let theme = Theme {
            border_dim: hex!(border_dim),
            border_default: hex!(border_default),
            border_accent: hex!(border_accent),
            border_focused: hex!(border_focused),
            label: hex!(label),
            value: hex!(value),
            value_hi: hex!(value_hi),
            status_ok: hex!(status_ok),
            status_warn: hex!(status_warn),
            status_crit: hex!(status_crit),
            peak_hold: hex!(peak_hold),
            noise_floor: hex!(noise_floor),
            stale: hex!(stale),
            observer: hex!(observer),
            net_ble: match &self.net_ble {
                Some(hex) => Theme::parse_hex(hex)
                    .ok_or_else(|| format!("net_ble: '{hex}' is not a #rrggbb colour"))?,
                None => hex!(border_focused),
            },
            net_bt: match &self.net_bt {
                Some(hex) => Theme::parse_hex(hex)
                    .ok_or_else(|| format!("net_bt: '{hex}' is not a #rrggbb colour"))?,
                None => hex!(value_hi),
            },
            net_wifi: match &self.net_wifi {
                Some(hex) => Theme::parse_hex(hex)
                    .ok_or_else(|| format!("net_wifi: '{hex}' is not a #rrggbb colour"))?,
                None => hex!(border_accent),
            },
            series: series.clone(),
            wifi_non_ht: ink("wifi_non_ht", &self.wifi_non_ht, nth(0))?,
            wifi_ht: ink("wifi_ht", &self.wifi_ht, nth(1))?,
            wifi_vht: ink("wifi_vht", &self.wifi_vht, nth(2))?,
            wifi_he: ink("wifi_he", &self.wifi_he, nth(3))?,
            wifi_eht: ink("wifi_eht", &self.wifi_eht, nth(4))?,
            wifi_dsss: ink("wifi_dsss", &self.wifi_dsss, nth(5))?,
            wifi_other_channel: ink("wifi_other_channel", &self.wifi_other_channel, hex!(label))?,
            wifi_undecoded: ink("wifi_undecoded", &self.wifi_undecoded, hex!(value))?,
            pager_case: ink("pager_case", &self.pager_case, hex!(stale))?,
            pager_edge: ink("pager_edge", &self.pager_edge, hex!(border_dim))?,
            pager_bezel: ink("pager_bezel", &self.pager_bezel, hex!(stale))?,
            lcd_day_glass: ink("lcd_day_glass", &self.lcd_day_glass, hex!(value))?,
            lcd_day_ink: ink("lcd_day_ink", &self.lcd_day_ink, hex!(stale))?,
            lcd_day_ghost: ink("lcd_day_ghost", &self.lcd_day_ghost, hex!(value))?,
            lcd_night_glass: ink("lcd_night_glass", &self.lcd_night_glass, hex!(stale))?,
            lcd_night_ink: ink("lcd_night_ink", &self.lcd_night_ink, hex!(value_hi))?,
            lcd_night_ghost: ink("lcd_night_ghost", &self.lcd_night_ghost, hex!(stale))?,
            map_land: ink("map_land", &self.map_land, hex!(border_default))?,
            map_night: ink("map_night", &self.map_night, hex!(stale))?,
            map_greyline: ink("map_greyline", &self.map_greyline, hex!(border_accent))?,
            map_grid: ink("map_grid", &self.map_grid, hex!(border_dim))?,
            map_home: ink("map_home", &self.map_home, hex!(status_ok))?,

            palette: self
                .palette
                .iter()
                .map(|s| match Theme::parse_hex(&s.color) {
                    Some(ratatui::style::Color::Rgb(r, g, b)) => Ok((s.at, r, g, b)),
                    _ => Err(format!(
                        "palette stop at {}: '{}' is not a #rrggbb colour",
                        s.at, s.color
                    )),
                })
                .collect::<Result<Vec<_>, _>>()?,
            name: self.name,
        };
        if theme.palette.len() < 2 {
            return Err(format!(
                "{}: a gradient needs at least two stops",
                theme.name
            ));
        }
        Ok(theme)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A complete, valid theme file, for tests that want to break exactly one thing.
    fn good() -> String {
        let mut t = String::from("name = \"test\"\n");
        for f in [
            "border_dim",
            "border_default",
            "border_accent",
            "border_focused",
            "label",
            "value",
            "value_hi",
            "status_ok",
            "status_warn",
            "status_crit",
            "peak_hold",
            "noise_floor",
            "stale",
            "observer",
        ] {
            t.push_str(&format!("{f} = \"#102030\"\n"));
        }
        t.push_str(
            "palette = [{ at = 0.0, color = \"#000000\" }, { at = 1.0, color = \"#ffffff\" }]\n",
        );
        t
    }

    #[test]
    fn a_complete_file_becomes_a_theme() {
        let t = ThemeFile::parse(&good()).unwrap().into_theme().unwrap();
        assert_eq!(t.name, "test");
        assert_eq!(
            t.border_accent,
            ratatui::style::Color::Rgb(0x10, 0x20, 0x30)
        );
        assert_eq!(t.palette, vec![(0.0, 0, 0, 0), (1.0, 255, 255, 255)]);
    }

    #[test]
    fn a_missing_colour_is_refused_rather_than_defaulted() {
        // 13 of 14 is not a theme: the missing one would come from somewhere else
        // and the result would be a blend nobody chose.
        let broken = good().replace("status_crit = \"#102030\"\n", "");
        let err = ThemeFile::parse(&broken).unwrap_err().to_string();
        assert!(
            err.contains("status_crit"),
            "the error should name the field: {err}"
        );
    }

    #[test]
    fn a_bad_hex_names_the_field_it_was_in() {
        let broken = good().replace("value_hi = \"#102030\"", "value_hi = \"tomato\"");
        let err = ThemeFile::parse(&broken).unwrap().into_theme().unwrap_err();
        assert!(err.contains("value_hi") && err.contains("tomato"), "{err}");
    }

    #[test]
    fn a_bad_palette_stop_names_its_position() {
        let broken = good().replace("color = \"#ffffff\" }]", "color = \"#fff\" }]");
        let err = ThemeFile::parse(&broken).unwrap().into_theme().unwrap_err();
        assert!(err.contains("palette stop at 1"), "{err}");
    }

    #[test]
    fn a_one_stop_gradient_is_refused() {
        // `palette_color` interpolates between stops; one stop is not a gradient
        // and would make every signal level the same colour.
        let broken = good().replace(
            "palette = [{ at = 0.0, color = \"#000000\" }, { at = 1.0, color = \"#ffffff\" }]",
            "palette = [{ at = 0.0, color = \"#000000\" }]",
        );
        let err = ThemeFile::parse(&broken).unwrap().into_theme().unwrap_err();
        assert!(err.contains("two stops"), "{err}");
    }
}
