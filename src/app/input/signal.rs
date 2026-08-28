//! Focus keys for the three `lab_signal` panels: signal metrics, signal
//! characterization, and the FM demodulator.

use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent};

use crate::ui::widgets::micro_common::fmt_bw;

use super::{global, metrics, InputCtx, KeyAction};

pub(super) fn signal_metrics(key: KeyEvent, ctx: &mut InputCtx<'_>) -> KeyAction {
    let (state, _device) = (ctx.state, ctx.device);
    if let KeyCode::Char('c') = key.code {
        let mut m = metrics(state);
        let snr = m.signal.peak_to_nf_db;
        let pwr = m.signal.channel_power_dbfs;
        let obw = m.signal.occupied_bw_hz;
        let nf = m.waterfall.last_fft.as_ref().map(|fr| fr.noise_floor);
        let obw_str = fmt_bw(obw);
        let nf_str = nf
            .map(|n| format!("{:.1} dBFS", n))
            .unwrap_or_else(|| "\u{2014}".into());
        m.push_log(format!(
            "Signal snapshot — SNR: {:.1} dB · Pwr: {:.1} dBFS · OBW: {} · NF: {}",
            snr, pwr, obw_str, nf_str
        ));
        return KeyAction::Continue;
    }
    global::handle(key, ctx)
}

/// `signal_characterization` focus (`[X]`): `[C]` logs a one-line snapshot of the
/// current characterization - modulation, SNR, occupied BW, and ACPR when it's
/// been measured. The `lab_signal` analogue of `handle_signal_metrics_focus`.
pub(super) fn signal_characterization(key: KeyEvent, ctx: &mut InputCtx<'_>) -> KeyAction {
    let (state, _device) = (ctx.state, ctx.device);
    if let KeyCode::Char('c') = key.code {
        let mut m = metrics(state);
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
    global::handle(key, ctx)
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
pub(super) fn fm_demod(key: KeyEvent, ctx: &mut InputCtx<'_>) -> KeyAction {
    let (state, _device) = (ctx.state, ctx.device);
    match key.code {
        KeyCode::Char(' ') => {
            let mut m = metrics(state);
            m.demod.user_on = !m.demod.user_on;
            let on = m.demod.user_on;
            if !on {
                // `enabled` is normally `App::draw`'s to compute, but waiting for
                // the next frame leaves a window in which the worker publishes one
                // more measurement - and that publish overwrites the clear below,
                // then sits on screen for the full staleness timeout. Switching it
                // off here means the worker's publish-time check sees the intent
                // however the two threads interleave.
                m.demod.enabled = false;
                // Every reading at once, not just `fm`: they all describe the same
                // channel, and dropping one left the MPX trace, the pilot lock and
                // the RDS station name on screen under a "DEMOD OFF" headline.
                m.demod.clear_measurements();
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
            let mut m = metrics(state);
            let limit = m.demod.offset_limit_hz(m.radio.config_sample_rate);
            let next = (m.demod.offset_hz + step).clamp(-limit, limit);
            if next != m.demod.offset_hz {
                m.demod.offset_hz = next;
                // The channel moved; every reading describes a different frequency
                // now, the accumulated RDS text included.
                m.demod.clear_measurements();
                m.push_log(format!("Demod offset: {:+.0} kHz", next as f64 / 1000.0));
            }
            return KeyAction::Continue;
        }
        KeyCode::Char('p') => {
            let mut m = metrics(state);
            let bins = m
                .waterfall
                .last_fft
                .as_ref()
                .map(|f| Arc::clone(&f.bins_dbfs));
            let sr = m.radio.config_sample_rate;
            match bins.and_then(|b| crate::state::strongest_offset_hz(&b, sr)) {
                Some(off) => {
                    let limit = m.demod.offset_limit_hz(sr);
                    let snapped = off.clamp(-limit, limit);
                    m.demod.offset_hz = snapped;
                    m.demod.clear_measurements();
                    m.push_log(format!(
                        "Demod snapped to carrier: {:+.0} kHz",
                        snapped as f64 / 1000.0
                    ));
                }
                None => m.push_log("Demod: no spectrum to snap to yet"),
            }
            return KeyAction::Continue;
        }
        KeyCode::Char('0') => {
            let mut m = metrics(state);
            m.demod.offset_hz = 0;
            m.demod.clear_measurements();
            m.push_log("Demod offset: centre");
            return KeyAction::Continue;
        }
        KeyCode::Char('t') => {
            // The classifier measures 99 % occupied bandwidth across the whole
            // span, so on a wide span it reads WFM for nearly anything - fine as a
            // badge, too coarse to choose a demodulator. This is the override.
            let mut m = metrics(state);
            let picked = m.demod.cycle_mode();
            // The previous demodulator's numbers describe a different measurement.
            m.demod.clear_measurements();
            m.push_log(match picked {
                Some(mode) => format!("Demod mode: {} (forced)", mode.label()),
                None => "Demod mode: auto".to_string(),
            });
            return KeyAction::Continue;
        }
        KeyCode::Char('c') => {
            let mut m = metrics(state);
            // The panel shows whichever demodulator is running; the snapshot used to
            // know only the FM one, so an AM depth reading or a decoded CTCSS tone
            // logged "no measurement to snapshot" with the number on screen above it.
            // The mode name is the *effective* one for the same reason the sections
            // are chosen by it - a forced mode logged the classifier's guess.
            let modulation = m.demod.effective_modulation(m.signal.modulation);
            let ch = format!(" \u{00b7} {:.0} kHz ch", m.demod.channel_rate_hz / 1000.0);
            let body = match (m.demod.live(), m.demod.live_am()) {
                (Some(fm), _) => {
                    // CTCSS rides on the same discriminator output, so when a tone is
                    // identified it belongs on the same line as the deviation it came
                    // from rather than in a snapshot of its own.
                    let tone = m
                        .demod
                        .live_ctcss()
                        .map(|t| format!(" \u{00b7} CTCSS {:.1} Hz", t.tone_hz))
                        .unwrap_or_default();
                    Some(format!(
                        "peak dev {:.1} kHz \u{00b7} RMS {:.1} kHz \u{00b7} carrier {:+.0} Hz{tone}",
                        fm.peak_dev_hz / 1000.0, fm.rms_dev_hz / 1000.0, fm.carrier_offset_hz,
                    ))
                }
                (None, Some(am)) => Some(format!(
                    "depth {:.0}% (+{:.0}/\u{2212}{:.0}) \u{00b7} carrier {:.1} dBFS",
                    am.depth_pct, am.positive_pct, am.negative_pct, am.carrier_dbfs,
                )),
                (None, None) => None,
            };
            let line = match body {
                Some(b) => format!("Demod \u{2014} {}: {b}{ch}", modulation.label()),
                None => "Demod \u{2014} no measurement to snapshot".to_string(),
            };
            m.push_log(line);
            return KeyAction::Continue;
        }
        _ => {}
    }
    global::handle(key, ctx)
}
