// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Focus keys for the instrument benches: the lab banner, IQ diagnostics, the
//! RF chain, and the two timing panels.
//!
//! What they have in common is that each drives the *radio* rather than the
//! view - gain, calibration, capture - so most of them need the device.

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
        // [M] - pin / unpin the carrier+image markers (override the live auto-track).
        KeyCode::Char('m') => {
            let mut m = metrics(state);
            let auto = ui::panels::lab::image_scope::carrier_image(&m);
            if m.lab.iq_marker_pin.is_some() {
                m.lab.iq_marker_pin = None;
                m.push_log("IQ markers: auto-tracking carrier/image".to_string());
            } else if let Some(ci) = auto {
                m.lab.iq_marker_pin = Some((ci.carrier_hz, ci.image_hz));
                let image = match ci.reading.rejection() {
                    Some((db, true)) => format!("image \u{2265} {db:.1} dB down"),
                    Some((db, false)) => format!("image {db:.1} dB down"),
                    None => "no image to measure".to_string(),
                };
                m.push_log(format!(
                    "IQ markers pinned — carrier {:.3} MHz · mirror {:.3} MHz · {image}",
                    ci.carrier_hz as f64 / 1e6,
                    ci.image_hz as f64 / 1e6,
                ));
            } else {
                m.push_log("IQ markers: no carrier detected yet".to_string());
            }
            return KeyAction::Continue;
        }
        // [D] - DC-block: subtract the live DC estimate from the stream.
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
        // [C] - auto-cal: capture (or clear) the I/Q quadrature correction.
        KeyCode::Char('c') => {
            let mut m = metrics(state);
            if m.iq.cal.cal_applied || m.iq.cal.cal_pending {
                m.iq.cal.cal_applied = false;
                m.iq.cal.cal_pending = false;
                m.iq.cal.c_qi = 0.0;
                m.iq.cal.c_qq = 1.0;
                m.push_log("IQ auto-cal cleared — quadrature uncorrected".to_string());
            } else if let Some(why) = m.iq_balance_refusal() {
                m.push_log(format!("IQ auto-cal refused: {why}"));
            } else {
                m.iq.cal.cal_pending = true;
                m.push_log("IQ auto-cal — capturing correction…".to_string());
            }
            return KeyAction::Continue;
        }
        // [F] - freeze / thaw the constellation cloud.
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
/// `[A]` - when the chain is off-optimal, one-shot jump LNA/VGA to the staging
/// target (signal ≈ −8 dBFS); when already optimal, toggle the continuous auto-track
/// latch. `[⎵]`/`[F]` freeze or thaw the histogram + level diagram. Everything else
/// (incl. the [↑↓]/[ ] gain nudges) falls through to the global handler.
pub(super) fn rf_chain(key: KeyEvent, ctx: &mut InputCtx<'_>) -> KeyAction {
    let (state, device) = (ctx.state, ctx.device);
    match key.code {
        // [A] - auto-gain: one-shot to optimal, or latch the continuous track once
        // already there. HackRF-only; never runs unless streaming.
        KeyCode::Char('a') => {
            let (peak, stages, current, streaming) = {
                let m = metrics(state);
                (
                    // Against the radio's ceiling: the target keeps its
                    // margin from where this radio clips.
                    m.peak_to_ceiling_db() as f64,
                    m.caps.gain.stages(),
                    m.radio.gains.clone(),
                    m.radio.hw_streaming,
                )
            };
            if !streaming {
                metrics(state).push_log("Auto-gain: start RX first ([Space])".to_string());
                return KeyAction::Continue;
            }
            // **No longer gated on a modelled chain.** The target used to be
            // computed against the HackRF's own grids, so it meant nothing
            // anywhere else; since R3 it is one policy laid over whatever stages
            // the device reports, and a radio with a stage list can be staged.
            if stages.is_empty() {
                metrics(state).push_log(
                    "Auto-gain: this device reports no gain stages \u{2014} nothing to set"
                        .to_string(),
                );
                return KeyAction::Continue;
            }
            // A target in dB means nothing where a stage's dB was not measured.
            // The latch goes off too: tracking on figures that do not hold is
            // the same decision made forty times a second.
            if let Some(why) = metrics(state).chain_db_refusal() {
                let mut m = metrics(state);
                m.lab.rf_autotrack = false;
                m.push_log(format!(
                    "Auto-gain: {why}, and its target is set in dB \u{2014} set the stages by hand"
                ));
                return KeyAction::Continue;
            }
            // Along the knob's curve when it walks one, as the arrows would.
            let (gm, curve) = {
                let m = metrics(state);
                (m.caps.gain.clone(), m.radio.gain_curve)
            };
            // From a peak at the ceiling the level is unknown, and the chain
            // goes to its least gain instead; the log says so.
            let (targets, known) = crate::ui::rf_calc::auto_gain_target(peak, &gm, curve, &current);
            // Judged on the **total**, not on the arrangement. Auto-gain is
            // about where the converter sits; how that total is spread is the
            // knob's business. Comparing per stage would make a hand-set chain
            // at a perfectly good level read as "off optimal" and rearrange
            // itself under the user.
            let now_total: f64 = current.iter().take(stages.len()).sum();
            let want_total: f64 = targets.iter().sum();
            let moved = (now_total - want_total).abs() >= 0.5;
            if moved {
                // Off-optimal → one-shot jump, one call per stage, through the
                // same per-stage path the manual keys use.
                if let Some(device) = device {
                    let mut failed = None;
                    for (i, spec) in stages.iter().enumerate() {
                        if let Err(e) = device.set_stage_gain(i, &spec.name, targets[i]) {
                            failed = Some(format!("{}: {e}", spec.name));
                            break;
                        }
                    }
                    let mut m = metrics(state);
                    match failed {
                        None => {
                            m.radio.gains = targets.clone();
                            m.ui.note_mode_action(RailMode::Bench);
                            let split: Vec<String> = stages
                                .iter()
                                .enumerate()
                                .map(|(i, spec)| format!("{} {:.0}", spec.name, targets[i]))
                                .collect();
                            // The step too, when the chain landed on the
                            // knob's curve, as the arrows' log names it.
                            let step = m
                                .curve_reading()
                                .filter(|c| c.on)
                                .map(|c| format!("{} {} \u{00b7} ", c.name, c.step_text()))
                                .unwrap_or_default();
                            // The target as the radio's own: 8 dB under its
                            // ceiling, -11 dBFS on an R2 whose tuner clips at
                            // -3.
                            let target = m.ceiling_dbfs() + crate::ui::rf_calc::OPT_PEAK_DBFS;
                            if known {
                                m.push_log(format!(
                                    "Auto-gain \u{2192} {step}{} dB (signal \u{2192} \u{2212}{:.0} dBFS)",
                                    split.join(" \u{00b7} "),
                                    -target
                                ));
                            } else {
                                let at = m.ceiling().map_or_else(
                                    || "full scale".to_string(),
                                    |c| format!("the {}'s {}", c.part, c.kind.noun()),
                                );
                                m.push_log(format!(
                                    "Auto-gain \u{2192} {step}{} dB, the least: the peak was at {at}, \
                                     so its level is unknown; [A] again lands it",
                                    split.join(" \u{00b7} ")
                                ));
                            }
                        }
                        Some(why) => m.push_log(format!("Auto-gain: {why}")),
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
        // [K] - the noise step measurement: walk the front stage across its
        // settings and watch what each step does to the noise floor. Pressing it
        // again during a run stops it and puts the stage back.
        KeyCode::Char('k') => {
            let (running, stages, current, streaming, tuned) = {
                let m = metrics(state);
                (
                    m.lab.noise_sweep.is_some(),
                    m.caps.gain.stages(),
                    m.radio.gains.clone(),
                    m.radio.hw_streaming,
                    m.radio.frequency,
                )
            };
            if running {
                let mut m = metrics(state);
                if let Some(sw) = m.lab.noise_sweep.as_mut() {
                    sw.abort();
                }
                m.push_log("Noise sweep: stopping, restoring gain".to_string());
                return KeyAction::Continue;
            }
            if !streaming {
                metrics(state).push_log("Noise sweep: start RX first ([Space])".to_string());
                return KeyAction::Continue;
            }
            // The front stage, because it is the one that decides the noise
            // figure of the whole chain. Sweeping a later stage measures the
            // stage in front of it, which is not the question.
            let Some(spec) = stages.first() else {
                metrics(state)
                    .push_log("Noise sweep: this device reports no gain stages".to_string());
                return KeyAction::Continue;
            };
            if let Some(why) = crate::signal::noise_slope::refusal(spec, tuned) {
                metrics(state).push_log(format!("Noise sweep: {why}"));
                return KeyAction::Continue;
            }
            let plan = crate::signal::noise_slope::plan_for(spec);
            if plan.is_empty() {
                metrics(state).push_log(format!(
                    "Noise sweep: {} has too few settings to sweep",
                    spec.name
                ));
                return KeyAction::Continue;
            }
            let restore = current.first().copied().unwrap_or(spec.min_db);
            let mut sweep = crate::signal::noise_slope::GainSweep::new(0, plan.clone(), restore);
            sweep.begin();
            let mut m = metrics(state);
            // The latch would spend the whole measurement putting the stage
            // back; see the note in `tasks/rx/control.rs::track_gain`.
            m.lab.rf_autotrack = false;
            m.lab.noise_reading = None;
            m.lab.noise_sweep = Some(sweep);
            m.ui.note_mode_action(RailMode::Bench);
            m.push_log(format!(
                "Noise sweep: {} across {} settings, {:.0}\u{2013}{:.0} dB",
                spec.name,
                plan.len(),
                plan[0],
                plan[plan.len() - 1]
            ));
            return KeyAction::Continue;
        }
        // [⎵]/[F] - freeze / thaw the histogram + level diagram (display only; RX
        // keeps running). Bound to focus, not global Space=RX.
        KeyCode::Char(' ') | KeyCode::Char('f') => {
            let mut m = metrics(state);
            if m.lab.rf_freeze.is_some() {
                m.lab.rf_freeze = None;
                m.push_log("Lab RF: live".to_string());
            } else {
                m.lab.rf_freeze = Some(crate::state::RfFreeze {
                    band_hist: m.iq.adc_band_hist,
                    peak_dbfs: m.signal.adc_peak_dbfs,
                    rms_dbfs: m.signal.adc_rms_dbfs,
                    clip_events: m.signal.adc_clip_events,
                    snr_db: m.signal.peak_to_nf_db,
                    amp_enabled: m.radio.amp_enabled,
                    lna_gain: m.radio.primary_gain(),
                    vga_gain: m.radio.secondary_gain(),
                    floor: m
                        .waterfall
                        .last_fft
                        .as_ref()
                        .filter(|fr| fr.enbw_hz > 0.0)
                        .map(|fr| (fr.noise_floor, fr.enbw_hz)),
                    readings: (0..m.caps.gain.stages().len())
                        .map(|i| m.stage_reading(i))
                        .collect(),
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
            m.signal.driver_drops_session = 0;
            // Both session totals, because the panel shows them one above the
            // other and a reset that cleared only one would read as the other
            // still counting.
            m.iq.fft_drops_session = 0;
            m.push_log("Session drop counters reset");
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use crossterm::event::{KeyCode, KeyEvent};

    use crate::config::LayoutConfig;
    use crate::state::SdrMetrics;
    use crate::ui::{LayoutEngine, PanelRegistry};

    use super::super::{metrics, InputCtx};

    fn press_on_iq_bench(state: &Arc<Mutex<SdrMetrics>>, c: char) {
        let mut engine = LayoutEngine::new(LayoutConfig::default_config(), PanelRegistry::new());
        let mut show_footer = true;
        let focus_keys = HashMap::new();
        let mut ctx = InputCtx {
            state,
            device: None,
            engine: &mut engine,
            show_footer: &mut show_footer,
            focus_keys: &focus_keys,
        };
        super::iq_diagnostics(KeyEvent::from(KeyCode::Char(c)), &mut ctx);
    }

    /// **Auto-cal is refused where the balance is not the front end's**: it
    /// would build a correction from moments the library's turning has
    /// already evened out, and apply it on top of the library's own. Where
    /// the LO is at the centre it captures as it always has.
    #[test]
    fn auto_cal_is_refused_where_the_balance_is_not_the_front_ends() {
        let state = Arc::new(Mutex::new(SdrMetrics::fixture().streaming()));
        metrics(&state).radio.lo_offset_hz = 5_000;
        press_on_iq_bench(&state, 'c');
        {
            let m = metrics(&state);
            assert!(!m.iq.cal.cal_pending, "nothing captured");
            let log = &m.ui.log.back().unwrap().text;
            assert!(
                log.starts_with("IQ auto-cal refused: the samples are turned +5.0 kHz off the LO"),
                "{log}"
            );
        }
        metrics(&state).radio.lo_offset_hz = 0;
        press_on_iq_bench(&state, 'c');
        assert!(metrics(&state).iq.cal.cal_pending, "captures at the centre");
    }
}
