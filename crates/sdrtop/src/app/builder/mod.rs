// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Startup: opening the session the rest of the app runs in.
//!
//! Two ways in, split by whether the radio is ours to command:
//!
//! - [`App::new_normal`] - the device opened, so it gets programmed, streamed
//!   from, and swept.
//! - [`App::new_observer`] - another process holds it, so everything is read
//!   from the USB scan and every control is inert.
//!
//! What they share is factored by kind rather than by order: the engine's
//! `boot` owns the `SdrMetrics` a session starts with (and the startup clamp,
//! which is pure and therefore testable without a radio), [`registry`] owns
//! the panel registry and the layout engine, and [`App::assemble`] is the tail
//! both paths end in.

mod registry;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::config::SdrtopConfig;
use crate::event::EventStream;
use crate::hardware;
use crate::state::SdrMetrics;

use crate::engine::boot::{initial_metrics, Boot};

use super::App;

impl App {
    /// `observable` is whether observer mode could watch this radio were
    /// another process to hold it: `DeviceKind::observer_profile`, asked by
    /// `App::new`, which has the listing.
    pub(super) fn new_normal(
        cfg: SdrtopConfig,
        config_path: Option<PathBuf>,
        device: Arc<dyn hardware::SdrDevice>,
        observable: bool,
    ) -> anyhow::Result<Self> {
        let geometry = device.capabilities().sample_geometry;
        let state = Arc::new(Mutex::new(crate::engine::start::open(
            &cfg.engine,
            cfg.tui.display.shown(),
            &device,
            observable,
        )?));

        // Read out before `cfg` is moved into `Self::assemble` below - a
        // plain `usize`, not worth threading the whole config through the
        // worker for.
        let bt_channels = cfg.engine.net.bt_channels;
        let (rx_ctx, feeds) = crate::engine::start::feeds(&state, geometry);

        let app = Self::assemble(
            cfg,
            config_path,
            Arc::clone(&state),
            Some(Arc::clone(&device)),
            Some(Arc::clone(&rx_ctx)),
            None,
        )?;

        crate::engine::start::run(&state, &device, &rx_ctx, feeds, bt_channels);

        Ok(app)
    }

    pub(super) fn new_observer(
        cfg: SdrtopConfig,
        config_path: Option<PathBuf>,
        sysinfo: hardware::usb::UsbDeviceInfo,
        profile: hardware::discovery::ObserverProfile,
    ) -> anyhow::Result<Self> {
        let state = Arc::new(Mutex::new(initial_metrics(
            &cfg.engine,
            Boot::observer(&cfg.engine, &sysinfo, profile).showing(cfg.tui.display.shown()),
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
        crate::engine::start::observe(&state, profile);
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
        cfg: SdrtopConfig,
        config_path: Option<PathBuf>,
        state: Arc<Mutex<SdrMetrics>>,
        device: Option<Arc<dyn hardware::SdrDevice>>,
        rx_ctx: Option<Arc<hardware::RxContext>>,
        preset_override: Option<&str>,
    ) -> anyhow::Result<Self> {
        let themes_dir = config_path
            .as_deref()
            .and_then(crate::config::TuiConfig::themes_dir);
        let presets_dir = config_path
            .as_deref()
            .and_then(crate::config::LayoutConfig::presets_dir);
        let theme = cfg.tui.build_theme(themes_dir.as_deref());
        // Whether a config was saved before this run: what makes a view
        // worth offering to resume. Read before anything writes one, which
        // only quitting does.
        let saved = config_path.as_deref().is_some_and(std::path::Path::exists);

        // Whether the NET section exists at all, decided once, from the radio's
        // own declaration. The reason travels with the decision so the log and
        // the menu cannot end up disagreeing about why a section is missing.
        let (net, wifi) = {
            let m = state.lock().unwrap_or_else(|e| e.into_inner());
            (
                crate::signal::net::gate::verdict(&m.caps),
                crate::signal::net::gate::wifi_verdict(&m.caps),
            )
        };

        let active = preset_override.unwrap_or(&cfg.tui.display.active_preset);
        let acquisition = state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .caps
            .acquisition;
        let (engine, focus_keys) = Self::build_ui_for(
            active,
            &cfg.tui.presets,
            presets_dir.as_deref(),
            registry::NetAdmitted {
                band: net.clone(),
                wifi: wifi.clone(),
            },
            acquisition,
        )?;

        // A user preset that wanted a number key already taken says so, once,
        // here. `menu::model::build` collects these instead of logging them so it
        // can stay a pure function; this is the one place that has both the
        // warnings and the lock.
        // The screen's own state: the menu in front of the first screen, and
        // the recall slots, which retune the radio and so stay empty when
        // another process holds it.
        let mut ui = crate::ui::UiState::default();
        {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            for warning in engine.menu_warnings() {
                m.push_log(warning.clone());
            }
            for warning in engine.startup_warnings() {
                m.push_log(warning.clone());
            }
            if let Err(why) = &net {
                m.push_log(format!("Bluetooth and Wi-Fi refused: {why}"));
            } else if let Err(why) = &wifi {
                // Only when the band itself was admitted: otherwise the
                // band's sentence already says why, and Wi-Fi's would repeat it.
                m.push_log(format!("Wi-Fi refused: {why}"));
            }

            ui.menu = startup_menu(&engine, m.observer.active, saved);
            if !m.observer.active {
                ui.recall = crate::ui::recall_from_hz(cfg.engine.radio.recall_hz);
            }
        }

        let core = device
            .as_ref()
            .map(|device| crate::engine::Engine::new(Arc::clone(&state), Arc::clone(device)));
        Ok(Self {
            screen: crate::ui::Screen::new(Arc::clone(&state), ui),
            state,
            device,
            core,
            rx_ctx,
            config_path,
            events: EventStream::new(Duration::from_millis(33)),
            show_footer: true,
            unsaved: None,
            deck_shown: false,
            engine,
            theme,
            focus_keys,
            theme_config: cfg.tui.theme.clone(),
            tinysa_config: cfg.engine.tinysa.clone(),
            airspy_config: cfg.engine.airspy.clone(),
            airspyhf_config: cfg.engine.airspyhf.clone(),
            sdrplay_config: cfg.engine.sdrplay.clone(),
            plutosky_config: cfg.engine.plutosky.clone(),
            boost_config: cfg.engine.radio.boost.clone(),
            net_config: cfg.engine.net.clone(),
            record_config: cfg.engine.record.clone(),
            recorder: None,
            user_presets: cfg.tui.presets,
        })
    }
}

/// The menu in front of the first screen, if any.
///
/// The menu is the first screen. When a saved config named a view the menu
/// lists, it opens on the Resume tile, with the Lab tree already standing on
/// that view: `Enter` resumes, `↓` goes in there. On a first run (no config
/// file yet) there is nothing to resume, no Resume tile, and it opens on Lab.
///
/// **Not in observer mode.** Its layout is given, not chosen, so the menu does
/// not list it, and resuming from the top would replace the one screen that
/// names the radio's holder with a layout waiting for a stream that never
/// comes. `Esc` still opens the menu, and closes it again onto this screen.
fn startup_menu(
    engine: &crate::ui::LayoutEngine,
    observing: bool,
    saved: bool,
) -> Option<crate::ui::MenuState> {
    if observing {
        return None;
    }
    Some(crate::ui::menu::opening(engine, saved))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine(active: &str) -> crate::ui::LayoutEngine {
        let mut cfg = crate::config::LayoutConfig::default_config();
        cfg.active_preset = active.to_string();
        crate::ui::LayoutEngine::new(cfg, crate::ui::PanelRegistry::new())
    }

    /// **Observer mode starts on its own screen, not behind the menu.** The
    /// menu does not list the observer layout, so its cursor sat on the first
    /// entry, and the `Enter` that resumes everywhere else replaced the one
    /// screen that names the radio's holder with a layout waiting for a
    /// stream, with nothing in the menu leading back.
    #[test]
    fn observer_mode_starts_on_its_own_screen() {
        let observer = engine("observer");
        assert_eq!(observer.active_preset(), "observer");
        assert!(observer.menu().locate("observer").is_none());
        assert!(startup_menu(&observer, true, true).is_none());

        // Everywhere else the menu still comes first, on the restored layout.
        let spectrum = engine("spectrum");
        let menu = startup_menu(&spectrum, false, true).expect("the menu comes first");
        assert_eq!(menu.tile, crate::ui::MenuTile::Resume);
        let (_, path) = spectrum.menu_tree().locate("spectrum").unwrap();
        assert_eq!(menu.lab, (&path).into());
    }

    /// A first run has nothing to resume: no Resume tile, and Lab selected.
    #[test]
    fn a_first_run_opens_on_lab_without_resume() {
        let spectrum = engine("spectrum");
        let menu = startup_menu(&spectrum, false, false).expect("the menu comes first");
        assert!(!menu.resume);
        assert_eq!(menu.tile, crate::ui::MenuTile::Lab);
    }

    /// What the header draws for a radio another process holds: its own
    /// backend's label, never the HackRF's, and never a library sdrtop
    /// cannot see the holder using.
    #[test]
    fn an_observed_radio_is_introduced_in_the_header_as_its_own_backend() {
        let cfg = crate::config::EngineConfig::default();
        let sysinfo = hardware::usb::UsbDeviceInfo {
            product: "RTL2838UHIDIR".into(),
            manufacturer: "Realtek".into(),
            serial: Some("00000001".into()),
            speed_mbits: Some(480.0),
            max_power: Some("500mA".into()),
            bus: Some(1),
            dev: Some(5),
            connected_secs: None,
            holder: None,
        };
        let header = |kind: hardware::DeviceKind| {
            let profile = kind
                .observer_profile()
                .expect("this backend can be observed");
            let observed = initial_metrics(&cfg, Boot::observer(&cfg, &sysinfo, profile)).unwrap();
            crate::ui::fixture::draw(
                crate::ui::panels::core::header::HeaderPanel,
                120,
                8,
                &observed,
            )
            .join("\n")
        };
        let rtl = header(hardware::DeviceKind::RtlSdr);
        assert!(rtl.contains("rtl-sdr"), "{rtl}");
        assert!(!rtl.contains("hackrf fw"), "not a HackRF:\n{rtl}");
        assert!(
            !rtl.contains("librtlsdr"),
            "the holder's library is not seen:\n{rtl}"
        );
        let hackrf = header(hardware::DeviceKind::HackRf);
        assert!(hackrf.contains("hackrf fw"), "{hackrf}");
    }
}
