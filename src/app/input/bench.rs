//! Focus keys for the instrument benches: the lab banner, IQ diagnostics, the
//! RF chain, and the two timing panels.
//!
//! What they have in common is that each drives the *radio* rather than the
//! view — gain, calibration, capture — so most of them need the device.

use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent};

use crate::state::RailMode;
use crate::ui;

use super::{global, metrics, InputCtx, KeyAction};

pub(super) fn lab_banner(key: KeyEvent, ctx: &mut InputCtx<'_>) -> KeyAction {
    let (state, _device) = (ctx.state, ctx.device);
    match key.code {
        KeyCode::Up => {
            metrics(state).lab.adjust_ref(1.0);
        }
        KeyCode::Down => {
            metrics(state).lab.adjust_ref(-1.0);
        }
        KeyCode::Char('[') => {
            let mut m = metrics(state);
            m.lab.adjust_avg(-1);
            let n = m.lab.avg_n;
            m.push_log(format!("Averaging: \u{00D7}{n}"));
        }
        KeyCode::Char(']') => {
            let mut m = metrics(state);
            m.lab.adjust_avg(1);
            let n = m.lab.avg_n;
            m.push_log(format!("Averaging: \u{00D7}{n}"));
        }
        KeyCode::Char('r') => {
            let mut m = metrics(state);
            m.lab.ref_dbfs = None;
            m.push_log("Reference level cleared");
        }
        KeyCode::Char('c') => {
            let mut m = metrics(state);
            if m.lab.ref_trace.is_some() {
                m.lab.ref_trace = None;
                m.push_log("Reference trace cleared");
            } else if let Some(bins) = m
                .waterfall
                .last_fft
                .as_ref()
                .map(|fr| Arc::clone(&fr.bins_dbfs))
            {
                m.lab.ref_trace = Some(bins);
                m.push_log("Reference trace captured");
            } else {
                m.push_log("No spectrum frame to capture");
            }
        }
        _ => return global::handle(key, ctx),
    }
    KeyAction::Continue
}

pub(super) fn iq_diagnostics(key: KeyEvent, ctx: &mut InputCtx<'_>) -> KeyAction {
    let (state, _device) = (ctx.state, ctx.device);
    match key.code {
        // [M] — pin / unpin the carrier+image markers (override the live auto-track).
        KeyCode::Char('m') => {
            let mut m = metrics(state);
            let auto = ui::panels::lab::image_scope::carrier_image(&m);
            if m.lab.iq_marker_pin.is_some() {
                m.lab.iq_marker_pin = None;
                m.push_log("IQ markers: auto-tracking carrier/image".to_string());
            } else if let Some(ci) = auto {
                m.lab.iq_marker_pin = Some((ci.carrier_hz, ci.image_hz));
                m.push_log(format!(
                    "IQ markers pinned — carrier {:.3} MHz · image {:.3} MHz · supp {:.1} dB",
                    ci.carrier_hz as f64 / 1e6,
                    ci.image_hz as f64 / 1e6,
                    ci.suppression_db,
                ));
            } else {
                m.push_log("IQ markers: no carrier detected yet".to_string());
            }
            return KeyAction::Continue;
        }
        // [D] — DC-block: subtract the live DC estimate from the stream.
        KeyCode::Char('d') => {
            let mut m = metrics(state);
            m.iq.cal.dc_block_on = !m.iq.cal.dc_block_on;
            let on = m.iq.cal.dc_block_on;
            m.push_log(
                if on {
                    "DC-block ON — subtracting DC offset from the stream"
                } else {
                    "DC-block OFF"
                }
                .to_string(),
            );
            return KeyAction::Continue;
        }
        // [C] — auto-cal: capture (or clear) the I/Q quadrature correction.
        KeyCode::Char('c') => {
            let mut m = metrics(state);
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
        KeyCode::Char('f') => {
            let mut m = metrics(state);
            m.iq.cal.frozen = !m.iq.cal.frozen;
            let frozen = m.iq.cal.frozen;
            m.push_log(
                if frozen {
                    "Constellation frozen"
                } else {
                    "Constellation live"
                }
                .to_string(),
            );
            return KeyAction::Continue;
        }
        _ => {}
    }
    global::handle(key, ctx)
}

/// `rf_chain` (RF Diagnostics) focus (`[D]`): the Lab RF bench actions.
/// `[A]` — when the chain is off-optimal, one-shot jump LNA/VGA to the staging
/// target (signal ≈ −8 dBFS); when already optimal, toggle the continuous auto-track
/// latch. `[⎵]`/`[F]` freeze or thaw the histogram + level diagram. Everything else
/// (incl. the [↑↓]/[ ] gain nudges) falls through to the global handler.
pub(super) fn rf_chain(key: KeyEvent, ctx: &mut InputCtx<'_>) -> KeyAction {
    let (state, device) = (ctx.state, ctx.device);
    use crate::ui::rf_calc::staging_target;
    match key.code {
        // [A] — auto-gain: one-shot to optimal, or latch the continuous track once
        // already there. HackRF-only; never runs unless streaming.
        KeyCode::Char('a') => {
            let (peak, lna, vga, friis, streaming) = {
                let m = metrics(state);
                (
                    m.signal.adc_peak_dbfs as f64,
                    m.radio.lna_gain,
                    m.radio.vga_gain,
                    m.caps.friis_applicable,
                    m.radio.hw_streaming,
                )
            };
            if !streaming {
                metrics(state).push_log("Auto-gain: start RX first ([Space])".to_string());
                return KeyAction::Continue;
            }
            if !friis {
                metrics(state)
                    .push_log("Auto-gain: single-tuner radio \u{2014} not applicable".to_string());
                return KeyAction::Continue;
            }
            let (lna_t, vga_t) = staging_target(peak, lna, vga);
            if (lna_t, vga_t) != (lna, vga) {
                // Off-optimal → one-shot jump through the same clamped gain path the
                // manual keys use. The latch is left as-is (hands-off one-shot).
                if let Some(device) = device {
                    let r1 = if lna_t != lna {
                        device.set_lna_gain(lna_t)
                    } else {
                        Ok(())
                    };
                    let r2 = if vga_t != vga {
                        device.set_vga_gain(vga_t)
                    } else {
                        Ok(())
                    };
                    let mut m = metrics(state);
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
                let mut m = metrics(state);
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
        KeyCode::Char(' ') | KeyCode::Char('f') => {
            let mut m = metrics(state);
            if m.lab.rf_freeze.is_some() {
                m.lab.rf_freeze = None;
                m.push_log("Lab RF: live".to_string());
            } else {
                m.lab.rf_freeze = Some(crate::state::RfFreeze {
                    signed_hist: m.iq.adc_signed_hist,
                    peak_dbfs: m.signal.adc_peak_dbfs,
                    rms_dbfs: m.signal.adc_rms_dbfs,
                    clip_events: m.signal.adc_clip_events,
                    snr_db: m.signal.peak_to_nf_db,
                    amp_enabled: m.radio.amp_enabled,
                    lna_gain: m.radio.lna_gain,
                    vga_gain: m.radio.vga_gain,
                });
                m.push_log("Lab RF: frozen \u{2014} histogram & diagram held".to_string());
            }
            return KeyAction::Continue;
        }
        _ => {}
    }
    global::handle(key, ctx)
}

/// `signal_metrics` focus (`[N]`): `[C]` logs a one-line snapshot of the current
/// signal quality metrics (SNR, channel power, occupied BW, noise floor).
/// `timing_vitals` focus (`[V]`): `[R]` resets the session drop counter, `[C]`
/// clears the trend sparkline histories.
///
/// Wired to `hardware_health` until the `lab_timing` rebuild retired that panel
/// and nobody moved the arm: the vitals panel then advertised `[R]` and `[C]` in
/// the footer while both fell through to the global handler, where `[R]` resets
/// the whole radio to defaults.
pub(super) fn timing_vitals(key: KeyEvent, ctx: &mut InputCtx<'_>) -> KeyAction {
    let (state, _device) = (ctx.state, ctx.device);
    match key.code {
        KeyCode::Char('r') => {
            let mut m = metrics(state);
            m.signal.total_drops_session = 0;
            m.push_log("Session drop counter reset");
        }
        KeyCode::Char('c') => {
            let mut m = metrics(state);
            m.signal.drop_history.clear();
            m.signal.saturation_history.clear();
            m.signal.usb_error_history.clear();
            m.iq.buf_fill_history.clear();
            m.system.cpu_history.clear();
            m.push_log("Health trend history cleared");
        }
        _ => return global::handle(key, ctx),
    }
    KeyAction::Continue
}

/// `timing_diagnostics` focus (`[T]`): `[R]` resets the session jitter peak,
/// `[C]` clears the jitter / throughput / sample-rate histories. Same story as
/// the vitals arm above: pointed at the retired `timing_panel` until now.
pub(super) fn timing_diagnostics(key: KeyEvent, ctx: &mut InputCtx<'_>) -> KeyAction {
    let (state, _device) = (ctx.state, ctx.device);
    match key.code {
        KeyCode::Char('r') => {
            let mut m = metrics(state);
            m.timing.jitter_session_max_us = 0;
            m.push_log("Jitter session peak reset");
        }
        KeyCode::Char('c') => {
            let mut m = metrics(state);
            m.iq.jitter_history.clear();
            m.radio.throughput_history.clear();
            m.radio.sample_rate_history.clear();
            m.push_log("Timing trend history cleared");
        }
        _ => return global::handle(key, ctx),
    }
    KeyAction::Continue
}
