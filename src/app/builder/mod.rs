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
use crate::signal::{DemodWorker, FftWorker, NetWorker, PowerWorker};
use crate::state::SdrMetrics;
use crate::tasks;

use boot::{initial_metrics, resolve_tuning, Boot};

use super::App;

impl App {
    /// `observable` is whether observer mode could watch this radio were
    /// another process to hold it: `DeviceKind::observer_profile`, asked by
    /// `App::new`, which has the listing.
    pub(super) fn new_normal(
        cfg: AppConfig,
        config_path: Option<PathBuf>,
        device: Arc<dyn hardware::SdrDevice>,
        observable: bool,
    ) -> anyhow::Result<Self> {
        let info = device.info();
        let caps = Arc::new(device.capabilities().clone());
        let geometry = caps.sample_geometry;

        let mut tuning = resolve_tuning(&cfg.radio, &caps);
        // Taken out before `tuning` is moved into `Boot`, rather than cloning
        // the whole thing to keep them.
        let mut boot_notes = std::mem::take(&mut tuning.notes);
        let asked_rate = tuning.sample_rate;
        let sr_result = match device.set_sample_rate(asked_rate) {
            // The device is the authority on both figures: the baseband width it
            // actually selected, and the rate it actually landed on. The computed
            // width and the requested rate stand in only when the call failed.
            Ok(set) => {
                tuning.sample_rate = set.rate_hz;
                tuning.bb_filter_hz = set.bb_filter_hz;
                // A driver that rounded the configured rate onto its own grid
                // would otherwise change it silently, and the config is rewritten
                // on quit, so the user's own figure would disappear without ever
                // having been contradicted.
                if set.rate_hz != asked_rate {
                    boot_notes.push(format!(
                        "Sample rate {:.6} MHz is not on this radio's grid; running at {:.6} MHz",
                        asked_rate / 1e6,
                        set.rate_hz / 1e6
                    ));
                }
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

        let device_options = device.options();
        hardware::debug_assert_device_options(&device_options);
        let mut initial = initial_metrics(
            &cfg,
            Boot::normal(&cfg, Arc::clone(&caps), tuning, &info, observable),
        )?;
        initial.device_options = Arc::new(device_options);
        let state = Arc::new(Mutex::new(initial));

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
                hardware::native::hackrf::board_rev_name(m.system.board_rev),
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
            } else if caps.acquisition == hardware::AcquisitionKind::IqSamples {
                m.push_log(board);
            }
            // Anything the backend declined while opening. Both native paths
            // have nothing to say; a SoapySDR device names any gain element
            // whose range the driver described unusably.
            for note in device.open_notes() {
                m.push_log(note.clone());
            }
            // Anything the configured tuning asked for and did not get: a gain
            // stage name this radio does not have, an entry that is not
            // `NAME=value` at all, or a sample rate the driver rounded. The gain
            // ones are computed by `resolve_tuning`, which is pure, and surfaced
            // here.
            for note in &boot_notes {
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
        let (demod_tx, demod_rx) = crossbeam_channel::bounded::<crate::hardware::StreamBlock>(2);
        // The NET queue is deeper than the demod's and for the opposite reason.
        // The demod duty-cycles to four updates a second, so anything deeper
        // than two would only hold blocks it will discard. This worker is meant
        // to see every block, and four is the same depth the FFT feed uses: a
        // burst that spans several driver blocks survives a hiccup on the UI
        // thread, and anything deeper starts hiding the losses rather than
        // absorbing them.
        let (net_tx, net_rx) = crossbeam_channel::bounded::<crate::hardware::StreamBlock>(4);
        // Read out before `cfg` is moved into `Self::assemble` below - a
        // plain `usize`, not worth threading the whole config through the
        // worker for.
        let bt_channels = cfg.net.bt_channels;
        let (power_tx, power_rx) = crossbeam_channel::bounded::<hardware::PowerTrace>(4);
        let rx_ctx = Arc::new(hardware::RxContext {
            metrics: Arc::clone(&state),
            sample_tx,
            fft_feed: hardware::FeedHealth::default(),
            demod_tx,
            net_tx,
            net_feed: hardware::FeedHealth::default(),
            power_tx,
            geometry,
            blocks_seen: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            stream_pairs: std::sync::atomic::AtomicU64::new(0),
        });

        let app = Self::assemble(
            cfg,
            config_path,
            Arc::clone(&state),
            Some(Arc::clone(&device)),
            Some(Arc::clone(&rx_ctx)),
            None,
        )?;

        match caps.acquisition {
            hardware::AcquisitionKind::IqSamples => {
                let fft_state = Arc::clone(&state);
                spawn_worker("fft-worker", move || {
                    FftWorker::new(sample_rx, fft_state, geometry).run()
                });

                let demod_state = Arc::clone(&state);
                spawn_worker("demod-worker", move || {
                    DemodWorker::new(demod_rx, demod_state, geometry).run()
                });

                // Spawned whether or not the gate admitted the section: with no section
                // on screen nothing is forwarded, so the thread costs one blocked
                // `recv`. Deciding here would mean two places that know what admits the
                // feature, and the one that already knows is `net::gate`.
                let net_state = Arc::clone(&state);
                spawn_worker("net-worker", move || {
                    NetWorker::new(net_rx, net_state, geometry, bt_channels).run()
                });

                tasks::spawn_rx_task(Arc::clone(&state), Arc::clone(&device), Arc::clone(&rx_ctx));
                tasks::spawn_net_survey_task(Arc::clone(&state), Arc::clone(&device));
            }
            hardware::AcquisitionKind::PowerTrace => {
                let power_state = Arc::clone(&state);
                spawn_worker("power-worker", move || {
                    PowerWorker::new(power_rx, power_state).run()
                });
                tasks::spawn_power_rx_task(
                    Arc::clone(&state),
                    Arc::clone(&device),
                    Arc::clone(&rx_ctx),
                );
            }
        }

        tasks::spawn_sweep_task(Arc::clone(&state), Arc::clone(&device));
        tasks::spawn_sys_resource_task(Arc::clone(&state));

        Ok(app)
    }

    pub(super) fn new_observer(
        cfg: AppConfig,
        config_path: Option<PathBuf>,
        sysinfo: hardware::sysfs::HackRfSysInfo,
        profile: hardware::discovery::ObserverProfile,
    ) -> anyhow::Result<Self> {
        let state = Arc::new(Mutex::new(initial_metrics(
            &cfg,
            Boot::observer(&cfg, &sysinfo, profile),
        )?));

        {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            let line = format!(
                "Observer Mode: {} (Serial: {})",
                m.system.board_name, m.system.serial
            );
            m.push_log(line);
            m.push_log("Device is in use by another process — hardware controls disabled");
        }

        let app = Self::assemble(
            cfg,
            config_path,
            Arc::clone(&state),
            None,
            None,
            Some("observer"),
        )?;
        tasks::spawn_observer_task(Arc::clone(&state), sysinfo.bus, sysinfo.dev, profile);
        tasks::spawn_sys_resource_task(Arc::clone(&state));
        Ok(app)
    }

    /// The tail both startups end in: resolve the config paths, build the theme
    /// and the layout, and hand the config's own copies of the user presets and
    /// theme block to `App` so `save_config` can write them back.
    ///
    /// `preset_override` is `None` for "whatever the config asks for". Observer
    /// mode passes `Some("observer")` because its layout is the only one that
    /// says anything useful with no stream behind it.
    pub(super) fn assemble(
        cfg: AppConfig,
        config_path: Option<PathBuf>,
        state: Arc<Mutex<SdrMetrics>>,
        device: Option<Arc<dyn hardware::SdrDevice>>,
        rx_ctx: Option<Arc<hardware::RxContext>>,
        preset_override: Option<&str>,
    ) -> anyhow::Result<Self> {
        let themes_dir = config_path
            .as_deref()
            .and_then(crate::config::AppConfig::themes_dir);
        let presets_dir = config_path
            .as_deref()
            .and_then(crate::config::LayoutConfig::presets_dir);
        let theme = cfg.build_theme(themes_dir.as_deref());

        // Whether the NET section exists at all, decided once, from the radio's
        // own declaration. The reason travels with the decision so the log and
        // the menu cannot end up disagreeing about why a section is missing.
        let net = {
            let m = state.lock().unwrap_or_else(|e| e.into_inner());
            crate::signal::net::gate::verdict(&m.caps)
        };

        let active = preset_override.unwrap_or(&cfg.display.active_preset);
        let acquisition = state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .caps
            .acquisition;
        let (engine, focus_keys) = Self::build_ui_for(
            active,
            &cfg.presets,
            presets_dir.as_deref(),
            net.is_ok(),
            acquisition,
        )?;

        // A user preset that wanted a number key already taken says so, once,
        // here. `menu::model::build` collects these instead of logging them so it
        // can stay a pure function; this is the one place that has both the
        // warnings and the lock.
        {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            for warning in engine.menu_warnings() {
                m.push_log(warning.clone());
            }
            for warning in engine.startup_warnings() {
                m.push_log(warning.clone());
            }
            if let Err(why) = &net {
                m.push_log(why.clone());
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

        Ok(Self {
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
            tinysa_config: cfg.tinysa.clone(),
            net_config: cfg.net.clone(),
            user_presets: cfg.presets,
        })
    }
}

/// Spawns one of the long-lived DSP workers under a name, so `top -H` and btop
/// can say which of them is spending the CPU. Linux keeps the first 15 bytes.
/// An unnamed thread shows only the process name, and then the NET, FFT and
/// demod workers cannot be told apart from each other or from the driver's own
/// USB thread. Failing to spawn is as fatal here as in `std::thread::spawn`.
fn spawn_worker(name: &str, run: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .name(name.to_string())
        .spawn(run)
        .unwrap_or_else(|e| panic!("cannot start the {name} thread: {e}"));
}
