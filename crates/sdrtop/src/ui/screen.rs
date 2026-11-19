// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

use std::time::{Duration, Instant};

use crate::state::{MicroView, SdrMetrics};

/// What a panel draws from: the engine's state and the screen's own,
/// together.
///
/// The engine's state ([`SdrMetrics`]) holds what is measured; the screen's
/// ([`UiState`]) holds what the terminal is doing with it (its layout, focus,
/// menu, the text being typed). They are two things, owned by two sides, and
/// a panel reads both. **`View` dereferences to the engine's state**, so a
/// panel writes `state.radio.frequency` and `state.ui.section` exactly as it
/// did when the screen's state was a field of the engine's: the line moved,
/// not the hundreds of places that read across it. `ui` is a field of the
/// view, never of [`SdrMetrics`].
#[derive(Clone)]
pub struct View<'a> {
    m: &'a SdrMetrics,
    pub ui: std::borrow::Cow<'a, UiState>,
}

impl<'a> View<'a> {
    pub fn new(m: &'a SdrMetrics, ui: &'a UiState) -> Self {
        Self {
            m,
            ui: std::borrow::Cow::Borrowed(ui),
        }
    }
}

#[cfg(test)]
impl View<'static> {
    /// The view of `m` as the terminal would draw it: the layout and section
    /// on screen are the ones `m`'s mode was asked for (the app sets both
    /// from one layout, every frame), everything else as a fresh terminal
    /// has it. For a test that sets the view with `show_view` and has
    /// nothing to say about focus, menu or typing.
    pub fn of(m: &SdrMetrics) -> View<'_> {
        View {
            m,
            ui: std::borrow::Cow::Owned(UiState {
                section: m.mode.section.clone(),
                active_preset: m.mode.view.clone(),
                ..UiState::default()
            }),
        }
    }
}

impl std::ops::Deref for View<'_> {
    type Target = SdrMetrics;
    fn deref(&self) -> &SdrMetrics {
        self.m
    }
}

/// The engine's state and the screen's, for the keys.
///
/// The engine's state is shared with every worker behind its mutex; the
/// screen's lives on the terminal's thread alone, so a `RefCell` is all it
/// needs. [`Screen::lock`] takes both, the one way the keys reach either.
pub struct Screen {
    pub m: std::sync::Arc<std::sync::Mutex<SdrMetrics>>,
    pub ui: std::cell::RefCell<UiState>,
}

impl Screen {
    /// A screen over `m` alone, as a fresh terminal has it: for tests.
    #[cfg(test)]
    pub fn of(m: SdrMetrics) -> Self {
        Self::new(
            std::sync::Arc::new(std::sync::Mutex::new(m)),
            UiState::default(),
        )
    }

    pub fn new(m: std::sync::Arc<std::sync::Mutex<SdrMetrics>>, ui: UiState) -> Self {
        Self {
            m,
            ui: std::cell::RefCell::new(ui),
        }
    }

    /// The engine's state locked, and the screen's borrowed, together. A
    /// poisoned mutex is recovered, never propagated: a panic in one handler
    /// must not take the terminal down on the next key.
    pub fn lock(&self) -> Locked<'_> {
        Locked {
            m: self.m.lock().unwrap_or_else(|e| e.into_inner()),
            ui: self.ui.borrow_mut(),
        }
    }
}

/// What [`Screen::lock`] gives: the engine's state, dereferenced as itself,
/// and the screen's as `ui`, so a key handler writes `m.ui.input_buf` and
/// `m.radio.frequency` side by side as it always has. See [`View`] for why.
pub struct Locked<'a> {
    m: std::sync::MutexGuard<'a, SdrMetrics>,
    pub ui: std::cell::RefMut<'a, UiState>,
}

impl Locked<'_> {
    /// What a panel would draw from right now, for a key that asks a panel's
    /// own logic (which row is selected, what is on screen).
    pub fn view(&self) -> View<'_> {
        View::new(&self.m, &self.ui)
    }
}

impl std::ops::Deref for Locked<'_> {
    type Target = SdrMetrics;
    fn deref(&self) -> &SdrMetrics {
        &self.m
    }
}

impl std::ops::DerefMut for Locked<'_> {
    fn deref_mut(&mut self) -> &mut SdrMetrics {
        &mut self.m
    }
}

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

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum DeviceOptionUpdate {
    #[default]
    Idle,
    Pending {
        request: crate::event::DeviceOptionRequest,
        quit_requested: bool,
    },
    Failed {
        id: String,
    },
}

impl DeviceOptionUpdate {
    pub fn is_pending(&self) -> bool {
        matches!(self, Self::Pending { .. })
    }

    pub fn quit_requested(&self) -> bool {
        matches!(
            self,
            Self::Pending {
                quit_requested: true,
                ..
            }
        )
    }

    pub fn request_quit(&mut self) -> bool {
        let Self::Pending { quit_requested, .. } = self else {
            return false;
        };
        let already_requested = *quit_requested;
        *quit_requested = true;
        already_requested
    }
}

#[derive(Clone, PartialEq)]
pub enum InputMode {
    Normal,
    DeviceOptionInput {
        id: String,
        error: Option<String>,
    },
    /// A `[station]` setting, typed in the Options pane.
    StationInput {
        field: crate::state::StationField,
        error: Option<String>,
    },
    FrequencyInput,
    SampleRateInput,
    MarkerNameInput,
    SweepStartInput,
    SweepStopInput,
    /// How far the user trusts the census device at `address` as a frequency
    /// reference, in ppm.
    ReferenceAccuracyInput {
        address: [u8; 6],
    },
}

#[derive(Clone)]
pub struct UiState {
    pub input_mode: InputMode,
    pub input_buf: String,
    pub focused_panel: Option<String>,
    /// Which gain stage the command rail's focus mode is pointed at, if any.
    ///
    /// `None` means the knob is the whole chain, which is what it has always
    /// been. Selecting a stage narrows `↑`/`↓` to that one and leaves the others
    /// alone, and **leaving the mode does not put them back**: a stage set by
    /// hand stays set until the one-knob control is used again, because undoing
    /// someone's deliberate setting on the way out is worse than keeping it.
    pub gain_stage: Option<usize>,
    pub focused_panel_bindings: &'static [(&'static str, &'static str)],
    /// Name of the engine's active preset, synced each frame before draw so the
    /// footer can show it. The engine owns the authoritative value; this is a
    /// render-time mirror.
    pub active_preset: String,
    /// The menu section the active preset is filed under, synced each frame
    /// alongside `active_preset`.
    ///
    /// The header renders differently inside the NET section, and this is what
    /// it asks. Mirrored from the engine rather than derived from the preset's
    /// name, because a name prefix would make the section a convention that a
    /// user preset could break by being called something sensible; the section
    /// is a field a preset **declares**, and this is that field.
    pub section: String,
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
    /// Each entry is `(slot, preset name, the menu's title for it)`.
    pub scope: Vec<(Option<u8>, String, String)>,
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
    pub device_option_update: DeviceOptionUpdate,
    /// The screen's width in columns as of this frame, set by the app before
    /// it draws, so a panel that gives way on a narrow screen and the panels
    /// that take over what it showed decide alike. Zero, unknown, reads as
    /// wide.
    pub screen_cols: u16,
}

/// The menu's tiles, in the order they stand on screen.
///
/// Resume is listed only when there is something to resume
/// ([`MenuState::resume`]). How each is drawn is `ui::menu::tiles`'.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum MenuTile {
    Resume,
    Radio,
    #[default]
    Lab,
    /// Settings exposed by the active device.
    Options,
    /// The key reference. Replaced the `?` overlay, which had drifted out of
    /// step with the dispatch because nothing checked it.
    Keys,
    Quit,
}

/// Where the keys go: the row of tiles, or what the selected tile opens
/// below it (a tree, the options, the key reference).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum MenuFocus {
    #[default]
    Tiles,
    Below,
}

/// A place in one of the menu's trees: a row index in each column. Fixed
/// size, so [`MenuState`] stays `Copy` and is cloned into the frame snapshot
/// without an allocation; the deepest path the trees have is four.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct MenuPath {
    rows: [u8; MenuPath::DEPTH],
    len: u8,
}

impl MenuPath {
    pub const DEPTH: usize = 8;

    /// Deeper than [`Self::DEPTH`] is cut there, and an index past 255 is
    /// held at 255: either way the renderer clamps it onto a real row.
    pub fn from_rows(rows: &[usize]) -> Self {
        let mut out = Self::default();
        for (slot, &i) in out.rows.iter_mut().zip(rows) {
            *slot = i.min(u8::MAX as usize) as u8;
        }
        out.len = rows.len().min(Self::DEPTH) as u8;
        out
    }

    pub fn rows(&self) -> Vec<usize> {
        self.rows[..self.len as usize]
            .iter()
            .map(|&i| i as usize)
            .collect()
    }
}

/// Where the cursor is while the menu is open.
///
/// Each tree keeps its own path, so moving between the Radio and Lab tiles
/// comes back to where you were in each. The paths index the engine's
/// `menu::tree::Tree`, which it builds once; the renderer still clamps them
/// rather than trusting them, because this is cloned into every frame's
/// snapshot and a panic during draw takes the terminal with it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct MenuState {
    pub tile: MenuTile,
    pub focus: MenuFocus,
    pub radio: MenuPath,
    pub lab: MenuPath,
    /// Whether there is a view to resume: in a session always, at startup
    /// only when a saved config named one.
    pub resume: bool,
    /// Scroll position in Keys or selected row in Options.
    pub scroll: usize,
}

impl UiState {
    /// Whether the active preset is a measurement lab (`lab_*`). Lab presets wear
    /// the instrument-chrome (banner + marker bar) and a cooler steel frame.
    /// Reads the per-frame `active_preset` mirror, so it is valid during draw.
    /// Whether the deck is in one of the NET sections, which the header
    /// varies on.
    pub fn is_net_section(&self) -> bool {
        crate::signal::net::is_net(&self.section)
    }

    /// Whether the deck is on a pager view, which turns the pager feed on.
    pub fn is_pager_section(&self) -> bool {
        self.section == crate::signal::pocsag::SECTION
    }

    /// Whether the deck is on an FT8 view, World or the Lab, which turns
    /// the FT8 feed on.
    pub fn is_ft8_section(&self) -> bool {
        crate::signal::ft8::SECTIONS.contains(&self.section.as_str())
    }

    /// Whether the deck is on the FT8 Lab rather than World.
    pub fn is_ft8_lab(&self) -> bool {
        self.section == crate::signal::ft8::LAB_SECTION
    }

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
            screen_cols: 0,
            input_mode: InputMode::Normal,
            input_buf: String::new(),
            focused_panel: None,
            gain_stage: None,
            focused_panel_bindings: &[],
            active_preset: String::new(),
            section: String::new(),
            preset_names: Vec::new(),
            scope: Vec::new(),
            rail_mode: RailMode::default(),
            rail_mode_auto: false,
            last_mode_action: None,
            recall: [None; RECALL_SLOTS],
            recall_cursor: 0,
            log_overlay: false,
            menu: None,
            device_option_update: DeviceOptionUpdate::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **Monas can compute what every built-in layout shows.** Each layout,
    /// in the section it declares, asks for what some named Monas mode asks
    /// for too (same feeds, same NET view, same FT8 lab), so `monasd` can run
    /// what any terminal view runs; and a layout that shares a name with a
    /// Monas mode computes the same thing by it.
    #[test]
    fn every_built_in_layout_asks_for_a_mode_monas_knows() {
        use crate::engine::mode::{Mode, MODES};
        let computes = |m: &Mode| (m.feeds, m.net, m.ft8_lab);
        let named: Vec<Mode> = MODES.iter().map(|(n, _)| Mode::named(n).unwrap()).collect();
        let layouts = crate::config::LayoutConfig::default_config();
        for (preset, cfg) in &layouts.presets {
            let section = cfg.section.clone().unwrap_or_default();
            let asked = Mode::for_view(&section, preset);
            assert!(
                named.iter().any(|m| computes(m) == computes(&asked)),
                "{preset} in {section} asks for a mode Monas has no name for"
            );
            if let Some(m) = Mode::named(preset) {
                assert_eq!(
                    computes(&m),
                    computes(&asked),
                    "{preset}: one name, two modes"
                );
            }
        }
    }

    /// **The screen and the engine read one view the same way.** Every
    /// built-in layout, with the section it declares, asks the engine for
    /// exactly the feeds the screen's own description of that section says
    /// it shows. If the two ever disagree, a view stops being fed or starts
    /// costing a core, and nothing on screen says which.
    #[test]
    fn every_built_in_layout_asks_the_engine_for_what_the_screen_says_it_shows() {
        use crate::config::LayoutConfig;
        use crate::engine::mode::Mode;
        use crate::signal::net::lock::{
            ADVERTISING_VIEWS, CLASSIC_VIEWS, CODED_VIEWS, SURVEY_VIEW, WIFI_VISIT_VIEWS,
        };
        let layouts = LayoutConfig::default_config();
        assert!(
            layouts.presets.len() > 20,
            "the built-in layouts were not found"
        );
        for (preset, cfg) in &layouts.presets {
            let section = cfg.section.clone().unwrap_or_default();
            let ui = UiState {
                section: section.clone(),
                active_preset: preset.clone(),
                ..UiState::default()
            };
            let mode = Mode::for_view(&section, preset);
            let at = format!("{preset} in {section}");
            assert_eq!(mode.section, section, "{at}");
            assert_eq!(mode.view, *preset, "{at}");
            assert_eq!(mode.feeds.net, ui.is_net_section(), "{at}: net feed");
            assert_eq!(mode.feeds.pager, ui.is_pager_section(), "{at}: pager feed");
            assert_eq!(mode.feeds.ft8, ui.is_ft8_section(), "{at}: ft8 feed");
            assert_eq!(mode.ft8_lab, ui.is_ft8_lab(), "{at}: ft8 lab");
            let p = preset.as_str();
            assert_eq!(
                mode.net.classic,
                CLASSIC_VIEWS.contains(&p),
                "{at}: classic"
            );
            assert_eq!(mode.net.survey, p == SURVEY_VIEW, "{at}: survey");
            assert_eq!(mode.net.coded, CODED_VIEWS.contains(&p), "{at}: coded");
            assert_eq!(
                mode.net.wifi,
                crate::signal::net::is_wifi(&section),
                "{at}: wifi"
            );
            assert_eq!(
                mode.net.wifi_visits,
                WIFI_VISIT_VIEWS.contains(&p),
                "{at}: wifi visits"
            );
            assert_eq!(
                mode.net.advertising,
                ADVERTISING_VIEWS.contains(&p),
                "{at}: advertising"
            );
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

    /// The band's section and each Bluetooth's are all NET.
    #[test]
    fn every_net_section_is_net() {
        let mut ui = UiState::default();
        for s in ["net", "le", "classic", "coded"] {
            ui.section = s.to_string();
            assert!(ui.is_net_section(), "{s}");
        }
        ui.section = "lab".to_string();
        assert!(!ui.is_net_section());
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
