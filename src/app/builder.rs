use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::config::{AppConfig, LayoutConfig};
use crate::event::EventStream;
use crate::hardware;
use crate::signal::{DemodWorker, FftWorker};
use crate::state::{
    Accumulators, IqState, ObserverState, RadioState, SdrMetrics, SignalState, SpectrumState,
    SweepConfig, SweepState, SystemState, TimingState, UiState, WaterfallState,
    THROUGHPUT_HISTORY_LEN,
};
use crate::tasks;
use crate::ui;

use super::App;

impl App {
    fn build_ui(
        active_preset: &str,
        user_presets: &HashMap<String, crate::config::PresetConfig>,
        presets_dir: Option<&std::path::Path>,
    ) -> (ui::LayoutEngine, HashMap<char, &'static str>) {
        let mut registry = ui::PanelRegistry::new();
        registry.register(ui::HeaderPanel);
        registry.register(ui::SlimHeaderPanel);
        registry.register(ui::CommandRailPanel);
        registry.register(ui::LabBannerPanel);
        registry.register(ui::LabMarkerPanel);
        registry.register(ui::SignalStripPanel);
        registry.register(ui::LogPanel);
        registry.register(ui::FooterPanel);
        registry.register(ui::IqConstellationPanel);
        registry.register(ui::IqDiagnosticsPanel);
        registry.register(ui::ImageScopePanel);
        registry.register(ui::SystemResourcesPanel);
        registry.register(ui::SpectrumPanel);
        registry.register(ui::WaterfallPanel::new());
        registry.register(ui::RfChainPanel);
        registry.register(ui::LevelDiagramPanel);
        registry.register(ui::AdcLoadingPanel);
        registry.register(ui::SignalMetricsPanel);
        registry.register(ui::SignalCharacterizationPanel);
        registry.register(ui::FmDemodPanel);
        registry.register(ui::IqHistogramPanel);
        registry.register(ui::ObserverPanel);
        registry.register(ui::MicroPanel);
        registry.register(ui::MicroSignalPanel);
        registry.register(ui::MicroGainPanel);
        registry.register(ui::MicroHealthPanel);
        registry.register(ui::TimingDiagnosticsPanel);
        registry.register(ui::TimingStripchartPanel);
        registry.register(ui::TimingVitalsPanel);
        registry.register(ui::SweepPanel);
        registry.register(ui::SweepStripPanel);
        registry.register(ui::MicroSweepPanel);

        let (focus_keys, collisions) = harvest_focus_keys(&registry);
        // A key claimed twice does not merely shadow. The registry is a HashMap, so
        // iteration order is randomised per process and the winner changes between
        // launches: the key then works on some runs and silently does nothing on
        // others. Loud in debug, and pinned by a test, so it can never ship quietly.
        debug_assert!(
            collisions.is_empty(),
            "focus key claimed by more than one panel: {collisions:?}",
        );

        let mut engine = ui::LayoutEngine::new(
            LayoutConfig::with_user_presets(user_presets, presets_dir),
            registry,
        );
        engine.set_preset(active_preset);
        (engine, focus_keys)
    }

    pub(super) fn new_normal(
        cfg: AppConfig,
        config_path: Option<PathBuf>,
        device: Arc<dyn hardware::SdrDevice>,
    ) -> anyhow::Result<Self> {
        let info = device.info();
        let board_name = info.board_name.clone();
        let serial = info.serial.clone();
        let fw_version = info
            .fw_version
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        let board_rev = info.board_rev.unwrap_or(0xFE);
        let usb_api_ver = info.usb_api_version.unwrap_or(0);
        let themes_dir = config_path
            .as_deref()
            .and_then(crate::config::AppConfig::themes_dir);
        let presets_dir = config_path
            .as_deref()
            .and_then(crate::config::LayoutConfig::presets_dir);
        let caps = Arc::new(device.capabilities().clone());
        let sample_format = caps.sample_format;
        let theme = cfg.build_theme(themes_dir.as_deref());

        // Clamp the stored config into THIS device's legal range, falling back to
        // its default when out of range — so a config saved on one device (e.g. a
        // HackRF at 2.4 GHz / 10 Msps) boots an RTL-SDR at a legal freq/rate
        // instead of failing, without discarding the original device's settings.
        let freq = if (caps.freq_min_hz..=caps.freq_max_hz).contains(&cfg.radio.frequency_hz) {
            cfg.radio.frequency_hz
        } else {
            caps.default_frequency_hz
        };
        let sr = if (caps.sample_rate_min_hz..=caps.sample_rate_max_hz)
            .contains(&cfg.radio.sample_rate)
        {
            cfg.radio.sample_rate
        } else {
            caps.default_sample_rate_hz
        };
        // Snap the stored gains into THIS device's gain model too, so a config from
        // another device family neither programs an illegal gain nor displays one
        // (e.g. an RTL tuner's 49 dB on a HackRF LNA). One clamp feeds both the
        // hardware set and the state below, so they always agree.
        let (lna_gain, vga_gain) = caps
            .gain
            .clamp_gains(cfg.radio.lna_gain, cfg.radio.vga_gain);

        let (sr_result, bb_filter_hz) = match device.set_sample_rate(sr) {
            Ok(bw) => (Ok(()), bw),
            Err(e) => (Err(e), hardware::compute_bb_filter_bw(sr)),
        };
        // `amp_enabled` is the front-end-boost state for both device families:
        // HackRF's RF amp (set_amp_enable) and RTL-SDR's tuner AGC (set_tuner_agc).
        // Calling both applies the right one per device (the other is a no-op) so
        // the programmed state matches what the UI shows.
        let startup_results = [
            device.set_frequency(freq),
            sr_result,
            device.set_lna_gain(lna_gain),
            device.set_vga_gain(vga_gain),
            device.set_amp_enable(cfg.radio.amp_enabled),
            device.set_tuner_agc(cfg.radio.amp_enabled),
        ];

        let state = Arc::new(Mutex::new(SdrMetrics {
            radio: RadioState {
                frequency: freq,
                config_sample_rate: sr,
                actual_sample_rate: 0,
                bb_filter_hz,
                lna_gain,
                vga_gain,
                amp_enabled: cfg.radio.amp_enabled,
                rx_enabled: false,
                hw_streaming: false,
                rx_start_time: None,
                bytes_since_last_poll: 0,
                last_poll_time: Instant::now(),
                current_throughput_bps: 0,
                throughput_history: std::collections::VecDeque::with_capacity(
                    THROUGHPUT_HISTORY_LEN,
                ),
                sample_rate_history: std::collections::VecDeque::with_capacity(
                    THROUGHPUT_HISTORY_LEN,
                ),
            },
            signal: SignalState {
                drops_per_sec: 0,
                total_drops_session: 0,
                drop_history: std::collections::VecDeque::with_capacity(THROUGHPUT_HISTORY_LEN),
                saturation_history: std::collections::VecDeque::with_capacity(
                    THROUGHPUT_HISTORY_LEN,
                ),
                usb_error_history: std::collections::VecDeque::with_capacity(
                    THROUGHPUT_HISTORY_LEN,
                ),
                // Everything else is the no-measurement state, which `SignalState`
                // defines once rather than here twice.
                ..Default::default()
            },
            iq: IqState {
                iq_imbalance_db: 0.0,
                dc_offset_i: 0.0,
                dc_offset_q: 0.0,
                cb_period_us: 0,
                cb_jitter_us: 0,
                jitter_history: std::collections::VecDeque::with_capacity(THROUGHPUT_HISTORY_LEN),
                iq_amplitude_hist: [0u64; 32],
                adc_signed_hist: [0u64; 32],
                buf_fill_pct: 0.0,
                buf_fill_history: std::collections::VecDeque::with_capacity(THROUGHPUT_HISTORY_LEN),
                phase_imbalance_deg: 0.0,
                cal: crate::state::IqCalState::default(),
                irr_history: std::collections::VecDeque::with_capacity(
                    crate::state::SNR_HISTORY_LEN,
                ),
                constellation: std::collections::VecDeque::new(),
            },
            observer: ObserverState::default(),
            spectrum: SpectrumState {
                step_hz: 100_000,
                y_min: -120.0,
                y_max: 0.0,
                hold: None,
                cursor_freq: None,
                markers: cfg.display.spectrum_markers.clone(),
                pending_marker: None,
                style: cfg.display.spectrum_style,
            },
            waterfall: WaterfallState::new(
                cfg.display.waterfall_max_rows,
                cfg.display.waterfall_palette,
            ),
            system: SystemState {
                board_name: Arc::from(board_name.as_str()),
                serial: Arc::from(serial.as_str()),
                fw_version: Arc::from(fw_version.as_str()),
                board_rev,
                usb_api_version: usb_api_ver,
                process_cpu_pct: 0.0,
                process_rss_mb: 0,
                cpu_history: std::collections::VecDeque::with_capacity(
                    crate::state::THROUGHPUT_HISTORY_LEN,
                ),
            },
            timing: TimingState::default(),
            sweep: SweepState {
                config: SweepConfig {
                    start_hz: cfg.sweep.start_hz,
                    stop_hz: cfg.sweep.stop_hz,
                    step_hz: 0,
                    dwell_ms: cfg.sweep.dwell_ms,
                },
                ..SweepState::default()
            },
            ui: UiState {
                recall: crate::state::recall_from_hz(cfg.radio.recall_hz),
                ..UiState::default()
            },
            lab: crate::state::LabState::default(),
            demod: crate::state::DemodState::default(),
            caps: Arc::clone(&caps),
            acc: Accumulators::default(),
        }));

        {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            m.push_log(format!("Connected: {} | Serial: {}", board_name, serial));
            // Firmware is a HackRF concept; RTL-SDR (no on-device FW) skips it.
            if let Some(fw) = &info.fw_version {
                m.push_log(format!("Firmware: {}", fw));
            }
            // RTL-SDR reports a tuner instead of a board revision / USB-API version.
            if let Some(tuner) = &info.tuner_name {
                m.push_log(format!("Tuner: {}", tuner));
            } else {
                m.push_log(format!(
                    "Board: {} | USB API: {:#06x}",
                    hardware::board_rev_name(board_rev),
                    usb_api_ver
                ));
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
            format: sample_format,
        });

        let fft_state = Arc::clone(&state);
        std::thread::spawn(move || FftWorker::new(sample_rx, fft_state, sample_format).run());

        let demod_state = Arc::clone(&state);
        std::thread::spawn(move || DemodWorker::new(demod_rx, demod_state, sample_format).run());

        tasks::spawn_rx_task(Arc::clone(&state), Arc::clone(&device), Arc::clone(&rx_ctx));
        tasks::spawn_sweep_task(Arc::clone(&state), Arc::clone(&device));
        tasks::spawn_sys_resource_task(Arc::clone(&state));

        let (engine, focus_keys) = Self::build_ui(
            &cfg.display.active_preset,
            &cfg.presets,
            presets_dir.as_deref(),
        );

        Ok(Self {
            state,
            device: Some(device),
            rx_ctx: Some(rx_ctx),
            config_path,
            events: EventStream::new(Duration::from_millis(33)),
            show_help: false,
            show_footer: true,
            engine,
            theme,
            focus_keys,
            theme_config: cfg.theme.clone(),
            user_presets: cfg.presets,
        })
    }

    pub(super) fn new_observer(
        cfg: AppConfig,
        config_path: Option<PathBuf>,
        sysinfo: hardware::sysfs::HackRfSysInfo,
        kind: hardware::DeviceKind,
    ) -> anyhow::Result<Self> {
        let themes_dir = config_path
            .as_deref()
            .and_then(crate::config::AppConfig::themes_dir);
        let presets_dir = config_path
            .as_deref()
            .and_then(crate::config::LayoutConfig::presets_dir);
        let board_name = sysinfo.product.clone();
        let serial = sysinfo.serial.clone();
        let theme = cfg.build_theme(themes_dir.as_deref());

        let state = Arc::new(Mutex::new(SdrMetrics {
            radio: RadioState {
                frequency: cfg.radio.frequency_hz,
                config_sample_rate: cfg.radio.sample_rate,
                actual_sample_rate: 0,
                bb_filter_hz: hardware::compute_bb_filter_bw(cfg.radio.sample_rate),
                lna_gain: cfg.radio.lna_gain,
                vga_gain: cfg.radio.vga_gain,
                amp_enabled: cfg.radio.amp_enabled,
                rx_enabled: false,
                hw_streaming: false,
                rx_start_time: None,
                bytes_since_last_poll: 0,
                last_poll_time: Instant::now(),
                current_throughput_bps: 0,
                throughput_history: std::collections::VecDeque::with_capacity(
                    THROUGHPUT_HISTORY_LEN,
                ),
                sample_rate_history: std::collections::VecDeque::with_capacity(
                    THROUGHPUT_HISTORY_LEN,
                ),
            },
            signal: SignalState {
                drops_per_sec: 0,
                total_drops_session: 0,
                drop_history: std::collections::VecDeque::with_capacity(THROUGHPUT_HISTORY_LEN),
                saturation_history: std::collections::VecDeque::with_capacity(
                    THROUGHPUT_HISTORY_LEN,
                ),
                usb_error_history: std::collections::VecDeque::with_capacity(
                    THROUGHPUT_HISTORY_LEN,
                ),
                // Everything else is the no-measurement state, which `SignalState`
                // defines once rather than here twice.
                ..Default::default()
            },
            iq: IqState {
                iq_imbalance_db: 0.0,
                dc_offset_i: 0.0,
                dc_offset_q: 0.0,
                cb_period_us: 0,
                cb_jitter_us: 0,
                jitter_history: std::collections::VecDeque::with_capacity(THROUGHPUT_HISTORY_LEN),
                iq_amplitude_hist: [0u64; 32],
                adc_signed_hist: [0u64; 32],
                buf_fill_pct: 0.0,
                buf_fill_history: std::collections::VecDeque::with_capacity(THROUGHPUT_HISTORY_LEN),
                phase_imbalance_deg: 0.0,
                cal: crate::state::IqCalState::default(),
                irr_history: std::collections::VecDeque::with_capacity(
                    crate::state::SNR_HISTORY_LEN,
                ),
                constellation: std::collections::VecDeque::new(),
            },
            observer: ObserverState {
                active: true,
                device: Some(format!("{} · {}", sysinfo.product, sysinfo.manufacturer)),
                serial: Some(sysinfo.serial.clone()),
                usb: Some(format!(
                    "High Speed ({} Mbit/s) · {} · Bus {}, Dev {}",
                    sysinfo.speed_mbits, sysinfo.max_power, sysinfo.bus, sysinfo.dev
                )),
                connected: sysinfo.connected_secs.map(tasks::fmt_duration),
                ..Default::default()
            },
            spectrum: SpectrumState {
                step_hz: 100_000,
                y_min: -120.0,
                y_max: 0.0,
                hold: None,
                cursor_freq: None,
                markers: vec![],
                pending_marker: None,
                style: cfg.display.spectrum_style,
            },
            waterfall: WaterfallState::new(
                cfg.display.waterfall_max_rows,
                cfg.display.waterfall_palette,
            ),
            system: SystemState {
                board_name: Arc::from(board_name.as_str()),
                serial: Arc::from(serial.as_str()),
                fw_version: Arc::from("Observer Mode"),
                board_rev: 0xFE,
                usb_api_version: 0,
                process_cpu_pct: 0.0,
                process_rss_mb: 0,
                cpu_history: std::collections::VecDeque::with_capacity(
                    crate::state::THROUGHPUT_HISTORY_LEN,
                ),
            },
            timing: TimingState::default(),
            sweep: SweepState {
                config: SweepConfig {
                    start_hz: cfg.sweep.start_hz,
                    stop_hz: cfg.sweep.stop_hz,
                    step_hz: 0,
                    dwell_ms: cfg.sweep.dwell_ms,
                },
                ..SweepState::default()
            },
            ui: UiState::default(),
            lab: crate::state::LabState::default(),
            demod: crate::state::DemodState::default(),
            // Observer mode has no open device to query; use the matching
            // backend's capability profile so the UI labels stay correct.
            caps: Arc::new(match kind {
                hardware::DeviceKind::HackRf => hardware::hackrf::caps(),
                hardware::DeviceKind::RtlSdr => hardware::rtlsdr::observer_caps(),
            }),
            acc: Accumulators::default(),
        }));

        {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            m.push_log(format!(
                "Observer Mode: {} (Serial: {})",
                board_name, serial
            ));
            m.push_log("Device is in use by another process — hardware controls disabled");
        }

        tasks::spawn_observer_task(Arc::clone(&state), sysinfo.bus, sysinfo.dev, kind);
        tasks::spawn_sys_resource_task(Arc::clone(&state));

        let (engine, focus_keys) = Self::build_ui("observer", &cfg.presets, presets_dir.as_deref());

        Ok(Self {
            state,
            device: None,
            rx_ctx: None,
            config_path,
            events: EventStream::new(Duration::from_millis(33)),
            show_help: false,
            show_footer: true,
            engine,
            theme,
            focus_keys,
            theme_config: cfg.theme.clone(),
            user_presets: cfg.presets,
        })
    }
}

/// The focus-key lookup, plus every key more than one panel claims.
type FocusHarvest = (HashMap<char, &'static str>, Vec<(char, Vec<&'static str>)>);

/// Collect each panel's focus key into the lookup the key handler uses, and report
/// any key more than one panel claims.
///
/// Split out so the collision is *visible*: `HashMap::insert` would silently drop
/// one of the two, which is exactly how `v` and `t` came to work only on some
/// launches. See the tests at the foot of this file.
fn harvest_focus_keys(registry: &ui::PanelRegistry) -> FocusHarvest {
    let mut claims: HashMap<char, Vec<&'static str>> = HashMap::new();
    for panel in registry.panels_iter() {
        if let Some(key) = panel.focus_key() {
            claims.entry(key).or_default().push(panel.name());
        }
    }
    let mut collisions: Vec<(char, Vec<&'static str>)> = claims
        .iter()
        .filter(|(_, v)| v.len() > 1)
        .map(|(k, v)| {
            let mut v = v.clone();
            v.sort();
            (*k, v)
        })
        .collect();
    collisions.sort();
    let keys = claims
        .into_iter()
        .map(|(k, mut v)| {
            v.sort();
            (k, v[0])
        })
        .collect();
    (keys, collisions)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every panel that can be focused must have a handler arm in `handle_normal`.
    ///
    /// The bug this guards is not hypothetical. The `lab_timing` rebuild replaced
    /// `hardware_health` and `timing_panel` with `timing_vitals` and
    /// `timing_diagnostics`, but the dispatch kept naming the old two. Focusing
    /// either new panel then highlighted its border, printed `[R] Reset drop
    /// counter · [C] Clear history` in the footer, and dropped every key through
    /// to `handle_global` — where `[R]` resets the whole radio to defaults.
    ///
    /// A panel that offers keys and silently ignores them is worse than one with
    /// no focus mode at all, so this is checked rather than remembered.
    #[test]
    fn every_focusable_panel_has_a_dispatch_arm() {
        let (_engine, focus_keys) = App::build_ui("command_rail", &HashMap::new(), None);
        // The dispatch table is read as source text: the arms are `&str` matches on
        // a panel name, so nothing in the type system ties them to the registry.
        let dispatch = include_str!("input/mod.rs");
        assert!(
            !focus_keys.is_empty(),
            "no focus keys were harvested at all"
        );
        for (key, panel) in &focus_keys {
            let arm = format!("Some(\"{panel}\")");
            assert!(
                dispatch.contains(&arm),
                "panel '{panel}' claims focus key '{key}' but handle_normal has no \
                 `{arm}` arm, so its keys fall through to the global handler",
            );
        }
    }

    /// Every panel a built-in preset names must exist in the registry.
    ///
    /// The presets are TOML and the registry is Rust, so nothing in the type
    /// system connects them: a panel renamed or removed leaves a preset quietly
    /// asking for a name that resolves to nothing, and the layout engine just
    /// draws a gap. Cheap to check, invisible otherwise.
    #[test]
    fn every_panel_named_by_a_builtin_preset_is_registered() {
        let (engine, _) = App::build_ui("command_rail", &HashMap::new(), None);
        let known: std::collections::HashSet<&str> = engine.registered_panel_names().collect();
        assert!(!known.is_empty(), "no panels were registered at all");

        let cfg = crate::config::LayoutConfig::default_config();
        let mut missing: Vec<String> = Vec::new();
        for (preset, spec) in &cfg.presets {
            for panel in &spec.panels {
                if !known.contains(panel.name.as_str()) {
                    missing.push(format!("{preset} -> {}", panel.name));
                }
            }
        }
        missing.sort();
        assert!(
            missing.is_empty(),
            "presets name panels that are not registered: {missing:?}"
        );
    }

    /// Two panels claiming one key must be *reported*, not silently resolved.
    ///
    /// `HashMap::insert` would drop one of them, which is exactly how `v` and `t`
    /// came to work only on some launches: the registry is a HashMap, iteration
    /// order is randomised per process, and the winner changed between runs. Two
    /// stand-in panels here prove the detector fires, so the assertion on the real
    /// registry below means something.
    #[test]
    fn a_duplicate_focus_key_is_reported() {
        struct First;
        struct Second;
        impl ui::panel::Panel for First {
            fn name(&self) -> &'static str {
                "first"
            }
            fn min_size(&self) -> (u16, u16) {
                (1, 1)
            }
            fn focus_key(&self) -> Option<char> {
                Some('z')
            }
            fn render(
                &self,
                _: &mut ratatui::Frame,
                _: ratatui::layout::Rect,
                _: &crate::state::SdrMetrics,
                _: &crate::Theme,
                _: bool,
            ) {
            }
        }
        impl ui::panel::Panel for Second {
            fn name(&self) -> &'static str {
                "second"
            }
            fn min_size(&self) -> (u16, u16) {
                (1, 1)
            }
            fn focus_key(&self) -> Option<char> {
                Some('z')
            }
            fn render(
                &self,
                _: &mut ratatui::Frame,
                _: ratatui::layout::Rect,
                _: &crate::state::SdrMetrics,
                _: &crate::Theme,
                _: bool,
            ) {
            }
        }
        let mut registry = ui::PanelRegistry::new();
        registry.register(First);
        registry.register(Second);
        let (_, collisions) = harvest_focus_keys(&registry);
        assert_eq!(collisions, vec![('z', vec!["first", "second"])]);
    }

    /// The full registry, which is the one that actually ships.
    #[test]
    fn the_real_registry_has_no_focus_key_collisions() {
        // `build_ui` debug-asserts this too; the test states it as a fact rather
        // than relying on someone running a debug build.
        let (_engine, keys) = App::build_ui("command_rail", &HashMap::new(), None);
        let mut by_key: HashMap<char, usize> = HashMap::new();
        for k in keys.keys() {
            *by_key.entry(*k).or_default() += 1;
        }
        assert!(by_key.values().all(|&n| n == 1));
        assert!(
            keys.len() >= 10,
            "expected the full focus set, got {}",
            keys.len()
        );
    }
}
