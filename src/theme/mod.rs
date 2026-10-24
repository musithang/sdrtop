// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

mod data;

use ratatui::style::Color;

/// All colors used anywhere in the UI. No panel file hardcodes a Color after Phase 12.
#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    pub name: String,

    // Borders - three tiers of visual weight
    pub border_dim: Color, // log, system_resources, gains (background panels)
    pub border_default: Color, // rf_chain, hardware_health, signal_metrics, iq_*
    pub border_accent: Color, // spectrum, waterfall (primary visual panels)
    pub border_focused: Color, // any panel currently in panel-focus mode

    // Text
    pub label: Color,    // dim labels: "Frequency", "LNA gain"
    pub value: Color,    // normal values
    pub value_hi: Color, // highlighted values: frequency, total gain, board name

    // Status indicators
    pub status_ok: Color,
    pub status_warn: Color,
    pub status_crit: Color,

    // Spectrum & waterfall gradient. Each stop: (t ∈ [0,1], r, g, b).
    // Cold (t=0) is weak signal; hot (t=1) is strong signal.
    pub palette: Vec<(f32, u8, u8, u8)>,
    pub peak_hold: Color,
    pub noise_floor: Color,

    // Misc
    pub stale: Color,    // [STALE] title + dim border when FFT frame is old
    pub observer: Color, // observer mode status dot + accent

    // Decoded protocols, marked over the coexistence heatmap's duty ramp. Chosen
    // to stand off every palette's gradient, so a mark never reads as a duty.
    pub net_ble: Color,
    pub net_bt: Color,
    // Wi-Fi's own ink, wherever its views mark what they found and the
    // header names its channel: apart from both Bluetooth marks, so the two
    // benches never share a colour.
    pub net_wifi: Color,

    // A Wi-Fi frame's format, one colour each, the same on every Wi-Fi
    // view: drawn on the background, never over a gradient. A frame of
    // another channel and one whose signalling did not read are the quiet
    // two, so the eye counts this channel's identified traffic first.
    pub wifi_non_ht: Color,
    pub wifi_ht: Color,
    pub wifi_vht: Color,
    pub wifi_he: Color,
    pub wifi_eht: Color,
    pub wifi_dsss: Color,
    pub wifi_other_channel: Color,
    pub wifi_undecoded: Color,

    // The pager: its case, the case's moulded edge, the bezel round the
    // display, and the display itself in two backlights, day (a green STN
    // glass with dark ink) and night (an amber backlight). `ghost` is an
    // unlit dot, a shade off the glass, as a real character display shows
    // its cells.
    pub pager_case: Color,
    pub pager_edge: Color,
    pub pager_bezel: Color,
    pub lcd_day_glass: Color,
    pub lcd_day_ink: Color,
    pub lcd_day_ghost: Color,
    pub lcd_night_glass: Color,
    pub lcd_night_ink: Color,
    pub lcd_night_ghost: Color,

    // Series: one colour per thing told apart on one plot, in order (the
    // classic hop scatter's piconets). Never empty; a plot with more things
    // than colours reuses them in order, and says so.
    pub series: Vec<Color>,
}

/// Shift a colour ~25% toward a cool steel-blue anchor, leaving 256/16-colour
/// values untouched (only the truecolor path is adjusted). Used by
/// [`Theme::steeled`] to cool the measurement labs' frames.
fn steel_color(c: Color) -> Color {
    match c {
        Color::Rgb(r, g, b) => {
            let mix = |ch: u8, anchor: u8| ((ch as u16 * 3 + anchor as u16) / 4) as u8;
            Color::Rgb(mix(r, 110), mix(g, 125), mix(b, 150))
        }
        other => other,
    }
}

impl Theme {
    /// A Wi-Fi format's colour.
    pub fn wifi(&self, family: crate::state::Family) -> Color {
        use crate::state::Family;
        match family {
            Family::NonHt => self.wifi_non_ht,
            Family::Ht => self.wifi_ht,
            Family::Vht => self.wifi_vht,
            Family::He => self.wifi_he,
            Family::Eht => self.wifi_eht,
            Family::Dsss => self.wifi_dsss,
            Family::Undecoded => self.wifi_undecoded,
        }
    }

    /// The pager display named `display` (one of [`display_names`]), by day
    /// or by night. `None`, or a name not in the list, is the theme's own.
    pub fn display(
        &self,
        display: Option<&str>,
        night: bool,
    ) -> crate::ui::widgets::dotmatrix::Look {
        match display.and_then(|name| displays().iter().find(|d| d.name == name)) {
            Some(d) if night => d.night,
            Some(d) => d.day,
            None => self.lcd(night),
        }
    }

    /// The theme's own pager display, in the day or the night backlight.
    pub fn lcd(&self, night: bool) -> crate::ui::widgets::dotmatrix::Look {
        use crate::ui::widgets::dotmatrix::Look;
        if night {
            Look {
                glass: self.lcd_night_glass,
                ink: self.lcd_night_ink,
                ghost: self.lcd_night_ghost,
            }
        } else {
            Look {
                glass: self.lcd_day_glass,
                ink: self.lcd_day_ink,
                ghost: self.lcd_day_ghost,
            }
        }
    }

    /// The `i`th series colour, wrapping round the list.
    pub fn series_color(&self, i: usize) -> Color {
        self.series
            .get(i % self.series.len().max(1))
            .copied()
            .unwrap_or(self.value_hi)
    }

    /// `c` pushed most of the way toward the theme's `stale` ink: what a plot
    /// draws a thing in when another is selected, so the selection stands out
    /// and the rest still shows where it was. Only the truecolor path mixes; a
    /// 256/16-colour value is returned as it is.
    pub fn receded(&self, c: Color) -> Color {
        match (c, self.stale) {
            (Color::Rgb(r, g, b), Color::Rgb(sr, sg, sb)) => {
                let mix = |ch: u8, to: u8| ((ch as u16 * 35 + to as u16 * 65) / 100) as u8;
                Color::Rgb(mix(r, sr), mix(g, sg), mix(b, sb))
            }
            (other, _) => other,
        }
    }

    /// A copy with the resting border tiers cooled toward steel-blue, for the
    /// measurement labs' "instrument mode". The focus border, text and status
    /// colours are left untouched so focus stays crisp and meaning stays readable.
    pub fn steeled(&self) -> Theme {
        Theme {
            border_dim: steel_color(self.border_dim),
            border_default: steel_color(self.border_default),
            border_accent: steel_color(self.border_accent),
            ..self.clone()
        }
    }
}

/// One of the pager display colours of `displays.toml`.
#[derive(Clone, Debug)]
pub struct Display {
    pub name: String,
    pub day: crate::ui::widgets::dotmatrix::Look,
    pub night: crate::ui::widgets::dotmatrix::Look,
}

/// The pager display colours, read once from `displays.toml`, which is
/// compiled in. Its own test holds every one of them readable, so the
/// `expect`s here cannot fire in a shipped binary.
pub fn displays() -> &'static [Display] {
    use std::sync::OnceLock;
    static DISPLAYS: OnceLock<Vec<Display>> = OnceLock::new();
    DISPLAYS.get_or_init(|| {
        let file: toml::Value =
            toml::from_str(include_str!("displays.toml")).expect("displays.toml parses");
        let look = |t: &toml::Value| {
            let ink = |k: &str| {
                t.get(k)
                    .and_then(toml::Value::as_str)
                    .and_then(Theme::parse_hex)
                    .expect("displays.toml colours are #rrggbb")
            };
            crate::ui::widgets::dotmatrix::Look {
                glass: ink("glass"),
                ink: ink("ink"),
                ghost: ink("ghost"),
            }
        };
        file.get("display")
            .and_then(toml::Value::as_array)
            .expect("displays.toml lists displays")
            .iter()
            .map(|d| Display {
                name: d
                    .get("name")
                    .and_then(toml::Value::as_str)
                    .expect("a display is named")
                    .to_string(),
                day: look(&d["day"]),
                night: look(&d["night"]),
            })
            .collect()
    })
}

/// The names a pager display can be set to, in the order `l` steps through
/// them after the theme's own.
pub fn display_names() -> Vec<&'static str> {
    displays().iter().map(|d| d.name.as_str()).collect()
}

/// The six built-in themes, embedded at compile time.
///
/// They are TOML rather than Rust so that a user can read one and copy it: the
/// file in `~/.config/sdrtop/themes/` has exactly this shape, so "make my own
/// theme" is "copy `sdr.toml` and edit the hex".
const BUILTIN: &[(&str, &str)] = &[
    ("sdr", include_str!("palettes/sdr.toml")),
    ("nord", include_str!("palettes/nord.toml")),
    ("dracula", include_str!("palettes/dracula.toml")),
    ("gruvbox", include_str!("palettes/gruvbox.toml")),
    ("catppuccin", include_str!("palettes/catppuccin.toml")),
    ("solarized", include_str!("palettes/solarized.toml")),
];

/// The name every fallback lands on.
pub const DEFAULT_THEME: &str = "sdr";

impl Theme {
    /// The names of the built-in themes, in the order they are offered.
    pub fn builtin_names() -> impl Iterator<Item = &'static str> {
        BUILTIN.iter().map(|(n, _)| *n)
    }

    /// A built-in theme by name, or `None` if there is no such built-in.
    ///
    /// The `expect` is safe by construction, not by optimism: the text is
    /// `include_str!`-ed at compile time, so it cannot differ between the test
    /// run and the shipped binary, and `every_builtin_theme_parses` parses all
    /// six. A panic here would mean the tests did not run.
    fn builtin(name: &str) -> Option<Self> {
        let (_, text) = BUILTIN.iter().find(|(n, _)| *n == name)?;
        Some(
            data::ThemeFile::parse(text)
                .unwrap_or_else(|e| panic!("built-in theme '{name}' is malformed: {e}"))
                .into_theme()
                .unwrap_or_else(|e| panic!("built-in theme '{name}' is malformed: {e}")),
        )
    }

    /// The default theme.
    ///
    /// Only `sdr` keeps a named constructor, because it is what 61 tests across
    /// the codebase draw against when they need *a* theme rather than a
    /// particular one. The other five are reached by name like a user's own.
    pub fn sdr() -> Self {
        Self::by_name(DEFAULT_THEME)
    }

    /// Return a built-in theme by name. Unknown name -> `sdr` (default).
    pub fn by_name(name: &str) -> Self {
        Self::builtin(name)
            .or_else(|| Self::builtin(DEFAULT_THEME))
            .expect("the default theme must always load")
    }

    /// A theme by name, from `<themes_dir>/<name>.toml` if there is one,
    /// otherwise the built-in of that name.
    ///
    /// The name is not checked against any list, so a file adds a theme as
    /// readily as it replaces one: `tokyonight.toml` makes `tokyonight` a name
    /// `[theme] base` and `--theme` accept, and `nord.toml` replaces the shipped
    /// Nord. Both are the same lookup.
    ///
    /// A user file that will not parse is **reported and skipped**, not fatal:
    /// stderr is redirected to `sdrtop.log` for the session, so the reason is
    /// recoverable, and the alternative is a radio that will not start because of
    /// a stray comma in a colour scheme.
    pub fn load(name: &str, themes_dir: Option<&std::path::Path>) -> Self {
        let Some(dir) = themes_dir else {
            return Self::by_name(name);
        };
        let path = dir.join(format!("{name}.toml"));
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::by_name(name);
        };
        match data::ThemeFile::parse(&text)
            .map_err(|e| e.to_string())
            .and_then(|f| f.into_theme())
        {
            Ok(t) => t,
            Err(e) => {
                eprintln!("Warning: ignoring theme {}: {e}", path.display());
                Self::by_name(name)
            }
        }
    }

    /// Parse a "#rrggbb" hex string into `Color::Rgb`. Returns `None` on invalid input.
    pub fn parse_hex(s: &str) -> Option<Color> {
        let s = s.trim().strip_prefix('#')?;
        if s.len() != 6 {
            return None;
        }
        let r = u8::from_str_radix(&s[0..2], 16).ok()?;
        let g = u8::from_str_radix(&s[2..4], 16).ok()?;
        let b = u8::from_str_radix(&s[4..6], 16).ok()?;
        Some(Color::Rgb(r, g, b))
    }

    /// Interpolate within the theme's gradient palette. `t` ∈ [0.0, 1.0].
    pub fn palette_color(&self, t: f32) -> Color {
        if self.palette.is_empty() {
            return Color::White;
        }
        let t = t.clamp(0.0, 1.0);
        for i in 0..self.palette.len().saturating_sub(1) {
            let (t0, r0, g0, b0) = self.palette[i];
            let (t1, r1, g1, b1) = self.palette[i + 1];
            if t <= t1 {
                let s = if (t1 - t0).abs() < f32::EPSILON {
                    0.0
                } else {
                    (t - t0) / (t1 - t0)
                };
                let lerp = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * s) as u8;
                return Color::Rgb(lerp(r0, r1), lerp(g0, g1), lerp(b0, b1));
            }
        }
        let (_, r, g, b) = *self.palette.last().unwrap();
        Color::Rgb(r, g, b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write `text` to `<tmp>/<name>.toml` and return the directory.
    ///
    /// **One directory per call, not per theme name.** Two tests wrote a good
    /// `tokyonight.toml` and a malformed one into the same path, ran
    /// concurrently, and whichever finished first deleted the directory out from
    /// under the other. It failed about once in fifty runs, which is the worst
    /// rate for a flake to have: often enough to matter, rarely enough to be
    /// blamed on whatever was changed that week.
    fn themes_dir_with(name: &str, text: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "sdrtop-theme-test-{}-{}-{}",
            name,
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{name}.toml")), text).unwrap();
        dir
    }

    #[test]
    fn every_builtin_theme_parses_and_is_complete() {
        // This is what makes `builtin`'s `expect` safe: the text is compiled in,
        // so it cannot differ between here and the shipped binary.
        let names: Vec<&str> = Theme::builtin_names().collect();
        assert_eq!(names.len(), 6, "six built-ins: {names:?}");
        for name in names {
            let t = Theme::builtin(name).unwrap_or_else(|| panic!("{name} did not load"));
            assert_eq!(t.name, name, "a theme file must name itself");
            assert!(t.palette.len() >= 2, "{name}: a gradient needs stops");
            // Every colour resolved: a `#000000` field is legal, an unset one is
            // not, and `into_theme` is what refuses the second case.
            for (field, c) in [
                ("border_accent", t.border_accent),
                ("status_crit", t.status_crit),
                ("value_hi", t.value_hi),
                ("observer", t.observer),
            ] {
                assert!(
                    matches!(c, Color::Rgb(..)),
                    "{name}.{field} is not truecolor"
                );
            }
        }
    }

    /// **The pager's display reads in both backlights, in every theme.** Each
    /// built-in states its own pager colours (not the fallback); the ink
    /// stands well off the glass, and an unlit dot sits just off it: there
    /// to be seen as a cell, never mistaken for a lit dot.
    #[test]
    fn every_pager_display_reads() {
        for name in Theme::builtin_names() {
            let text = BUILTIN.iter().find(|(n, _)| *n == name).unwrap().1;
            let file = data::ThemeFile::parse(text).unwrap();
            assert!(
                file.lcd_day_glass.is_some()
                    && file.lcd_night_ink.is_some()
                    && file.pager_case.is_some(),
                "{name} relies on the pager fallback"
            );
            let t = Theme::builtin(name).unwrap();
            for night in [false, true] {
                let l = t.lcd(night);
                let ink = ciede2000(l.ink, l.glass);
                let ghost = ciede2000(l.ghost, l.glass);
                assert!(
                    ink >= 40.0,
                    "{name} night={night}: ink only {ink:.1} off the glass"
                );
                assert!(
                    (0.8..=6.0).contains(&ghost),
                    "{name} night={night}: a ghost dot {ghost:.1} off the glass"
                );
                assert!(ghost < ink / 5.0, "{name} night={night}");
            }
        }
    }

    /// Every pager display colour reads, by day and by night, as the
    /// themes' own displays must: the ink well off the glass, the unlit dot
    /// just off it.
    #[test]
    fn every_display_colour_reads() {
        let names = display_names();
        assert_eq!(names, ["green", "amber", "blue", "grey", "red"]);
        let t = Theme::sdr();
        for name in names {
            for night in [false, true] {
                let l = t.display(Some(name), night);
                let ink = ciede2000(l.ink, l.glass);
                let ghost = ciede2000(l.ghost, l.glass);
                assert!(ink >= 40.0, "{name} night={night}: ink {ink:.1}");
                assert!(
                    (0.8..=6.0).contains(&ghost),
                    "{name} night={night}: ghost {ghost:.1}"
                );
            }
        }
        assert_eq!(t.display(None, false), t.lcd(false));
        assert_eq!(t.display(Some("no such"), true), t.lcd(true));
    }

    /// **Series colours are told apart at a glance.** Every built-in states
    /// its own six (not the fallback), and no two of them sit closer than 60
    /// (Euclidean, 0-255 RGB): two piconets in near colours would read as one
    /// on the scatter. Measured when chosen: the pastel themes cannot find six
    /// of their own further apart (nord 61, solarized 62, catppuccin and
    /// gruvbox 65, where sdr has 98). Red reads as an alarm, so where a theme
    /// needed its red to reach six (nord, catppuccin) it comes last, the
    /// colour a sixth piconet gets.
    #[test]
    fn every_builtin_series_is_six_colours_told_apart() {
        for name in Theme::builtin_names() {
            let text = BUILTIN.iter().find(|(n, _)| *n == name).unwrap().1;
            let file = data::ThemeFile::parse(text).unwrap();
            assert!(
                file.series.is_some(),
                "{name} leaves series to the fallback"
            );
            let t = Theme::builtin(name).unwrap();
            assert_eq!(t.series.len(), 6, "{name}");
            let rgb = |c: Color| match c {
                Color::Rgb(r, g, b) => (r as f64, g as f64, b as f64),
                _ => panic!("{name}: series colour is not truecolor"),
            };
            for (i, a) in t.series.iter().enumerate() {
                for b in &t.series[i + 1..] {
                    let (a, b) = (rgb(*a), rgb(*b));
                    let d =
                        ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2) + (a.2 - b.2).powi(2)).sqrt();
                    assert!(d >= 60.0, "{name}: two series colours {d:.0} apart");
                }
            }
            assert_eq!(t.series_color(6), t.series[0], "wraps in order");
        }
    }

    /// **A protocol mark never reads as a duty.** Every built-in states its
    /// two mark colours outright (not the fallback), and each sits at least
    /// 80 (Euclidean, 0-255 RGB) from every point of that theme's own
    /// gradient, which is what the coexistence heatmap draws them over.
    /// Measured when chosen: catppuccin's first two picks and nord's first BLE
    /// colour sat 56 to 67 away and were replaced.
    #[test]
    fn the_protocol_marks_stand_off_every_gradient() {
        for name in Theme::builtin_names() {
            let text = BUILTIN.iter().find(|(n, _)| *n == name).unwrap().1;
            let file = data::ThemeFile::parse(text).unwrap();
            assert!(
                file.net_ble.is_some() && file.net_bt.is_some() && file.net_wifi.is_some(),
                "{name}"
            );
            let t = Theme::builtin(name).unwrap();
            for (field, c) in [
                ("net_ble", t.net_ble),
                ("net_bt", t.net_bt),
                ("net_wifi", t.net_wifi),
            ] {
                let Color::Rgb(r, g, b) = c else {
                    panic!("{name}.{field} is not truecolor")
                };
                let nearest = (0..=100)
                    .map(|i| match t.palette_color(i as f32 / 100.0) {
                        Color::Rgb(pr, pg, pb) => {
                            let d = |a: u8, b: u8| (a as f64 - b as f64).powi(2);
                            (d(r, pr) + d(g, pg) + d(b, pb)).sqrt()
                        }
                        _ => f64::INFINITY,
                    })
                    .fold(f64::INFINITY, f64::min);
                assert!(
                    nearest >= 80.0,
                    "{name}.{field} is {nearest:.0} from its gradient"
                );
            }
        }
    }

    /// **Wi-Fi never reads as Bluetooth.** Every built-in's Wi-Fi colour
    /// sits at least 80 from both Bluetooth marks, so a Wi-Fi frame and a
    /// Bluetooth hit drawn side by side are two colours, not two shades.
    #[test]
    fn wifi_stands_off_the_bluetooth_marks() {
        let dist = |a: Color, b: Color| match (a, b) {
            (Color::Rgb(r, g, b), Color::Rgb(pr, pg, pb)) => {
                let d = |a: u8, b: u8| (a as f64 - b as f64).powi(2);
                (d(r, pr) + d(g, pg) + d(b, pb)).sqrt()
            }
            _ => panic!("not truecolor"),
        };
        for name in Theme::builtin_names() {
            let t = Theme::builtin(name).unwrap();
            for (field, mark) in [("net_ble", t.net_ble), ("net_bt", t.net_bt)] {
                let apart = dist(t.net_wifi, mark);
                assert!(apart >= 80.0, "{name}.net_wifi is {apart:.0} from {field}");
            }
        }
    }

    /// A user theme written before the marks existed still loads, and draws
    /// them in its own `border_focused`, `value_hi` and `border_accent`.
    #[test]
    fn a_theme_without_mark_colours_still_loads() {
        let mut text = String::from("name = \"older\"\n");
        for (f, hex) in [
            ("border_dim", "#010101"),
            ("border_default", "#020202"),
            ("border_accent", "#030303"),
            ("border_focused", "#aabbcc"),
            ("label", "#050505"),
            ("value", "#060606"),
            ("value_hi", "#ddeeff"),
            ("status_ok", "#080808"),
            ("status_warn", "#090909"),
            ("status_crit", "#0a0a0a"),
            ("peak_hold", "#0b0b0b"),
            ("noise_floor", "#0c0c0c"),
            ("stale", "#0d0d0d"),
            ("observer", "#0e0e0e"),
        ] {
            text.push_str(&format!("{f} = \"{hex}\"\n"));
        }
        text.push_str(
            "palette = [{ at = 0.0, color = \"#000000\" }, { at = 1.0, color = \"#ffffff\" }]\n",
        );
        let t = data::ThemeFile::parse(&text).unwrap().into_theme().unwrap();
        assert_eq!(t.net_ble, Color::Rgb(0xaa, 0xbb, 0xcc));
        assert_eq!(t.net_bt, Color::Rgb(0xdd, 0xee, 0xff));
        assert_eq!(t.net_wifi, Color::Rgb(3, 3, 3));
        // The Wi-Fi formats: the series in order, the quiet two in the
        // theme's own label and value.
        use crate::state::Family;
        let series: Vec<Color> = t.series.clone();
        for (i, f) in Family::ALL[..6].iter().enumerate() {
            assert_eq!(t.wifi(*f), series[i % series.len()], "{f:?}");
        }
        assert_eq!(t.wifi(Family::Undecoded), t.value);
        assert_eq!(t.wifi_other_channel, t.label);
        // The pager: the theme's own colours, and no ghost dots.
        assert_eq!(t.pager_case, t.stale);
        assert_eq!(t.lcd(false).ink, t.stale);
        assert_eq!(t.lcd(false).ghost, t.lcd(false).glass);
        assert_eq!(t.lcd(true).ink, t.value_hi);
    }

    /// CIEDE2000, the perceptual distance between two colours: what the eye
    /// tells apart, where RGB distance calls two pastels apart that it
    /// does not.
    fn ciede2000(a: Color, b: Color) -> f64 {
        fn lab(c: Color) -> (f64, f64, f64) {
            let Color::Rgb(r, g, b) = c else {
                panic!("not truecolor")
            };
            let lin = |v: u8| {
                let v = v as f64 / 255.0;
                if v <= 0.04045 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            let (r, g, b) = (lin(r), lin(g), lin(b));
            let x = (0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.95047;
            let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
            let z = (0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.08883;
            let f = |t: f64| {
                if t > 0.008856 {
                    t.cbrt()
                } else {
                    7.787 * t + 16.0 / 116.0
                }
            };
            (
                116.0 * f(y) - 16.0,
                500.0 * (f(x) - f(y)),
                200.0 * (f(y) - f(z)),
            )
        }
        let ((l1, a1, b1), (l2, a2, b2)) = (lab(a), lab(b));
        let cb = (a1.hypot(b1) + a2.hypot(b2)) / 2.0;
        let g = 0.5 * (1.0 - (cb.powi(7) / (cb.powi(7) + 25f64.powi(7))).sqrt());
        let (a1, a2) = ((1.0 + g) * a1, (1.0 + g) * a2);
        let (c1, c2) = (a1.hypot(b1), a2.hypot(b2));
        let hue = |b: f64, a: f64| b.atan2(a).to_degrees().rem_euclid(360.0);
        let (h1, h2) = (hue(b1, a1), hue(b2, a2));
        let dl = l2 - l1;
        let dc = c2 - c1;
        let mut dh = h2 - h1;
        if c1 * c2 == 0.0 {
            dh = 0.0;
        } else if dh > 180.0 {
            dh -= 360.0;
        } else if dh < -180.0 {
            dh += 360.0;
        }
        let dhh = 2.0 * (c1 * c2).sqrt() * (dh / 2.0).to_radians().sin();
        let lb = (l1 + l2) / 2.0;
        let cbp = (c1 + c2) / 2.0;
        let hb = if c1 * c2 == 0.0 {
            h1 + h2
        } else if (h1 - h2).abs() <= 180.0 {
            (h1 + h2) / 2.0
        } else if h1 + h2 < 360.0 {
            (h1 + h2 + 360.0) / 2.0
        } else {
            (h1 + h2 - 360.0) / 2.0
        };
        let t = 1.0 - 0.17 * (hb - 30.0).to_radians().cos()
            + 0.24 * (2.0 * hb).to_radians().cos()
            + 0.32 * (3.0 * hb + 6.0).to_radians().cos()
            - 0.20 * (4.0 * hb - 63.0).to_radians().cos();
        let dth = 30.0 * (-((hb - 275.0) / 25.0).powi(2)).exp();
        let rc = 2.0 * (cbp.powi(7) / (cbp.powi(7) + 25f64.powi(7))).sqrt();
        let sl = 1.0 + 0.015 * (lb - 50.0).powi(2) / (20.0 + (lb - 50.0).powi(2)).sqrt();
        let sc = 1.0 + 0.045 * cbp;
        let sh = 1.0 + 0.015 * cbp * t;
        let rt = -(2.0 * dth).to_radians().sin() * rc;
        ((dl / sl).powi(2) + (dc / sc).powi(2) + (dhh / sh).powi(2) + rt * (dc / sc) * (dhh / sh))
            .sqrt()
    }

    /// The helper against two pairs worked elsewhere: black and white 100,
    /// catppuccin's blue and mauve 18.9.
    #[test]
    fn ciede2000_reads_as_published() {
        assert!((ciede2000(Color::Rgb(0, 0, 0), Color::Rgb(255, 255, 255)) - 100.0).abs() < 0.1);
        let d = ciede2000(Color::Rgb(0x89, 0xb4, 0xfa), Color::Rgb(0xcb, 0xa6, 0xf7));
        assert!((d - 18.9).abs() < 0.1, "{d}");
    }

    /// **The Wi-Fi formats are told apart at a glance in every theme.**
    /// Every built-in states all eight; the three that fill the air
    /// (non-HT, HT, DSSS) are at least 20 apart (CIEDE2000), any two of the
    /// six formats at least 12, and none within 12 of the theme's alarm
    /// colour, so no frame reads as an alarm. Measured when chosen: the
    /// closest three 22 (nord), the closest pair 12.3 (gruvbox).
    #[test]
    fn the_wifi_formats_are_told_apart_in_every_theme() {
        use crate::state::Family;
        for name in Theme::builtin_names() {
            let text = BUILTIN.iter().find(|(n, _)| *n == name).unwrap().1;
            let file = data::ThemeFile::parse(text).unwrap();
            assert!(
                [
                    &file.wifi_non_ht,
                    &file.wifi_ht,
                    &file.wifi_vht,
                    &file.wifi_he,
                    &file.wifi_eht,
                    &file.wifi_dsss,
                    &file.wifi_other_channel,
                    &file.wifi_undecoded
                ]
                .iter()
                .all(|f| f.is_some()),
                "{name} leaves a Wi-Fi format to the fallback"
            );
            let t = Theme::builtin(name).unwrap();
            let six: Vec<(Family, Color)> =
                Family::ALL[..6].iter().map(|f| (*f, t.wifi(*f))).collect();
            for (i, (fa, a)) in six.iter().enumerate() {
                assert!(
                    ciede2000(*a, t.status_crit) >= 12.0,
                    "{name}: {fa:?} reads as the alarm"
                );
                for (fb, b) in &six[i + 1..] {
                    let d = ciede2000(*a, *b);
                    let air = |f: &Family| matches!(f, Family::NonHt | Family::Ht | Family::Dsss);
                    let floor = if air(fa) && air(fb) { 20.0 } else { 12.0 };
                    assert!(d >= floor, "{name}: {fa:?} and {fb:?} are {d:.1} apart");
                }
            }
        }
    }

    #[test]
    fn the_sdr_defaults_are_the_ones_users_have_been_looking_at() {
        // Pinned because the six themes moved from Rust into TOML in R6: a
        // transcription slip would repaint the whole deck silently.
        let t = Theme::by_name("sdr");
        assert_eq!(t.border_accent, Color::Rgb(0, 215, 255));
        assert_eq!(t.border_focused, Color::Rgb(255, 255, 255));
        assert_eq!(t.value_hi, Color::Rgb(255, 175, 0));
        assert_eq!(t.status_crit, Color::Rgb(255, 90, 90));
        assert_eq!(t.stale, Color::Rgb(60, 65, 75));
        assert_eq!(t.palette.first(), Some(&(0.00, 10, 10, 80)));
        assert_eq!(t.palette.last(), Some(&(1.00, 255, 50, 20)));
    }

    #[test]
    fn a_user_file_replaces_the_builtin_of_the_same_name() {
        let mut text = String::from("name = \"sdr\"\n");
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
            text.push_str(&format!("{f} = \"#010203\"\n"));
        }
        text.push_str(
            "palette = [{ at = 0.0, color = \"#000000\" }, { at = 1.0, color = \"#ffffff\" }]\n",
        );
        let dir = themes_dir_with("sdr", &text);
        let t = Theme::load("sdr", Some(&dir));
        assert_eq!(
            t.border_accent,
            Color::Rgb(1, 2, 3),
            "the user file should win"
        );
        // And the built-in is untouched for anyone not overriding it.
        assert_eq!(Theme::by_name("sdr").border_accent, Color::Rgb(0, 215, 255));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_malformed_user_theme_falls_back_instead_of_aborting() {
        // A stray comma in a colour scheme must not stop the radio starting.
        let dir = themes_dir_with("nord", "name = \"nord\"\nborder_accent = ");
        assert_eq!(Theme::load("nord", Some(&dir)), Theme::by_name("nord"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_user_file_adds_a_theme_as_readily_as_it_replaces_one() {
        // A name that is not a built-in at all: dropping the file is the whole
        // installation step, and `--theme tokyonight` then works.
        let mut text = String::from("name = \"tokyonight\"\n");
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
            text.push_str(&format!("{f} = \"#7aa2f7\"\n"));
        }
        text.push_str(
            "palette = [{ at = 0.0, color = \"#1a1b26\" }, { at = 1.0, color = \"#f7768e\" }]\n",
        );
        let dir = themes_dir_with("tokyonight", &text);

        let t = Theme::load("tokyonight", Some(&dir));
        assert_eq!(
            t.name, "tokyonight",
            "a brand-new name must load, not fall back"
        );
        assert_eq!(t.border_accent, Color::Rgb(0x7a, 0xa2, 0xf7));
        assert_eq!(t.palette.last(), Some(&(1.0, 0xf7, 0x76, 0x8e)));
        // It is additive: the built-ins are all still reachable.
        assert_eq!(Theme::load("nord", Some(&dir)).name, "nord");
        assert_eq!(Theme::builtin_names().count(), 6);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_malformed_theme_with_a_novel_name_falls_back_to_the_default() {
        // Nothing to fall back *to* by that name, so it lands on `sdr` rather
        // than leaving the app with no theme at all.
        let dir = themes_dir_with("tokyonight", "name = \"tokyonight\"\nborder_accent = ");
        assert_eq!(Theme::load("tokyonight", Some(&dir)).name, "sdr");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unknown_user_theme_still_falls_back_to_sdr() {
        let dir = themes_dir_with("unused", "");
        assert_eq!(Theme::load("no_such_theme", Some(&dir)).name, "sdr");
        assert_eq!(Theme::load("no_such_theme", None).name, "sdr");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn by_name_unknown_falls_back_to_sdr() {
        let t = Theme::by_name("does_not_exist");
        assert_eq!(t.name, "sdr");
    }

    #[test]
    fn by_name_returns_correct_theme() {
        assert_eq!(Theme::by_name("nord").name, "nord");
        assert_eq!(Theme::by_name("dracula").name, "dracula");
        assert_eq!(Theme::by_name("gruvbox").name, "gruvbox");
        assert_eq!(Theme::by_name("catppuccin").name, "catppuccin");
        assert_eq!(Theme::by_name("solarized").name, "solarized");
    }

    #[test]
    fn all_themes_have_non_empty_palette() {
        for name in &[
            "sdr",
            "nord",
            "dracula",
            "gruvbox",
            "catppuccin",
            "solarized",
        ] {
            let t = Theme::by_name(name);
            assert!(!t.palette.is_empty(), "theme '{}' has empty palette", name);
        }
    }

    #[test]
    fn parse_hex_valid_colors() {
        assert_eq!(Theme::parse_hex("#00d7ff"), Some(Color::Rgb(0, 215, 255)));
        assert_eq!(Theme::parse_hex("#88c0d0"), Some(Color::Rgb(136, 192, 208)));
        assert_eq!(Theme::parse_hex("#000000"), Some(Color::Rgb(0, 0, 0)));
        assert_eq!(Theme::parse_hex("#ffffff"), Some(Color::Rgb(255, 255, 255)));
    }

    #[test]
    fn parse_hex_invalid_returns_none() {
        assert_eq!(Theme::parse_hex("00d7ff"), None); // missing #
        assert_eq!(Theme::parse_hex("#gggggg"), None); // invalid hex chars
        assert_eq!(Theme::parse_hex("#fff"), None); // too short
        assert_eq!(Theme::parse_hex(""), None);
    }

    #[test]
    fn palette_color_cold_end() {
        let t = Theme::sdr();
        let c = t.palette_color(0.0);
        assert_eq!(c, Color::Rgb(10, 10, 80));
    }

    #[test]
    fn palette_color_hot_end() {
        let t = Theme::sdr();
        let c = t.palette_color(1.0);
        assert_eq!(c, Color::Rgb(255, 50, 20));
    }

    #[test]
    fn palette_color_clamps_out_of_range() {
        let t = Theme::sdr();
        assert_eq!(t.palette_color(-1.0), t.palette_color(0.0));
        assert_eq!(t.palette_color(2.0), t.palette_color(1.0));
    }
}
