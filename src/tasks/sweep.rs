// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `sweep_task` - drives the frequency scanner for the `lab_sweep` preset.
//!
//! While `state.sweep.active` (set when `lab_sweep` is the active preset), the
//! task walks the configured band one position at a time: retune → settle →
//! dwell while harvesting the shared FFT frames → record peak / mean per bin →
//! advance. A completed pass is published as a `SweepFrame`. It reuses the
//! existing RX → FFT pipeline rather than running its own FFT: it just steers the
//! tuner and reads `state.waterfall.last_fft`, so it stays mutually exclusive
//! with normal RX (both can't own the tuner at once) while sharing the plumbing.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::hardware::{DirectSweepConfig, SdrDevice};
use crate::state::{SdrMetrics, SweepFrame, SWEEP_SETTLING_MS};

const DWELL_POLL_MS: u64 = 10;
const FRAME_FRESH_MS: u128 = 200;
const DIRECT_SWEEP_RETRY: Duration = Duration::from_secs(1);

pub fn spawn_sweep_task(state: Arc<Mutex<SdrMetrics>>, device: Arc<dyn SdrDevice>) {
    if device.capabilities().acquisition == crate::hardware::AcquisitionKind::PowerTrace {
        spawn_direct_sweep_task(state, device);
        return;
    }
    tokio::spawn(async move {
        let mut was_active = false;
        let mut saved_rx_enabled = false;

        loop {
            let (active, config, sample_rate, caps) = {
                let m = state.lock().unwrap_or_else(|e| e.into_inner());
                (
                    m.sweep.active,
                    m.sweep.config.clone(),
                    m.radio.config_sample_rate,
                    Arc::clone(&m.caps),
                )
            };
            let (fmin, fmax) = (caps.freq_min_hz, caps.freq_max_hz);

            if active && !was_active {
                was_active = true;
                let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
                m.sweep.pre_sweep_hz = Some(m.radio.frequency);
                saved_rx_enabled = m.radio.rx_enabled;
                m.radio.rx_enabled = true;
                m.sweep.cycle_count = 0;
                m.sweep.positions_done = 0;
                m.sweep.positions_total = config.positions_total(sample_rate);
                m.push_log(format!(
                    "Sweep started: {:.1}–{:.1} MHz",
                    config.start_hz as f64 / 1e6,
                    config.stop_hz as f64 / 1e6
                ));
            }

            if !active {
                if was_active {
                    was_active = false;
                    let exit = {
                        let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
                        let tuned = m.radio.frequency;
                        m.sweep.end(tuned)
                    };
                    let _ = device.set_frequency(exit.tune_hz);
                    let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
                    m.radio.frequency = exit.tune_hz;
                    m.radio.rx_enabled = if exit.jumped { true } else { saved_rx_enabled };
                    m.push_log(if exit.jumped {
                        format!("Tuned to {:.3} MHz from sweep", exit.tune_hz as f64 / 1e6)
                    } else {
                        "Sweep stopped".to_string()
                    });
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }

            let positions = config.positions_total(sample_rate);
            if positions == 0 {
                tokio::time::sleep(Duration::from_millis(200)).await;
                continue;
            }

            let cycle_start = Instant::now();
            let mut freq_hz: Vec<u64> = Vec::new();
            let mut peak: Vec<f32> = Vec::new();
            let mut mean: Vec<f32> = Vec::new();

            for i in 0..positions {
                if !state.lock().unwrap_or_else(|e| e.into_inner()).sweep.active {
                    break;
                }
                let hz = config.position_hz(i, sample_rate).clamp(fmin, fmax);
                // A position in a gap between bands is not tuned: the radio
                // would deliver something there, and it would not be a
                // reading of that frequency. The curve keeps the hole.
                if !caps.tunes(hz) {
                    continue;
                }
                let _ = device.set_frequency(hz);
                {
                    let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
                    m.radio.frequency = hz;
                    m.sweep.current_hz = hz;
                    m.sweep.positions_done = i;
                }
                tokio::time::sleep(Duration::from_millis(SWEEP_SETTLING_MS)).await;

                let dwell_start = Instant::now();
                let mut pos_peak: Vec<f32> = Vec::new();
                let mut pos_mean_sum: Vec<f64> = Vec::new();
                let mut pos_sr = sample_rate;
                let mut frames = 0u32;
                while dwell_start.elapsed() < Duration::from_millis(config.dwell_ms) {
                    {
                        let m = state.lock().unwrap_or_else(|e| e.into_inner());
                        if let Some(fr) = &m.waterfall.last_fft {
                            if fr.center_freq_hz == hz
                                && fr.timestamp.elapsed().as_millis() < FRAME_FRESH_MS
                            {
                                let bins = &fr.bins_dbfs;
                                if pos_peak.len() != bins.len() {
                                    pos_peak = vec![f32::NEG_INFINITY; bins.len()];
                                    pos_mean_sum = vec![0.0; bins.len()];
                                    pos_sr = fr.sample_rate;
                                }
                                for (j, &b) in bins.iter().enumerate() {
                                    if b > pos_peak[j] {
                                        pos_peak[j] = b;
                                    }
                                    pos_mean_sum[j] += b as f64;
                                }
                                frames += 1;
                            }
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(DWELL_POLL_MS)).await;
                }

                if frames > 0 {
                    let n = pos_peak.len();
                    for j in 0..n {
                        let f = (hz as f64 - pos_sr / 2.0 + (j as f64 / n as f64) * pos_sr) as u64;
                        // Bins a position near a band's edge sees across it,
                        // in the gap, are left out for the same reason.
                        if caps.in_gap(f) {
                            continue;
                        }
                        freq_hz.push(f);
                        peak.push(pos_peak[j]);
                        mean.push((pos_mean_sum[j] / frames as f64) as f32);
                    }
                }
            }

            {
                let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
                if m.sweep.active && !freq_hz.is_empty() {
                    let dur = cycle_start.elapsed().as_millis() as u64;
                    m.sweep.cycle_count += 1;
                    m.sweep.cycle_duration_ms = dur;
                    m.sweep.positions_done = positions;
                    let cc = m.sweep.cycle_count;
                    m.sweep.current_frame = Some(Arc::new(SweepFrame {
                        start_hz: config.start_hz,
                        stop_hz: config.stop_hz,
                        freq_hz,
                        peak_dbfs: peak,
                        mean_dbfs: mean,
                        timestamp: Instant::now(),
                        cycle_count: cc,
                        cycle_duration_ms: dur,
                    }));
                }
            }
        }
    });
}

trait DirectSweepControl {
    fn set_direct_sweep(&self, config: Option<DirectSweepConfig>) -> anyhow::Result<()>;
    fn set_frequency(&self, hz: u64) -> anyhow::Result<()>;
}

impl DirectSweepControl for dyn SdrDevice {
    fn set_direct_sweep(&self, config: Option<DirectSweepConfig>) -> anyhow::Result<()> {
        SdrDevice::set_direct_sweep(self, config)
    }

    fn set_frequency(&self, hz: u64) -> anyhow::Result<()> {
        SdrDevice::set_frequency(self, hz)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SweepRequest {
    start_hz: u64,
    stop_hz: u64,
}

struct FailedRequest {
    config: DirectSweepConfig,
    at: Instant,
    message: String,
}

struct PendingExit {
    tune_hz: u64,
    jumped: bool,
    direct_disabled: bool,
    retry_at: Instant,
    last_error: Option<String>,
}

#[derive(Default)]
struct DirectSweepController {
    was_active: bool,
    requested: Option<SweepRequest>,
    applied: Option<DirectSweepConfig>,
    failed: Option<FailedRequest>,
    request_generation: u64,
    saved_frequency: u64,
    saved_rx_enabled: bool,
    pending_exit: Option<PendingExit>,
}

impl DirectSweepController {
    fn update<C: DirectSweepControl + ?Sized>(
        &mut self,
        state: &Arc<Mutex<SdrMetrics>>,
        device: &C,
        now: Instant,
    ) {
        let (active, config) = {
            let metrics = state.lock().unwrap_or_else(|error| error.into_inner());
            (metrics.sweep.active, metrics.sweep.config.clone())
        };

        if self.pending_exit.is_some() {
            self.leave(state, device, now);
            if self.pending_exit.is_some() || !active {
                return;
            }
        }

        if active && !self.was_active {
            self.was_active = true;
            let mut metrics = state.lock().unwrap_or_else(|error| error.into_inner());
            self.saved_frequency = metrics.radio.frequency;
            self.saved_rx_enabled = metrics.radio.rx_enabled;
            metrics.sweep.pre_sweep_hz = Some(self.saved_frequency);
            metrics.radio.rx_enabled = true;
            metrics.sweep.cycle_count = 0;
            metrics.sweep.positions_done = 0;
            metrics.sweep.positions_total = 0;
            metrics.sweep.current_frame = None;
            metrics.sweep.generation = metrics.sweep.generation.wrapping_add(1);
            metrics.push_log(format!(
                "Sweep started: {:.1}\u{2013}{:.1} MHz",
                config.start_hz as f64 / 1e6,
                config.stop_hz as f64 / 1e6
            ));
        }

        if active {
            self.apply_request(state, device, now, config);
        } else if self.was_active {
            self.leave(state, device, now);
        }
    }

    fn apply_request<C: DirectSweepControl + ?Sized>(
        &mut self,
        state: &Arc<Mutex<SdrMetrics>>,
        device: &C,
        now: Instant,
        config: crate::state::SweepConfig,
    ) {
        let request = SweepRequest {
            start_hz: config.start_hz,
            stop_hz: config.stop_hz,
        };
        let generation = {
            let mut metrics = state.lock().unwrap_or_else(|error| error.into_inner());
            if self.requested.is_some()
                && self.requested != Some(request)
                && metrics.sweep.generation == self.request_generation
            {
                metrics.sweep.generation = metrics.sweep.generation.wrapping_add(1);
            }
            self.requested = Some(request);
            self.request_generation = metrics.sweep.generation;
            metrics.sweep.generation
        };
        let requested = DirectSweepConfig {
            start_hz: request.start_hz,
            stop_hz: request.stop_hz,
            generation,
        };
        let retry_due = self.failed.as_ref().is_none_or(|failed| {
            failed.config != requested
                || now.saturating_duration_since(failed.at) >= DIRECT_SWEEP_RETRY
        });
        if self.applied == Some(requested) || !retry_due {
            return;
        }
        match device.set_direct_sweep(Some(requested)) {
            Ok(()) => {
                self.applied = Some(requested);
                self.failed = None;
            }
            Err(error) => {
                let message = error.to_string();
                let mut metrics = state.lock().unwrap_or_else(|error| error.into_inner());
                if self
                    .failed
                    .as_ref()
                    .is_none_or(|failed| failed.config != requested || failed.message != message)
                {
                    metrics.push_log(format!("Sweep error: {message}"));
                }
                self.failed = Some(FailedRequest {
                    config: requested,
                    at: now,
                    message,
                });
            }
        }
    }

    fn leave<C: DirectSweepControl + ?Sized>(
        &mut self,
        state: &Arc<Mutex<SdrMetrics>>,
        device: &C,
        now: Instant,
    ) {
        if self.pending_exit.is_none() {
            let exit = {
                let mut metrics = state.lock().unwrap_or_else(|error| error.into_inner());
                let exit = metrics.sweep.end(self.saved_frequency);
                metrics.sweep.generation = metrics.sweep.generation.wrapping_add(1);
                exit
            };
            self.pending_exit = Some(PendingExit {
                tune_hz: exit.tune_hz,
                jumped: exit.jumped,
                direct_disabled: false,
                retry_at: now,
                last_error: None,
            });
        }

        let pending = self.pending_exit.as_mut().unwrap();
        if now < pending.retry_at {
            return;
        }
        if !pending.direct_disabled {
            match device.set_direct_sweep(None) {
                Ok(()) => pending.direct_disabled = true,
                Err(error) => {
                    record_exit_error(state, pending, now, error);
                    return;
                }
            }
        }
        if let Err(error) = device.set_frequency(pending.tune_hz) {
            record_exit_error(state, pending, now, error);
            return;
        }

        let tune_hz = pending.tune_hz;
        let jumped = pending.jumped;
        self.pending_exit = None;
        self.was_active = false;
        self.requested = None;
        self.applied = None;
        self.failed = None;
        let mut metrics = state.lock().unwrap_or_else(|error| error.into_inner());
        metrics.radio.frequency = tune_hz;
        metrics.radio.rx_enabled = if jumped { true } else { self.saved_rx_enabled };
        metrics.push_log(if jumped {
            format!("Tuned to {:.3} MHz from sweep", tune_hz as f64 / 1e6)
        } else {
            "Sweep stopped".to_string()
        });
    }
}

fn record_exit_error(
    state: &Arc<Mutex<SdrMetrics>>,
    pending: &mut PendingExit,
    now: Instant,
    error: anyhow::Error,
) {
    let message = error.to_string();
    if pending.last_error.as_deref() != Some(&message) {
        state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push_log(format!("Sweep restore error: {message}"));
    }
    pending.last_error = Some(message);
    pending.retry_at = now + DIRECT_SWEEP_RETRY;
}

fn spawn_direct_sweep_task(state: Arc<Mutex<SdrMetrics>>, device: Arc<dyn SdrDevice>) {
    tokio::spawn(async move {
        let mut controller = DirectSweepController::default();
        loop {
            controller.update(&state, device.as_ref(), Instant::now());
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });
}

#[cfg(test)]
mod direct_tests {
    use super::*;

    #[derive(Default)]
    struct TestDevice {
        requests: Mutex<Vec<Option<DirectSweepConfig>>>,
        frequencies: Mutex<Vec<u64>>,
        failures: Mutex<usize>,
        frequency_failures: Mutex<usize>,
    }

    impl DirectSweepControl for TestDevice {
        fn set_direct_sweep(&self, config: Option<DirectSweepConfig>) -> anyhow::Result<()> {
            self.requests.lock().unwrap().push(config);
            let mut failures = self.failures.lock().unwrap();
            if *failures > 0 {
                *failures -= 1;
                anyhow::bail!("temporary sweep failure");
            }
            Ok(())
        }

        fn set_frequency(&self, hz: u64) -> anyhow::Result<()> {
            self.frequencies.lock().unwrap().push(hz);
            let mut failures = self.frequency_failures.lock().unwrap();
            if *failures > 0 {
                *failures -= 1;
                anyhow::bail!("temporary frequency failure");
            }
            Ok(())
        }
    }

    fn active_state(rx_enabled: bool) -> Arc<Mutex<SdrMetrics>> {
        let state = Arc::new(Mutex::new(SdrMetrics::fixture()));
        {
            let mut metrics = state.lock().unwrap();
            metrics.radio.frequency = 145_500_000;
            metrics.radio.rx_enabled = rx_enabled;
            metrics.sweep.active = true;
            metrics.sweep.config.start_hz = 88_000_000;
            metrics.sweep.config.stop_hz = 108_000_000;
            metrics.sweep.config.dwell_ms = 200;
        }
        state
    }

    #[test]
    fn requests_follow_session_and_range_transitions() {
        let state = active_state(false);
        let device = TestDevice::default();
        let mut controller = DirectSweepController::default();
        let now = Instant::now();
        controller.update(&state, &device, now);

        {
            let mut metrics = state.lock().unwrap();
            metrics.sweep.config.start_hz = 400_000_000;
            metrics.sweep.config.stop_hz = 500_000_000;
        }
        controller.update(&state, &device, now);
        state.lock().unwrap().sweep.active = false;
        controller.update(&state, &device, now);

        let requests = device.requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[0].unwrap().generation, 1);
        assert_eq!(requests[1].unwrap().generation, 2);
        assert_eq!(requests[1].unwrap().start_hz, 400_000_000);
        assert_eq!(requests[2], None);
        assert_eq!(state.lock().unwrap().sweep.generation, 3);
    }

    #[test]
    fn failures_retry_after_the_existing_delay() {
        let state = active_state(false);
        let device = TestDevice::default();
        *device.failures.lock().unwrap() = 1;
        let mut controller = DirectSweepController::default();
        let now = Instant::now();

        controller.update(&state, &device, now);
        controller.update(&state, &device, now + Duration::from_millis(999));
        assert_eq!(device.requests.lock().unwrap().len(), 1);
        controller.update(&state, &device, now + DIRECT_SWEEP_RETRY);
        assert_eq!(device.requests.lock().unwrap().len(), 2);
    }

    #[test]
    fn dwell_changes_do_not_restart_the_device_sweep() {
        let state = active_state(false);
        let device = TestDevice::default();
        let mut controller = DirectSweepController::default();
        let now = Instant::now();
        controller.update(&state, &device, now);

        state.lock().unwrap().sweep.config.dwell_ms += 50;
        controller.update(&state, &device, now);
        assert_eq!(device.requests.lock().unwrap().len(), 1);
        assert_eq!(state.lock().unwrap().sweep.generation, 1);
    }

    #[test]
    fn a_new_session_clears_the_previous_frame() {
        let state = Arc::new(Mutex::new(
            SdrMetrics::fixture()
                .streaming()
                .with_sweep(88_000_000, 108_000_000),
        ));
        let device = TestDevice::default();
        let mut controller = DirectSweepController::default();
        controller.update(&state, &device, Instant::now());

        assert!(state.lock().unwrap().sweep.current_frame.is_none());
    }

    #[test]
    fn pause_state_is_preserved() {
        for started_enabled in [false, true] {
            let state = active_state(started_enabled);
            let device = TestDevice::default();
            let mut controller = DirectSweepController::default();
            let now = Instant::now();

            controller.update(&state, &device, now);
            assert!(state.lock().unwrap().radio.rx_enabled);
            state.lock().unwrap().radio.rx_enabled = false;
            controller.update(&state, &device, now);
            assert!(!state.lock().unwrap().radio.rx_enabled);
            state.lock().unwrap().sweep.active = false;
            controller.update(&state, &device, now);
            assert_eq!(state.lock().unwrap().radio.rx_enabled, started_enabled);
        }
    }

    #[test]
    fn tuner_restoration_keeps_exact_jumps() {
        let state = active_state(false);
        let device = TestDevice::default();
        let mut controller = DirectSweepController::default();
        let now = Instant::now();
        controller.update(&state, &device, now);

        {
            let mut metrics = state.lock().unwrap();
            metrics.sweep.pending_tune = Some(433_920_001);
            metrics.sweep.active = false;
        }
        controller.update(&state, &device, now);
        assert_eq!(device.frequencies.lock().unwrap().as_slice(), [433_920_001]);
        let metrics = state.lock().unwrap();
        assert_eq!(metrics.radio.frequency, 433_920_001);
        assert!(metrics.radio.rx_enabled);
        assert_eq!(metrics.sweep.pre_sweep_hz, None);
    }

    #[test]
    fn reactivation_finishes_a_pending_exit_before_restarting() {
        let state = active_state(false);
        let device = TestDevice::default();
        let mut controller = DirectSweepController::default();
        let now = Instant::now();
        controller.update(&state, &device, now);

        *device.frequency_failures.lock().unwrap() = 1;
        state.lock().unwrap().sweep.active = false;
        controller.update(&state, &device, now);
        assert!(controller
            .pending_exit
            .as_ref()
            .is_some_and(|pending| pending.direct_disabled));

        state.lock().unwrap().sweep.active = true;
        controller.update(&state, &device, now + DIRECT_SWEEP_RETRY);
        assert!(controller.pending_exit.is_none());
        assert!(controller.was_active);
        let requests = device.requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        assert!(requests[0].is_some());
        assert_eq!(requests[1], None);
        assert!(requests[2].is_some());
    }
}
