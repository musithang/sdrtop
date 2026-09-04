// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

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
use crate::hardware::{self, DeviceCapabilities, GainModel};
use crate::state::{
    Accumulators, IqState, ObserverState, RadioState, SdrMetrics, SignalState, SpectrumMarker,
    SpectrumState, SweepConfig, SweepState, SystemState, TimingState, UiState, WaterfallState,
    DEFAULT_LNA_GAIN, DEFAULT_VGA_GAIN, RECALL_SLOTS, THROUGHPUT_HISTORY_LEN, WATERFALL_MIN_ROWS,
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
    /// One value per stage, in `caps.gain.stages()` order, already snapped.
    pub gains: Vec<f64>,
    /// What the configured gain string could not be honoured about.
    ///
    /// Carried out of here rather than logged here, because this function is
    /// pure and its tests depend on that. `new_normal` pushes them. They were
    /// dropped on the floor until 0.5.0: a `gain` naming a stage the radio does
    /// not have produced a perfectly good diagnostic that nothing ever showed.
    pub notes: Vec<String>,
}

/// Who the device says it is. `new_observer` fills this from sysfs, with
/// placeholders for what only an opened device can report.
pub(super) struct Identity {
    pub board_name: String,
    pub serial: String,
    pub fw_version: String,
    pub stack: Option<crate::hardware::SoftwareStack>,
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
                stack: info.stack.clone(),
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
        profile: hardware::discovery::ObserverProfile,
    ) -> Self {
        Self {
            // Observer mode has no open device to query; the profile carries the
            // matching backend's capabilities so the UI labels stay correct.
            //
            // There is no arm here for a backend that cannot be observed, and no
            // assertion that we are not in one. `App::new` cannot reach this
            // function without a profile, and a profile only exists for a
            // backend that has one, so the case a `DeviceKind` match had to
            // handle is now one the type system does not offer.
            caps: Arc::new((profile.caps)()),
            tuning: Tuning {
                frequency_hz: cfg.radio.frequency_hz,
                sample_rate: cfg.radio.sample_rate,
                bb_filter_hz: hardware::native::hackrf::compute_bb_filter_bw(cfg.radio.sample_rate),
                // **Unclamped, deliberately.** Observer capability profiles are
                // placeholders whose gain tables hold a single zero, so snapping
                // here would replace every gain the operator set with 0 dB and
                // present it as a legal value. The test below pins the reasoning.
                gains: observer_gains(&cfg.radio),
                // Nothing to report: with no real stage list there is nothing
                // to fail to match a name against.
                notes: Vec::new(),
            },
            identity: Identity {
                board_name: sysinfo.product.clone(),
                serial: sysinfo.serial.clone(),
                fw_version: "Observer Mode".to_string(),
                // Observer mode reads sysfs for a device sdrtop knows by name,
                // and only the two native backends have such a profile.
                stack: None,
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
    let (gains, notes) = resolve_gains(radio, &caps.gain);
    Tuning {
        frequency_hz,
        sample_rate,
        bb_filter_hz: hardware::native::hackrf::compute_bb_filter_bw(sample_rate),
        gains,
        notes,
    }
}

/// The configured gains as they are, with no device to snap them onto.
///
/// Observer mode's capability profile is a placeholder, so there is no real
/// stage list to resolve against. The named form is read positionally in the
/// order it was written, which is the best that can be done without knowing the
/// device, and the pre-0.5.0 pair falls straight through.
fn observer_gains(radio: &RadioConfig) -> Vec<f64> {
    let mut values: Vec<f64> = match radio.gain.as_deref().map(str::trim) {
        Some(text) if !text.is_empty() => match text.parse::<f64>() {
            Ok(total) => vec![total],
            Err(_) => hardware::gain::parse_named(text)
                .0
                .into_iter()
                .map(|(_, v)| v)
                .collect(),
        },
        _ => vec![DEFAULT_LNA_GAIN as f64, DEFAULT_VGA_GAIN as f64],
    };
    for (index, override_db) in [(0usize, radio.lna_gain), (1, radio.vga_gain)] {
        if let Some(db) = override_db {
            if values.len() <= index {
                values.resize(index + 1, 0.0);
            }
            values[index] = db as f64;
        }
    }
    values
}

/// Turn what the config says about gain into one value per stage.
///
/// Three layers, in increasing order of how deliberate they are, which is the
/// same order the presets merge in:
///
/// 1. the device's own minimums, so every stage has a value;
/// 2. the `gain` string, which is either a **whole-chain total** to distribute
///    or `NAME=value` pairs to place;
/// 3. the pre-0.5.0 `lna_gain` / `vga_gain`, which are **positional** overrides
///    on the first two stages.
///
/// Layer 3 is doing two jobs at once and that is deliberate. It migrates an old
/// config, whose gains are exactly a positional pair, and it carries `--lna` and
/// `--vga`, which are positional by nature: `--lna` on an RTL-SDR means its
/// tuner, whatever the driver calls it. Layering them on top of the string
/// rather than instead of it means a flag overrides one stage without silently
/// resetting the others.
///
/// Never fails. A gain string nobody can parse leaves the defaults in place and
/// says so, because a config that refuses to load is worse than one setting that
/// did not take.
pub fn resolve_gains(radio: &RadioConfig, gm: &GainModel) -> (Vec<f64>, Vec<String>) {
    let stages = gm.stages();
    let mut notes = Vec::new();
    let mut values: Vec<f64> = stages.iter().map(|s| s.min_db).collect();

    match radio.gain.as_deref().map(str::trim) {
        Some(text) if !text.is_empty() => match text.parse::<f64>() {
            // A bare number is the whole chain, spread the way the knob spreads
            // it, so the file and the `[UP]` key mean the same thing.
            Ok(total) => values = hardware::gain::distribute(&stages, total).0,
            Err(_) => {
                let (pairs, mut n) = hardware::gain::parse_named(text);
                let (v, more) = hardware::gain::apply_named(&stages, &pairs, &values);
                n.extend(more);
                notes.extend(n);
                values = v;
            }
        },
        // No string at all: the historical defaults, so a first run is unchanged.
        _ => {
            if let Some(first) = stages.first() {
                values[0] = first.snap(DEFAULT_LNA_GAIN as f64);
            }
            if let Some(second) = stages.get(1) {
                values[1] = second.snap(DEFAULT_VGA_GAIN as f64);
            }
        }
    }

    for (index, override_db) in [(0usize, radio.lna_gain), (1, radio.vga_gain)] {
        if let (Some(db), Some(spec)) = (override_db, stages.get(index)) {
            values[index] = spec.snap(db as f64);
        }
    }
    (values, notes)
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
            // Already one value per stage, snapped onto the device's own grids
            // by `resolve_gains`.
            gains: tuning.gains.clone(),
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
        // Clamped, not trusted: `save_config` writes the live buffer depth back,
        // so a config written before the floor existed carries a value too small
        // to fill a full-height waterfall. See [`WATERFALL_MIN_ROWS`].
        waterfall: WaterfallState::new(
            cfg.display.waterfall_max_rows.max(WATERFALL_MIN_ROWS),
            cfg.display.waterfall_palette,
        ),
        system: SystemState {
            board_name: Arc::from(identity.board_name.as_str()),
            serial: Arc::from(identity.serial.as_str()),
            fw_version: Arc::from(identity.fw_version.as_str()),
            stack: identity.stack,
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

    /// A config written on a HackRF: 2.4 GHz, 10 Msps, LNA 24 / VGA 30.
    fn hackrf_config() -> AppConfig {
        let mut cfg = AppConfig::default();
        cfg.radio.frequency_hz = 2_400_000_000;
        cfg.radio.sample_rate = 10_000_000.0;
        cfg.radio.lna_gain = Some(24);
        cfg.radio.vga_gain = Some(30);
        cfg
    }

    #[test]
    fn a_hackrf_config_is_snapped_into_rtl_range() {
        // The case the clamp exists for: the same config.toml, opened on the
        // other device family. 2.4 GHz and 10 Msps are both out of an RTL-SDR's
        // reach, so each falls back to the device's own default rather than
        // being programmed as-is (which the driver would reject) or refused.
        let caps = hardware::native::rtlsdr::observer_caps();
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
        let caps = hardware::native::hackrf::caps();
        let t = resolve_tuning(&hackrf_config().radio, &caps);
        assert_eq!(t.frequency_hz, 2_400_000_000);
        assert!((t.sample_rate - 10_000_000.0).abs() < f64::EPSILON);
        // 24 is already on the HackRF's 8 dB LNA grid and 30 on its 2 dB VGA grid.
        assert_eq!(t.gains, vec![24.0, 30.0]);
    }

    #[test]
    fn gains_are_snapped_to_the_devices_own_grid() {
        // An RTL tuner's 49 dB has no meaning as a HackRF LNA value: the LNA is
        // an 0..=40 dB, 8 dB-stepped attenuator. Displaying 49 there would be a
        // number the hardware can never be in.
        let caps = hardware::native::hackrf::caps();
        let mut cfg = AppConfig::default();
        cfg.radio.lna_gain = Some(49);
        cfg.radio.vga_gain = Some(99);
        let t = resolve_tuning(&cfg.radio, &caps);
        assert_eq!(t.gains[0], 40.0, "clamped to the LNA maximum");
        assert_eq!(t.gains[0] % 8.0, 0.0, "and onto the 8 dB grid");
        assert_eq!(t.gains[1], 62.0, "clamped to the VGA maximum");
        assert_eq!(t.gains[1] % 2.0, 0.0, "and onto the 2 dB grid");
    }

    #[test]
    fn a_snapped_gain_is_one_the_device_actually_offers() {
        // The RTL path is a lookup into the tuner's own step table, not a range
        // clamp, so the general property is "the answer is in the table".
        let steps = vec![0u32, 9, 14, 27, 37, 49];
        let mut caps = hardware::native::rtlsdr::observer_caps();
        caps.gain = crate::hardware::native::rtlsdr::gain_model(&steps);
        for asked in [0u32, 5, 12, 30, 100] {
            let mut cfg = AppConfig::default();
            cfg.radio.lna_gain = Some(asked);
            let t = resolve_tuning(&cfg.radio, &caps);
            let got = t.gains[0] as u32;
            assert!(
                steps.contains(&got),
                "asked {asked} dB, got {got} which is not a step this tuner has"
            );
        }
    }

    #[test]
    fn resolve_tuning_carries_the_gain_notes_out_rather_than_dropping_them() {
        // The diagnostic was computed and thrown away until 0.5.0: a `gain`
        // naming a stage the radio does not have looked exactly like one that
        // worked.
        let mut cfg = hackrf_config();
        cfg.radio.gain = Some("IF1=10".to_string());
        let t = resolve_tuning(&cfg.radio, &hardware::native::hackrf::caps());
        assert_eq!(t.notes.len(), 1, "{:?}", t.notes);
        assert!(t.notes[0].contains("IF1"), "{}", t.notes[0]);
        assert!(t.notes[0].contains("LNA"), "{}", t.notes[0]);
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
        let caps = hardware::native::rtlsdr::observer_caps();
        assert_eq!(
            resolve_tuning(&cfg.radio, &caps).gains,
            vec![0.0],
            "the placeholder profile's table holds only 0, so everything snaps there"
        );

        let boot = Boot::observer(&cfg, &sysinfo(), profile(hardware::DeviceKind::RtlSdr));
        assert_eq!(
            boot.tuning.gains,
            vec![24.0, 30.0],
            "so observer mode shows the config"
        );
        assert_eq!(boot.tuning.frequency_hz, 2_400_000_000);
    }

    /// The observer profile for a backend that has one. The `expect` is the
    /// point: a test naming a backend that cannot be observed should say so
    /// loudly rather than quietly testing something else.
    fn profile(kind: hardware::DeviceKind) -> hardware::discovery::ObserverProfile {
        kind.observer_profile()
            .expect("this backend can be observed")
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
                Boot::observer(&cfg, &sysinfo(), profile(hardware::DeviceKind::HackRf)),
            ),
            initial_metrics(
                &cfg,
                Boot::normal(
                    &cfg,
                    Arc::new(hardware::native::hackrf::caps()),
                    resolve_tuning(&cfg.radio, &hardware::native::hackrf::caps()),
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

    /// A config that asks for less waterfall history than a full-height panel
    /// needs is raised, not honoured.
    ///
    /// `save_config` writes the live buffer depth back, so the old 64-row default
    /// is baked into the config of anyone who has ever quit the app. Honouring it
    /// leaves a blank strip above the waterfall's bottom border that never fills.
    #[test]
    fn a_shallow_waterfall_buffer_is_raised_to_the_floor() {
        let mut cfg = AppConfig::default();
        cfg.display.waterfall_max_rows = 64;
        let m = initial_metrics(
            &cfg,
            Boot::observer(&cfg, &sysinfo(), profile(hardware::DeviceKind::HackRf)),
        );
        assert_eq!(
            m.waterfall.buffer.max_rows,
            crate::state::WATERFALL_MIN_ROWS
        );
    }

    /// A deeper setting is the user's to make, and is left alone.
    #[test]
    fn a_deep_waterfall_buffer_is_left_alone() {
        let mut cfg = AppConfig::default();
        cfg.display.waterfall_max_rows = 4_096;
        let m = initial_metrics(
            &cfg,
            Boot::observer(&cfg, &sysinfo(), profile(hardware::DeviceKind::HackRf)),
        );
        assert_eq!(m.waterfall.buffer.max_rows, 4_096);
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
                Arc::new(hardware::native::hackrf::caps()),
                resolve_tuning(&cfg.radio, &hardware::native::hackrf::caps()),
                &hardware::DeviceInfo::default(),
            ),
        );
        assert_eq!(live.spectrum.markers.len(), 1);
        assert_eq!(live.ui.recall[0], Some(100_000_000));

        let obs = initial_metrics(
            &cfg,
            Boot::observer(&cfg, &sysinfo(), profile(hardware::DeviceKind::HackRf)),
        );
        assert!(obs.spectrum.markers.is_empty());
        assert!(obs.ui.recall.iter().all(|s| s.is_none()));
        assert!(obs.observer.active);
    }

    // ── The config's gain, and its migration ────────────────────────────────

    /// A file written before 0.5.0 has a positional pair and no string. It must
    /// keep its gains: someone's carefully set radio is not a good place to
    /// discover that the format changed.
    #[test]
    fn a_pre_0_5_0_config_keeps_its_gains() {
        let caps = hardware::native::hackrf::caps();
        let cfg = hackrf_config();
        assert!(cfg.radio.gain.is_none(), "the old form has no string");
        let (gains, notes) = resolve_gains(&cfg.radio, &caps.gain);
        assert_eq!(gains, vec![24.0, 30.0]);
        assert!(notes.is_empty(), "{notes:?}");
    }

    /// A file with neither form is a first run, and must land where it always
    /// did rather than at the device's floor.
    #[test]
    fn a_config_with_no_gain_at_all_uses_the_historical_defaults() {
        let caps = hardware::native::hackrf::caps();
        let cfg = AppConfig::default();
        let (gains, _) = resolve_gains(&cfg.radio, &caps.gain);
        assert_eq!(gains, vec![16.0, 20.0]);
    }

    /// The named form names the device's own stages, and each value lands on
    /// that stage's grid.
    #[test]
    fn the_named_form_places_each_stage() {
        let caps = hardware::native::hackrf::caps();
        let mut cfg = AppConfig::default();
        cfg.radio.gain = Some("VGA=41,LNA=27".into());
        let (gains, notes) = resolve_gains(&cfg.radio, &caps.gain);
        assert_eq!(
            gains,
            vec![24.0, 42.0],
            "order in the string does not matter"
        );
        assert!(notes.is_empty(), "{notes:?}");
    }

    /// A bare number is the whole chain, spread the way the knob spreads it, so
    /// `gain = "45"` and holding `[UP]` to 45 dB mean the same thing.
    #[test]
    fn a_bare_number_is_a_whole_chain_total() {
        let caps = hardware::native::hackrf::caps();
        let mut cfg = AppConfig::default();
        cfg.radio.gain = Some("45".into());
        let (gains, _) = resolve_gains(&cfg.radio, &caps.gain);
        assert_eq!(
            gains,
            vec![40.0, 4.0],
            "front to back, 44 of the 45 reachable"
        );
    }

    /// `--lna` and `--vga` are positional overrides layered **on top of** the
    /// string, so one flag does not silently reset the other stages.
    #[test]
    fn the_positional_flags_override_one_stage_and_leave_the_rest() {
        let caps = hardware::native::hackrf::caps();
        let mut cfg = AppConfig::default();
        cfg.radio.gain = Some("LNA=8,VGA=40".into());
        cfg.radio.lna_gain = Some(32);
        let (gains, _) = resolve_gains(&cfg.radio, &caps.gain);
        assert_eq!(gains, vec![32.0, 40.0], "the VGA kept what the string set");
    }

    /// A config written for another radio. The unknown stage is reported and the
    /// known one still applies; the file loads either way, because a config that
    /// refuses to load is worse than one setting that did not take.
    #[test]
    fn a_string_naming_another_radios_stage_is_reported_not_fatal() {
        let caps = hardware::native::hackrf::caps();
        let mut cfg = AppConfig::default();
        cfg.radio.gain = Some("IFGR=20,LNA=16".into());
        let (gains, notes) = resolve_gains(&cfg.radio, &caps.gain);
        assert_eq!(gains[0], 16.0);
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("IFGR"), "{:?}", notes[0]);
    }

    /// Text nobody can parse leaves the stage list intact rather than emptying
    /// it, and says what it could not read.
    #[test]
    fn unreadable_text_leaves_the_defaults_and_says_so() {
        let caps = hardware::native::hackrf::caps();
        let mut cfg = AppConfig::default();
        cfg.radio.gain = Some("banana".into());
        let (gains, notes) = resolve_gains(&cfg.radio, &caps.gain);
        assert_eq!(gains.len(), 2, "still one value per stage");
        assert!(!notes.is_empty(), "and it said why");
    }

    /// Observer mode has no device to snap against, so the configured gain
    /// passes through untouched. The placeholder profile would flatten it to 0.
    #[test]
    fn observer_reads_the_named_form_without_a_device() {
        let mut cfg = AppConfig::default();
        cfg.radio.gain = Some("LNA=37,VGA=41".into());
        assert_eq!(observer_gains(&cfg.radio), vec![37.0, 41.0], "not snapped");
        cfg.radio.gain = Some("55".into());
        assert_eq!(observer_gains(&cfg.radio), vec![55.0]);
    }
}
