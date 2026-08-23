//! The two primary plots' focus keys.
//!
//! `[E]` on the spectrum tunes, steps, zooms and places markers; `[L]` on the
//! waterfall scrolls history, sets the row stride and moves the cursor. Both
//! fall through to the global handler for anything they do not claim.


use crossterm::event::{KeyCode, KeyEvent};

use crate::state::{InputMode, RailMode};
use crate::ui::widgets::micro_common::fmt_bw;
use crate::ui::panels::core::spectrum::{fmt_spectrum_step, next_spectrum_step, prev_spectrum_step};
use crate::ui::panels::core::waterfall::{next_wf_stride, prev_wf_stride, next_wf_zoom, prev_wf_zoom};

use super::{InputCtx, KeyAction, global, metrics};

// ── Spectrum focus keys ───────────────────────────────────────────────────────

pub(super) fn spectrum(key: KeyEvent, ctx: &mut InputCtx<'_>) -> KeyAction {
    let (state, device) = (ctx.state, ctx.device);
    match key.code {
        KeyCode::Left => {
            if let Some(device) = device {
                let fmin = device.capabilities().freq_min_hz;
                let new_freq = {
                    let m = metrics(state);
                    m.radio.frequency.saturating_sub(m.spectrum.step_hz).max(fmin)
                };
                let result = device.set_frequency(new_freq);
                let mut m = metrics(state);
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
                    let m = metrics(state);
                    (m.radio.frequency + m.spectrum.step_hz).min(fmax)
                };
                let result = device.set_frequency(new_freq);
                let mut m = metrics(state);
                match result {
                    Ok(()) => { m.radio.frequency = new_freq; m.ui.note_mode_action(RailMode::Hunt); }
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
        // Shared frequency zoom — in the bonded spectrum+waterfall view both plots
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
            let new_min = (m.spectrum.y_min + 10.0).min(m.spectrum.y_max - 20.0);
            m.spectrum.y_min = new_min;
            let ymax = m.spectrum.y_max;
            m.push_log(format!("Zoom: {:.0}…{:.0} dBFS", new_min, ymax));
        }
        KeyCode::Down => {
            let mut m = metrics(state);
            let new_min = (m.spectrum.y_min - 10.0).max(-120.0);
            m.spectrum.y_min = new_min;
            let ymax = m.spectrum.y_max;
            m.push_log(format!("Zoom: {:.0}…{:.0} dBFS", new_min, ymax));
        }
        KeyCode::Char('j') => {
            let mut m = metrics(state);
            let step = m.spectrum.step_hz;
            m.spectrum.cursor_freq = Some(match m.spectrum.cursor_freq {
                Some(f) => f.saturating_sub(step).max(m.caps.freq_min_hz),
                None    => m.radio.frequency,
            });
        }
        KeyCode::Char('k') => {
            let mut m = metrics(state);
            let step = m.spectrum.step_hz;
            m.spectrum.cursor_freq = Some(match m.spectrum.cursor_freq {
                Some(f) => (f + step).min(m.caps.freq_max_hz),
                None    => m.radio.frequency,
            });
        }
        KeyCode::Char('m') => {
            let (marker_freq, existing_idx) = {
                let m = metrics(state);
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
            let new_min = (m.waterfall.db_min + 10.0).min(-20.0);
            m.waterfall.db_min = new_min;
            m.push_log(format!("Waterfall zoom: {:.0}…0 dBFS", new_min));
        }
        KeyCode::Down => {
            let mut m = metrics(state);
            let new_min = (m.waterfall.db_min - 10.0).max(-120.0);
            m.waterfall.db_min = new_min;
            m.push_log(format!("Waterfall zoom: {:.0}…0 dBFS", new_min));
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
                m.waterfall.cursor_freq = Some(cf.saturating_sub(m.spectrum.step_hz).max(m.caps.freq_min_hz));
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

