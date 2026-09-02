// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! **Lock block 2**: write the computed measurements back, and read `rx_enabled`
//! in the same critical section.
//!
//! The read rides along deliberately. Taking the mutex again just to ask whether
//! the user pressed `[Space]` would be a third lock per poll for one `bool`, and
//! the answer would be no fresher for it.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::hardware::SdrDevice;
use crate::state::{SdrMetrics, SNR_HISTORY_LEN, THROUGHPUT_HISTORY_LEN};

use super::metrics::IqMetrics;

/// The per-session throughput statistics, accumulated online.
///
/// Welford rather than a running sum of squares: the mean of a few hundred
/// samples around 20 MB/s is large enough that the naive form loses precision in
/// the variance. Reset on each RX start, so the timing panel reports the current
/// session and not the whole uptime.
#[derive(Default)]
pub(super) struct Throughput {
    count: u64,
    mean: f64,
    m2: f64,
}

impl Throughput {
    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }

    fn push(&mut self, mbps: f64) {
        self.count += 1;
        let delta = mbps - self.mean;
        self.mean += delta / self.count as f64;
        self.m2 += delta * (mbps - self.mean);
    }

    /// `(mean, standard deviation)`. The deviation is zero until there are two
    /// samples to have one.
    ///
    /// This is the one `sqrt` in the file that runs with the lock held, and it
    /// was in the original too. It stays because moving it out would mean
    /// splitting the lock block: the sample it folds in is read from the guard,
    /// and the result is consumed by `TimingState::compute` under the same guard.
    /// One square root of two scalars is not what stalls a frame; the per-sample
    /// maths in `metrics` is, and that is outside.
    fn stats(&self) -> (f64, f64) {
        let std = if self.count > 1 {
            (self.m2 / (self.count - 1) as f64).sqrt()
        } else {
            0.0
        };
        (self.mean, std)
    }
}

/// Everything the poll computed while the lock was down.
pub(super) struct Computed {
    pub iq: IqMetrics,
    /// Whether the window had any samples at all; `IqMetrics::idle` is a valid
    /// reading of "nothing", and must not be written over live numbers.
    pub had_samples: bool,
    /// Mean callback period and jitter, when a callback landed in the window.
    pub callback: Option<(u64, u64)>,
    /// Delivered sample rate over the long baseline, or `None` while that
    /// baseline is still too short to mean anything.
    pub measured_rate: Option<u32>,
}

/// Write the results, sample the trend histories, rebuild the timing snapshot,
/// and return whether the user wants RX running.
pub(super) fn write_back(
    state: &Arc<Mutex<SdrMetrics>>,
    device: &Arc<dyn SdrDevice>,
    c: &Computed,
    tp: &mut Throughput,
    now: Instant,
    hw_streaming: bool,
    last_snr_push: &mut Instant,
) -> bool {
    let mut m = state.lock().unwrap_or_else(|e| e.into_inner());

    // The delivered sample rate, measured over tens of seconds rather than over
    // this one window. `None` means the baseline is not long enough yet, and the
    // last good figure is kept rather than replaced by a worse one.
    if let Some(rate) = c.measured_rate {
        m.radio.actual_sample_rate = rate;
        if m.radio.sample_rate_history.len() >= crate::state::THROUGHPUT_HISTORY_LEN {
            m.radio.sample_rate_history.pop_front();
        }
        m.radio.sample_rate_history.push_back(rate as u64);
    }

    if c.had_samples {
        let iq = &c.iq;
        m.iq.dc_offset_i = iq.dc_i;
        m.iq.dc_offset_q = iq.dc_q;
        m.signal.adc_peak_dbfs = iq.adc_peak_dbfs;
        m.signal.adc_rms_dbfs = iq.adc_rms_dbfs;
        if let Some(v) = iq.iq_imbalance_db {
            m.iq.iq_imbalance_db = v;
        }
        if let Some(v) = iq.phase_imbalance {
            m.iq.phase_imbalance_deg = v;
        }

        // DC-block tracks the live DC estimate so it follows slow drift.
        if m.iq.cal.dc_block_on || m.iq.cal.cal_applied {
            m.iq.cal.dc_i_raw = iq.dc_i_raw;
            m.iq.cal.dc_q_raw = iq.dc_q_raw;
        }
        // [C] pressed → capture the correction matrix from this window
        // (a one-shot snapshot; it stays fixed until the next auto-cal).
        if m.iq.cal.cal_pending {
            m.iq.cal.c_qi = iq.iq_corr.0;
            m.iq.cal.c_qq = iq.iq_corr.1;
            m.iq.cal.dc_i_raw = iq.dc_i_raw;
            m.iq.cal.dc_q_raw = iq.dc_q_raw;
            m.iq.cal.cal_applied = true;
            m.iq.cal.cal_pending = false;
            m.iq.cal.last_cal_at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .ok();
            let irr =
                crate::signal::image_rejection_db(m.iq.iq_imbalance_db, m.iq.phase_imbalance_deg);
            m.push_log(format!(
                "IQ auto-cal applied \u{2014} quadrature corrected (was IRR {irr:.1} dB)"
            ));
        }
    }

    if let Some((period, jitter)) = c.callback {
        m.iq.cb_period_us = period;
        m.iq.cb_jitter_us = jitter;
        if m.iq.jitter_history.len() >= THROUGHPUT_HISTORY_LEN {
            m.iq.jitter_history.pop_front();
        }
        m.iq.jitter_history.push_back(jitter);
    }

    // Sample SNR / PWR / NF / SAT into their trend histories at ~500 ms while
    // streaming - one cadence and depth so the command rail's four SIGNAL traces
    // fill and align together.
    if hw_streaming && now.duration_since(*last_snr_push) >= Duration::from_millis(500) {
        *last_snr_push = now;
        push_trends(&mut m);
    }

    // Timing accuracy: fold this window's throughput into the running Welford
    // accumulator, then rebuild the TimingState snapshot from the latest jitter /
    // sample-rate / drop measurements.
    if hw_streaming {
        tp.push(m.radio.current_throughput_bps as f64 / 1024.0 / 1024.0);
    }
    let (tp_mean, tp_std) = tp.stats();
    let jitter_snapshot: Vec<u64> = m.iq.jitter_history.iter().copied().collect();
    // Snapshot the rolling per-callback gap ring for the strip chart; deviation /
    // late-count / percentiles are derived in `compute`.
    let gaps_snapshot: Vec<u64> = m.acc.cb_gaps_us.iter().copied().collect();
    // Carry the session jitter peak across the wholesale rebuild - it is reset on
    // RX start and by the timing panel's [R] focus binding.
    let prev_peak = m.timing.jitter_session_max_us;
    m.timing = crate::state::TimingState::compute(
        m.iq.cb_period_us,
        m.radio.config_sample_rate,
        device.capabilities().samples_per_transfer,
        &jitter_snapshot,
        &gaps_snapshot,
        m.iq.cb_jitter_us,
        m.radio.actual_sample_rate,
        m.signal.drops_per_sec,
        tp_mean,
        tp_std,
    );
    m.timing.jitter_session_max_us = prev_peak.max(m.timing.jitter_max_us);

    m.radio.rx_enabled
}

/// Push one sample onto each of the five signal trend rings.
///
/// Split out because the borrow dance is the whole content: every value has to be
/// read out of `m.signal` before the first `&mut` history borrow, or the
/// immutable reads and the mutable pushes overlap.
fn push_trends(m: &mut SdrMetrics) {
    let push = |h: &mut std::collections::VecDeque<f32>, v: f32| {
        if h.len() >= SNR_HISTORY_LEN {
            h.pop_front();
        }
        h.push_back(v);
    };
    let snr = m.signal.peak_to_nf_db;
    let pwr = m.signal.channel_power_dbfs;
    let sat = m.signal.adc_saturation_pct;
    let nf = m.waterfall.last_fft.as_ref().map(|f| f.noise_floor);
    // IRR from the freshly-written imbalance, via the shared helper the Lab IQ
    // panel also uses (so trend and read-out agree).
    let irr =
        crate::signal::image_rejection_db(m.iq.iq_imbalance_db, m.iq.phase_imbalance_deg) as f32;
    push(&mut m.signal.snr_history, snr);
    if pwr.is_finite() {
        push(&mut m.signal.pwr_history, pwr);
    }
    if let Some(nf) = nf {
        push(&mut m.signal.nf_history, nf);
    }
    push(&mut m.signal.sat_history, sat);
    push(&mut m.iq.irr_history, irr);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn throughput_reports_no_spread_until_it_has_two_samples() {
        let mut tp = Throughput::default();
        assert_eq!(tp.stats(), (0.0, 0.0), "nothing measured yet");
        tp.push(20.0);
        let (mean, std) = tp.stats();
        assert!((mean - 20.0).abs() < 1e-9);
        assert_eq!(std, 0.0, "one sample has a mean but no deviation");
    }

    #[test]
    fn throughput_is_a_running_mean_and_sample_deviation() {
        let mut tp = Throughput::default();
        for v in [18.0, 20.0, 22.0] {
            tp.push(v);
        }
        let (mean, std) = tp.stats();
        assert!((mean - 20.0).abs() < 1e-9, "got {mean}");
        // Sample standard deviation of 18/20/22 is exactly 2.
        assert!((std - 2.0).abs() < 1e-9, "got {std}");
    }

    #[test]
    fn a_reset_starts_the_session_over() {
        // RX start resets it, so the timing panel reports this session and not
        // whatever the last one was doing.
        let mut tp = Throughput::default();
        for v in [1.0, 100.0] {
            tp.push(v);
        }
        tp.reset();
        assert_eq!(tp.stats(), (0.0, 0.0));
        tp.push(20.0);
        assert!(
            (tp.stats().0 - 20.0).abs() < 1e-9,
            "the old samples must be gone"
        );
    }
}
