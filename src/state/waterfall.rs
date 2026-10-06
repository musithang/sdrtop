// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

/// Define how bins cover a frequency span
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BinAxis {
    /// Each FFT bin owns one interval starting at its frequency
    #[default]
    FftBins,
    MeasuredPoints,
}

/// A nonempty centre slice retains the full frame's bin spacing
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BinWindow {
    pub first_bin: usize,
    pub bin_count: usize,
    pub left_hz: f64,
    pub span_hz: f64,
}

impl BinAxis {
    /// Return the number of frequency intervals, or `None` for an empty axis
    pub fn interval_count(self, bin_count: usize) -> Option<usize> {
        match self {
            Self::FftBins => (bin_count > 0).then_some(bin_count),
            Self::MeasuredPoints => bin_count.checked_sub(1).filter(|count| *count > 0),
        }
    }

    /// Select a centre slice with at least one bin. Zero zoom means full span.
    /// Empty axes and non-positive or non-finite spans have no window.
    pub fn window(
        self,
        center_hz: u64,
        span_hz: f64,
        bin_count: usize,
        zoom: usize,
    ) -> Option<BinWindow> {
        self.window_from_start(center_hz as f64 - span_hz / 2.0, span_hz, bin_count, zoom)
    }

    pub fn window_from_start(
        self,
        start_hz: f64,
        span_hz: f64,
        bin_count: usize,
        zoom: usize,
    ) -> Option<BinWindow> {
        if bin_count == 0 || !start_hz.is_finite() || !span_hz.is_finite() || span_hz <= 0.0 {
            return None;
        }

        let minimum = if self == Self::MeasuredPoints && bin_count > 1 {
            2
        } else {
            1
        };
        let visible = (bin_count / zoom.max(1)).max(minimum).min(bin_count);
        let first = (bin_count / 2)
            .saturating_sub(visible / 2)
            .min(bin_count - visible);
        let intervals = self.interval_count(bin_count)?;
        let bin_hz = span_hz / intervals as f64;
        let visible_intervals = match self {
            Self::FftBins => visible,
            Self::MeasuredPoints => visible.saturating_sub(1),
        };

        Some(BinWindow {
            first_bin: first,
            bin_count: visible,
            left_hz: start_hz + first as f64 * bin_hz,
            span_hz: visible_intervals as f64 * bin_hz,
        })
    }

    /// Return a bin's frequency
    ///
    /// The last FFT bin starts below the right edge.
    pub fn frequency_of_bin(
        self,
        left_hz: f64,
        span_hz: f64,
        bin_count: usize,
        index: usize,
    ) -> Option<f64> {
        if bin_count == 0 || index >= bin_count {
            return None;
        }
        let intervals = self.interval_count(bin_count)?;
        if !left_hz.is_finite() || !span_hz.is_finite() || span_hz <= 0.0 {
            return None;
        }
        Some(left_hz + index as f64 * span_hz / intervals as f64)
    }

    /// Look up the FFT interval containing `frequency_hz`
    ///
    /// The right edge reads the last bin. Frequencies outside the window return `None`.
    pub fn nearest_bin(
        self,
        left_hz: f64,
        span_hz: f64,
        bin_count: usize,
        frequency_hz: f64,
    ) -> Option<usize> {
        if !left_hz.is_finite()
            || !span_hz.is_finite()
            || span_hz <= 0.0
            || !frequency_hz.is_finite()
            || !(left_hz..=left_hz + span_hz).contains(&frequency_hz)
        {
            return None;
        }
        let intervals = self.interval_count(bin_count)?;
        let position = (frequency_hz - left_hz) * intervals as f64 / span_hz;
        let index = match self {
            Self::FftBins => position.floor() as usize,
            Self::MeasuredPoints => position.round() as usize,
        };
        Some(index.min(bin_count.saturating_sub(1)))
    }
}

#[derive(Clone)]
pub struct FftFrame {
    pub bins_dbfs: Arc<Vec<f32>>,
    pub peak_hold: Arc<Vec<f32>>,
    pub noise_floor: f32,
    pub center_freq_hz: u64,
    pub axis_start_hz: f64,
    pub sample_rate: f64,
    pub timestamp: Instant,
    pub peak_to_nf_db: f32,
    pub channel_power_dbfs: f32,
    pub occupied_bw_hz: u64,
    pub enbw_hz: f64,
    pub bin_axis: BinAxis,
}

impl FftFrame {
    pub fn window(&self, zoom: usize) -> Option<BinWindow> {
        self.window_for_bins(self.bins_dbfs.len(), zoom)
    }

    pub fn window_for_bins(&self, bin_count: usize, zoom: usize) -> Option<BinWindow> {
        self.bin_axis
            .window_from_start(self.axis_start_hz, self.sample_rate, bin_count, zoom)
    }

    pub fn frequency_of_bin(&self, index: usize) -> Option<f64> {
        let window = self.window(1)?;
        self.bin_axis
            .frequency_of_bin(window.left_hz, window.span_hz, window.bin_count, index)
    }
}

pub struct WaterfallBuffer {
    /// Each row: (push timestamp, averaged bins). Newest row first.
    pub rows: VecDeque<(Instant, Arc<Vec<f32>>)>,
    pub max_rows: usize,
    pub paused: bool,
    pub row_stride: usize,
    acc_bins: Vec<f32>,
    acc_count: usize,
}

impl Clone for WaterfallBuffer {
    fn clone(&self) -> Self {
        Self {
            rows: self.rows.clone(),
            max_rows: self.max_rows,
            paused: self.paused,
            row_stride: self.row_stride,
            // acc_bins is an internal FFT accumulator never read by the UI -
            // skip the 8 KB copy and give the clone an empty buffer.
            acc_bins: Vec::new(),
            acc_count: self.acc_count,
        }
    }
}

/// Fewest rows of history a waterfall buffer may keep.
///
/// Two data rows per character cell, so this fills a 128-row-tall waterfall - far
/// beyond any real terminal - with nothing left over. Below it a full-height
/// waterfall draws every row it has and then leaves the rest of the panel blank,
/// which reads as the plot being cut off short of its own border.
///
/// A floor rather than only a default, because `save_config` writes the *live*
/// buffer depth back to `config.toml`: anyone who has quit the app once has the
/// old 64 baked into their config, and raising the default alone would never
/// reach them. Clamped at startup, the same way an out-of-range frequency is.
pub const WATERFALL_MIN_ROWS: usize = 256;

impl WaterfallBuffer {
    pub fn new(max_rows: usize) -> Self {
        Self {
            rows: VecDeque::new(),
            max_rows,
            paused: false,
            row_stride: 1,
            acc_bins: Vec::new(),
            acc_count: 0,
        }
    }

    /// Accumulate one FFT frame. Returns `true` when this call materialized a
    /// new row (i.e. the stride was reached), `false` while still accumulating
    /// or when paused. Callers use the signal to pace the spectrum display in
    /// lockstep with the waterfall.
    pub fn push(&mut self, bins: &[f32]) -> bool {
        if self.paused || self.max_rows == 0 {
            return false;
        }

        if self.acc_count == 0 || self.acc_bins.len() != bins.len() {
            // Fresh accumulation: the first frame of a stride, or the bin count
            // changed mid-stride. Restart cleanly (count = 1) so the materialised
            // row divides by the number of frames it actually summed - not a stale
            // count carried over from the previous, differently-sized run.
            self.acc_bins.resize(bins.len(), 0.0);
            self.acc_bins.copy_from_slice(bins);
            self.acc_count = 1;
        } else {
            for (a, &b) in self.acc_bins.iter_mut().zip(bins.iter()) {
                *a += b;
            }
            self.acc_count += 1;
        }

        if self.acc_count >= self.row_stride {
            let inv = 1.0 / self.acc_count as f32;
            for a in self.acc_bins.iter_mut() {
                *a *= inv;
            }
            // Clone acc_bins into the row Arc - acc_bins keeps its allocation for the next push.
            let averaged = Arc::new(self.acc_bins.clone());
            if self.rows.len() >= self.max_rows {
                self.rows.pop_back();
            }
            self.rows.push_front((Instant::now(), averaged));
            self.acc_count = 0;
            return true;
        }
        false
    }

    pub fn set_row_stride(&mut self, stride: usize) {
        self.row_stride = stride.max(1);
        self.acc_bins.clear();
        self.acc_count = 0;
    }
}

#[derive(Clone)]
pub struct WaterfallState {
    pub db_min: f32,
    pub db_max: f32,
    pub scroll_offset: usize,
    pub cursor_freq: Option<u64>,
    pub hz_zoom: u32,
    pub buffer: WaterfallBuffer,
    pub last_fft: Option<FftFrame>,
    /// Selected colour gradient (DSN-2026-04 §03); cycled live with `P`.
    pub palette: crate::palette::WaterfallPalette,
}

impl WaterfallState {
    /// Initialize the view with finite ordered bounds from validated device capabilities
    pub fn new(
        max_rows: usize,
        palette: crate::palette::WaterfallPalette,
        db_min: f32,
        db_max: f32,
    ) -> Self {
        Self {
            db_min,
            db_max,
            scroll_offset: 0,
            cursor_freq: None,
            hz_zoom: 1,
            buffer: WaterfallBuffer::new(max_rows),
            last_fft: None,
            palette,
        }
    }
}

#[cfg(test)]
mod min_rows_tests {
    use super::*;

    /// Two data rows per character cell, so the floor has to be twice the tallest
    /// waterfall anyone can produce. 128 character rows is far beyond any real
    /// terminal; below that the plot draws what it has and leaves the rest of the
    /// panel blank.
    #[test]
    fn the_floor_fills_a_taller_waterfall_than_any_terminal() {
        const ROWS_PER_CELL: usize = 2;
        const TALLEST_PLAUSIBLE_PANEL: usize = 128;
        const { assert!(WATERFALL_MIN_ROWS >= ROWS_PER_CELL * TALLEST_PLAUSIBLE_PANEL) };
    }

    /// The buffer keeps what it is told to and no more, so the floor is the only
    /// thing standing between a saved config and a short waterfall.
    #[test]
    fn the_buffer_keeps_exactly_its_depth() {
        let mut b = WaterfallBuffer::new(4);
        for _ in 0..10 {
            b.push(&[-50.0; 8]);
        }
        assert_eq!(b.rows.len(), 4);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measured_points_keep_endpoints_across_zoom() {
        let full = BinAxis::MeasuredPoints
            .window(131_500_000, 63_000_000.0, 64, 1)
            .unwrap();
        assert_eq!(full.left_hz, 100_000_000.0);
        assert_eq!(full.span_hz, 63_000_000.0);
        assert_eq!(
            BinAxis::MeasuredPoints.frequency_of_bin(
                full.left_hz,
                full.span_hz,
                full.bin_count,
                63,
            ),
            Some(163_000_000.0)
        );

        let zoomed = BinAxis::MeasuredPoints
            .window(131_500_000, 63_000_000.0, 64, 4)
            .unwrap();
        assert_eq!(zoomed.first_bin, 24);
        assert_eq!(zoomed.bin_count, 16);
        assert_eq!(zoomed.left_hz, 124_000_000.0);
        assert_eq!(zoomed.span_hz, 15_000_000.0);
    }

    #[test]
    fn measured_points_keep_odd_span_endpoints() {
        let axis = BinAxis::MeasuredPoints;
        let window = axis.window_from_start(100_000.0, 3.0, 4, 1).unwrap();
        assert_eq!(window.left_hz, 100_000.0);
        assert_eq!(window.span_hz, 3.0);
        assert_eq!(
            axis.frequency_of_bin(window.left_hz, window.span_hz, window.bin_count, 3),
            Some(100_003.0)
        );
        assert_eq!(
            axis.nearest_bin(window.left_hz, window.span_hz, window.bin_count, 100_003.0,),
            Some(3)
        );
    }

    #[test]
    fn fft_bins_keep_n_intervals() {
        let axis = BinAxis::FftBins;
        let window = axis.window(100_000_000, 64_000_000.0, 64, 1).unwrap();
        assert_eq!(axis.interval_count(64), Some(64));
        assert_eq!(window.first_bin, 0);
        assert_eq!(window.bin_count, 64);
        assert_eq!(window.left_hz, 68_000_000.0);
        assert_eq!(window.span_hz, 64_000_000.0);
        assert_eq!(
            axis.frequency_of_bin(window.left_hz, window.span_hz, window.bin_count, 63),
            Some(131_000_000.0)
        );
        for (frequency, index) in [
            (68_000_000.0, 0),
            (100_000_000.0, 32),
            (131_000_000.0, 63),
            (132_000_000.0, 63),
        ] {
            assert_eq!(
                axis.nearest_bin(window.left_hz, window.span_hz, window.bin_count, frequency),
                Some(index)
            );
        }
    }

    #[test]
    fn fft_zoom_preserves_bin_spacing_and_clamps_to_one_bin() {
        let axis = BinAxis::FftBins;
        assert_eq!(
            axis.window(100, 10.0, 10, 3),
            Some(BinWindow {
                first_bin: 4,
                bin_count: 3,
                left_hz: 99.0,
                span_hz: 3.0,
            })
        );
        assert_eq!(axis.window(100, 10.0, 10, 0), axis.window(100, 10.0, 10, 1));
        for count in [1, 10] {
            let window = axis.window(100, 10.0, count, usize::MAX).unwrap();
            assert_eq!(window.bin_count, 1);
            assert_eq!(window.span_hz, 10.0 / count as f64);
            assert_eq!(
                axis.nearest_bin(window.left_hz, window.span_hz, 1, window.left_hz),
                Some(0)
            );
        }
    }

    #[test]
    fn bin_axis_rejects_invalid_bounds() {
        for axis in [BinAxis::FftBins, BinAxis::MeasuredPoints] {
            assert_eq!(axis.interval_count(0), None);
            assert!(axis.window(100, 10.0, 0, 1).is_none());
            assert!(axis.frequency_of_bin(0.0, 10.0, 0, 0).is_none());
            assert!(axis.frequency_of_bin(0.0, 10.0, 4, 4).is_none());
            assert!(axis.nearest_bin(0.0, 10.0, 0, 0.0).is_none());
            for span in [0.0, -1.0, f64::NAN, f64::INFINITY] {
                assert!(axis.window(100, span, 32, 1).is_none());
                assert!(axis.frequency_of_bin(0.0, span, 4, 0).is_none());
                assert!(axis.nearest_bin(0.0, span, 4, 0.0).is_none());
            }
            for left in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
                assert!(axis.frequency_of_bin(left, 10.0, 4, 0).is_none());
                assert!(axis.nearest_bin(left, 10.0, 4, 0.0).is_none());
            }
            for frequency in [-1.0, 11.0, f64::NAN, f64::INFINITY] {
                assert!(axis.nearest_bin(0.0, 10.0, 4, frequency).is_none());
            }
        }
        assert!(BinAxis::MeasuredPoints.window(100, 10.0, 1, 1).is_none());
    }

    #[test]
    fn push_adds_newest_row_first() {
        let mut buf = WaterfallBuffer::new(4);
        buf.push(&[1.0, 2.0]);
        buf.push(&[3.0, 4.0]);
        assert_eq!(
            *buf.rows[0].1,
            vec![3.0, 4.0],
            "newest row should be at index 0"
        );
        assert_eq!(*buf.rows[1].1, vec![1.0, 2.0]);
    }

    #[test]
    fn push_respects_max_rows() {
        let mut buf = WaterfallBuffer::new(3);
        for i in 0..5u32 {
            buf.push(&[i as f32]);
        }
        assert_eq!(buf.rows.len(), 3, "should not exceed max_rows");
    }

    #[test]
    fn paused_ignores_push() {
        let mut buf = WaterfallBuffer::new(4);
        buf.paused = true;
        buf.push(&[1.0, 2.0]);
        assert!(
            buf.rows.is_empty(),
            "paused buffer should not accept new rows"
        );
    }

    #[test]
    fn stride_averages_frames() {
        let mut buf = WaterfallBuffer::new(4);
        buf.set_row_stride(2);
        buf.push(&[10.0, 20.0]);
        assert!(buf.rows.is_empty(), "first frame should not push yet");
        buf.push(&[20.0, 40.0]);
        assert_eq!(buf.rows.len(), 1, "second frame should push averaged row");
        assert_eq!(*buf.rows[0].1, vec![15.0, 30.0]);
    }

    #[test]
    fn stride_restarts_average_on_bin_count_change() {
        let mut buf = WaterfallBuffer::new(4);
        buf.set_row_stride(3);
        buf.push(&[10.0, 10.0]); // 2-bin frame → acc_count 1
        buf.push(&[20.0, 20.0]); //             → acc_count 2
                                 // Bin count changes mid-stride: accumulation must restart, not carry the
                                 // stale count of 2 (which would materialise a single frame divided by 3).
        assert!(
            !buf.push(&[4.0, 4.0, 4.0]),
            "size change restarts → still accumulating"
        );
        assert!(!buf.push(&[6.0, 6.0, 6.0]));
        assert!(
            buf.push(&[8.0, 8.0, 8.0]),
            "third post-change frame materialises"
        );
        // Row = average of the three 3-bin frames only: (4+6+8)/3 = 6.
        assert_eq!(
            *buf.rows[0].1,
            vec![6.0, 6.0, 6.0],
            "average restarts cleanly after a bin-count change"
        );
    }

    #[test]
    fn stride_reset_clears_accumulator() {
        let mut buf = WaterfallBuffer::new(4);
        buf.set_row_stride(3);
        buf.push(&[10.0]);
        buf.set_row_stride(1);
        buf.push(&[5.0]);
        assert_eq!(buf.rows.len(), 1);
        assert_eq!(*buf.rows[0].1, vec![5.0]);
    }

    #[test]
    fn push_returns_true_only_when_row_materializes() {
        let mut buf = WaterfallBuffer::new(8);
        // Stride 1: every push materializes a row.
        assert!(buf.push(&[1.0]));
        assert!(buf.push(&[2.0]));
    }

    #[test]
    fn push_returns_false_while_accumulating() {
        let mut buf = WaterfallBuffer::new(8);
        buf.set_row_stride(2);
        assert!(
            !buf.push(&[1.0]),
            "first of a stride-2 pair accumulates only"
        );
        assert!(
            buf.push(&[3.0]),
            "second of the pair materializes the averaged row"
        );
        assert!(!buf.push(&[5.0]));
        assert!(buf.push(&[7.0]));
    }

    #[test]
    fn paused_push_returns_false() {
        let mut buf = WaterfallBuffer::new(8);
        buf.paused = true;
        assert!(!buf.push(&[1.0]));
    }
}
