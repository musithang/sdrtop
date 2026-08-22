use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crossterm::event::{KeyCode, KeyEvent};

use crate::hardware;
use crate::state::{InputMode, MicroView, RailMode, SdrMetrics, SpectrumMarker};
use crate::ui::{self, spectrum::{fmt_spectrum_step, next_spectrum_step, prev_spectrum_step}};
use crate::ui::waterfall::{next_wf_stride, prev_wf_stride, next_wf_zoom, prev_wf_zoom};

fn fmt_bw(hz: u64) -> String {
    if hz >= 1_000_000 { format!("{:.1} MHz", hz as f64 / 1_000_000.0) }
    else if hz >= 1_000 { format!("{} kHz", hz / 1_000) }
    else                { format!("{} Hz", hz) }
}

pub enum KeyAction {
    Continue,
    Quit,
}

/// Next value for the primary front-end gain when stepping up/down: HackRF's LNA
/// moves in 8 dB steps (0–40); RTL-SDR's single tuner gain walks its discrete
/// table to the neighbouring entry.
fn next_primary_gain(gain: &hardware::GainModel, current: u32, up: bool) -> u32 {
    match gain {
        hardware::GainModel::HackRf => {
            if up { (current + 8).min(40) } else { current.saturating_sub(8) }
        }
        hardware::GainModel::RtlSingle { gain_steps_db, .. } => {
            if gain_steps_db.is_empty() {
                return current;
            }
            let idx = gain_steps_db
                .iter()
                .enumerate()
                .min_by_key(|(_, &g)| (g as i64 - current as i64).abs())
                .map(|(i, _)| i)
                .unwrap_or(0);
            let new_idx = if up {
                (idx + 1).min(gain_steps_db.len() - 1)
            } else {
                idx.saturating_sub(1)
            };
            gain_steps_db[new_idx]
        }
    }
}

/// Label for the primary gain stage in log messages.
fn primary_gain_label(gain: &hardware::GainModel) -> &'static str {
    match gain {
        hardware::GainModel::HackRf => "LNA",
        hardware::GainModel::RtlSingle { .. } => "Tuner",
    }
}

pub fn handle_key(
    key: KeyEvent,
    state: &Arc<Mutex<SdrMetrics>>,
    device: Option<&Arc<dyn hardware::SdrDevice>>,
    engine: &mut ui::LayoutEngine,
    show_help: &mut bool,
    show_footer: &mut bool,
    focus_keys: &HashMap<char, &'static str>,
) -> KeyAction {
    let input_mode = state.lock().unwrap_or_else(|e| e.into_inner()).ui.input_mode.clone();
    match input_mode {
        InputMode::Normal        => handle_normal(key, state, device, engine, show_help, show_footer, focus_keys),
        InputMode::FrequencyInput  => { handle_freq_input(key, state, device); KeyAction::Continue }
        InputMode::SampleRateInput => { handle_sr_input(key, state, device);   KeyAction::Continue }
        InputMode::MarkerNameInput => { handle_marker_input(key, state);        KeyAction::Continue }
        InputMode::SweepStartInput => { handle_sweep_range_input(key, state, true);  KeyAction::Continue }
        InputMode::SweepStopInput  => { handle_sweep_range_input(key, state, false); KeyAction::Continue }
    }
}

// ── Normal mode ───────────────────────────────────────────────────────────────

fn handle_normal(
    key: KeyEvent,
    state: &Arc<Mutex<SdrMetrics>>,
    device: Option<&Arc<dyn hardware::SdrDevice>>,
    engine: &mut ui::LayoutEngine,
    show_help: &mut bool,
    show_footer: &mut bool,
    focus_keys: &HashMap<char, &'static str>,
) -> KeyAction {
    let focused = engine.focused_panel_name().map(|s| s.to_string());

    match focused.as_deref() {
        Some("spectrum")        => handle_spectrum_focus(key, state, device, engine, show_help, show_footer, focus_keys),
        Some("waterfall")       => handle_waterfall_focus(key, state, engine, show_help, show_footer, focus_keys),
        Some("iq_diagnostics")  => handle_iq_focus(key, state, device, engine, show_help, show_footer, focus_keys),
        Some("rf_chain")        => handle_rf_focus(key, state, device, engine, show_help, show_footer, focus_keys),
        Some("hardware_health") => handle_health_focus(key, state, device, engine, show_help, show_footer, focus_keys),
        Some("timing_panel")    => handle_timing_focus(key, state, device, engine, show_help, show_footer, focus_keys),
        Some("sweep_panel")      => handle_sweep_focus(key, state, device, engine, show_help, show_footer, focus_keys),
        Some("signal_metrics")   => handle_signal_metrics_focus(key, state, device, engine, show_help, show_footer, focus_keys),
        Some("signal_characterization") => handle_signal_characterization_focus(key, state, device, engine, show_help, show_footer, focus_keys),
        Some("fm_demod")         => handle_demod_focus(key, state, device, engine, show_help, show_footer, focus_keys),
        Some("command_rail")     => handle_command_rail_focus(key, state, device, engine, show_help, show_footer, focus_keys),
        Some("lab_banner")      => handle_lab_banner_focus(key, state, device, engine, show_help, show_footer, focus_keys),
        _                       => handle_global(key, state, device, engine, show_help, show_footer, focus_keys),
    }
}

// ── Spectrum focus keys ───────────────────────────────────────────────────────

fn handle_spectrum_focus(
    key: KeyEvent,
    state: &Arc<Mutex<SdrMetrics>>,
    device: Option<&Arc<dyn hardware::SdrDevice>>,
    engine: &mut ui::LayoutEngine,
    show_help: &mut bool,
    show_footer: &mut bool,
    focus_keys: &HashMap<char, &'static str>,
) -> KeyAction {
    match key.code {
        KeyCode::Left => {
            if let Some(device) = device {
                let fmin = device.capabilities().freq_min_hz;
                let new_freq = {
                    let m = state.lock().unwrap_or_else(|e| e.into_inner());
                    m.radio.frequency.saturating_sub(m.spectrum.step_hz).max(fmin)
                };
                let result = device.set_frequency(new_freq);
                let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
                match result {
                    Ok(()) => { m.radio.frequency = new_freq; m.ui.note_mode_action(RailMode::Hunt); }
                    Err(e) => m.push_log(format!("Tune error: {}", e)),
                }
            }
        }
        KeyCode::Right => {
            if let Some(device) = device {
                let fmax = device.capabilities().freq_max_hz;
                let new_freq = {
                    let m = state.lock().unwrap_or_else(|e| e.into_inner());
                    (m.radio.frequency + m.spectrum.step_hz).min(fmax)
                };
                let result = device.set_frequency(new_freq);
                let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
                match result {
                    Ok(()) => { m.radio.frequency = new_freq; m.ui.note_mode_action(RailMode::Hunt); }
                    Err(e) => m.push_log(format!("Tune error: {}", e)),
                }
            }
        }
        KeyCode::Char('[') => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            let new_step = prev_spectrum_step(m.spectrum.step_hz);
            m.spectrum.step_hz = new_step;
            m.push_log(format!("Step → {}", fmt_spectrum_step(new_step)));
        }
        KeyCode::Char(']') => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            let new_step = next_spectrum_step(m.spectrum.step_hz);
            m.spectrum.step_hz = new_step;
            m.push_log(format!("Step → {}", fmt_spectrum_step(new_step)));
        }
        // Shared frequency zoom — in the bonded spectrum+waterfall view both plots
        // share one span, so `+`/`-` here drive the same `hz_zoom` the waterfall
        // does, narrowing the whole instrument together.
        KeyCode::Char('+') | KeyCode::Char('=') => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            let new_zoom = next_wf_zoom(m.waterfall.hz_zoom);
            m.waterfall.hz_zoom = new_zoom;
            m.push_log(format!("Freq zoom: ×{}", new_zoom));
        }
        KeyCode::Char('-') => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            let new_zoom = prev_wf_zoom(m.waterfall.hz_zoom);
            m.waterfall.hz_zoom = new_zoom;
            if new_zoom == 1 {
                m.push_log("Freq zoom: off".to_string());
            } else {
                m.push_log(format!("Freq zoom: ×{}", new_zoom));
            }
        }
        KeyCode::Up => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            let new_min = (m.spectrum.y_min + 10.0).min(m.spectrum.y_max - 20.0);
            m.spectrum.y_min = new_min;
            let ymax = m.spectrum.y_max;
            m.push_log(format!("Zoom: {:.0}…{:.0} dBFS", new_min, ymax));
        }
        KeyCode::Down => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            let new_min = (m.spectrum.y_min - 10.0).max(-120.0);
            m.spectrum.y_min = new_min;
            let ymax = m.spectrum.y_max;
            m.push_log(format!("Zoom: {:.0}…{:.0} dBFS", new_min, ymax));
        }
        KeyCode::Char('j') => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            let step = m.spectrum.step_hz;
            m.spectrum.cursor_freq = Some(match m.spectrum.cursor_freq {
                Some(f) => f.saturating_sub(step).max(m.caps.freq_min_hz),
                None    => m.radio.frequency,
            });
        }
        KeyCode::Char('k') => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            let step = m.spectrum.step_hz;
            m.spectrum.cursor_freq = Some(match m.spectrum.cursor_freq {
                Some(f) => (f + step).min(m.caps.freq_max_hz),
                None    => m.radio.frequency,
            });
        }
        KeyCode::Char('m') => {
            let (marker_freq, existing_idx) = {
                let m = state.lock().unwrap_or_else(|e| e.into_inner());
                let freq = if let Some(f) = m.spectrum.cursor_freq {
                    f
                } else if let Some(frame) = &m.waterfall.last_fft {
                    let peak_bin = frame.bins_dbfs.iter()
                        .enumerate()
                        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
                        .map(|(i, _)| i)
                        .unwrap_or(frame.bins_dbfs.len() / 2);
                    let left_hz = m.radio.frequency as f64 - frame.sample_rate / 2.0;
                    (left_hz + peak_bin as f64 / frame.bins_dbfs.len() as f64 * frame.sample_rate).round() as u64
                } else {
                    m.radio.frequency
                };
                let step = m.spectrum.step_hz;
                let idx = m.spectrum.markers.iter().position(|mk| {
                    (mk.freq_hz as i64 - freq as i64).unsigned_abs() < step
                });
                (freq, idx)
            };
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(idx) = existing_idx {
                let removed = m.spectrum.markers.remove(idx);
                m.push_log(format!("Marker removed: {}", removed.label));
            } else {
                m.spectrum.pending_marker = Some(marker_freq);
                m.ui.input_mode = InputMode::MarkerNameInput;
                m.ui.input_buf.clear();
                m.push_log(format!(
                    "Name this marker at {:.3} MHz (Enter = confirm, empty = auto-label)",
                    marker_freq as f64 / 1_000_000.0
                ));
            }
        }
        KeyCode::Char('b') => {
            const BW_STEPS: &[u64] = &[6_250, 12_500, 25_000, 50_000, 100_000, 200_000, 500_000];
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            let cursor = m.spectrum.cursor_freq.unwrap_or(m.radio.frequency);
            let step   = m.spectrum.step_hz;
            if let Some(mk) = m.spectrum.markers.iter_mut()
                .min_by_key(|mk| (mk.freq_hz as i64 - cursor as i64).unsigned_abs())
                .filter(|mk| (mk.freq_hz as i64 - cursor as i64).unsigned_abs() < step * 4)
            {
                let next = match mk.channel_bw_hz {
                    None      => Some(BW_STEPS[0]),
                    Some(cur) => {
                        let idx = BW_STEPS.iter().position(|&b| b == cur);
                        idx.and_then(|i| BW_STEPS.get(i + 1)).copied()
                    }
                };
                mk.channel_bw_hz = next;
                mk.measured_bw_hz = None;
                let msg = match next {
                    Some(bw) => format!("Marker '{}' channel BW → {}", mk.label, fmt_bw(bw)),
                    None     => format!("Marker '{}' channel BW cleared", mk.label),
                };
                m.push_log(msg);
            } else {
                m.push_log("No marker near cursor — place one with [M] first");
            }
        }
        // `D` cycles the trace render style (braille → fill → scatter); persisted.
        KeyCode::Char('d') | KeyCode::Char('D') => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            let next = m.spectrum.style.next();
            m.spectrum.style = next;
            m.push_log(format!("Spectrum style: {}", next.label()));
        }
        // All other keys fall through to global handler
        _ => return handle_global(key, state, device, engine, show_help, show_footer, focus_keys),
    }
    KeyAction::Continue
}

// ── Waterfall focus keys ──────────────────────────────────────────────────────

fn handle_waterfall_focus(
    key: KeyEvent,
    state: &Arc<Mutex<SdrMetrics>>,
    engine: &mut ui::LayoutEngine,
    show_help: &mut bool,
    show_footer: &mut bool,
    focus_keys: &HashMap<char, &'static str>,
) -> KeyAction {
    match key.code {
        KeyCode::Up => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            let new_min = (m.waterfall.db_min + 10.0).min(-20.0);
            m.waterfall.db_min = new_min;
            m.push_log(format!("Waterfall zoom: {:.0}…0 dBFS", new_min));
        }
        KeyCode::Down => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            let new_min = (m.waterfall.db_min - 10.0).max(-120.0);
            m.waterfall.db_min = new_min;
            m.push_log(format!("Waterfall zoom: {:.0}…0 dBFS", new_min));
        }
        KeyCode::Char('[') => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            let new_stride = prev_wf_stride(m.waterfall.buffer.row_stride);
            m.waterfall.buffer.set_row_stride(new_stride);
            m.push_log(format!("Waterfall: ×{} frames/row", new_stride));
        }
        KeyCode::Char(']') => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            let new_stride = next_wf_stride(m.waterfall.buffer.row_stride);
            m.waterfall.buffer.set_row_stride(new_stride);
            m.push_log(format!("Waterfall: ×{} frames/row", new_stride));
        }
        KeyCode::Char('+') | KeyCode::Char('=') => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            let new_zoom = next_wf_zoom(m.waterfall.hz_zoom);
            m.waterfall.hz_zoom = new_zoom;
            m.push_log(format!("Waterfall zoom: ×{}", new_zoom));
        }
        KeyCode::Char('-') => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            let new_zoom = prev_wf_zoom(m.waterfall.hz_zoom);
            m.waterfall.hz_zoom = new_zoom;
            if new_zoom == 1 {
                m.push_log("Waterfall zoom: off".to_string());
            } else {
                m.push_log(format!("Waterfall zoom: ×{}", new_zoom));
            }
        }
        KeyCode::Char('m') => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            m.waterfall.cursor_freq = if m.waterfall.cursor_freq.is_some() {
                None
            } else {
                Some(m.radio.frequency)
            };
        }
        KeyCode::Left => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(cf) = m.waterfall.cursor_freq {
                m.waterfall.cursor_freq = Some(cf.saturating_sub(m.spectrum.step_hz).max(m.caps.freq_min_hz));
            }
        }
        KeyCode::Right => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(cf) = m.waterfall.cursor_freq {
                m.waterfall.cursor_freq = Some((cf + m.spectrum.step_hz).min(m.caps.freq_max_hz));
            }
        }
        KeyCode::Char('j') => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            let max = m.waterfall.buffer.rows.len() / 2;
            m.waterfall.scroll_offset = (m.waterfall.scroll_offset + 1).min(max);
        }
        KeyCode::Char('k') => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            m.waterfall.scroll_offset = m.waterfall.scroll_offset.saturating_sub(1);
        }
        // `P` cycles the colour gradient (classic → amber → ice → phosphor). The
        // choice persists to `[display] waterfall_palette` on quit.
        KeyCode::Char('p') | KeyCode::Char('P') => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            let next = m.waterfall.palette.next();
            m.waterfall.palette = next;
            m.push_log(format!("Waterfall palette: {}", next.label()));
        }
        _ => return handle_global_no_device(key, state, engine, show_help, show_footer, focus_keys),
    }
    KeyAction::Continue
}

// ── Lab panel focus keys ──────────────────────────────────────────────────────
//
// Each lab panel's focus mode adds only panel-specific actions; every other key
// falls through to the global handler (so Esc, Space, gain, etc. keep working).
// `rf_chain` (the Lab RF "RF Diagnostics" panel) adds a focus mode for the bench's
// own actions: `[A]` auto-gain (one-shot to optimal, then a press at optimum latches
// a continuous track) and `[⎵]`/`[F]` to freeze the histogram + level diagram. The
// gain nudges themselves ([↑↓] LNA, [ ] VGA) still fall through to the global keys.

/// `iq_diagnostics` focus: `[C]` logs a one-line snapshot of the current IQ
/// balance figures as a reference capture.
/// `lab_banner` focus (`b`): drive the lab measurement controls — REF level
/// (`↑/↓`, `R` to clear), trace averaging (`[ ]`), and CAL reference-trace
/// capture/clear (`C`). Everything else falls through to the global handler.
fn handle_lab_banner_focus(
    key: KeyEvent,
    state: &Arc<Mutex<SdrMetrics>>,
    device: Option<&Arc<dyn hardware::SdrDevice>>,
    engine: &mut ui::LayoutEngine,
    show_help: &mut bool,
    show_footer: &mut bool,
    focus_keys: &HashMap<char, &'static str>,
) -> KeyAction {
    match key.code {
        KeyCode::Up   => { state.lock().unwrap_or_else(|e| e.into_inner()).lab.adjust_ref(1.0); }
        KeyCode::Down => { state.lock().unwrap_or_else(|e| e.into_inner()).lab.adjust_ref(-1.0); }
        KeyCode::Char('[') => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            m.lab.adjust_avg(-1);
            let n = m.lab.avg_n;
            m.push_log(format!("Averaging: \u{00D7}{n}"));
        }
        KeyCode::Char(']') => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            m.lab.adjust_avg(1);
            let n = m.lab.avg_n;
            m.push_log(format!("Averaging: \u{00D7}{n}"));
        }
        KeyCode::Char('r') | KeyCode::Char('R') => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            m.lab.ref_dbfs = None;
            m.push_log("Reference level cleared");
        }
        KeyCode::Char('c') | KeyCode::Char('C') => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            if m.lab.ref_trace.is_some() {
                m.lab.ref_trace = None;
                m.push_log("Reference trace cleared");
            } else if let Some(bins) = m.waterfall.last_fft.as_ref().map(|fr| Arc::clone(&fr.bins_dbfs)) {
                m.lab.ref_trace = Some(bins);
                m.push_log("Reference trace captured");
            } else {
                m.push_log("No spectrum frame to capture");
            }
        }
        _ => return handle_global(key, state, device, engine, show_help, show_footer, focus_keys),
    }
    KeyAction::Continue
}

fn handle_iq_focus(
    key: KeyEvent,
    state: &Arc<Mutex<SdrMetrics>>,
    device: Option<&Arc<dyn hardware::SdrDevice>>,
    engine: &mut ui::LayoutEngine,
    show_help: &mut bool,
    show_footer: &mut bool,
    focus_keys: &HashMap<char, &'static str>,
) -> KeyAction {
    match key.code {
        // [M] — pin / unpin the carrier+image markers (override the live auto-track).
        KeyCode::Char('m') | KeyCode::Char('M') => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            let auto = ui::image_scope::carrier_image(&m);
            if m.lab.iq_marker_pin.is_some() {
                m.lab.iq_marker_pin = None;
                m.push_log("IQ markers: auto-tracking carrier/image".to_string());
            } else if let Some(ci) = auto {
                m.lab.iq_marker_pin = Some((ci.carrier_hz, ci.image_hz));
                m.push_log(format!(
                    "IQ markers pinned — carrier {:.3} MHz · image {:.3} MHz · supp {:.1} dB",
                    ci.carrier_hz as f64 / 1e6, ci.image_hz as f64 / 1e6, ci.suppression_db,
                ));
            } else {
                m.push_log("IQ markers: no carrier detected yet".to_string());
            }
            return KeyAction::Continue;
        }
        // [D] — DC-block: subtract the live DC estimate from the stream.
        KeyCode::Char('d') | KeyCode::Char('D') => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            m.iq.cal.dc_block_on = !m.iq.cal.dc_block_on;
            let on = m.iq.cal.dc_block_on;
            m.push_log(if on { "DC-block ON — subtracting DC offset from the stream" }
                       else  { "DC-block OFF" }.to_string());
            return KeyAction::Continue;
        }
        // [C] — auto-cal: capture (or clear) the I/Q quadrature correction.
        KeyCode::Char('c') | KeyCode::Char('C') => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            if m.iq.cal.cal_applied || m.iq.cal.cal_pending {
                m.iq.cal.cal_applied = false;
                m.iq.cal.cal_pending = false;
                m.iq.cal.c_qi = 0.0;
                m.iq.cal.c_qq = 1.0;
                m.push_log("IQ auto-cal cleared — quadrature uncorrected".to_string());
            } else {
                m.iq.cal.cal_pending = true;
                m.push_log("IQ auto-cal — capturing correction…".to_string());
            }
            return KeyAction::Continue;
        }
        // [F] — freeze / thaw the constellation cloud.
        KeyCode::Char('f') | KeyCode::Char('F') => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            m.iq.cal.frozen = !m.iq.cal.frozen;
            let frozen = m.iq.cal.frozen;
            m.push_log(if frozen { "Constellation frozen" } else { "Constellation live" }.to_string());
            return KeyAction::Continue;
        }
        _ => {}
    }
    handle_global(key, state, device, engine, show_help, show_footer, focus_keys)
}

/// `rf_chain` (RF Diagnostics) focus (`[D]`): the Lab RF bench actions.
/// `[A]` — when the chain is off-optimal, one-shot jump LNA/VGA to the staging
/// target (signal ≈ −8 dBFS); when already optimal, toggle the continuous auto-track
/// latch. `[⎵]`/`[F]` freeze or thaw the histogram + level diagram. Everything else
/// (incl. the [↑↓]/[ ] gain nudges) falls through to the global handler.
fn handle_rf_focus(
    key: KeyEvent,
    state: &Arc<Mutex<SdrMetrics>>,
    device: Option<&Arc<dyn hardware::SdrDevice>>,
    engine: &mut ui::LayoutEngine,
    show_help: &mut bool,
    show_footer: &mut bool,
    focus_keys: &HashMap<char, &'static str>,
) -> KeyAction {
    use crate::ui::rf_calc::staging_target;
    match key.code {
        // [A] — auto-gain: one-shot to optimal, or latch the continuous track once
        // already there. HackRF-only; never runs unless streaming.
        KeyCode::Char('a') | KeyCode::Char('A') => {
            let (peak, lna, vga, friis, streaming) = {
                let m = state.lock().unwrap_or_else(|e| e.into_inner());
                (m.signal.adc_peak_dbfs as f64, m.radio.lna_gain, m.radio.vga_gain,
                 m.caps.friis_applicable, m.radio.hw_streaming)
            };
            if !streaming {
                state.lock().unwrap_or_else(|e| e.into_inner())
                    .push_log("Auto-gain: start RX first ([Space])".to_string());
                return KeyAction::Continue;
            }
            if !friis {
                state.lock().unwrap_or_else(|e| e.into_inner())
                    .push_log("Auto-gain: single-tuner radio \u{2014} not applicable".to_string());
                return KeyAction::Continue;
            }
            let (lna_t, vga_t) = staging_target(peak, lna, vga);
            if (lna_t, vga_t) != (lna, vga) {
                // Off-optimal → one-shot jump through the same clamped gain path the
                // manual keys use. The latch is left as-is (hands-off one-shot).
                if let Some(device) = device {
                    let r1 = if lna_t != lna { device.set_lna_gain(lna_t) } else { Ok(()) };
                    let r2 = if vga_t != vga { device.set_vga_gain(vga_t) } else { Ok(()) };
                    let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
                    match (r1, r2) {
                        (Ok(()), Ok(())) => {
                            m.radio.lna_gain = lna_t;
                            m.radio.vga_gain = vga_t;
                            m.ui.note_mode_action(RailMode::Bench);
                            m.push_log(format!(
                                "Auto-gain \u{2192} LNA {lna_t} \u{00b7} VGA {vga_t} dB (signal \u{2192} \u{2212}8 dBFS)"));
                        }
                        _ => m.push_log("Auto-gain: device error".to_string()),
                    }
                }
            } else {
                // Already optimal → toggle the continuous auto-track latch.
                let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
                m.lab.rf_autotrack = !m.lab.rf_autotrack;
                let on = m.lab.rf_autotrack;
                m.push_log(if on {
                    "Auto-gain: continuous track ON \u{2014} re-nudges on drift".to_string()
                } else {
                    "Auto-gain: continuous track OFF".to_string()
                });
            }
            return KeyAction::Continue;
        }
        // [⎵]/[F] — freeze / thaw the histogram + level diagram (display only; RX
        // keeps running). Bound to focus, not global Space=RX.
        KeyCode::Char(' ') | KeyCode::Char('f') | KeyCode::Char('F') => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            if m.lab.rf_freeze.is_some() {
                m.lab.rf_freeze = None;
                m.push_log("Lab RF: live".to_string());
            } else {
                m.lab.rf_freeze = Some(crate::state::RfFreeze {
                    signed_hist:  m.iq.adc_signed_hist,
                    peak_dbfs:    m.signal.adc_peak_dbfs,
                    rms_dbfs:     m.signal.adc_rms_dbfs,
                    clip_events:  m.signal.adc_clip_events,
                    snr_db:       m.signal.peak_to_nf_db,
                    amp_enabled:  m.radio.amp_enabled,
                    lna_gain:     m.radio.lna_gain,
                    vga_gain:     m.radio.vga_gain,
                });
                m.push_log("Lab RF: frozen \u{2014} histogram & diagram held".to_string());
            }
            return KeyAction::Continue;
        }
        _ => {}
    }
    handle_global(key, state, device, engine, show_help, show_footer, focus_keys)
}

/// `signal_metrics` focus (`[N]`): `[C]` logs a one-line snapshot of the current
/// signal quality metrics (SNR, channel power, occupied BW, noise floor).
fn handle_signal_metrics_focus(
    key: KeyEvent,
    state: &Arc<Mutex<SdrMetrics>>,
    device: Option<&Arc<dyn hardware::SdrDevice>>,
    engine: &mut ui::LayoutEngine,
    show_help: &mut bool,
    show_footer: &mut bool,
    focus_keys: &HashMap<char, &'static str>,
) -> KeyAction {
    if let KeyCode::Char('c') = key.code {
        let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
        let snr = m.signal.peak_to_nf_db;
        let pwr = m.signal.channel_power_dbfs;
        let obw = m.signal.occupied_bw_hz;
        let nf  = m.waterfall.last_fft.as_ref().map(|fr| fr.noise_floor);
        let obw_str = if obw >= 1_000_000 {
            format!("{:.3} MHz", obw as f64 / 1_000_000.0)
        } else if obw >= 1_000 {
            format!("{:.1} kHz", obw as f64 / 1_000.0)
        } else {
            format!("{} Hz", obw)
        };
        let nf_str = nf.map(|n| format!("{:.1} dBFS", n)).unwrap_or_else(|| "\u{2014}".into());
        m.push_log(format!(
            "Signal snapshot — SNR: {:.1} dB · Pwr: {:.1} dBFS · OBW: {} · NF: {}",
            snr, pwr, obw_str, nf_str
        ));
        return KeyAction::Continue;
    }
    handle_global(key, state, device, engine, show_help, show_footer, focus_keys)
}

/// `signal_characterization` focus (`[X]`): `[C]` logs a one-line snapshot of the
/// current characterization — modulation, SNR, occupied BW, and ACPR when it's
/// been measured. The `lab_signal` analogue of `handle_signal_metrics_focus`.
fn handle_signal_characterization_focus(
    key: KeyEvent,
    state: &Arc<Mutex<SdrMetrics>>,
    device: Option<&Arc<dyn hardware::SdrDevice>>,
    engine: &mut ui::LayoutEngine,
    show_help: &mut bool,
    show_footer: &mut bool,
    focus_keys: &HashMap<char, &'static str>,
) -> KeyAction {
    if let KeyCode::Char('c') = key.code {
        let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
        let modulation = m.signal.modulation.label();
        let snr = m.signal.peak_to_nf_db;
        let obw = m.signal.occupied_bw_hz;
        let obw_str = fmt_bw(obw);
        let acpr_lo = m.signal.acpr_lower_db;
        let acpr_hi = m.signal.acpr_upper_db;
        // Both sides, or neither: the line prints the pair, so one alone would log
        // "ACPR -inf/-38 dB".
        let acpr_str = if acpr_lo.is_finite() && acpr_hi.is_finite() {
            format!(" \u{00b7} ACPR {acpr_lo:.0}/{acpr_hi:.0} dB")
        } else {
            String::new()
        };
        m.push_log(format!(
            "Signal characterization \u{2014} {modulation}: SNR {snr:.1} dB \u{00b7} OBW {obw_str}{acpr_str}"
        ));
        return KeyAction::Continue;
    }
    handle_global(key, state, device, engine, show_help, show_footer, focus_keys)
}

/// `fm_demod` focus (`[M]`): `[Space]` switches the demodulator on and off,
/// `←`/`→` walk the channel offset, `[P]` snaps it onto the strongest carrier,
/// `[0]` recentres it, and `[C]` snapshots the measurement to the log.
///
/// `[Space]` is deliberately shadowed here rather than falling through to the
/// global RX toggle: inside the demod bench the nearer meaning of "start/stop" is
/// the demodulator, and RX remains reachable by leaving focus with `Esc`.
///
/// The offset exists because the tuned centre is where both front-ends put their
/// DC offset and LO leakage; moving the channel off it is what makes the reading
/// trustworthy, so it belongs on the arrow keys rather than buried in a submenu.
fn handle_demod_focus(
    key: KeyEvent,
    state: &Arc<Mutex<SdrMetrics>>,
    device: Option<&Arc<dyn hardware::SdrDevice>>,
    engine: &mut ui::LayoutEngine,
    show_help: &mut bool,
    show_footer: &mut bool,
    focus_keys: &HashMap<char, &'static str>,
) -> KeyAction {
    match key.code {
        KeyCode::Char(' ') => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            m.demod.user_on = !m.demod.user_on;
            let on = m.demod.user_on;
            if !on {
                // Drop the reading immediately so the panel can't keep showing a
                // number the demod is no longer producing.
                m.demod.fm = None;
            }
            m.push_log(if on { "Demod: on" } else { "Demod: off" });
            return KeyAction::Continue;
        }
        KeyCode::Left | KeyCode::Right => {
            let step = if matches!(key.code, KeyCode::Left) {
                -crate::state::OFFSET_STEP_HZ
            } else {
                crate::state::OFFSET_STEP_HZ
            };
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            let limit = m.demod.offset_limit_hz(m.radio.config_sample_rate);
            let next = (m.demod.offset_hz + step).clamp(-limit, limit);
            if next != m.demod.offset_hz {
                m.demod.offset_hz = next;
                // The channel moved; the old reading describes a different
                // frequency, so drop it rather than let it linger.
                m.demod.fm = None;
                m.push_log(format!("Demod offset: {:+.0} kHz", next as f64 / 1000.0));
            }
            return KeyAction::Continue;
        }
        KeyCode::Char('p') | KeyCode::Char('P') => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            let bins = m.waterfall.last_fft.as_ref().map(|f| Arc::clone(&f.bins_dbfs));
            let sr = m.radio.config_sample_rate;
            match bins.and_then(|b| crate::state::strongest_offset_hz(&b, sr)) {
                Some(off) => {
                    let limit = m.demod.offset_limit_hz(sr);
                    let snapped = off.clamp(-limit, limit);
                    m.demod.offset_hz = snapped;
                    m.demod.fm = None;
                    m.push_log(format!("Demod snapped to carrier: {:+.0} kHz",
                                       snapped as f64 / 1000.0));
                }
                None => m.push_log("Demod: no spectrum to snap to yet"),
            }
            return KeyAction::Continue;
        }
        KeyCode::Char('0') => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            m.demod.offset_hz = 0;
            m.demod.fm = None;
            m.push_log("Demod offset: centre");
            return KeyAction::Continue;
        }
        KeyCode::Char('t') | KeyCode::Char('T') => {
            // The classifier measures 99 % occupied bandwidth across the whole
            // span, so on a wide span it reads WFM for nearly anything — fine as a
            // badge, too coarse to choose a demodulator. This is the override.
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            let picked = m.demod.cycle_mode();
            // The previous demodulator's numbers describe a different measurement.
            m.demod.fm = None;
            m.demod.am = None;
            m.demod.ctcss = None;
            m.demod.ctcss_fill = 0.0;
            m.push_log(match picked {
                Some(mode) => format!("Demod mode: {} (forced)", mode.label()),
                None       => "Demod mode: auto".to_string(),
            });
            return KeyAction::Continue;
        }
        KeyCode::Char('c') | KeyCode::Char('C') => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            let line = match m.demod.live() {
                Some(fm) => format!(
                    "Demod \u{2014} {}: peak dev {:.1} kHz \u{00b7} RMS {:.1} kHz \u{00b7} offset {:+.0} Hz \u{00b7} {:.0} kHz ch",
                    m.signal.modulation.label(),
                    fm.peak_dev_hz / 1000.0,
                    fm.rms_dev_hz / 1000.0,
                    fm.carrier_offset_hz,
                    m.demod.channel_rate_hz / 1000.0,
                ),
                None => "Demod \u{2014} no measurement to snapshot".to_string(),
            };
            m.push_log(line);
            return KeyAction::Continue;
        }
        _ => {}
    }
    handle_global(key, state, device, engine, show_help, show_footer, focus_keys)
}

/// `hardware_health` focus (`[V]`): `[R]` resets the session drop counter, `[C]`
/// clears the trend sparkline histories.
fn handle_health_focus(
    key: KeyEvent,
    state: &Arc<Mutex<SdrMetrics>>,
    device: Option<&Arc<dyn hardware::SdrDevice>>,
    engine: &mut ui::LayoutEngine,
    show_help: &mut bool,
    show_footer: &mut bool,
    focus_keys: &HashMap<char, &'static str>,
) -> KeyAction {
    match key.code {
        KeyCode::Char('r') => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            m.signal.total_drops_session = 0;
            m.push_log("Session drop counter reset");
        }
        KeyCode::Char('c') => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            m.signal.drop_history.clear();
            m.signal.saturation_history.clear();
            m.signal.usb_error_history.clear();
            m.iq.buf_fill_history.clear();
            m.system.cpu_history.clear();
            m.push_log("Health trend history cleared");
        }
        _ => return handle_global(key, state, device, engine, show_help, show_footer, focus_keys),
    }
    KeyAction::Continue
}

/// `timing_panel` focus (`[T]`): `[R]` resets the session jitter peak, `[C]`
/// clears the jitter / throughput / sample-rate histories.
fn handle_timing_focus(
    key: KeyEvent,
    state: &Arc<Mutex<SdrMetrics>>,
    device: Option<&Arc<dyn hardware::SdrDevice>>,
    engine: &mut ui::LayoutEngine,
    show_help: &mut bool,
    show_footer: &mut bool,
    focus_keys: &HashMap<char, &'static str>,
) -> KeyAction {
    match key.code {
        KeyCode::Char('r') => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            m.timing.jitter_session_max_us = 0;
            m.push_log("Jitter session peak reset");
        }
        KeyCode::Char('c') => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            m.iq.jitter_history.clear();
            m.radio.throughput_history.clear();
            m.radio.sample_rate_history.clear();
            m.push_log("Timing trend history cleared");
        }
        _ => return handle_global(key, state, device, engine, show_help, show_footer, focus_keys),
    }
    KeyAction::Continue
}

/// `sweep_panel` focus (`[G]`): cursor with `←/→`, peak/mean with `M`, dwell with
/// `+/-`, and `[Enter]` to leave the sweep tuned to the cursor frequency.
fn handle_sweep_focus(
    key: KeyEvent,
    state: &Arc<Mutex<SdrMetrics>>,
    device: Option<&Arc<dyn hardware::SdrDevice>>,
    engine: &mut ui::LayoutEngine,
    show_help: &mut bool,
    show_footer: &mut bool,
    focus_keys: &HashMap<char, &'static str>,
) -> KeyAction {
    /// Cursor step as a fraction of the swept band per key press.
    const CURSOR_STEP: f64 = 0.01;
    match key.code {
        KeyCode::Left => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            let cur = m.sweep.cursor_frac.unwrap_or(0.5);
            m.sweep.cursor_frac = Some((cur - CURSOR_STEP).clamp(0.0, 1.0));
        }
        KeyCode::Right => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            let cur = m.sweep.cursor_frac.unwrap_or(0.5);
            m.sweep.cursor_frac = Some((cur + CURSOR_STEP).clamp(0.0, 1.0));
        }
        KeyCode::Char('m') => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            m.sweep.show_peak = !m.sweep.show_peak;
            let mode = if m.sweep.show_peak { "peak" } else { "mean" };
            m.push_log(format!("Sweep: {} curve", mode));
        }
        KeyCode::Char('+') | KeyCode::Char('=') => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            m.sweep.config.dwell_ms = (m.sweep.config.dwell_ms + 50).min(2000);
            let d = m.sweep.config.dwell_ms;
            m.push_log(format!("Sweep dwell → {} ms", d));
        }
        KeyCode::Char('-') => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            m.sweep.config.dwell_ms = m.sweep.config.dwell_ms.saturating_sub(50).max(50);
            let d = m.sweep.config.dwell_ms;
            m.push_log(format!("Sweep dwell → {} ms", d));
        }
        KeyCode::Char('c') => {
            let m = state.lock().unwrap_or_else(|e| e.into_inner());
            let msg = if let Some(frame) = m.sweep.current_frame.as_ref() {
                let curve = if m.sweep.show_peak { &frame.peak_dbfs } else { &frame.mean_dbfs };
                let cursor_str = if let Some(frac) = m.sweep.cursor_frac {
                    let hz = frame.freq_at_fraction(frac);
                    // Find the bin in freq_hz closest to the cursor frequency.
                    let level = frame.freq_hz.iter().enumerate()
                        .min_by_key(|(_, &f)| f.abs_diff(hz))
                        .and_then(|(i, _)| curve.get(i).copied().filter(|v| v.is_finite()));
                    let db_str = level.map(|v| format!("{:.1} dBFS", v)).unwrap_or_else(|| "\u{2014}".into());
                    format!("cursor {:.3} MHz {} · ", hz as f64 / 1e6, db_str)
                } else {
                    String::new()
                };
                let top = frame.top_peaks(1, 500_000).into_iter().next()
                    .map(|(f, v)| format!("top {:.3} MHz {:.1} dBFS", f as f64 / 1e6, v))
                    .unwrap_or_else(|| "no data".into());
                format!(
                    "Sweep snapshot — {}{} · {:.1}–{:.1} MHz ({:.1}s/cycle)",
                    cursor_str, top,
                    frame.start_hz as f64 / 1e6, frame.stop_hz as f64 / 1e6,
                    frame.cycle_duration_ms as f64 / 1000.0,
                )
            } else {
                "Sweep snapshot — no sweep data yet".into()
            };
            drop(m);
            state.lock().unwrap_or_else(|e| e.into_inner()).push_log(msg);
        }
        KeyCode::Char('s') => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            m.ui.input_mode = InputMode::SweepStartInput;
            m.ui.input_buf.clear();
            m.push_log("Enter sweep START frequency in MHz, then Enter");
        }
        KeyCode::Char('e') => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            m.ui.input_mode = InputMode::SweepStopInput;
            m.ui.input_buf.clear();
            m.push_log("Enter sweep STOP frequency in MHz, then Enter");
        }
        KeyCode::Enter => {
            // Resolve the cursor frequency, stash it as the jump target, then leave
            // lab_sweep — the sweep_task tunes there as it stops.
            let target = {
                let m = state.lock().unwrap_or_else(|e| e.into_inner());
                match (m.sweep.cursor_frac, m.sweep.current_frame.as_ref()) {
                    (Some(fr), Some(f)) => Some(f.freq_at_fraction(fr)),
                    _ => None,
                }
            };
            if let Some(hz) = target {
                {
                    let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
                    m.sweep.pending_tune = Some(hz);
                }
                engine.clear_focus();
                engine.set_preset("spectrum_waterfall");
                let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
                m.ui.focused_panel = None;
                m.ui.focused_panel_bindings = &[];
                m.push_log(format!("Jumping to {:.3} MHz from sweep…", hz as f64 / 1e6));
            }
        }
        _ => return handle_global(key, state, device, engine, show_help, show_footer, focus_keys),
    }
    KeyAction::Continue
}

// ── Command Rail focus keys ───────────────────────────────────────────────────

/// `command_rail` focus (`[C]`): `←/→` tune by the spectrum step (which auto-
/// switches the lead card to Hunt), `Tab` cycles the mode manually. Recall slots
/// (`1·2·3·M`) and the log overlay (`L`) arrive in later steps. Every other key
/// falls through to the global handler (so `Esc` exits focus as usual).
fn handle_command_rail_focus(
    key: KeyEvent,
    state: &Arc<Mutex<SdrMetrics>>,
    device: Option<&Arc<dyn hardware::SdrDevice>>,
    engine: &mut ui::LayoutEngine,
    show_help: &mut bool,
    show_footer: &mut bool,
    focus_keys: &HashMap<char, &'static str>,
) -> KeyAction {
    match key.code {
        // Esc closes the log overlay first (if open), only then exits focus.
        KeyCode::Esc => {
            let closed = {
                let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
                if m.ui.log_overlay { m.ui.log_overlay = false; true } else { false }
            };
            if !closed { return handle_global(key, state, device, engine, show_help, show_footer, focus_keys); }
        }
        // Toggle the full-log overlay (in rail-focus; globally `l` focuses waterfall).
        KeyCode::Char('l') | KeyCode::Char('L') => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            m.ui.log_overlay = !m.ui.log_overlay;
        }
        KeyCode::Left | KeyCode::Right => {
            if let Some(device) = device {
                let caps = device.capabilities();
                let new_freq = {
                    let m = state.lock().unwrap_or_else(|e| e.into_inner());
                    let step = m.spectrum.step_hz;
                    if matches!(key.code, KeyCode::Left) {
                        m.radio.frequency.saturating_sub(step).max(caps.freq_min_hz)
                    } else {
                        (m.radio.frequency + step).min(caps.freq_max_hz)
                    }
                };
                let result = device.set_frequency(new_freq);
                let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
                match result {
                    Ok(()) => { m.radio.frequency = new_freq; m.ui.note_mode_action(RailMode::Hunt); }
                    Err(e) => m.push_log(format!("Tune error: {}", e)),
                }
            }
        }
        KeyCode::Tab => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            let mode = m.ui.cycle_rail_mode();
            m.push_log(format!("Rail mode: {}", mode.label()));
        }
        // Save the current tuning into a recall slot (free slot, else oldest).
        KeyCode::Char('m') | KeyCode::Char('M') => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            let freq = m.radio.frequency;
            let slot = m.ui.save_recall(freq);
            m.push_log(format!("Recall {} ← {:.3} MHz", slot + 1, freq as f64 / 1e6));
        }
        // Jump to recall slot 1/2/3 (rail-focus only; globally these switch presets).
        KeyCode::Char(c @ '1'..='3') => {
            let slot = c as usize - '1' as usize;
            let target = { state.lock().unwrap_or_else(|e| e.into_inner()).ui.recall[slot] };
            match (target, device) {
                (Some(hz), Some(device)) => {
                    let result = device.set_frequency(hz);
                    let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
                    match result {
                        Ok(()) => {
                            m.radio.frequency = hz;
                            m.ui.note_mode_action(RailMode::Hunt);
                            m.push_log(format!("Recall {} → {:.3} MHz", slot + 1, hz as f64 / 1e6));
                        }
                        Err(e) => m.push_log(format!("Recall error: {}", e)),
                    }
                }
                (None, _) => {
                    state.lock().unwrap_or_else(|e| e.into_inner())
                        .push_log(format!("Recall {} is empty — save with [M]", slot + 1));
                }
                _ => {}
            }
        }
        _ => return handle_global(key, state, device, engine, show_help, show_footer, focus_keys),
    }
    KeyAction::Continue
}

// ── Global keys (no panel focus) ─────────────────────────────────────────────

fn handle_global(
    key: KeyEvent,
    state: &Arc<Mutex<SdrMetrics>>,
    device: Option<&Arc<dyn hardware::SdrDevice>>,
    engine: &mut ui::LayoutEngine,
    show_help: &mut bool,
    show_footer: &mut bool,
    focus_keys: &HashMap<char, &'static str>,
) -> KeyAction {
    match key.code {
        KeyCode::Esc => {
            if engine.focused_panel_name().is_some() {
                engine.clear_focus();
                let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
                m.ui.focused_panel = None;
                m.ui.focused_panel_bindings = &[];
                m.ui.log_overlay = false;
                m.spectrum.cursor_freq = None;
                m.waterfall.scroll_offset = 0;
                m.waterfall.cursor_freq = None;
            }
        }
        KeyCode::Char('q') => return KeyAction::Quit,
        KeyCode::Char(' ') if device.is_some() => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            m.radio.rx_enabled = !m.radio.rx_enabled;
        }
        KeyCode::Char('r') => {
            use crate::state::{DEFAULT_LNA_GAIN, DEFAULT_VGA_GAIN};
            if let Some(device) = device {
                // Reset to the active device's own defaults so RTL-SDR lands on a
                // legal freq/rate instead of HackRF's 2.4 GHz / 10 Msps.
                let caps = device.capabilities();
                let def_freq = caps.default_frequency_hz;
                let def_sr   = caps.default_sample_rate_hz;
                // Snap the default gains into this device's gain model so RTL-SDR
                // lands on a legal tuner step, not HackRF's raw LNA/VGA constants.
                let (lna_def, vga_def) = caps.gain.clamp_gains(DEFAULT_LNA_GAIN, DEFAULT_VGA_GAIN);
                let (sr_result, bb_bw) = match device.set_sample_rate(def_sr) {
                    Ok(bw) => (Ok(()), bw),
                    Err(e) => (Err(e), crate::hardware::compute_bb_filter_bw(def_sr)),
                };
                let results = [
                    device.set_lna_gain(lna_def),
                    device.set_vga_gain(vga_def),
                    device.set_frequency(def_freq),
                    sr_result,
                    device.set_amp_enable(false),
                ];
                let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
                if results.iter().all(|r| r.is_ok()) {
                    m.radio.lna_gain           = lna_def;
                    m.radio.vga_gain           = vga_def;
                    m.radio.amp_enabled        = false;
                    m.lab.rf_autotrack         = false;
                    m.radio.frequency          = def_freq;
                    m.radio.config_sample_rate = def_sr;
                    m.radio.bb_filter_hz       = bb_bw;
                    m.push_log("Settings reset to defaults");
                } else {
                    for r in &results {
                        if let Err(e) = r { m.push_log(format!("Reset error: {}", e)); }
                    }
                }
            }
        }
        KeyCode::Char('f') if device.is_some() => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            m.ui.input_mode = InputMode::FrequencyInput;
            m.ui.input_buf.clear();
            m.push_log("Enter frequency in MHz, then press Enter");
        }
        KeyCode::Char('s') if device.is_some() => {
            let (lo, hi) = {
                let c = device.unwrap().capabilities();
                (c.sample_rate_min_hz / 1e6, c.sample_rate_max_hz / 1e6)
            };
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            m.ui.input_mode = InputMode::SampleRateInput;
            m.ui.input_buf.clear();
            m.push_log(format!("Enter sample rate in MHz ({:.1}–{:.1}), then press Enter", lo, hi));
        }
        KeyCode::Char('?') => *show_help = !*show_help,
        KeyCode::Tab       => *show_footer = !*show_footer,
        KeyCode::Char('p') => {
            engine.cycle_preset();
            let name = engine.active_preset().to_string();
            state.lock().unwrap_or_else(|e| e.into_inner()).push_log(format!("Preset: {}", name));
        }
        KeyCode::Char('1') => { engine.set_preset("command_rail");     state.lock().unwrap_or_else(|e| e.into_inner()).push_log("Preset: command rail"); }
        KeyCode::Char('2') => { engine.set_preset("spectrum");         state.lock().unwrap_or_else(|e| e.into_inner()).push_log("Preset: spectrum"); }
        KeyCode::Char('3') => { engine.set_preset("waterfall");        state.lock().unwrap_or_else(|e| e.into_inner()).push_log("Preset: waterfall"); }
        KeyCode::Char('4') => { engine.set_preset("spectrum_waterfall"); state.lock().unwrap_or_else(|e| e.into_inner()).push_log("Preset: spectrum+waterfall"); }
        // Lab family on [5]–[8]. Each lights up automatically once its preset is
        // defined; until then it logs without switching.
        KeyCode::Char('5') => { try_set_preset(engine, state, "lab_iq"); }
        KeyCode::Char('6') => { try_set_preset(engine, state, "lab_rf"); }
        KeyCode::Char('7') => { try_set_preset(engine, state, "lab_timing"); }
        KeyCode::Char('8') => { try_set_preset(engine, state, "lab_signal"); }
        // [9] reserved for the future lab_sweep (Phase 6); pre-wired so it activates
        // the moment that preset exists.
        KeyCode::Char('9') => { try_set_preset(engine, state, "lab_sweep"); }
        KeyCode::Char('0') => { cycle_micro(engine, state); }
        KeyCode::Char('w') => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            m.waterfall.buffer.paused = !m.waterfall.buffer.paused;
            let s = if m.waterfall.buffer.paused { "paused" } else { "resumed" };
            m.push_log(format!("Waterfall {}", s));
        }
        KeyCode::Char('h') => {
            let held = {
                let m = state.lock().unwrap_or_else(|e| e.into_inner());
                m.waterfall.last_fft.as_ref().map(|fr| Arc::clone(&fr.bins_dbfs))
            };
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            if m.spectrum.hold.is_some() {
                m.spectrum.hold = None;
                m.push_log("Hold: off");
            } else if let Some(bins) = held {
                m.spectrum.hold = Some(bins);
                m.push_log("Hold: on — ghost spectrum frozen");
            }
        }
        KeyCode::Up => {
            if let Some(device) = device {
                let gain = &device.capabilities().gain;
                let cur = { state.lock().unwrap_or_else(|e| e.into_inner()).radio.lna_gain };
                let new_gain = next_primary_gain(gain, cur, true);
                let result = device.set_lna_gain(new_gain);
                let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
                match result {
                    Ok(()) => {
                        m.radio.lna_gain = new_gain;
                        // On a single-tuner device, setting a manual gain turns the
                        // tuner AGC off in hardware — keep the UI's AGC flag in sync.
                        if gain.is_single() { m.radio.amp_enabled = false; }
                        m.lab.rf_autotrack = false; m.ui.note_mode_action(RailMode::Bench);
                        m.push_log(format!("{} gain → {} dB", primary_gain_label(gain), new_gain));
                    }
                    Err(e) => m.push_log(format!("Gain error: {}", e)),
                }
            }
        }
        KeyCode::Down => {
            if let Some(device) = device {
                let gain = &device.capabilities().gain;
                let cur = { state.lock().unwrap_or_else(|e| e.into_inner()).radio.lna_gain };
                let new_gain = next_primary_gain(gain, cur, false);
                let result = device.set_lna_gain(new_gain);
                let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
                match result {
                    Ok(()) => {
                        m.radio.lna_gain = new_gain;
                        // On a single-tuner device, setting a manual gain turns the
                        // tuner AGC off in hardware — keep the UI's AGC flag in sync.
                        if gain.is_single() { m.radio.amp_enabled = false; }
                        m.lab.rf_autotrack = false; m.ui.note_mode_action(RailMode::Bench);
                        m.push_log(format!("{} gain → {} dB", primary_gain_label(gain), new_gain));
                    }
                    Err(e) => m.push_log(format!("Gain error: {}", e)),
                }
            }
        }
        // VGA is HackRF-only; on a single-tuner device (RTL-SDR) these keys no-op.
        KeyCode::Char('[') => {
            if let Some(device) = device {
                if matches!(device.capabilities().gain, hardware::GainModel::HackRf) {
                    let new_gain = { let m = state.lock().unwrap_or_else(|e| e.into_inner()); m.radio.vga_gain.saturating_sub(2) };
                    let result = device.set_vga_gain(new_gain);
                    let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
                    match result {
                        Ok(()) => { m.radio.vga_gain = new_gain; m.lab.rf_autotrack = false; m.ui.note_mode_action(RailMode::Bench); m.push_log(format!("VGA gain → {} dB", new_gain)); }
                        Err(e) => m.push_log(format!("VGA gain error: {}", e)),
                    }
                }
            }
        }
        KeyCode::Char(']') => {
            if let Some(device) = device {
                if matches!(device.capabilities().gain, hardware::GainModel::HackRf) {
                    let new_gain = { let m = state.lock().unwrap_or_else(|e| e.into_inner()); (m.radio.vga_gain + 2).min(62) };
                    let result = device.set_vga_gain(new_gain);
                    let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
                    match result {
                        Ok(()) => { m.radio.vga_gain = new_gain; m.lab.rf_autotrack = false; m.ui.note_mode_action(RailMode::Bench); m.push_log(format!("VGA gain → {} dB", new_gain)); }
                        Err(e) => m.push_log(format!("VGA gain error: {}", e)),
                    }
                }
            }
        }
        KeyCode::Char('a') => {
            if let Some(device) = device {
                // `amp_enabled` doubles as the front-end-boost toggle: HackRF's RF
                // amp, RTL-SDR's tuner AGC. The label follows the gain model.
                let is_rtl = matches!(device.capabilities().gain, hardware::GainModel::RtlSingle { .. });
                let new_state = { let m = state.lock().unwrap_or_else(|e| e.into_inner()); !m.radio.amp_enabled };
                let result = if is_rtl { device.set_tuner_agc(new_state) } else { device.set_amp_enable(new_state) };
                let label = if is_rtl { "AGC" } else { "AMP" };
                let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
                match result {
                    Ok(()) => {
                        m.radio.amp_enabled = new_state;
                        m.lab.rf_autotrack = false;
                        m.ui.note_mode_action(RailMode::Bench);
                        m.push_log(format!("{} {}", label, if new_state { "ON" } else { "OFF" }));
                    }
                    Err(e) => m.push_log(format!("{} error: {}", label, e)),
                }
            }
        }
        KeyCode::Char(c) => {
            if let Some(&panel_name) = focus_keys.get(&c) {
                if engine.is_panel_visible(panel_name) {
                    engine.focus(panel_name);
                    let bindings = engine.get_panel_bindings(panel_name);
                    let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
                    m.ui.focused_panel = Some(panel_name.to_string());
                    m.ui.focused_panel_bindings = bindings;
                }
            }
        }
        _ => {}
    }
    KeyAction::Continue
}

fn handle_global_no_device(
    key: KeyEvent,
    state: &Arc<Mutex<SdrMetrics>>,
    engine: &mut ui::LayoutEngine,
    show_help: &mut bool,
    show_footer: &mut bool,
    focus_keys: &HashMap<char, &'static str>,
) -> KeyAction {
    handle_global(key, state, None, engine, show_help, show_footer, focus_keys)
}

/// Switch to `name` if the preset is defined, otherwise log that it is not yet
/// available. This keeps the number-key framework (`[6]`–`[9]`, `[0]`) in place
/// before the presets themselves exist, so each one activates the moment it is
/// added to the layout config.
fn try_set_preset(engine: &mut ui::LayoutEngine, state: &Arc<Mutex<SdrMetrics>>, name: &str) -> KeyAction {
    let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
    if engine.has_preset(name) {
        engine.set_preset(name);
        m.push_log(format!("Preset: {}", name));
    } else {
        m.push_log(format!("Preset '{}' not yet available", name));
    }
    KeyAction::Continue
}

/// The `[0]` micro-ecosystem cycle. Entering from a non-micro preset lands on
/// `micro_main`; pressing `[0]` again while already in a micro preset advances
/// to the next view. A target whose preset is not yet defined is logged and
/// skipped (micro_view does not advance), so the cycle never strands the user on
/// a blank view while the micro presets are still being built out.
fn cycle_micro(engine: &mut ui::LayoutEngine, state: &Arc<Mutex<SdrMetrics>>) {
    // The sweep step is part of the cycle: entering micro_sweep starts a scan.
    const SWEEP_ACTIVE: bool = true;
    let in_micro = engine.active_preset().starts_with("micro_");
    let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
    let target = if in_micro { m.ui.micro_view.next(SWEEP_ACTIVE) } else { MicroView::Main };
    if engine.has_preset(target.preset_name()) {
        m.ui.micro_view = target;
        engine.set_preset(target.preset_name());
        m.push_log(format!("Micro: {} ({}/{})", target.label(), target.position(), MicroView::total(SWEEP_ACTIVE)));
    } else {
        m.push_log(format!("Preset '{}' not yet available", target.preset_name()));
    }
}

// ── Text input modes ──────────────────────────────────────────────────────────

fn handle_freq_input(
    key: KeyEvent,
    state: &Arc<Mutex<SdrMetrics>>,
    device: Option<&Arc<dyn hardware::SdrDevice>>,
) {
    match key.code {
        KeyCode::Esc => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            m.ui.input_mode = InputMode::Normal;
            m.ui.input_buf.clear();
            m.push_log("Frequency input cancelled");
        }
        KeyCode::Backspace => { state.lock().unwrap_or_else(|e| e.into_inner()).ui.input_buf.pop(); }
        KeyCode::Char(c) if c.is_ascii_digit() || c == '.' => {
            state.lock().unwrap_or_else(|e| e.into_inner()).ui.input_buf.push(c);
        }
        KeyCode::Enter => {
            if let Some(device) = device {
                let caps = device.capabilities();
                // Clamp into the tuning range rather than rejecting (matches the
                // arrow-key tuning, which already clamps).
                let freq_hz: Option<u64> = {
                    let m = state.lock().unwrap_or_else(|e| e.into_inner());
                    m.ui.input_buf.parse::<f64>().ok()
                        .filter(|&mhz| mhz > 0.0)
                        .map(|mhz| ((mhz * 1_000_000.0) as u64).clamp(caps.freq_min_hz, caps.freq_max_hz))
                };
                let result = freq_hz.map(|hz| device.set_frequency(hz));
                let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
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

fn handle_sr_input(
    key: KeyEvent,
    state: &Arc<Mutex<SdrMetrics>>,
    device: Option<&Arc<dyn hardware::SdrDevice>>,
) {
    match key.code {
        KeyCode::Esc => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            m.ui.input_mode = InputMode::Normal;
            m.ui.input_buf.clear();
            m.push_log("Sample rate input cancelled");
        }
        KeyCode::Backspace => { state.lock().unwrap_or_else(|e| e.into_inner()).ui.input_buf.pop(); }
        KeyCode::Char(c) if c.is_ascii_digit() || c == '.' => {
            state.lock().unwrap_or_else(|e| e.into_inner()).ui.input_buf.push(c);
        }
        KeyCode::Enter => {
            if let Some(device) = device {
                let caps = device.capabilities();
                let lo_hz = caps.sample_rate_min_hz;
                let hi_hz = caps.sample_rate_max_hz;
                // Clamp into the device's legal range rather than rejecting, so a
                // boundary entry like "0.9" on RTL-SDR snaps up to a valid rate.
                let rate_hz: Option<f64> = {
                    let m = state.lock().unwrap_or_else(|e| e.into_inner());
                    m.ui.input_buf.parse::<f64>().ok()
                        .filter(|&mhz| mhz > 0.0)
                        .map(|mhz| (mhz * 1_000_000.0).clamp(lo_hz, hi_hz))
                };
                // Release lock before calling device — set_sample_rate is a
                // blocking USB control transfer; holding the mutex here deadlocks the
                // rx_callback thread that needs the same lock to return.
                let result = rate_hz.map(|hz| device.set_sample_rate(hz));
                let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
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
fn handle_sweep_range_input(key: KeyEvent, state: &Arc<Mutex<SdrMetrics>>, is_start: bool) {
    match key.code {
        KeyCode::Esc => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            m.ui.input_mode = InputMode::Normal;
            m.ui.input_buf.clear();
            m.push_log("Sweep range input cancelled");
        }
        KeyCode::Backspace => { state.lock().unwrap_or_else(|e| e.into_inner()).ui.input_buf.pop(); }
        KeyCode::Char(c) if c.is_ascii_digit() || c == '.' => {
            state.lock().unwrap_or_else(|e| e.into_inner()).ui.input_buf.push(c);
        }
        KeyCode::Enter => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
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

fn handle_marker_input(key: KeyEvent, state: &Arc<Mutex<SdrMetrics>>) {
    match key.code {
        KeyCode::Esc => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            m.ui.input_mode = InputMode::Normal;
            m.ui.input_buf.clear();
            m.spectrum.pending_marker = None;
            m.push_log("Marker cancelled");
        }
        KeyCode::Backspace => { state.lock().unwrap_or_else(|e| e.into_inner()).ui.input_buf.pop(); }
        KeyCode::Char(c) => { state.lock().unwrap_or_else(|e| e.into_inner()).ui.input_buf.push(c); }
        KeyCode::Enter => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
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
