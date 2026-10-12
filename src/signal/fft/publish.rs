// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The lock block: the only place this worker writes to the shared state.
//!
//! Everything expensive has already been computed by [`super::analysis`], so the
//! mutex is held for a sequence of writes and one buffer copy - not for a
//! spectrum's worth of `powf`. The one read taken while holding it is the lab's
//! averaging factor, which is a single field and would cost a second lock
//! acquisition to fetch on its own.
//!
//! Split out so the boundary is visible in the file layout rather than in a
//! comment, the way `tasks/rx/` makes its two lock blocks visible.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::state::{FftFrame, SdrMetrics};

use super::analysis::Reading;
use super::DB_FLOOR;

/// Pace the drawn spectrum to the *visible* waterfall: each waterfall character
/// row packs this many data rows (half-block ▀), so the spectrum refreshes once
/// per visible line rather than every FFT frame, keeping the two panels moving in
/// lockstep. Signal metrics stay at full rate.
const ROWS_PER_WATERFALL_LINE: u32 = 2;
const S_METER_TIME_CONSTANT: Duration = Duration::from_millis(300);
const S_METER_PEAK_HOLD: Duration = Duration::from_secs(1);

/// How much of a marker's channel the measured-bandwidth cut-offs bracket.
const MARKER_BW_LOW: f32 = 0.005;
const MARKER_BW_HIGH: f32 = 0.995;

/// The display-paced spectrum refresh's state, carried between frames.
pub(super) struct Pacing {
    rows_since_spectrum: u32,
    last_update: Option<Instant>,
    meter_dbfs: Option<f32>,
    meter_peak_dbfs: Option<f32>,
    meter_peak_held_until: Option<Instant>,
    last_meter_update: Option<Instant>,
}

impl Pacing {
    pub(super) fn new() -> Self {
        Self {
            rows_since_spectrum: 0,
            last_update: None,
            meter_dbfs: None,
            meter_peak_dbfs: None,
            meter_peak_held_until: None,
            last_meter_update: None,
        }
    }

    /// Apply the S-meter's display-only ballistics outside the state lock.
    fn s_meter(&mut self, reading_dbfs: f32, now: Instant) -> (Option<f32>, Option<f32>) {
        let dt = self
            .last_meter_update
            .map(|last| now.duration_since(last))
            .unwrap_or_default();
        self.last_meter_update = Some(now);

        if dt > Duration::from_secs(1) {
            self.meter_dbfs = None;
            self.meter_peak_dbfs = None;
            self.meter_peak_held_until = None;
        }

        let Some(previous) = self.meter_dbfs else {
            if reading_dbfs.is_finite() {
                self.meter_dbfs = Some(reading_dbfs);
                self.meter_peak_dbfs = Some(reading_dbfs);
                self.meter_peak_held_until = Some(now + S_METER_PEAK_HOLD);
            }
            return (self.meter_dbfs, self.meter_peak_dbfs);
        };

        let elapsed = dt.as_secs_f32();
        let tau = S_METER_TIME_CONSTANT.as_secs_f32();
        let alpha = 1.0 - (-elapsed / tau).exp();
        let target_dbfs = if reading_dbfs.is_finite() {
            reading_dbfs
        } else {
            DB_FLOOR
        };
        let previous_power = 10.0_f32.powf(previous / 10.0);
        let target_power = 10.0_f32.powf(target_dbfs / 10.0);
        let filtered_power = previous_power + alpha * (target_power - previous_power);
        let filtered_dbfs = (10.0 * filtered_power.log10()).max(DB_FLOOR);
        self.meter_dbfs = Some(filtered_dbfs);

        if self
            .meter_peak_dbfs
            .is_none_or(|peak| filtered_dbfs >= peak)
        {
            self.meter_peak_dbfs = Some(filtered_dbfs);
            self.meter_peak_held_until = Some(now + S_METER_PEAK_HOLD);
        } else if self.meter_peak_held_until.is_some_and(|until| now >= until) {
            self.meter_peak_dbfs = Some(filtered_dbfs);
        }

        (self.meter_dbfs, self.meter_peak_dbfs)
    }

    /// Whether to redraw the spectrum: there is none yet, a visible waterfall line
    /// has gone by, or it would otherwise age toward `[STALE]`.
    fn due(&self, has_frame: bool, trace_stale_ms: u128, now: Instant) -> bool {
        // Reserve one fifth of the trace-age budget for display scheduling
        let guard_ms = trace_stale_ms - trace_stale_ms.div_ceil(5);
        !has_frame
            || self.rows_since_spectrum >= ROWS_PER_WATERFALL_LINE
            || self
                .last_update
                .is_none_or(|last| now.duration_since(last).as_millis() >= guard_ms)
    }

    fn mark(&mut self, now: Instant) {
        self.rows_since_spectrum = 0;
        self.last_update = Some(now);
    }
}

/// The frame being published, gathered so the lock block is writes and nothing
/// else.
pub(super) struct Snapshot<'a> {
    pub reading: &'a Reading,
    pub smoothed: &'a [f32],
    pub peak: &'a [f32],
    pub linear: &'a [f32],
    pub center_freq_hz: u64,
    pub sample_rate: f64,
    pub enbw_hz: f64,
}

/// Write one display frame's results back.
///
/// Returns the EMA factor read from the lab control, or `None` if the mutex was
/// poisoned - in which case nothing was written and the caller keeps the factor
/// it had. A poisoned mutex here means the UI has already panicked; the worker
/// keeps computing but stops publishing rather than tearing anything else down.
pub(super) fn publish(
    state: &Arc<Mutex<SdrMetrics>>,
    snap: Snapshot<'_>,
    pacing: &mut Pacing,
) -> Option<f32> {
    let now = Instant::now();
    let (s_meter_dbfs, s_meter_peak_dbfs) = pacing.s_meter(snap.reading.channel_power_dbfs, now);
    let Ok(mut m) = state.lock() else {
        return None;
    };
    let r = snap.reading;

    // Refresh the averaging factor from the lab control (cheap read under the
    // lock we already hold for the result write-back).
    let alpha = m.lab.ema_alpha();

    m.signal.peak_to_nf_db = r.peak_to_nf_db;
    m.signal.channel_power_dbfs = r.channel_power_dbfs;
    m.signal.s_meter_dbfs = s_meter_dbfs;
    m.signal.s_meter_peak_dbfs = s_meter_peak_dbfs;
    m.signal.occupied_bw_hz = r.occupied_bw_hz;
    m.signal.modulation = r.modulation;
    m.signal.acpr_lower_db = r.acpr_lower_db;
    m.signal.acpr_upper_db = r.acpr_upper_db;
    m.signal.adj_carrier_dbfs = r.adj_carrier_dbfs;
    m.signal.acpr_offset_hz = r.acpr_offset_hz;

    update_marker_bandwidths(&mut m, &snap);

    // The noise step measurement eats one reading per frame. It only ever
    // returns a decision here; the radio is moved by the rx poll task, which is
    // the only thread allowed to make a device call.
    if let Some(sweep) = m.lab.noise_sweep.as_mut() {
        sweep.feed(r.noise_floor);
    }

    // Advance the waterfall every display frame.
    if m.waterfall.buffer.push(snap.smoothed) {
        pacing.rows_since_spectrum += 1;
    }

    if pacing.due(m.waterfall.last_fft.is_some(), m.caps.trace_stale_ms, now) {
        pacing.mark(now);
        refresh_spectrum(&mut m, &snap);
    }
    Some(alpha)
}

/// Per-marker occupied bandwidth, within each marker's own channel window.
///
/// **Every marker leaves this pass carrying this frame's answer or nothing.**
/// That is why the measurement is its own function: the loop assigns whatever it
/// returns, unconditionally, so no later edit can add a way out that forgets and
/// leaves the previous frame's figure standing beside a live spectrum. A marker
/// with no channel window is one of those ways out, and it clears too.
fn update_marker_bandwidths(m: &mut SdrMetrics, snap: &Snapshot<'_>) {
    let n = snap.linear.len();
    let bin_hz = if n > 0 {
        snap.sample_rate / n as f64
    } else {
        0.0
    };
    let left_hz = snap.center_freq_hz as f64 - snap.sample_rate / 2.0;

    for mk in m.spectrum.markers.iter_mut() {
        mk.measured_bw_hz = mk.channel_bw_hz.and_then(|ch_bw| {
            marker_bandwidth_hz(snap.linear, bin_hz, left_hz, mk.freq_hz as f64, ch_bw)
        });
    }
}

/// The occupied bandwidth inside one marker's channel window, or `None` when
/// this frame cannot measure it.
///
/// The span between the points where the cumulative power in the window crosses
/// [`MARKER_BW_LOW`] and [`MARKER_BW_HIGH`], which is the occupied-bandwidth
/// definition the whole app uses.
///
/// Pure, and taking numbers rather than a `Snapshot`, so the four ways a frame
/// can fail to answer are visible together: no spectrum at all, a marker outside
/// the band, a channel window that lands off the end of it, and a window with no
/// power in it.
fn marker_bandwidth_hz(
    linear: &[f32],
    bin_hz: f64,
    left_hz: f64,
    marker_hz: f64,
    channel_bw_hz: u64,
) -> Option<u64> {
    let n = linear.len();
    // A bin width has to be a positive, finite number: a NaN sample rate would
    // otherwise walk straight through the comparisons below and index nothing.
    if n == 0 || !bin_hz.is_finite() || bin_hz <= 0.0 {
        return None;
    }
    let right_hz = left_hz + bin_hz * n as f64;
    if marker_hz < left_hz || marker_hz > right_hz {
        return None;
    }
    let half_bw = channel_bw_hz as f64 / 2.0;
    let lo_bin = ((marker_hz - half_bw - left_hz) / bin_hz).floor().max(0.0) as usize;
    // `min` bounds this to the last bin, so the slice below cannot run off the
    // end; the only ordering left to rule out is a window entirely past it.
    let hi_bin = ((marker_hz + half_bw - left_hz) / bin_hz)
        .ceil()
        .min((n - 1) as f64) as usize;
    if lo_bin > hi_bin {
        return None;
    }
    let slice = &linear[lo_bin..=hi_bin];
    let total: f32 = slice.iter().sum();
    if total <= 0.0 {
        return None;
    }
    let (lo_t, hi_t) = (total * MARKER_BW_LOW, total * MARKER_BW_HIGH);
    let mut acc = 0f32;
    let mut lo_b = 0usize;
    let mut hi_b = slice.len() - 1;
    for (i, &lin) in slice.iter().enumerate() {
        acc += lin;
        if acc < lo_t {
            lo_b = i;
        }
        if acc < hi_t {
            hi_b = i;
        }
    }
    Some(((hi_b.saturating_sub(lo_b) + 1) as f64 * bin_hz) as u64)
}

/// Replace the drawn spectrum frame, reusing the previous one's buffers when
/// they can be had.
///
/// **Usually they cannot, and that is not a fault.** `App::draw` clones the
/// whole `SdrMetrics` under the lock and then renders from the clone with the
/// lock released, so the UI thread holds the previous frame's `Arc`s for most of
/// the interval between publishes and `try_unwrap` gives them back rather than
/// yielding the vector. Holding the mutex says nothing about the refcount: the
/// clone outlives the guard that made it. The comment here used to claim the
/// unwrap was guaranteed, which would make the fallback beside it unreachable
/// and invite someone to replace it with an `unwrap` that panics on the UI
/// thread's timing.
///
/// Both branches produce a buffer of exactly `n`, which is what the
/// `copy_from_slice` below requires. The reused one is that long because the
/// worker's FFT size is fixed for the life of the worker, so every frame it has
/// ever published is the same size.
fn refresh_spectrum(m: &mut SdrMetrics, snap: &Snapshot<'_>) {
    let n = snap.smoothed.len();
    let (mut bins_vec, mut peak_vec) = match m.waterfall.last_fft.take() {
        Some(old) => (
            Arc::try_unwrap(old.bins_dbfs).unwrap_or_else(|_| vec![0.0_f32; n]),
            Arc::try_unwrap(old.peak_hold).unwrap_or_else(|_| vec![0.0_f32; n]),
        ),
        None => (vec![0.0_f32; n], vec![0.0_f32; n]),
    };
    bins_vec.copy_from_slice(snap.smoothed);
    peak_vec.copy_from_slice(snap.peak);

    let r = snap.reading;
    m.waterfall.last_fft = Some(FftFrame {
        bins_dbfs: Arc::new(bins_vec),
        peak_hold: Arc::new(peak_vec),
        noise_floor: r.noise_floor,
        center_freq_hz: snap.center_freq_hz,
        axis_start_hz: snap.center_freq_hz as f64 - snap.sample_rate / 2.0,
        sample_rate: snap.sample_rate,
        timestamp: Instant::now(),
        peak_to_nf_db: r.peak_to_nf_db,
        channel_power_dbfs: r.channel_power_dbfs,
        occupied_bw_hz: r.occupied_bw_hz,
        enbw_hz: snap.enbw_hz,
        bin_axis: crate::state::BinAxis::FftBins,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::SpectrumMarker;
    use std::time::Duration;

    #[test]
    fn pacing_reserves_one_fifth_of_each_trace_age_budget() {
        let now = Instant::now();
        let mut pacing = Pacing::new();
        for (budget, guard_ms) in [(500, 400), (100, 80), (5_000, 4_000), (2, 1)] {
            pacing.mark(now);
            assert!(!pacing.due(true, budget, now + Duration::from_millis(guard_ms - 1)));
            assert!(pacing.due(true, budget, now + Duration::from_millis(guard_ms)));
        }
        assert!(pacing.due(true, 1, now));
        assert!(!pacing.due(true, u128::MAX, now + Duration::from_secs(1)));
    }

    #[test]
    fn pacing_still_refreshes_initial_frames_and_visible_waterfall_lines() {
        let now = Instant::now();
        let mut pacing = Pacing::new();
        assert!(pacing.due(true, 500, now));
        pacing.mark(now);
        assert!(pacing.due(false, 500, now));
        assert!(!pacing.due(true, 500, now));
        pacing.rows_since_spectrum = 1;
        assert!(!pacing.due(true, 500, now));
        pacing.rows_since_spectrum = 2;
        assert!(pacing.due(true, 500, now));
        pacing.mark(now);
        assert_eq!(pacing.rows_since_spectrum, 0);
        assert!(!pacing.due(true, 500, now));
    }

    #[test]
    fn s_meter_smooths_bursts_and_holds_the_peak_for_one_second() {
        let start = Instant::now();
        let mut pacing = Pacing::new();
        assert_eq!(pacing.s_meter(-40.0, start), (Some(-40.0), Some(-40.0)));

        let (rising, peak) = pacing.s_meter(-20.0, start + Duration::from_millis(100));
        let rising = rising.unwrap();
        assert!(rising > -40.0 && rising < -20.0);
        assert_eq!(peak, Some(rising));

        let (falling, held_peak) = pacing.s_meter(-80.0, start + Duration::from_millis(500));
        assert!(falling.unwrap() < rising);
        assert_eq!(held_peak, Some(rising));

        let (released, released_peak) = pacing.s_meter(-80.0, start + Duration::from_millis(1_200));
        assert_eq!(released_peak, released);
    }

    fn marker(freq_hz: u64, channel_bw_hz: u64, measured_bw_hz: Option<u64>) -> SpectrumMarker {
        SpectrumMarker {
            freq_hz,
            label: "M1".to_string(),
            channel_bw_hz: Some(channel_bw_hz),
            measured_bw_hz,
        }
    }

    /// A `Reading` to hang a `Snapshot` on. Nothing in the marker pass reads it,
    /// but it is not a type that can be conjured, so it is measured for real off
    /// a flat spectrum.
    fn flat_reading(smoothed: &[f32]) -> Reading {
        let mut linear = vec![0.0f32; smoothed.len()];
        let mut scratch = vec![0.0f32; smoothed.len()];
        super::super::analysis::measure(smoothed, &mut linear, &mut scratch, 1_000_000.0)
    }

    /// Every other way out of the marker pass says "no measurement" by writing
    /// `None`. A channel window with no power in it took the fourth way out and
    /// left the previous frame's figure on screen, beside a live spectrum, with
    /// nothing to say it was old.
    #[test]
    fn a_channel_with_no_power_in_it_clears_the_marker_rather_than_keeping_the_last_one() {
        let smoothed = vec![-160.0f32; 64];
        let reading = flat_reading(&smoothed);
        let linear = vec![0.0f32; 64];
        let snap = Snapshot {
            reading: &reading,
            smoothed: &smoothed,
            peak: &smoothed,
            linear: &linear,
            center_freq_hz: 100_000_000,
            sample_rate: 1_000_000.0,
            enbw_hz: 1.0,
        };
        let mut m = SdrMetrics::fixture();
        m.spectrum.markers = vec![marker(100_000_000, 100_000, Some(37_000))];

        update_marker_bandwidths(&mut m, &snap);

        assert_eq!(m.spectrum.markers[0].measured_bw_hz, None);
    }

    /// A marker the current band does not contain has no measurement, not the
    /// one it had when the radio was tuned somewhere else.
    #[test]
    fn a_marker_outside_the_band_measures_nothing() {
        let linear = vec![1.0f32; 100];
        assert_eq!(
            marker_bandwidth_hz(&linear, 1_000.0, 0.0, -1.0, 20_000),
            None
        );
        assert_eq!(
            marker_bandwidth_hz(&linear, 1_000.0, 0.0, 100_001.0, 20_000),
            None
        );
    }

    /// No spectrum, and a sample rate that gives no bin width, are both "this
    /// frame cannot answer" rather than a width of nothing.
    #[test]
    fn a_frame_with_nothing_in_it_measures_nothing() {
        assert_eq!(
            marker_bandwidth_hz(&[], 1_000.0, 0.0, 50_000.0, 20_000),
            None
        );
        let linear = vec![1.0f32; 100];
        assert_eq!(
            marker_bandwidth_hz(&linear, 0.0, 0.0, 50_000.0, 20_000),
            None
        );
    }

    /// A channel window whose bins are all empty. Unreachable while the trace is
    /// floored at -160 dBFS, which is a positive linear value; pinned because
    /// the branch that handles it is the one that used to leave a stale reading.
    #[test]
    fn a_channel_window_with_no_power_measures_nothing() {
        let linear = vec![0.0f32; 100];
        assert_eq!(
            marker_bandwidth_hz(&linear, 1_000.0, 0.0, 50_000.0, 20_000),
            None
        );
    }

    /// The measurement itself. Five bins of equal power inside a twenty-bin
    /// channel window: the 0.5 % / 99.5 % crossings sit either side of exactly
    /// those five, so the answer is five bin widths.
    #[test]
    fn a_tone_measures_its_own_width_and_not_the_windows() {
        let mut linear = vec![0.0f32; 100];
        for l in linear.iter_mut().take(53).skip(48) {
            *l = 1.0;
        }
        assert_eq!(
            marker_bandwidth_hz(&linear, 1_000.0, 0.0, 50_000.0, 20_000),
            Some(5_000)
        );
    }

    /// The path that actually runs, pinned because the comment above
    /// `refresh_spectrum` used to say it could not.
    ///
    /// `App::draw` clones the state and renders from the clone, so the frame the
    /// UI is drawing is still alive when the next one is published. The new frame
    /// must carry the new trace, and the one the UI is holding must not be
    /// written through underneath it.
    #[test]
    fn a_frame_the_ui_is_still_holding_is_rebuilt_rather_than_written_over() {
        let first = vec![-40.0f32; 64];
        let second = vec![-70.0f32; 64];
        let linear = vec![1.0f32; 64];
        let r1 = flat_reading(&first);
        let mut m = SdrMetrics::fixture();
        refresh_spectrum(
            &mut m,
            &Snapshot {
                reading: &r1,
                smoothed: &first,
                peak: &first,
                linear: &linear,
                center_freq_hz: 100_000_000,
                sample_rate: 1_000_000.0,
                enbw_hz: 1.0,
            },
        );

        // What the UI thread does every frame: take the whole state away and
        // draw from it while the worker carries on publishing.
        let being_drawn = m.clone();

        let r2 = flat_reading(&second);
        refresh_spectrum(
            &mut m,
            &Snapshot {
                reading: &r2,
                smoothed: &second,
                peak: &second,
                linear: &linear,
                center_freq_hz: 100_000_000,
                sample_rate: 1_000_000.0,
                enbw_hz: 1.0,
            },
        );

        let published = m.waterfall.last_fft.expect("a frame was published");
        assert_eq!(published.bins_dbfs[0], -70.0, "the new trace");
        assert_eq!(published.bin_axis, crate::state::BinAxis::FftBins);
        assert_eq!(published.frequency_of_bin(48), Some(100_250_000.0));
        let held = being_drawn.waterfall.last_fft.expect("the UI's copy");
        assert_eq!(held.bins_dbfs[0], -40.0, "still the frame it was drawing");
    }
}
