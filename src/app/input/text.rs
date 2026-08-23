//! The text-entry modes: frequency, sample rate, the two sweep-range fields and
//! a marker's label.
//!
//! These are reached through [`InputMode`] rather than through panel focus, and
//! they are the one place a key is **not** case-folded — a marker label is typed,
//! capitals and all.

use std::sync::{Arc, Mutex};

use crossterm::event::{KeyCode, KeyEvent};

use crate::hardware;
use crate::state::{InputMode, RailMode, SdrMetrics, SpectrumMarker};

use super::metrics;

pub(super) fn frequency(
    key: KeyEvent,
    state: &Arc<Mutex<SdrMetrics>>,
    device: Option<&Arc<dyn hardware::SdrDevice>>,
) {
    match key.code {
        KeyCode::Esc => {
            let mut m = metrics(state);
            m.ui.input_mode = InputMode::Normal;
            m.ui.input_buf.clear();
            m.push_log("Frequency input cancelled");
        }
        KeyCode::Backspace => { metrics(state).ui.input_buf.pop(); }
        KeyCode::Char(c) if c.is_ascii_digit() || c == '.' => {
            metrics(state).ui.input_buf.push(c);
        }
        KeyCode::Enter => {
            if let Some(device) = device {
                let caps = device.capabilities();
                // Clamp into the tuning range rather than rejecting (matches the
                // arrow-key tuning, which already clamps).
                let freq_hz: Option<u64> = {
                    let m = metrics(state);
                    m.ui.input_buf.parse::<f64>().ok()
                        .filter(|&mhz| mhz > 0.0)
                        .map(|mhz| ((mhz * 1_000_000.0) as u64).clamp(caps.freq_min_hz, caps.freq_max_hz))
                };
                let result = freq_hz.map(|hz| device.set_frequency(hz));
                let mut m = metrics(state);
                match (freq_hz, result) {
                    (Some(hz), Some(Ok(()))) => {
                        m.radio.frequency = hz;
                        m.ui.note_mode_action(RailMode::Hunt);
                        m.ui.input_mode = InputMode::Normal;
                        m.ui.input_buf.clear();
                        m.push_log(format!("Frequency set to {:.3} MHz", hz as f64 / 1_000_000.0));
                    }
                    (Some(_), Some(Err(e))) => m.push_log(format!("Frequency error: {}", e)),
                    _ => {
                        let bad = m.ui.input_buf.clone();
                        m.push_log(format!("Invalid frequency: '{}' ({:.0}–{:.0} MHz)",
                            bad, caps.freq_min_hz as f64 / 1e6, caps.freq_max_hz as f64 / 1e6));
                    }
                }
            }
        }
        _ => {}
    }
}

pub(super) fn sample_rate(
    key: KeyEvent,
    state: &Arc<Mutex<SdrMetrics>>,
    device: Option<&Arc<dyn hardware::SdrDevice>>,
) {
    match key.code {
        KeyCode::Esc => {
            let mut m = metrics(state);
            m.ui.input_mode = InputMode::Normal;
            m.ui.input_buf.clear();
            m.push_log("Sample rate input cancelled");
        }
        KeyCode::Backspace => { metrics(state).ui.input_buf.pop(); }
        KeyCode::Char(c) if c.is_ascii_digit() || c == '.' => {
            metrics(state).ui.input_buf.push(c);
        }
        KeyCode::Enter => {
            if let Some(device) = device {
                let caps = device.capabilities();
                let lo_hz = caps.sample_rate_min_hz;
                let hi_hz = caps.sample_rate_max_hz;
                // Clamp into the device's legal range rather than rejecting, so a
                // boundary entry like "0.9" on RTL-SDR snaps up to a valid rate.
                let rate_hz: Option<f64> = {
                    let m = metrics(state);
                    m.ui.input_buf.parse::<f64>().ok()
                        .filter(|&mhz| mhz > 0.0)
                        .map(|mhz| (mhz * 1_000_000.0).clamp(lo_hz, hi_hz))
                };
                // Release lock before calling device — set_sample_rate is a
                // blocking USB control transfer; holding the mutex here deadlocks the
                // rx_callback thread that needs the same lock to return.
                let result = rate_hz.map(|hz| device.set_sample_rate(hz));
                let mut m = metrics(state);
                match (rate_hz, result) {
                    (Some(hz), Some(Ok(bw))) => {
                        m.radio.config_sample_rate = hz;
                        m.radio.bb_filter_hz = bw;
                        m.ui.input_mode = InputMode::Normal;
                        m.ui.input_buf.clear();
                        m.push_log(format!("Sample rate set to {:.1} MHz", hz / 1_000_000.0));
                    }
                    (Some(_), Some(Err(e))) => m.push_log(format!("Sample rate error: {}", e)),
                    _ => {
                        let bad = m.ui.input_buf.clone();
                        m.push_log(format!("Invalid sample rate: '{}' (valid: {:.1}–{:.1} MHz)",
                            bad, lo_hz / 1e6, hi_hz / 1e6));
                    }
                }
            }
        }
        _ => {}
    }
}

/// Sweep START / STOP frequency entry (MHz), reached from the sweep panel's
/// `[` / `]` focus keys. Validates the new bound against the other one and the
/// HackRF tuning range before committing, and clears the stale frame so the next
/// cycle rebuilds over the new band.
pub(super) fn sweep_range(key: KeyEvent, state: &Arc<Mutex<SdrMetrics>>, is_start: bool) {
    match key.code {
        KeyCode::Esc => {
            let mut m = metrics(state);
            m.ui.input_mode = InputMode::Normal;
            m.ui.input_buf.clear();
            m.push_log("Sweep range input cancelled");
        }
        KeyCode::Backspace => { metrics(state).ui.input_buf.pop(); }
        KeyCode::Char(c) if c.is_ascii_digit() || c == '.' => {
            metrics(state).ui.input_buf.push(c);
        }
        KeyCode::Enter => {
            let mut m = metrics(state);
            let fmin = m.caps.freq_min_hz;
            let fmax = m.caps.freq_max_hz;
            let parsed = m.ui.input_buf.parse::<f64>().ok()
                .filter(|&mhz| mhz > 0.0)
                .map(|mhz| (mhz * 1_000_000.0) as u64)
                .filter(|&hz| (fmin..=fmax).contains(&hz));
            match parsed {
                Some(hz) => {
                    let (start, stop) = (m.sweep.config.start_hz, m.sweep.config.stop_hz);
                    let ordered = if is_start { hz < stop } else { hz > start };
                    if ordered {
                        if is_start { m.sweep.config.start_hz = hz; } else { m.sweep.config.stop_hz = hz; }
                        m.sweep.cycle_count = 0;
                        m.sweep.positions_done = 0;
                        m.sweep.current_frame = None;
                        m.sweep.cursor_frac = None;
                        m.ui.input_mode = InputMode::Normal;
                        m.ui.input_buf.clear();
                        m.push_log(format!(
                            "Sweep {} → {:.3} MHz",
                            if is_start { "START" } else { "STOP" }, hz as f64 / 1e6
                        ));
                    } else {
                        m.push_log(format!(
                            "Invalid: START must be below STOP (now {:.1}–{:.1} MHz)",
                            start as f64 / 1e6, stop as f64 / 1e6
                        ));
                    }
                }
                None => {
                    let bad = m.ui.input_buf.clone();
                    m.push_log(format!("Invalid frequency: '{}' ({:.0}–{:.0} MHz)",
                        bad, fmin as f64 / 1e6, fmax as f64 / 1e6));
                }
            }
        }
        _ => {}
    }
}

pub(super) fn marker_name(key: KeyEvent, state: &Arc<Mutex<SdrMetrics>>) {
    match key.code {
        KeyCode::Esc => {
            let mut m = metrics(state);
            m.ui.input_mode = InputMode::Normal;
            m.ui.input_buf.clear();
            m.spectrum.pending_marker = None;
            m.push_log("Marker cancelled");
        }
        KeyCode::Backspace => { metrics(state).ui.input_buf.pop(); }
        KeyCode::Char(c) => { metrics(state).ui.input_buf.push(c); }
        KeyCode::Enter => {
            let mut m = metrics(state);
            if let Some(freq) = m.spectrum.pending_marker.take() {
                let label = if m.ui.input_buf.trim().is_empty() {
                    format!("M{}", m.spectrum.markers.len() + 1)
                } else {
                    m.ui.input_buf.trim().to_string()
                };
                m.push_log(format!("Marker: {} → {:.3} MHz", label, freq as f64 / 1_000_000.0));
                m.spectrum.markers.push(SpectrumMarker { freq_hz: freq, label, channel_bw_hz: None, measured_bw_hz: None });
            }
            m.ui.input_mode = InputMode::Normal;
            m.ui.input_buf.clear();
        }
        _ => {}
    }
}
