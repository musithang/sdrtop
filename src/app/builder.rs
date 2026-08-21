use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::config::{AppConfig, LayoutConfig};
use crate::event::EventStream;
use crate::hardware;
use crate::signal::{DemodWorker, FftWorker};
use crate::state::{
    Accumulators, IqState, ObserverState, RadioState, SdrMetrics,
    SignalState, SpectrumState, SweepConfig, SweepState, SystemState, TimingState, UiState,
    WaterfallState, THROUGHPUT_HISTORY_LEN,
};
use crate::tasks;
use crate::ui;

use super::App;

impl App {
    fn build_ui(
        board_name: &str,
        serial: &str,
        active_preset: &str,
        user_presets: &HashMap<String, crate::config::PresetConfig>,
    ) -> (ui::LayoutEngine, HashMap<char, &'static str>) {
        let mut registry = ui::PanelRegistry::new();
        registry.register(ui::HeaderPanel);
        registry.register(ui::SlimHeaderPanel);
        registry.register(ui::CommandRailPanel);
        registry.register(ui::LabBannerPanel);
        registry.register(ui::LabMarkerPanel);
        registry.register(ui::TelemetryPanel {
            board_name: board_name.to_string(),
            serial: serial.to_string(),
        });
        registry.register(ui::GainsPanel);
        registry.register(ui::ThroughputPanel);
        registry.register(ui::SampleRatePanel);
        registry.register(ui::SignalStripPanel);
        registry.register(ui::UsbSrPanel);
        registry.register(ui::LogPanel);
        registry.register(ui::FooterPanel);
        registry.register(ui::HardwareHealthPanel);
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
        registry.register(ui::TimingPanel);
        registry.register(ui::TimingDiagnosticsPanel);
        registry.register(ui::TimingStripchartPanel);
        registry.register(ui::TimingVitalsPanel);
        registry.register(ui::SweepPanel);
        registry.register(ui::SweepStripPanel);
        registry.register(ui::MicroSweepPanel);

        let mut focus_keys: HashMap<char, &'static str> = HashMap::new();
        for panel in registry.panels_iter() {
            if let Some(key) = panel.focus_key() {
                focus_keys.insert(key, panel.name());
            }
        }

        let mut engine = ui::LayoutEngine::new(LayoutConfig::with_user_presets(user_presets), registry);
        engine.set_preset(active_preset);
        (engine, focus_keys)
    }

    pub(super) fn new_normal(
        cfg: AppConfig,
        config_path: Option<PathBuf>,
        device: Arc<dyn hardware::SdrDevice>,
    ) -> anyhow::Result<Self> {
        let info         = device.info();
        let board_name   = info.board_name.clone();
        let serial       = info.serial.clone();
        let fw_version   = info.fw_version.clone().unwrap_or_else(|| "unknown".to_string());
        let board_rev    = info.board_rev.unwrap_or(0xFE);
        let usb_api_ver  = info.usb_api_version.unwrap_or(0);
        let caps          = Arc::new(device.capabilities().clone());
        let sample_format = caps.sample_format;
        let theme         = cfg.build_theme();

        // Clamp the stored config into THIS device's legal range, falling back to
        // its default when out of range — so a config saved on one device (e.g. a
        // HackRF at 2.4 GHz / 10 Msps) boots an RTL-SDR at a legal freq/rate
        // instead of failing, without discarding the original device's settings.
        let freq = if (caps.freq_min_hz..=caps.freq_max_hz).contains(&cfg.radio.frequency_hz) {
            cfg.radio.frequency_hz
        } else {
            caps.default_frequency_hz
        };
        let sr = if (caps.sample_rate_min_hz..=caps.sample_rate_max_hz).contains(&cfg.radio.sample_rate) {
            cfg.radio.sample_rate
        } else {
            caps.default_sample_rate_hz
        };
        // Snap the stored gains into THIS device's gain model too, so a config from
        // another device family neither programs an illegal gain nor displays one
        // (e.g. an RTL tuner's 49 dB on a HackRF LNA). One clamp feeds both the
        // hardware set and the state below, so they always agree.
        let (lna_gain, vga_gain) = caps.gain.clamp_gains(cfg.radio.lna_gain, cfg.radio.vga_gain);

        let (sr_result, bb_filter_hz) = match device.set_sample_rate(sr) {
            Ok(bw)  => (Ok(()), bw),
            Err(e)  => (Err(e), hardware::compute_bb_filter_bw(sr)),
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
                frequency:           freq,
                config_sample_rate:  sr,
                actual_sample_rate:  0,
                bb_filter_hz,
                lna_gain,
                vga_gain,
                amp_enabled:         cfg.radio.amp_enabled,
                rx_enabled:          false,
                hw_streaming:        false,
                rx_start_time:       None,
                bytes_since_last_poll: 0,
                last_poll_time:      Instant::now(),
                current_throughput_bps: 0,
                throughput_history:  std::collections::VecDeque::with_capacity(THROUGHPUT_HISTORY_LEN),
                sample_rate_history: std::collections::VecDeque::with_capacity(THROUGHPUT_HISTORY_LEN),
            },
            signal: SignalState {
                drops_per_sec: 0, total_drops_session: 0,
                drop_history: std::collections::VecDeque::with_capacity(THROUGHPUT_HISTORY_LEN),
                adc_saturation_pct: 0.0, adc_saturation_peak: 0.0,
                saturation_history: std::collections::VecDeque::with_capacity(THROUGHPUT_HISTORY_LEN),
                peak_to_nf_db: 0.0, channel_power_dbfs: f32::NEG_INFINITY,
                occupied_bw_hz: 0,
                acpr_lower_db: f32::NEG_INFINITY, acpr_upper_db: f32::NEG_INFINITY,
                adj_carrier_dbfs: f32::NEG_INFINITY,
                usb_errors_session: 0,
                usb_errors_last_poll: 0,
                usb_error_history: std::collections::VecDeque::with_capacity(THROUGHPUT_HISTORY_LEN),
                snr_history: std::collections::VecDeque::with_capacity(crate::state::SNR_HISTORY_LEN),
                pwr_history: std::collections::VecDeque::with_capacity(crate::state::SNR_HISTORY_LEN),
                nf_history:  std::collections::VecDeque::with_capacity(crate::state::SNR_HISTORY_LEN),
                sat_history: std::collections::VecDeque::with_capacity(crate::state::SNR_HISTORY_LEN),
                last_clip_at: None,
                modulation: crate::state::Modulation::Unknown,
                adc_peak_dbfs: 0.0, adc_rms_dbfs: 0.0, adc_clip_events: 0,
            },
            iq: IqState { iq_imbalance_db: 0.0, dc_offset_i: 0.0, dc_offset_q: 0.0, cb_period_us: 0, cb_jitter_us: 0, jitter_history: std::collections::VecDeque::with_capacity(THROUGHPUT_HISTORY_LEN), iq_amplitude_hist: [0u64; 32], adc_signed_hist: [0u64; 32], buf_fill_pct: 0.0, buf_fill_history: std::collections::VecDeque::with_capacity(THROUGHPUT_HISTORY_LEN), phase_imbalance_deg: 0.0, cal: crate::state::IqCalState::default(), irr_history: std::collections::VecDeque::with_capacity(crate::state::SNR_HISTORY_LEN), constellation: std::collections::VecDeque::new() },
            observer: ObserverState::default(),
            spectrum: SpectrumState {
                step_hz: 100_000, y_min: -120.0, y_max: 0.0,
                hold: None, cursor_freq: None,
                markers: cfg.display.spectrum_markers.clone(), pending_marker: None,
                style: cfg.display.spectrum_style,
            },
            waterfall: WaterfallState::new(cfg.display.waterfall_max_rows, cfg.display.waterfall_palette),
            system: SystemState {
                board_name: Arc::from(board_name.as_str()), serial: Arc::from(serial.as_str()),
                fw_version: Arc::from(fw_version.as_str()), board_rev,
                usb_api_version: usb_api_ver,
                process_cpu_pct: 0.0, process_rss_mb: 0,
                cpu_history: std::collections::VecDeque::with_capacity(crate::state::THROUGHPUT_HISTORY_LEN),
            },
            timing: TimingState::default(),
            sweep: SweepState {
                config: SweepConfig {
                    start_hz: cfg.sweep.start_hz,
                    stop_hz:  cfg.sweep.stop_hz,
                    step_hz:  0,
                    dwell_ms: cfg.sweep.dwell_ms,
                },
                ..SweepState::default()
            },
            ui:  UiState { recall: crate::state::recall_from_hz(cfg.radio.recall_hz), ..UiState::default() },
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
                m.push_log(format!("Board: {} | USB API: {:#06x}",
                    hardware::board_rev_name(board_rev), usb_api_ver));
            }
            let names = ["frequency", "sample rate", "LNA gain", "VGA gain", "amp", "tuner AGC"];
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
            metrics: Arc::clone(&state), sample_tx, demod_tx, format: sample_format,
        });

        let fft_state = Arc::clone(&state);
        std::thread::spawn(move || FftWorker::new(sample_rx, fft_state, sample_format).run());

        let demod_state = Arc::clone(&state);
        std::thread::spawn(move || DemodWorker::new(demod_rx, demod_state, sample_format).run());

        tasks::spawn_rx_task(Arc::clone(&state), Arc::clone(&device), Arc::clone(&rx_ctx));
        tasks::spawn_sweep_task(Arc::clone(&state), Arc::clone(&device));
        tasks::spawn_sys_resource_task(Arc::clone(&state));

        let (engine, focus_keys) = Self::build_ui(&board_name, &serial, &cfg.display.active_preset, &cfg.presets);

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
            user_presets: cfg.presets,
        })
    }

    pub(super) fn new_observer(
        cfg: AppConfig,
        config_path: Option<PathBuf>,
        sysinfo: hardware::sysfs::HackRfSysInfo,
        kind: hardware::DeviceKind,
    ) -> anyhow::Result<Self> {
        let board_name = sysinfo.product.clone();
        let serial     = sysinfo.serial.clone();
        let theme      = cfg.build_theme();

        let state = Arc::new(Mutex::new(SdrMetrics {
            radio: RadioState {
                frequency:           cfg.radio.frequency_hz,
                config_sample_rate:  cfg.radio.sample_rate,
                actual_sample_rate:  0,
                bb_filter_hz:        hardware::compute_bb_filter_bw(cfg.radio.sample_rate),
                lna_gain:            cfg.radio.lna_gain,
                vga_gain:            cfg.radio.vga_gain,
                amp_enabled:         cfg.radio.amp_enabled,
                rx_enabled:          false,
                hw_streaming:        false,
                rx_start_time:       None,
                bytes_since_last_poll: 0,
                last_poll_time:      Instant::now(),
                current_throughput_bps: 0,
                throughput_history:  std::collections::VecDeque::with_capacity(THROUGHPUT_HISTORY_LEN),
                sample_rate_history: std::collections::VecDeque::with_capacity(THROUGHPUT_HISTORY_LEN),
            },
            signal: SignalState {
                drops_per_sec: 0, total_drops_session: 0,
                drop_history: std::collections::VecDeque::with_capacity(THROUGHPUT_HISTORY_LEN),
                adc_saturation_pct: 0.0, adc_saturation_peak: 0.0,
                saturation_history: std::collections::VecDeque::with_capacity(THROUGHPUT_HISTORY_LEN),
                peak_to_nf_db: 0.0, channel_power_dbfs: f32::NEG_INFINITY,
                occupied_bw_hz: 0,
                acpr_lower_db: f32::NEG_INFINITY, acpr_upper_db: f32::NEG_INFINITY,
                adj_carrier_dbfs: f32::NEG_INFINITY,
                usb_errors_session: 0,
                usb_errors_last_poll: 0,
                usb_error_history: std::collections::VecDeque::with_capacity(THROUGHPUT_HISTORY_LEN),
                snr_history: std::collections::VecDeque::with_capacity(crate::state::SNR_HISTORY_LEN),
                pwr_history: std::collections::VecDeque::with_capacity(crate::state::SNR_HISTORY_LEN),
                nf_history:  std::collections::VecDeque::with_capacity(crate::state::SNR_HISTORY_LEN),
                sat_history: std::collections::VecDeque::with_capacity(crate::state::SNR_HISTORY_LEN),
                last_clip_at: None,
                modulation: crate::state::Modulation::Unknown,
                adc_peak_dbfs: 0.0, adc_rms_dbfs: 0.0, adc_clip_events: 0,
            },
            iq: IqState { iq_imbalance_db: 0.0, dc_offset_i: 0.0, dc_offset_q: 0.0, cb_period_us: 0, cb_jitter_us: 0, jitter_history: std::collections::VecDeque::with_capacity(THROUGHPUT_HISTORY_LEN), iq_amplitude_hist: [0u64; 32], adc_signed_hist: [0u64; 32], buf_fill_pct: 0.0, buf_fill_history: std::collections::VecDeque::with_capacity(THROUGHPUT_HISTORY_LEN), phase_imbalance_deg: 0.0, cal: crate::state::IqCalState::default(), irr_history: std::collections::VecDeque::with_capacity(crate::state::SNR_HISTORY_LEN), constellation: std::collections::VecDeque::new() },
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
                step_hz: 100_000, y_min: -120.0, y_max: 0.0,
                hold: None, cursor_freq: None, markers: vec![], pending_marker: None,
                style: cfg.display.spectrum_style,
            },
            waterfall: WaterfallState::new(cfg.display.waterfall_max_rows, cfg.display.waterfall_palette),
            system: SystemState {
                board_name: Arc::from(board_name.as_str()), serial: Arc::from(serial.as_str()),
                fw_version: Arc::from("Observer Mode"),
                board_rev: 0xFE, usb_api_version: 0,
                process_cpu_pct: 0.0, process_rss_mb: 0,
                cpu_history: std::collections::VecDeque::with_capacity(crate::state::THROUGHPUT_HISTORY_LEN),
            },
            timing: TimingState::default(),
            sweep: SweepState {
                config: SweepConfig {
                    start_hz: cfg.sweep.start_hz,
                    stop_hz:  cfg.sweep.stop_hz,
                    step_hz:  0,
                    dwell_ms: cfg.sweep.dwell_ms,
                },
                ..SweepState::default()
            },
            ui:  UiState::default(),
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
            m.push_log(format!("Observer Mode: {} (Serial: {})", board_name, serial));
            m.push_log("Device is in use by another process — hardware controls disabled");
        }

        tasks::spawn_observer_task(Arc::clone(&state), sysinfo.bus, sysinfo.dev, kind);
        tasks::spawn_sys_resource_task(Arc::clone(&state));

        let (engine, focus_keys) = Self::build_ui(&board_name, &serial, "observer", &cfg.presets);

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
            user_presets: cfg.presets,
        })
    }
}
