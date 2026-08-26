//! **Lock block 1**: drain the accumulators and do the integer work.
//!
//! One critical section, entered once per poll. Everything in here is integer
//! arithmetic, ring-buffer pushes and field copies — cheap operations that the
//! UI thread can afford to wait behind. The `sqrt` / `log10` / `asin` that turn
//! these sums into readings happen in [`metrics`](super::metrics), after the
//! guard is dropped.
//!
//! That division is the whole discipline of this file, and it is easy to undo by
//! accident: a single float computed here holds the mutex through a
//! transcendental while `App::draw` waits to clone the snapshot.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::hardware::RxContext;
use crate::state::{IqCalState, SdrMetrics, THROUGHPUT_HISTORY_LEN};

use super::metrics::Moments;

/// What one poll takes out of the shared state, for the float half to work on.
pub(super) struct Drained {
    pub moments: Moments,
    /// The correction that was live while this window was captured.
    pub cal: IqCalState,
    pub jitter_sum_us: u64,
    pub jitter_sq_sum: u64,
    pub jitter_count: u64,
}

/// Enter the lock, take everything this window produced, reset the accumulators,
/// and update every reading that is a matter of counting.
pub(super) fn drain(
    state: &Arc<Mutex<SdrMetrics>>,
    rx_ctx: &Arc<RxContext>,
    now: Instant,
    hw_streaming: bool,
) -> Drained {
    let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
    let elapsed_ms = now.duration_since(m.radio.last_poll_time).as_millis() as u64;
    let bytes = m.radio.bytes_since_last_poll;
    m.radio.bytes_since_last_poll = 0;
    m.radio.last_poll_time = now;
    m.radio.hw_streaming = hw_streaming;

    if let Some(bps) = (bytes * 1000).checked_div(elapsed_ms) {
        m.radio.current_throughput_bps = bps;
        m.radio.actual_sample_rate = (m.radio.current_throughput_bps / 2) as u32;
        let throughput_kb = m.radio.current_throughput_bps / 1024;
        if m.radio.throughput_history.len() >= THROUGHPUT_HISTORY_LEN {
            m.radio.throughput_history.pop_front();
        }
        m.radio.throughput_history.push_back(throughput_kb);
        let actual_sr = m.radio.actual_sample_rate as u64;
        if m.radio.sample_rate_history.len() >= THROUGHPUT_HISTORY_LEN {
            m.radio.sample_rate_history.pop_front();
        }
        m.radio.sample_rate_history.push_back(actual_sr);
    }
    if let Some(dps) = (m.acc.drops * 1000).checked_div(elapsed_ms) {
        m.signal.drops_per_sec = dps;
    }
    let drops_snapshot = m.signal.drops_per_sec;
    if m.signal.drop_history.len() >= THROUGHPUT_HISTORY_LEN {
        m.signal.drop_history.pop_front();
    }
    m.signal.drop_history.push_back(drops_snapshot);

    let acc_saturated = m.acc.saturated;
    let drained = Drained {
        moments: Moments {
            i_sum: m.acc.i_sum,
            q_sum: m.acc.q_sum,
            i_sq_sum: m.acc.i_sq_sum,
            q_sq_sum: m.acc.q_sq_sum,
            cross_sum: m.acc.iq_cross_sum,
            samples: m.acc.sample_count,
            peak_amp: m.acc.peak_amp,
        },
        cal: m.iq.cal,
        jitter_sum_us: m.acc.jitter_sum_us,
        jitter_sq_sum: m.acc.jitter_sq_sum,
        jitter_count: m.acc.jitter_count,
    };
    m.acc.drops = 0;
    m.acc.saturated = 0;
    m.acc.i_sum = 0;
    m.acc.q_sum = 0;
    m.acc.i_sq_sum = 0;
    m.acc.q_sq_sum = 0;
    m.acc.iq_cross_sum = 0;
    m.acc.sample_count = 0;
    m.acc.jitter_sum_us = 0;
    m.acc.jitter_sq_sum = 0;
    m.acc.jitter_count = 0;

    m.iq.iq_amplitude_hist = m.acc.iq_hist;
    m.acc.iq_hist = [0u64; 32];
    m.iq.adc_signed_hist = m.acc.adc_signed_hist;
    m.acc.adc_signed_hist = [0u64; 32];
    m.acc.peak_amp = 0;
    m.signal.adc_clip_events = acc_saturated;

    let saturable = drained.moments.samples * 2;
    m.signal.adc_saturation_pct = if saturable > 0 {
        (acc_saturated as f32 / saturable as f32) * 100.0
    } else {
        0.0
    };
    if m.signal.adc_saturation_pct > m.signal.adc_saturation_peak {
        m.signal.adc_saturation_peak = m.signal.adc_saturation_pct;
    }
    let sat_snapshot = m.signal.adc_saturation_pct;
    if m.signal.saturation_history.len() >= THROUGHPUT_HISTORY_LEN {
        m.signal.saturation_history.pop_front();
    }
    m.signal.saturation_history.push_back(sat_snapshot);
    // Remember the moment of a real clip so the rail can show a fading
    // "last clip Xs" memory (decays in render; nothing flickers here).
    if sat_snapshot >= crate::state::SAT_CLIP_PCT {
        m.signal.last_clip_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .ok();
    }

    let usb_now = m.signal.usb_errors_session;
    let usb_delta = usb_now.saturating_sub(m.signal.usb_errors_last_poll);
    m.signal.usb_errors_last_poll = usb_now;
    if m.signal.usb_error_history.len() >= THROUGHPUT_HISTORY_LEN {
        m.signal.usb_error_history.pop_front();
    }
    m.signal.usb_error_history.push_back(usb_delta);

    let cap = rx_ctx.sample_tx.capacity().unwrap_or(4);
    m.iq.buf_fill_pct = if cap > 0 {
        rx_ctx.sample_tx.len() as f32 / cap as f32 * 100.0
    } else {
        0.0
    };
    let buf_sample = (m.iq.buf_fill_pct * 10.0) as u64;
    if m.iq.buf_fill_history.len() >= THROUGHPUT_HISTORY_LEN {
        m.iq.buf_fill_history.pop_front();
    }
    m.iq.buf_fill_history.push_back(buf_sample);

    drained
}
