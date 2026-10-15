// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The visible slice of the FFT frame.
//!
//! Bonded under the waterfall, the two plots narrow to the same centre slice of
//! bins so the instrument zooms as one around the tuned frequency. Standalone,
//! the spectrum shows the whole span. Everything downstream - the trace, the
//! peak flags, the marker columns, the axes - works in *this* window's
//! coordinates, which is what makes it worth naming.
//!
//! Getting this wrong is not hypothetical: detecting peaks against the full
//! frame while drawing the zoomed one mislocated every flag and printed the
//! wrong MHz beside it.

use std::sync::Arc;

use crate::state::{BinAxis, BinWindow, FftFrame};

/// The window the panel is actually drawing: the bins in view and the frequency
/// span they cover.
pub(super) struct SpectrumView {
    /// Live bins, windowed to the visible slice.
    pub bins: Arc<Vec<f32>>,
    /// Decaying peak-hold envelope, same window.
    pub peaks: Arc<Vec<f32>>,
    /// The frozen `[HOLD]` snapshot, same window, when one is held.
    pub held: Option<Arc<Vec<f32>>>,
    /// How many bins are in view. Never zero once a view exists.
    pub n_bins: usize,
    /// Frequency of the left edge.
    pub left_hz: f64,
    /// Width of the window in hertz.
    pub bw: f64,
    bin_axis: BinAxis,
}

impl SpectrumView {
    /// Window `full_*` down to the centre `1/zoom` of its bins. A `zoom` of 1
    /// (or a frame with nothing in it) returns the whole span, sharing the
    /// frame's `Arc`s rather than copying.
    ///
    /// `held` may have been captured at a different bin count than the live
    /// frame, so it is windowed against its own length. Slicing it blind is a
    /// panic waiting for the user to change sample rate while holding.
    #[cfg(test)]
    pub fn new(
        bins: &Arc<Vec<f32>>,
        peaks: &Arc<Vec<f32>>,
        held: Option<Arc<Vec<f32>>>,
        center_hz: u64,
        sample_rate: f64,
        zoom: usize,
        bin_axis: BinAxis,
    ) -> Option<Self> {
        let window = bin_axis.window(center_hz, sample_rate, bins.len(), zoom)?;
        Self::from_window(bins, peaks, held, window, bin_axis)
    }

    pub fn from_frame(frame: &FftFrame, held: Option<Arc<Vec<f32>>>, zoom: usize) -> Option<Self> {
        let window = frame.window(zoom)?;
        Self::from_window(
            &frame.bins_dbfs,
            &frame.peak_hold,
            held,
            window,
            frame.bin_axis,
        )
    }

    fn from_window(
        bins: &Arc<Vec<f32>>,
        peaks: &Arc<Vec<f32>>,
        held: Option<Arc<Vec<f32>>>,
        window: BinWindow,
        bin_axis: BinAxis,
    ) -> Option<Self> {
        let full_n = bins.len();
        let lo = window.first_bin;
        let hi = lo + window.bin_count;

        if lo == 0 && hi == full_n {
            // Arc::clone is O(1) - no data copied.
            return Some(Self {
                bins: Arc::clone(bins),
                peaks: Arc::clone(peaks),
                held,
                n_bins: full_n,
                left_hz: window.left_hz,
                bw: window.span_hz,
                bin_axis,
            });
        }

        let win = |v: &[f32]| Arc::new(v[lo.min(v.len())..hi.min(v.len())].to_vec());

        Some(Self {
            bins: win(bins),
            peaks: win(peaks),
            held: held.map(|h| win(&h)),
            n_bins: window.bin_count,
            left_hz: window.left_hz,
            bw: window.span_hz,
            bin_axis,
        })
    }

    /// Frequency of the right edge.
    pub fn right_hz(&self) -> f64 {
        self.left_hz + self.bw
    }

    /// Right edge of the canvas in bin-interval units.
    pub fn n(&self) -> f64 {
        self.bin_axis.interval_count(self.n_bins).unwrap_or(1) as f64
    }

    pub fn bin_end(&self, index: usize) -> f64 {
        match self.bin_axis {
            BinAxis::FftBins => (index + 1) as f64,
            BinAxis::MeasuredPoints => index as f64,
        }
    }

    /// The level at `freq_hz`, or `None` when it falls outside the window.
    pub fn level_at(&self, freq_hz: u64) -> Option<f32> {
        let idx = self
            .bin_axis
            .nearest_bin(self.left_hz, self.bw, self.n_bins, freq_hz as f64)?;
        self.bins.get(idx).copied()
    }

    /// The centre frequency of bin `idx`.
    pub fn freq_of_bin(&self, idx: usize) -> f64 {
        self.bin_axis
            .frequency_of_bin(self.left_hz, self.bw, self.n_bins, idx)
            .unwrap_or(self.left_hz)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(n: usize) -> Arc<Vec<f32>> {
        Arc::new((0..n).map(|i| i as f32).collect())
    }

    #[test]
    fn zoom_one_shows_the_whole_span_without_copying() {
        let bins = ramp(1024);
        let peaks = ramp(1024);
        let v = SpectrumView::new(
            &bins,
            &peaks,
            None,
            92_800_000,
            2_000_000.0,
            1,
            BinAxis::FftBins,
        )
        .unwrap();
        assert_eq!(v.n_bins, 1024);
        assert_eq!(v.left_hz, 91_800_000.0);
        assert_eq!(v.bw, 2_000_000.0);
        assert!(
            Arc::ptr_eq(&v.bins, &bins),
            "zoom 1 shares the frame's buffer"
        );
    }

    #[test]
    fn zoom_takes_the_centre_slice_around_the_tuned_frequency() {
        let bins = ramp(1000);
        let v = SpectrumView::new(
            &bins,
            &ramp(1000),
            None,
            92_800_000,
            2_000_000.0,
            4,
            BinAxis::FftBins,
        )
        .unwrap();
        assert_eq!(v.n_bins, 250);
        assert_eq!(v.bins[0], 375.0, "starts a quarter of the way in, not at 0");
        assert_eq!(v.bw, 500_000.0, "a quarter of the span");
        // The window still straddles the tuned centre.
        assert!(v.left_hz < 92_800_000.0 && v.right_hz() > 92_800_000.0);
    }

    #[test]
    fn measured_point_zoom_keeps_the_slice_endpoints() {
        let bins = ramp(64);
        let v = SpectrumView::new(
            &bins,
            &ramp(64),
            None,
            131_500_000,
            63_000_000.0,
            4,
            BinAxis::MeasuredPoints,
        )
        .unwrap();

        assert_eq!(v.bins.first(), Some(&24.0));
        assert_eq!(v.bins.last(), Some(&39.0));
        assert_eq!(v.freq_of_bin(0), 124_000_000.0);
        assert_eq!(v.freq_of_bin(15), 139_000_000.0);
        assert_eq!(v.left_hz, 124_000_000.0);
        assert_eq!(v.right_hz(), 139_000_000.0);
    }

    #[test]
    fn measured_point_view_reads_the_last_odd_span_endpoint() {
        let bins = ramp(4);
        let frame = FftFrame {
            bins_dbfs: Arc::clone(&bins),
            peak_hold: Arc::clone(&bins),
            noise_floor: -90.0,
            center_freq_hz: 100_001,
            axis_start_hz: 100_000.0,
            sample_rate: 3.0,
            timestamp: std::time::Instant::now(),
            peak_to_nf_db: 0.0,
            occupied_bw_hz: 0,
            enbw_hz: 1.0,
            bin_axis: BinAxis::MeasuredPoints,
        };
        let view = SpectrumView::from_frame(&frame, None, 1).unwrap();

        assert_eq!(view.freq_of_bin(0), 100_000.0);
        assert_eq!(view.freq_of_bin(3), 100_003.0);
        assert_eq!(view.level_at(100_003), Some(3.0));
        assert_eq!(view.right_hz(), 100_003.0);
    }

    #[test]
    fn bin_frequencies_map_to_their_canvas_positions() {
        for axis in [BinAxis::FftBins, BinAxis::MeasuredPoints] {
            let bins = ramp(32);
            let view =
                SpectrumView::new(&bins, &ramp(32), None, 100_000_000, 32_000_000.0, 1, axis)
                    .unwrap();
            for index in [0, 8, 16, 24, 31] {
                let x = super::super::scale::freq_to_canvas_x(
                    view.freq_of_bin(index),
                    view.left_hz,
                    view.bw,
                    view.n(),
                )
                .unwrap();
                assert!((x - index as f64).abs() < 1e-9, "{axis:?} bin {index}");
            }
        }
    }

    #[test]
    fn a_hold_captured_at_another_bin_count_does_not_panic() {
        // The user changed sample rate while holding: the snapshot is shorter
        // than the live frame, so the window has to clamp to its own length.
        let bins = ramp(1024);
        let held = Some(ramp(200));
        let v = SpectrumView::new(
            &bins,
            &ramp(1024),
            held,
            92_800_000,
            2_000_000.0,
            4,
            BinAxis::FftBins,
        )
        .unwrap();
        assert!(v.held.unwrap().len() <= 200);
    }

    #[test]
    fn an_empty_frame_yields_no_view() {
        assert!(SpectrumView::new(
            &ramp(0),
            &ramp(0),
            None,
            92_800_000,
            2_000_000.0,
            1,
            BinAxis::FftBins,
        )
        .is_none());
        assert!(
            SpectrumView::new(
                &ramp(64),
                &ramp(64),
                None,
                92_800_000,
                0.0,
                1,
                BinAxis::FftBins,
            )
            .is_none(),
            "a zero sample rate has no span to draw"
        );
    }

    #[test]
    fn level_at_reads_the_window_not_the_frame() {
        let bins = ramp(1000);
        let v = SpectrumView::new(
            &bins,
            &ramp(1000),
            None,
            92_800_000,
            2_000_000.0,
            4,
            BinAxis::FftBins,
        )
        .unwrap();
        // Mid-window is bin 125 of the slice, which held the value 500.
        assert_eq!(v.level_at((v.left_hz + v.bw / 2.0) as u64), Some(500.0));
        assert!(v.level_at(90_000_000).is_none(), "outside the window");
    }

    #[test]
    fn fft_bin_24_maps_to_canvas_24() {
        let bins = ramp(32);
        let view = SpectrumView::new(
            &bins,
            &bins,
            None,
            100_000_000,
            32_000_000.0,
            1,
            BinAxis::FftBins,
        )
        .unwrap();
        assert_eq!(view.freq_of_bin(24), 108_000_000.0);
        assert_eq!(view.level_at(108_000_000), Some(24.0));
        assert_eq!(
            super::super::scale::freq_to_canvas_x(108_000_000.0, view.left_hz, view.bw, view.n(),),
            Some(24.0)
        );
    }

    #[test]
    fn sub_bin_lookup_stays_in_its_fft_interval_at_high_zoom() {
        let bins = ramp(256);
        let view = SpectrumView::new(
            &bins,
            &bins,
            None,
            100_000_000,
            32_000_000.0,
            32,
            BinAxis::FftBins,
        )
        .unwrap();
        for i in 0..view.n_bins {
            let hz = view.freq_of_bin(i) as u64;
            for offset in [0, 62_500, 100_000, 124_999] {
                assert_eq!(view.level_at(hz + offset), Some(view.bins[i]));
            }
        }
    }

    #[test]
    fn bin_frequencies_map_to_their_canvas_positions_across_zoom() {
        let bins = ramp(32);
        for zoom in [0, 1, 2, 3, 4, 32, 64] {
            let view = SpectrumView::new(
                &bins,
                &bins,
                None,
                100_000_000,
                32_000_000.0,
                zoom,
                BinAxis::FftBins,
            )
            .unwrap();
            for index in 0..view.n_bins {
                let frequency = 84_000_000.0 + view.bins[index] as f64 * 1_000_000.0;
                assert_eq!(view.freq_of_bin(index), frequency);
                assert_eq!(view.level_at(frequency as u64), Some(view.bins[index]));
                let x = super::super::scale::freq_to_canvas_x(
                    frequency,
                    view.left_hz,
                    view.bw,
                    view.n(),
                )
                .unwrap();
                assert!((x - index as f64).abs() < 1e-9, "zoom {zoom} bin {index}");
            }
            assert_eq!(
                view.level_at(view.right_hz() as u64),
                view.bins.last().copied()
            );
        }
    }
}
