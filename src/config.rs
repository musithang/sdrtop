// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::palette::WaterfallPalette;
use crate::state::{SpectrumMarker, SpectrumStyle, DEFAULT_FREQUENCY, DEFAULT_SAMPLE_RATE};

fn default_frequency_hz() -> u64 {
    DEFAULT_FREQUENCY
}
fn default_sample_rate() -> f64 {
    DEFAULT_SAMPLE_RATE
}
fn default_recall() -> [u64; 3] {
    [0; 3]
}
fn default_active_preset() -> String {
    "command_rail".into()
}
/// Rows of waterfall history kept, and therefore how far `J`/`K` can scroll back.
///
/// Each character cell shows **two** rows (half-block `▀`), so this is twice the
/// visible height of the tallest waterfall plus the scrollback. 64 was the value
/// for years and it is not enough for a full-height waterfall: on the `waterfall`
/// preset a tall terminal ran out of history and left a blank strip above the
/// bottom border that never filled. See [`crate::state::WATERFALL_MIN_ROWS`].
fn default_waterfall_max_rows() -> usize {
    512
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct RadioConfig {
    #[serde(default = "default_frequency_hz")]
    pub frequency_hz: u64,
    #[serde(default = "default_sample_rate")]
    pub sample_rate: f64,
    /// Gain as named stages: `"LNA=28,VGA=12"`.
    ///
    /// The device's own names, in its own units, which is the only form that
    /// works on a radio whose stages are called `IFGR` and `RFGR`. Written on
    /// every save; read in preference to the two fields below.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gain: Option<String>,
    /// Pre-0.5.0 gain, kept **readable and no longer written**.
    ///
    /// `Option` rather than a defaulted number so that a file which never had
    /// them is distinguishable from one that set them to zero, and so a save
    /// stops emitting them. An existing config keeps its gains: these migrate
    /// into the first two stages on load.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lna_gain: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vga_gain: Option<u32>,
    #[serde(default)]
    pub amp_enabled: bool,
    /// Command Rail recall slots (the rail's `M` save / `1·2·3` jump). Three
    /// fixed slots; `0` means empty. Tuning memory belongs with the radio.
    #[serde(default = "default_recall")]
    pub recall_hz: [u64; 3],
}

impl Default for RadioConfig {
    fn default() -> Self {
        Self {
            frequency_hz: DEFAULT_FREQUENCY,
            sample_rate: DEFAULT_SAMPLE_RATE,
            gain: None,
            lna_gain: None,
            vga_gain: None,
            amp_enabled: false,
            recall_hz: [0; 3],
        }
    }
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct DisplayConfig {
    #[serde(default = "default_active_preset")]
    pub active_preset: String,
    #[serde(default = "default_waterfall_max_rows")]
    pub waterfall_max_rows: usize,
    #[serde(default)]
    pub waterfall_palette: WaterfallPalette,
    #[serde(default)]
    pub spectrum_style: SpectrumStyle,
    #[serde(default)]
    pub spectrum_markers: Vec<SpectrumMarker>,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            active_preset: "command_rail".into(),
            waterfall_max_rows: 64,
            waterfall_palette: WaterfallPalette::Classic,
            spectrum_style: SpectrumStyle::Braille,
            spectrum_markers: vec![],
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct ThemeConfig {
    #[serde(default = "ThemeConfig::default_base")]
    pub base: String,
    // Per-field overrides. "#rrggbb" strings. None = use theme default.
    pub border_accent: Option<String>,
    pub border_dim: Option<String>,
    pub border_default: Option<String>,
    pub border_focused: Option<String>,
    pub label: Option<String>,
    pub value: Option<String>,
    pub value_hi: Option<String>,
    pub status_ok: Option<String>,
    pub status_warn: Option<String>,
    pub status_crit: Option<String>,
    pub peak_hold: Option<String>,
    pub noise_floor: Option<String>,
    pub stale: Option<String>,
    pub observer: Option<String>,
}

impl ThemeConfig {
    fn default_base() -> String {
        "sdr".into()
    }
}

fn default_sweep_start() -> u64 {
    400_000_000
}
fn default_sweep_stop() -> u64 {
    500_000_000
}
fn default_sweep_dwell() -> u64 {
    200
}

/// `[sweep]` config for the `lab_sweep` / `micro_sweep` scanner. Read at startup;
/// the dwell can also be nudged live with `+`/`-` in the sweep panel's focus mode.
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct SweepSettings {
    #[serde(default = "default_sweep_start")]
    pub start_hz: u64,
    #[serde(default = "default_sweep_stop")]
    pub stop_hz: u64,
    #[serde(default = "default_sweep_dwell")]
    pub dwell_ms: u64,
}

impl Default for SweepSettings {
    fn default() -> Self {
        Self {
            start_hz: default_sweep_start(),
            stop_hz: default_sweep_stop(),
            dwell_ms: default_sweep_dwell(),
        }
    }
}

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
pub struct AppConfig {
    #[serde(default)]
    pub radio: RadioConfig,
    #[serde(default)]
    pub display: DisplayConfig,
    #[serde(default)]
    pub theme: ThemeConfig,
    #[serde(default)]
    pub sweep: SweepSettings,
    /// User-defined layout presets, merged into the built-in set at startup.
    /// A preset here with the same name as a built-in overrides it. Preserved
    /// verbatim across save so hand-written presets survive a quit.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub presets: HashMap<String, PresetConfig>,
}

impl AppConfig {
    pub fn load_or_default(path: &Path) -> Self {
        let content = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(_) => return Self::default(),
        };
        match toml::from_str(&content) {
            Ok(cfg) => cfg,
            Err(e) => {
                eprintln!(
                    "Warning: failed to parse {}: {e}. Using defaults.",
                    path.display()
                );
                Self::default()
            }
        }
    }

    /// Where a user's own themes live, given the config file's location:
    /// `~/.config/sdrtop/themes/`. `None` when the config path has no parent,
    /// which means there is nowhere to look and only the built-ins exist.
    pub fn themes_dir(config_path: &Path) -> Option<PathBuf> {
        config_path.parent().map(|p| p.join("themes"))
    }

    /// The theme named by `[theme] base`, with any per-field overrides applied.
    ///
    /// `themes_dir` is searched before the built-ins, so a user file named
    /// `sdr.toml` replaces the shipped `sdr` rather than sitting beside it.
    pub fn build_theme(&self, themes_dir: Option<&Path>) -> crate::Theme {
        let mut t = crate::Theme::load(&self.theme.base, themes_dir);
        let tc = &self.theme;
        macro_rules! apply {
            ($field:ident) => {
                if let Some(ref s) = tc.$field {
                    if let Some(c) = crate::Theme::parse_hex(s) {
                        t.$field = c;
                    }
                }
            };
        }
        apply!(border_accent);
        apply!(border_dim);
        apply!(border_default);
        apply!(border_focused);
        apply!(label);
        apply!(value);
        apply!(value_hi);
        apply!(status_ok);
        apply!(status_warn);
        apply!(status_crit);
        apply!(peak_hold);
        apply!(noise_floor);
        apply!(stale);
        apply!(observer);
        t
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self)?;
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, content)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Position {
    Top,
    Bottom,
    Left,
    Right,
    Body,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct PanelSpec {
    pub name: String,
    pub position: Position,
    /// Height in terminal rows - used for Top and Bottom panels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u16>,
    /// Width as a percentage of the body zone - used for Left and Right panels.
    /// All panels in the same column carry the same value; the LayoutEngine
    /// reads only the first panel's value to determine column width.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width_pct: Option<u16>,
}

/// One layout: which panels it draws, and where the menu files it.
///
/// The four menu fields are all optional, so a preset written before the menu
/// existed still loads. `skip_serializing_if` is what keeps `save_config` from
/// writing four empty keys into every `[presets.*]` block it round-trips.
///
/// `Default` is derived for the tests that care only about `panels`: they close
/// their literals with `..Default::default()`, so the next field added here does
/// not break them the way this one did.
#[derive(Deserialize, Serialize, Clone, Debug, Default)]
pub struct PresetConfig {
    pub panels: Vec<PanelSpec>,

    /// Which menu section this layout belongs to, e.g. `"lab"`. Absent means the
    /// Other section. The reserved id `"hidden"` keeps it out of the menu
    /// entirely, which is what `observer` uses: it is given, not chosen.
    ///
    /// Declared here rather than derived from the preset's name because the name
    /// already carries runtime behaviour. `lab_sweep` belongs in the Sweep
    /// section but must keep its `lab_` prefix, which drives both the steel
    /// frame (`UiState::is_lab_mode`) and the sweep task's start and stop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,

    /// The number key inside that section, 1 to 9. Absent means no number key:
    /// the menu still lists the layout, and the cursor still reaches it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot: Option<u8>,

    /// What the menu calls it. Absent means the preset name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// The second line under the title in the menu. Absent means no second line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blurb: Option<String>,
}

/// The sixteen built-in layouts, embedded at compile time.
const BUILTIN_PRESETS: &[(&str, &str)] = &[
    ("spectrum", include_str!("config/presets/spectrum.toml")),
    ("waterfall", include_str!("config/presets/waterfall.toml")),
    (
        "spectrum_waterfall",
        include_str!("config/presets/spectrum_waterfall.toml"),
    ),
    ("observer", include_str!("config/presets/observer.toml")),
    ("main", include_str!("config/presets/main.toml")),
    (
        "command_rail",
        include_str!("config/presets/command_rail.toml"),
    ),
    ("lab_iq", include_str!("config/presets/lab_iq.toml")),
    ("lab_rf", include_str!("config/presets/lab_rf.toml")),
    ("lab_signal", include_str!("config/presets/lab_signal.toml")),
    ("lab_timing", include_str!("config/presets/lab_timing.toml")),
    ("lab_sweep", include_str!("config/presets/lab_sweep.toml")),
    ("micro_main", include_str!("config/presets/micro_main.toml")),
    (
        "micro_signal",
        include_str!("config/presets/micro_signal.toml"),
    ),
    ("micro_gain", include_str!("config/presets/micro_gain.toml")),
    (
        "micro_health",
        include_str!("config/presets/micro_health.toml"),
    ),
    (
        "micro_sweep",
        include_str!("config/presets/micro_sweep.toml"),
    ),
];

/// The layout sdrtop opens on when the config does not say otherwise.
pub const DEFAULT_PRESET: &str = "command_rail";

#[derive(Deserialize, Clone, Debug)]
pub struct LayoutConfig {
    pub active_preset: String,
    pub presets: HashMap<String, PresetConfig>,
}

impl LayoutConfig {
    /// The built-in presets, in the order `[P]` was designed to walk them.
    ///
    /// TOML rather than Rust for the same reason the themes are: a user writing a
    /// layout can read one and copy it, because their own file has exactly this
    /// shape. Adding a built-in is a file plus one line here.
    pub fn default_config() -> Self {
        let presets = BUILTIN_PRESETS
            .iter()
            .map(|(name, text)| (name.to_string(), Self::parse_builtin(name, text)))
            .collect();
        Self {
            active_preset: DEFAULT_PRESET.into(),
            presets,
        }
    }

    /// Parse an embedded preset.
    ///
    /// The `expect` is safe by construction, not optimism: the text is
    /// `include_str!`-ed at compile time, so it cannot differ between the test run
    /// and the shipped binary, and `every_builtin_preset_parses` parses all
    /// sixteen. A panic here would mean the tests did not run.
    fn parse_builtin(name: &str, text: &str) -> PresetConfig {
        toml::from_str(text)
            .unwrap_or_else(|e| panic!("built-in preset '{name}' is malformed: {e}"))
    }

    /// Where a user's own presets live, given the config file's location:
    /// `~/.config/sdrtop/presets/`. `None` when the config path has no parent.
    pub fn presets_dir(config_path: &Path) -> Option<PathBuf> {
        config_path.parent().map(|p| p.join("presets"))
    }

    /// Built-in presets with the user's own merged on top, in increasing order of
    /// deliberateness: built-in, then `<presets_dir>/*.toml`, then the
    /// `[presets.*]` blocks in `config.toml`.
    ///
    /// A name that matches a built-in replaces it; a name that does not is simply
    /// **added**, and so joins the `[P]` cycle automatically. That is the whole
    /// installation step for a layout of your own - there is no list to register
    /// it in.
    ///
    /// `config.toml` wins over a file because it is the one the user edits by hand
    /// and the one sdrtop itself rewrites; if a name is defined in both, the
    /// nearer definition is the one they meant.
    pub fn with_user_presets(
        user: &HashMap<String, PresetConfig>,
        presets_dir: Option<&Path>,
    ) -> Self {
        let mut cfg = Self::default_config();
        for (name, preset) in Self::read_preset_dir(presets_dir) {
            cfg.presets.insert(name, preset);
        }
        for (name, preset) in user {
            cfg.presets.insert(name.clone(), preset.clone());
        }
        cfg
    }

    /// Every `*.toml` in `dir` that parses, named after its file stem.
    ///
    /// A file that will not parse is reported and skipped, never fatal: stderr is
    /// redirected to `sdrtop.log` for the session, and a stray comma in one layout
    /// must not stop the radio from starting or take the other layouts with it.
    fn read_preset_dir(dir: Option<&Path>) -> Vec<(String, PresetConfig)> {
        let Some(dir) = dir else { return Vec::new() };
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut found: Vec<(String, PresetConfig)> = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("toml") {
                continue;
            }
            let Some(name) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            match toml::from_str::<PresetConfig>(&text) {
                Ok(p) if p.panels.is_empty() => eprintln!(
                    "Warning: ignoring preset {}: it lists no panels",
                    path.display()
                ),
                Ok(p) => found.push((name.to_string(), p)),
                Err(e) => eprintln!("Warning: ignoring preset {}: {e}", path.display()),
            }
        }
        // Deterministic order, so two files defining the same name do not depend
        // on the order the filesystem happened to hand them back.
        found.sort_by(|a, b| a.0.cmp(&b.0));
        found
    }

    pub fn active_panels(&self) -> &[PanelSpec] {
        self.presets
            .get(&self.active_preset)
            .map(|p| p.panels.as_slice())
            .unwrap_or(&[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write `files` into a fresh temp directory and return it.
    fn presets_dir_with(tag: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("sdrtop-preset-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (name, body) in files {
            std::fs::write(dir.join(format!("{name}.toml")), body).unwrap();
        }
        dir
    }

    const A_LAYOUT: &str = r#"
panels = [
    { name = "header",   position = "top", height = 5 },
    { name = "spectrum", position = "body" },
    { name = "footer",   position = "bottom" },
]
"#;

    #[test]
    fn every_builtin_preset_parses() {
        // This is what makes `parse_builtin`'s `expect` safe: the text is compiled
        // in, so it cannot differ between here and the shipped binary.
        let cfg = LayoutConfig::default_config();
        assert_eq!(cfg.presets.len(), 16, "sixteen built-ins");
        assert_eq!(cfg.active_preset, DEFAULT_PRESET);
        for (name, preset) in &cfg.presets {
            assert!(!preset.panels.is_empty(), "{name} lists no panels");
        }
        for want in [
            "command_rail",
            "spectrum",
            "waterfall",
            "spectrum_waterfall",
            "observer",
            "main",
            "lab_iq",
            "lab_rf",
            "lab_signal",
            "lab_timing",
            "lab_sweep",
            "micro_main",
            "micro_signal",
            "micro_gain",
            "micro_health",
            "micro_sweep",
        ] {
            assert!(cfg.presets.contains_key(want), "missing built-in '{want}'");
        }
    }

    #[test]
    fn the_command_rail_layout_is_the_one_it_has_always_been() {
        // Pinned because the sixteen presets moved from Rust into TOML in R7: a
        // transcription slip would silently rearrange the default screen.
        let cfg = LayoutConfig::default_config();
        let p = &cfg.presets["command_rail"];
        let got: Vec<(&str, &Position, Option<u16>, Option<u16>)> = p
            .panels
            .iter()
            .map(|s| (s.name.as_str(), &s.position, s.height, s.width_pct))
            .collect();
        assert_eq!(
            got,
            vec![
                ("header_slim", &Position::Top, Some(4), None),
                ("command_rail", &Position::Left, None, Some(28)),
                ("spectrum", &Position::Body, None, None),
                ("waterfall", &Position::Body, None, None),
                ("footer", &Position::Bottom, None, None),
            ]
        );
    }

    #[test]
    fn a_preset_file_adds_a_layout_as_readily_as_it_replaces_one() {
        // The point of the directory: a name nobody has used before is simply
        // added, and joins the [P] cycle. No list to register it in.
        let dir = presets_dir_with("add", &[("nightwatch", A_LAYOUT)]);
        let cfg = LayoutConfig::with_user_presets(&HashMap::new(), Some(&dir));
        assert!(
            cfg.presets.contains_key("nightwatch"),
            "a new name should be added"
        );
        assert_eq!(cfg.presets["nightwatch"].panels.len(), 3);
        assert_eq!(cfg.presets.len(), 17, "added, not replaced");
        // And every built-in is still there.
        assert!(cfg.presets.contains_key("lab_signal"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_preset_file_named_after_a_builtin_replaces_it() {
        let dir = presets_dir_with("replace", &[("lab_iq", A_LAYOUT)]);
        let cfg = LayoutConfig::with_user_presets(&HashMap::new(), Some(&dir));
        assert_eq!(cfg.presets["lab_iq"].panels.len(), 3, "the file should win");
        assert_eq!(cfg.presets.len(), 16, "replaced, not added");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_config_block_outranks_a_file_of_the_same_name() {
        // config.toml is the file the user edits by hand and the one sdrtop
        // rewrites, so a name defined in both resolves to the nearer definition.
        let dir = presets_dir_with("precedence", &[("nightwatch", A_LAYOUT)]);
        let mut inline = HashMap::new();
        inline.insert(
            "nightwatch".to_string(),
            PresetConfig {
                panels: vec![PanelSpec {
                    name: "footer".into(),
                    position: Position::Bottom,
                    height: None,
                    width_pct: None,
                }],
                ..Default::default()
            },
        );
        let cfg = LayoutConfig::with_user_presets(&inline, Some(&dir));
        assert_eq!(
            cfg.presets["nightwatch"].panels.len(),
            1,
            "the config block wins"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_broken_preset_file_is_skipped_without_taking_the_others_with_it() {
        // One stray comma must not cost the user their other layouts, or stop the
        // radio from starting at all.
        let dir = presets_dir_with(
            "broken",
            &[
                ("good", A_LAYOUT),
                ("broken", "panels = [ { name = "),
                ("empty", "panels = []"),
                ("notes", "this is not toml at all"),
            ],
        );
        std::fs::write(dir.join("README.md"), "not a preset").unwrap();
        let cfg = LayoutConfig::with_user_presets(&HashMap::new(), Some(&dir));
        assert!(
            cfg.presets.contains_key("good"),
            "the good file should still load"
        );
        assert!(!cfg.presets.contains_key("broken"));
        assert!(
            !cfg.presets.contains_key("empty"),
            "a layout with no panels is not a layout"
        );
        assert!(!cfg.presets.contains_key("notes"));
        assert!(!cfg.presets.contains_key("README"), "only .toml is read");
        assert_eq!(
            cfg.presets.len(),
            17,
            "sixteen built-ins plus the one good file"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_preset_directory_is_not_an_error() {
        assert_eq!(
            LayoutConfig::with_user_presets(&HashMap::new(), None)
                .presets
                .len(),
            16
        );
        let missing = std::env::temp_dir().join("sdrtop-presets-that-do-not-exist");
        assert_eq!(
            LayoutConfig::with_user_presets(&HashMap::new(), Some(&missing))
                .presets
                .len(),
            16
        );
    }

    #[test]
    fn default_config_has_minimal_preset() {
        let cfg = LayoutConfig::default_config();
        assert_eq!(cfg.active_preset, "command_rail");
        assert!(!cfg.active_panels().is_empty());
    }

    #[test]
    fn active_panels_returns_correct_names() {
        let cfg = LayoutConfig::default_config();
        let names: Vec<&str> = cfg
            .active_panels()
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        // The [1] default is now the Command Rail: slim header + left rail + bond.
        assert!(names.contains(&"header_slim"));
        assert!(names.contains(&"command_rail"));
        assert!(names.contains(&"footer"));
        assert!(names.contains(&"spectrum"));
        assert!(names.contains(&"waterfall"));
    }

    #[test]
    fn deserialize_from_toml() {
        let raw = r#"
            active_preset = "minimal"
            [presets.minimal]
            panels = [
              { name = "header", position = "top", height = 3 },
              { name = "footer", position = "bottom", height = 3 },
            ]
        "#;
        let cfg: LayoutConfig = toml::from_str(raw).unwrap();
        assert_eq!(cfg.active_panels().len(), 2);
    }

    #[test]
    fn default_radio_config_frequency() {
        assert_eq!(RadioConfig::default().frequency_hz, 2_400_000_000);
        assert_eq!(
            RadioConfig::default().gain,
            None,
            "a fresh config names no gain"
        );
    }

    #[test]
    fn load_or_default_missing_file_returns_default() {
        let cfg = AppConfig::load_or_default(Path::new("/nonexistent/sdrtop/config.toml"));
        assert_eq!(cfg.radio.frequency_hz, RadioConfig::default().frequency_hz);
    }

    #[test]
    fn deserialize_partial_toml_fills_missing_with_defaults() {
        let toml_str = "[radio]\nfrequency_hz = 433_000_000\n";
        let cfg: AppConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.radio.frequency_hz, 433_000_000);
        assert_eq!(cfg.display.active_preset, "command_rail");
    }

    #[test]
    fn serialize_deserialize_round_trip() {
        let mut cfg = AppConfig::default();
        cfg.radio.gain = Some("LNA=24,VGA=30".into());
        cfg.display.active_preset = "spectrum".into();
        let serialized = toml::to_string_pretty(&cfg).unwrap();
        let restored: AppConfig = toml::from_str(&serialized).unwrap();
        assert_eq!(restored.radio.gain.as_deref(), Some("LNA=24,VGA=30"));
        assert_eq!(restored.display.active_preset, "spectrum");
    }

    #[test]
    fn spectrum_style_round_trips_and_defaults_braille() {
        let cfg: AppConfig = toml::from_str("[display]\nactive_preset = \"spectrum\"\n").unwrap();
        assert_eq!(cfg.display.spectrum_style, SpectrumStyle::Braille);
        let mut cfg = AppConfig::default();
        cfg.display.spectrum_style = SpectrumStyle::Scatter;
        let serialized = toml::to_string_pretty(&cfg).unwrap();
        assert!(
            serialized.contains("spectrum_style = \"scatter\""),
            "got:\n{serialized}"
        );
        let restored: AppConfig = toml::from_str(&serialized).unwrap();
        assert_eq!(restored.display.spectrum_style, SpectrumStyle::Scatter);
    }

    #[test]
    fn waterfall_palette_round_trips_and_defaults_classic() {
        // Missing key → Classic (the existing look).
        let cfg: AppConfig = toml::from_str("[display]\nactive_preset = \"spectrum\"\n").unwrap();
        assert_eq!(cfg.display.waterfall_palette, WaterfallPalette::Classic);
        // Explicit choice survives a save/load round trip and serializes lowercase.
        let mut cfg = AppConfig::default();
        cfg.display.waterfall_palette = WaterfallPalette::Phosphor;
        let serialized = toml::to_string_pretty(&cfg).unwrap();
        assert!(
            serialized.contains("waterfall_palette = \"phosphor\""),
            "got:\n{serialized}"
        );
        let restored: AppConfig = toml::from_str(&serialized).unwrap();
        assert_eq!(
            restored.display.waterfall_palette,
            WaterfallPalette::Phosphor
        );
    }

    #[test]
    fn radio_recall_round_trips_and_defaults_empty() {
        // Missing key → all-empty slots.
        let cfg: AppConfig = toml::from_str("[radio]\nfrequency_hz = 100_000_000\n").unwrap();
        assert_eq!(cfg.radio.recall_hz, [0, 0, 0]);
        // Explicit slots survive a save/load round trip.
        let mut cfg = AppConfig::default();
        cfg.radio.recall_hz = [92_800_000, 0, 446_006_000];
        let restored: AppConfig = toml::from_str(&toml::to_string_pretty(&cfg).unwrap()).unwrap();
        assert_eq!(restored.radio.recall_hz, [92_800_000, 0, 446_006_000]);
    }

    #[test]
    fn default_config_has_lab_presets() {
        let cfg = LayoutConfig::default_config();
        for name in ["lab_iq", "lab_rf", "lab_signal", "lab_timing"] {
            let p = cfg
                .presets
                .get(name)
                .unwrap_or_else(|| panic!("missing preset {name}"));
            assert!(!p.panels.is_empty(), "{name} has no panels");
            // Every lab preset carries a header and a footer.
            let names: Vec<&str> = p.panels.iter().map(|s| s.name.as_str()).collect();
            assert!(names.contains(&"header"), "{name} missing header");
            assert!(names.contains(&"footer"), "{name} missing footer");
        }
    }

    #[test]
    fn default_config_lab_sweep_has_sweep_panels() {
        let cfg = LayoutConfig::default_config();
        let p = cfg
            .presets
            .get("lab_sweep")
            .expect("lab_sweep preset present");
        let names: Vec<&str> = p.panels.iter().map(|s| s.name.as_str()).collect();
        assert!(
            names.contains(&"sweep_panel"),
            "lab_sweep missing sweep_panel"
        );
        assert!(
            names.contains(&"sweep_strip"),
            "lab_sweep missing sweep_strip"
        );
    }

    #[test]
    fn default_config_lab_signal_has_redesign_panels() {
        // The lab_signal redesign is a three-zone instrument: the characterization
        // rail, the bonded spectrum + waterfall center, and the FM demod column.
        let cfg = LayoutConfig::default_config();
        let p = cfg
            .presets
            .get("lab_signal")
            .expect("lab_signal preset present");
        let names: Vec<&str> = p.panels.iter().map(|s| s.name.as_str()).collect();
        for panel in [
            "signal_characterization",
            "spectrum",
            "waterfall",
            "fm_demod",
        ] {
            assert!(names.contains(&panel), "lab_signal missing {panel}");
        }
        // spectrum + waterfall both live in the Body column so the engine bonds
        // them into one shared-ruler instrument.
        let body: Vec<&str> = p
            .panels
            .iter()
            .filter(|s| matches!(s.position, Position::Body))
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(
            body,
            vec!["spectrum", "waterfall"],
            "bond pair must be the Body column"
        );
        // The bottom lab chrome (marker bar) rides along like the other labs.
        assert!(
            names.contains(&"lab_marker"),
            "lab_signal missing lab_marker"
        );
    }

    #[test]
    fn default_config_lab_timing_has_redesign_panels() {
        // The lab_timing redesign is a three-zone instrument: diagnostics rail,
        // the per-callback strip chart, and the hardware-vitals column.
        let cfg = LayoutConfig::default_config();
        let p = cfg
            .presets
            .get("lab_timing")
            .expect("lab_timing preset present");
        let names: Vec<&str> = p.panels.iter().map(|s| s.name.as_str()).collect();
        for panel in ["timing_diagnostics", "timing_stripchart", "timing_vitals"] {
            assert!(names.contains(&panel), "lab_timing missing {panel}");
        }
        // The bottom lab chrome (marker bar) rides along like the other labs.
        assert!(
            names.contains(&"lab_marker"),
            "lab_timing missing lab_marker"
        );
    }

    #[test]
    fn default_config_has_micro_main() {
        let cfg = LayoutConfig::default_config();
        let p = cfg
            .presets
            .get("micro_main")
            .expect("micro_main preset present");
        let names: Vec<&str> = p.panels.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["micro_panel", "footer"]);
    }

    #[test]
    fn default_config_has_full_micro_cycle() {
        // Every micro view must have a defined preset + its dedicated panel.
        let cfg = LayoutConfig::default_config();
        for (preset, panel) in [
            ("micro_main", "micro_panel"),
            ("micro_signal", "micro_signal_panel"),
            ("micro_gain", "micro_gain_panel"),
            ("micro_health", "micro_health_panel"),
        ] {
            let p = cfg
                .presets
                .get(preset)
                .unwrap_or_else(|| panic!("missing {preset}"));
            assert_eq!(
                p.panels.first().map(|s| s.name.as_str()),
                Some(panel),
                "{preset} body panel"
            );
            assert_eq!(
                p.panels.last().map(|s| s.name.as_str()),
                Some("footer"),
                "{preset} footer"
            );
        }
    }

    #[test]
    fn with_user_presets_adds_new_and_overrides_builtin() {
        let raw = r#"
            [presets.custom]
            panels = [
              { name = "header", position = "top", height = 3 },
              { name = "footer", position = "bottom" },
            ]
            [presets.main]
            panels = [
              { name = "spectrum", position = "body" },
            ]
        "#;
        let app: AppConfig = toml::from_str(raw).unwrap();
        let cfg = LayoutConfig::with_user_presets(&app.presets, None);
        // New preset joined the set.
        assert!(cfg.presets.contains_key("custom"));
        // Built-in presets still present.
        assert!(cfg.presets.contains_key("spectrum_waterfall"));
        // User override replaced the built-in "main".
        let main = cfg.presets.get("main").unwrap();
        assert_eq!(main.panels.len(), 1);
        assert_eq!(main.panels[0].name, "spectrum");
    }

    #[test]
    fn app_config_round_trip_preserves_user_presets() {
        let raw = r#"
            [presets.custom]
            panels = [
              { name = "header", position = "top", height = 3 },
              { name = "footer", position = "bottom" },
            ]
        "#;
        let app: AppConfig = toml::from_str(raw).unwrap();
        let serialized = toml::to_string_pretty(&app).unwrap();
        let restored: AppConfig = toml::from_str(&serialized).unwrap();
        let custom = restored
            .presets
            .get("custom")
            .expect("custom preset survives round-trip");
        assert_eq!(custom.panels.len(), 2);
        assert_eq!(custom.panels[0].height, Some(3));
        assert_eq!(custom.panels[1].name, "footer");
    }

    #[test]
    fn app_config_without_presets_omits_section() {
        let app = AppConfig::default();
        let serialized = toml::to_string_pretty(&app).unwrap();
        assert!(
            !serialized.contains("[presets"),
            "empty presets should not emit a section: {serialized}"
        );
    }

    #[test]
    fn per_field_theme_overrides_survive_a_save() {
        // The bug the lint reported from R3a to R6: `App::save_config` rebuilt the
        // theme block as `ThemeConfig { base, ..Default::default() }`, so every
        // per-field colour the user had written was deleted from their config the
        // next time they pressed `q`. The docs carried a warning about it.
        let mut loaded = ThemeConfig {
            base: "nord".into(),
            ..Default::default()
        };
        loaded.border_accent = Some("#ff00ff".into());
        loaded.stale = Some("#101010".into());

        // Exactly what `save_config` writes now: the loaded block, `base` refreshed.
        let written = AppConfig {
            theme: ThemeConfig {
                base: "nord".into(),
                ..loaded.clone()
            },
            ..AppConfig::default()
        };
        let path =
            std::env::temp_dir().join(format!("sdrtop-theme-save-{}.toml", std::process::id()));
        written.save(&path).unwrap();
        let back = AppConfig::load_or_default(&path);
        let _ = std::fs::remove_file(&path);

        assert_eq!(back.theme.base, "nord");
        assert_eq!(
            back.theme.border_accent.as_deref(),
            Some("#ff00ff"),
            "the override was dropped on save again"
        );
        assert_eq!(back.theme.stale.as_deref(), Some("#101010"));

        // And the shape that caused it, so this test cannot pass by accident.
        let old_way = ThemeConfig {
            base: "nord".into(),
            ..Default::default()
        };
        assert!(
            old_way.border_accent.is_none(),
            "spreading Default is what deleted the overrides"
        );
    }

    #[test]
    fn build_theme_prefers_a_user_theme_over_the_builtin() {
        // `~/.config/sdrtop/themes/<base>.toml` replaces the shipped theme of that
        // name, and per-field overrides still apply on top of whichever won.
        let dir = std::env::temp_dir().join(format!("sdrtop-cfg-theme-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut text = String::from("name = \"gruvbox\"\n");
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
            text.push_str(&format!("{f} = \"#020202\"\n"));
        }
        text.push_str(
            "palette = [{ at = 0.0, color = \"#000000\" }, { at = 1.0, color = \"#ffffff\" }]\n",
        );
        std::fs::write(dir.join("gruvbox.toml"), text).unwrap();

        let mut cfg = AppConfig::default();
        cfg.theme.base = "gruvbox".into();
        cfg.theme.value_hi = Some("#abcdef".into());
        let t = cfg.build_theme(Some(&dir));
        assert_eq!(
            t.border_accent,
            ratatui::style::Color::Rgb(2, 2, 2),
            "user file should win"
        );
        assert_eq!(
            t.value_hi,
            ratatui::style::Color::Rgb(0xab, 0xcd, 0xef),
            "an override still applies on top of a user theme"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_theme_default_is_sdr() {
        let cfg = AppConfig::load_or_default(Path::new("/nonexistent/sdrtop/config.toml"));
        let t = cfg.build_theme(None);
        assert_eq!(t.name, "sdr");
    }

    #[test]
    fn build_theme_unknown_base_falls_back_to_sdr() {
        let toml = "[theme]\nbase = \"nonexistent\"\n";
        let cfg: AppConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.build_theme(None).name, "sdr");
    }

    #[test]
    fn build_theme_override_applies_hex_color() {
        let toml = "[theme]\nbase = \"nord\"\nborder_accent = \"#ff0000\"\n";
        let cfg: AppConfig = toml::from_str(toml).unwrap();
        let t = cfg.build_theme(None);
        assert_eq!(t.border_accent, ratatui::style::Color::Rgb(255, 0, 0));
    }

    #[test]
    fn build_theme_invalid_hex_override_ignored() {
        let toml = "[theme]\nbase = \"nord\"\nborder_accent = \"notahex\"\n";
        let cfg: AppConfig = toml::from_str(toml).unwrap();
        let t = cfg.build_theme(None);
        assert_eq!(t.name, "nord");
    }

    // ── The menu metadata ────────────────────────────────────────────────────
    //
    // The menu is built from these fields rather than from a list in the code,
    // so these four tests are what stop a built-in layout going missing from it.

    /// Every built-in preset files itself somewhere.
    ///
    /// A missing section is legal for a *user* preset, which lands in Other. For
    /// a built-in it means somebody added a layout and forgot the menu, which is
    /// exactly the drift this design replaces.
    #[test]
    fn every_builtin_preset_declares_a_section() {
        let cfg = LayoutConfig::default_config();
        for (name, preset) in &cfg.presets {
            assert!(
                preset.section.is_some(),
                "built-in preset '{name}' declares no section"
            );
        }
    }

    /// A slot is a number key, so two presets in one section cannot both claim
    /// it. Across sections they can and must: that is the whole point of scoping
    /// the digits.
    #[test]
    fn slots_are_unique_within_a_section() {
        use std::collections::HashSet;
        let cfg = LayoutConfig::default_config();
        let mut seen: HashSet<(&str, u8)> = HashSet::new();
        for (name, preset) in &cfg.presets {
            let (Some(section), Some(slot)) = (preset.section.as_deref(), preset.slot) else {
                continue;
            };
            assert!(
                seen.insert((section, slot)),
                "'{name}' claims slot {slot} in section '{section}', which is taken"
            );
        }
    }

    /// A slot is a single number key, so there is no slot 0 and no slot 12.
    #[test]
    fn slots_are_single_digits() {
        let cfg = LayoutConfig::default_config();
        for (name, preset) in &cfg.presets {
            if let Some(slot) = preset.slot {
                assert!(
                    (1..=9).contains(&slot),
                    "'{name}' has slot {slot}, which is not a number key"
                );
            }
        }
    }

    /// A preset written before the menu existed still loads, and lands nowhere in
    /// particular rather than failing. Loading has always been forgiving here and
    /// four new fields must not change that.
    #[test]
    fn a_preset_without_menu_fields_still_parses() {
        let src = r#"
            panels = [ { name = "footer", position = "bottom" } ]
        "#;
        let preset: PresetConfig = toml::from_str(src).expect("must still parse");
        assert!(preset.section.is_none());
        assert!(preset.slot.is_none());
        assert!(preset.title.is_none());
        assert!(preset.blurb.is_none());
        assert_eq!(preset.panels.len(), 1);
    }

    /// And a preset that carries no menu fields serialises without inventing
    /// them. `save_config` rewrites the whole file, so without the
    /// `skip_serializing_if` every round-tripped `[presets.*]` block in a user's
    /// config would grow four empty keys on the next quit.
    #[test]
    fn absent_menu_fields_are_not_written_back() {
        let preset = PresetConfig {
            panels: vec![PanelSpec {
                name: "footer".into(),
                position: Position::Bottom,
                height: None,
                width_pct: None,
            }],
            ..Default::default()
        };
        let out = toml::to_string_pretty(&preset).unwrap();
        for key in ["section", "slot", "title", "blurb"] {
            assert!(!out.contains(key), "'{key}' should not appear in:\n{out}");
        }
    }
}
