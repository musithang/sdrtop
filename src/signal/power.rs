// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;

use crate::hardware::{PowerTrace, PowerTraceTarget};
use crate::state::{FftFrame, SdrMetrics, SweepFrame};

const EMA_ALPHA: f32 = 0.2;
const PEAK_DECAY_DB: f32 = 0.5;
const REJECTION_REPORT_INTERVAL: Duration = Duration::from_secs(5);

pub struct PowerWorker {
    trace_rx: Receiver<PowerTrace>,
    state: Arc<Mutex<SdrMetrics>>,
}

impl PowerWorker {
    pub fn new(trace_rx: Receiver<PowerTrace>, state: Arc<Mutex<SdrMetrics>>) -> Self {
        Self { trace_rx, state }
    }

    pub fn run(self) {
        let mut spectrum = SpectrumAccumulator::default();
        let mut sweep = SweepAccumulator::default();
        let mut rejection_reporter = RejectionReporter::default();
        while let Ok(trace) = self.trace_rx.recv() {
            let result = match trace.target {
                PowerTraceTarget::Spectrum => spectrum.publish(&self.state, trace),
                PowerTraceTarget::Sweep => sweep.push(&self.state, trace),
            };
            if let Err(reason) = result {
                rejection_reporter.report(&self.state, reason, Instant::now());
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TraceRejection {
    Empty,
    LengthMismatch,
    NonUniformGrid,
    NonAscendingGrid,
}

impl TraceRejection {
    fn message(self) -> &'static str {
        match self {
            Self::Empty => "empty trace",
            Self::LengthMismatch => "frequency and level counts differ",
            Self::NonUniformGrid => "frequency grid is not uniform and ascending",
            Self::NonAscendingGrid => "frequency grid is not strictly ascending",
        }
    }
}

#[derive(Default)]
struct RejectionReporter {
    last_report: Option<Instant>,
    suppressed: usize,
}

impl RejectionReporter {
    fn report(&mut self, state: &Arc<Mutex<SdrMetrics>>, reason: TraceRejection, now: Instant) {
        let should_report = self
            .last_report
            .is_none_or(|last| now.duration_since(last) >= REJECTION_REPORT_INTERVAL);
        if !should_report {
            self.suppressed += 1;
            return;
        }

        let suffix = match self.suppressed {
            0 => String::new(),
            1 => "; 1 additional trace rejected".to_string(),
            count => format!("; {count} additional traces rejected"),
        };
        let mut metrics = state.lock().unwrap_or_else(|error| error.into_inner());
        metrics.push_log(format!(
            "Power trace rejected: {}{suffix}",
            reason.message()
        ));
        self.last_report = Some(now);
        self.suppressed = 0;
    }
}

#[derive(Default)]
struct SpectrumAccumulator {
    start_hz: u64,
    stop_hz: u64,
    rbw_hz: Option<u32>,
    smoothed: Vec<f32>,
    peak: Vec<f32>,
    noise_scratch: Vec<f32>,
}

impl SpectrumAccumulator {
    fn publish(
        &mut self,
        state: &Arc<Mutex<SdrMetrics>>,
        trace: PowerTrace,
    ) -> Result<(), TraceRejection> {
        if trace.frequencies_hz.is_empty() {
            return Err(TraceRejection::Empty);
        }
        if trace.frequencies_hz.len() != trace.levels_dbm.len() {
            return Err(TraceRejection::LengthMismatch);
        }
        let Some((axis_start_hz, sample_rate)) = trace_window(&trace.frequencies_hz) else {
            return Err(TraceRejection::NonUniformGrid);
        };
        let start_hz = trace.frequencies_hz[0];
        let stop_hz = *trace.frequencies_hz.last().unwrap_or(&start_hz);
        let center_freq_hz = start_hz.saturating_add(stop_hz.saturating_sub(start_hz) / 2);
        let reset = self.start_hz != start_hz
            || self.stop_hz != stop_hz
            || self.rbw_hz != trace.rbw_hz
            || self.smoothed.len() != trace.levels_dbm.len();
        if reset {
            self.start_hz = start_hz;
            self.stop_hz = stop_hz;
            self.rbw_hz = trace.rbw_hz;
            self.smoothed
                .resize(trace.levels_dbm.len(), f32::NEG_INFINITY);
            self.peak.resize(trace.levels_dbm.len(), f32::NEG_INFINITY);
        }
        crate::signal::stats::average_and_peak(
            &trace.levels_dbm,
            &mut self.smoothed,
            &mut self.peak,
            EMA_ALPHA,
            PEAK_DECAY_DB,
            !reset,
        );

        self.noise_scratch.resize(self.smoothed.len(), 0.0);
        let noise_floor =
            crate::signal::stats::quietest_mean(&self.smoothed, &mut self.noise_scratch, 10);
        let strongest = self
            .smoothed
            .iter()
            .copied()
            .filter(|value| value.is_finite())
            .fold(f32::NEG_INFINITY, f32::max);
        let prominence = if strongest.is_finite() && noise_floor.is_finite() {
            strongest - noise_floor
        } else {
            0.0
        };

        let bins = Arc::new(self.smoothed.clone());
        let peak = Arc::new(self.peak.clone());
        let mut metrics = state.lock().unwrap_or_else(|error| error.into_inner());
        metrics.waterfall.buffer.push(&bins);
        metrics.signal.peak_to_nf_db = prominence;
        metrics.waterfall.last_fft = Some(FftFrame {
            bins_dbfs: bins,
            peak_hold: peak,
            noise_floor,
            center_freq_hz,
            axis_start_hz,
            sample_rate,
            timestamp: Instant::now(),
            peak_to_nf_db: prominence,
            occupied_bw_hz: 0,
            enbw_hz: trace.rbw_hz.unwrap_or(0) as f64,
            bin_axis: crate::state::BinAxis::MeasuredPoints,
        });
        // The Lab prints its channel figures from the readout frame. A trace
        // is one whole sweep, already averaged, and arrives slower than the
        // readout's four a second, so every trace is taken.
        metrics.signal.readout = metrics.waterfall.last_fft.clone();
        Ok(())
    }
}

#[derive(Default)]
struct SweepAccumulator {
    generation: u64,
    start_hz: u64,
    stop_hz: u64,
    frequencies_hz: Vec<u64>,
    sums: Vec<f64>,
    samples: Vec<u32>,
    peak: Vec<f32>,
    traces: u32,
    started: Option<Instant>,
}

impl SweepAccumulator {
    fn push(
        &mut self,
        state: &Arc<Mutex<SdrMetrics>>,
        trace: PowerTrace,
    ) -> Result<(), TraceRejection> {
        let requested = {
            let metrics = state.lock().unwrap_or_else(|error| error.into_inner());
            (
                metrics.sweep.active,
                metrics.sweep.generation,
                metrics.sweep.config.start_hz,
                metrics.sweep.config.stop_hz,
                metrics.sweep.config.dwell_ms,
            )
        };
        if !requested.0 {
            self.reset();
            return Ok(());
        }
        if trace.generation != requested.1 {
            return Ok(());
        }
        if trace.frequencies_hz.is_empty() {
            return Err(TraceRejection::Empty);
        }
        if trace.frequencies_hz.len() != trace.levels_dbm.len() {
            return Err(TraceRejection::LengthMismatch);
        }
        if trace
            .frequencies_hz
            .windows(2)
            .any(|pair| pair[1] <= pair[0])
        {
            return Err(TraceRejection::NonAscendingGrid);
        }
        if !trace_covers_requested_range(&trace.frequencies_hz, requested.2, requested.3) {
            return Ok(());
        }

        let changed = self.generation != trace.generation
            || self.start_hz != requested.2
            || self.stop_hz != requested.3
            || self.frequencies_hz != trace.frequencies_hz;
        if changed {
            self.generation = trace.generation;
            self.start_hz = requested.2;
            self.stop_hz = requested.3;
            self.frequencies_hz.clone_from(&trace.frequencies_hz);
            self.sums = vec![0.0; trace.levels_dbm.len()];
            self.samples = vec![0; trace.levels_dbm.len()];
            self.peak = vec![f32::NEG_INFINITY; trace.levels_dbm.len()];
            self.traces = 0;
            self.started = Some(Instant::now());
        }

        for (((sum, samples), peak), level) in self
            .sums
            .iter_mut()
            .zip(self.samples.iter_mut())
            .zip(self.peak.iter_mut())
            .zip(trace.levels_dbm.iter().copied())
        {
            if level.is_finite() {
                *sum += level as f64;
                *samples += 1;
                *peak = peak.max(level);
            }
        }
        self.traces += 1;

        let elapsed = self
            .started
            .map(|started| started.elapsed())
            .unwrap_or_default();
        {
            let mut metrics = state.lock().unwrap_or_else(|error| error.into_inner());
            metrics.sweep.positions_total = self.frequencies_hz.len();
            metrics.sweep.positions_done = self.frequencies_hz.len();
            metrics.sweep.current_hz = *self.frequencies_hz.last().unwrap_or(&self.start_hz);
        }
        if elapsed.as_millis() < requested.4 as u128 {
            return Ok(());
        }

        let mean = self
            .sums
            .iter()
            .zip(&self.samples)
            .map(|(sum, samples)| {
                if *samples == 0 {
                    f32::NEG_INFINITY
                } else {
                    (sum / *samples as f64) as f32
                }
            })
            .collect();
        let duration_ms = elapsed.as_millis() as u64;
        let mut metrics = state.lock().unwrap_or_else(|error| error.into_inner());
        if !metrics.sweep.active
            || metrics.sweep.generation != self.generation
            || metrics.sweep.config.start_hz != self.start_hz
            || metrics.sweep.config.stop_hz != self.stop_hz
        {
            return Ok(());
        }
        metrics.sweep.cycle_count += 1;
        metrics.sweep.cycle_duration_ms = duration_ms;
        metrics.sweep.current_frame = Some(Arc::new(SweepFrame {
            start_hz: self.start_hz,
            stop_hz: self.stop_hz,
            freq_hz: self.frequencies_hz.clone(),
            peak_dbfs: self.peak.clone(),
            mean_dbfs: mean,
            timestamp: Instant::now(),
            cycle_count: metrics.sweep.cycle_count,
            cycle_duration_ms: duration_ms,
        }));
        drop(metrics);

        self.sums.fill(0.0);
        self.samples.fill(0);
        self.peak.fill(f32::NEG_INFINITY);
        self.traces = 0;
        self.started = Some(Instant::now());
        Ok(())
    }

    fn reset(&mut self) {
        *self = Self::default();
    }
}

fn trace_covers_requested_range(frequencies_hz: &[u64], start_hz: u64, stop_hz: u64) -> bool {
    let Some((&first, rest)) = frequencies_hz.split_first() else {
        return false;
    };
    let Some(&last) = rest.last() else {
        return false;
    };
    if first != start_hz || last > stop_hz {
        return false;
    }
    let points = frequencies_hz.len() as u64;
    let requested_span = stop_hz.saturating_sub(start_hz);
    let remainder = requested_span % points;
    let rounding_tolerance = requested_span
        .div_ceil(1_u64 << (f32::MANTISSA_DIGITS - 1))
        .saturating_add(1);
    let max_spacing = frequencies_hz
        .windows(2)
        .map(|pair| pair[1] - pair[0])
        .max()
        .unwrap_or_default();
    let allowed_gap = max_spacing
        .saturating_add(remainder)
        .saturating_add(rounding_tolerance);
    max_spacing > 0 && stop_hz.saturating_sub(last) <= allowed_gap
}

pub(crate) fn trace_window(frequencies_hz: &[u64]) -> Option<(f64, f64)> {
    let start = *frequencies_hz.first()?;
    let stop = *frequencies_hz.last()?;
    let intervals = frequencies_hz.len().checked_sub(1)?;
    let span = stop.checked_sub(start)?;
    if span == 0 {
        return None;
    }
    let low_step = span / intervals as u64;
    let high_step = span.div_ceil(intervals as u64);
    if !frequencies_hz.windows(2).all(|pair| {
        pair[1]
            .checked_sub(pair[0])
            .is_some_and(|step| step > 0 && (low_step..=high_step).contains(&step))
    }) {
        return None;
    }
    Some((start as f64, span as f64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_worker_publishes_spectrum_and_waterfall_frames() {
        let state = Arc::new(Mutex::new(SdrMetrics::fixture()));
        let (tx, rx) = crossbeam_channel::bounded(1);
        let worker_state = Arc::clone(&state);
        let worker = std::thread::spawn(move || PowerWorker::new(rx, worker_state).run());

        tx.send(PowerTrace {
            target: PowerTraceTarget::Spectrum,
            generation: 0,
            frequencies_hz: vec![100_000_000, 101_000_000, 102_000_000],
            levels_dbm: vec![-90.0, -45.0, -80.0],
            rbw_hz: Some(10_000),
        })
        .unwrap();
        drop(tx);
        worker.join().unwrap();

        let metrics = state.lock().unwrap_or_else(|error| error.into_inner());
        let frame = metrics.waterfall.last_fft.as_ref().unwrap();
        assert_eq!(frame.bins_dbfs.as_slice(), &[-90.0, -45.0, -80.0]);
        assert_eq!(frame.peak_hold.as_slice(), &[-90.0, -45.0, -80.0]);
        assert_eq!(frame.center_freq_hz, 101_000_000);
        assert_eq!(frame.axis_start_hz, 100_000_000.0);
        assert_eq!(frame.sample_rate, 2_000_000.0);
        assert_eq!(frame.enbw_hz, 10_000.0);
        assert_eq!(metrics.waterfall.buffer.rows.len(), 1);
        assert_eq!(metrics.signal.peak_to_nf_db, 45.0);
        assert_eq!(metrics.radio.frequency, 100_000_000);
        assert_eq!(metrics.signal.channel_power_dbfs, f32::NEG_INFINITY);
        assert_eq!(
            metrics.signal.readout.as_ref().map(|r| r.timestamp),
            Some(frame.timestamp),
            "the Lab's channel figures read the trace"
        );
        assert!(metrics
            .ui
            .log
            .iter()
            .all(|entry| !entry.text.contains("Power trace rejected")));
    }

    #[test]
    fn noise_floor_uses_the_quiet_part_of_the_trace() {
        let mut levels = vec![-100.0; 80];
        levels.extend([-30.0; 20]);
        let mut scratch = vec![0.0; levels.len()];
        assert_eq!(
            crate::signal::stats::quietest_mean(&levels, &mut scratch, 10),
            -100.0
        );
    }

    #[test]
    fn smoothing_and_peak_hold_stay_ordered() {
        let state = Arc::new(Mutex::new(SdrMetrics::fixture()));
        let mut accumulator = SpectrumAccumulator::default();
        for levels in [[-90.0, -80.0], [-91.0, -70.0], [f32::NAN, -72.0]] {
            accumulator
                .publish(
                    &state,
                    PowerTrace {
                        target: PowerTraceTarget::Spectrum,
                        generation: 0,
                        frequencies_hz: vec![100_000_000, 101_000_000],
                        levels_dbm: levels.to_vec(),
                        rbw_hz: None,
                    },
                )
                .unwrap();
        }

        let metrics = state.lock().unwrap_or_else(|error| error.into_inner());
        let frame = metrics.waterfall.last_fft.as_ref().unwrap();
        assert!((frame.bins_dbfs[0] - -90.2).abs() < 1e-4);
        assert!((frame.bins_dbfs[1] - -76.8).abs() < 1e-4);
        assert!((frame.peak_hold[0] - -90.2).abs() < 1e-4);
        assert!((frame.peak_hold[1] - -76.8).abs() < 1e-4);
        for (peak, smoothed) in frame.peak_hold.iter().zip(frame.bins_dbfs.iter()) {
            assert!(peak >= smoothed);
        }
    }

    #[test]
    fn a_finite_level_recovers_a_non_finite_bin() {
        let state = Arc::new(Mutex::new(SdrMetrics::fixture()));
        let mut accumulator = SpectrumAccumulator::default();
        accumulator
            .publish(
                &state,
                PowerTrace {
                    target: PowerTraceTarget::Spectrum,
                    generation: 0,
                    frequencies_hz: vec![100_000_000, 101_000_000],
                    levels_dbm: vec![f32::NAN, -90.0],
                    rbw_hz: None,
                },
            )
            .unwrap();
        accumulator
            .publish(
                &state,
                PowerTrace {
                    target: PowerTraceTarget::Spectrum,
                    generation: 0,
                    frequencies_hz: vec![100_000_000, 101_000_000],
                    levels_dbm: vec![-70.0, -80.0],
                    rbw_hz: None,
                },
            )
            .unwrap();

        let metrics = state.lock().unwrap_or_else(|error| error.into_inner());
        let frame = metrics.waterfall.last_fft.as_ref().unwrap();
        assert_eq!(frame.bins_dbfs[0], -70.0);
        assert!(frame.bins_dbfs[1].is_finite());
        assert!(frame.peak_hold[0] >= frame.bins_dbfs[0]);
    }

    #[test]
    fn non_finite_initial_levels_publish_as_unavailable() {
        let state = Arc::new(Mutex::new(SdrMetrics::fixture()));
        let mut accumulator = SpectrumAccumulator::default();
        accumulator
            .publish(
                &state,
                PowerTrace {
                    target: PowerTraceTarget::Spectrum,
                    generation: 0,
                    frequencies_hz: vec![100_000_000, 101_000_000],
                    levels_dbm: vec![f32::NAN, f32::INFINITY],
                    rbw_hz: None,
                },
            )
            .unwrap();

        let metrics = state.lock().unwrap_or_else(|error| error.into_inner());
        let frame = metrics.waterfall.last_fft.as_ref().unwrap();
        assert_eq!(
            frame.bins_dbfs.as_slice(),
            &[f32::NEG_INFINITY, f32::NEG_INFINITY]
        );
        assert_eq!(frame.peak_hold.as_slice(), frame.bins_dbfs.as_slice());
        assert_eq!(frame.noise_floor, f32::NEG_INFINITY);
    }

    #[test]
    fn an_rbw_change_resets_smoothing() {
        let state = Arc::new(Mutex::new(SdrMetrics::fixture()));
        let mut accumulator = SpectrumAccumulator::default();
        for (rbw_hz, levels) in [(Some(10_000), -90.0), (Some(20_000), -40.0)] {
            accumulator
                .publish(
                    &state,
                    PowerTrace {
                        target: PowerTraceTarget::Spectrum,
                        generation: 0,
                        frequencies_hz: vec![100_000_000, 101_000_000],
                        levels_dbm: vec![levels; 2],
                        rbw_hz,
                    },
                )
                .unwrap();
        }
        let metrics = state.lock().unwrap_or_else(|error| error.into_inner());
        let frame = metrics.waterfall.last_fft.as_ref().unwrap();
        assert_eq!(frame.bins_dbfs.as_slice(), &[-40.0, -40.0]);
        assert_eq!(frame.peak_hold.as_slice(), &[-40.0, -40.0]);
        assert_eq!(frame.enbw_hz, 20_000.0);
    }

    #[test]
    fn rejected_traces_are_reported_at_a_bounded_rate() {
        let state = Arc::new(Mutex::new(SdrMetrics::fixture()));
        let mut reporter = RejectionReporter::default();
        let now = Instant::now();
        reporter.report(&state, TraceRejection::Empty, now);
        reporter.report(
            &state,
            TraceRejection::LengthMismatch,
            now + Duration::from_secs(1),
        );
        reporter.report(
            &state,
            TraceRejection::NonUniformGrid,
            now + REJECTION_REPORT_INTERVAL,
        );

        let metrics = state.lock().unwrap_or_else(|error| error.into_inner());
        assert_eq!(metrics.ui.log.len(), 2);
        assert!(metrics.ui.log[0].text.contains("empty trace"));
        assert!(metrics.ui.log[1]
            .text
            .contains("1 additional trace rejected"));
    }

    #[test]
    fn invalid_traces_return_specific_rejections() {
        let state = Arc::new(Mutex::new(SdrMetrics::fixture()));
        let mut accumulator = SpectrumAccumulator::default();
        let cases = [
            (
                PowerTrace {
                    target: PowerTraceTarget::Spectrum,
                    generation: 0,
                    frequencies_hz: vec![],
                    levels_dbm: vec![],
                    rbw_hz: None,
                },
                TraceRejection::Empty,
            ),
            (
                PowerTrace {
                    target: PowerTraceTarget::Spectrum,
                    generation: 0,
                    frequencies_hz: vec![100, 200],
                    levels_dbm: vec![-90.0],
                    rbw_hz: None,
                },
                TraceRejection::LengthMismatch,
            ),
            (
                PowerTrace {
                    target: PowerTraceTarget::Spectrum,
                    generation: 0,
                    frequencies_hz: vec![100, 200, 350],
                    levels_dbm: vec![-90.0; 3],
                    rbw_hz: None,
                },
                TraceRejection::NonUniformGrid,
            ),
        ];
        for (trace, expected) in cases {
            assert_eq!(accumulator.publish(&state, trace), Err(expected));
        }
    }

    #[test]
    fn trace_window_uses_the_measured_edges() {
        assert_eq!(
            trace_window(&[100_000, 200_000, 300_000]),
            Some((100_000.0, 200_000.0))
        );
        assert_eq!(trace_window(&[100_000]), None);
        assert_eq!(trace_window(&[100_000, 200_000, 350_000]), None);
        assert_eq!(trace_window(&[300_000, 200_000, 100_000]), None);
        assert_eq!(
            trace_window(&[100_000, 133_333, 166_667, 200_000]),
            Some((100_000.0, 100_000.0))
        );
    }

    #[test]
    fn odd_span_trace_keeps_its_first_and_last_frequencies() {
        let state = Arc::new(Mutex::new(SdrMetrics::fixture()));
        let mut accumulator = SpectrumAccumulator::default();
        accumulator
            .publish(
                &state,
                PowerTrace {
                    target: PowerTraceTarget::Spectrum,
                    generation: 0,
                    frequencies_hz: vec![100_000, 100_001, 100_002, 100_003],
                    levels_dbm: vec![-90.0, -80.0, -70.0, -60.0],
                    rbw_hz: None,
                },
            )
            .unwrap();

        let metrics = state.lock().unwrap_or_else(|error| error.into_inner());
        let frame = metrics.waterfall.last_fft.as_ref().unwrap();
        assert_eq!(frame.frequency_of_bin(0), Some(100_000.0));
        assert_eq!(frame.frequency_of_bin(3), Some(100_003.0));
    }

    fn sweep_trace(generation: u64, levels_dbm: Vec<f32>) -> PowerTrace {
        PowerTrace {
            target: PowerTraceTarget::Sweep,
            generation,
            frequencies_hz: vec![100_000_000, 100_500_000, 101_000_000],
            levels_dbm,
            rbw_hz: None,
        }
    }

    fn sweeping_state(generation: u64) -> Arc<Mutex<SdrMetrics>> {
        let state = Arc::new(Mutex::new(SdrMetrics::fixture()));
        {
            let mut metrics = state.lock().unwrap();
            metrics.sweep.active = true;
            metrics.sweep.generation = generation;
            metrics.sweep.config.start_hz = 100_000_000;
            metrics.sweep.config.stop_hz = 101_000_000;
            metrics.sweep.config.dwell_ms = 10_000;
        }
        state
    }

    #[test]
    fn sweep_accumulation_publishes_measured_peak_and_mean() {
        let state = sweeping_state(4);
        let mut accumulator = SweepAccumulator::default();
        accumulator
            .push(&state, sweep_trace(4, vec![-80.0, f32::NAN, -60.0]))
            .unwrap();
        accumulator.started = Some(Instant::now() - std::time::Duration::from_secs(11));
        accumulator
            .push(&state, sweep_trace(4, vec![-70.0, -50.0, -90.0]))
            .unwrap();

        let metrics = state.lock().unwrap();
        let frame = metrics.sweep.current_frame.as_ref().unwrap();
        assert_eq!(frame.peak_dbfs, [-70.0, -50.0, -60.0]);
        assert_eq!(frame.mean_dbfs, [-75.0, -50.0, -75.0]);
        assert_eq!(frame.freq_hz, [100_000_000, 100_500_000, 101_000_000]);
        assert_eq!(metrics.sweep.positions_total, 3);
        assert_eq!(metrics.sweep.positions_done, 3);
    }

    #[test]
    fn sweep_generations_isolate_queued_traces() {
        let state = sweeping_state(8);
        let mut accumulator = SweepAccumulator::default();
        accumulator
            .push(&state, sweep_trace(7, vec![-10.0, -10.0, -10.0]))
            .unwrap();
        assert_eq!(accumulator.traces, 0);

        accumulator
            .push(&state, sweep_trace(8, vec![-80.0, -70.0, -60.0]))
            .unwrap();
        assert_eq!(accumulator.traces, 1);
        state.lock().unwrap().sweep.generation = 9;
        accumulator
            .push(&state, sweep_trace(8, vec![-20.0, -20.0, -20.0]))
            .unwrap();
        assert_eq!(accumulator.traces, 1);

        accumulator
            .push(&state, sweep_trace(9, vec![-90.0, -80.0, -70.0]))
            .unwrap();
        assert_eq!(accumulator.generation, 9);
        assert_eq!(accumulator.traces, 1);
        assert_eq!(accumulator.sums, [-90.0, -80.0, -70.0]);
    }

    #[test]
    fn stale_sweep_traces_are_discarded_before_shape_validation() {
        let state = sweeping_state(8);
        let mut accumulator = SweepAccumulator::default();
        let malformed = PowerTrace {
            target: PowerTraceTarget::Sweep,
            generation: 7,
            frequencies_hz: vec![],
            levels_dbm: vec![-80.0],
            rbw_hz: None,
        };
        assert_eq!(accumulator.push(&state, malformed), Ok(()));

        let mut matching = sweep_trace(8, vec![-80.0, -70.0, -60.0]);
        matching.frequencies_hz = vec![100_000_000, 100_000_000, 101_000_000];
        assert_eq!(
            accumulator.push(&state, matching),
            Err(TraceRejection::NonAscendingGrid)
        );

        state.lock().unwrap().sweep.active = false;
        let inactive = PowerTrace {
            target: PowerTraceTarget::Sweep,
            generation: 8,
            frequencies_hz: vec![],
            levels_dbm: vec![-80.0],
            rbw_hz: None,
        };
        assert_eq!(accumulator.push(&state, inactive), Ok(()));
    }

    #[test]
    fn a_trace_from_a_previous_range_is_not_relabelled() {
        assert!(trace_covers_requested_range(
            &[100_000_000, 100_499_999, 100_999_998],
            100_000_000,
            101_000_000,
        ));
        assert!(!trace_covers_requested_range(
            &[100_000_000, 100_500_000, 101_000_000],
            90_000_000,
            110_000_000,
        ));
        assert!(!trace_covers_requested_range(
            &[100_000_000, 100_250_000, 100_500_000],
            100_000_000,
            101_000_000,
        ));
    }

    #[test]
    fn native_half_open_sweep_grid_covers_requested_range() {
        let frequencies = (0..450)
            .map(|index| 400_000_000 + (222_222.0_f32 * index as f32) as u64)
            .collect::<Vec<_>>();

        assert_eq!(frequencies.last(), Some(&499_777_680));
        assert!(trace_covers_requested_range(
            &frequencies,
            400_000_000,
            500_000_000,
        ));
    }

    #[test]
    fn native_half_open_grid_allows_f32_offset_rounding() {
        let start_hz = 100_000;
        let stop_hz = 270_508_336;
        let points = 450;
        let step = ((stop_hz - start_hz) / points) as f32;
        let frequencies = (0..points)
            .map(|index| start_hz + (step * index as f32) as u64)
            .collect::<Vec<_>>();

        assert_eq!(frequencies.last(), Some(&269_907_232));
        assert!(trace_covers_requested_range(
            &frequencies,
            start_hz,
            stop_hz,
        ));
    }
}
