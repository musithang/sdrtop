// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

mod builder;
pub mod input;

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{backend::Backend, Terminal};

use crate::config::{AppConfig, DisplayConfig, RadioConfig};
use crate::event::{AppEvent, DeviceOptionCompletion, DeviceOptionRequest, EventStream};
use crate::hardware::{self, RxContext, SdrDevice};
use crate::state::SdrMetrics;
use crate::ui;

/// Focus letter → the panels that claim it, in name order.
///
/// **A letter may belong to more than one panel**, as long as no layout shows
/// two of them: a Lab bench's letter means nothing on a NET screen, and every
/// letter of the alphabet was taken before the NET section needed its own
/// (Viktor's rule, 2026-09-22, generalising the one that let `i` and `m` be
/// reused there). The key handler focuses the first of them that is on screen
/// (`input::global::view::enter_focus`); the name order makes that
/// deterministic even where a user preset does show two.
pub(crate) type FocusKeys = HashMap<char, Vec<&'static str>>;

pub struct App {
    pub(super) state: Arc<Mutex<SdrMetrics>>,
    pub(super) device: Option<Arc<dyn SdrDevice>>,
    #[allow(dead_code)]
    pub(super) rx_ctx: Option<Arc<RxContext>>,
    pub(super) config_path: Option<PathBuf>,
    pub(super) events: EventStream,
    pub(super) show_footer: bool,
    /// Whether the deck has been on screen yet this session.
    ///
    /// The menu is the first screen and owns the whole terminal there; once you
    /// have picked a layout it becomes a box floating over that layout. This is
    /// the only difference between the two, so it is a flag rather than a second
    /// renderer: same function, different `Rect`.
    pub(super) deck_shown: bool,
    pub(super) engine: ui::LayoutEngine,
    pub(super) theme: crate::Theme,
    pub(super) focus_keys: FocusKeys,
    /// User-defined presets as loaded from config.toml, kept so save_config can
    /// write them back verbatim instead of erasing hand-edited presets.
    pub(super) user_presets: HashMap<String, crate::config::PresetConfig>,
    /// The `[theme]` block exactly as it was loaded.
    ///
    /// Kept for the same reason as `user_presets`: `save_config` rewrites the
    /// whole file, so anything it does not carry forward is deleted. This block
    /// holds the per-field colour overrides, which the app reads once at startup
    /// and never touches again - so without a copy of them there is nothing left
    /// to write back.
    pub(super) theme_config: crate::config::ThemeConfig,
    pub(super) tinysa_config: crate::config::TinySaSettings,
    /// The `[net]` block exactly as it was loaded - `[net].bt_channels`
    /// has no in-app control that would change it while running, so the
    /// only way `save_config` can carry it forward is to hold the loaded
    /// value, the same reasoning `tinysa_config` already follows.
    pub(super) net_config: crate::config::NetSettings,
}

impl App {
    pub fn new(
        cfg: AppConfig,
        config_path: Option<PathBuf>,
        listing: &hardware::DeviceListing,
    ) -> anyhow::Result<Self> {
        match hardware::open_device(listing, &cfg.tinysa) {
            Ok(device) => {
                let observable = listing.kind.observer_profile().is_some();
                Self::new_normal(cfg, config_path, device, observable)
            }
            Err(open_err) => {
                // Device is present but couldn't be opened (e.g. busy) - fall back
                // to read-only observer mode via the matching backend's sysfs
                // scan. A backend that cannot be observed has no profile, and
                // then the open error is the answer.
                let Some(profile) = listing.kind.observer_profile() else {
                    return Err(open_err);
                };
                let Some(sysinfo) = (profile.scan)() else {
                    return Err(open_err);
                };
                Self::new_observer(cfg, config_path, sysinfo, profile)
            }
        }
    }

    pub fn run<B: Backend>(&mut self, terminal: &mut Terminal<B>) -> io::Result<()> {
        const FRAME_DURATION: Duration = Duration::from_millis(33);
        let mut last_draw = Instant::now();

        // Repaint from a clean slate: the device selector and any backend chatter
        // during open may have left the alternate screen dirty before we get here.
        terminal.clear()?;
        self.draw(terminal)?;

        loop {
            let needs_redraw = match self.events.recv() {
                AppEvent::Key(key) => {
                    if self.device_option_pending() && Self::is_option_quit_key(key) {
                        if self.request_pending_option_quit() {
                            return Err(self.forced_option_quit_error());
                        }
                        true
                    } else {
                        match input::handle_key(
                            key,
                            &self.state,
                            self.device.as_ref(),
                            &mut self.engine,
                            &mut self.show_footer,
                            &self.focus_keys,
                        ) {
                            input::KeyAction::Quit => {
                                if self.device_option_pending() {
                                    if self.request_pending_option_quit() {
                                        return Err(self.forced_option_quit_error());
                                    }
                                } else {
                                    self.finish_session()?;
                                    return Ok(());
                                }
                            }
                            input::KeyAction::Continue => {}
                            input::KeyAction::ApplyDeviceOption(request) => {
                                self.start_device_option(request);
                            }
                        }
                        last_draw.elapsed() >= FRAME_DURATION
                    }
                }
                AppEvent::Tick => true,
                AppEvent::DeviceOptionComplete(completion) => {
                    if input::complete_device_option(&self.state, completion) {
                        self.finish_session()?;
                        return Ok(());
                    }
                    true
                }
            };

            if needs_redraw {
                self.draw(terminal)?;
                last_draw = Instant::now();
            }
        }
    }

    fn start_device_option(&self, request: DeviceOptionRequest) {
        let Some(device) = self.device.as_ref().cloned() else {
            input::complete_device_option(
                &self.state,
                DeviceOptionCompletion {
                    result: Err("device is unavailable".to_string()),
                },
            );
            return;
        };
        let tx = self.events.sender();
        let worker_request = request.clone();
        Self::spawn_device_option_task(tx, move || {
            Self::execute_device_option(
                &worker_request,
                |id, choice| device.set_option(id, choice),
                || device.options(),
            )
        });
    }

    fn device_option_pending(&self) -> bool {
        let m = self.state.lock().unwrap_or_else(|error| error.into_inner());
        m.ui.device_option_update.is_pending()
    }

    fn is_option_quit_key(key: KeyEvent) -> bool {
        matches!(key.code, KeyCode::Char('q') | KeyCode::Char('Q'))
            || matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'))
                && key.modifiers.contains(KeyModifiers::CONTROL)
    }

    fn request_pending_option_quit(&self) -> bool {
        let mut m = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let forced = m.ui.device_option_update.request_quit();
        if !forced {
            m.push_log(
                "Waiting for device option update; Q/Ctrl-C again forces quit without saving"
                    .to_string(),
            );
        }
        forced
    }

    fn forced_option_quit_error(&self) -> io::Error {
        // A backend destructor can block. Leak one owner on this process-exit
        // path so App teardown and worker completion cannot run that destructor.
        if let Some(device) = self.device.as_ref().cloned() {
            std::mem::forget(device);
        }
        io::Error::other(
            "Forced quit while a device option update is still running; device state is uncertain and settings were not saved",
        )
    }

    fn finish_session(&self) -> io::Result<()> {
        self.restore_noise_sweep();
        self.restore_sweep_tuning();
        self.save_config().map_err(io::Error::other)
    }

    fn draw<B: Backend>(&mut self, terminal: &mut Terminal<B>) -> io::Result<()> {
        let active_preset = self.engine.active_preset().to_string();
        let sweep_active = self.engine.is_panel_visible("sweep_panel")
            || self.engine.is_panel_visible("micro_sweep_panel");
        // The demod is gated on its panel being on screen, not on the preset being
        // called `lab_signal`: presets are data, and a user preset that lists
        // `fm_demod` used to get a panel that never received a block - it sat at
        // "DEMOD IDLE — waiting for a usable channel" forever on a station the
        // built-in preset locked onto instantly. Asking the engine which panels are
        // active is how the rest of the layout already works. The gate itself stays,
        // so the extra per-block copy still costs nothing on every screen without it.
        let demod_preset = self.engine.is_panel_visible("fm_demod");
        // Written into the **shared state**, not into the snapshot below.
        //
        // Everything after the clone is the UI thread talking to itself: the
        // footer reads it back out of the frame it was just handed. These three
        // have consumers on other threads, so they have to be here, on the
        // shared side of the clone, and the section is the one that proves it -
        // `hardware::process` gates the NET sample feed on it and
        // `tasks::net` decides whether to survey by it. It used to be set on the
        // snapshot with the footer's fields, which looked right on every screen
        // and meant both of those read an empty string for ever.
        let mut m = {
            let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
            guard.sweep.active = sweep_active;
            guard.demod.enabled = demod_preset && guard.demod.user_on;
            guard.ui.section = self
                .engine
                .scope()
                .map(|s| s.id.clone())
                .unwrap_or_default();
            // On the shared side, not only the snapshot below - `tasks::net`'s
            // survey task reads this off this thread to decide whether a BLE
            // preset means rotating the three advertising channels instead of
            // the wideband occupancy grid (B11). Setting it only on `m`, the
            // clone, was exactly the bug `section` above already had and was
            // fixed for: it looked right on every screen and meant the one
            // reader on another thread saw an empty string for ever.
            guard.ui.active_preset = active_preset.clone();
            guard.clone()
        };
        m.ui.preset_names = self.engine.preset_names();
        // The footer names the keys that work right now, and the digits are
        // scoped, so it reads the active section rather than keeping a table.
        // The section itself is set above, on the shared state, because it has
        // readers off this thread.
        m.ui.scope = self
            .engine
            .scope()
            .map(|s| {
                s.entries
                    .iter()
                    .map(|e| (e.slot, e.preset.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let hide_footer = !self.show_footer
            && matches!(
                m.ui.input_mode,
                crate::state::InputMode::Normal | crate::state::InputMode::DeviceOptionInput { .. }
            );
        self.engine.set_panel_hidden("footer", hide_footer);
        // Copied out before the closure borrows `self` for the engine.
        let deck_shown = self.deck_shown;
        // Measurement labs wear "instrument mode": the resting frames cool toward
        // steel-blue. One per-frame tint at the draw root keeps every lab panel
        // (and its chrome) cohesive without each panel knowing about lab mode.
        let frame_theme = if m.ui.is_lab_mode() {
            self.theme.steeled()
        } else {
            self.theme.clone()
        };
        terminal.draw(|f| {
            self.engine.draw(f, &m, &frame_theme);
            // The rail's full-log overlay only floats while the rail is focused.
            if m.ui.log_overlay && m.ui.focused_panel.as_deref() == Some("command_rail") {
                ui::overlay::render_log(f, &m, &frame_theme);
            }
            // The menu floats over the deck, the same way the log overlay does.
            // Drawn last so nothing else lands on top of it, and outside the
            // layout engine because it is not a panel.
            if let Some(menu_state) = m.ui.menu {
                let full = f.size();
                // Startup gets the whole terminal, because there is no deck
                // behind it worth showing yet. Afterwards it is a box over the
                // layout you are on, so you can see what you are leaving.
                let area = if deck_shown {
                    ui::overlay::centered_rect(
                        (full.width * 8 / 10).clamp(1, full.width).max(1),
                        (full.height * 8 / 10).clamp(1, full.height).max(1),
                        full,
                    )
                } else {
                    full
                };
                f.render_widget(ratatui::widgets::Clear, area);
                let controls = self.engine.section_controls(menu_state.section, &m);
                ui::menu::render(
                    f,
                    area,
                    &m,
                    self.engine.menu(),
                    &menu_state,
                    &controls,
                    &frame_theme,
                );
            }
            if m.ui.device_option_update.quit_requested() {
                let full = f.size();
                let area = ratatui::layout::Rect::new(
                    full.x,
                    full.y + full.height.saturating_sub(1),
                    full.width,
                    1,
                );
                f.render_widget(ratatui::widgets::Clear, area);
                f.render_widget(
                    ratatui::widgets::Paragraph::new(
                        " Waiting for device; Q/Ctrl-C again forces quit without saving",
                    )
                    .style(ratatui::style::Style::default().fg(frame_theme.status_warn)),
                    area,
                );
            }
        })?;
        // The deck is behind the menu from the moment it has been drawn once
        // without one in front of it.
        if m.ui.menu.is_none() {
            self.deck_shown = true;
        }

        Ok(())
    }

    /// Put the swept stage back before the app goes away.
    ///
    /// A sweep parks the front stage at each of its settings in turn, so quitting
    /// mid-measurement would leave the radio at whatever step it had reached -
    /// and, worse, `save_config` would then write that step out as the user's
    /// gain. Restoring here fixes both: the radio ends where it started, and the
    /// config records the setting that was actually chosen.
    fn restore_noise_sweep(&self) {
        let restore = {
            let mut m = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let r = m.lab.noise_sweep.as_ref().map(|sw| sw.restore());
            m.lab.noise_sweep = None;
            r
        };
        let (Some((idx, db)), Some(device)) = (restore, self.device.as_ref()) else {
            return;
        };
        let stages = {
            let m = self.state.lock().unwrap_or_else(|e| e.into_inner());
            m.caps.gain.stages()
        };
        let Some(spec) = stages.get(idx) else {
            return;
        };
        // Device call with no lock held, as everywhere else.
        let _ = device.set_stage_gain(idx, &spec.name, db);
        let mut m = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(g) = m.radio.gains.get_mut(idx) {
            *g = db;
        }
    }

    fn spawn_device_option_task(
        tx: std::sync::mpsc::Sender<AppEvent>,
        apply: impl FnOnce() -> anyhow::Result<Vec<hardware::DeviceOption>> + Send + 'static,
    ) {
        std::thread::spawn(move || {
            let completion = DeviceOptionCompletion {
                result: apply().map_err(|error| format!("{error:#}")),
            };
            let _ = tx.send(AppEvent::DeviceOptionComplete(completion));
        });
    }

    fn execute_device_option(
        request: &DeviceOptionRequest,
        set: impl FnOnce(&str, &str) -> anyhow::Result<()>,
        refresh: impl FnOnce() -> Vec<hardware::DeviceOption>,
    ) -> anyhow::Result<Vec<hardware::DeviceOption>> {
        set(&request.id, &request.choice)?;
        Ok(refresh())
    }

    /// Put the tuner back before the app goes away.
    ///
    /// The same problem as `restore_noise_sweep`, one field along. A frequency
    /// sweep parks the radio at each position in turn and writes each one to
    /// `radio.frequency`, because that is the field the FFT worker stamps its
    /// frames with. The `sweep_task` restores the interrupted tuning when it
    /// notices `sweep.active` has gone false, but quitting never gives it
    /// another iteration: the process ends first, and `save_config` writes out
    /// whichever position the scan was parked on. Quitting from `lab_sweep` or
    /// `micro_sweep` therefore reopened the app somewhere in the middle of the
    /// swept band, one band-width further along each time.
    ///
    /// The NET survey does the same thing for the same reason, so it gives the
    /// tuner back here too. Harmless on a state that did neither, so it is not
    /// conditional.
    fn restore_sweep_tuning(&self) {
        let moved = {
            let mut m = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let was = m.radio.frequency;
            m.radio.frequency = m.sweep.end(was).tune_hz;
            // The NET survey parks the radio at each position in turn for
            // exactly the same reason and with exactly the same consequence on
            // quit, so it gives the tuner back through the same path. Harmless
            // on a state that never surveyed, and it runs second because either
            // may have moved the radio but never both: the two live in different
            // sections and only one section is on screen.
            let after_sweep = m.radio.frequency;
            m.radio.frequency = m.net.end(after_sweep).tune_hz;
            // Measured against where the radio actually was when quitting, not
            // against whatever the first of the two handed on.
            (m.radio.frequency != was).then_some(m.radio.frequency)
        };
        // Only when the sweep had actually moved the radio: every quit comes
        // through here, and a retune to the frequency the radio is already on is
        // one more device call during teardown for nothing.
        let (Some(hz), Some(device)) = (moved, self.device.as_ref()) else {
            return;
        };
        // Device call with no lock held, as everywhere else.
        let _ = device.set_frequency(hz);
    }

    fn save_config(&self) -> anyhow::Result<()> {
        let Some(device) = self.device.as_ref() else {
            return Ok(());
        };
        let Some(path) = &self.config_path else {
            return Ok(());
        };
        let (freq, rate, gains, amp, wf_rows, wf_palette, spec_style, markers, sweep_cfg, recall) = {
            let m = self.state.lock().unwrap_or_else(|e| e.into_inner());
            (
                m.radio.frequency,
                m.radio.config_sample_rate,
                crate::hardware::gain::format_named(&m.caps.gain.stages(), &m.radio.gains),
                m.radio.amp_enabled,
                m.waterfall.buffer.max_rows,
                m.waterfall.palette,
                m.spectrum.style,
                m.spectrum.markers.clone(),
                m.sweep.config.clone(),
                crate::state::recall_to_hz(&m.ui.recall),
            )
        };
        let cfg = AppConfig {
            radio: RadioConfig {
                frequency_hz: freq,
                sample_rate: rate,
                // The named form, and only that: the pre-0.5.0 positional pair
                // is still read on load and is no longer written, so a saved
                // file names the stages the device actually has.
                gain: Some(gains),
                lna_gain: None,
                vga_gain: None,
                amp_enabled: amp,
                recall_hz: recall,
            },
            display: DisplayConfig {
                active_preset: self.engine.saved_active_preset().to_string(),
                waterfall_max_rows: wf_rows,
                waterfall_palette: wf_palette,
                spectrum_style: spec_style,
                spectrum_markers: markers,
            },
            // The loaded block, not a fresh one: `..Default::default()` here
            // silently deleted every per-field colour override on every quit.
            // Only `base` is owned by the running app (it follows `--theme`).
            theme: crate::config::ThemeConfig {
                base: self.theme.name.clone(),
                ..self.theme_config.clone()
            },
            sweep: crate::config::SweepSettings {
                start_hz: sweep_cfg.start_hz,
                stop_hz: sweep_cfg.stop_hz,
                dwell_ms: sweep_cfg.dwell_ms,
            },
            tinysa: self.tinysa_config.clone(),
            net: self.net_config.clone(),
            presets: self.user_presets.clone(),
        };
        let mut candidate = cfg.clone();
        let hook_error = device.update_config(&mut candidate).err();
        let save_result = if hook_error.is_some() {
            cfg.save(path)
        } else {
            candidate.save(path)
        };
        match (hook_error, save_result) {
            (None, result) => result,
            (Some(error), Ok(())) => Err(error.context(
                "device settings could not be updated; previous device settings and other settings were saved",
            )),
            (Some(hook_error), Err(save_error)) => anyhow::bail!(
                "failed to save config: {save_error:#}; device settings update also failed: {hook_error:#}"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::{DeviceCapabilities, DeviceInfo, RateSet};
    use crate::state::DeviceOptionUpdate;
    use std::cell::Cell;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::sync::{mpsc, Arc, Mutex};

    #[derive(Clone, Copy)]
    enum ConfigHook {
        Preserve,
        Apply,
        FailAfterMutation,
    }

    struct QuitDevice {
        caps: DeviceCapabilities,
        tuned_hz: AtomicU64,
        tune_calls: AtomicUsize,
        drops: Arc<AtomicUsize>,
        config_hook: ConfigHook,
    }

    impl QuitDevice {
        fn new(caps: DeviceCapabilities) -> Self {
            Self {
                caps,
                tuned_hz: AtomicU64::new(0),
                tune_calls: AtomicUsize::new(0),
                drops: Arc::new(AtomicUsize::new(0)),
                config_hook: ConfigHook::Preserve,
            }
        }

        fn with_config_hook(mut self, config_hook: ConfigHook) -> Self {
            self.config_hook = config_hook;
            self
        }
    }

    impl Drop for QuitDevice {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::Relaxed);
        }
    }

    impl SdrDevice for QuitDevice {
        fn capabilities(&self) -> &DeviceCapabilities {
            &self.caps
        }

        fn info(&self) -> DeviceInfo {
            DeviceInfo::default()
        }

        fn start_rx(&self, _ctx: Arc<RxContext>) -> anyhow::Result<()> {
            Ok(())
        }

        fn stop_rx(&self) -> anyhow::Result<()> {
            Ok(())
        }

        fn is_streaming(&self) -> bool {
            false
        }

        fn set_frequency(&self, hz: u64) -> anyhow::Result<()> {
            self.tuned_hz.store(hz, Ordering::Relaxed);
            self.tune_calls.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }

        fn set_sample_rate(&self, hz: f64) -> anyhow::Result<RateSet> {
            Ok(RateSet::new(hz, Some(hz), 0))
        }

        fn set_lna_gain(&self, _db: u32) -> anyhow::Result<()> {
            Ok(())
        }

        fn update_config(&self, config: &mut AppConfig) -> anyhow::Result<()> {
            match self.config_hook {
                ConfigHook::Preserve => Ok(()),
                ConfigHook::Apply => {
                    config.tinysa.points = 900;
                    Ok(())
                }
                ConfigHook::FailAfterMutation => {
                    config.tinysa.points = 900;
                    anyhow::bail!("injected backend config failure")
                }
            }
        }
    }

    fn pending_request() -> DeviceOptionRequest {
        DeviceOptionRequest {
            id: "bandwidth".into(),
            label: "Bandwidth".into(),
            choice: "Wide".into(),
        }
    }

    fn quit_test_app() -> (
        App,
        mpsc::Sender<AppEvent>,
        Arc<QuitDevice>,
        std::path::PathBuf,
    ) {
        static NEXT_PATH: AtomicUsize = AtomicUsize::new(0);
        let state = Arc::new(Mutex::new(SdrMetrics::fixture()));
        {
            let mut m = state.lock().unwrap();
            m.radio.frequency = 200_000_000;
            m.sweep.pre_sweep_hz = Some(100_000_000);
            m.ui.device_option_update = DeviceOptionUpdate::Pending {
                request: pending_request(),
                quit_requested: false,
            };
        }
        let device = Arc::new(QuitDevice::new(state.lock().unwrap().caps.as_ref().clone()));
        let path = std::env::temp_dir().join(format!(
            "sdrtop-option-quit-{}-{}.toml",
            std::process::id(),
            NEXT_PATH.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_file(&path);
        let mut config = AppConfig::default();
        config.tinysa.lna = true;
        let mut app = App::assemble(
            config,
            Some(path.clone()),
            state,
            Some(device.clone()),
            None,
            None,
        )
        .unwrap();
        let (tx, rx) = mpsc::channel();
        app.events = EventStream::from_channel(tx.clone(), rx);
        (app, tx, device, path)
    }

    #[test]
    fn first_quit_waits_for_success_then_restores_before_saving() {
        let (mut app, tx, device, path) = quit_test_app();
        tx.send(AppEvent::Key(KeyEvent::from(KeyCode::Char('q'))))
            .unwrap();
        tx.send(AppEvent::DeviceOptionComplete(DeviceOptionCompletion {
            result: Ok(vec![hardware::DeviceOption {
                id: "bandwidth".into(),
                label: "Bandwidth".into(),
                choices: vec!["Narrow".into(), "Wide".into()],
                selected_choice: "Wide".into(),
                integer_range: None,
            }]),
        }))
        .unwrap();
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();

        app.run(&mut terminal).unwrap();

        assert_eq!(device.tune_calls.load(Ordering::Relaxed), 1);
        assert_eq!(device.tuned_hz.load(Ordering::Relaxed), 100_000_000);
        assert_eq!(
            AppConfig::load_or_default(&path).radio.frequency_hz,
            100_000_000
        );
        assert!(AppConfig::load_or_default(&path).tinysa.lna);
        let m = app.state.lock().unwrap();
        assert_eq!(m.device_options[0].selected_choice, "Wide");
        assert!(matches!(
            m.ui.device_option_update,
            DeviceOptionUpdate::Idle
        ));
        assert!(m
            .ui
            .log
            .back()
            .is_some_and(|entry| entry.text.contains("Bandwidth set to Wide")));
        drop(m);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn successful_backend_config_hook_updates_the_saved_snapshot() {
        let state = Arc::new(Mutex::new(SdrMetrics::fixture()));
        state.lock().unwrap().radio.frequency = 144_390_000;
        let device = Arc::new(
            QuitDevice::new(state.lock().unwrap().caps.as_ref().clone())
                .with_config_hook(ConfigHook::Apply),
        );
        let path = std::env::temp_dir().join(format!(
            "sdrtop-config-hook-success-{}.toml",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let app = App::assemble(
            AppConfig::default(),
            Some(path.clone()),
            state,
            Some(device),
            None,
            None,
        )
        .unwrap();

        app.save_config().unwrap();

        let saved = AppConfig::load_or_default(&path);
        assert_eq!(saved.radio.frequency_hz, 144_390_000);
        assert_eq!(saved.tinysa.points, 900);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn failed_backend_config_hook_saves_the_unmodified_backend_block() {
        let state = Arc::new(Mutex::new(SdrMetrics::fixture()));
        {
            let mut metrics = state.lock().unwrap();
            metrics.radio.frequency = 433_920_000;
            metrics.spectrum.markers.push(crate::state::SpectrumMarker {
                freq_hz: 434_500_000,
                label: "review".into(),
                channel_bw_hz: Some(25_000),
                measured_bw_hz: None,
            });
        }
        let device = Arc::new(
            QuitDevice::new(state.lock().unwrap().caps.as_ref().clone())
                .with_config_hook(ConfigHook::FailAfterMutation),
        );
        let path = std::env::temp_dir().join(format!(
            "sdrtop-config-hook-failure-{}.toml",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let mut config = AppConfig::default();
        config.tinysa.points = 450;
        config.tinysa.lna = true;
        let app =
            App::assemble(config, Some(path.clone()), state, Some(device), None, None).unwrap();

        let error = format!("{:#}", app.save_config().unwrap_err());

        assert!(error.contains("device settings could not be updated"));
        assert!(error.contains("previous device settings and other settings were saved"));
        assert!(error.contains("injected backend config failure"));
        let saved = AppConfig::load_or_default(&path);
        assert_eq!(saved.radio.frequency_hz, 433_920_000);
        assert_eq!(saved.tinysa.points, 450);
        assert!(saved.tinysa.lna);
        assert_eq!(saved.display.spectrum_markers.len(), 1);
        assert_eq!(saved.display.spectrum_markers[0].freq_hz, 434_500_000);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn backend_hook_and_file_save_failures_are_both_reported() {
        let state = Arc::new(Mutex::new(SdrMetrics::fixture()));
        let device = Arc::new(
            QuitDevice::new(state.lock().unwrap().caps.as_ref().clone())
                .with_config_hook(ConfigHook::FailAfterMutation),
        );
        let root = std::env::temp_dir().join(format!(
            "sdrtop-config-hook-double-failure-{}",
            std::process::id()
        ));
        let blocker = root.join("not-a-directory");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&blocker, b"block config parent").unwrap();
        let app = App::assemble(
            AppConfig::default(),
            Some(blocker.join("config.toml")),
            state,
            Some(device),
            None,
            None,
        )
        .unwrap();

        let error = app.save_config().unwrap_err().to_string();

        assert!(error.contains("failed to save config"));
        assert!(error.contains("device settings update also failed"));
        assert!(error.contains("injected backend config failure"));
        std::fs::remove_file(blocker).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn ordinary_quit_reports_save_failure_and_preserves_existing_config() {
        static NEXT_PATH: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "sdrtop-quit-save-failure-{}-{}",
            std::process::id(),
            NEXT_PATH.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("config.toml");
        let original = b"[radio]\nfrequency_hz = 123456789\n";
        std::fs::write(&path, original).unwrap();
        std::fs::create_dir(path.with_extension("tmp")).unwrap();

        let state = Arc::new(Mutex::new(SdrMetrics::fixture()));
        let device = Arc::new(QuitDevice::new(state.lock().unwrap().caps.as_ref().clone()));
        let mut app = App::assemble(
            AppConfig::default(),
            Some(path.clone()),
            state,
            Some(device),
            None,
            None,
        )
        .unwrap();
        let (tx, rx) = mpsc::channel();
        app.events = EventStream::from_channel(tx.clone(), rx);
        tx.send(AppEvent::Key(KeyEvent::from(KeyCode::Char('q'))))
            .unwrap();
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();

        let error = app.run(&mut terminal).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::Other);
        assert_eq!(std::fs::read(&path).unwrap(), original);
        std::fs::remove_dir(path.with_extension("tmp")).unwrap();
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn numeric_menu_entry_preserves_the_deck_footer_setting() {
        use crate::state::{InputMode, MenuPane, MenuState};
        let (mut app, _, _, _) = quit_test_app();
        app.engine.set_preset("command_rail");
        {
            let mut m = app.state.lock().unwrap();
            m.ui.device_option_update = DeviceOptionUpdate::Idle;
            m.ui.menu = Some(MenuState {
                pane: MenuPane::Options,
                ..Default::default()
            });
        }
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        for show_footer in [false, true] {
            app.show_footer = show_footer;
            for mode in [
                InputMode::Normal,
                InputMode::DeviceOptionInput {
                    id: "level".into(),
                    error: None,
                },
                InputMode::FrequencyInput,
                InputMode::SampleRateInput,
                InputMode::SweepStartInput,
                InputMode::SweepStopInput,
                InputMode::MarkerNameInput,
                InputMode::ReferenceAccuracyInput { address: [0; 6] },
            ] {
                let expected = show_footer
                    || !matches!(
                        mode,
                        InputMode::Normal | InputMode::DeviceOptionInput { .. }
                    );
                app.state.lock().unwrap().ui.input_mode = mode;
                app.draw(&mut terminal).unwrap();
                assert_eq!(app.engine.is_panel_visible("footer"), expected);
                assert_eq!(app.show_footer, show_footer);
            }
        }
    }

    #[test]
    fn first_quit_also_waits_for_failure_before_orderly_finish() {
        let (mut app, tx, device, path) = quit_test_app();
        tx.send(AppEvent::Key(KeyEvent::from(KeyCode::Char('q'))))
            .unwrap();
        tx.send(AppEvent::DeviceOptionComplete(DeviceOptionCompletion {
            result: Err("device rejected choice".into()),
        }))
        .unwrap();
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();

        app.run(&mut terminal).unwrap();

        assert_eq!(device.tune_calls.load(Ordering::Relaxed), 1);
        assert!(path.exists());
        let m = app.state.lock().unwrap();
        assert!(matches!(
            &m.ui.device_option_update,
            DeviceOptionUpdate::Failed { id } if id == "bandwidth"
        ));
        assert!(m.ui.log.back().is_some_and(|entry| entry
            .text
            .contains("Bandwidth error: device rejected choice")));
        drop(m);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn second_quit_or_control_c_escapes_without_restore_or_save() {
        let escapes = [
            KeyEvent::from(KeyCode::Char('q')),
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        ];
        for escape in escapes {
            let (mut app, tx, device, path) = quit_test_app();
            tx.send(AppEvent::Key(KeyEvent::from(KeyCode::Char('q'))))
                .unwrap();
            tx.send(AppEvent::Key(escape)).unwrap();
            let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();

            let error = app.run(&mut terminal).unwrap_err();

            assert!(error.to_string().contains("device state is uncertain"));
            assert!(error.to_string().contains("settings were not saved"));
            assert_eq!(device.tune_calls.load(Ordering::Relaxed), 0);
            assert!(!path.exists());
            assert!(
                app.state
                    .lock()
                    .unwrap()
                    .ui
                    .device_option_update
                    .is_pending(),
                "forced exit must not pretend the worker was cancelled"
            );
            let drops = Arc::clone(&device.drops);
            drop(app);
            drop(device);
            assert_eq!(
                drops.load(Ordering::Relaxed),
                0,
                "the retained device owner must suppress backend Drop"
            );
        }
    }

    #[test]
    fn slow_device_option_work_runs_off_the_event_thread() {
        let state = Arc::new(Mutex::new(SdrMetrics::fixture()));
        let request = DeviceOptionRequest {
            id: "bandwidth".into(),
            label: "Bandwidth".into(),
            choice: "Wide".into(),
        };
        state.lock().unwrap().ui.device_option_update = DeviceOptionUpdate::Pending {
            request: request.clone(),
            quit_requested: false,
        };
        let (event_tx, event_rx) = mpsc::channel();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let worker_state = Arc::clone(&state);
        let worker_request = request.clone();

        App::spawn_device_option_task(event_tx, move || {
            App::execute_device_option(
                &worker_request,
                |_, _| {
                    assert!(
                        worker_state.try_lock().is_ok(),
                        "state was locked during set_option"
                    );
                    started_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Ok(())
                },
                || {
                    assert!(
                        worker_state.try_lock().is_ok(),
                        "state was locked during options"
                    );
                    vec![
                        hardware::DeviceOption {
                            id: "bandwidth".into(),
                            label: "Bandwidth".into(),
                            choices: vec!["Narrow".into(), "Wide".into()],
                            selected_choice: "Wide".into(),
                            integer_range: None,
                        },
                        hardware::DeviceOption {
                            id: "attenuation".into(),
                            label: "Attenuation".into(),
                            choices: vec!["0 dB".into(), "10 dB".into()],
                            selected_choice: "0 dB".into(),
                            integer_range: None,
                        },
                    ]
                },
            )
        });

        started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("option worker did not start");
        assert!(
            state.try_lock().is_ok(),
            "event thread cannot read state while the backend waits"
        );
        release_tx.send(()).unwrap();
        let AppEvent::DeviceOptionComplete(completion) = event_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("option worker did not complete")
        else {
            panic!("worker sent the wrong event");
        };
        input::complete_device_option(&state, completion);

        let m = state.lock().unwrap();
        assert_eq!(m.device_options.len(), 2);
        assert_eq!(m.device_options[0].selected_choice, "Wide");
        assert_eq!(m.device_options[1].selected_choice, "0 dB");
    }

    #[test]
    fn failed_device_option_set_does_not_refresh() {
        let request = DeviceOptionRequest {
            id: "bandwidth".into(),
            label: "Bandwidth".into(),
            choice: "Wide".into(),
        };
        let refreshed = Cell::new(false);

        let result = App::execute_device_option(
            &request,
            |_, _| anyhow::bail!("device rejected choice"),
            || {
                refreshed.set(true);
                Vec::new()
            },
        );

        assert!(result.is_err());
        assert!(!refreshed.get());
    }

    /// **What other threads read is written to the shared state, not to the
    /// snapshot.**
    ///
    /// The frame the UI draws is a *clone* of `SdrMetrics`, and most of what is
    /// stamped onto it afterwards is the UI thread talking to itself - the
    /// footer reads back the preset name it was just handed. `ui.section` is not
    /// like that: `hardware::process` gates the NET sample feed on it and
    /// `tasks::net` decides whether to survey by it, and both read the shared
    /// state.
    ///
    /// It was set on the snapshot, one line below the clone. Every screen looked
    /// right, because every panel renders from the snapshot - and the two
    /// consumers off this thread read an empty string for ever, so the NET
    /// section rendered its header, its mode and its empty panel while no block
    /// was ever forwarded and the radio never hopped. Nothing failed: every test
    /// of the consumers sets `ui.section` by hand, which is exactly the shape of
    /// test that verifies a reader and never its writer.
    ///
    /// Read as source text because nothing in the type system tells `guard.ui`
    /// from `m.ui`: they are the same type, one lock apart.
    #[test]
    fn the_section_is_mirrored_into_the_shared_state_and_not_the_snapshot() {
        let src = include_str!("mod.rs");
        let body = src
            .split_once("let demod_preset = self.engine.is_panel_visible(\"fm_demod\");")
            .expect("the frame composition has been rewritten")
            .1
            .split_once("self.engine.render(")
            .map(|(before, _)| before)
            .unwrap_or(src);
        let (guard_side, snapshot_side) = body
            .split_once("guard.clone()")
            .expect("the snapshot is no longer a clone of the guard");
        assert!(
            guard_side.contains("guard.ui.section ="),
            "ui.section must be written to the shared state: the NET feed gate \
             and the survey task read it from there, not from the frame"
        );
        assert!(
            !snapshot_side.contains("ui.section ="),
            "ui.section is written to the snapshot after the clone, so every \
             reader off the UI thread sees an empty string"
        );
    }

    /// **The same bug, in the same shape, found the same way `ui.section`'s
    /// own bug was.** B11 made `tasks::net`'s survey task the first reader of
    /// `active_preset` off the UI thread - to decide whether a BLE preset
    /// means rotating the three advertising channels rather than covering the
    /// wideband occupancy grid - and it read an empty string every time,
    /// because `active_preset` was written to `m`, the snapshot, one line
    /// after `guard.clone()`, exactly where `ui.section` used to be written
    /// before the test above existed. Every screen still looked right, for
    /// the same reason: every panel renders from the snapshot, and the one
    /// consumer that does not is off this thread.
    #[test]
    fn the_active_preset_is_mirrored_into_the_shared_state_and_not_only_the_snapshot() {
        let src = include_str!("mod.rs");
        let body = src
            .split_once("let demod_preset = self.engine.is_panel_visible(\"fm_demod\");")
            .expect("the frame composition has been rewritten")
            .1
            .split_once("self.engine.render(")
            .map(|(before, _)| before)
            .unwrap_or(src);
        let (guard_side, _snapshot_side) = body
            .split_once("guard.clone()")
            .expect("the snapshot is no longer a clone of the guard");
        assert!(
            guard_side.contains("guard.ui.active_preset ="),
            "active_preset must be written to the shared state: tasks::net's \
             survey task reads it from there, not from the frame, to tell a \
             BLE preset's rotation from the wideband survey"
        );
    }

    /// **Every scanner that parks the radio gives the tuner back on quit.**
    ///
    /// Two now do: the frequency sweep and the NET survey. Both write their
    /// current position into `radio.frequency` because that is the field the FFT
    /// worker stamps frames with, and `save_config` persists that field, so a
    /// scanner missing from this function reopens the app somewhere in the
    /// middle of its band - one position further along each time.
    ///
    /// Read as source text, like the test above and for the same reason: adding
    /// a third scanner and forgetting this line compiles perfectly, and the
    /// symptom shows up a week later as "the app forgot where I was tuned".
    #[test]
    fn every_scanner_gives_the_tuner_back_on_quit() {
        let body = include_str!("mod.rs")
            .split_once("fn restore_sweep_tuning(&self) {")
            .expect("restore_sweep_tuning has been renamed")
            .1
            .split_once("\n    fn ")
            .expect("restore_sweep_tuning no longer ends")
            .0;
        for scanner in ["m.sweep.end(", "m.net.end("] {
            assert!(
                body.contains(scanner),
                "{scanner} is missing: quitting mid-scan will save the scan position"
            );
        }
    }

    #[test]
    fn iq_imbalance_zero_for_balanced() {
        let n = 1000_f64;
        let i_rms = (500_000_f64 / n).sqrt();
        let q_rms = (500_000_f64 / n).sqrt();
        let imbalance = (20.0 * (i_rms / q_rms).log10()) as f32;
        assert!(imbalance.abs() < 0.001, "expected ~0, got {}", imbalance);
    }

    #[test]
    fn iq_imbalance_positive_when_i_stronger() {
        let n = 1000_f64;
        let i_rms = (800_000_f64 / n).sqrt();
        let q_rms = (200_000_f64 / n).sqrt();
        let imbalance = (20.0 * (i_rms / q_rms).log10()) as f32;
        assert!(imbalance > 0.0, "expected positive, got {}", imbalance);
    }

    #[test]
    fn adc_saturation_pct_full() {
        let acc_saturated = 200_u64;
        let acc_samples = 100_u64;
        let saturable = acc_samples * 2;
        let pct = (acc_saturated as f32 / saturable as f32) * 100.0;
        assert!((pct - 100.0).abs() < 0.01, "expected 100%, got {}", pct);
    }
}
