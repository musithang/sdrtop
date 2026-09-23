// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! A theme as it is *written down*: hex strings and gradient stops.
//!
//! This is the shape of `theme/palettes/*.toml` and of anything a user drops in
//! `~/.config/sdrtop/themes/`. [`Theme`](super::Theme) is the shape the renderer
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
            series: match &self.series {
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
            },
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
