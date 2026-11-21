// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The latest spectrum frame: its bins, the axis they lie on, the
//! resolution bandwidth they were measured with, and when.
//!
//! Its own reading rather than part of the snapshot: thousands of numbers a
//! frame would make every snapshot heavy for a client that wanted the
//! figures, and a stream of them would be a video feed, not an event.

use serde::Serialize;

use super::{Clock, Group, Reading};
use crate::engine::testimony::{Staleness, Testimony};
use crate::state::{BinAxis, SdrMetrics};

/// One spectrum frame.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Frame {
    pub center_hz: u64,
    /// The frequency of the axis's start, Hz.
    pub start_hz: f64,
    pub sample_rate_hz: f64,
    /// The noise-equivalent bandwidth of one bin: the resolution bandwidth
    /// every bin's power was measured in.
    pub enbw_hz: f64,
    /// `fft_bins` (bin `i` covers `[start + i*w, start + (i+1)*w)`) or
    /// `measured_points` (point `i` is the power at `start + i*w`).
    pub axis: &'static str,
    pub noise_floor_dbfs: f32,
    /// Power per bin, dBFS, from the axis's start.
    pub bins_dbfs: Vec<f32>,
}

/// The latest frame. Absent before the first, and once it is older than
/// the radio's trace-age limit: a frozen spectrum never reads as live.
pub fn spectrum(m: &SdrMetrics, clock: Clock) -> Group<Reading<Frame>> {
    let stale = Staleness::FftAge.resolve(m);
    let last = m.waterfall.last_fft.as_ref();
    let frame = Reading::of(last.filter(|_| !stale).map(frame_of), || {
        if last.is_none() {
            "no spectrum frame yet".to_string()
        } else {
            "the spectrum is stale".to_string()
        }
    });
    Group {
        testimony: Testimony::resolve(
            m,
            Staleness::FftAge,
            None,
            last.map(|f| clock.unix_ms_of(f.timestamp)),
        ),
        readings: frame,
    }
}

fn frame_of(f: &crate::state::FftFrame) -> Frame {
    Frame {
        center_hz: f.center_freq_hz,
        start_hz: f.axis_start_hz,
        sample_rate_hz: f.sample_rate,
        enbw_hz: f.enbw_hz,
        axis: match f.bin_axis {
            BinAxis::FftBins => "fft_bins",
            BinAxis::MeasuredPoints => "measured_points",
        },
        noise_floor_dbfs: f.noise_floor,
        bins_dbfs: f.bins_dbfs.as_ref().clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clock() -> Clock {
        Clock {
            now: std::time::Instant::now(),
            unix_ms: 1_788_632_561_000,
        }
    }

    #[test]
    fn before_the_first_frame_there_is_none() {
        let g = spectrum(&SdrMetrics::fixture(), clock());
        assert_eq!(g.readings.absent(), Some("no spectrum frame yet"));
        assert!(g.testimony.stale);
    }

    #[test]
    fn a_live_frame_is_served_whole_and_dated() {
        let m = SdrMetrics::fixture().streaming().with_carrier(0.0, 40.0);
        let g = spectrum(&m, clock());
        let frame = g.readings.value().expect("a live frame");
        assert_eq!(
            frame.bins_dbfs.len(),
            m.waterfall.last_fft.as_ref().unwrap().bins_dbfs.len()
        );
        assert!(frame.enbw_hz > 0.0);
        assert!(g.testimony.at_unix_ms.is_some());
        // On the wire, the frame sits under `value`, beside the testimony.
        let json = serde_json::to_value(&g).unwrap();
        assert!(json["value"]["bins_dbfs"].is_array(), "{json}");
        assert_eq!(json["testimony"]["stale"], false);
    }

    #[test]
    fn a_stale_frame_is_not_served() {
        let m = SdrMetrics::fixture()
            .streaming()
            .with_carrier(0.0, 40.0)
            .with_stale_fft();
        assert_eq!(
            spectrum(&m, clock()).readings.absent(),
            Some("the spectrum is stale")
        );
    }
}
