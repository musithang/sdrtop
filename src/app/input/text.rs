// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The text-entry modes include device options, frequency, sample rate, sweep
//! bounds, and marker labels.
//!
//! These are reached through [`InputMode`] rather than through panel focus, and
//! they are the one place a key is **not** case-folded - a marker label is typed,
//! capitals and all.

use std::sync::{Arc, Mutex};

use crossterm::event::{KeyCode, KeyEvent};

use crate::hardware;
use crate::state::{InputMode, RailMode, SdrMetrics, SpectrumMarker};

use super::{menu, metrics, KeyAction};

pub(super) fn device_option(key: KeyEvent, state: &Arc<Mutex<SdrMetrics>>, id: &str) -> KeyAction {
    let mut m = metrics(state);
    if m.ui.device_option_update.is_pending() {
        return KeyAction::Continue;
    }
    match key.code {
        KeyCode::Esc => {
            m.ui.input_mode = InputMode::Normal;
            m.ui.input_buf.clear();
            m.push_log("Device option input cancelled");
        }
        KeyCode::Backspace => {
            m.ui.input_buf.pop();
            clear_option_error(&mut m);
        }
        KeyCode::Char(c) if c.is_ascii_digit() || (c == '-' && m.ui.input_buf.is_empty()) => {
            m.ui.input_buf.push(c);
            clear_option_error(&mut m);
        }
        KeyCode::Enter => {
            let result = m
                .device_options
                .iter()
                .find(|option| option.id == id)
                .ok_or_else(|| "Option is no longer available".to_string())
                .and_then(|option| {
                    let choice = option
                        .integer_choice(&m.ui.input_buf)
                        .map_err(|error| error.to_string())?;
                    Ok((
                        crate::event::DeviceOptionRequest {
                            id: option.id.clone(),
                            label: option.label.clone(),
                            choice: choice.to_string(),
                        },
                        choice == option.selected_choice,
                    ))
                });
            match result {
                Ok((request, unchanged)) => {
                    m.ui.input_mode = InputMode::Normal;
                    m.ui.input_buf.clear();
                    if !unchanged {
                        return menu::submit_option_request(&mut m, request);
                    }
                }
                Err(message) => {
                    if let InputMode::DeviceOptionInput { error, .. } = &mut m.ui.input_mode {
                        *error = Some(message);
                    }
                }
            }
        }
        _ => {}
    }
    KeyAction::Continue
}

fn clear_option_error(m: &mut SdrMetrics) {
    if let InputMode::DeviceOptionInput { error, .. } = &mut m.ui.input_mode {
        *error = None;
    }
}

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
        KeyCode::Backspace => {
            metrics(state).ui.input_buf.pop();
        }
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
                    m.ui.input_buf
                        .parse::<f64>()
                        .ok()
                        .filter(|&mhz| mhz > 0.0)
                        .map(|mhz| {
                            ((mhz * 1_000_000.0) as u64).clamp(caps.freq_min_hz, caps.freq_max_hz)
                        })
                };
                let result = freq_hz.map(|hz| device.set_frequency(hz));
                let mut m = metrics(state);
                match (freq_hz, result) {
                    (Some(hz), Some(Ok(()))) => {
                        m.radio.frequency = hz;
                        m.ui.note_mode_action(RailMode::Hunt);
                        m.ui.input_mode = InputMode::Normal;
                        m.ui.input_buf.clear();
                        m.push_log(format!(
                            "Frequency set to {:.3} MHz",
                            hz as f64 / 1_000_000.0
                        ));
                    }
                    (Some(_), Some(Err(e))) => m.push_log(format!("Frequency error: {}", e)),
                    _ => {
                        let bad = m.ui.input_buf.clone();
                        m.push_log(format!(
                            "Invalid frequency: '{}' ({:.0}–{:.0} MHz)",
                            bad,
                            caps.freq_min_hz as f64 / 1e6,
                            caps.freq_max_hz as f64 / 1e6
                        ));
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
            let name = if m.caps.sample_rate_is_span {
                "Span"
            } else {
                "Sample rate"
            };
            m.push_log(format!("{name} input cancelled"));
        }
        KeyCode::Backspace => {
            metrics(state).ui.input_buf.pop();
        }
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
                    m.ui.input_buf
                        .parse::<f64>()
                        .ok()
                        .filter(|&mhz| mhz > 0.0)
                        .map(|mhz| (mhz * 1_000_000.0).clamp(lo_hz, hi_hz))
                };
                // Release lock before calling device - set_sample_rate is a
                // blocking USB control transfer; holding the mutex here deadlocks the
                // rx_callback thread that needs the same lock to return.
                let result = rate_hz.map(|hz| device.set_sample_rate(hz));
                let mut m = metrics(state);
                let (name, lower_name) = if m.caps.sample_rate_is_span {
                    ("Span", "span")
                } else {
                    ("Sample rate", "sample rate")
                };
                match (rate_hz, result) {
                    // The rate recorded is the one the device came back with,
                    // not the one that was typed: a driver is free to round onto
                    // its own grid, and every frequency on screen is derived from
                    // this figure.
                    (Some(hz), Some(Ok(set))) => {
                        m.radio.config_sample_rate = set.rate_hz;
                        m.radio.bb_filter_hz = set.bb_filter_hz;
                        m.ui.input_mode = InputMode::Normal;
                        m.ui.input_buf.clear();
                        m.push_log(if set.rate_hz == hz {
                            format!("{name} set to {:.3} MHz", hz / 1e6)
                        } else {
                            format!(
                                "{name} {:.3} MHz is not on this device's grid; running at \
                                 {:.3} MHz",
                                hz / 1e6,
                                set.rate_hz / 1e6
                            )
                        });
                    }
                    (Some(_), Some(Err(e))) => m.push_log(format!("{name} error: {e}")),
                    _ => {
                        let bad = m.ui.input_buf.clone();
                        m.push_log(format!(
                            "Invalid {lower_name}: '{}' (valid: {:.1}–{:.1} MHz)",
                            bad,
                            lo_hz / 1e6,
                            hi_hz / 1e6
                        ));
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
        KeyCode::Backspace => {
            metrics(state).ui.input_buf.pop();
        }
        KeyCode::Char(c) if c.is_ascii_digit() || c == '.' => {
            metrics(state).ui.input_buf.push(c);
        }
        KeyCode::Enter => {
            let mut m = metrics(state);
            let fmin = m.caps.freq_min_hz;
            let fmax = m.caps.freq_max_hz;
            let parsed =
                m.ui.input_buf
                    .parse::<f64>()
                    .ok()
                    .filter(|&mhz| mhz > 0.0)
                    .map(|mhz| (mhz * 1_000_000.0) as u64)
                    .filter(|&hz| (fmin..=fmax).contains(&hz));
            match parsed {
                Some(hz) => {
                    let (start, stop) = (m.sweep.config.start_hz, m.sweep.config.stop_hz);
                    let ordered = if is_start { hz < stop } else { hz > start };
                    if ordered {
                        let changed = if is_start { hz != start } else { hz != stop };
                        if is_start {
                            m.sweep.config.start_hz = hz;
                        } else {
                            m.sweep.config.stop_hz = hz;
                        }
                        if changed {
                            m.sweep.generation = m.sweep.generation.wrapping_add(1);
                        }
                        m.sweep.cycle_count = 0;
                        m.sweep.positions_done = 0;
                        m.sweep.current_frame = None;
                        m.sweep.cursor_frac = None;
                        m.ui.input_mode = InputMode::Normal;
                        m.ui.input_buf.clear();
                        m.push_log(format!(
                            "Sweep {} → {:.3} MHz",
                            if is_start { "START" } else { "STOP" },
                            hz as f64 / 1e6
                        ));
                    } else {
                        m.push_log(format!(
                            "Invalid: START must be below STOP (now {:.1}–{:.1} MHz)",
                            start as f64 / 1e6,
                            stop as f64 / 1e6
                        ));
                    }
                }
                None => {
                    let bad = m.ui.input_buf.clone();
                    m.push_log(format!(
                        "Invalid frequency: '{}' ({:.0}–{:.0} MHz)",
                        bad,
                        fmin as f64 / 1e6,
                        fmax as f64 / 1e6
                    ));
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
        KeyCode::Backspace => {
            metrics(state).ui.input_buf.pop();
        }
        KeyCode::Char(c) => {
            metrics(state).ui.input_buf.push(c);
        }
        KeyCode::Enter => {
            let mut m = metrics(state);
            if let Some(freq) = m.spectrum.pending_marker.take() {
                let label = if m.ui.input_buf.trim().is_empty() {
                    format!("M{}", m.spectrum.markers.len() + 1)
                } else {
                    m.ui.input_buf.trim().to_string()
                };
                m.push_log(format!(
                    "Marker: {} → {:.3} MHz",
                    label,
                    freq as f64 / 1_000_000.0
                ));
                m.spectrum.markers.push(SpectrumMarker {
                    freq_hz: freq,
                    label,
                    channel_bw_hz: None,
                    measured_bw_hz: None,
                });
            }
            m.ui.input_mode = InputMode::Normal;
            m.ui.input_buf.clear();
        }
        _ => {}
    }
}
/// How far the user trusts the census device at `address` as a frequency
/// reference, typed in ppm (net-ux-polish-plan 4.7, `T` in the census).
///
/// **A positive figure or nothing.** Zero would claim a crystal known
/// exactly, which no user can state; a refusal keeps the entry open so the
/// figure can be corrected rather than retyped. On Enter the reference is
/// built by [`crate::state::FrequencyReference::from_trusted`] from the
/// device's offset as it stands, replaces whatever reference there was, and
/// expires on the same declared interval every reference does.
pub(super) fn reference_accuracy(key: KeyEvent, state: &Arc<Mutex<SdrMetrics>>, address: [u8; 6]) {
    match key.code {
        KeyCode::Esc => {
            let mut m = metrics(state);
            m.ui.input_mode = InputMode::Normal;
            m.ui.input_buf.clear();
            m.push_log("Reference: cancelled");
        }
        KeyCode::Backspace => {
            metrics(state).ui.input_buf.pop();
        }
        KeyCode::Char(c) if c.is_ascii_digit() || c == '.' => {
            metrics(state).ui.input_buf.push(c);
        }
        KeyCode::Enter => {
            let now = std::time::Instant::now();
            let mut m = metrics(state);
            let stated =
                m.ui.input_buf
                    .parse::<f64>()
                    .ok()
                    .filter(|p| p.is_finite() && *p > 0.0);
            let Some(stated) = stated else {
                m.push_log("Reference: give the device's accuracy as a positive figure in ppm");
                return;
            };
            let found = m
                .net
                .census
                .devices
                .iter()
                .find(|d| d.address == address)
                .and_then(|d| Some((d.crystal_offset_ppm?, d.address_text(&m.net, None))));
            m.ui.input_mode = InputMode::Normal;
            m.ui.input_buf.clear();
            match found {
                Some((raw, name)) => {
                    let r = crate::state::FrequencyReference::from_trusted(raw, stated, &name, now);
                    m.push_log(format!(
                        "Reference: {} = {:+.2} ±{:.2} ppm, REFERENCED",
                        r.source, r.ppm, r.sigma_ppm
                    ));
                    m.radio.reference = Some(r);
                }
                None => m.push_log("Reference: the device has no offset measured, nothing set"),
            }
        }
        _ => {}
    }
}
