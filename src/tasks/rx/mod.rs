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
        let mut rate = metrics::RateBaseline::default();
        // Last reading of the pull backend's cumulative read-loop clock, so each
        // poll reports this window rather than the whole session.
        let mut last_loop_us: Option<(u64, u64)> = None;

        loop {
            // Single is_streaming() call per iteration - the result is used for
            // both the unexpected-stop check and the hw_streaming state update.
            let hw_streaming = device.is_streaming();
            let now = Instant::now();

            hw_rx_active =
                control::note_unexpected_stop(&state, &device, hw_rx_active, hw_streaming);

            let drained = poll::drain(&state, &rx_ctx, now, hw_streaming);
            rate.push(drained.elapsed_us, drained.bytes);

            let computed = Computed {
                iq: metrics::iq_metrics(
                    drained.moments,
                    drained.cal,
                    device.capabilities().sample_geometry.full_scale as f64,
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

            tokio::time::sleep(POLL_INTERVAL).await;
        }
    });
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
