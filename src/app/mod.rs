// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

mod builder;
pub mod input;

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ratatui::{backend::Backend, Terminal};

use crate::config::{AppConfig, DisplayConfig, RadioConfig};
use crate::event::{AppEvent, EventStream};
use crate::hardware::{self, RxContext, SdrDevice};
use crate::state::SdrMetrics;
use crate::ui;

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
    pub(super) focus_keys: HashMap<char, &'static str>,
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
}

impl App {
    pub fn new(
        cfg: AppConfig,
        config_path: Option<PathBuf>,
        listing: &hardware::DeviceListing,
    ) -> anyhow::Result<Self> {
        match hardware::open_device(listing) {
            Ok(device) => Self::new_normal(cfg, config_path, device),
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
                    match input::handle_key(
                        key,
                        &self.state,
                        self.device.as_ref(),
                        &mut self.engine,
                        &mut self.show_footer,
                        &self.focus_keys,
                    ) {
                        input::KeyAction::Quit => {
                            self.restore_noise_sweep();
                            self.restore_sweep_tuning();
                            self.save_config();
                            return Ok(());
                        }
                        input::KeyAction::Continue => {}
                    }
                    last_draw.elapsed() >= FRAME_DURATION
                }
                AppEvent::Tick => true,
            };

            if needs_redraw {
                self.draw(terminal)?;
                last_draw = Instant::now();
            }
        }
    }

    fn draw<B: Backend>(&mut self, terminal: &mut Terminal<B>) -> io::Result<()> {
        // Sweep mode is owned by the `lab_sweep` preset: keep the real state's
        // `sweep.active` in sync with the active preset so the sweep_task starts
        // and stops with it, then take the render snapshot.
        let active_preset = self.engine.active_preset().to_string();
        let sweep_active = active_preset == "lab_sweep" || active_preset == "micro_sweep";
        // The demod is gated on its panel being on screen, not on the preset being
        // called `lab_signal`: presets are data, and a user preset that lists
        // `fm_demod` used to get a panel that never received a block - it sat at
        // "DEMOD IDLE — waiting for a usable channel" forever on a station the
        // built-in preset locked onto instantly. Asking the engine which panels are
        // active is how the rest of the layout already works. The gate itself stays,
        // so the extra per-block copy still costs nothing on every screen without it.
        let demod_preset = self.engine.is_panel_visible("fm_demod");
        let mut m = {
            let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
            guard.sweep.active = sweep_active;
            guard.demod.enabled = demod_preset && guard.demod.user_on;
            guard.clone()
        };
        // Mirror the engine's active preset into the cloned snapshot so the
        // footer can render it without reaching into the engine.
        m.ui.active_preset = active_preset;
        m.ui.preset_names = self.engine.preset_names();
        // The footer names the keys that work right now, and the digits are
        // scoped, so it reads the active section rather than keeping a table.
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
        let hide_footer = !self.show_footer && m.ui.input_mode == crate::state::InputMode::Normal;
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
                ui::menu::render(f, area, &m, self.engine.menu(), &menu_state, &frame_theme);
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
    /// Harmless on a state that was not sweeping, so it is not conditional.
    fn restore_sweep_tuning(&self) {
        let moved = {
            let mut m = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let tuned = m.radio.frequency;
            let exit = m.sweep.end(tuned);
            m.radio.frequency = exit.tune_hz;
            (exit.tune_hz != tuned).then_some(exit.tune_hz)
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

    fn save_config(&self) {
        if self.device.is_none() {
            return;
        }
        let Some(path) = &self.config_path else {
            return;
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
                active_preset: self.engine.active_preset().to_string(),
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
            presets: self.user_presets.clone(),
        };
        let _ = cfg.save(path);
    }
}

#[cfg(test)]
mod tests {
    /// Quitting must give the tuner back before the config is written.
    ///
    /// `save_config` persists `radio.frequency`, and while a sweep is running
    /// that field holds the position the scan has reached, not anything the user
    /// tuned. The ordering of the two calls is the whole of the fix, and nothing
    /// in the type system holds it: both are `&self` methods returning `()`, so
    /// swapping them or dropping one still compiles and quietly puts the bug
    /// back. Read as source text for the same reason the dispatch table is.
    #[test]
    fn quitting_gives_the_tuner_back_before_saving_the_config() {
        let arm = include_str!("mod.rs")
            .split_once("KeyAction::Quit => {")
            .expect("the quit arm has been renamed")
            .1
            .split_once("return Ok(());")
            .expect("the quit arm no longer returns")
            .0;
        let restore = arm.find("self.restore_sweep_tuning()").expect(
            "quitting no longer ends the sweep, so save_config writes out \
             whichever position the scan was parked on as the tuned frequency",
        );
        let save = arm
            .find("self.save_config()")
            .expect("the quit arm no longer saves the config");
        assert!(
            restore < save,
            "restore_sweep_tuning must run before save_config, or the config \
             still records the scan position"
        );
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
