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
}

impl SpectrumView {
    /// Window `full_*` down to the centre `1/zoom` of its bins. A `zoom` of 1
    /// (or a frame with nothing in it) returns the whole span, sharing the
    /// frame's `Arc`s rather than copying.
    ///
    /// `held` may have been captured at a different bin count than the live
    /// frame, so it is windowed against its own length. Slicing it blind is a
    /// panic waiting for the user to change sample rate while holding.
    pub fn new(
        bins: &Arc<Vec<f32>>,
        peaks: &Arc<Vec<f32>>,
        held: Option<Arc<Vec<f32>>>,
        center_hz: u64,
        sample_rate: f64,
        zoom: usize,
    ) -> Option<Self> {
        let full_n = bins.len();
        if full_n == 0 || sample_rate <= 0.0 {
            return None;
        }
        let full_left = center_hz as f64 - sample_rate / 2.0;
        let zoom = zoom.max(1);

        if zoom == 1 {
            // Arc::clone is O(1) - no data copied.
            return Some(Self {
                bins: Arc::clone(bins),
                peaks: Arc::clone(peaks),
                held,
                n_bins: full_n,
                left_hz: full_left,
                bw: sample_rate,
            });
        }

        let visible_n = (full_n / zoom).max(1);
        let lo = (full_n / 2)
            .saturating_sub(visible_n / 2)
            .min(full_n - visible_n);
        let hi = lo + visible_n;
        let bin_hz = sample_rate / full_n as f64;
        let win = |v: &[f32]| Arc::new(v[lo.min(v.len())..hi.min(v.len())].to_vec());

        Some(Self {
            bins: win(bins),
            peaks: win(peaks),
            held: held.map(|h| win(&h)),
            n_bins: visible_n,
            left_hz: full_left + lo as f64 * bin_hz,
            bw: visible_n as f64 * bin_hz,
        })
    }

    /// Frequency of the right edge.
    pub fn right_hz(&self) -> f64 {
        self.left_hz + self.bw
    }

    /// Canvas width in the units the paint closure works in: `0..n-1`.
    pub fn n(&self) -> f64 {
        self.n_bins as f64
    }

    /// The level at `freq_hz`, or `None` when it falls outside the window.
    pub fn level_at(&self, freq_hz: u64) -> Option<f32> {
        let frac = (freq_hz as f64 - self.left_hz) / self.bw;
        if !(0.0..=1.0).contains(&frac) {
            return None;
        }
        let idx = (frac * (self.n_bins - 1) as f64).round() as usize;
        self.bins.get(idx.min(self.n_bins - 1)).copied()
    }

    /// The centre frequency of bin `idx`.
    pub fn freq_of_bin(&self, idx: usize) -> f64 {
        self.left_hz + self.bw * (idx as f64 / (self.n_bins - 1).max(1) as f64)
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
        let v = SpectrumView::new(&bins, &peaks, None, 92_800_000, 2_000_000.0, 1).unwrap();
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
        let v = SpectrumView::new(&bins, &ramp(1000), None, 92_800_000, 2_000_000.0, 4).unwrap();
        assert_eq!(v.n_bins, 250);
        assert_eq!(v.bins[0], 375.0, "starts a quarter of the way in, not at 0");
        assert_eq!(v.bw, 500_000.0, "a quarter of the span");
        // The window still straddles the tuned centre.
        assert!(v.left_hz < 92_800_000.0 && v.right_hz() > 92_800_000.0);
    }

    #[test]
    fn a_hold_captured_at_another_bin_count_does_not_panic() {
        // The user changed sample rate while holding: the snapshot is shorter
        // than the live frame, so the window has to clamp to its own length.
        let bins = ramp(1024);
        let held = Some(ramp(200));
        let v = SpectrumView::new(&bins, &ramp(1024), held, 92_800_000, 2_000_000.0, 4).unwrap();
        assert!(v.held.unwrap().len() <= 200);
    }

    #[test]
    fn an_empty_frame_yields_no_view() {
        assert!(SpectrumView::new(&ramp(0), &ramp(0), None, 92_800_000, 2_000_000.0, 1).is_none());
        assert!(
            SpectrumView::new(&ramp(64), &ramp(64), None, 92_800_000, 0.0, 1).is_none(),
            "a zero sample rate has no span to draw"
        );
    }

    #[test]
    fn level_at_reads_the_window_not_the_frame() {
        let bins = ramp(1000);
        let v = SpectrumView::new(&bins, &ramp(1000), None, 92_800_000, 2_000_000.0, 4).unwrap();
        // Mid-window is bin 125 of the slice, which held the value 500.
        assert_eq!(v.level_at((v.left_hz + v.bw / 2.0) as u64), Some(500.0));
        assert!(v.level_at(90_000_000).is_none(), "outside the window");
    }
}
