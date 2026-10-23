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

pub(super) enum RxRequestTransition {
    Unchanged(bool),
    Started,
    StartFailed(anyhow::Error),
    Stopped(anyhow::Result<()>),
}

pub(super) fn request_transition(
    rx_enabled: bool,
    hw_rx_active: bool,
    start: impl FnOnce() -> anyhow::Result<()>,
    stop: impl FnOnce() -> anyhow::Result<()>,
) -> RxRequestTransition {
    match (rx_enabled, hw_rx_active) {
        (true, false) => match start() {
            Ok(()) => RxRequestTransition::Started,
            Err(error) => RxRequestTransition::StartFailed(error),
        },
        (false, true) => RxRequestTransition::Stopped(stop()),
        (_, active) => RxRequestTransition::Unchanged(active),
    }
}

pub(super) fn unexpected_stop(
    hw_rx_active: bool,
    hw_streaming: bool,
    cleanup: impl FnOnce() -> anyhow::Result<()>,
) -> Option<anyhow::Result<()>> {
    (hw_rx_active && !hw_streaming).then(cleanup)
}

/// Notice that the radio stopped streaming without being asked, and say so.
///
/// Returns the new `hw_rx_active`. The device is told to stop as well: it has
/// already stopped producing, but the driver-side session still needs closing or
/// the next `start_rx` inherits it.
pub(super) fn note_unexpected_stop(
    state: &Arc<Mutex<SdrMetrics>>,
    device: &Arc<dyn SdrDevice>,
    rx_ctx: &Arc<RxContext>,
    hw_rx_active: bool,
    hw_streaming: bool,
) -> bool {
    let Some(_) = unexpected_stop(hw_rx_active, hw_streaming, || device.stop_rx()) else {
        return hw_rx_active;
    };
    // A recording of a stream that has ended is finished, not paused.
    rx_ctx.record.disarm();
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
    match request_transition(
        rx_enabled,
        hw_rx_active,
        || {
            // A new stream counts its positions from zero, so its first block
            // is never mistaken for the continuation of the last stream's.
            rx_ctx.begin_stream();
            device.start_rx(Arc::clone(rx_ctx))
        },
        || device.stop_rx(),
    ) {
        RxRequestTransition::Started => {
            // Keep the sample-rate baseline within one RX session
            tp.reset();
            rate.reset();
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            m.radio.rx_start_time = Some(Instant::now());
            m.timing.jitter_session_max_us = 0;
            m.push_log("RX streaming started");
            true
        }
        RxRequestTransition::StartFailed(error) => {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            m.radio.rx_enabled = false;
            m.push_log(format!("Error starting RX: {error}"));
            false
        }
        RxRequestTransition::Stopped(result) => {
            // The recording ends with its stream; the writer says so in the
            // file once it has written what was queued.
            rx_ctx.record.disarm();
            rate.reset();
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            m.radio.rx_start_time = None;
            match result {
                Ok(()) => m.push_log("RX streaming stopped"),
                Err(e) => m.push_log(format!("Error stopping RX: {}", e)),
            }
            false
        }
        RxRequestTransition::Unchanged(active) => active,
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
    let (latched, gm, curve, current) = {
        let m = state.lock().unwrap_or_else(|e| e.into_inner());
        // A noise sweep moves one stage on purpose and reads what that does to
        // the floor. Auto-track exists to undo exactly that. Whichever ran
        // second would win, and the measurement would be of the argument
        // between them, so the sweep holds the chain while it lasts.
        if m.lab.noise_sweep.is_some() {
            return;
        }
        // Retuned out of a stage's measured band: its dB no longer hold, so
        // neither does a target in dB. Off, once, and said.
        if m.lab.rf_autotrack {
            if let Some(why) = m.chain_db_refusal() {
                drop(m);
                let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
                m.lab.rf_autotrack = false;
                m.push_log(format!("Auto-gain track OFF: {why}"));
                return;
            }
        }
        (
            m.lab.rf_autotrack,
            m.caps.gain.clone(),
            m.radio.gain_curve,
            m.radio.gains.clone(),
        )
    };
    let stages = gm.stages();
    // Against the radio's ceiling: the comfortable window and the target
    // keep their margin from where this radio clips.
    let adc_peak_dbfs = adc_peak_dbfs - gm.ceiling_dbfs() as f32;
    // No longer gated on a modelled chain: since R3 the target is one policy
    // over whatever stages the device reports, so any radio that names some can
    // be tracked. A radio that names none has nothing to move.
    if !latched || stages.is_empty() || AUTOGAIN_COMFORT_DBFS.contains(&adc_peak_dbfs) {
        return;
    }

    // Along the knob's curve when it walks one, as the arrows would.
    let targets = crate::ui::rf_calc::staging_target_on(adc_peak_dbfs as f64, &gm, curve, &current);
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
            let step = m
                .curve_reading()
                .filter(|c| c.on)
                .map(|c| format!("{} {} \u{00b7} ", c.name, c.step_text()))
                .unwrap_or_default();
            m.push_log(format!(
                "Auto-gain track \u{2192} {step}{} dB",
                split.join(" \u{00b7} ")
            ));
        }
        Some(why) => m.push_log(format!("Auto-gain track: {why}")),
    }
}

/// Carry out whatever step the noise sweep is waiting on.
///
/// The sweep itself lives under the lock and is fed by the FFT publish path, at
/// frame rate. It cannot make the call it needs: a device call must not happen
/// with the mutex held, and the FFT worker must not talk to the radio at all. So
/// it leaves the step behind as `pending_set`, and this picks it up on the next
/// poll: lock, read, drop, set, lock, acknowledge. `applied` is what starts the
/// settle count, so the sweep waits for the radio rather than for the clock.
pub(super) fn advance_noise_sweep(state: &Arc<Mutex<SdrMetrics>>, device: &Arc<dyn SdrDevice>) {
    let (pending, stages, streaming) = {
        let m = state.lock().unwrap_or_else(|e| e.into_inner());
        match m.lab.noise_sweep.as_ref() {
            Some(sw) => (sw.pending_set(), m.caps.gain.stages(), m.radio.hw_streaming),
            None => return,
        }
    };

    // No frames means no readings, and a sweep that is never fed never ends.
    // Put the stage back and say so rather than leaving the radio parked at some
    // intermediate step of a measurement nobody is watching.
    if !streaming && pending.is_none() {
        let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(sw) = m.lab.noise_sweep.as_mut() {
            sw.abort();
        }
        m.push_log("Noise sweep: streaming stopped, restoring gain".to_string());
        return;
    }

    let Some((idx, db)) = pending else {
        return;
    };
    let Some(spec) = stages.get(idx) else {
        return;
    };

    // Device set with no lock held.
    let outcome = device.set_stage_gain(idx, &spec.name, db);

    let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
    let Some(sw) = m.lab.noise_sweep.as_mut() else {
        return;
    };
    match outcome {
        Ok(()) => {
            sw.applied();
            let finished = sw.is_finished();
            let reading = finished.then(|| sw.reading()).flatten();
            if let Some(g) = m.radio.gains.get_mut(idx) {
                *g = db;
            }
            if finished {
                m.lab.noise_sweep = None;
                // A stopped sweep returns no reading, and the stop was already
                // logged where it was asked for: a second line saying the same
                // thing is noise.
                if let Some(r) = reading {
                    // The knee and the slope above it, matching the panel. The
                    // span slope averages the converter-limited half with the
                    // front-end-limited half and describes neither, so it is not
                    // quoted here either.
                    m.push_log(match (r.knee_db, r.slope_above_knee) {
                        (Some(k), Some(above)) => format!(
                            "Noise sweep: knee at {k:.0} dB, {above:.2} dB/dB above it ({} points)",
                            r.points.len()
                        ),
                        _ => format!(
                            "Noise sweep: no knee in range over {} points \u{2014} the converter set the floor throughout",
                            r.points.len()
                        ),
                    });
                    m.lab.noise_reading = Some(crate::state::NoiseReading {
                        at_hz: m.radio.frequency,
                        reading: r,
                    });
                }
            }
        }
        Err(e) => {
            // The radio refused mid-measurement. Anything gathered so far is a
            // sweep of a chain that did not do what it was told, so it is
            // dropped rather than reported.
            m.lab.noise_sweep = None;
            m.push_log(format!("Noise sweep: {}: {e}", spec.name));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /// **Auto-track's comfortable window is under the radio's ceiling**: a
    /// peak at -6 dBFS is 3 dB from an R2's tuner limit, too hot, and the
    /// chain comes down; on a HackRF it is inside the window and left alone.
    #[test]
    fn auto_track_keeps_clear_of_the_radios_ceiling() {
        let r2 = crate::hardware::native::airspy::bench::BenchR2::new();
        let mut m = SdrMetrics::fixture().band_measured_chain().on_curve(0, 15);
        m.radio.frequency = 92_800_000;
        m.lab.rf_autotrack = true;
        let before: f64 = m.radio.gains.iter().sum();
        let state = Arc::new(Mutex::new(m));
        track_gain(&state, &r2.device, -6.0);
        let after: f64 = state.lock().unwrap().radio.gains.iter().sum();
        assert!(after < before, "{before} -> {after}");
    }

    /// **Auto-track stops under the AGC**: the tuner is setting the stages it
    /// would move.
    #[test]
    fn auto_track_stops_under_the_agc() {
        let r2 = crate::hardware::native::airspy::bench::BenchR2::new();
        let mut m = SdrMetrics::fixture().band_measured_chain().on_curve(0, 10);
        m.radio.frequency = 92_800_000;
        m.radio.amp_enabled = true;
        m.lab.rf_autotrack = true;
        let state = Arc::new(Mutex::new(m));
        track_gain(&state, &r2.device, -30.0);
        let m = state.lock().unwrap();
        assert!(!m.lab.rf_autotrack);
        let last = m.ui.log.back().map(|e| e.text.clone()).unwrap_or_default();
        assert!(last.contains("the tuner AGC sets LNA and MIX"), "{last}");
        assert_eq!(r2.registers(), [-1, -1, -1]);
    }

    /// **Auto-track on a curve stays on it**: the chain it sets is a step of
    /// the curve, in the radio's registers and in the state.
    #[test]
    fn auto_track_on_a_curve_lands_on_a_step_of_it() {
        let r2 = crate::hardware::native::airspy::bench::BenchR2::new();
        let mut m = SdrMetrics::fixture().band_measured_chain().on_curve(0, 10);
        m.radio.frequency = 92_800_000;
        m.lab.rf_autotrack = true;
        let state = Arc::new(Mutex::new(m));
        track_gain(&state, &r2.device, -30.0);
        let m = state.lock().unwrap();
        let gm = &m.caps.gain;
        let step = gm.curves()[0].step_of(&gm.stages(), &m.radio.gains);
        assert!(step.is_some_and(|s| s > 10), "{:?}", m.radio.gains);
        let last = m.ui.log.back().map(|e| e.text.clone()).unwrap_or_default();
        assert!(
            last.contains(&format!("Linearity {}/21", step.unwrap())),
            "{last}"
        );
        let positions: Vec<i32> = gm
            .stages()
            .iter()
            .zip(&m.radio.gains)
            .map(|(s, &db)| s.position_of(db).unwrap() as i32)
            .collect();
        assert_eq!(r2.registers().to_vec(), positions);
    }

    /// **Auto-track stops where a stage's dB stops holding**: tuned out of the
    /// R2's LNA band with the peak off its window, it switches itself off and
    /// says why, and writes no register; inside the band it moves the chain.
    #[test]
    fn auto_track_stops_outside_a_stages_measured_band() {
        let r2 = crate::hardware::native::airspy::bench::BenchR2::new();
        let mut m = SdrMetrics::fixture().band_measured_chain();
        m.lab.rf_autotrack = true;
        let state = Arc::new(Mutex::new(m));
        track_gain(&state, &r2.device, -30.0);
        let m = state.lock().unwrap();
        assert!(!m.lab.rf_autotrack);
        let last = m.ui.log.back().map(|e| e.text.clone()).unwrap_or_default();
        assert!(last.contains("LNA's gain in dB was measured"), "{last}");
        assert_eq!(r2.registers(), [-1, -1, -1]);
        drop(m);

        let mut m = SdrMetrics::fixture().band_measured_chain();
        m.radio.frequency = 92_800_000;
        m.radio.gains = vec![9.09, 8.25, 6.80];
        m.lab.rf_autotrack = true;
        let state = Arc::new(Mutex::new(m));
        track_gain(&state, &r2.device, -30.0);
        assert!(state.lock().unwrap().lab.rf_autotrack);
        assert!(
            r2.registers().iter().all(|&r| r >= 0),
            "{:?}",
            r2.registers()
        );
    }

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

    #[test]
    fn request_transitions_call_only_the_needed_device_action() {
        let starts = Cell::new(0);
        let stops = Cell::new(0);
        let started = request_transition(
            true,
            false,
            || {
                starts.set(starts.get() + 1);
                Ok(())
            },
            || {
                stops.set(stops.get() + 1);
                Ok(())
            },
        );
        assert!(matches!(started, RxRequestTransition::Started));
        assert_eq!((starts.get(), stops.get()), (1, 0));

        let stopped = request_transition(
            false,
            true,
            || {
                starts.set(starts.get() + 1);
                Ok(())
            },
            || {
                stops.set(stops.get() + 1);
                Ok(())
            },
        );
        assert!(matches!(stopped, RxRequestTransition::Stopped(Ok(()))));
        assert_eq!((starts.get(), stops.get()), (1, 1));
    }

    #[test]
    fn unchanged_requests_do_not_call_the_device() {
        for active in [false, true] {
            let transition = request_transition(
                active,
                active,
                || panic!("unexpected start"),
                || panic!("unexpected stop"),
            );
            assert!(matches!(transition, RxRequestTransition::Unchanged(value) if value == active));
        }
    }

    #[test]
    fn request_transitions_preserve_device_errors() {
        let started = request_transition(
            true,
            false,
            || anyhow::bail!("start failed"),
            || panic!("unexpected stop"),
        );
        assert!(
            matches!(started, RxRequestTransition::StartFailed(error) if error.to_string() == "start failed")
        );

        let stopped = request_transition(
            false,
            true,
            || panic!("unexpected start"),
            || anyhow::bail!("stop failed"),
        );
        assert!(
            matches!(stopped, RxRequestTransition::Stopped(Err(error)) if error.to_string() == "stop failed")
        );
    }

    #[test]
    fn unexpected_stop_runs_cleanup_once() {
        let cleanups = Cell::new(0);
        let result = unexpected_stop(true, false, || {
            cleanups.set(cleanups.get() + 1);
            anyhow::bail!("cleanup failed")
        });
        assert!(matches!(result, Some(Err(_))));
        assert_eq!(cleanups.get(), 1);
        for (active, streaming) in [(false, false), (false, true), (true, true)] {
            assert!(unexpected_stop(active, streaming, || panic!("unexpected cleanup")).is_none());
        }
        assert!(matches!(
            unexpected_stop(true, false, || Ok(())),
            Some(Ok(()))
        ));
    }
}
