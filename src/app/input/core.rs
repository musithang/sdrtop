// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The two primary plots' focus keys.
//!
//! `[E]` on the spectrum tunes, steps, zooms and places markers; `[L]` on the
//! waterfall scrolls history, sets the row stride and moves the cursor. Both
//! fall through to the global handler for anything they do not claim.

use crossterm::event::{KeyCode, KeyEvent};

use crate::state::{InputMode, RailMode};
use crate::ui::panels::core::spectrum::{
    fmt_spectrum_step, next_spectrum_step, prev_spectrum_step,
};
use crate::ui::panels::core::waterfall::{
    next_wf_stride, next_wf_zoom, prev_wf_stride, prev_wf_zoom,
};
use crate::ui::widgets::micro_common::fmt_bw;

use super::{global, metrics, InputCtx, KeyAction};

fn strongest_bin_frequency(frame: &crate::state::FftFrame) -> Option<u64> {
    let peak_bin = frame
        .bins_dbfs
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(index, _)| index)?;
    frame
        .frequency_of_bin(peak_bin)
        .filter(|frequency| *frequency >= 0.0)
        .map(|frequency| frequency.round() as u64)
}
const LEVEL_ZOOM_STEP_DB: f32 = 10.0;
const LEVEL_MIN_WINDOW_DB: f32 = 20.0;
fn adjust_level_floor(
    floor: f32,
    ceiling: f32,
    delta: f32,
    caps: &crate::hardware::DeviceCapabilities,
) -> f32 {
    let window = LEVEL_MIN_WINDOW_DB.min(caps.level_max_db - caps.level_min_db);
    let highest_floor = (ceiling.min(caps.level_max_db) - window).max(caps.level_min_db);
    (floor + delta).clamp(caps.level_min_db, highest_floor)
}

// ── Spectrum focus keys ───────────────────────────────────────────────────────

pub(super) fn spectrum(key: KeyEvent, ctx: &mut InputCtx<'_>) -> KeyAction {
    let (state, device) = (ctx.state, ctx.device);
    match key.code {
        KeyCode::Left => {
            if let Some(device) = device {
                let caps = device.capabilities();
                let new_freq = {
                    let m = metrics(state);
                    caps.step_within_band(m.radio.frequency, m.spectrum.step_hz, false)
                };
                let result = device.set_frequency(new_freq);
                let mut m = metrics(state);
                match result {
                    Ok(()) => {
                        m.radio.frequency = new_freq;
                        m.ui.note_mode_action(RailMode::Hunt);
                    }
                    Err(e) => m.push_log(format!("Tune error: {}", e)),
                }
            }
        }
        KeyCode::Right => {
            if let Some(device) = device {
                let caps = device.capabilities();
                let new_freq = {
                    let m = metrics(state);
                    caps.step_within_band(m.radio.frequency, m.spectrum.step_hz, true)
                };
                let result = device.set_frequency(new_freq);
                let mut m = metrics(state);
                match result {
                    Ok(()) => {
                        m.radio.frequency = new_freq;
                        m.ui.note_mode_action(RailMode::Hunt);
                    }
                    Err(e) => m.push_log(format!("Tune error: {}", e)),
                }
            }
        }
        KeyCode::Char('[') => {
            let mut m = metrics(state);
            let new_step = prev_spectrum_step(m.spectrum.step_hz);
            m.spectrum.step_hz = new_step;
            m.push_log(format!("Step → {}", fmt_spectrum_step(new_step)));
        }
        KeyCode::Char(']') => {
            let mut m = metrics(state);
            let new_step = next_spectrum_step(m.spectrum.step_hz);
            m.spectrum.step_hz = new_step;
            m.push_log(format!("Step → {}", fmt_spectrum_step(new_step)));
        }
        // Shared frequency zoom - in the bonded spectrum+waterfall view both plots
        // share one span, so `+`/`-` here drive the same `hz_zoom` the waterfall
        // does, narrowing the whole instrument together.
        KeyCode::Char('+') | KeyCode::Char('=') => {
            let mut m = metrics(state);
            let new_zoom = next_wf_zoom(m.waterfall.hz_zoom);
            m.waterfall.hz_zoom = new_zoom;
            m.push_log(format!("Freq zoom: ×{}", new_zoom));
        }
        KeyCode::Char('-') => {
            let mut m = metrics(state);
            let new_zoom = prev_wf_zoom(m.waterfall.hz_zoom);
            m.waterfall.hz_zoom = new_zoom;
            if new_zoom == 1 {
                m.push_log("Freq zoom: off".to_string());
            } else {
                m.push_log(format!("Freq zoom: ×{}", new_zoom));
            }
        }
        KeyCode::Up => {
            let mut m = metrics(state);
            let new_min = adjust_level_floor(
                m.spectrum.y_min,
                m.spectrum.y_max,
                LEVEL_ZOOM_STEP_DB,
                &m.caps,
            );
            m.spectrum.y_min = new_min;
            let ymax = m.spectrum.y_max;
            let unit = m.caps.level_unit.label();
            m.push_log(format!("Zoom: {new_min:.0}\u{2026}{ymax:.0} {unit}"));
        }
        KeyCode::Down => {
            let mut m = metrics(state);
            let new_min = adjust_level_floor(
                m.spectrum.y_min,
                m.spectrum.y_max,
                -LEVEL_ZOOM_STEP_DB,
                &m.caps,
            );
            m.spectrum.y_min = new_min;
            let ymax = m.spectrum.y_max;
            let unit = m.caps.level_unit.label();
            m.push_log(format!("Zoom: {new_min:.0}\u{2026}{ymax:.0} {unit}"));
        }
        KeyCode::Char('j') => {
            let mut m = metrics(state);
            let step = m.spectrum.step_hz;
            m.spectrum.cursor_freq = Some(match m.spectrum.cursor_freq {
                Some(f) => f.saturating_sub(step).max(m.caps.freq_min_hz),
                None => m.radio.frequency,
            });
        }
        KeyCode::Char('k') => {
            let mut m = metrics(state);
            let step = m.spectrum.step_hz;
            m.spectrum.cursor_freq = Some(match m.spectrum.cursor_freq {
                Some(f) => (f + step).min(m.caps.freq_max_hz),
                None => m.radio.frequency,
            });
        }
        KeyCode::Char('m') => {
            let (marker_freq, existing_idx) = {
                let m = metrics(state);
                let freq = if let Some(f) = m.spectrum.cursor_freq {
                    f
                } else if let Some(frame) = &m.waterfall.last_fft {
                    strongest_bin_frequency(frame).unwrap_or(m.radio.frequency)
                } else {
                    m.radio.frequency
                };
                let step = m.spectrum.step_hz;
                let idx = m
                    .spectrum
                    .markers
                    .iter()
                    .position(|mk| (mk.freq_hz as i64 - freq as i64).unsigned_abs() < step);
                (freq, idx)
            };
            let mut m = metrics(state);
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
            let mut m = metrics(state);
            let cursor = m.spectrum.cursor_freq.unwrap_or(m.radio.frequency);
            let step = m.spectrum.step_hz;
            if let Some(mk) = m
                .spectrum
                .markers
                .iter_mut()
                .min_by_key(|mk| (mk.freq_hz as i64 - cursor as i64).unsigned_abs())
                .filter(|mk| (mk.freq_hz as i64 - cursor as i64).unsigned_abs() < step * 4)
            {
                let next = match mk.channel_bw_hz {
                    None => Some(BW_STEPS[0]),
                    Some(cur) => {
                        let idx = BW_STEPS.iter().position(|&b| b == cur);
                        idx.and_then(|i| BW_STEPS.get(i + 1)).copied()
                    }
                };
                mk.channel_bw_hz = next;
                mk.measured_bw_hz = None;
                let msg = match next {
                    Some(bw) => format!("Marker '{}' channel BW → {}", mk.label, fmt_bw(bw)),
                    None => format!("Marker '{}' channel BW cleared", mk.label),
                };
                m.push_log(msg);
            } else {
                m.push_log("No marker near cursor — place one with [M] first");
            }
        }
        // `D` cycles the trace render style (braille → fill → scatter); persisted.
        KeyCode::Char('d') => {
            let mut m = metrics(state);
            let next = m.spectrum.style.next();
            m.spectrum.style = next;
            m.push_log(format!("Spectrum style: {}", next.label()));
        }
        // All other keys fall through to global handler
        _ => return global::handle(key, ctx),
    }
    KeyAction::Continue
}

// ── Waterfall focus keys ──────────────────────────────────────────────────────

pub(super) fn waterfall(key: KeyEvent, ctx: &mut InputCtx<'_>) -> KeyAction {
    let state = ctx.state;
    match key.code {
        KeyCode::Up => {
            let mut m = metrics(state);
            let new_min = adjust_level_floor(
                m.waterfall.db_min,
                m.waterfall.db_max,
                LEVEL_ZOOM_STEP_DB,
                &m.caps,
            );
            m.waterfall.db_min = new_min;
            let max = m.waterfall.db_max;
            let unit = m.caps.level_unit.label();
            m.push_log(format!(
                "Waterfall zoom: {new_min:.0}\u{2026}{max:.0} {unit}"
            ));
        }
        KeyCode::Down => {
            let mut m = metrics(state);
            let new_min = adjust_level_floor(
                m.waterfall.db_min,
                m.waterfall.db_max,
                -LEVEL_ZOOM_STEP_DB,
                &m.caps,
            );
            m.waterfall.db_min = new_min;
            let max = m.waterfall.db_max;
            let unit = m.caps.level_unit.label();
            m.push_log(format!(
                "Waterfall zoom: {new_min:.0}\u{2026}{max:.0} {unit}"
            ));
        }
        KeyCode::Char('[') => {
            let mut m = metrics(state);
            let new_stride = prev_wf_stride(m.waterfall.buffer.row_stride);
            m.waterfall.buffer.set_row_stride(new_stride);
            m.push_log(format!("Waterfall: ×{} frames/row", new_stride));
        }
        KeyCode::Char(']') => {
            let mut m = metrics(state);
            let new_stride = next_wf_stride(m.waterfall.buffer.row_stride);
            m.waterfall.buffer.set_row_stride(new_stride);
            m.push_log(format!("Waterfall: ×{} frames/row", new_stride));
        }
        KeyCode::Char('+') | KeyCode::Char('=') => {
            let mut m = metrics(state);
            let new_zoom = next_wf_zoom(m.waterfall.hz_zoom);
            m.waterfall.hz_zoom = new_zoom;
            m.push_log(format!("Waterfall zoom: ×{}", new_zoom));
        }
        KeyCode::Char('-') => {
            let mut m = metrics(state);
            let new_zoom = prev_wf_zoom(m.waterfall.hz_zoom);
            m.waterfall.hz_zoom = new_zoom;
            if new_zoom == 1 {
                m.push_log("Waterfall zoom: off".to_string());
            } else {
                m.push_log(format!("Waterfall zoom: ×{}", new_zoom));
            }
        }
        KeyCode::Char('m') => {
            let mut m = metrics(state);
            m.waterfall.cursor_freq = if m.waterfall.cursor_freq.is_some() {
                None
            } else {
                Some(m.radio.frequency)
            };
        }
        KeyCode::Left => {
            let mut m = metrics(state);
            if let Some(cf) = m.waterfall.cursor_freq {
                m.waterfall.cursor_freq = Some(
                    cf.saturating_sub(m.spectrum.step_hz)
                        .max(m.caps.freq_min_hz),
                );
            }
        }
        KeyCode::Right => {
            let mut m = metrics(state);
            if let Some(cf) = m.waterfall.cursor_freq {
                m.waterfall.cursor_freq = Some((cf + m.spectrum.step_hz).min(m.caps.freq_max_hz));
            }
        }
        KeyCode::Char('j') => {
            let mut m = metrics(state);
            let max = m.waterfall.buffer.rows.len() / 2;
            m.waterfall.scroll_offset = (m.waterfall.scroll_offset + 1).min(max);
        }
        KeyCode::Char('k') => {
            let mut m = metrics(state);
            m.waterfall.scroll_offset = m.waterfall.scroll_offset.saturating_sub(1);
        }
        // `P` cycles the colour gradient (classic → amber → ice → phosphor). The
        // choice persists to `[display] waterfall_palette` on quit.
        KeyCode::Char('p') => {
            let mut m = metrics(state);
            let next = m.waterfall.palette.next();
            m.waterfall.palette = next;
            m.push_log(format!("Waterfall palette: {}", next.label()));
        }
        _ => return global::handle_no_device(key, ctx),
    }
    KeyAction::Continue
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::time::Instant;

    use super::*;
    use crate::state::{BinAxis, FftFrame, SdrMetrics};
    use crate::ui::{LayoutEngine, PanelRegistry};

    #[test]
    fn peak_jump_uses_the_captured_fft_frequency_after_retuning() {
        let mut state = SdrMetrics::fixture();
        state.radio.frequency = 100_000_000;
        state.radio.config_sample_rate = 32_000_000.0;
        let mut state = state.with_carrier(8_000_000.0, 70.0);
        state.radio.frequency = 200_000_000;
        let frame = state.waterfall.last_fft.as_ref().unwrap();
        assert_eq!(strongest_bin_frequency(frame), Some(108_000_000));
        assert_eq!(frame.bin_axis, BinAxis::FftBins);
        assert_eq!(frame.window(4).unwrap().span_hz, 8_000_000.0);
    }

    #[test]
    fn peak_jump_rejects_an_empty_frame() {
        let state = SdrMetrics::fixture().with_carrier(0.0, 70.0);
        let mut frame = state.waterfall.last_fft.unwrap();
        frame.bins_dbfs = Arc::new(Vec::new());
        assert_eq!(strongest_bin_frequency(&frame), None);
    }

    #[test]
    fn peak_jump_reaches_the_last_measured_point() {
        let mut bins = vec![-90.0; 64];
        bins[63] = -20.0;
        let bins = Arc::new(bins);
        let frame = FftFrame {
            bins_dbfs: Arc::clone(&bins),
            peak_hold: bins,
            noise_floor: -90.0,
            center_freq_hz: 131_500_000,
            axis_start_hz: 100_000_000.0,
            sample_rate: 63_000_000.0,
            timestamp: Instant::now(),
            peak_to_nf_db: 70.0,
            occupied_bw_hz: 0,
            enbw_hz: 0.0,
            bin_axis: BinAxis::MeasuredPoints,
        };

        assert_eq!(strongest_bin_frequency(&frame), Some(163_000_000));
    }

    #[test]
    fn peak_jump_falls_back_for_a_negative_frequency() {
        let mut state = crate::state::SdrMetrics::fixture();
        state.radio.frequency = 1_000_000;
        state.radio.config_sample_rate = 32_000_000.0;
        let state = state.with_carrier(-8_000_000.0, 70.0);
        let frame = state.waterfall.last_fft.as_ref().unwrap();
        assert_eq!(strongest_bin_frequency(frame), None);
        assert_eq!(
            strongest_bin_frequency(frame).unwrap_or(state.radio.frequency),
            1_000_000
        );
    }

    #[test]
    fn either_zoom_direction_keeps_the_floor_within_device_limits() {
        let mut caps = crate::hardware::native::hackrf::caps();
        for (min, max) in [(-120.0, 0.0), (-10.0, 0.0), (-10.5, -10.0)] {
            caps.level_min_db = min;
            caps.level_max_db = max;
            for delta in [-LEVEL_ZOOM_STEP_DB, LEVEL_ZOOM_STEP_DB] {
                for floor in [min - 100.0, min, max, max + 100.0] {
                    for ceiling in [max - 5.0, max, max + 100.0] {
                        let adjusted = adjust_level_floor(floor, ceiling, delta, &caps);
                        assert!((min..max).contains(&adjusted));
                        assert!(adjusted <= max - (max - min).min(20.0));
                    }
                }
            }
        }
    }

    #[test]
    fn level_zoom_uses_device_bounds_and_preserves_iq_log_text() {
        for (min, max) in [
            (-120.0_f32, 0.0),
            (-120.0, 20.0),
            (-110.0, -10.0),
            (-10.0, 0.0),
            (-10.5, -10.0),
        ] {
            let highest_floor = max - (max - min).min(20.0_f32);
            for is_waterfall in [false, true] {
                let mut m = SdrMetrics::fixture();
                Arc::make_mut(&mut m.caps).level_min_db = min;
                Arc::make_mut(&mut m.caps).level_max_db = max;
                m.spectrum.y_min = min;
                m.spectrum.y_max = max;
                m.waterfall.db_min = min;
                m.waterfall.db_max = max;
                let state = Arc::new(Mutex::new(m));
                let mut engine = LayoutEngine::new(
                    crate::config::LayoutConfig::default_config(),
                    PanelRegistry::new(),
                );
                let mut show_footer = true;
                let focus_keys = HashMap::new();
                let mut ctx = InputCtx {
                    state: &state,
                    device: None,
                    engine: &mut engine,
                    show_footer: &mut show_footer,
                    focus_keys: &focus_keys,
                };
                let prefix = if is_waterfall {
                    "Waterfall zoom"
                } else {
                    "Zoom"
                };
                let mut press = |code| {
                    let key = KeyEvent::new(code, crossterm::event::KeyModifiers::NONE);
                    if is_waterfall {
                        waterfall(key, &mut ctx);
                    } else {
                        spectrum(key, &mut ctx);
                    }
                };
                press(KeyCode::Up);
                assert_eq!(
                    metrics(&state).ui.log.back().unwrap().text.as_ref(),
                    format!(
                        "{prefix}: {:.0}\u{2026}{max:.0} dBFS",
                        (min + 10.0).min(highest_floor)
                    )
                );
                for _ in 0..20 {
                    press(KeyCode::Up);
                }
                {
                    let m = metrics(&state);
                    let floor = if is_waterfall {
                        m.waterfall.db_min
                    } else {
                        m.spectrum.y_min
                    };
                    assert_eq!(floor, highest_floor);
                }
                for _ in 0..20 {
                    press(KeyCode::Down);
                }
                let m = metrics(&state);
                let floor = if is_waterfall {
                    m.waterfall.db_min
                } else {
                    m.spectrum.y_min
                };
                assert_eq!(floor, min);
                assert_eq!(
                    m.ui.log.back().unwrap().text.as_ref(),
                    format!("{prefix}: {min:.0}\u{2026}{max:.0} dBFS")
                );
            }
        }
    }
}
