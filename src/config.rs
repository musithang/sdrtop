use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::palette::WaterfallPalette;
use crate::state::{SpectrumMarker, SpectrumStyle, DEFAULT_FREQUENCY, DEFAULT_LNA_GAIN, DEFAULT_SAMPLE_RATE, DEFAULT_VGA_GAIN};

fn default_frequency_hz() -> u64     { DEFAULT_FREQUENCY }
fn default_sample_rate()  -> f64     { DEFAULT_SAMPLE_RATE }
fn default_lna_gain()     -> u32     { DEFAULT_LNA_GAIN }
fn default_vga_gain()     -> u32     { DEFAULT_VGA_GAIN }
fn default_recall()       -> [u64; 3] { [0; 3] }
fn default_active_preset() -> String { "command_rail".into() }
fn default_waterfall_max_rows() -> usize { 64 }

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct RadioConfig {
    #[serde(default = "default_frequency_hz")]
    pub frequency_hz: u64,
    #[serde(default = "default_sample_rate")]
    pub sample_rate: f64,
    #[serde(default = "default_lna_gain")]
    pub lna_gain: u32,
    #[serde(default = "default_vga_gain")]
    pub vga_gain: u32,
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
            sample_rate:  DEFAULT_SAMPLE_RATE,
            lna_gain:     DEFAULT_LNA_GAIN,
            vga_gain:     DEFAULT_VGA_GAIN,
            amp_enabled:  false,
            recall_hz:    [0; 3],
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
            active_preset:      "command_rail".into(),
            waterfall_max_rows: 64,
            waterfall_palette:  WaterfallPalette::Classic,
            spectrum_style:     SpectrumStyle::Braille,
            spectrum_markers:   vec![],
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct ThemeConfig {
    #[serde(default = "ThemeConfig::default_base")]
    pub base: String,
    // Per-field overrides. "#rrggbb" strings. None = use theme default.
    pub border_accent:  Option<String>,
    pub border_dim:     Option<String>,
    pub border_default: Option<String>,
    pub border_focused: Option<String>,
    pub label:          Option<String>,
    pub value:          Option<String>,
    pub value_hi:       Option<String>,
    pub status_ok:      Option<String>,
    pub status_warn:    Option<String>,
    pub status_crit:    Option<String>,
    pub peak_hold:      Option<String>,
    pub noise_floor:    Option<String>,
    pub stale:          Option<String>,
    pub observer:       Option<String>,
}

impl ThemeConfig {
    fn default_base() -> String { "sdr".into() }
}

fn default_sweep_start() -> u64 { 400_000_000 }
fn default_sweep_stop()  -> u64 { 500_000_000 }
fn default_sweep_dwell() -> u64 { 200 }

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
        Self { start_hz: default_sweep_start(), stop_hz: default_sweep_stop(), dwell_ms: default_sweep_dwell() }
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
                eprintln!("Warning: failed to parse {}: {e}. Using defaults.", path.display());
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
    /// Height in terminal rows — used for Top and Bottom panels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u16>,
    /// Width as a percentage of the body zone — used for Left and Right panels.
    /// All panels in the same column carry the same value; the LayoutEngine
    /// reads only the first panel's value to determine column width.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width_pct: Option<u16>,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct PresetConfig {
    pub panels: Vec<PanelSpec>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct LayoutConfig {
    pub active_preset: String,
    pub presets: HashMap<String, PresetConfig>,
}

impl LayoutConfig {
    pub fn default_config() -> Self {
        use Position::*;
        let spectrum = PresetConfig {
            panels: vec![
                PanelSpec { name: "header".into(),   position: Top,    height: Some(5), width_pct: None },
                PanelSpec { name: "spectrum".into(),  position: Body,   height: None,    width_pct: None },
                PanelSpec { name: "log".into(),       position: Bottom, height: Some(5), width_pct: None },
                PanelSpec { name: "footer".into(),    position: Bottom, height: None,    width_pct: None },
            ],
        };
        let waterfall = PresetConfig {
            panels: vec![
                PanelSpec { name: "header".into(),   position: Top,    height: Some(5), width_pct: None },
                PanelSpec { name: "waterfall".into(), position: Body,   height: None,    width_pct: None },
                PanelSpec { name: "log".into(),       position: Bottom, height: Some(5), width_pct: None },
                PanelSpec { name: "footer".into(),    position: Bottom, height: None,    width_pct: None },
            ],
        };
        let spectrum_waterfall = PresetConfig {
            panels: vec![
                PanelSpec { name: "header".into(),    position: Top,    height: Some(5), width_pct: None },
                PanelSpec { name: "spectrum".into(),  position: Body,   height: None,    width_pct: None },
                PanelSpec { name: "waterfall".into(), position: Body,   height: None,    width_pct: None },
                PanelSpec { name: "log".into(),       position: Bottom, height: Some(5), width_pct: None },
                PanelSpec { name: "footer".into(),    position: Bottom, height: None,    width_pct: None },
            ],
        };
        let observer = PresetConfig {
            panels: vec![
                PanelSpec { name: "header".into(),           position: Top,    height: Some(5), width_pct: None     },
                PanelSpec { name: "observer".into(),         position: Left,   height: None,    width_pct: Some(60) },
                PanelSpec { name: "system_resources".into(), position: Right,  height: None,    width_pct: Some(40) },
                PanelSpec { name: "log".into(),              position: Bottom, height: Some(5), width_pct: None     },
                PanelSpec { name: "footer".into(),           position: Bottom, height: None,    width_pct: None     },
            ],
        };
        // Lab IQ — the I/Q quality bench: diagnostics left, constellation 2-D cloud
        // centre, image-rejection scope right (carrier vs mirror vs DC spike). The
        // amplitude histogram is superseded by the constellation (richer view of the
        // same data); the broadband spectrum gives way to the LO-centred scope.
        let lab_iq = PresetConfig {
            panels: vec![
                PanelSpec { name: "header".into(),              position: Top,    height: Some(5), width_pct: None     },
                PanelSpec { name: "lab_banner".into(),          position: Top,    height: Some(2), width_pct: None     },
                PanelSpec { name: "iq_diagnostics".into(),      position: Left,   height: None,    width_pct: Some(28) },
                PanelSpec { name: "iq_constellation".into(),    position: Body,   height: None,    width_pct: None     },
                PanelSpec { name: "image_scope".into(),         position: Right,  height: None,    width_pct: Some(32) },
                PanelSpec { name: "lab_marker".into(),          position: Bottom, height: Some(2), width_pct: None     },
                PanelSpec { name: "log".into(),                 position: Bottom, height: Some(5), width_pct: None     },
                PanelSpec { name: "footer".into(),              position: Bottom, height: None,    width_pct: None     },
            ],
        };
        let main = PresetConfig {
            panels: vec![
                PanelSpec { name: "header".into(),       position: Top,    height: Some(5), width_pct: None },
                PanelSpec { name: "spectrum".into(),      position: Body,   height: None,    width_pct: None },
                PanelSpec { name: "waterfall".into(),     position: Body,   height: None,    width_pct: None },
                // height 4 → 2 inner rows for the signal_strip's 2×4 gauge grid.
                PanelSpec { name: "signal_strip".into(),  position: Bottom, height: Some(4), width_pct: None },
                PanelSpec { name: "log".into(),           position: Bottom, height: Some(5), width_pct: None },
                PanelSpec { name: "footer".into(),        position: Bottom, height: None,    width_pct: None },
            ],
        };
        // Command Rail — the [1] poweruser default (DSN-2026-02): a slim header
        // (status + γ-dial), a left instrument rail gathering freq-hero + signal +
        // gain + stream + log foot, and the bonded spectrum/waterfall filling the
        // body. The [spectrum, waterfall] Body pair triggers the bond automatically.
        let command_rail = PresetConfig {
            panels: vec![
                PanelSpec { name: "header_slim".into(),   position: Top,    height: Some(4), width_pct: None     },
                PanelSpec { name: "command_rail".into(),  position: Left,   height: None,    width_pct: Some(28) },
                PanelSpec { name: "spectrum".into(),      position: Body,   height: None,    width_pct: None     },
                PanelSpec { name: "waterfall".into(),     position: Body,   height: None,    width_pct: None     },
                PanelSpec { name: "footer".into(),        position: Bottom, height: None,    width_pct: None     },
            ],
        };
        // Lab RF — front-end / gain chain focus: RF chain + NF/MDS left, spectrum
        // centre, hardware health right.
        let lab_rf = PresetConfig {
            panels: vec![
                PanelSpec { name: "header".into(),          position: Top,    height: Some(5), width_pct: None     },
                PanelSpec { name: "lab_banner".into(),      position: Top,    height: Some(2), width_pct: None     },
                PanelSpec { name: "rf_chain".into(),        position: Left,   height: None,    width_pct: Some(30) },
                PanelSpec { name: "level_diagram".into(),   position: Body,   height: None,    width_pct: None     },
                PanelSpec { name: "adc_loading".into(),     position: Right,  height: None,    width_pct: Some(32) },
                PanelSpec { name: "lab_marker".into(),      position: Bottom, height: Some(2), width_pct: None     },
                PanelSpec { name: "log".into(),             position: Bottom, height: Some(5), width_pct: None     },
                PanelSpec { name: "footer".into(),          position: Bottom, height: None,    width_pct: None     },
            ],
        };
        // Lab signal — signal-characterization instrument (DSN-2026-07 redesign):
        // a left characterization rail (headline / metrics / ACPR / spectral
        // shape), the bonded spectrum + waterfall filling the body, and the FM
        // MPX · demod column on the right. The [spectrum, waterfall] Body pair
        // triggers the engine's bond automatically (shared frequency ruler).
        let lab_signal = PresetConfig {
            panels: vec![
                PanelSpec { name: "header".into(),                  position: Top,    height: Some(5), width_pct: None     },
                PanelSpec { name: "lab_banner".into(),              position: Top,    height: Some(2), width_pct: None     },
                PanelSpec { name: "signal_characterization".into(), position: Left,   height: None,    width_pct: Some(26) },
                PanelSpec { name: "spectrum".into(),                position: Body,   height: None,    width_pct: None     },
                PanelSpec { name: "waterfall".into(),               position: Body,   height: None,    width_pct: None     },
                PanelSpec { name: "fm_demod".into(),                position: Right,  height: None,    width_pct: Some(26) },
                PanelSpec { name: "lab_marker".into(),              position: Bottom, height: Some(2), width_pct: None     },
                PanelSpec { name: "log".into(),                     position: Bottom, height: Some(5), width_pct: None     },
                PanelSpec { name: "footer".into(),                  position: Bottom, height: None,    width_pct: None     },
            ],
        };
        // Lab timing — host-side stream-timing instrument (DSN-2026-06 redesign):
        // a left diagnostics rail (callback period / jitter / deadline budget /
        // sample-rate), the real-time per-callback strip chart filling the body,
        // and a hardware-vitals column on the right.
        let lab_timing = PresetConfig {
            panels: vec![
                PanelSpec { name: "header".into(),             position: Top,    height: Some(5), width_pct: None     },
                PanelSpec { name: "lab_banner".into(),         position: Top,    height: Some(2), width_pct: None     },
                PanelSpec { name: "timing_diagnostics".into(), position: Left,   height: None,    width_pct: Some(28) },
                PanelSpec { name: "timing_stripchart".into(),  position: Body,   height: None,    width_pct: None     },
                PanelSpec { name: "timing_vitals".into(),      position: Right,  height: None,    width_pct: Some(24) },
                PanelSpec { name: "lab_marker".into(),         position: Bottom, height: Some(2), width_pct: None     },
                PanelSpec { name: "log".into(),                position: Bottom, height: Some(5), width_pct: None     },
                PanelSpec { name: "footer".into(),             position: Bottom, height: None,    width_pct: None     },
            ],
        };
        // Lab sweep — frequency-scanner mode: the wide sweep curve fills the body,
        // cursor metrics on the right, sweep status strip below. Compact header
        // (gain matters less while sweeping).
        let lab_sweep = PresetConfig {
            panels: vec![
                PanelSpec { name: "header".into(),         position: Top,    height: Some(3), width_pct: None     },
                PanelSpec { name: "lab_banner".into(),     position: Top,    height: Some(2), width_pct: None     },
                PanelSpec { name: "sweep_panel".into(),    position: Body,   height: None,    width_pct: None     },
                PanelSpec { name: "signal_metrics".into(), position: Right,  height: None,    width_pct: Some(22) },
                PanelSpec { name: "sweep_strip".into(),    position: Bottom, height: Some(3), width_pct: None     },
                PanelSpec { name: "log".into(),            position: Bottom, height: Some(3), width_pct: None     },
                PanelSpec { name: "footer".into(),         position: Bottom, height: None,    width_pct: None     },
            ],
        };
        // Micro main — the [0] field-mode entry view. A single self-contained
        // panel that manages its own zones, plus the footer.
        let micro_main = PresetConfig {
            panels: vec![
                PanelSpec { name: "micro_panel".into(), position: Body,   height: None, width_pct: None },
                PanelSpec { name: "footer".into(),      position: Bottom, height: None, width_pct: None },
            ],
        };
        // Micro signal — [0] cycle step 2: large SNR view for antenna aiming.
        let micro_signal = PresetConfig {
            panels: vec![
                PanelSpec { name: "micro_signal_panel".into(), position: Body,   height: None, width_pct: None },
                PanelSpec { name: "footer".into(),             position: Bottom, height: None, width_pct: None },
            ],
        };
        // Micro gain — [0] cycle step 3: gain-staging view for fast setup.
        let micro_gain = PresetConfig {
            panels: vec![
                PanelSpec { name: "micro_gain_panel".into(), position: Body,   height: None, width_pct: None },
                PanelSpec { name: "footer".into(),           position: Bottom, height: None, width_pct: None },
            ],
        };
        // Micro health — [0] cycle step 4: hardware monitoring for long sessions.
        let micro_health = PresetConfig {
            panels: vec![
                PanelSpec { name: "micro_health_panel".into(), position: Body,   height: None, width_pct: None },
                PanelSpec { name: "footer".into(),             position: Bottom, height: None, width_pct: None },
            ],
        };
        // Micro sweep — [0] cycle step 5: field scanner glance (starts a sweep).
        let micro_sweep = PresetConfig {
            panels: vec![
                PanelSpec { name: "micro_sweep_panel".into(), position: Body,   height: None, width_pct: None },
                PanelSpec { name: "footer".into(),            position: Bottom, height: None, width_pct: None },
            ],
        };
        let mut presets = HashMap::new();
        presets.insert("spectrum".into(), spectrum);
        presets.insert("waterfall".into(), waterfall);
        presets.insert("spectrum_waterfall".into(), spectrum_waterfall);
        presets.insert("observer".into(), observer);
        presets.insert("main".into(), main);
        presets.insert("command_rail".into(), command_rail);
        presets.insert("lab_iq".into(), lab_iq);
        presets.insert("lab_rf".into(), lab_rf);
        presets.insert("lab_signal".into(), lab_signal);
        presets.insert("lab_timing".into(), lab_timing);
        presets.insert("lab_sweep".into(), lab_sweep);
        presets.insert("micro_main".into(), micro_main);
        presets.insert("micro_signal".into(), micro_signal);
        presets.insert("micro_gain".into(), micro_gain);
        presets.insert("micro_health".into(), micro_health);
        presets.insert("micro_sweep".into(), micro_sweep);
        Self { active_preset: "command_rail".into(), presets }
    }

    /// Built-in presets with the user's custom presets merged on top. A user
    /// preset whose name matches a built-in replaces it; new names are added
    /// (and so join the `[P]` cycle automatically).
    pub fn with_user_presets(user: &HashMap<String, PresetConfig>) -> Self {
        let mut cfg = Self::default_config();
        for (name, preset) in user {
            cfg.presets.insert(name.clone(), preset.clone());
        }
        cfg
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

    #[test]
    fn default_config_has_minimal_preset() {
        let cfg = LayoutConfig::default_config();
        assert_eq!(cfg.active_preset, "command_rail");
        assert!(!cfg.active_panels().is_empty());
    }

    #[test]
    fn active_panels_returns_correct_names() {
        let cfg = LayoutConfig::default_config();
        let names: Vec<&str> = cfg.active_panels().iter().map(|p| p.name.as_str()).collect();
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
        assert_eq!(RadioConfig::default().lna_gain, 16);
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
        cfg.radio.lna_gain = 24;
        cfg.display.active_preset = "spectrum".into();
        let serialized = toml::to_string_pretty(&cfg).unwrap();
        let restored: AppConfig = toml::from_str(&serialized).unwrap();
        assert_eq!(restored.radio.lna_gain, 24);
        assert_eq!(restored.display.active_preset, "spectrum");
    }

    #[test]
    fn spectrum_style_round_trips_and_defaults_braille() {
        let cfg: AppConfig = toml::from_str("[display]\nactive_preset = \"spectrum\"\n").unwrap();
        assert_eq!(cfg.display.spectrum_style, SpectrumStyle::Braille);
        let mut cfg = AppConfig::default();
        cfg.display.spectrum_style = SpectrumStyle::Scatter;
        let serialized = toml::to_string_pretty(&cfg).unwrap();
        assert!(serialized.contains("spectrum_style = \"scatter\""), "got:\n{serialized}");
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
        assert!(serialized.contains("waterfall_palette = \"phosphor\""), "got:\n{serialized}");
        let restored: AppConfig = toml::from_str(&serialized).unwrap();
        assert_eq!(restored.display.waterfall_palette, WaterfallPalette::Phosphor);
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
            let p = cfg.presets.get(name).unwrap_or_else(|| panic!("missing preset {name}"));
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
        let p = cfg.presets.get("lab_sweep").expect("lab_sweep preset present");
        let names: Vec<&str> = p.panels.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"sweep_panel"), "lab_sweep missing sweep_panel");
        assert!(names.contains(&"sweep_strip"), "lab_sweep missing sweep_strip");
    }

    #[test]
    fn default_config_lab_signal_has_redesign_panels() {
        // The lab_signal redesign is a three-zone instrument: the characterization
        // rail, the bonded spectrum + waterfall center, and the FM demod column.
        let cfg = LayoutConfig::default_config();
        let p = cfg.presets.get("lab_signal").expect("lab_signal preset present");
        let names: Vec<&str> = p.panels.iter().map(|s| s.name.as_str()).collect();
        for panel in ["signal_characterization", "spectrum", "waterfall", "fm_demod"] {
            assert!(names.contains(&panel), "lab_signal missing {panel}");
        }
        // spectrum + waterfall both live in the Body column so the engine bonds
        // them into one shared-ruler instrument.
        let body: Vec<&str> = p.panels.iter()
            .filter(|s| matches!(s.position, Position::Body))
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(body, vec!["spectrum", "waterfall"], "bond pair must be the Body column");
        // The bottom lab chrome (marker bar) rides along like the other labs.
        assert!(names.contains(&"lab_marker"), "lab_signal missing lab_marker");
    }

    #[test]
    fn default_config_lab_timing_has_redesign_panels() {
        // The lab_timing redesign is a three-zone instrument: diagnostics rail,
        // the per-callback strip chart, and the hardware-vitals column.
        let cfg = LayoutConfig::default_config();
        let p = cfg.presets.get("lab_timing").expect("lab_timing preset present");
        let names: Vec<&str> = p.panels.iter().map(|s| s.name.as_str()).collect();
        for panel in ["timing_diagnostics", "timing_stripchart", "timing_vitals"] {
            assert!(names.contains(&panel), "lab_timing missing {panel}");
        }
        // The bottom lab chrome (marker bar) rides along like the other labs.
        assert!(names.contains(&"lab_marker"), "lab_timing missing lab_marker");
    }

    #[test]
    fn default_config_has_micro_main() {
        let cfg = LayoutConfig::default_config();
        let p = cfg.presets.get("micro_main").expect("micro_main preset present");
        let names: Vec<&str> = p.panels.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["micro_panel", "footer"]);
    }

    #[test]
    fn default_config_has_full_micro_cycle() {
        // Every step of the [0] cycle must have a defined preset + its dedicated panel.
        let cfg = LayoutConfig::default_config();
        for (preset, panel) in [
            ("micro_main",   "micro_panel"),
            ("micro_signal", "micro_signal_panel"),
            ("micro_gain",   "micro_gain_panel"),
            ("micro_health", "micro_health_panel"),
        ] {
            let p = cfg.presets.get(preset).unwrap_or_else(|| panic!("missing {preset}"));
            assert_eq!(p.panels.first().map(|s| s.name.as_str()), Some(panel), "{preset} body panel");
            assert_eq!(p.panels.last().map(|s| s.name.as_str()), Some("footer"), "{preset} footer");
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
        let cfg = LayoutConfig::with_user_presets(&app.presets);
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
        let custom = restored.presets.get("custom").expect("custom preset survives round-trip");
        assert_eq!(custom.panels.len(), 2);
        assert_eq!(custom.panels[0].height, Some(3));
        assert_eq!(custom.panels[1].name, "footer");
    }

    #[test]
    fn app_config_without_presets_omits_section() {
        let app = AppConfig::default();
        let serialized = toml::to_string_pretty(&app).unwrap();
        assert!(!serialized.contains("[presets"), "empty presets should not emit a section: {serialized}");
    }

    #[test]
    fn per_field_theme_overrides_survive_a_save() {
        // The bug the lint reported from R3a to R6: `App::save_config` rebuilt the
        // theme block as `ThemeConfig { base, ..Default::default() }`, so every
        // per-field colour the user had written was deleted from their config the
        // next time they pressed `q`. The docs carried a warning about it.
        let mut loaded = ThemeConfig { base: "nord".into(), ..Default::default() };
        loaded.border_accent = Some("#ff00ff".into());
        loaded.stale         = Some("#101010".into());

        // Exactly what `save_config` writes now: the loaded block, `base` refreshed.
        let written = AppConfig {
            theme: ThemeConfig { base: "nord".into(), ..loaded.clone() },
            ..AppConfig::default()
        };
        let path = std::env::temp_dir()
            .join(format!("sdrtop-theme-save-{}.toml", std::process::id()));
        written.save(&path).unwrap();
        let back = AppConfig::load_or_default(&path);
        let _ = std::fs::remove_file(&path);

        assert_eq!(back.theme.base, "nord");
        assert_eq!(back.theme.border_accent.as_deref(), Some("#ff00ff"),
                   "the override was dropped on save again");
        assert_eq!(back.theme.stale.as_deref(), Some("#101010"));

        // And the shape that caused it, so this test cannot pass by accident.
        let old_way = ThemeConfig { base: "nord".into(), ..Default::default() };
        assert!(old_way.border_accent.is_none(),
                "spreading Default is what deleted the overrides");
    }

    #[test]
    fn build_theme_prefers_a_user_theme_over_the_builtin() {
        // `~/.config/sdrtop/themes/<base>.toml` replaces the shipped theme of that
        // name, and per-field overrides still apply on top of whichever won.
        let dir = std::env::temp_dir()
            .join(format!("sdrtop-cfg-theme-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut text = String::from("name = \"gruvbox\"\n");
        for f in ["border_dim", "border_default", "border_accent", "border_focused",
                  "label", "value", "value_hi", "status_ok", "status_warn",
                  "status_crit", "peak_hold", "noise_floor", "stale", "observer"] {
            text.push_str(&format!("{f} = \"#020202\"\n"));
        }
        text.push_str("palette = [{ at = 0.0, color = \"#000000\" }, { at = 1.0, color = \"#ffffff\" }]\n");
        std::fs::write(dir.join("gruvbox.toml"), text).unwrap();

        let mut cfg = AppConfig::default();
        cfg.theme.base = "gruvbox".into();
        cfg.theme.value_hi = Some("#abcdef".into());
        let t = cfg.build_theme(Some(&dir));
        assert_eq!(t.border_accent, ratatui::style::Color::Rgb(2, 2, 2), "user file should win");
        assert_eq!(t.value_hi, ratatui::style::Color::Rgb(0xab, 0xcd, 0xef),
                   "an override still applies on top of a user theme");
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
}
