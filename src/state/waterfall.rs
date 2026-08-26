use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

#[derive(Clone)]
#[allow(dead_code)]
pub struct FftFrame {
    pub bins_dbfs: Arc<Vec<f32>>,
    pub peak_hold: Arc<Vec<f32>>,
    pub noise_floor: f32,
    pub center_freq_hz: u64,
    pub sample_rate: f64,
    pub timestamp: Instant,
    pub peak_to_nf_db: f32,
    pub channel_power_dbfs: f32,
    pub occupied_bw_hz: u64,
    pub enbw_hz: f64,
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
            // acc_bins is an internal FFT accumulator never read by the UI —
            // skip the 8 KB copy and give the clone an empty buffer.
            acc_bins: Vec::new(),
            acc_count: self.acc_count,
        }
    }
}

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
            // row divides by the number of frames it actually summed — not a stale
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
            // Clone acc_bins into the row Arc — acc_bins keeps its allocation for the next push.
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
    pub scroll_offset: usize,
    pub cursor_freq: Option<u64>,
    pub hz_zoom: u32,
    pub buffer: WaterfallBuffer,
    pub last_fft: Option<FftFrame>,
    /// Selected colour gradient (DSN-2026-04 §03); cycled live with `P`.
    pub palette: crate::palette::WaterfallPalette,
}

impl WaterfallState {
    pub fn new(max_rows: usize, palette: crate::palette::WaterfallPalette) -> Self {
        Self {
            db_min: -120.0,
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
mod tests {
    use super::*;

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
