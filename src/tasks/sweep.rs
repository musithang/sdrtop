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

use crate::hardware::SdrDevice;
use crate::state::{SdrMetrics, SweepFrame, SWEEP_SETTLING_MS};

/// How often the dwell loop samples the shared FFT frame.
const DWELL_POLL_MS: u64 = 10;
/// A frame older than this is treated as stale and skipped.
const FRAME_FRESH_MS: u128 = 200;

pub fn spawn_sweep_task(state: Arc<Mutex<SdrMetrics>>, device: Arc<dyn SdrDevice>) {
    tokio::spawn(async move {
        let mut was_active = false;
        let mut saved_freq: u64 = 0;
        let mut saved_rx_enabled = false;

        loop {
            let (active, config, sample_rate, fmin, fmax) = {
                let m = state.lock().unwrap_or_else(|e| e.into_inner());
                (
                    m.sweep.active,
                    m.sweep.config.clone(),
                    m.radio.config_sample_rate,
                    m.caps.freq_min_hz,
                    m.caps.freq_max_hz,
                )
            };

            // ── Enter sweep: remember normal-RX state, force streaming on.
            if active && !was_active {
                was_active = true;
                let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
                saved_freq = m.radio.frequency;
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

            // ── Leave sweep: restore the tuner and the prior RX state.
            if !active {
                if was_active {
                    was_active = false;
                    // An [Enter] jump lands the radio on the cursor frequency;
                    // otherwise restore the pre-sweep tuning.
                    let target = {
                        let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
                        m.sweep.pending_tune.take().unwrap_or(saved_freq)
                    };
                    let _ = device.set_frequency(target);
                    let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
                    m.radio.frequency = target;
                    // A jump resumes normal RX; a plain stop restores the prior state.
                    m.radio.rx_enabled = if target != saved_freq {
                        true
                    } else {
                        saved_rx_enabled
                    };
                    m.sweep.cursor_frac = None;
                    m.push_log(if target != saved_freq {
                        format!("Tuned to {:.3} MHz from sweep", target as f64 / 1e6)
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

            // ── One full sweep cycle, stitched into per-bin arrays.
            let cycle_start = Instant::now();
            let mut freq_hz: Vec<u64> = Vec::new();
            let mut peak: Vec<f32> = Vec::new();
            let mut mean: Vec<f32> = Vec::new();

            for i in 0..positions {
                if !state.lock().unwrap_or_else(|e| e.into_inner()).sweep.active {
                    break;
                }
                let hz = config.position_hz(i, sample_rate).clamp(fmin, fmax);
                let _ = device.set_frequency(hz);
                {
                    // Tag m.radio.frequency too so the FFT worker stamps frames
                    // with this position's center (it reads radio.frequency).
                    let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
                    m.radio.frequency = hz;
                    m.sweep.current_hz = hz;
                    m.sweep.positions_done = i;
                }
                tokio::time::sleep(Duration::from_millis(SWEEP_SETTLING_MS)).await;

                // Dwell: fold matching, fresh FFT frames into per-bin peak / mean.
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
                        freq_hz.push(f);
                        peak.push(pos_peak[j]);
                        mean.push((pos_mean_sum[j] / frames as f64) as f32);
                    }
                }
            }

            // ── Publish the completed cycle.
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
