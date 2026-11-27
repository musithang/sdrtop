// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The engine's blocks of the config file, and how the file is read and
//! written: each front end composes the blocks it owns with these and saves
//! only its own through [`save_blocks`].

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

use crate::state::{DEFAULT_FREQUENCY, DEFAULT_SAMPLE_RATE};

fn default_frequency_hz() -> u64 {
    DEFAULT_FREQUENCY
}
fn default_sample_rate() -> f64 {
    DEFAULT_SAMPLE_RATE
}
fn default_recall() -> [u64; 3] {
    [0; 3]
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
    /// What the gain knob walks, on a radio that has vendor curves: a curve
    /// by its name (`"Linearity"`), or `"manual"` for the stages by hand.
    /// Absent: the radio's own default. Written only for a radio with curves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gain_mode: Option<String>,
    /// The front-end boost (an amp, a tuner or firmware AGC) each radio was
    /// left with, by the radio's own name (`"HackRF One" = true`), so one
    /// radio's boost is never another's start. A radio not listed starts with
    /// it off. Written only for a radio that has a boost. The single
    /// `amp_enabled` this replaced is no longer read: which radio it was saved
    /// on is not known, so nothing can honestly start from it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub boost: BTreeMap<String, bool>,
    /// Command Rail recall slots (the rail's `M` save / `1·2·3` jump). Three
    /// fixed slots; `0` means empty. Tuning memory belongs with the radio.
    #[serde(default = "default_recall")]
    pub recall_hz: [u64; 3],
}

impl RadioConfig {
    /// The boost the radio called `board` was left with; off for one never
    /// seen.
    pub fn boost_for(&self, board: &str) -> bool {
        self.boost.get(board).copied().unwrap_or(false)
    }
}

impl Default for RadioConfig {
    fn default() -> Self {
        Self {
            frequency_hz: DEFAULT_FREQUENCY,
            sample_rate: DEFAULT_SAMPLE_RATE,
            gain: None,
            lna_gain: None,
            vga_gain: None,
            gain_mode: None,
            boost: BTreeMap::new(),
            recall_hz: [0; 3],
        }
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

/// How many classic Bluetooth channels
/// [`crate::signal::net::worker:: NetWorker`] gives a live receiver at once, of
/// however many the current tuning and sample rate let it see at all
/// (`signal::bt::channel:: channels_in_span`). Small on purpose - see
/// `signal::bt::receive`'s own doc for the measured cost a channel-select
/// filter at 1 MHz spacing carries per channel, well past a budget of a few
/// operations per sample for always-on detection once more than a handful run
/// at once. Raising this is a real request for more CPU, not a free wider view,
/// and `NetWorker::new` logs as much once when it is asked for more than
/// [`crate::signal::net::worker::SAFE_BT_CHANNELS`].
fn default_bt_channels() -> usize {
    8
}

/// `[net]` config for the NET section shared by the Bluetooth and Wi-Fi
/// arcs. Small on purpose, the same reasoning `NetState`'s own doc gives:
/// only a setting that has to survive a restart belongs here at all.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct NetSettings {
    #[serde(default = "default_bt_channels")]
    pub bt_channels: usize,
}

impl Default for NetSettings {
    fn default() -> Self {
        Self {
            bt_channels: default_bt_channels(),
        }
    }
}

/// `[doppler]`: the carriers the doppler lab watches, as the `doppler`
/// command takes them: the first is the one the others are differenced
/// against.
#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq)]
pub struct DopplerSettings {
    #[serde(default)]
    pub carriers: Vec<crate::signal::doppler::CarrierSetting>,
}

impl DopplerSettings {
    pub fn is_default(&self) -> bool {
        self.carriers.is_empty()
    }
}

/// `[floor]`: the floor lab's bin width. The zoom is a viewing choice and
/// is not kept.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
pub struct FloorSettings {
    #[serde(default = "default_floor_bin_hz")]
    pub bin_hz: f64,
}

fn default_floor_bin_hz() -> f64 {
    crate::signal::floor::FloorSetting::default().bin_hz
}

impl Default for FloorSettings {
    fn default() -> Self {
        FloorSettings {
            bin_hz: default_floor_bin_hz(),
        }
    }
}

impl FloorSettings {
    pub fn is_default(&self) -> bool {
        *self == FloorSettings::default()
    }
}

/// `[pager]`: what the pager view keeps between runs. Only how its display
/// looks: which page was on it, or whose pager it was, is a session's.
#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct PagerSettings {
    #[serde(default)]
    pub night: bool,
    /// The display's colour by name (`theme::display_names`); absent is the
    /// theme's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<String>,
}

impl PagerSettings {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// `[station]`: where you listen from and your call, kept across radios.
///
/// The FT8 map is drawn around this position, and a message naming this call
/// is marked as yours. Both are optional: without a position there is no
/// map (it is never guessed), without a call nothing is marked. A value out
/// of range in a hand-edited file is refused at start, said in the log, and
/// treated as absent.
#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq)]
pub struct StationSettings {
    /// Degrees north, -90 to 90.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latitude: Option<f64>,
    /// Degrees east, -180 to 180.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub longitude: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callsign: Option<String>,
}

impl StationSettings {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// `[ft8]`: what the FT8 map draws, as its focus mode left it. Every field
/// optional and written only when it differs from the default; a word this
/// version does not know is said in the log and left at the default.
#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq)]
pub struct Ft8Settings {
    /// `log`, `1500`, `3000` or `9000`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rays: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grid: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rings: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sun: Option<bool>,
    /// `all`, `this cycle` or `none`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub names: Option<String>,
    /// `this cycle`, `15 min`, `1 h` or `session`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub this_band: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub long_path: Option<bool>,
}

impl Ft8Settings {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// `[record]`: how long an IQ recording may run, whichever limit comes first.
///
/// No in-app control changes these, so `App` holds the loaded block and
/// writes it back as it was, the way it holds `[net]`.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
pub struct RecordSettings {
    #[serde(default = "default_record_seconds")]
    pub max_seconds: f64,
    #[serde(default = "default_record_gb")]
    pub max_gb: f64,
}

fn default_record_seconds() -> f64 {
    crate::export::iq::recording::Limits::DEFAULT.max_secs
}

fn default_record_gb() -> f64 {
    crate::export::iq::recording::Limits::DEFAULT.max_bytes as f64 / 1e9
}

impl Default for RecordSettings {
    fn default() -> Self {
        Self {
            max_seconds: default_record_seconds(),
            max_gb: default_record_gb(),
        }
    }
}

impl RecordSettings {
    /// The limits a recording runs to, and a sentence for each value that
    /// could not be one and was replaced by its default.
    pub fn limits(&self) -> (crate::export::iq::recording::Limits, Vec<String>) {
        let mut notes = Vec::new();
        let mut pick = |name: &str, v: f64, default: f64| {
            if v.is_finite() && v > 0.0 {
                v
            } else {
                notes.push(format!(
                    "[record] {name} = {v} is not a limit, using {default}"
                ));
                default
            }
        };
        let secs = pick("max_seconds", self.max_seconds, default_record_seconds());
        let gb = pick("max_gb", self.max_gb, default_record_gb());
        (
            crate::export::iq::recording::Limits {
                max_secs: secs,
                max_bytes: (gb * 1e9) as u64,
            },
            notes,
        )
    }
}

fn default_tinysa_points() -> u32 {
    450
}

fn default_auto() -> String {
    "auto".into()
}

/// Settings applied when a tinySA backend opens.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct TinySaSettings {
    #[serde(default)]
    pub basic_input: crate::hardware::tinysa::BasicInput,
    #[serde(default = "default_tinysa_points")]
    pub points: u32,
    #[serde(default = "default_auto")]
    pub rbw: String,
    #[serde(default = "default_auto")]
    pub attenuation: String,
    #[serde(default)]
    pub high_attenuation: bool,
    #[serde(default)]
    pub lna: bool,
    #[serde(default = "default_auto")]
    pub spur: String,
    #[serde(default)]
    pub ext_gain_db: i32,
}

impl Default for TinySaSettings {
    fn default() -> Self {
        Self {
            basic_input: crate::hardware::tinysa::BasicInput::default(),
            points: default_tinysa_points(),
            rbw: default_auto(),
            attenuation: default_auto(),
            high_attenuation: false,
            lna: false,
            spur: default_auto(),
            ext_gain_db: 0,
        }
    }
}

/// Settings the Airspy R2's own backend applies when it opens.
///
/// The bias tee is deliberately not here: it starts off every session,
/// because 4.5 V on an antenna port rewired since the last one is how an LNA
/// dies.
#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct AirspySettings {
    /// libairspy's 12-bit packing: a quarter less USB traffic, unpacked on
    /// the host.
    #[serde(default)]
    pub packing: bool,
}

impl AirspySettings {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// Settings the PlutoSky R2's own backend applies when it opens.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct PlutoSkySettings {
    /// Which of the AD9361's automatic gain modes `[A]` switches on:
    /// `slow_attack` or `fast_attack` (the hybrid mode does not follow the
    /// signal on the R2, measured).
    #[serde(default = "PlutoSkySettings::default_agc")]
    pub agc: String,
}

impl Default for PlutoSkySettings {
    fn default() -> Self {
        Self {
            agc: Self::default_agc(),
        }
    }
}

impl PlutoSkySettings {
    fn default_agc() -> String {
        "slow_attack".to_string()
    }

    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// Settings the Airspy HF+ Discovery's own backend applies when it opens.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct AirspyHfSettings {
    /// libairspyhf's own processing: the IQ balance correction at the zero-IF
    /// rates, the 5 kHz IF shift and the fine tuning. Off, the samples are the
    /// radio's words as they came, centred on the firmware's kHz grid.
    #[serde(default = "AirspyHfSettings::default_dsp")]
    pub dsp: bool,
}

impl Default for AirspyHfSettings {
    fn default() -> Self {
        Self { dsp: true }
    }
}

impl AirspyHfSettings {
    fn default_dsp() -> bool {
        true
    }

    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// `[sdrplay]`: the RSP1B's switches that are kept. The bias tee is not
/// among them: it starts off every time, and is switched by hand.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct SdrplaySettings {
    /// SDRplay's API removes DC and balances I and Q before sdrtop sees a
    /// sample. On by default: the radio's own balance falls to 27 dB of
    /// image rejection at 950 MHz, the API's lifts it by 20 to 34 dB.
    #[serde(default = "SdrplaySettings::on")]
    pub corrections: bool,
    /// The AM/FM notch (broadcast MW and FM bands).
    #[serde(default)]
    pub fm_notch: bool,
    /// The DAB notch (band III).
    #[serde(default)]
    pub dab_notch: bool,
}

impl Default for SdrplaySettings {
    fn default() -> Self {
        Self {
            corrections: true,
            fm_notch: false,
            dab_notch: false,
        }
    }
}

impl SdrplaySettings {
    fn on() -> bool {
        true
    }

    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// The blocks Monas owns: the radio and everything it measures with. The
/// terminal and the headless server both run the engine, so both read these
/// and both write them back, through [`EngineConfig::save`].
#[derive(Deserialize, Serialize, Clone, Debug, Default)]
pub struct EngineConfig {
    #[serde(default)]
    pub radio: RadioConfig,
    #[serde(default)]
    pub sweep: SweepSettings,
    #[serde(default)]
    pub net: NetSettings,
    #[serde(default)]
    pub record: RecordSettings,
    /// Written only once it differs from the default, as `[airspy]` is.
    #[serde(default, skip_serializing_if = "PagerSettings::is_default")]
    pub pager: PagerSettings,
    /// Written only once something is set.
    #[serde(default, skip_serializing_if = "StationSettings::is_default")]
    pub station: StationSettings,
    /// Written only once the FT8 map differs from its default.
    #[serde(default, skip_serializing_if = "Ft8Settings::is_default")]
    pub ft8: Ft8Settings,
    /// Written only once a carrier is chosen.
    #[serde(default, skip_serializing_if = "DopplerSettings::is_default")]
    pub doppler: DopplerSettings,
    /// Written only once the bin width differs from one hertz.
    #[serde(default, skip_serializing_if = "FloorSettings::is_default")]
    pub floor: FloorSettings,
    #[serde(default)]
    pub tinysa: TinySaSettings,
    /// Written only once it differs from the default, so a file from a radio
    /// that is not an Airspy carries no block about one.
    #[serde(default, skip_serializing_if = "AirspySettings::is_default")]
    pub airspy: AirspySettings,
    /// The same for the Airspy HF+.
    #[serde(default, skip_serializing_if = "AirspyHfSettings::is_default")]
    pub airspyhf: AirspyHfSettings,
    /// Written only once it differs from the default, as `[airspyhf]` is.
    #[serde(default, skip_serializing_if = "SdrplaySettings::is_default")]
    pub sdrplay: SdrplaySettings,
    /// Written only once it differs from the default, as `[airspyhf]` is.
    #[serde(default, skip_serializing_if = "PlutoSkySettings::is_default")]
    pub plutosky: PlutoSkySettings,
}

impl EngineConfig {
    /// Write the engine's blocks into the file at `path`, and nothing else.
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        save_blocks(self, path, ENGINE_BLOCKS)
    }
}

/// The config at `path`, or the defaults when there is none. A file that does
/// not parse is said on stderr and read as the defaults rather than refusing
/// to start.
pub fn load_or_default<T: serde::de::DeserializeOwned + Default>(path: &Path) -> T {
    let content = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return T::default(),
    };
    match toml::from_str(&content) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!(
                "Warning: failed to parse {}: {e}. Using defaults.",
                path.display()
            );
            T::default()
        }
    }
}

/// Write the blocks named in `owned` from `value` into the file at `path`,
/// and keep every other block exactly as the file has it.
///
/// **One file, three owners.** The engine owns the radio and what it
/// measures with; the terminal owns how it is shown; `[monasd]` is the
/// operator's. Each writes only its own, so a headless instance's save cannot
/// reset the layout and the theme, and neither half can delete the other's by
/// not carrying it forward. A block of `owned` that serialises to nothing
/// (back at its default) is removed, as a whole-file save would leave it out.
/// A file that is not TOML has nothing to keep and is written fresh. The
/// write goes through a temporary file beside it, so a failed write never
/// leaves half a file, and a failure names the file it could not create,
/// write or replace.
pub fn save_blocks<T: Serialize>(value: &T, path: &Path, owned: &[&str]) -> anyhow::Result<()> {
    use anyhow::Context;
    let mine: toml::Table = toml::Table::try_from(value)?;
    let mut merged: toml::Table = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| text.parse().ok())
        .unwrap_or_default();
    for block in owned {
        match mine.get(*block) {
            Some(value) => {
                merged.insert((*block).to_string(), value.clone());
            }
            None => {
                merged.remove(*block);
            }
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    let content = toml::to_string_pretty(&merged)?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, content).with_context(|| format!("could not write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("could not replace {}", path.display()))?;
    Ok(())
}

/// The blocks of the config file the engine writes: the radio and
/// everything it measures with. See [`save_blocks`].
pub const ENGINE_BLOCKS: &[&str] = &[
    "radio", "sweep", "net", "record", "pager", "station", "ft8", "doppler", "floor", "tinysa",
    "airspy", "airspyhf", "sdrplay", "plutosky",
];

#[cfg(test)]
mod tests {

    /// `[floor]` keeps the bin width, written only once it differs from
    /// one hertz; the zoom is a viewing choice and is not kept.
    #[test]
    fn the_floor_block_round_trips() {
        let mut cfg = EngineConfig::default();
        assert_eq!(cfg.floor.bin_hz, 1.0);
        assert!(!toml::to_string(&cfg).unwrap().contains("[floor]"));
        cfg.floor.bin_hz = 0.5;
        let text = toml::to_string(&cfg).unwrap();
        assert!(text.contains("[floor]\nbin_hz = 0.5\n"), "{text}");
        let back: EngineConfig = toml::from_str(&text).unwrap();
        assert_eq!(back.floor, cfg.floor);
        assert!(!text.contains("zoom"));
    }

    /// `[doppler]` holds the carriers as the command takes them, span and
    /// resolution filled in where a line leaves them out.
    #[test]
    fn the_doppler_block_takes_carriers_as_the_command_does() {
        let cfg: EngineConfig = toml::from_str(
            "[doppler]\n[[doppler.carriers]]\nhz = 9996000\n[[doppler.carriers]]\nhz = 10000000\nspan_hz = 50.0\n",
        )
        .unwrap();
        let c = &cfg.doppler.carriers;
        assert_eq!(
            (c[0].hz, c[0].span_hz, c[0].resolution_hz),
            (9_996_000, 25.0, 0.1)
        );
        assert_eq!((c[1].hz, c[1].span_hz), (10_000_000, 50.0));
        assert!(EngineConfig::default().doppler.carriers.is_empty());
    }
    use super::*;

    /// `[sdrplay]` is written only once a switch differs from its default,
    /// and read back as written; the corrections default to on.
    #[test]
    fn the_sdrplay_block_is_written_only_when_it_says_something() {
        let mut cfg = EngineConfig::default();
        assert!(cfg.sdrplay.corrections);
        assert!(!toml::to_string(&cfg).unwrap().contains("[sdrplay]"));
        cfg.sdrplay.dab_notch = true;
        let text = toml::to_string(&cfg).unwrap();
        assert!(text.contains("[sdrplay]"), "{text}");
        let back: EngineConfig = toml::from_str(&text).unwrap();
        assert_eq!(back.sdrplay, cfg.sdrplay);
        let partial: EngineConfig = toml::from_str("[sdrplay]\nfm_notch = true\n").unwrap();
        assert!(partial.sdrplay.corrections && partial.sdrplay.fm_notch);
    }

    /// **A save that fails names the file it could not write**: the
    /// directory it could not create, or the temporary file it could not
    /// write, never a bare "Permission denied (os error 13)" with no path.
    #[test]
    fn a_failed_save_names_the_file() {
        let dir = std::env::temp_dir().join(format!("sdrtop-save-fails-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // The config's directory is a file: it cannot be created.
        let blocker = dir.join("not-a-dir");
        std::fs::write(&blocker, "").unwrap();
        let err = format!(
            "{:#}",
            EngineConfig::default()
                .save(&blocker.join("config.toml"))
                .unwrap_err()
        );
        assert!(err.contains(&blocker.display().to_string()), "{err}");

        // The temporary file's name is taken by a directory: it cannot be
        // written.
        std::fs::create_dir_all(dir.join("config.tmp")).unwrap();
        let err = format!(
            "{:#}",
            EngineConfig::default()
                .save(&dir.join("config.toml"))
                .unwrap_err()
        );
        assert!(err.contains("config.tmp"), "{err}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **`[airspy]` appears only once it says something**: a default file has
    /// no Airspy block, and packing set comes back as written.
    #[test]
    fn the_airspy_block_is_written_only_when_it_says_something() {
        let plain = toml::to_string(&EngineConfig::default()).unwrap();
        assert!(!plain.contains("[airspy]"), "{plain}");
        let mut cfg = EngineConfig::default();
        cfg.airspy.packing = true;
        let text = toml::to_string(&cfg).unwrap();
        assert!(text.contains("[airspy]\npacking = true"), "{text}");
        let back: EngineConfig = toml::from_str(&text).unwrap();
        assert!(back.airspy.packing);
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
        let cfg = load_or_default::<EngineConfig>(Path::new("/nonexistent/sdrtop/config.toml"));
        assert_eq!(cfg.radio.frequency_hz, RadioConfig::default().frequency_hz);
    }

    /// `[record]` defaults to 60 s or 4 GB, survives a save, and a value
    /// that cannot be a limit is replaced by its default and named.
    #[test]
    fn record_settings_default_round_trip_and_refuse_nonsense() {
        let cfg: EngineConfig = toml::from_str("").unwrap();
        let (limits, notes) = cfg.record.limits();
        assert_eq!((limits.max_secs, limits.max_bytes), (60.0, 4_000_000_000));
        assert!(notes.is_empty());

        let cfg: EngineConfig = toml::from_str("[record]\nmax_seconds = 5.5").unwrap();
        let back: EngineConfig = toml::from_str(&toml::to_string(&cfg).unwrap()).unwrap();
        assert_eq!(back.record.max_seconds, 5.5);
        assert_eq!(back.record.max_gb, 4.0);

        let cfg: EngineConfig =
            toml::from_str("[record]\nmax_seconds = -1.0\nmax_gb = 0.0").unwrap();
        let (limits, notes) = cfg.record.limits();
        assert_eq!((limits.max_secs, limits.max_bytes), (60.0, 4_000_000_000));
        assert_eq!(notes.len(), 2, "{notes:?}");
        assert!(notes[0].contains("max_seconds = -1"), "{notes:?}");
    }

    #[test]
    fn tinysa_settings_default_and_round_trip() {
        let defaults: EngineConfig = toml::from_str("").unwrap();
        assert_eq!(defaults.tinysa, TinySaSettings::default());

        let source = r#"
            [tinysa]
            basic_input = "high"
            points = 900
            rbw = "0.2"
            attenuation = "12"
            high_attenuation = true
            lna = true
            spur = "off"
            ext_gain_db = -8
        "#;
        let config: EngineConfig = toml::from_str(source).unwrap();
        let serialized = toml::to_string_pretty(&config).unwrap();
        let restored: EngineConfig = toml::from_str(&serialized).unwrap();
        assert_eq!(restored.tinysa, config.tinysa);
        assert!(serialized.contains("[tinysa]"));
        assert!(serialized.contains("basic_input = \"high\""));
        assert!(restored.tinysa.high_attenuation);
    }

    /// The HF+'s library DSP is on unless the file says off, and only an
    /// off is written: a config that never met the radio carries no section.
    #[test]
    fn plutosky_settings_default_to_slow_attack_and_round_trip() {
        let defaults = EngineConfig::default();
        assert_eq!(defaults.plutosky.agc, "slow_attack");
        assert!(!toml::to_string(&defaults).unwrap().contains("[plutosky]"));
        let config: EngineConfig = toml::from_str("[plutosky]\nagc = \"fast_attack\"\n").unwrap();
        let serialized = toml::to_string(&config).unwrap();
        assert!(serialized.contains("[plutosky]"), "{serialized}");
        let restored: EngineConfig = toml::from_str(&serialized).unwrap();
        assert_eq!(restored.plutosky.agc, "fast_attack");
    }

    #[test]
    fn airspyhf_settings_default_on_and_round_trip_off() {
        let defaults: EngineConfig = toml::from_str("").unwrap();
        assert!(defaults.airspyhf.dsp);
        assert!(!toml::to_string_pretty(&defaults)
            .unwrap()
            .contains("[airspyhf]"));
        let empty: EngineConfig = toml::from_str("[airspyhf]\n").unwrap();
        assert!(empty.airspyhf.dsp);

        let config: EngineConfig = toml::from_str("[airspyhf]\ndsp = false\n").unwrap();
        let serialized = toml::to_string_pretty(&config).unwrap();
        assert!(serialized.contains("[airspyhf]"), "{serialized}");
        let restored: EngineConfig = toml::from_str(&serialized).unwrap();
        assert!(!restored.airspyhf.dsp);
    }

    /// The boost is kept per radio by name and written only when there is one;
    /// the single `amp_enabled` it replaced is read past and not written.
    #[test]
    fn the_boost_is_per_radio_and_the_old_flag_is_let_go() {
        let defaults: EngineConfig = toml::from_str("").unwrap();
        assert!(!toml::to_string_pretty(&defaults).unwrap().contains("boost"));

        let source = r#"
            [radio]
            amp_enabled = true

            [radio.boost]
            "HackRF One" = true
            "Airspy HF+ Discovery" = false
        "#;
        let config: EngineConfig = toml::from_str(source).unwrap();
        assert!(config.radio.boost_for("HackRF One"));
        assert!(!config.radio.boost_for("Airspy HF+ Discovery"));
        assert!(
            !config.radio.boost_for("Airspy R2"),
            "the old flag starts nobody"
        );
        let serialized = toml::to_string_pretty(&config).unwrap();
        assert!(!serialized.contains("amp_enabled"), "{serialized}");
        assert!(serialized.contains("[radio.boost]"), "{serialized}");
        let restored: EngineConfig = toml::from_str(&serialized).unwrap();
        assert_eq!(restored.radio.boost, config.radio.boost);
    }

    #[test]
    fn partial_tinysa_settings_fill_field_defaults() {
        let config: EngineConfig = toml::from_str("[tinysa]\npoints = 64\n").unwrap();
        assert_eq!(config.tinysa.points, 64);
        assert_eq!(
            config.tinysa.basic_input,
            crate::hardware::tinysa::BasicInput::Low
        );
        assert_eq!(config.tinysa.rbw, "auto");
        assert_eq!(config.tinysa.attenuation, "auto");
        assert!(!config.tinysa.high_attenuation);
        assert!(!config.tinysa.lna);
        assert_eq!(config.tinysa.spur, "auto");
        assert_eq!(config.tinysa.ext_gain_db, 0);
    }

    #[test]
    fn radio_recall_round_trips_and_defaults_empty() {
        // Missing key → all-empty slots.
        let cfg: EngineConfig = toml::from_str("[radio]\nfrequency_hz = 100_000_000\n").unwrap();
        assert_eq!(cfg.radio.recall_hz, [0, 0, 0]);
        // Explicit slots survive a save/load round trip.
        let mut cfg = EngineConfig::default();
        cfg.radio.recall_hz = [92_800_000, 0, 446_006_000];
        let restored: EngineConfig =
            toml::from_str(&toml::to_string_pretty(&cfg).unwrap()).unwrap();
        assert_eq!(restored.radio.recall_hz, [92_800_000, 0, 446_006_000]);
    }

    #[test]
    fn the_station_survives_a_save_and_an_empty_one_is_not_written() {
        let written = EngineConfig {
            station: StationSettings {
                latitude: Some(47.497),
                longitude: Some(19.04),
                callsign: Some("HA5ZZT".into()),
            },
            ..EngineConfig::default()
        };
        let path =
            std::env::temp_dir().join(format!("sdrtop-station-save-{}.toml", std::process::id()));
        written.save(&path).unwrap();
        let back = load_or_default::<EngineConfig>(&path);
        let text = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(back.station, written.station);
        assert!(text.contains("[station]"), "{text}");

        let none = toml::to_string(&EngineConfig::default()).unwrap();
        assert!(!none.contains("[station]"), "{none}");
    }

    #[test]
    fn a_hand_written_station_reads_as_written() {
        let cfg: EngineConfig = toml::from_str(
            "[station]\nlatitude = -33.87\nlongitude = 151.21\ncallsign = \"vk2abc\"\n",
        )
        .unwrap();
        assert_eq!(cfg.station.latitude, Some(-33.87));
        assert_eq!(cfg.station.longitude, Some(151.21));
        // Kept as written; the state checks and upper-cases it.
        assert_eq!(cfg.station.callsign.as_deref(), Some("vk2abc"));
    }

    /// A file of its own for one test, gone when the test is.
    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new(tag: &str, text: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("sdrtop-owners-{tag}-{}.toml", std::process::id()));
            std::fs::write(&path, text).unwrap();
            Self(path)
        }

        fn read(&self) -> toml::Table {
            std::fs::read_to_string(&self.0).unwrap().parse().unwrap()
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    /// A block back at its default is written as absent, as a whole-file
    /// save writes it: the owner's word on its block is the whole word.
    #[test]
    fn a_block_back_at_its_default_leaves_the_file() {
        let file = Scratch::new("default", "[pager]\nnight = true\n");
        EngineConfig::default().save(&file.0).unwrap();
        assert!(file.read().get("pager").is_none());
    }

    /// A file that is not TOML has no blocks to keep: it is written fresh,
    /// as a whole-file save always did, rather than refused.
    #[test]
    fn a_broken_file_is_written_fresh() {
        let file = Scratch::new("broken", "this is [not toml");
        EngineConfig::default().save(&file.0).unwrap();
        assert!(file.read().get("radio").is_some());
    }

    /// The blocks `struct name` holds of its own, read off this file's source.
    fn blocks_of(name: &str) -> Vec<&'static str> {
        let src = include_str!("config.rs");
        let body = src
            .split_once(&format!("pub struct {name} {{"))
            .unwrap()
            .1
            .split_once("\n}")
            .unwrap()
            .0;
        // A flattened field is a half, not a block of the file.
        let mut flattened = false;
        let mut blocks = vec![];
        for line in body.lines().map(str::trim) {
            if line == "#[serde(flatten)]" {
                flattened = true;
            } else if let Some((name, _)) =
                line.strip_prefix("pub ").and_then(|l| l.split_once(':'))
            {
                if !std::mem::take(&mut flattened) {
                    blocks.push(name);
                }
            }
        }
        blocks
    }

    /// **The engine's struct holds exactly the blocks it says it owns**: one
    /// added to the struct and not to [`ENGINE_BLOCKS`] would never be saved,
    /// and the other way round would be saved from nothing. The terminal's
    /// tests hold the other owners to theirs and all three apart.
    #[test]
    fn the_engine_holds_exactly_its_blocks() {
        let mut engine = blocks_of("EngineConfig");
        assert!(engine.len() > 10, "{engine:?}");
        let mut list = ENGINE_BLOCKS.to_vec();
        engine.sort_unstable();
        list.sort_unstable();
        assert_eq!(engine, list);
    }
}
