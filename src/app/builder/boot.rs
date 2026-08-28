//! What a fresh [`SdrMetrics`] looks like, and the handful of fields that depend
//! on whether there is a device to talk to.
//!
//! `new_normal` and `new_observer` each used to carry a copy of the same
//! ~110-line `SdrMetrics` literal. The two copies differed in **eleven** places;
//! everything else was byte-identical, which is the kind of duplication that
//! drifts silently - a field added to one boot path and not the other shows up
//! as a panel that reads correctly on a free radio and wrongly on a busy one.
//!
//! So the eleven live in [`Boot`], one per field, and the literal lives once in
//! [`initial_metrics`]. [`Boot::normal`] and [`Boot::observer`] are then the
//! whole of the difference between the two startups, in two readable blocks.
//!
//! [`resolve_tuning`] is the startup clamp, split out because it is pure - no
//! device, no clock, no lock - which is what lets the cross-device-family gain
//! and range snapping be tested with no radio attached.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

use crate::config::{AppConfig, RadioConfig};
use crate::hardware::{self, DeviceCapabilities};
use crate::state::{
    Accumulators, IqState, ObserverState, RadioState, SdrMetrics, SignalState, SpectrumMarker,
    SpectrumState, SweepConfig, SweepState, SystemState, TimingState, UiState, WaterfallState,
    RECALL_SLOTS, THROUGHPUT_HISTORY_LEN,
};

/// The radio settings a session starts at, after the startup clamp.
///
/// One struct so the clamp's five outputs travel together: the programmed
/// hardware state and the displayed state are the same numbers by construction,
/// rather than by two call sites remembering to agree.
pub(super) struct Tuning {
    pub frequency_hz: u64,
    pub sample_rate: f64,
    pub bb_filter_hz: u32,
    pub lna_gain: u32,
    pub vga_gain: u32,
}

/// Who the device says it is. `new_observer` fills this from sysfs, with
/// placeholders for what only an opened device can report.
pub(super) struct Identity {
    pub board_name: String,
    pub serial: String,
    pub fw_version: String,
    pub board_rev: u8,
    pub usb_api_version: u16,
}

/// Everything that differs between a live device and observer mode.
///
/// Adding a field here is the honest way to add a difference: it has to be given
/// a value in both [`Boot::normal`] and [`Boot::observer`], so a new divergence
/// cannot be introduced in one boot path and forgotten in the other.
pub(super) struct Boot {
    pub caps: Arc<DeviceCapabilities>,
    pub tuning: Tuning,
    pub identity: Identity,
    pub observer: ObserverState,
    /// Saved spectrum markers, restored only when there is a spectrum to put
    /// them on: observer mode spawns no FFT worker, so a marker there would
    /// annotate an axis that never draws - and `save_config` is a no-op in
    /// observer mode, so there is no path back for them either.
    pub markers: Vec<SpectrumMarker>,
    /// Command Rail recall slots. Empty in observer mode for the same reason:
    /// a recall jump retunes the radio, which observer mode cannot do.
    pub recall: [Option<u64>; RECALL_SLOTS],
}

impl Boot {
    /// A radio we opened: capabilities read from the device, config clamped into
    /// them, identity as the device reports it.
    pub(super) fn normal(
        cfg: &AppConfig,
        caps: Arc<DeviceCapabilities>,
        tuning: Tuning,
        info: &hardware::DeviceInfo,
    ) -> Self {
        Self {
            caps,
            tuning,
            identity: Identity {
                board_name: info.board_name.clone(),
                serial: info.serial.clone(),
                fw_version: info
                    .fw_version
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string()),
                board_rev: info.board_rev.unwrap_or(0xFE),
                usb_api_version: info.usb_api_version.unwrap_or(0),
            },
            observer: ObserverState::default(),
            markers: cfg.display.spectrum_markers.clone(),
            recall: crate::state::recall_from_hz(cfg.radio.recall_hz),
        }
    }

    /// A radio another process is holding: everything comes from sysfs or from a
    /// static capability profile for the matching backend.
    ///
    /// The config's tuning is taken **verbatim**, not clamped like [`Self::normal`]'s.
    /// The clamp exists to make a setting legal for the device it is about to be
    /// programmed into, and here nothing is programmed; worse, the observer
    /// capability profiles are placeholders - `rtlsdr::observer_caps()` carries
    /// an empty gain table, so snapping against it would report every RTL gain as
    /// 0 dB. A config value the operator recognises beats a fabricated legal one.
    pub(super) fn observer(
        cfg: &AppConfig,
        sysinfo: &hardware::sysfs::HackRfSysInfo,
        kind: hardware::DeviceKind,
    ) -> Self {
        Self {
            // Observer mode has no open device to query; use the matching
            // backend's capability profile so the UI labels stay correct.
            caps: Arc::new(match kind {
                hardware::DeviceKind::HackRf => hardware::hackrf::caps(),
                hardware::DeviceKind::RtlSdr => hardware::rtlsdr::observer_caps(),
            }),
            tuning: Tuning {
                frequency_hz: cfg.radio.frequency_hz,
                sample_rate: cfg.radio.sample_rate,
                bb_filter_hz: hardware::compute_bb_filter_bw(cfg.radio.sample_rate),
                lna_gain: cfg.radio.lna_gain,
                vga_gain: cfg.radio.vga_gain,
            },
            identity: Identity {
                board_name: sysinfo.product.clone(),
                serial: sysinfo.serial.clone(),
                fw_version: "Observer Mode".to_string(),
                board_rev: 0xFE,
                usb_api_version: 0,
            },
            observer: ObserverState {
                active: true,
                device: Some(format!("{} · {}", sysinfo.product, sysinfo.manufacturer)),
                serial: Some(sysinfo.serial.clone()),
                usb: Some(format!(
                    "High Speed ({} Mbit/s) · {} · Bus {}, Dev {}",
                    sysinfo.speed_mbits, sysinfo.max_power, sysinfo.bus, sysinfo.dev
                )),
                connected: sysinfo.connected_secs.map(crate::tasks::fmt_duration),
                ..Default::default()
            },
            markers: vec![],
            recall: [None; RECALL_SLOTS],
        }
    }
}

/// Clamp the stored config into THIS device's legal range, falling back to its
/// default when out of range - so a config saved on one device (e.g. a HackRF at
/// 2.4 GHz / 10 Msps) boots an RTL-SDR at a legal freq/rate instead of failing,
/// without discarding the original device's settings.
///
/// The gains are snapped into the device's gain model for the same reason, so a
/// config from another family neither programs an illegal gain nor displays one
/// (e.g. an RTL tuner's 49 dB on a HackRF LNA). One clamp feeds both the hardware
/// set and the state, so they always agree.
///
/// `bb_filter_hz` here is the computed baseband width for the clamped rate; the
/// caller overwrites it with what the device actually reported once
/// `set_sample_rate` has answered.
pub(super) fn resolve_tuning(radio: &RadioConfig, caps: &DeviceCapabilities) -> Tuning {
    let frequency_hz = if (caps.freq_min_hz..=caps.freq_max_hz).contains(&radio.frequency_hz) {
        radio.frequency_hz
    } else {
        caps.default_frequency_hz
    };
    let sample_rate =
        if (caps.sample_rate_min_hz..=caps.sample_rate_max_hz).contains(&radio.sample_rate) {
            radio.sample_rate
        } else {
            caps.default_sample_rate_hz
        };
    let (lna_gain, vga_gain) = caps.gain.clamp_gains(radio.lna_gain, radio.vga_gain);
    Tuning {
        frequency_hz,
        sample_rate,
        bb_filter_hz: hardware::compute_bb_filter_bw(sample_rate),
        lna_gain,
        vga_gain,
    }
}

/// The one shared `SdrMetrics` literal, parameterised by [`Boot`].
///
/// Everything not reached through `boot` is the same on both startups by
/// definition: history ring buffers sized but empty, every measurement at its
/// no-measurement value.
pub(super) fn initial_metrics(cfg: &AppConfig, boot: Boot) -> SdrMetrics {
    let Boot {
        caps,
        tuning,
        identity,
        observer,
        markers,
        recall,
    } = boot;

    SdrMetrics {
        radio: RadioState {
            frequency: tuning.frequency_hz,
            config_sample_rate: tuning.sample_rate,
            actual_sample_rate: 0,
            bb_filter_hz: tuning.bb_filter_hz,
            lna_gain: tuning.lna_gain,
            vga_gain: tuning.vga_gain,
            amp_enabled: cfg.radio.amp_enabled,
            rx_enabled: false,
            hw_streaming: false,
            rx_start_time: None,
            bytes_since_last_poll: 0,
            last_poll_time: Instant::now(),
            current_throughput_bps: 0,
            throughput_history: VecDeque::with_capacity(THROUGHPUT_HISTORY_LEN),
            sample_rate_history: VecDeque::with_capacity(THROUGHPUT_HISTORY_LEN),
        },
        signal: SignalState {
            drops_per_sec: 0,
            total_drops_session: 0,
            drop_history: VecDeque::with_capacity(THROUGHPUT_HISTORY_LEN),
            saturation_history: VecDeque::with_capacity(THROUGHPUT_HISTORY_LEN),
            usb_error_history: VecDeque::with_capacity(THROUGHPUT_HISTORY_LEN),
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
            jitter_history: VecDeque::with_capacity(THROUGHPUT_HISTORY_LEN),
            iq_amplitude_hist: [0u64; 32],
            adc_signed_hist: [0u64; 32],
            buf_fill_pct: 0.0,
            buf_fill_history: VecDeque::with_capacity(THROUGHPUT_HISTORY_LEN),
            phase_imbalance_deg: 0.0,
            cal: crate::state::IqCalState::default(),
            irr_history: VecDeque::with_capacity(crate::state::SNR_HISTORY_LEN),
            constellation: VecDeque::new(),
        },
        observer,
        spectrum: SpectrumState {
            step_hz: 100_000,
            y_min: -120.0,
            y_max: 0.0,
            hold: None,
            cursor_freq: None,
            markers,
            pending_marker: None,
            style: cfg.display.spectrum_style,
        },
        waterfall: WaterfallState::new(
            cfg.display.waterfall_max_rows,
            cfg.display.waterfall_palette,
        ),
        system: SystemState {
            board_name: Arc::from(identity.board_name.as_str()),
            serial: Arc::from(identity.serial.as_str()),
            fw_version: Arc::from(identity.fw_version.as_str()),
            board_rev: identity.board_rev,
            usb_api_version: identity.usb_api_version,
            process_cpu_pct: 0.0,
            process_rss_mb: 0,
            cpu_history: VecDeque::with_capacity(THROUGHPUT_HISTORY_LEN),
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
            recall,
            ..UiState::default()
        },
        lab: crate::state::LabState::default(),
        demod: crate::state::DemodState::default(),
        caps,
        acc: Accumulators::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::GainModel;

    /// A config written on a HackRF: 2.4 GHz, 10 Msps, LNA 24 / VGA 30.
    fn hackrf_config() -> AppConfig {
        let mut cfg = AppConfig::default();
        cfg.radio.frequency_hz = 2_400_000_000;
        cfg.radio.sample_rate = 10_000_000.0;
        cfg.radio.lna_gain = 24;
        cfg.radio.vga_gain = 30;
        cfg
    }

    #[test]
    fn a_hackrf_config_is_snapped_into_rtl_range() {
        // The case the clamp exists for: the same config.toml, opened on the
        // other device family. 2.4 GHz and 10 Msps are both out of an RTL-SDR's
        // reach, so each falls back to the device's own default rather than
        // being programmed as-is (which the driver would reject) or refused.
        let caps = hardware::rtlsdr::observer_caps();
        let t = resolve_tuning(&hackrf_config().radio, &caps);
        assert_eq!(t.frequency_hz, caps.default_frequency_hz);
        assert!((t.sample_rate - caps.default_sample_rate_hz).abs() < f64::EPSILON);
        assert!(
            (caps.freq_min_hz..=caps.freq_max_hz).contains(&t.frequency_hz),
            "fallback frequency must itself be legal"
        );
    }

    #[test]
    fn an_in_range_config_is_left_alone() {
        let caps = hardware::hackrf::caps();
        let t = resolve_tuning(&hackrf_config().radio, &caps);
        assert_eq!(t.frequency_hz, 2_400_000_000);
        assert!((t.sample_rate - 10_000_000.0).abs() < f64::EPSILON);
        // 24 is already on the HackRF's 8 dB LNA grid and 30 on its 2 dB VGA grid.
        assert_eq!((t.lna_gain, t.vga_gain), (24, 30));
    }

    #[test]
    fn gains_are_snapped_to_the_devices_own_grid() {
        // An RTL tuner's 49 dB has no meaning as a HackRF LNA value: the LNA is
        // an 0..=40 dB, 8 dB-stepped attenuator. Displaying 49 there would be a
        // number the hardware can never be in.
        let caps = hardware::hackrf::caps();
        let mut cfg = AppConfig::default();
        cfg.radio.lna_gain = 49;
        cfg.radio.vga_gain = 99;
        let t = resolve_tuning(&cfg.radio, &caps);
        assert_eq!(t.lna_gain, 40, "clamped to the LNA maximum");
        assert!(t.lna_gain.is_multiple_of(8), "and onto the 8 dB grid");
        assert_eq!(t.vga_gain, 62, "clamped to the VGA maximum");
        assert!(t.vga_gain.is_multiple_of(2), "and onto the 2 dB grid");
    }

    #[test]
    fn a_snapped_gain_is_one_the_device_actually_offers() {
        // The RTL path is a lookup into the tuner's own step table, not a range
        // clamp, so the general property is "the answer is in the table".
        let steps = vec![0u32, 9, 14, 27, 37, 49];
        let mut caps = hardware::rtlsdr::observer_caps();
        caps.gain = GainModel::RtlSingle {
            gain_steps_db: steps.clone(),
        };
        for asked in [0u32, 5, 12, 30, 100] {
            let mut cfg = AppConfig::default();
            cfg.radio.lna_gain = asked;
            let t = resolve_tuning(&cfg.radio, &caps);
            assert!(
                steps.contains(&t.lna_gain),
                "asked {asked} dB, got {} which is not a step this tuner has",
                t.lna_gain
            );
        }
    }

    /// The reason `Boot::observer` does **not** reuse [`resolve_tuning`].
    ///
    /// `observer_caps()` is a placeholder profile with an empty gain table, and
    /// `clamp_gains` on an empty table has one answer to give: 0. So clamping in
    /// observer mode would replace every gain the operator set with 0 dB and call
    /// it a legal value. This test pins that reasoning, so the "why not just
    /// share it" question is answered by the suite rather than re-litigated.
    #[test]
    fn observer_keeps_the_configured_gain_the_clamp_would_erase() {
        let cfg = hackrf_config();
        let caps = hardware::rtlsdr::observer_caps();
        assert_eq!(
            resolve_tuning(&cfg.radio, &caps).lna_gain,
            0,
            "the placeholder profile has no gain table to snap onto"
        );

        let boot = Boot::observer(&cfg, &sysinfo(), hardware::DeviceKind::RtlSdr);
        assert_eq!(
            boot.tuning.lna_gain, 24,
            "so observer mode shows the config"
        );
        assert_eq!(boot.tuning.frequency_hz, 2_400_000_000);
    }

    fn sysinfo() -> hardware::sysfs::HackRfSysInfo {
        hardware::sysfs::HackRfSysInfo {
            product: "RTL2838UHIDIR".into(),
            manufacturer: "Realtek".into(),
            serial: "00000001".into(),
            speed_mbits: 480,
            max_power: "500mA".into(),
            bus: 1,
            dev: 5,
            connected_secs: None,
        }
    }

    /// The two boot paths must agree about everything they do not deliberately
    /// disagree about - that is the whole point of the shared literal.
    #[test]
    fn both_startups_begin_with_no_measurements() {
        let cfg = AppConfig::default();
        for m in [
            initial_metrics(
                &cfg,
                Boot::observer(&cfg, &sysinfo(), hardware::DeviceKind::HackRf),
            ),
            initial_metrics(
                &cfg,
                Boot::normal(
                    &cfg,
                    Arc::new(hardware::hackrf::caps()),
                    resolve_tuning(&cfg.radio, &hardware::hackrf::caps()),
                    &hardware::DeviceInfo::default(),
                ),
            ),
        ] {
            assert!(!m.radio.rx_enabled);
            assert!(!m.radio.hw_streaming);
            assert_eq!(m.radio.actual_sample_rate, 0);
            assert_eq!(m.radio.current_throughput_bps, 0);
            assert_eq!(m.signal.total_drops_session, 0);
            assert!(m.radio.throughput_history.is_empty());
            assert!(m.iq.constellation.is_empty());
            assert!(m.waterfall.last_fft.is_none());
            assert_eq!(m.iq.iq_amplitude_hist, [0u64; 32]);
        }
    }

    #[test]
    fn observer_mode_starts_without_markers_or_recall() {
        let mut cfg = AppConfig::default();
        cfg.display.spectrum_markers = vec![SpectrumMarker {
            freq_hz: 100_000_000,
            label: "FM".into(),
            channel_bw_hz: None,
            measured_bw_hz: None,
        }];
        cfg.radio.recall_hz = [100_000_000, 0, 0];

        let live = initial_metrics(
            &cfg,
            Boot::normal(
                &cfg,
                Arc::new(hardware::hackrf::caps()),
                resolve_tuning(&cfg.radio, &hardware::hackrf::caps()),
                &hardware::DeviceInfo::default(),
            ),
        );
        assert_eq!(live.spectrum.markers.len(), 1);
        assert_eq!(live.ui.recall[0], Some(100_000_000));

        let obs = initial_metrics(
            &cfg,
            Boot::observer(&cfg, &sysinfo(), hardware::DeviceKind::HackRf),
        );
        assert!(obs.spectrum.markers.is_empty());
        assert!(obs.ui.recall.iter().all(|s| s.is_none()));
        assert!(obs.observer.active);
    }
}
