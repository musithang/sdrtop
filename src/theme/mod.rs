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
    fn themes_dir_with(name: &str, text: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("sdrtop-theme-test-{}-{}", name, std::process::id()));
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
