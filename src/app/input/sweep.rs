//! `[G]` sweep-panel focus: band entry, dwell, and starting or stopping a sweep.

use crossterm::event::{KeyCode, KeyEvent};

use crate::state::InputMode;

use super::{global, metrics, InputCtx, KeyAction};

/// `sweep_panel` focus (`[G]`): cursor with `←/→`, peak/mean with `M`, dwell with
/// `+/-`, and `[Enter]` to leave the sweep tuned to the cursor frequency.
pub(super) fn sweep_panel(key: KeyEvent, ctx: &mut InputCtx<'_>) -> KeyAction {
    let (state, _device) = (ctx.state, ctx.device);
    /// Cursor step as a fraction of the swept band per key press.
    const CURSOR_STEP: f64 = 0.01;
    match key.code {
        KeyCode::Left => {
            let mut m = metrics(state);
            let cur = m.sweep.cursor_frac.unwrap_or(0.5);
            m.sweep.cursor_frac = Some((cur - CURSOR_STEP).clamp(0.0, 1.0));
        }
        KeyCode::Right => {
            let mut m = metrics(state);
            let cur = m.sweep.cursor_frac.unwrap_or(0.5);
            m.sweep.cursor_frac = Some((cur + CURSOR_STEP).clamp(0.0, 1.0));
        }
        KeyCode::Char('m') => {
            let mut m = metrics(state);
            m.sweep.show_peak = !m.sweep.show_peak;
            let mode = if m.sweep.show_peak { "peak" } else { "mean" };
            m.push_log(format!("Sweep: {} curve", mode));
        }
        KeyCode::Char('+') | KeyCode::Char('=') => {
            let mut m = metrics(state);
            m.sweep.config.dwell_ms = (m.sweep.config.dwell_ms + 50).min(2000);
            let d = m.sweep.config.dwell_ms;
            m.push_log(format!("Sweep dwell → {} ms", d));
        }
        KeyCode::Char('-') => {
            let mut m = metrics(state);
            m.sweep.config.dwell_ms = m.sweep.config.dwell_ms.saturating_sub(50).max(50);
            let d = m.sweep.config.dwell_ms;
            m.push_log(format!("Sweep dwell → {} ms", d));
        }
        KeyCode::Char('c') => {
            let m = metrics(state);
            let msg = if let Some(frame) = m.sweep.current_frame.as_ref() {
                let curve = if m.sweep.show_peak {
                    &frame.peak_dbfs
                } else {
                    &frame.mean_dbfs
                };
                let cursor_str = if let Some(frac) = m.sweep.cursor_frac {
                    let hz = frame.freq_at_fraction(frac);
                    // Find the bin in freq_hz closest to the cursor frequency.
                    let level = frame
                        .freq_hz
                        .iter()
                        .enumerate()
                        .min_by_key(|(_, &f)| f.abs_diff(hz))
                        .and_then(|(i, _)| curve.get(i).copied().filter(|v| v.is_finite()));
                    let db_str = level
                        .map(|v| format!("{:.1} dBFS", v))
                        .unwrap_or_else(|| "\u{2014}".into());
                    format!("cursor {:.3} MHz {} · ", hz as f64 / 1e6, db_str)
                } else {
                    String::new()
                };
                let top = frame
                    .top_peaks(1, 500_000)
                    .into_iter()
                    .next()
                    .map(|(f, v)| format!("top {:.3} MHz {:.1} dBFS", f as f64 / 1e6, v))
                    .unwrap_or_else(|| "no data".into());
                format!(
                    "Sweep snapshot — {}{} · {:.1}–{:.1} MHz ({:.1}s/cycle)",
                    cursor_str,
                    top,
                    frame.start_hz as f64 / 1e6,
                    frame.stop_hz as f64 / 1e6,
                    frame.cycle_duration_ms as f64 / 1000.0,
                )
            } else {
                "Sweep snapshot — no sweep data yet".into()
            };
            drop(m);
            metrics(state).push_log(msg);
        }
        KeyCode::Char('s') => {
            let mut m = metrics(state);
            m.ui.input_mode = InputMode::SweepStartInput;
            m.ui.input_buf.clear();
            m.push_log("Enter sweep START frequency in MHz, then Enter");
        }
        KeyCode::Char('e') => {
            let mut m = metrics(state);
            m.ui.input_mode = InputMode::SweepStopInput;
            m.ui.input_buf.clear();
            m.push_log("Enter sweep STOP frequency in MHz, then Enter");
        }
        KeyCode::Enter => {
            // Resolve the cursor frequency, stash it as the jump target, then leave
            // lab_sweep — the sweep_task tunes there as it stops.
            let target = {
                let m = metrics(state);
                match (m.sweep.cursor_frac, m.sweep.current_frame.as_ref()) {
                    (Some(fr), Some(f)) => Some(f.freq_at_fraction(fr)),
                    _ => None,
                }
            };
            if let Some(hz) = target {
                {
                    let mut m = metrics(state);
                    m.sweep.pending_tune = Some(hz);
                }
                ctx.engine.clear_focus();
                ctx.engine.set_preset("spectrum_waterfall");
                let mut m = metrics(state);
                m.ui.focused_panel = None;
                m.ui.focused_panel_bindings = &[];
                m.push_log(format!("Jumping to {:.3} MHz from sweep…", hz as f64 / 1e6));
            }
        }
        _ => return global::handle(key, ctx),
    }
    KeyAction::Continue
}
