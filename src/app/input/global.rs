//! The keys that work everywhere: quit, help, presets, gain, RX on/off, and the
//! focus keys that enter a panel's own mode.
//!
//! Every focus handler falls through to [`handle`] for anything it does not
//! claim, so this is the bottom of the dispatch and the only place a key can be
//! claimed without a panel being focused.

use std::sync::{Arc, Mutex};

use crossterm::event::{KeyCode, KeyEvent};

use crate::hardware;
use crate::state::{InputMode, MicroView, RailMode, SdrMetrics};
use crate::ui;

use super::{metrics, InputCtx, KeyAction};

/// Next value for the primary front-end gain when stepping up/down: HackRF's LNA
/// moves in 8 dB steps (0–40); RTL-SDR's single tuner gain walks its discrete
/// table to the neighbouring entry.
fn next_primary_gain(gain: &hardware::GainModel, current: u32, up: bool) -> u32 {
    match gain {
        hardware::GainModel::HackRf => {
            if up {
                (current + 8).min(40)
            } else {
                current.saturating_sub(8)
            }
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

pub(super) fn handle(key: KeyEvent, ctx: &mut InputCtx<'_>) -> KeyAction {
    let (state, device) = (ctx.state, ctx.device);
    match key.code {
        KeyCode::Esc => {
            if ctx.engine.focused_panel_name().is_some() {
                ctx.engine.clear_focus();
                let mut m = metrics(state);
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
            let mut m = metrics(state);
            m.radio.rx_enabled = !m.radio.rx_enabled;
        }
        KeyCode::Char('r') => {
            use crate::state::{DEFAULT_LNA_GAIN, DEFAULT_VGA_GAIN};
            if let Some(device) = device {
                // Reset to the active device's own defaults so RTL-SDR lands on a
                // legal freq/rate instead of HackRF's 2.4 GHz / 10 Msps.
                let caps = device.capabilities();
                let def_freq = caps.default_frequency_hz;
                let def_sr = caps.default_sample_rate_hz;
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
                let mut m = metrics(state);
                if results.iter().all(|r| r.is_ok()) {
                    m.radio.lna_gain = lna_def;
                    m.radio.vga_gain = vga_def;
                    m.radio.amp_enabled = false;
                    m.lab.rf_autotrack = false;
                    m.radio.frequency = def_freq;
                    m.radio.config_sample_rate = def_sr;
                    m.radio.bb_filter_hz = bb_bw;
                    m.push_log("Settings reset to defaults");
                } else {
                    for r in &results {
                        if let Err(e) = r {
                            m.push_log(format!("Reset error: {}", e));
                        }
                    }
                }
            }
        }
        KeyCode::Char('f') if device.is_some() => {
            let mut m = metrics(state);
            m.ui.input_mode = InputMode::FrequencyInput;
            m.ui.input_buf.clear();
            m.push_log("Enter frequency in MHz, then press Enter");
        }
        KeyCode::Char('s') if device.is_some() => {
            let (lo, hi) = {
                let c = device.unwrap().capabilities();
                (c.sample_rate_min_hz / 1e6, c.sample_rate_max_hz / 1e6)
            };
            let mut m = metrics(state);
            m.ui.input_mode = InputMode::SampleRateInput;
            m.ui.input_buf.clear();
            m.push_log(format!(
                "Enter sample rate in MHz ({:.1}–{:.1}), then press Enter",
                lo, hi
            ));
        }
        KeyCode::Char('?') => *ctx.show_help = !*ctx.show_help,
        KeyCode::Tab => *ctx.show_footer = !*ctx.show_footer,
        KeyCode::Char('p') => {
            ctx.engine.cycle_preset();
            let name = ctx.engine.active_preset().to_string();
            metrics(state).push_log(format!("Preset: {}", name));
        }
        KeyCode::Char('1') => {
            ctx.engine.set_preset("command_rail");
            metrics(state).push_log("Preset: command rail");
        }
        KeyCode::Char('2') => {
            ctx.engine.set_preset("spectrum");
            metrics(state).push_log("Preset: spectrum");
        }
        KeyCode::Char('3') => {
            ctx.engine.set_preset("waterfall");
            metrics(state).push_log("Preset: waterfall");
        }
        KeyCode::Char('4') => {
            ctx.engine.set_preset("spectrum_waterfall");
            metrics(state).push_log("Preset: spectrum+waterfall");
        }
        // Lab family on [5]–[8]. Each lights up automatically once its preset is
        // defined; until then it logs without switching.
        KeyCode::Char('5') => {
            try_set_preset(ctx.engine, state, "lab_iq");
        }
        KeyCode::Char('6') => {
            try_set_preset(ctx.engine, state, "lab_rf");
        }
        KeyCode::Char('7') => {
            try_set_preset(ctx.engine, state, "lab_timing");
        }
        KeyCode::Char('8') => {
            try_set_preset(ctx.engine, state, "lab_signal");
        }
        // [9] reserved for the future lab_sweep (Phase 6); pre-wired so it activates
        // the moment that preset exists.
        KeyCode::Char('9') => {
            try_set_preset(ctx.engine, state, "lab_sweep");
        }
        KeyCode::Char('0') => {
            cycle_micro(ctx.engine, state);
        }
        KeyCode::Char('w') => {
            let mut m = metrics(state);
            m.waterfall.buffer.paused = !m.waterfall.buffer.paused;
            let s = if m.waterfall.buffer.paused {
                "paused"
            } else {
                "resumed"
            };
            m.push_log(format!("Waterfall {}", s));
        }
        KeyCode::Char('h') => {
            let held = {
                let m = metrics(state);
                m.waterfall
                    .last_fft
                    .as_ref()
                    .map(|fr| Arc::clone(&fr.bins_dbfs))
            };
            let mut m = metrics(state);
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
                let cur = { metrics(state).radio.lna_gain };
                let new_gain = next_primary_gain(gain, cur, true);
                let result = device.set_lna_gain(new_gain);
                let mut m = metrics(state);
                match result {
                    Ok(()) => {
                        m.radio.lna_gain = new_gain;
                        // On a single-tuner device, setting a manual gain turns the
                        // tuner AGC off in hardware — keep the UI's AGC flag in sync.
                        if gain.is_single() {
                            m.radio.amp_enabled = false;
                        }
                        m.lab.rf_autotrack = false;
                        m.ui.note_mode_action(RailMode::Bench);
                        m.push_log(format!(
                            "{} gain → {} dB",
                            primary_gain_label(gain),
                            new_gain
                        ));
                    }
                    Err(e) => m.push_log(format!("Gain error: {}", e)),
                }
            }
        }
        KeyCode::Down => {
            if let Some(device) = device {
                let gain = &device.capabilities().gain;
                let cur = { metrics(state).radio.lna_gain };
                let new_gain = next_primary_gain(gain, cur, false);
                let result = device.set_lna_gain(new_gain);
                let mut m = metrics(state);
                match result {
                    Ok(()) => {
                        m.radio.lna_gain = new_gain;
                        // On a single-tuner device, setting a manual gain turns the
                        // tuner AGC off in hardware — keep the UI's AGC flag in sync.
                        if gain.is_single() {
                            m.radio.amp_enabled = false;
                        }
                        m.lab.rf_autotrack = false;
                        m.ui.note_mode_action(RailMode::Bench);
                        m.push_log(format!(
                            "{} gain → {} dB",
                            primary_gain_label(gain),
                            new_gain
                        ));
                    }
                    Err(e) => m.push_log(format!("Gain error: {}", e)),
                }
            }
        }
        // VGA is HackRF-only; on a single-tuner device (RTL-SDR) these keys no-op.
        KeyCode::Char('[') => {
            if let Some(device) = device {
                if matches!(device.capabilities().gain, hardware::GainModel::HackRf) {
                    let new_gain = {
                        let m = metrics(state);
                        m.radio.vga_gain.saturating_sub(2)
                    };
                    let result = device.set_vga_gain(new_gain);
                    let mut m = metrics(state);
                    match result {
                        Ok(()) => {
                            m.radio.vga_gain = new_gain;
                            m.lab.rf_autotrack = false;
                            m.ui.note_mode_action(RailMode::Bench);
                            m.push_log(format!("VGA gain → {} dB", new_gain));
                        }
                        Err(e) => m.push_log(format!("VGA gain error: {}", e)),
                    }
                }
            }
        }
        KeyCode::Char(']') => {
            if let Some(device) = device {
                if matches!(device.capabilities().gain, hardware::GainModel::HackRf) {
                    let new_gain = {
                        let m = metrics(state);
                        (m.radio.vga_gain + 2).min(62)
                    };
                    let result = device.set_vga_gain(new_gain);
                    let mut m = metrics(state);
                    match result {
                        Ok(()) => {
                            m.radio.vga_gain = new_gain;
                            m.lab.rf_autotrack = false;
                            m.ui.note_mode_action(RailMode::Bench);
                            m.push_log(format!("VGA gain → {} dB", new_gain));
                        }
                        Err(e) => m.push_log(format!("VGA gain error: {}", e)),
                    }
                }
            }
        }
        KeyCode::Char('a') => {
            if let Some(device) = device {
                // `amp_enabled` doubles as the front-end-boost toggle: HackRF's RF
                // amp, RTL-SDR's tuner AGC. The label follows the gain model.
                let is_rtl = matches!(
                    device.capabilities().gain,
                    hardware::GainModel::RtlSingle { .. }
                );
                let new_state = {
                    let m = metrics(state);
                    !m.radio.amp_enabled
                };
                let result = if is_rtl {
                    device.set_tuner_agc(new_state)
                } else {
                    device.set_amp_enable(new_state)
                };
                let label = if is_rtl { "AGC" } else { "AMP" };
                let mut m = metrics(state);
                match result {
                    Ok(()) => {
                        m.radio.amp_enabled = new_state;
                        m.lab.rf_autotrack = false;
                        m.ui.note_mode_action(RailMode::Bench);
                        m.push_log(format!(
                            "{} {}",
                            label,
                            if new_state { "ON" } else { "OFF" }
                        ));
                    }
                    Err(e) => m.push_log(format!("{} error: {}", label, e)),
                }
            }
        }
        KeyCode::Char(c) => {
            if let Some(&panel_name) = ctx.focus_keys.get(&c) {
                if ctx.engine.is_panel_visible(panel_name) {
                    ctx.engine.focus(panel_name);
                    let bindings = ctx.engine.get_panel_bindings(panel_name);
                    let mut m = metrics(state);
                    m.ui.focused_panel = Some(panel_name.to_string());
                    m.ui.focused_panel_bindings = bindings;
                }
            }
        }
        _ => {}
    }
    KeyAction::Continue
}

/// [`handle`] with the radio hidden.
///
/// The waterfall's focus handler never had a `device` parameter, so anything
/// falling through from it could not reach the hardware. Hiding the device here
/// keeps that exactly true now that the context carries one: `[Space]`, `[R]`,
/// `[F]` and the gain keys stay inert while the waterfall holds focus, rather
/// than quietly becoming live.
pub(super) fn handle_no_device(key: KeyEvent, ctx: &mut InputCtx<'_>) -> KeyAction {
    let saved = ctx.device.take();
    let action = handle(key, ctx);
    ctx.device = saved;
    action
}

/// Switch to `name` if the preset is defined, otherwise log that it is not yet
/// available. This keeps the number-key framework (`[6]`–`[9]`, `[0]`) in place
/// before the presets themselves exist, so each one activates the moment it is
/// added to the layout config.
fn try_set_preset(
    engine: &mut ui::LayoutEngine,
    state: &Arc<Mutex<SdrMetrics>>,
    name: &str,
) -> KeyAction {
    let mut m = metrics(state);
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
    let mut m = metrics(state);
    let target = if in_micro {
        m.ui.micro_view.next(SWEEP_ACTIVE)
    } else {
        MicroView::Main
    };
    if engine.has_preset(target.preset_name()) {
        m.ui.micro_view = target;
        engine.set_preset(target.preset_name());
        m.push_log(format!(
            "Micro: {} ({}/{})",
            target.label(),
            target.position(),
            MicroView::total(SWEEP_ACTIVE)
        ));
    } else {
        m.push_log(format!(
            "Preset '{}' not yet available",
            target.preset_name()
        ));
    }
}
