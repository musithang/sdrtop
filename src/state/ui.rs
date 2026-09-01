use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::MicroView;

pub const LOG_MAX_ENTRIES: usize = 100;

/// Which lead view the Command Rail's mode-card shows. It auto-follows what you
/// are doing - tuning relaxes to **Hunt** (find signals), gain changes to
/// **Bench** (set up the chain) - and falls back to **Monitor** when idle. `Tab`
/// in rail-focus pins a mode manually (no auto-decay).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum RailMode {
    Hunt,
    #[default]
    Monitor,
    Bench,
}

impl RailMode {
    /// The full-width tab label.
    pub fn label(self) -> &'static str {
        match self {
            RailMode::Hunt => "HUNT",
            RailMode::Monitor => "MONITOR",
            RailMode::Bench => "BENCH",
        }
    }

    /// Left-to-right tab order, also the manual `Tab` cycle order.
    pub const ALL: [RailMode; 3] = [RailMode::Hunt, RailMode::Monitor, RailMode::Bench];

    /// Next mode in the `Tab` cycle: HUNT → MONITOR → BENCH → HUNT.
    pub fn next(self) -> RailMode {
        match self {
            RailMode::Hunt => RailMode::Monitor,
            RailMode::Monitor => RailMode::Bench,
            RailMode::Bench => RailMode::Hunt,
        }
    }
}

/// How long an auto-set mode (Hunt/Bench) lingers before relaxing back to
/// Monitor.
pub const RAIL_MODE_DECAY: Duration = Duration::from_secs(8);

/// The mode to actually render, after decay: an auto-set Hunt/Bench falls back
/// to Monitor once it's been idle past [`RAIL_MODE_DECAY`]; a manually-pinned
/// mode (`auto == false`) never decays. Pure so it's testable without a clock.
pub fn decayed_mode(mode: RailMode, auto: bool, since: Option<Duration>) -> RailMode {
    match (auto, since) {
        (true, Some(d)) if d >= RAIL_MODE_DECAY => RailMode::Monitor,
        _ => mode,
    }
}

/// Number of Command Rail recall slots (`M` save / `1·2·3` jump).
pub const RECALL_SLOTS: usize = 3;

/// A recalled frequency this many Hz from the current tuning counts as "parked
/// on" that slot - the device may round a tuned frequency slightly.
pub const RECALL_MATCH_HZ: u64 = 1_000;

/// Which slot a save should write: the lowest empty slot, or, when all are full,
/// the rotating `cursor` (oldest-overwrite). Pure for testability.
pub fn next_recall_slot(slots: &[Option<u64>; RECALL_SLOTS], cursor: usize) -> usize {
    slots
        .iter()
        .position(Option::is_none)
        .unwrap_or(cursor % RECALL_SLOTS)
}

/// The slot the radio is currently parked on (within [`RECALL_MATCH_HZ`]), if any.
pub fn active_recall_slot(slots: &[Option<u64>; RECALL_SLOTS], freq: u64) -> Option<usize> {
    slots
        .iter()
        .position(|s| s.is_some_and(|hz| hz.abs_diff(freq) <= RECALL_MATCH_HZ))
}

/// Config `recall_hz` (0 = empty) → in-memory slots.
pub fn recall_from_hz(hz: [u64; RECALL_SLOTS]) -> [Option<u64>; RECALL_SLOTS] {
    hz.map(|h| (h != 0).then_some(h))
}

/// In-memory slots → config `recall_hz` (None = 0).
pub fn recall_to_hz(slots: &[Option<u64>; RECALL_SLOTS]) -> [u64; RECALL_SLOTS] {
    slots.map(|s| s.unwrap_or(0))
}

/// Severity of a log line, used by the log panel to pick a status lamp + colour.
/// Derived from the message text (see [`LogLevel::infer`]) so the ~86 existing
/// `push_log` call sites keep working unchanged.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LogLevel {
    Info,
    Ok,
    Warn,
    Error,
}

impl LogLevel {
    /// Classify a message by keyword. Order matters: hard failures win over the
    /// "warning" prefix, which wins over success words. Everything else is Info.
    pub fn infer(msg: &str) -> LogLevel {
        let m = msg.to_ascii_lowercase();
        let has = |needles: &[&str]| needles.iter().any(|n| m.contains(n));
        if has(&["error", "fail", "unable", "in use", "could not"]) {
            LogLevel::Error
        } else if has(&["warning", "warn", "no marker", "unexpectedly"]) {
            LogLevel::Warn
        } else if has(&["started", "connected", "enabled", "success"]) {
            LogLevel::Ok
        } else {
            LogLevel::Info
        }
    }
}

/// One structured log line: when it happened (Unix epoch seconds, captured at
/// push time so the timestamp is fixed), its severity, and the message text.
#[derive(Clone)]
pub struct LogEntry {
    pub at_epoch_secs: u64,
    pub level: LogLevel,
    pub text: Arc<str>,
}

#[derive(Clone, PartialEq)]
pub enum InputMode {
    Normal,
    FrequencyInput,
    SampleRateInput,
    MarkerNameInput,
    SweepStartInput,
    SweepStopInput,
}

#[derive(Clone)]
pub struct UiState {
    pub input_mode: InputMode,
    pub input_buf: String,
    pub focused_panel: Option<String>,
    pub focused_panel_bindings: &'static [(&'static str, &'static str)],
    /// Name of the engine's active preset, synced each frame before draw so the
    /// footer can show it. The engine owns the authoritative value; this is a
    /// render-time mirror.
    pub active_preset: String,
    /// Names of all defined presets, synced each frame alongside active_preset.
    /// Lets the footer build the lab map from presets that actually exist.
    pub preset_names: Vec<String>,
    /// The active section's layouts as `(slot, preset name)`, synced each frame
    /// alongside `active_preset`. Empty for a preset the menu hides.
    ///
    /// The footer needs to name the keys that work *right now*, and the keys are
    /// scoped, so it cannot hold a table of its own. It used to: `LAB_FAMILY`
    /// mapped `[5]`-`[8]` to the four lab benches, and the moment the digits
    /// became section-relative that table started advertising keys that do
    /// nothing. Mirroring the real thing is the same trick `active_preset`
    /// already uses to keep the footer out of the engine.
    pub scope: Vec<(Option<u8>, String)>,
    pub log: VecDeque<LogEntry>,
    /// Command Rail lead-view mode (Hunt/Monitor/Bench). See [`RailMode`].
    pub rail_mode: RailMode,
    /// Whether `rail_mode` was set by auto-follow (decays) vs. pinned by `Tab`.
    pub rail_mode_auto: bool,
    /// When the last auto mode-change happened, for the idle decay to Monitor.
    pub last_mode_action: Option<Instant>,
    /// Command Rail recall slots; `None` is empty. See [`next_recall_slot`].
    pub recall: [Option<u64>; RECALL_SLOTS],
    /// Rotation pointer for overwriting once all recall slots are full.
    pub recall_cursor: usize,
    /// Whether the Command Rail's full-log overlay (`L` in rail-focus) is open.
    pub log_overlay: bool,
    /// Where the cursor is while the menu is open, `None` when it is closed.
    /// See [`MenuState`].
    pub menu: Option<MenuState>,
}

/// Which pane the menu's right column is showing.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum MenuPane {
    /// The selected section's layouts. The only pane the number keys work in.
    #[default]
    Views,
    /// The key reference. Replaces the `?` overlay, which had drifted out of
    /// step with the dispatch because nothing checked it.
    Keys,
    /// Settings. Empty so far, and it says so on screen. The variant exists
    /// ahead of its first row so that adding one is a row rather than a
    /// reshuffle of the enum, the column and the dispatch together.
    Options,
}

/// Where the cursor is while the menu is open.
///
/// The cursor sits on a **view**, never on a section heading. That is what lets
/// `Enter` mean exactly one thing, and it is why resume needs no key of its own:
/// the menu opens with the cursor already on the active preset, so `Enter` puts
/// you back where you were using the same key that opens anything else.
///
/// Indices into `menu::model::Menu`, which the engine owns and rebuilds never.
/// A stale index cannot outlive the table it points into, but the renderer still
/// clamps rather than trusting them, because this is cloned into every frame's
/// snapshot and a panic during draw takes the terminal with it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct MenuState {
    pub section: usize,
    pub entry: usize,
    pub pane: MenuPane,
    /// First visible row of the [`MenuPane::Keys`] list.
    ///
    /// Its own field rather than reusing `entry`: the reference is taller than a
    /// 24 row terminal, so it has to scroll, and one field meaning two things
    /// depending on the pane is how a cursor ends up somewhere nobody expected.
    pub scroll: usize,
}

impl UiState {
    pub fn push_log(&mut self, msg: impl Into<String>) {
        let text = msg.into();
        let level = LogLevel::infer(&text);
        let at_epoch_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if self.log.len() >= LOG_MAX_ENTRIES {
            self.log.pop_front();
        }
        self.log.push_back(LogEntry {
            at_epoch_secs,
            level,
            text: Arc::from(text),
        });
    }

    /// Whether the active preset is a measurement lab (`lab_*`). Lab presets wear
    /// the instrument-chrome (banner + marker bar) and a cooler steel frame.
    /// Reads the per-frame `active_preset` mirror, so it is valid during draw.
    pub fn is_lab_mode(&self) -> bool {
        self.active_preset.starts_with("lab_")
    }

    /// Which micro view is on screen, read off the active preset exactly the way
    /// [`UiState::is_lab_mode`] reads the lab chrome.
    ///
    /// This used to be a stored field. The old `[0]` cycle kept it in step and nothing
    /// else did, so any other route into a micro preset left the footer showing
    /// the position and key hints of a view that was not on screen. Reading the
    /// preset removes the possibility rather than fixing the instance.
    ///
    /// Reads the per-frame `active_preset` mirror, so it is valid during draw.
    pub fn micro_view(&self) -> MicroView {
        MicroView::from_preset(&self.active_preset).unwrap_or_default()
    }

    /// Record an auto-follow mode change (tuning → Hunt, gain → Bench): set the
    /// mode and (re)start its idle decay timer.
    pub fn note_mode_action(&mut self, mode: RailMode) {
        self.rail_mode = mode;
        self.rail_mode_auto = true;
        self.last_mode_action = Some(Instant::now());
    }

    /// Manual `Tab` cycle in rail-focus: pin the next mode so it won't decay.
    pub fn cycle_rail_mode(&mut self) -> RailMode {
        self.rail_mode = self.rail_mode.next();
        self.rail_mode_auto = false;
        self.last_mode_action = None;
        self.rail_mode
    }

    /// The mode to render right now, after applying the idle decay.
    pub fn effective_rail_mode(&self) -> RailMode {
        decayed_mode(
            self.rail_mode,
            self.rail_mode_auto,
            self.last_mode_action.map(|t| t.elapsed()),
        )
    }

    /// Store `freq` in the next recall slot (free slot, else oldest), advance the
    /// rotation cursor, and return the slot index for the log message.
    pub fn save_recall(&mut self, freq: u64) -> usize {
        let slot = next_recall_slot(&self.recall, self.recall_cursor);
        self.recall[slot] = Some(freq);
        self.recall_cursor = (slot + 1) % RECALL_SLOTS;
        slot
    }
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            input_mode: InputMode::Normal,
            input_buf: String::new(),
            focused_panel: None,
            focused_panel_bindings: &[],
            active_preset: String::new(),
            preset_names: Vec::new(),
            scope: Vec::new(),
            log: VecDeque::new(),
            rail_mode: RailMode::default(),
            rail_mode_auto: false,
            last_mode_action: None,
            recall: [None; RECALL_SLOTS],
            recall_cursor: 0,
            log_overlay: false,
            menu: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infer_flags_failures_as_error() {
        for msg in [
            "Tune error: out of range",
            "Error stopping RX: timeout",
            "Startup: failed to set gain",
            "Device is in use by another process",
            "Reset error: denied",
        ] {
            assert_eq!(LogLevel::infer(msg), LogLevel::Error, "{msg:?}");
        }
    }

    #[test]
    fn infer_flags_warnings() {
        for msg in [
            "WARNING: Streaming stopped unexpectedly — press [Space]",
            "No marker near cursor — place one with [M] first",
        ] {
            assert_eq!(LogLevel::infer(msg), LogLevel::Warn, "{msg:?}");
        }
    }

    #[test]
    fn infer_flags_success_events() {
        assert_eq!(LogLevel::infer("RX streaming started"), LogLevel::Ok);
        assert_eq!(
            LogLevel::infer("Connected: HackRF One | Serial: …"),
            LogLevel::Ok
        );
    }

    #[test]
    fn infer_defaults_to_info() {
        for msg in [
            "Step → 100 kHz",
            "Freq zoom: ×4",
            "Preset: spectrum",
            "RX streaming stopped",
        ] {
            assert_eq!(LogLevel::infer(msg), LogLevel::Info, "{msg:?}");
        }
    }

    #[test]
    fn rail_mode_cycles_hunt_monitor_bench() {
        assert_eq!(RailMode::Hunt.next(), RailMode::Monitor);
        assert_eq!(RailMode::Monitor.next(), RailMode::Bench);
        assert_eq!(RailMode::Bench.next(), RailMode::Hunt);
        assert_eq!(RailMode::default(), RailMode::Monitor);
        assert_eq!(
            RailMode::ALL,
            [RailMode::Hunt, RailMode::Monitor, RailMode::Bench]
        );
    }

    #[test]
    fn auto_mode_decays_to_monitor_when_idle() {
        // Fresh auto-set Hunt holds; past the decay window it relaxes to Monitor.
        assert_eq!(
            decayed_mode(RailMode::Hunt, true, Some(Duration::from_secs(2))),
            RailMode::Hunt
        );
        assert_eq!(
            decayed_mode(RailMode::Hunt, true, Some(RAIL_MODE_DECAY)),
            RailMode::Monitor
        );
        // A manually-pinned mode never decays.
        assert_eq!(
            decayed_mode(RailMode::Bench, false, Some(Duration::from_secs(999))),
            RailMode::Bench
        );
        // No timer yet → whatever the mode is.
        assert_eq!(decayed_mode(RailMode::Bench, true, None), RailMode::Bench);
    }

    #[test]
    fn note_and_cycle_set_auto_flag() {
        let mut ui = UiState::default();
        ui.note_mode_action(RailMode::Hunt);
        assert_eq!(ui.rail_mode, RailMode::Hunt);
        assert!(ui.rail_mode_auto && ui.last_mode_action.is_some());
        // Tab pins the next mode and clears the decay timer.
        assert_eq!(ui.cycle_rail_mode(), RailMode::Monitor);
        assert!(!ui.rail_mode_auto && ui.last_mode_action.is_none());
    }

    #[test]
    fn recall_save_fills_empty_then_rotates_oldest() {
        let mut ui = UiState::default();
        assert_eq!(ui.save_recall(92_800_000), 0);
        assert_eq!(ui.save_recall(145_500_000), 1);
        assert_eq!(ui.save_recall(446_006_000), 2);
        assert_eq!(
            ui.recall,
            [Some(92_800_000), Some(145_500_000), Some(446_006_000)]
        );
        // All full → overwrite the oldest (slot 0), then 1, …
        assert_eq!(ui.save_recall(100_000_000), 0);
        assert_eq!(ui.recall[0], Some(100_000_000));
        assert_eq!(ui.save_recall(101_000_000), 1);
    }

    #[test]
    fn active_recall_slot_matches_within_tolerance() {
        let slots = [Some(92_800_000), None, Some(446_006_000)];
        assert_eq!(active_recall_slot(&slots, 92_800_500), Some(0)); // within 1 kHz
        assert_eq!(active_recall_slot(&slots, 446_006_000), Some(2));
        assert_eq!(active_recall_slot(&slots, 145_500_000), None);
    }

    #[test]
    fn recall_hz_round_trips_through_config() {
        let slots = [Some(92_800_000), None, Some(446_006_000)];
        let hz = recall_to_hz(&slots);
        assert_eq!(hz, [92_800_000, 0, 446_006_000]);
        assert_eq!(recall_from_hz(hz), slots);
    }

    #[test]
    fn push_log_captures_level_and_time() {
        let mut ui = UiState::default();
        ui.push_log("Tune error: nope");
        let e = ui.log.back().unwrap();
        assert_eq!(e.level, LogLevel::Error);
        assert_eq!(e.text.as_ref(), "Tune error: nope");
        assert!(e.at_epoch_secs > 0, "timestamp captured");
    }

    /// The micro view follows the active preset **however that preset was
    /// reached**.
    ///
    /// This is the bug the field caused. The old `[0]` cycle wrote both the field and
    /// the preset, so `[0]` looked correct; every other route wrote only the
    /// preset, and the footer went on showing the position and key hints of the
    /// view you had left. Now there is nothing to write.
    #[test]
    fn the_micro_view_follows_the_active_preset_by_any_route() {
        let mut ui = UiState::default();
        for (preset, expected) in [
            ("micro_main", MicroView::Main),
            ("micro_gain", MicroView::Gain),
            ("micro_sweep", MicroView::Sweep),
            ("micro_health", MicroView::Health),
        ] {
            ui.active_preset = preset.to_string();
            assert_eq!(ui.micro_view(), expected, "reached {preset}");
        }
    }

    /// Off the micro family the answer is the default rather than whatever the
    /// last micro view happened to be, so a stale position cannot follow you onto
    /// a lab bench.
    #[test]
    fn a_non_micro_preset_reports_the_default_view() {
        let mut ui = UiState {
            active_preset: "micro_health".to_string(),
            ..Default::default()
        };
        assert_eq!(ui.micro_view(), MicroView::Health);
        ui.active_preset = "lab_iq".to_string();
        assert_eq!(ui.micro_view(), MicroView::default());
    }

    /// `is_lab_mode` and `micro_view` read the same field and must never both
    /// claim the deck.
    #[test]
    fn a_layout_is_not_both_a_lab_and_a_micro_view() {
        let mut ui = UiState::default();
        for preset in ["lab_iq", "lab_sweep", "micro_main", "spectrum"] {
            ui.active_preset = preset.to_string();
            let micro = MicroView::from_preset(preset).is_some();
            assert!(!(ui.is_lab_mode() && micro), "{preset} claims to be both");
        }
    }
}
