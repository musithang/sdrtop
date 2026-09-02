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
                // to read-only observer mode via the matching backend's sysfs scan.
                let sysinfo = match listing.kind {
                    hardware::DeviceKind::HackRf => hardware::sysfs::find_hackrf(),
                    hardware::DeviceKind::RtlSdr => hardware::sysfs::find_rtlsdr(),
                    // Observer mode reads sysfs for a USB device sdrtop knows
                    // by name. There is no generic equivalent for "whatever
                    // SoapySDR was talking to", so a Soapy device that will not
                    // open simply will not open, and says why.
                    hardware::DeviceKind::Soapy => None,
                };
                let Some(sysinfo) = sysinfo else {
                    return Err(open_err);
                };
                Self::new_observer(cfg, config_path, sysinfo, listing.kind)
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

    fn save_config(&self) {
        if self.device.is_none() {
            return;
        }
        let Some(path) = &self.config_path else {
            return;
        };
        let (
            freq,
            rate,
            lna,
            vga,
            amp,
            wf_rows,
            wf_palette,
            spec_style,
            markers,
            sweep_cfg,
            recall,
        ) = {
            let m = self.state.lock().unwrap_or_else(|e| e.into_inner());
            (
                m.radio.frequency,
                m.radio.config_sample_rate,
                m.radio.lna_gain,
                m.radio.vga_gain,
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
                lna_gain: lna,
                vga_gain: vga,
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
