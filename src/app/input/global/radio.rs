// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The keys that command the radio: `[Space]`, `[R]`, `[F]`, `[S]`.
//!
//! Each is a no-op without a device, which is what makes observer mode and the
//! waterfall's fall-through safe: `handle_no_device` hides the radio and every
//! one of these returns immediately.

use crate::state::{InputMode, DEFAULT_LNA_GAIN, DEFAULT_VGA_GAIN};

use super::super::{metrics, InputCtx};

/// `[Space]` - start or stop streaming. The RX task sees the flag on its next
/// poll and talks to the device itself, so nothing here touches hardware.
pub(super) fn toggle_rx(ctx: &mut InputCtx<'_>) {
    if ctx.device.is_none() {
        return;
    }
    let mut m = metrics(ctx.state);
    m.radio.rx_enabled = !m.radio.rx_enabled;
}

/// `[R]` - back to the **active device's own** defaults, so an RTL-SDR lands on a
/// legal freq/rate instead of HackRF's 2.4 GHz / 10 Msps, and on a legal tuner
/// step instead of HackRF's raw LNA/VGA constants.
///
/// Every device call is made before the guard is taken, and the state is written
/// only if all of them succeeded - a half-applied reset would leave the panels
/// describing a radio that is not in that state.
pub(super) fn reset_defaults(ctx: &mut InputCtx<'_>) {
    let Some(device) = ctx.device else { return };
    let caps = device.capabilities();
    let def_freq = caps.default_frequency_hz;
    let def_sr = caps.default_sample_rate_hz;
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

    let mut m = metrics(ctx.state);
    if results.iter().all(|r| r.is_ok()) {
        m.radio.set_primary_gain(lna_def);
        m.radio.set_secondary_gain(vga_def);
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

/// `[F]` - type a frequency.
pub(super) fn begin_frequency_input(ctx: &mut InputCtx<'_>) {
    if ctx.device.is_none() {
        return;
    }
    let mut m = metrics(ctx.state);
    m.ui.input_mode = InputMode::FrequencyInput;
    m.ui.input_buf.clear();
    m.push_log("Enter frequency in MHz, then press Enter");
}

/// `[S]` - type a sample rate, with the device's own legal range in the prompt.
pub(super) fn begin_sample_rate_input(ctx: &mut InputCtx<'_>) {
    let Some(device) = ctx.device else { return };
    let (lo, hi) = {
        let c = device.capabilities();
        (c.sample_rate_min_hz / 1e6, c.sample_rate_max_hz / 1e6)
    };
    let mut m = metrics(ctx.state);
    m.ui.input_mode = InputMode::SampleRateInput;
    m.ui.input_buf.clear();
    m.push_log(format!(
        "Enter sample rate in MHz ({:.1}\u{2013}{:.1}), then press Enter",
        lo, hi
    ));
}
