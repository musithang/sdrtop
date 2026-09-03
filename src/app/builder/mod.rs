// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Startup: opening the session the rest of the app runs in.
//!
//! Two ways in, split by whether the radio is ours to command:
//!
//! - [`App::new_normal`] - the device opened, so it gets programmed, streamed
//!   from, and swept.
//! - [`App::new_observer`] - another process holds it, so everything is read
//!   from sysfs and every control is inert.
//!
//! What they share is factored by kind rather than by order: [`boot`] owns the
//! `SdrMetrics` a session starts with (and the startup clamp, which is pure and
//! therefore testable without a radio), [`registry`] owns the panel registry and
//! the layout engine, and [`App::assemble`] is the tail both paths end in.

mod boot;
mod registry;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::config::AppConfig;
use crate::event::EventStream;
use crate::hardware;
use crate::signal::{DemodWorker, FftWorker};
use crate::state::SdrMetrics;
use crate::tasks;

use boot::{initial_metrics, resolve_tuning, Boot};

use super::App;

impl App {
    pub(super) fn new_normal(
        cfg: AppConfig,
        config_path: Option<PathBuf>,
        device: Arc<dyn hardware::SdrDevice>,
    ) -> anyhow::Result<Self> {
        let info = device.info();
        let caps = Arc::new(device.capabilities().clone());
        let geometry = caps.sample_geometry;

        let mut tuning = resolve_tuning(&cfg.radio, &caps);
        // Taken out before `tuning` is moved into `Boot`, rather than cloning
        // the whole thing to keep them.
        let gain_notes = std::mem::take(&mut tuning.notes);
        let sr_result = match device.set_sample_rate(tuning.sample_rate) {
            // The device is the authority on the baseband width it actually
            // selected; the computed value stands in only when the call failed.
            Ok(bw) => {
                tuning.bb_filter_hz = bw;
                Ok(())
            }
            Err(e) => Err(e),
        };
        // `amp_enabled` is the front-end-boost state for both device families:
        // HackRF's RF amp (set_amp_enable) and RTL-SDR's tuner AGC (set_tuner_agc).
        // Calling both applies the right one per device (the other is a no-op) so
        // the programmed state matches what the UI shows.
        // One call per stage the device actually has, in its own order. The two
        // named setters are what the default `set_stage_gain` maps onto, so both
        // native radios are programmed exactly as they always were.
        let stages = caps.gain.stages();
        let mut startup_results = vec![device.set_frequency(tuning.frequency_hz), sr_result];
        for (index, spec) in stages.iter().enumerate() {
            let db = tuning.gains.get(index).copied().unwrap_or(spec.min_db);
            startup_results.push(device.set_stage_gain(index, &spec.name, db));
        }
        startup_results.push(device.set_amp_enable(cfg.radio.amp_enabled));
        startup_results.push(device.set_tuner_agc(cfg.radio.amp_enabled));

        let state = Arc::new(Mutex::new(initial_metrics(
            &cfg,
            Boot::normal(&cfg, Arc::clone(&caps), tuning, &info),
        )));

        {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            // Read the identity back out of the state rather than off `info`
            // again: `Boot::normal` already applied the "unknown" / `0xFE`
            // fall-backs, and the log must say the same thing the header does.
            let connected = format!(
                "Connected: {} | Serial: {}",
                m.system.board_name, m.system.serial
            );
            let board = format!(
                "Board: {} | USB API: {:#06x}",
                hardware::board_rev_name(m.system.board_rev),
                m.system.usb_api_version
            );
            m.push_log(connected);
            // Firmware is a HackRF concept; RTL-SDR (no on-device FW) skips it.
            if let Some(fw) = &info.fw_version {
                m.push_log(format!("Firmware: {}", fw));
            }
            // RTL-SDR reports a tuner instead of a board revision / USB-API version.
            if let Some(tuner) = &info.tuner_name {
                m.push_log(format!("Tuner: {}", tuner));
            } else {
                m.push_log(board);
            }
            // Anything the backend declined while opening. Both native paths
            // have nothing to say; a SoapySDR device names any gain element
            // whose range the driver described unusably.
            for note in device.open_notes() {
                m.push_log(note.clone());
            }
            // Anything the configured `gain` string asked for and did not get:
            // a stage name this radio does not have, or an entry that is not
            // `NAME=value` at all. Computed by `resolve_tuning`, which is pure,
            // and surfaced here.
            for note in &gain_notes {
                m.push_log(note.clone());
            }
            let names = [
                "frequency",
                "sample rate",
                "LNA gain",
                "VGA gain",
                "amp",
                "tuner AGC",
            ];
            for (result, name) in startup_results.iter().zip(names.iter()) {
                if let Err(e) = result {
                    m.push_log(format!("Startup: failed to set {}: {}", name, e));
                }
            }
        }

        let (sample_tx, sample_rx) = crossbeam_channel::bounded::<Vec<u8>>(4);
        // The demod queue is deliberately shallow: it duty-cycles to one update
        // per 250 ms, so anything deeper would only hold blocks it will discard.
        let (demod_tx, demod_rx) = crossbeam_channel::bounded::<(u64, Vec<u8>)>(2);
        let rx_ctx = Arc::new(hardware::RxContext {
            metrics: Arc::clone(&state),
            sample_tx,
            demod_tx,
            geometry,
        });

        let fft_state = Arc::clone(&state);
        std::thread::spawn(move || FftWorker::new(sample_rx, fft_state, geometry).run());

        let demod_state = Arc::clone(&state);
        std::thread::spawn(move || DemodWorker::new(demod_rx, demod_state, geometry).run());

        tasks::spawn_rx_task(Arc::clone(&state), Arc::clone(&device), Arc::clone(&rx_ctx));
        tasks::spawn_sweep_task(Arc::clone(&state), Arc::clone(&device));
        tasks::spawn_sys_resource_task(Arc::clone(&state));

        Ok(Self::assemble(
            cfg,
            config_path,
            state,
            Some(device),
            Some(rx_ctx),
            None,
        ))
    }

    pub(super) fn new_observer(
        cfg: AppConfig,
        config_path: Option<PathBuf>,
        sysinfo: hardware::sysfs::HackRfSysInfo,
        kind: hardware::DeviceKind,
    ) -> anyhow::Result<Self> {
        let state = Arc::new(Mutex::new(initial_metrics(
            &cfg,
            Boot::observer(&cfg, &sysinfo, kind),
        )));

        {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            let line = format!(
                "Observer Mode: {} (Serial: {})",
                m.system.board_name, m.system.serial
            );
            m.push_log(line);
            m.push_log("Device is in use by another process — hardware controls disabled");
        }

        tasks::spawn_observer_task(Arc::clone(&state), sysinfo.bus, sysinfo.dev, kind);
        tasks::spawn_sys_resource_task(Arc::clone(&state));

        Ok(Self::assemble(
            cfg,
            config_path,
            state,
            None,
            None,
            Some("observer"),
        ))
    }

    /// The tail both startups end in: resolve the config paths, build the theme
    /// and the layout, and hand the config's own copies of the user presets and
    /// theme block to `App` so `save_config` can write them back.
    ///
    /// `preset_override` is `None` for "whatever the config asks for". Observer
    /// mode passes `Some("observer")` because its layout is the only one that
    /// says anything useful with no stream behind it.
    fn assemble(
        cfg: AppConfig,
        config_path: Option<PathBuf>,
        state: Arc<Mutex<SdrMetrics>>,
        device: Option<Arc<dyn hardware::SdrDevice>>,
        rx_ctx: Option<Arc<hardware::RxContext>>,
        preset_override: Option<&str>,
    ) -> Self {
        let themes_dir = config_path
            .as_deref()
            .and_then(crate::config::AppConfig::themes_dir);
        let presets_dir = config_path
            .as_deref()
            .and_then(crate::config::LayoutConfig::presets_dir);
        let theme = cfg.build_theme(themes_dir.as_deref());

        let active = preset_override.unwrap_or(&cfg.display.active_preset);
        let (engine, focus_keys) = Self::build_ui(active, &cfg.presets, presets_dir.as_deref());

        // A user preset that wanted a number key already taken says so, once,
        // here. `menu::model::build` collects these instead of logging them so it
        // can stay a pure function; this is the one place that has both the
        // warnings and the lock.
        {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            for warning in engine.menu_warnings() {
                m.push_log(warning.clone());
            }

            // The menu is the first screen. The cursor starts on the layout the
            // config restored, so `Enter` resumes and no other key is needed for
            // it: resume is where the cursor is, not a command of its own. On a
            // first run, or after the config names a layout the menu hides, there
            // is nothing to restore and the cursor starts at the top.
            let (section, entry) = engine
                .menu()
                .locate(engine.active_preset())
                .unwrap_or((0, 0));
            m.ui.menu = Some(crate::state::MenuState {
                section,
                entry,
                pane: crate::state::MenuPane::Views,
                scroll: 0,
            });
        }

        Self {
            state,
            device,
            rx_ctx,
            config_path,
            events: EventStream::new(Duration::from_millis(33)),
            show_footer: true,
            deck_shown: false,
            engine,
            theme,
            focus_keys,
            theme_config: cfg.theme.clone(),
            user_presets: cfg.presets,
        }
    }
}
