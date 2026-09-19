// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The RX poll: one 200 ms loop that is the only writer of most of `SdrMetrics`.
//!
//! The loop is split by **when it holds the lock**, because that is the property
//! the hot path is built around and the one an edit is most likely to break:
//!
//! ```text
//! control::note_unexpected_stop   device query, then a short lock
//! poll::drain                     ── LOCK ──  integer work only
//! metrics::iq_metrics             ── no lock ──  sqrt / log10 / asin
//! publish::write_back             ── LOCK ──  write results, read rx_enabled
//! control::apply_rx_request       device call with no lock held
//! control::track_gain             lock, drop, device call, lock
//! control::advance_noise_sweep     lock, drop, device call, lock
//! ```
//!
//! Two lock blocks per poll, with every transcendental between them. The UI
//! thread clones the whole of `SdrMetrics` under this same mutex on every frame,
//! so a float computed inside a lock block is a dropped frame; a device call
//! inside one is a visible stall.
//!
//! - [`poll`]: lock block 1, the accumulator drain.
//! - [`metrics`]: the pure maths in between. No lock, no device, no clock.
//! - [`publish`]: lock block 2, writing the results back.
//! - [`control`]: everything that talks to the radio.

mod control;
mod metrics;
mod poll;
mod publish;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::hardware::{RxContext, SdrDevice};
use crate::state::SdrMetrics;

use publish::{Computed, Throughput};

/// How often the poll runs. Everything derived here is a rate over this window.
const POLL_INTERVAL: Duration = Duration::from_millis(200);

/// Poll the device every 200 ms:
///   - start / stop RX in response to `state.rx_enabled`
///   - compute throughput, drop rate, ADC saturation, IQ metrics, jitter
///   - write results back to `state`
pub fn spawn_rx_task(
    state: Arc<Mutex<SdrMetrics>>,
    device: Arc<dyn SdrDevice>,
    rx_ctx: Arc<RxContext>,
) {
    tokio::spawn(async move {
        let mut hw_rx_active = false;
        // Throttles SNR history sampling to ~500 ms regardless of the 200 ms poll.
        let mut last_snr_push = Instant::now();
        let mut throughput = Throughput::default();
        // Task local: the sample-rate baseline is neither drawn nor shared, so it
        // stays out of the per-frame clone of SdrMetrics.
        let mut rate = metrics::RateTracker::default();
        // Last reading of the pull backend's cumulative read-loop clock, so each
        // poll reports this window rather than the whole session.
        let mut last_loop_us: Option<(u64, u64)> = None;
        // The rate the baselines below are averaging over. A change makes every
        // sample they hold describe a different stream.
        //
        // Seeded NaN, which compares unequal to everything including itself, so
        // the first poll always clears. They are empty then, so it costs
        // nothing, and it means there is no separate first-time branch to get
        // wrong.
        let mut baseline_rate = f64::NAN;

        loop {
            // Single is_streaming() call per iteration - the result is used for
            // both the unexpected-stop check and the hw_streaming state update.
            let hw_streaming = device.is_streaming();
            let now = Instant::now();

            hw_rx_active =
                control::note_unexpected_stop(&state, &device, hw_rx_active, hw_streaming);

            let drained = poll::drain(&state, &rx_ctx, now, hw_streaming);
            // `[S]` retunes the rate mid-stream. Averaging across that would
            // report the blend of two rates as an offset from one of them, for
            // the whole length of the baseline.
            if drained.config_sample_rate != baseline_rate {
                baseline_rate = drained.config_sample_rate;
                rate.reset();
                throughput.reset();
            }
            rate.push(drained.last_block_at, drained.bytes);

            let computed = Computed {
                iq: metrics::iq_metrics(
                    drained.moments,
                    drained.cal,
                    device.capabilities().sample_geometry,
                ),
                had_samples: drained.moments.samples > 0,
                callback: metrics::callback_timing(
                    drained.jitter_sum_us,
                    drained.jitter_sq_sum,
                    drained.jitter_count,
                ),
                measured_rate: rate.rate(device.capabilities().sample_geometry.bytes_per_pair()),
                read_occupancy: window_occupancy(device.read_loop_us(), &mut last_loop_us),
            };

            let rx_enabled = publish::write_back(
                &state,
                &device,
                &computed,
                &mut throughput,
                now,
                hw_streaming,
                &mut last_snr_push,
            );

            hw_rx_active = control::apply_rx_request(
                &state,
                &device,
                &rx_ctx,
                &mut throughput,
                &mut rate,
                rx_enabled,
                hw_rx_active,
            );

            if hw_streaming && hw_rx_active && computed.had_samples {
                control::track_gain(&state, &device, computed.iq.adc_peak_dbfs);
            }
            // **Outside the streaming gate, deliberately.** Auto-track only has
            // something to do while blocks are arriving, but the sweep's most
            // important job is the one it does when they stop: a sweep parks the
            // front stage at each setting in turn, so a run interrupted by [Space]
            // would leave the radio at some intermediate step with nothing left
            // running to put it back.
            control::advance_noise_sweep(&state, &device);

            tokio::time::sleep(POLL_INTERVAL).await;
        }
    });
}

/// Control a backend that publishes complete power-spectrum traces.
pub fn spawn_power_rx_task(
    state: Arc<Mutex<SdrMetrics>>,
    device: Arc<dyn SdrDevice>,
    rx_ctx: Arc<RxContext>,
) {
    tokio::spawn(async move {
        let mut active = false;
        loop {
            active = power_control_step(&state, &device, &rx_ctx, active);
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    });
}

fn power_control_step(
    state: &Arc<Mutex<SdrMetrics>>,
    device: &Arc<dyn SdrDevice>,
    rx_ctx: &Arc<RxContext>,
    active: bool,
) -> bool {
    let requested = state
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .radio
        .rx_enabled;
    let transition = control::request_transition(
        requested,
        active,
        || {
            // A new stream counts its positions from zero, so its first block
            // is never mistaken for the continuation of the last stream's.
            rx_ctx.begin_stream();
            device.start_rx(Arc::clone(rx_ctx))
        },
        || device.stop_rx(),
    );
    let unchanged_active = matches!(transition, control::RxRequestTransition::Unchanged(true));
    let active = match transition {
        control::RxRequestTransition::Started => {
            let mut metrics = state.lock().unwrap_or_else(|error| error.into_inner());
            metrics.radio.rx_start_time = Some(Instant::now());
            metrics.radio.hw_streaming = true;
            metrics.push_log("Power trace acquisition started");
            true
        }
        control::RxRequestTransition::StartFailed(error) => {
            let mut metrics = state.lock().unwrap_or_else(|e| e.into_inner());
            metrics.radio.rx_enabled = false;
            metrics.radio.hw_streaming = false;
            metrics.push_log(format!("Error starting power trace acquisition: {error}"));
            false
        }
        control::RxRequestTransition::Stopped(result) => {
            let mut metrics = state.lock().unwrap_or_else(|error| error.into_inner());
            metrics.radio.rx_start_time = None;
            metrics.radio.hw_streaming = false;
            match result {
                Ok(()) => metrics.push_log("Power trace acquisition stopped"),
                Err(error) => {
                    metrics.push_log(format!("Error stopping power trace acquisition: {error}"))
                }
            }
            false
        }
        control::RxRequestTransition::Unchanged(active) => active,
    };

    if unchanged_active {
        let streaming = device.is_streaming();
        if let Some(cleanup) = control::unexpected_stop(active, streaming, || device.stop_rx()) {
            let mut metrics = state.lock().unwrap_or_else(|error| error.into_inner());
            metrics.radio.rx_enabled = false;
            metrics.radio.hw_streaming = false;
            metrics.radio.rx_start_time = None;
            metrics.push_log(
                "WARNING: Power trace acquisition stopped unexpectedly \u{2014} press [Space] to restart",
            );
            if let Err(error) = cleanup {
                metrics.push_log(format!(
                    "Error cleaning up power trace acquisition: {error}"
                ));
            }
            return false;
        }
        state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .radio
            .hw_streaming = streaming;
    }
    active
}

/// Turn the read loop's cumulative clock into this window's occupancy.
///
/// The counters are cumulative and never reset, so the first poll of a session
/// has nothing to subtract from and reports `None` rather than the whole
/// session's average. `saturating_sub` guards the case a push backend or a
/// restart could otherwise turn into a wrapped subtraction.
fn window_occupancy(current: Option<(u64, u64)>, last: &mut Option<(u64, u64)>) -> Option<f32> {
    let (wait, work) = current?;
    let previous = last.replace((wait, work));
    let (prev_wait, prev_work) = previous?;
    metrics::occupancy(
        wait.saturating_sub(prev_wait),
        work.saturating_sub(prev_work),
    )
}

#[cfg(test)]
mod power_control_tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use super::*;
    use crate::hardware::{DeviceCapabilities, DeviceInfo, FeedHealth, RateSet, SampleGeometry};

    struct TestDevice {
        caps: DeviceCapabilities,
        streaming: AtomicBool,
        fail_start: AtomicBool,
        fail_stop: AtomicBool,
        starts: AtomicUsize,
        stops: AtomicUsize,
    }

    impl TestDevice {
        fn new() -> Self {
            let mut caps = crate::hardware::native::hackrf::caps();
            caps.acquisition = crate::hardware::AcquisitionKind::PowerTrace;
            Self {
                caps,
                streaming: AtomicBool::new(false),
                fail_start: AtomicBool::new(false),
                fail_stop: AtomicBool::new(false),
                starts: AtomicUsize::new(0),
                stops: AtomicUsize::new(0),
            }
        }
    }

    impl SdrDevice for TestDevice {
        fn capabilities(&self) -> &DeviceCapabilities {
            &self.caps
        }

        fn info(&self) -> DeviceInfo {
            DeviceInfo::default()
        }

        fn start_rx(&self, _ctx: Arc<RxContext>) -> anyhow::Result<()> {
            self.starts.fetch_add(1, Ordering::Relaxed);
            if self.fail_start.load(Ordering::Relaxed) {
                anyhow::bail!("start failed")
            }
            self.streaming.store(true, Ordering::Relaxed);
            Ok(())
        }

        fn stop_rx(&self) -> anyhow::Result<()> {
            self.stops.fetch_add(1, Ordering::Relaxed);
            self.streaming.store(false, Ordering::Relaxed);
            if self.fail_stop.load(Ordering::Relaxed) {
                anyhow::bail!("stop failed")
            }
            Ok(())
        }

        fn is_streaming(&self) -> bool {
            self.streaming.load(Ordering::Relaxed)
        }

        fn set_frequency(&self, _hz: u64) -> anyhow::Result<()> {
            Ok(())
        }

        fn set_sample_rate(&self, hz: f64) -> anyhow::Result<RateSet> {
            Ok(RateSet::new(hz, Some(hz), 0))
        }

        fn set_lna_gain(&self, _db: u32) -> anyhow::Result<()> {
            Ok(())
        }
    }

    fn context(state: &Arc<Mutex<SdrMetrics>>) -> Arc<RxContext> {
        let (sample_tx, _) = crossbeam_channel::bounded(1);
        let (demod_tx, _) = crossbeam_channel::bounded(1);
        let (net_tx, _) = crossbeam_channel::bounded(1);
        let (power_tx, _) = crossbeam_channel::bounded(1);
        Arc::new(RxContext {
            metrics: Arc::clone(state),
            sample_tx,
            fft_feed: FeedHealth::default(),
            demod_tx,
            net_tx,
            net_feed: FeedHealth::default(),
            power_tx,
            geometry: SampleGeometry::default(),
            stream_pairs: std::sync::atomic::AtomicU64::new(0),
        })
    }

    #[test]
    fn a_power_start_failure_clears_the_request() {
        let state = Arc::new(Mutex::new(SdrMetrics::fixture()));
        state.lock().unwrap().radio.rx_enabled = true;
        let device = Arc::new(TestDevice::new());
        device.fail_start.store(true, Ordering::Relaxed);
        let dyn_device: Arc<dyn SdrDevice> = device;

        assert!(!power_control_step(
            &state,
            &dyn_device,
            &context(&state),
            false
        ));
        let metrics = state.lock().unwrap();
        assert!(!metrics.radio.rx_enabled);
        assert!(!metrics.radio.hw_streaming);
        assert!(metrics.ui.log.back().unwrap().text.contains("start failed"));
    }

    #[test]
    fn a_requested_power_stop_closes_the_active_session() {
        let state = Arc::new(Mutex::new(SdrMetrics::fixture()));
        state.lock().unwrap().radio.rx_enabled = true;
        let device = Arc::new(TestDevice::new());
        let dyn_device: Arc<dyn SdrDevice> = device.clone();
        let ctx = context(&state);
        let active = power_control_step(&state, &dyn_device, &ctx, false);
        assert!(active);

        state.lock().unwrap().radio.rx_enabled = false;
        assert!(!power_control_step(&state, &dyn_device, &ctx, active));
        assert_eq!(device.stops.load(Ordering::Relaxed), 1);
        assert!(!state.lock().unwrap().radio.hw_streaming);
    }

    #[test]
    fn an_unexpected_power_stop_reports_cleanup_failure() {
        let state = Arc::new(Mutex::new(SdrMetrics::fixture()));
        {
            let mut metrics = state.lock().unwrap();
            metrics.radio.rx_enabled = true;
            metrics.radio.hw_streaming = true;
        }
        let device = Arc::new(TestDevice::new());
        device.fail_stop.store(true, Ordering::Relaxed);
        let dyn_device: Arc<dyn SdrDevice> = device.clone();

        assert!(!power_control_step(
            &state,
            &dyn_device,
            &context(&state),
            true
        ));
        let metrics = state.lock().unwrap();
        assert!(!metrics.radio.rx_enabled);
        assert!(!metrics.radio.hw_streaming);
        assert_eq!(device.stops.load(Ordering::Relaxed), 1);
        assert!(metrics.ui.log.iter().any(|entry| {
            entry.text.contains("stopped unexpectedly")
                && entry.text.contains("press [Space] to restart")
        }));
        assert!(metrics
            .ui
            .log
            .iter()
            .any(|entry| entry.text.contains("cleaning up") && entry.text.contains("stop failed")));
    }
}
