// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

mod builder;
pub mod input;

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{backend::Backend, Terminal};

use crate::config::{DisplayConfig, EngineConfig, RadioConfig, SdrtopConfig, TuiConfig};
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
    /// The engine's state beside the screen's own, which the keys and the
    /// frame read together. The screen's lives only here.
    pub(super) screen: crate::ui::Screen,
    /// The engine the keys send their commands to; `None` in observer mode.
    pub(super) core: Option<crate::engine::Engine>,
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
    /// Kept for the same reason as `user_presets`: `save_config` rewrites every
    /// block it owns whole, so anything in one it does not carry forward is
    /// deleted. This block
    /// holds the per-field colour overrides, which the app reads once at startup
    /// and never touches again - so without a copy of them there is nothing left
    /// to write back.
    pub(super) theme_config: crate::config::ThemeConfig,
    pub(super) tinysa_config: crate::config::TinySaSettings,
    /// The `[airspy]` block, carried forward the same way; the R2's backend
    /// writes its own part back through `update_config`.
    pub(super) airspy_config: crate::config::AirspySettings,
    /// The `[airspyhf]` block, likewise.
    pub(super) airspyhf_config: crate::config::AirspyHfSettings,
    /// The `[sdrplay]` block, likewise.
    pub(super) sdrplay_config: crate::config::SdrplaySettings,
    pub(super) plutosky_config: crate::config::PlutoSkySettings,
    /// `[radio.boost]` as loaded: every other radio's entry is carried
    /// forward, and the open radio's is written from the live state.
    pub(super) boost_config: std::collections::BTreeMap<String, bool>,
    /// The `[net]` block exactly as it was loaded - `[net].bt_channels`
    /// has no in-app control that would change it while running, so the
    /// only way `save_config` can carry it forward is to hold the loaded
    /// value, the same reasoning `tinysa_config` already follows.
    pub(super) net_config: crate::config::NetSettings,
    /// The `[record]` block as loaded, carried forward for the same reason.
    pub(super) record_config: crate::config::RecordSettings,
    /// The IQ recording `Ctrl+R` started, while its writer may still be at
    /// work. See `export::iq`.
    pub(super) recorder: Option<crate::export::iq::Recorder>,
    /// Whether the stream's own end, if it has one, was already answered
    /// with the key that starts it again: said once per end.
    pub(super) end_answered: bool,
    /// What could not be saved when the session ended, and why: said once
    /// the terminal is restored, as a warning, since the session itself
    /// ended as asked.
    pub(super) unsaved: Option<String>,
}

impl App {
    pub fn new(
        cfg: SdrtopConfig,
        config_path: Option<PathBuf>,
        listing: &hardware::DeviceListing,
    ) -> anyhow::Result<Self> {
        match hardware::open_device(listing, &cfg.engine) {
            Ok(device) => {
                let observable = listing.kind.observer_profile().is_some();
                Self::new_normal(cfg, config_path, device, observable)
            }
            Err(open_err) => {
                // Device is present but couldn't be opened (e.g. busy) - fall back
                // to read-only observer mode via the matching backend's USB
                // scan. A backend that cannot be observed has no profile, and
                // then the open error is the answer; a scan the system refused
                // is said beside it rather than dropped.
                let Some(profile) = listing.kind.observer_profile() else {
                    return Err(open_err);
                };
                let sysinfo = match (profile.scan)() {
                    Ok(Some(info)) => info,
                    Ok(None) => return Err(open_err),
                    Err(scan) => {
                        return Err(open_err.context(format!(
                            "observer mode could not look for the radio either: {scan}"
                        )))
                    }
                };
                Self::new_observer(cfg, config_path, sysinfo, profile)
            }
        }
    }

    pub fn run<B: Backend>(&mut self, terminal: &mut Terminal<B>) -> io::Result<()>
    where
        B::Error: Send + Sync + 'static,
    {
        const FRAME_DURATION: Duration = Duration::from_millis(33);
        let mut last_draw = Instant::now();

        // Repaint from a clean slate: the device selector and any backend chatter
        // during open may have left the alternate screen dirty before we get here.
        terminal.clear().map_err(backend_error)?;
        self.draw(terminal)?;

        loop {
            let needs_redraw = match self.events.recv() {
                AppEvent::Key(key) => {
                    if self.device_option_pending() && Self::is_option_quit_key(key) {
                        if self.request_pending_option_quit() {
                            return Err(self.forced_option_quit_error());
                        }
                        true
                    } else if Self::is_unsaved_quit_key(key) {
                        // Ctrl+C: out, with the radio put back as `q` puts
                        // it, and nothing written. It reached the key
                        // handlers as a plain `c` before, and focused a panel.
                        self.restore_noise_sweep();
                        self.restore_sweep_tuning();
                        self.end_recording();
                        return Ok(());
                    } else if Self::is_record_key(key) {
                        // Before the handlers: they match the character
                        // alone, and a plain `r` resets every setting.
                        self.toggle_recording();
                        true
                    } else {
                        match input::handle_key(
                            key,
                            &self.screen,
                            self.core.as_ref(),
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
                                    self.end_recording();
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
                    if input::complete_device_option(&self.screen, completion) {
                        self.finish_session()?;
                        return Ok(());
                    }
                    true
                }
                AppEvent::InputError(error) => return Err(error),
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
                &self.screen,
                DeviceOptionCompletion {
                    retune: None,
                    result: Err("device is unavailable".to_string()),
                },
            );
            return;
        };
        let tx = self.events.sender();
        let worker_request = request.clone();
        let state = Arc::clone(&self.state);
        Self::spawn_device_option_task(tx, move || {
            let result = crate::engine::option::set(
                &worker_request.id,
                &worker_request.choice,
                |id, choice| device.set_option(id, choice),
                || device.options(),
            );
            let retune = result
                .is_ok()
                .then(|| {
                    crate::engine::option::retune_after(device.as_ref(), &worker_request.id, &state)
                })
                .flatten();
            DeviceOptionCompletion {
                result: result.map_err(|error| format!("{error:#}")),
                retune,
            }
        });
    }

    fn device_option_pending(&self) -> bool {
        self.screen.ui.borrow().device_option_update.is_pending()
    }

    /// `Ctrl+C`: quit without saving, as `user_docs/keys.md` promises and
    /// `config.md` and `troubleshooting.md` tell a reader to rely on when a
    /// hand-edited config must survive the session.
    fn is_unsaved_quit_key(key: KeyEvent) -> bool {
        matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'))
            && key.modifiers.contains(KeyModifiers::CONTROL)
    }

    /// `Ctrl+R`: start or stop recording the raw IQ stream.
    fn is_record_key(key: KeyEvent) -> bool {
        matches!(key.code, KeyCode::Char('r') | KeyCode::Char('R'))
            && key.modifiers.contains(KeyModifiers::CONTROL)
    }

    /// Start a recording, or stop the one running, and say in the log what
    /// happened either way.
    fn toggle_recording(&mut self) {
        let running = {
            let m = self.state.lock().unwrap_or_else(|e| e.into_inner());
            m.record.current.as_ref().is_some_and(|p| p.ended.is_none())
        };
        let log = |msg: String| {
            self.state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push_log(msg);
        };
        if running {
            if let Some(recorder) = &self.recorder {
                recorder.stop();
                log("IQ: stopping, writing what was already queued".to_string());
            }
            return;
        }
        let Some(ctx) = self.rx_ctx.as_ref() else {
            log("IQ: not recording: another program holds this radio, and sdrtop is only watching it".to_string());
            return;
        };
        let dir = crate::export::destination::default_dir();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            log(format!(
                "IQ: not recording: cannot use {}: {e}",
                dir.display()
            ));
            return;
        }
        let (limits, notes) = self.record_config.limits();
        for note in notes {
            log(format!("IQ: {note}"));
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        let recorder = format!("sdrtop {}", crate::cli::VERSION);
        match crate::export::iq::Recorder::start(
            &self.state,
            &ctx.record,
            &dir,
            limits,
            now,
            &recorder,
        ) {
            Ok(recorder) => self.recorder = Some(recorder),
            Err(why) => log(format!("IQ: not recording: {why}")),
        }
    }

    /// On the way out: stop a recording and wait for its last write, so the
    /// file on disk says `finished` and why.
    fn end_recording(&mut self) {
        if let Some(recorder) = self.recorder.take() {
            recorder.stop();
            recorder.finish();
        }
    }

    fn is_option_quit_key(key: KeyEvent) -> bool {
        matches!(key.code, KeyCode::Char('q') | KeyCode::Char('Q'))
            || matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'))
                && key.modifiers.contains(KeyModifiers::CONTROL)
    }

    fn request_pending_option_quit(&self) -> bool {
        let mut m = self.screen.lock();
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

    fn finish_session(&mut self) -> io::Result<()> {
        self.restore_noise_sweep();
        self.restore_sweep_tuning();
        if let Err(e) = self.save_config() {
            self.unsaved = Some(format!("not all settings were saved: {e:#}"));
        }
        Ok(())
    }

    /// What could not be saved when the session ended, if anything.
    pub fn unsaved(&self) -> Option<&str> {
        self.unsaved.as_deref()
    }

    /// A line in the app's own log, from outside the event loop.
    pub fn note(&self, line: impl Into<String>) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_log(line.into());
    }

    fn draw<B: Backend>(&mut self, terminal: &mut Terminal<B>) -> io::Result<()>
    where
        B::Error: Send + Sync + 'static,
    {
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
        let spectrum_wanted = self.engine.shows_spectrum_reader();
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
            // The engine says the stream stopped by itself and why; how to
            // start it again is the terminal's to say, with its own key.
            match (guard.radio.stream_ended.is_some(), self.end_answered) {
                (true, false) => {
                    guard.push_log("Press [Space] to start receiving again");
                    self.end_answered = true;
                }
                (false, true) => self.end_answered = false,
                _ => {}
            }
            // The FFT worker is fed only while a panel on screen reads what it
            // makes: on a NET view nothing does, and at 20 Msps it is a core.
            guard.spectrum.wanted = spectrum_wanted;
            let section = self
                .engine
                .scope()
                .map(|s| s.id.clone())
                .unwrap_or_default();
            // On the shared side, not only the snapshot below: the section
            // and the layout are what the screen asks the engine to compute,
            // and the engine reads that request, its mode, on other threads.
            // Setting them only on `m`, the clone, once meant `tasks::net`
            // and the NET feed read an empty string for ever while every
            // screen looked right.
            guard.show_view(&section, &active_preset);
            guard.clone()
        };
        // The screen's own state, beside the snapshot: what the screen shows
        // and how wide it is, read back by the footer and the panels that
        // give way on a narrow screen.
        let mut ui = self.screen.ui.borrow_mut();
        let section = self
            .engine
            .scope()
            .map(|s| s.id.clone())
            .unwrap_or_default();
        ui.section = section;
        ui.active_preset = active_preset.clone();
        ui.preset_names = self.engine.preset_names();
        // The footer names the keys that work right now, and the digits are
        // scoped, so it reads the active section rather than keeping a table.
        // The section itself is set above, on the shared state, because it has
        // readers off this thread.
        ui.scope = self
            .engine
            .scope()
            .map(|s| {
                s.entries
                    .iter()
                    .map(|e| (e.slot, e.preset.clone(), e.title.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let hide_footer = !self.show_footer
            && matches!(
                ui.input_mode,
                crate::ui::InputMode::Normal | crate::ui::InputMode::DeviceOptionInput { .. }
            );
        self.engine.set_panel_hidden("footer", hide_footer);
        // The width this frame is drawn at, for the panels that give way on
        // a narrow screen and the ones that take over what they showed.
        let size = terminal.size().unwrap_or_default();
        let size = ratatui::layout::Rect::new(0, 0, size.width, size.height);
        ui.screen_cols = size.width;
        // The one place the wall clock reaches the panels: the header's RX
        // lamp breathes by it.
        ui.drawn_at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let ui = ui;
        // The Frames capture is as long as the river's lanes on this screen,
        // and the NET worker takes it: written on the shared side, and only
        // when it changed, so most frames take no second lock.
        let cells = self
            .engine
            .area_of("net_wifi_river", size, &crate::ui::View::new(&m, &ui))
            .map_or(0, ui::panels::net::wifi::river::capture_cells);
        if cells != m.net.wifi.scope.window_cells {
            m.net.wifi.scope.window_cells = cells;
            self.state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .net
                .wifi
                .scope
                .window_cells = cells;
        }
        // Copied out before the closure borrows `self` for the engine.
        let deck_shown = self.deck_shown;
        // Measurement labs wear "instrument mode": the resting frames cool toward
        // steel-blue. One per-frame tint at the draw root keeps every lab panel
        // (and its chrome) cohesive without each panel knowing about lab mode.
        let frame_theme = if ui.is_lab_mode() {
            self.theme.steeled()
        } else {
            self.theme.clone()
        };
        let view = crate::ui::View::new(&m, &ui);
        let drawn = terminal.draw(|f| {
            self.engine.draw(f, &view, &frame_theme);
            // The rail's full-log overlay only floats while the rail is focused.
            if ui.log_overlay && ui.focused_panel.as_deref() == Some("command_rail") {
                ui::overlay::render_log(f, &view, &frame_theme);
            }
            // The menu floats over the deck, the same way the log overlay does.
            // Drawn last so nothing else lands on top of it, and outside the
            // layout engine because it is not a panel.
            if let Some(menu_state) = ui.menu {
                let full = f.area();
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
                let section = ui::menu::keys_section(&self.engine, &menu_state);
                let controls = self.engine.section_controls(section, &view);
                ui::menu::render(
                    f,
                    area,
                    &view,
                    &self.engine,
                    &menu_state,
                    &controls,
                    &frame_theme,
                );
            }
            if ui.device_option_update.quit_requested() {
                let full = f.area();
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
        });
        drawn.map_err(backend_error)?;
        // The deck is behind the menu from the moment it has been drawn once
        // without one in front of it.
        let menu_up = ui.menu.is_some();
        drop(ui);
        if !menu_up {
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
        apply: impl FnOnce() -> DeviceOptionCompletion + Send + 'static,
    ) {
        std::thread::spawn(move || {
            let _ = tx.send(AppEvent::DeviceOptionComplete(apply()));
        });
    }

    /// Put the tuner back before the app goes away: see
    /// [`crate::engine::Engine::hand_back_the_tuner`].
    fn restore_sweep_tuning(&self) {
        if let Some(core) = &self.core {
            core.hand_back_the_tuner();
        }
    }

    fn save_config(&self) -> anyhow::Result<()> {
        let Some(device) = self.device.as_ref() else {
            return Ok(());
        };
        let Some(path) = &self.config_path else {
            return Ok(());
        };
        let (
            radio,
            wf_rows,
            wf_palette,
            spec_style,
            markers,
            sweep_cfg,
            night,
            display,
            station,
            ft8,
            doppler,
            floor,
        ) = {
            let m = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let loaded = RadioConfig {
                boost: self.boost_config.clone(),
                ..RadioConfig::default()
            };
            let radio = RadioConfig {
                // The recall slots are the terminal's own.
                recall_hz: crate::ui::recall_to_hz(&self.screen.ui.borrow().recall),
                ..crate::engine::boot::radio_block(&m, &loaded)
            };
            (
                radio,
                m.waterfall.buffer.max_rows,
                m.waterfall.palette,
                m.spectrum.style,
                m.spectrum.markers.clone(),
                m.sweep.config.clone(),
                m.pager.view.night,
                m.pager.view.display.clone(),
                m.home.settings(),
                m.ft8.view.settings(),
                // The terminal chooses no carriers; what the config held, or
                // monasd wrote, goes back as it was.
                crate::engine::boot::doppler_block(&m),
                // The same for the floor's bin width.
                crate::engine::boot::floor_block(&m),
            )
        };
        let engine = EngineConfig {
            radio,
            sweep: crate::config::SweepSettings {
                start_hz: sweep_cfg.start_hz,
                stop_hz: sweep_cfg.stop_hz,
                dwell_ms: sweep_cfg.dwell_ms,
            },
            tinysa: self.tinysa_config.clone(),
            airspy: self.airspy_config.clone(),
            airspyhf: self.airspyhf_config.clone(),
            sdrplay: self.sdrplay_config.clone(),
            plutosky: self.plutosky_config.clone(),
            net: self.net_config.clone(),
            record: self.record_config.clone(),
            pager: crate::config::PagerSettings { night, display },
            station,
            ft8,
            doppler,
            floor,
        };
        let tui = TuiConfig {
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
            presets: self.user_presets.clone(),
        };
        let cfg = SdrtopConfig { engine, tui };
        let mut candidate = cfg.clone();
        let hook_error = device.update_config(&mut candidate.engine).err();
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

/// The terminal backend's error as the `io::Error` the run loop speaks. A real
/// terminal's error already is one and comes back unchanged, kind and all; a
/// backend with an error type of its own (the tests' `TestBackend`, which
/// cannot fail) is wrapped.
fn backend_error<E: std::error::Error + Send + Sync + 'static>(err: E) -> io::Error {
    let boxed: Box<dyn std::error::Error + Send + Sync> = Box::new(err);
    match boxed.downcast::<io::Error>() {
        Ok(io) => *io,
        Err(other) => io::Error::other(other),
    }
}
#[cfg(test)]
mod tests {

    /// `Ctrl+R` is the recorder's, and a plain `r` is not: the handlers
    /// match the character alone, and `r` resets every setting.
    #[test]
    fn ctrl_r_is_the_record_key_and_r_is_not() {
        let ctrl = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        assert!(App::is_record_key(ctrl('r')));
        assert!(App::is_record_key(ctrl('R')));
        assert!(!App::is_record_key(KeyEvent::new(
            KeyCode::Char('r'),
            KeyModifiers::NONE
        )));
        assert!(!App::is_record_key(KeyEvent::new(
            KeyCode::Char('R'),
            KeyModifiers::SHIFT
        )));
    }

    /// **A terminal's own error keeps its kind** through `backend_error`, so
    /// the run loop reports a broken pipe as one; another backend's error
    /// becomes `Other` rather than being lost.
    #[test]
    fn a_backend_error_keeps_its_io_kind() {
        let pipe = io::Error::new(io::ErrorKind::BrokenPipe, "gone");
        let got = backend_error(pipe);
        assert_eq!(got.kind(), io::ErrorKind::BrokenPipe);
        assert_eq!(got.to_string(), "gone");
        let fmt = backend_error(std::fmt::Error);
        assert_eq!(fmt.kind(), io::ErrorKind::Other);
    }

    /// **And it is answered before any handler sees it.** The loop is not
    /// driven in a test, so its order is read from the source, the same way
    /// the registry's tests read the preset files.
    #[test]
    fn the_record_key_is_taken_before_the_key_handlers() {
        let source = include_str!("mod.rs");
        let run = &source[source.find("pub fn run<").unwrap()..];
        let record = run.find("Self::is_record_key(key)").unwrap();
        let handlers = run.find("input::handle_key(").unwrap();
        assert!(record < handlers, "Ctrl+R would reach the handlers as r");
    }
    use super::*;
    use crate::hardware::{DeviceCapabilities, DeviceInfo, RateSet};
    use crate::ui::DeviceOptionUpdate;
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

        fn set_frequency(&self, hz: u64) -> anyhow::Result<u64> {
            self.tuned_hz.store(hz, Ordering::Relaxed);
            self.tune_calls.fetch_add(1, Ordering::Relaxed);
            Ok(hz)
        }

        fn set_sample_rate(&self, hz: f64) -> anyhow::Result<RateSet> {
            Ok(RateSet::new(hz, Some(hz), 0))
        }

        fn set_lna_gain(&self, _db: u32) -> anyhow::Result<()> {
            Ok(())
        }

        fn update_config(&self, config: &mut EngineConfig) -> anyhow::Result<()> {
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
        }
        let device = Arc::new(QuitDevice::new(state.lock().unwrap().caps.as_ref().clone()));
        let path = std::env::temp_dir().join(format!(
            "sdrtop-option-quit-{}-{}.toml",
            std::process::id(),
            NEXT_PATH.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_file(&path);
        let mut config = SdrtopConfig::default();
        config.engine.tinysa.lna = true;
        let mut app = App::assemble(
            config,
            Some(path.clone()),
            state,
            Some(device.clone()),
            None,
            None,
        )
        .unwrap();
        app.screen.ui.borrow_mut().device_option_update = DeviceOptionUpdate::Pending {
            request: pending_request(),
            quit_requested: false,
        };
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
            retune: None,
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
            crate::config::load_or_default::<EngineConfig>(&path)
                .radio
                .frequency_hz,
            100_000_000
        );
        assert!(
            crate::config::load_or_default::<EngineConfig>(&path)
                .tinysa
                .lna
        );
        let m = app.state.lock().unwrap();
        assert_eq!(m.device_options[0].selected_choice, "Wide");
        assert!(matches!(
            app.screen.ui.borrow().device_option_update,
            DeviceOptionUpdate::Idle
        ));
        assert!(m
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
            SdrtopConfig::default(),
            Some(path.clone()),
            state,
            Some(device),
            None,
            None,
        )
        .unwrap();

        app.save_config().unwrap();

        let saved = crate::config::load_or_default::<EngineConfig>(&path);
        assert_eq!(saved.radio.frequency_hz, 144_390_000);
        assert_eq!(saved.tinysa.points, 900);
        std::fs::remove_file(path).unwrap();
    }

    /// The open radio's boost is saved under its name and every other radio's
    /// line is carried forward; a radio with no boost adds no line.
    #[test]
    fn the_open_radios_boost_is_saved_under_its_name() {
        let save = |board: &str, boost: Option<hardware::Boost>| {
            let state = Arc::new(Mutex::new(SdrMetrics::fixture()));
            let caps = {
                let mut m = state.lock().unwrap();
                m.system.board_name = Arc::from(board);
                m.radio.amp_enabled = true;
                let mut caps = m.caps.as_ref().clone();
                caps.gain = match boost {
                    Some(b) => caps.gain.clone().with_boost(b),
                    None => hardware::GainModel::new(caps.gain.stages().to_vec(), "Gain", "G"),
                };
                m.caps = Arc::new(caps.clone());
                caps
            };
            let device: Arc<dyn SdrDevice> = Arc::new(QuitDevice::new(caps));
            let path = std::env::temp_dir().join(format!(
                "sdrtop-boost-{}-{}.toml",
                std::process::id(),
                board.len()
            ));
            let _ = std::fs::remove_file(&path);
            let mut config = SdrtopConfig::default();
            config.engine.radio.boost.insert("Airspy R2".into(), false);
            let app =
                App::assemble(config, Some(path.clone()), state, Some(device), None, None).unwrap();
            app.save_config().unwrap();
            let saved = crate::config::load_or_default::<EngineConfig>(&path);
            std::fs::remove_file(path).unwrap();
            saved.radio.boost
        };
        let amp = || Some(hardware::Boost::GainMode);
        let saved = save("HackRF One", amp());
        assert_eq!(saved.get("HackRF One"), Some(&true));
        assert_eq!(saved.get("Airspy R2"), Some(&false), "carried forward");
        let saved = save("tinySA", None);
        assert_eq!(saved.len(), 1, "no boost, no line: {saved:?}");
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
        let mut config = SdrtopConfig::default();
        config.engine.tinysa.points = 450;
        config.engine.tinysa.lna = true;
        let app =
            App::assemble(config, Some(path.clone()), state, Some(device), None, None).unwrap();

        let error = format!("{:#}", app.save_config().unwrap_err());

        assert!(error.contains("device settings could not be updated"));
        assert!(error.contains("previous device settings and other settings were saved"));
        assert!(error.contains("injected backend config failure"));
        let saved = SdrtopConfig::load_or_default(&path);
        assert_eq!(saved.engine.radio.frequency_hz, 433_920_000);
        assert_eq!(saved.engine.tinysa.points, 450);
        assert!(saved.engine.tinysa.lna);
        assert_eq!(saved.tui.display.spectrum_markers.len(), 1);
        assert_eq!(saved.tui.display.spectrum_markers[0].freq_hz, 434_500_000);
        std::fs::remove_file(path).unwrap();
    }

    /// **Quitting with settings that cannot be written is a warning, not a
    /// failed exit**: the session ends as asked, and what was not saved, and
    /// where, is kept for the line printed once the terminal is back.
    #[test]
    fn quitting_with_settings_that_cannot_be_written_warns_and_ends_the_session() {
        let state = Arc::new(Mutex::new(SdrMetrics::fixture()));
        let device = Arc::new(QuitDevice::new(state.lock().unwrap().caps.as_ref().clone()));
        let blocker =
            std::env::temp_dir().join(format!("sdrtop-quit-unsaved-{}", std::process::id()));
        std::fs::write(&blocker, "").unwrap();
        let path = blocker.join("config.toml");
        let mut app = App::assemble(
            SdrtopConfig::default(),
            Some(path),
            state,
            Some(device),
            None,
            None,
        )
        .unwrap();

        assert!(app.finish_session().is_ok());
        let warning = app.unsaved().expect("the failed save is kept").to_string();
        assert!(warning.contains("not all settings were saved"), "{warning}");
        assert!(
            warning.contains(&blocker.display().to_string()),
            "{warning}"
        );
        let _ = std::fs::remove_file(&blocker);
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
            SdrtopConfig::default(),
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
    fn ordinary_quit_warns_of_a_save_failure_and_preserves_existing_config() {
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
            SdrtopConfig::default(),
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

        // The quit itself succeeds; the save failure is a warning that names
        // the file, and the config already there is left as it was.
        app.run(&mut terminal).unwrap();

        let warning = app.unsaved().expect("the failed save is kept");
        assert!(warning.contains("config.tmp"), "{warning}");
        assert_eq!(std::fs::read(&path).unwrap(), original);
        std::fs::remove_dir(path.with_extension("tmp")).unwrap();
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn terminal_input_errors_are_returned_from_the_event_loop() {
        let (mut app, tx, _, path) = quit_test_app();
        tx.send(AppEvent::InputError(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "terminal input failed",
        )))
        .unwrap();
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();

        let error = app.run(&mut terminal).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
        assert_eq!(error.to_string(), "terminal input failed");
        assert!(!path.exists());
    }

    #[test]
    fn numeric_menu_entry_preserves_the_deck_footer_setting() {
        use crate::ui::{InputMode, MenuFocus, MenuState, MenuTile};
        let (mut app, _, _, _) = quit_test_app();
        app.engine.set_preset("command_rail");
        {
            let mut ui = app.screen.ui.borrow_mut();
            ui.device_option_update = DeviceOptionUpdate::Idle;
            ui.menu = Some(MenuState {
                tile: MenuTile::Options,
                focus: MenuFocus::Below,
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
                app.screen.ui.borrow_mut().input_mode = mode;
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
            retune: None,
            result: Err("device rejected choice".into()),
        }))
        .unwrap();
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();

        app.run(&mut terminal).unwrap();

        assert_eq!(device.tune_calls.load(Ordering::Relaxed), 1);
        assert!(path.exists());
        let m = app.state.lock().unwrap();
        assert!(matches!(
            &app.screen.ui.borrow().device_option_update,
            DeviceOptionUpdate::Failed { id } if id == "bandwidth"
        ));
        assert!(m.log.back().is_some_and(|entry| entry
            .text
            .contains("Bandwidth error: device rejected choice")));
        drop(m);
        std::fs::remove_file(path).unwrap();
    }

    /// **Ctrl+C quits and saves nothing**, as the key guide says: no
    /// config file is written, where `q` writes one. Before, it reached the
    /// key handlers as a plain `c` and focused a panel.
    #[test]
    fn control_c_quits_without_saving() {
        let (mut app, tx, _device, path) = quit_test_app();
        app.screen.ui.borrow_mut().device_option_update = DeviceOptionUpdate::Idle;
        tx.send(AppEvent::Key(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
        )))
        .unwrap();
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        app.run(&mut terminal).unwrap();
        assert!(!path.exists(), "Ctrl+C wrote the config");
        assert_eq!(app.screen.ui.borrow_mut().focused_panel, None);

        // And `q`, for contrast, does write it.
        let (mut app, tx, _device, path) = quit_test_app();
        app.screen.ui.borrow_mut().device_option_update = DeviceOptionUpdate::Idle;
        tx.send(AppEvent::Key(KeyEvent::from(KeyCode::Char('q'))))
            .unwrap();
        app.run(&mut terminal).unwrap();
        assert!(path.exists(), "q did not write the config");
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
                app.screen.ui.borrow().device_option_update.is_pending(),
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
        let screen = crate::ui::Screen::of(SdrMetrics::fixture());
        let state = Arc::clone(&screen.m);
        let request = DeviceOptionRequest {
            id: "bandwidth".into(),
            label: "Bandwidth".into(),
            choice: "Wide".into(),
        };
        screen.ui.borrow_mut().device_option_update = DeviceOptionUpdate::Pending {
            request: request.clone(),
            quit_requested: false,
        };
        let (event_tx, event_rx) = mpsc::channel();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let worker_state = Arc::clone(&state);
        let worker_request = request.clone();

        App::spawn_device_option_task(event_tx, move || DeviceOptionCompletion {
            retune: None,
            result: crate::engine::option::set(
                &worker_request.id,
                &worker_request.choice,
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
            .map_err(|error| format!("{error:#}")),
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
        input::complete_device_option(&screen, completion);

        let m = state.lock().unwrap();
        assert_eq!(m.device_options.len(), 2);
        assert_eq!(m.device_options[0].selected_choice, "Wide");
        assert_eq!(m.device_options[1].selected_choice, "0 dB");
    }

    /// A radio on a kHz tuning grid whose `grid` option moves the tuning.
    struct GridDevice(DeviceCapabilities);

    impl SdrDevice for GridDevice {
        fn capabilities(&self) -> &DeviceCapabilities {
            &self.0
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
        fn set_frequency(&self, hz: u64) -> anyhow::Result<u64> {
            if hz < 1_000 {
                anyhow::bail!("below the grid");
            }
            Ok((hz + 500) / 1000 * 1000)
        }
        fn set_sample_rate(&self, hz: f64) -> anyhow::Result<RateSet> {
            Ok(RateSet::new(hz, Some(hz), 0))
        }
        fn set_lna_gain(&self, _db: u32) -> anyhow::Result<()> {
            Ok(())
        }
        fn option_moves_tuning(&self, id: &str) -> bool {
            id == "grid"
        }
    }

    /// **An option that moves the tuning is followed by the frequency**, and
    /// the state keeps where the radio says it landed, with a log line when
    /// that is not where it was; a refusal is logged and leaves the state. An
    /// option that does not move it is not followed.
    #[test]
    fn an_option_that_moves_the_tuning_retunes_and_keeps_the_answer() {
        let device = GridDevice(SdrMetrics::fixture().caps.as_ref().clone());
        let screen = crate::ui::Screen::of(SdrMetrics::fixture());
        let state = Arc::clone(&screen.m);
        state.lock().unwrap().radio.frequency = 7_074_300;
        assert_eq!(
            crate::engine::option::retune_after(&device, "other", &state),
            None
        );
        let retune = crate::engine::option::retune_after(&device, "grid", &state);
        assert_eq!(retune, Some(Ok(7_074_000)));

        let request = DeviceOptionRequest {
            id: "grid".into(),
            label: "Grid".into(),
            choice: "On".into(),
        };
        let complete = |retune| {
            screen.ui.borrow_mut().device_option_update = DeviceOptionUpdate::Pending {
                request: request.clone(),
                quit_requested: false,
            };
            input::complete_device_option(
                &screen,
                DeviceOptionCompletion {
                    result: Ok(Vec::new()),
                    retune,
                },
            );
        };
        complete(retune);
        {
            let m = state.lock().unwrap();
            assert_eq!(m.radio.frequency, 7_074_000);
            assert!(m
                .log
                .back()
                .is_some_and(|e| e.text.contains("Grid: tuned to 7.074000 MHz, -300 Hz")));
        }
        complete(Some(Err("below the grid".into())));
        let m = state.lock().unwrap();
        assert_eq!(m.radio.frequency, 7_074_000);
        assert!(m
            .log
            .back()
            .is_some_and(|e| e.text.contains("Grid: retune refused: below the grid")));
    }

    #[test]
    fn failed_device_option_set_does_not_refresh() {
        let request = DeviceOptionRequest {
            id: "bandwidth".into(),
            label: "Bandwidth".into(),
            choice: "Wide".into(),
        };
        let refreshed = Cell::new(false);

        let result = crate::engine::option::set(
            &request.id,
            &request.choice,
            |_, _| anyhow::bail!("device rejected choice"),
            || {
                refreshed.set(true);
                Vec::new()
            },
        );

        assert!(result.is_err());
        assert!(!refreshed.get());
    }

    /// **The view and the mode it asks for are set on the shared state.**
    ///
    /// The engine reads the mode off this thread: the hot path gates the
    /// NET, pager and FT8 feeds on it and `tasks::net` decides whether and
    /// how to survey by it. Both bugs this replaces had the same shape: the
    /// section, then the layout, written to `m`, the snapshot, one line after
    /// `guard.clone()`, so every panel looked right and every reader on
    /// another thread saw an empty string for ever.
    #[test]
    fn the_view_and_its_mode_are_set_on_the_shared_state_and_not_only_the_snapshot() {
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
            guard_side.contains("guard.show_view(&section, &active_preset)"),
            "the view must be set on the shared state, where the engine reads its mode"
        );
        // The screen's own record of what it shows is written after the
        // clone, into its own state, which no other thread reads: only the
        // engine's mode has to be on the shared side.
        for late in ["show_view(", ".mode ="] {
            assert!(
                !snapshot_side.contains(late),
                "`{late}` after the clone reaches only the snapshot, never the engine"
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

    /// **The recall slots are the screen's, filled from the config**, and
    /// empty when another process holds the radio: a recall jump retunes it,
    /// which observer mode cannot do.
    #[test]
    fn recall_slots_come_from_the_config_and_stay_empty_in_observer_mode() {
        let mut config = SdrtopConfig::default();
        config.engine.radio.recall_hz = [100_000_000, 0, 0];
        let open = App::assemble(
            config.clone(),
            None,
            Arc::new(Mutex::new(SdrMetrics::fixture())),
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(open.screen.ui.borrow().recall[0], Some(100_000_000));

        let mut observed = SdrMetrics::fixture();
        observed.observer.active = true;
        let watching = App::assemble(
            config,
            None,
            Arc::new(Mutex::new(observed)),
            None,
            None,
            Some("observer"),
        )
        .unwrap();
        assert!(watching
            .screen
            .ui
            .borrow()
            .recall
            .iter()
            .all(|s| s.is_none()));
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
