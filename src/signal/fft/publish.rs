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

/// Pace the drawn spectrum to the *visible* waterfall: each waterfall character
/// row packs this many data rows (half-block ▀), so the spectrum refreshes once
/// per visible line rather than every FFT frame, keeping the two panels moving in
/// lockstep. Signal metrics stay at full rate.
const ROWS_PER_WATERFALL_LINE: u32 = 2;

/// Never let the drawn frame age past the panels' 500 ms STALE threshold, even at
/// large frames or row strides.
const SPECTRUM_STALE_GUARD: Duration = Duration::from_millis(400);

/// How much of a marker's channel the measured-bandwidth cut-offs bracket.
const MARKER_BW_LOW: f32 = 0.005;
const MARKER_BW_HIGH: f32 = 0.995;

/// The display-paced spectrum refresh's state, carried between frames.
pub(super) struct Pacing {
    rows_since_spectrum: u32,
    last_update: Instant,
}

impl Pacing {
    pub(super) fn new() -> Self {
        Self {
            rows_since_spectrum: 0,
            last_update: Instant::now()
                .checked_sub(SPECTRUM_STALE_GUARD)
                .unwrap_or_else(Instant::now),
        }
    }

    /// Whether to redraw the spectrum: there is none yet, a visible waterfall line
    /// has gone by, or it would otherwise age toward `[STALE]`.
    fn due(&self, has_frame: bool) -> bool {
        !has_frame
            || self.rows_since_spectrum >= ROWS_PER_WATERFALL_LINE
            || self.last_update.elapsed() >= SPECTRUM_STALE_GUARD
    }

    fn mark(&mut self) {
        self.rows_since_spectrum = 0;
        self.last_update = Instant::now();
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
    let Ok(mut m) = state.lock() else {
        return None;
    };
    let r = snap.reading;

    // Refresh the averaging factor from the lab control (cheap read under the
    // lock we already hold for the result write-back).
    let alpha = m.lab.ema_alpha();

    m.signal.peak_to_nf_db = r.peak_to_nf_db;
    m.signal.channel_power_dbfs = r.channel_power_dbfs;
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

    if pacing.due(m.waterfall.last_fft.is_some()) {
        pacing.mark();
        refresh_spectrum(&mut m, &snap);
    }
    Some(alpha)
}

/// Per-marker occupied bandwidth, within each marker's own channel window.
fn update_marker_bandwidths(m: &mut SdrMetrics, snap: &Snapshot<'_>) {
    let n = snap.linear.len();
    if snap.sample_rate <= 0.0 || n == 0 {
        return;
    }
    let bin_hz = snap.sample_rate / n as f64;
    let left_hz = snap.center_freq_hz as f64 - snap.sample_rate / 2.0;
    let right_hz = left_hz + snap.sample_rate;

    for mk in m.spectrum.markers.iter_mut() {
        let Some(ch_bw) = mk.channel_bw_hz else {
            continue;
        };
        let mf = mk.freq_hz as f64;
        // A marker outside the current band has no measurement, not a stale one.
        if mf < left_hz || mf > right_hz {
            mk.measured_bw_hz = None;
            continue;
        }
        let lo_bin = ((mf - ch_bw as f64 / 2.0 - left_hz) / bin_hz)
            .floor()
            .max(0.0) as usize;
        let hi_bin = ((mf + ch_bw as f64 / 2.0 - left_hz) / bin_hz)
            .ceil()
            .min((n - 1) as f64) as usize;
        if lo_bin > hi_bin || hi_bin >= n {
            mk.measured_bw_hz = None;
            continue;
        }
        let slice = &snap.linear[lo_bin..=hi_bin];
        let total: f32 = slice.iter().sum();
        if total <= 0.0 {
            continue;
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
        mk.measured_bw_hz = Some(((hi_b.saturating_sub(lo_b) + 1) as f64 * bin_hz) as u64);
    }
}

/// Replace the drawn spectrum frame, reusing the previous one's allocations.
///
/// We hold the mutex, so the refcount is 1 and `try_unwrap` is guaranteed to
/// succeed - the frame is rebuilt with no heap traffic at all.
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
        sample_rate: snap.sample_rate,
        timestamp: Instant::now(),
        peak_to_nf_db: r.peak_to_nf_db,
        channel_power_dbfs: r.channel_power_dbfs,
        occupied_bw_hz: r.occupied_bw_hz,
        enbw_hz: snap.enbw_hz,
    });
}
