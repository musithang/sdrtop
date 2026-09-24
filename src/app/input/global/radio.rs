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

    // The rate is the device's own default, so a driver has no reason to round
    // it; taking what it reports anyway costs nothing and keeps one account of
    // where the recorded rate comes from.
    let (sr_result, settled_sr, bb_bw) = match device.set_sample_rate(def_sr) {
        Ok(set) => (Ok(()), set.rate_hz, set.bb_filter_hz),
        Err(e) => (
            Err(e),
            def_sr,
            crate::hardware::native::hackrf::compute_bb_filter_bw(def_sr),
        ),
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
        m.radio.config_sample_rate = settled_sr;
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
    let (lo, hi, name) = {
        let c = device.capabilities();
        (
            c.sample_rate_min_hz / 1e6,
            c.sample_rate_max_hz / 1e6,
            if c.sample_rate_is_span {
                "span"
            } else {
                "sample rate"
            },
        )
    };
    let mut m = metrics(ctx.state);
    m.ui.input_mode = InputMode::SampleRateInput;
    m.ui.input_buf.clear();
    m.push_log(format!(
        "Enter {name} in MHz ({lo:.1}\u{2013}{hi:.1}), then press Enter",
    ));
}

/// `[m]` - survey the band, or lock to where the radio is pointed.
///
/// Design section 13.1 makes this a mode rather than a setting, so switching it
/// changes what every reading in the section *claims*, not just what the
/// receiver does.
///
/// **Returns whether it claimed the key, and that return value is the whole
/// point.** `m` was already the FM demodulator's focus key, and a global arm
/// that swallowed it would have made that panel unreachable everywhere outside
/// this section - silently, because the existing structural test checks that a
/// focusable panel *has* a dispatch arm, not that its key still reaches it. A
/// section-scoped key has to decline rather than absorb.
pub(super) fn toggle_net_mode(ctx: &mut InputCtx<'_>) -> bool {
    let mut m = metrics(ctx.state);
    if !m.ui.is_net_section() {
        return false;
    }
    let mode = m.net.mode.toggled();
    m.net.mode = mode;
    // Survey and lock are different regimes for how often the radio is looking
    // at any one megahertz, so the coverage accounting starts again. The
    // measurements stay: they are still what was on the air.
    m.net.band.restart_watch();
    // The band keeps what it measured, and every cell keeps the time it was
    // measured at, so the panel goes on showing the last pass while the new mode
    // fills in over it. Clearing it here would throw away good measurements to
    // make a point about the mode.
    //
    // **The lock is not logged here**, and that is the fix rather than an
    // omission: this handler knows the mode the user asked for, not the
    // frequency the radio ends on, and it used to name the current hop in a
    // line the survey task then falsified by retuning somewhere else. The task
    // logs it, once, with the frequency it actually left the radio on.
    if mode == crate::state::NetMode::Survey {
        m.push_log("NET surveying the band".to_string());
    }
    true
}

/// `[←]` `[→]` - in NET and locked, the previous or next place this view
/// listens (`signal::net::lock`): the next advertising channel on the views
/// the advertising decoder feeds, the next block of the band elsewhere.
/// Surveying, the survey owns the tuning, so the key says what it is for
/// rather than fighting it. Outside NET it does nothing.
pub(super) fn step_net_channel(ctx: &mut InputCtx<'_>, forward: bool) {
    use crate::signal::net::lock;
    let mut m = metrics(ctx.state);
    if !m.ui.is_net_section() {
        return;
    }
    if m.net.mode != crate::state::NetMode::Lock {
        m.push_log("NET: ← → step the channel while locked; [M] locks".to_string());
        return;
    }
    // From a step already asked for and not yet applied, so two quick
    // presses go two places rather than one twice.
    let from = m
        .net
        .lock_at
        .as_ref()
        .map_or(m.radio.frequency, |t| t.tune_hz);
    // A constant block: the span, or on the classic view the most channels
    // it watches at once where that is fewer. The watched list itself is
    // shorter at the band's edges and would make the steps uneven.
    let span = if m.radio.bb_filter_hz > 0 {
        (m.radio.bb_filter_hz as f64).min(m.radio.config_sample_rate)
    } else {
        m.radio.config_sample_rate
    };
    let span_mhz = (span / 1e6).floor() as u64;
    let block_mhz = match (m.ui.active_preset.as_str(), m.net.bt_capacity as u64) {
        ("net_bt", cap) if cap > 0 => span_mhz.min(cap),
        _ => span_mhz,
    };
    let how = lock::stepping(&m.ui.active_preset, block_mhz);
    m.net.lock_at = Some(lock::step(from, how, forward));
}

/// `[i]` - cycle how addresses are shown throughout the NET section
/// (foundation design 1.1: `full`, `oui`, and `masked` once it exists).
///
/// Section-scoped and declining, like [`toggle_net_mode`]: outside NET there
/// are no addresses to show, and the key is left for whatever claims it next.
/// Logged, because the switch changes every panel and the export at once and
/// the log is where "why did the addresses change" gets answered.
pub(super) fn cycle_address_display(ctx: &mut InputCtx<'_>) -> bool {
    let mut m = metrics(ctx.state);
    if !m.ui.is_net_section() {
        return false;
    }
    let display = m.net.address_display.next();
    m.net.address_display = display;
    m.push_log(format!("NET addresses shown: {}", display.label()));
    true
}

/// `[y]` - establish the frequency reference from what is on centre now.
///
/// Design section 7: every ppm reading in the app contains our own oscillator's
/// error, and this is the one action that takes it out of all of them at once.
/// It asks; the FFT worker answers on its next block, because that is where the
/// raw samples already are.
pub(super) fn capture_reference(ctx: &mut InputCtx<'_>) {
    let mut m = metrics(ctx.state);
    if !m.radio.hw_streaming {
        m.push_log("Frequency reference: start RX first".to_string());
        return;
    }
    m.radio.reference_request = true;
}
