// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The parts of the poll that talk to the radio: starting and stopping RX,
//! noticing that it stopped on its own, and the Lab RF auto-gain latch.
//!
//! One rule runs through all three: **device calls happen with no lock held.**
//! `start_rx`, `stop_rx` and the gain setters go over USB and can block for
//! milliseconds; holding the mutex across one would stall every frame the UI
//! draws. Each function here reads what it needs, drops the guard, calls the
//! device, then takes the guard again to record the outcome.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::hardware::{RxContext, SdrDevice};
// The ADC peak window auto-gain is content to leave alone. Shared with the
// Command Rail's CHAIN verdict, which calls anything above the top of it
// "hot": two constants would let the rail advise backing off a level this
// latch is happy to hold.
use crate::state::{SdrMetrics, ADC_COMFORT_DBFS as AUTOGAIN_COMFORT_DBFS};

use super::metrics::RateTracker;
use super::publish::Throughput;

/// Notice that the radio stopped streaming without being asked, and say so.
///
/// Returns the new `hw_rx_active`. The device is told to stop as well: it has
/// already stopped producing, but the driver-side session still needs closing or
/// the next `start_rx` inherits it.
pub(super) fn note_unexpected_stop(
    state: &Arc<Mutex<SdrMetrics>>,
    device: &Arc<dyn SdrDevice>,
    hw_rx_active: bool,
    hw_streaming: bool,
) -> bool {
    if !hw_rx_active || hw_streaming {
        return hw_rx_active;
    }
    let _ = device.stop_rx();
    let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
    m.radio.rx_enabled = false;
    m.radio.hw_streaming = false;
    m.radio.rx_start_time = None;
    m.push_log("WARNING: Streaming stopped unexpectedly \u{2014} press [Space] to restart");
    false
}

/// Bring the hardware into line with what the user asked for. Returns the new
/// `hw_rx_active`.
pub(super) fn apply_rx_request(
    state: &Arc<Mutex<SdrMetrics>>,
    device: &Arc<dyn SdrDevice>,
    rx_ctx: &Arc<RxContext>,
    tp: &mut Throughput,
    rate: &mut RateTracker,
    rx_enabled: bool,
    hw_rx_active: bool,
) -> bool {
    match (rx_enabled, hw_rx_active) {
        (true, false) => match device.start_rx(Arc::clone(rx_ctx)) {
            Ok(()) => {
                // Fresh per-session throughput statistics, and a rate baseline
                // that does not span the stop. Averaging across one would mix a
                // silent stretch into the sample-rate offset.
                tp.reset();
                rate.reset();
                let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
                m.radio.rx_start_time = Some(Instant::now());
                m.timing.jitter_session_max_us = 0;
                m.push_log("RX streaming started");
                true
            }
            Err(e) => {
                let msg = format!("Error starting RX: {}", e);
                let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
                m.radio.rx_enabled = false;
                m.push_log(msg);
                false
            }
        },
        (false, true) => {
            let result = device.stop_rx();
            rate.reset();
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            m.radio.rx_start_time = None;
            match result {
                Ok(()) => m.push_log("RX streaming stopped"),
                Err(e) => m.push_log(format!("Error stopping RX: {}", e)),
            }
            false
        }
        (_, active) => active,
    }
}

/// Lab RF continuous auto-gain (AGC-lite).
///
/// Only when the `[A]` latch is set, streaming, and on a cascade-capable radio.
/// Re-centres the ADC peak when it drifts out of the comfortable window, jumping
/// LNA/VGA to the same staging target the one-shot uses. At the rails the target
/// equals the current gain, so there is no action and no log spam.
pub(super) fn track_gain(
    state: &Arc<Mutex<SdrMetrics>>,
    device: &Arc<dyn SdrDevice>,
    adc_peak_dbfs: f32,
) {
    let (latched, stages, current) = {
        let m = state.lock().unwrap_or_else(|e| e.into_inner());
        (
            m.lab.rf_autotrack,
            m.caps.gain.stages(),
            m.radio.gains.clone(),
        )
    };
    // No longer gated on a modelled chain: since R3 the target is one policy
    // over whatever stages the device reports, so any radio that names some can
    // be tracked. A radio that names none has nothing to move.
    if !latched || stages.is_empty() || AUTOGAIN_COMFORT_DBFS.contains(&adc_peak_dbfs) {
        return;
    }

    let targets = crate::ui::rf_calc::staging_target(adc_peak_dbfs as f64, &stages, &current);
    // The total, not the arrangement: see the note in `input/bench.rs`.
    let now_total: f64 = current.iter().take(stages.len()).sum();
    let want_total: f64 = targets.iter().sum();
    if (now_total - want_total).abs() < 0.5 {
        return;
    }

    // Device sets run with no lock held.
    let mut failed = None;
    for (i, spec) in stages.iter().enumerate() {
        if let Err(e) = device.set_stage_gain(i, &spec.name, targets[i]) {
            failed = Some(format!("{}: {e}", spec.name));
            break;
        }
    }
    let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
    match failed {
        None => {
            m.radio.gains = targets.clone();
            let split: Vec<String> = stages
                .iter()
                .enumerate()
                .map(|(i, spec)| format!("{} {:.0}", spec.name, targets[i]))
                .collect();
            m.push_log(format!(
                "Auto-gain track \u{2192} {} dB",
                split.join(" \u{00b7} ")
            ));
        }
        Some(why) => m.push_log(format!("Auto-gain track: {why}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_comfort_window_is_closed_at_both_ends() {
        // A peak sitting exactly on a boundary is comfortable, so auto-gain leaves
        // it alone rather than nudging it and logging on every poll.
        assert!(AUTOGAIN_COMFORT_DBFS.contains(&-12.0));
        assert!(AUTOGAIN_COMFORT_DBFS.contains(&-4.0));
        assert!(AUTOGAIN_COMFORT_DBFS.contains(&-8.0));
        // Outside it, the latch acts.
        assert!(!AUTOGAIN_COMFORT_DBFS.contains(&-12.1));
        assert!(!AUTOGAIN_COMFORT_DBFS.contains(&-3.9));
        // The window brackets the staging target, or auto-gain would chase its
        // own tail: settle at the target, find it out of window, move again.
        let opt = crate::ui::rf_calc::OPT_PEAK_DBFS as f32;
        assert!(
            AUTOGAIN_COMFORT_DBFS.contains(&opt),
            "the optimum {opt} dBFS must be inside the comfort window"
        );
    }
}
