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
use crate::state::SdrMetrics;

use super::publish::Throughput;

/// The ADC peak window auto-gain is content to leave alone, in dBFS. Outside it,
/// the latch re-centres the level; inside, it does nothing at all.
const AUTOGAIN_COMFORT_DBFS: std::ops::RangeInclusive<f32> = -12.0..=-4.0;

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
    rx_enabled: bool,
    hw_rx_active: bool,
) -> bool {
    match (rx_enabled, hw_rx_active) {
        (true, false) => match device.start_rx(Arc::clone(rx_ctx)) {
            Ok(()) => {
                // Fresh per-session throughput statistics.
                tp.reset();
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
    let (latched, friis, lna, vga) = {
        let m = state.lock().unwrap_or_else(|e| e.into_inner());
        (
            m.lab.rf_autotrack,
            m.caps.friis_applicable,
            m.radio.lna_gain,
            m.radio.vga_gain,
        )
    };
    if !latched || !friis || AUTOGAIN_COMFORT_DBFS.contains(&adc_peak_dbfs) {
        return;
    }

    let (lna_t, vga_t) = crate::ui::rf_calc::staging_target(adc_peak_dbfs as f64, lna, vga);
    if (lna_t, vga_t) == (lna, vga) {
        return;
    }

    // Device sets run with no lock held.
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
    let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
    match (r1, r2) {
        (Ok(()), Ok(())) => {
            m.radio.lna_gain = lna_t;
            m.radio.vga_gain = vga_t;
            m.push_log(format!(
                "Auto-gain track \u{2192} LNA {lna_t} \u{00b7} VGA {vga_t} dB"
            ));
        }
        _ => m.push_log("Auto-gain track: device error".to_string()),
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
