//! `TimingState` - host-side timing accuracy of the RX stream, derived by the
//! `rx` polling task and rendered by the `timing_panel` (`lab_timing`, `[8]`).
//!
//! Everything here is measured against the host monotonic clock: the per-callback
//! period and its jitter come from the timestamps the RX callback records, the
//! sample-rate offset from the observed throughput, and the throughput mean/std
//! from an online Welford accumulator the task keeps across polls. None of it
//! needs hardware support beyond the bytes already flowing.

/// Complex samples delivered per HackRF USB transfer: the libhackrf transfer
/// buffer is 262 144 bytes of interleaved 8-bit I/Q, i.e. 131 072 IQ pairs. The
/// expected callback period is this many samples divided by the sample rate.
pub const HACKRF_SAMPLES_PER_TRANSFER: u64 = 131_072;

/// Fraction of the expected callback period allowed before a callback counts as
/// "late". ~0.046 lands the deadline band at ~600 µs around the 13.107 ms period
/// of a 10 Msps HackRF stream, and scales honestly at every other sample rate.
pub const DEADLINE_BUDGET_FRAC: f64 = 0.046;

/// Floor for the deadline budget (µs) so very high sample rates keep a usable
/// band rather than collapsing it to nothing.
pub const DEADLINE_BUDGET_FLOOR_US: u64 = 150;

/// Number of most-recent callbacks the strip chart shows and the late count is
/// measured over (~2.1 s at the HackRF/RTL cadence).
pub const STRIP_WINDOW: usize = 160;

/// One-glance verdict on stream timing. Ordered best → worst.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TimingQuality {
    #[default]
    Excellent,
    Good,
    Marginal,
    Poor,
}

impl TimingQuality {
    /// Decision tree over the p99 jitter (as a fraction of the expected callback
    /// period), the sample-rate offset, and whether samples are being dropped.
    /// A longer expected period tolerates proportionally more absolute jitter, so
    /// the jitter test is relative rather than a fixed microsecond threshold.
    pub fn classify(
        jitter_p99_us: u64,
        cb_period_expected: u64,
        sr_delta_ppm: i64,
        drops_per_sec: u64,
    ) -> Self {
        // No timing data yet (not streaming / first poll) reads as the best case
        // rather than alarming the user with a red verdict on an idle radio.
        if cb_period_expected == 0 {
            return TimingQuality::Excellent;
        }
        let ratio = jitter_p99_us as f64 / cb_period_expected as f64;
        let ppm = sr_delta_ppm.unsigned_abs();
        if drops_per_sec > 0 || ratio > 0.50 || ppm > 500 {
            TimingQuality::Poor
        } else if ratio > 0.25 || ppm > 200 {
            TimingQuality::Marginal
        } else if ratio > 0.10 || ppm > 50 {
            TimingQuality::Good
        } else {
            TimingQuality::Excellent
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            TimingQuality::Excellent => "EXCELLENT",
            TimingQuality::Good => "GOOD",
            TimingQuality::Marginal => "MARGINAL",
            TimingQuality::Poor => "POOR",
        }
    }

    /// Severity 0 (best) → 3 (worst); the panel maps this to a theme color.
    pub fn severity(self) -> u8 {
        match self {
            TimingQuality::Excellent => 0,
            TimingQuality::Good => 1,
            TimingQuality::Marginal => 2,
            TimingQuality::Poor => 3,
        }
    }
}

#[derive(Clone, Default)]
pub struct TimingState {
    /// Measured mean time between RX callbacks (µs).
    pub cb_period_us: u64,
    /// Expected callback period at the configured sample rate (µs).
    pub cb_period_expected: u64,
    /// Signed offset of the measured period from expected, in ppm.
    pub cb_period_delta_ppm: i64,
    /// Std-dev of the callback period over the last poll window (µs).
    pub cb_jitter_us: u64,
    /// Largest callback jitter seen in the most recent poll window.
    pub jitter_max_us: u64,
    /// Largest callback jitter seen since RX start (or the last `[R]` reset in the
    /// timing panel's focus mode). Carried forward by the rx task across windows,
    /// so a one-off spike that scrolls out of the window stays visible.
    pub jitter_session_max_us: u64,
    /// Sample-rate offset (actual vs configured), in ppm.
    pub sr_delta_ppm: i64,
    pub throughput_mean_mbps: f64,
    pub throughput_std_mbps: f64,
    pub timing_quality: TimingQuality,

    // ── Per-callback deadline view (drives the lab_timing strip chart) ──────────
    /// Signed per-callback deviation from the expected period (µs), newest last.
    /// The full snapshot of the hot-path gap ring (not just the deadline window),
    /// so the strip chart can fill a wide panel; it plots this directly.
    pub cb_deviations_us: Vec<i32>,
    /// Late-callback deadline budget for this sample rate (µs), proportional to
    /// the expected period (see [`DEADLINE_BUDGET_FRAC`]).
    pub deadline_budget_us: u64,
    /// Callbacks in the shown window whose |deviation| exceeded the budget.
    pub late_callbacks: u32,
    /// Window size the late count was measured over (denominator for "n / N").
    pub late_window: u32,
    /// Percentiles / peak of the *absolute per-callback deviation* over the shown
    /// window (µs) - the "how late do callbacks actually get" figures, distinct
    /// from the per-window jitter-rms percentiles above.
    pub dev_p95_us: u64,
    pub dev_p99_us: u64,
    pub dev_peak_us: u64,
}

impl TimingState {
    /// Build the snapshot from the latest poll-window measurements. The throughput
    /// mean/std come from the task's running Welford accumulator; everything else
    /// is derived here so the math stays in one testable place.
    #[allow(clippy::too_many_arguments)]
    pub fn compute(
        cb_period_us: u64,
        config_sample_rate: f64,
        samples_per_transfer: u64,
        jitter: &[u64],
        cb_gaps: &[u64],
        cb_jitter_us: u64,
        actual_sample_rate: u32,
        drops_per_sec: u64,
        throughput_mean_mbps: f64,
        throughput_std_mbps: f64,
    ) -> Self {
        let cb_period_expected = if config_sample_rate > 0.0 {
            (samples_per_transfer as f64 / config_sample_rate * 1e6).round() as u64
        } else {
            0
        };
        let cb_period_delta_ppm = if cb_period_expected > 0 && cb_period_us > 0 {
            ((cb_period_us as f64 - cb_period_expected as f64) / cb_period_expected as f64 * 1e6)
                .round() as i64
        } else {
            0
        };
        // p99 feeds the verdict but is not displayed anywhere, so it stays a local.
        // It was a stored field until the `timing_panel` that printed it was retired.
        let jitter_p99_us = percentile_u64(jitter, 99.0);
        let jitter_max_us = jitter.iter().copied().max().unwrap_or(0);
        let sr_delta_ppm = if config_sample_rate > 0.0 && actual_sample_rate > 0 {
            ((actual_sample_rate as f64 - config_sample_rate) / config_sample_rate * 1e6).round()
                as i64
        } else {
            0
        };
        let timing_quality = TimingQuality::classify(
            jitter_p99_us,
            cb_period_expected,
            sr_delta_ppm,
            drops_per_sec,
        );

        // ── Per-callback deadline view ──────────────────────────────────────────
        // Budget scales with the expected period (floored), so "late" means the
        // same proportional slip at any sample rate.
        let deadline_budget_us = if cb_period_expected > 0 {
            ((DEADLINE_BUDGET_FRAC * cb_period_expected as f64).round() as u64)
                .max(DEADLINE_BUDGET_FLOOR_US)
        } else {
            DEADLINE_BUDGET_FLOOR_US
        };
        // The strip chart plots the whole snapshot ring so a wide panel fills with
        // real samples (newest at the right edge) rather than blanking out - the
        // chart shows as many of these as its width allows.
        let cb_deviations_us: Vec<i32> = cb_gaps
            .iter()
            .map(|&g| {
                (g as i64 - cb_period_expected as i64).clamp(i32::MIN as i64, i32::MAX as i64)
                    as i32
            })
            .collect();
        // The deadline figures (late count, |deviation| percentiles) are evaluated
        // over a fixed recent window so "n / N over budget" stays a stable 2.1 s
        // measure regardless of how wide the chart happens to be drawn.
        let win_start = cb_deviations_us.len().saturating_sub(STRIP_WINDOW);
        let dl_window = &cb_deviations_us[win_start..];
        let late_window = dl_window.len() as u32;
        let abs_dev: Vec<u64> = dl_window.iter().map(|&d| d.unsigned_abs() as u64).collect();
        let late_callbacks = abs_dev.iter().filter(|&&d| d > deadline_budget_us).count() as u32;
        let dev_p95_us = percentile_u64(&abs_dev, 95.0);
        let dev_p99_us = percentile_u64(&abs_dev, 99.0);
        let dev_peak_us = abs_dev.iter().copied().max().unwrap_or(0);

        Self {
            cb_period_us,
            cb_period_expected,
            cb_period_delta_ppm,
            cb_jitter_us,
            jitter_max_us,
            // Session peak is carried forward by the caller (rx task), not derived
            // from this window - start at 0 here.
            jitter_session_max_us: 0,
            sr_delta_ppm,
            throughput_mean_mbps,
            throughput_std_mbps,
            timing_quality,
            cb_deviations_us,
            deadline_budget_us,
            late_callbacks,
            late_window,
            dev_p95_us,
            dev_p99_us,
            dev_peak_us,
        }
    }
}

/// Nearest-rank percentile of a small unsorted sample set. Copies and sorts -
/// the window is bounded (≤ `THROUGHPUT_HISTORY_LEN`), so this stays cheap.
/// `p` is a percentage in `0.0..=100.0`. Empty input yields 0.
pub fn percentile_u64(samples: &[u64], p: f64) -> u64 {
    if samples.is_empty() {
        return 0;
    }
    let mut v: Vec<u64> = samples.to_vec();
    v.sort_unstable();
    let rank = ((p / 100.0) * (v.len() - 1) as f64).round() as usize;
    v[rank.min(v.len() - 1)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentile_edges_and_ranks() {
        assert_eq!(percentile_u64(&[], 95.0), 0);
        assert_eq!(percentile_u64(&[42], 99.0), 42);
        let data = [10, 20, 30, 40, 50, 60, 70, 80, 90, 100];
        // Nearest-rank: p95 over 10 samples → rank round(0.95*9)=9 → 100.
        assert_eq!(percentile_u64(&data, 95.0), 100);
        assert_eq!(percentile_u64(&data, 50.0), 60); // round(0.5*9)=5 → data[5]
                                                     // Unsorted input is handled.
        assert_eq!(percentile_u64(&[100, 10, 50], 0.0), 10);
        assert_eq!(percentile_u64(&[100, 10, 50], 100.0), 100);
    }

    #[test]
    fn expected_period_scales_with_transfer_size() {
        // RTL-SDR: 8192 pairs / 2.4 Msps ≈ 3413 µs (vs HackRF's 131072-pair transfer).
        let t = TimingState::compute(
            3_400,
            2_400_000.0,
            8_192,
            &[10],
            &[],
            10,
            2_400_000,
            0,
            4.5,
            0.1,
        );
        assert_eq!(t.cb_period_expected, 3_413);
    }

    #[test]
    fn expected_period_from_sample_rate() {
        // 10 Msps → 131072 / 10e6 = 13107.2 µs ≈ 13107.
        let t = TimingState::compute(
            13_100,
            10_000_000.0,
            HACKRF_SAMPLES_PER_TRANSFER,
            &[40, 50, 60],
            &[],
            50,
            10_000_000,
            0,
            19.5,
            0.2,
        );
        assert_eq!(t.cb_period_expected, 13_107);
        // Measured slightly under expected → negative ppm.
        assert!(t.cb_period_delta_ppm < 0, "got {}", t.cb_period_delta_ppm);
    }

    #[test]
    fn sr_delta_ppm_sign_and_magnitude() {
        // actual 9.998 MHz vs configured 10.000 MHz → -200 ppm.
        let t = TimingState::compute(
            13_107,
            10_000_000.0,
            HACKRF_SAMPLES_PER_TRANSFER,
            &[10],
            &[],
            10,
            9_998_000,
            0,
            19.5,
            0.2,
        );
        assert_eq!(t.sr_delta_ppm, -200);
    }

    #[test]
    fn no_data_is_excellent_not_alarming() {
        let t = TimingState::compute(
            0,
            0.0,
            HACKRF_SAMPLES_PER_TRANSFER,
            &[],
            &[],
            0,
            0,
            0,
            0.0,
            0.0,
        );
        assert_eq!(t.cb_period_expected, 0);
        assert_eq!(t.timing_quality, TimingQuality::Excellent);
    }

    #[test]
    fn quality_decision_tree() {
        let exp = 13_107u64;
        // Clean stream.
        assert_eq!(
            TimingQuality::classify(500, exp, 10, 0),
            TimingQuality::Excellent
        );
        // Mild jitter (~11% of period) → Good.
        assert_eq!(
            TimingQuality::classify(1_500, exp, 0, 0),
            TimingQuality::Good
        );
        // Sample-rate offset alone pushes to Good / Marginal.
        assert_eq!(TimingQuality::classify(0, exp, 120, 0), TimingQuality::Good);
        assert_eq!(
            TimingQuality::classify(0, exp, 300, 0),
            TimingQuality::Marginal
        );
        // Any drops → Poor regardless of jitter.
        assert_eq!(TimingQuality::classify(0, exp, 0, 5), TimingQuality::Poor);
        // Severe jitter (>50%) → Poor.
        assert_eq!(
            TimingQuality::classify(7_000, exp, 0, 0),
            TimingQuality::Poor
        );
    }

    #[test]
    fn jitter_p99_still_reaches_the_verdict() {
        // p99 stopped being a stored field when the panel that printed it was
        // retired, but it is still what `classify` grades. This pins that wiring:
        // the only difference between the two windows is the jitter tail, and the
        // verdict has to move with it. On a 13.1 ms period the Good boundary is
        // 10 % of the period, so a 200 µs tail is Excellent and a 5 ms one is not.
        let calm = [10u64, 20, 30, 40, 200];
        let rough = [10u64, 20, 30, 40, 5_000];
        let t_calm = TimingState::compute(
            13_107,
            10_000_000.0,
            HACKRF_SAMPLES_PER_TRANSFER,
            &calm,
            &[],
            35,
            10_000_000,
            0,
            19.5,
            0.2,
        );
        let t_rough = TimingState::compute(
            13_107,
            10_000_000.0,
            HACKRF_SAMPLES_PER_TRANSFER,
            &rough,
            &[],
            35,
            10_000_000,
            0,
            19.5,
            0.2,
        );
        assert_eq!(t_calm.jitter_max_us, 200);
        assert_eq!(t_rough.jitter_max_us, 5_000);
        assert_eq!(t_calm.timing_quality, TimingQuality::Excellent);
        assert_eq!(t_rough.timing_quality, TimingQuality::Marginal);
    }

    #[test]
    fn deadline_view_budget_late_count_and_percentiles() {
        let exp = 13_107u64;
        // Budget ≈ 0.046 * 13107 ≈ 603 µs.
        // Gaps: four on time (small deviation), one early, two very late.
        let gaps = [
            exp + 50,   // dev +50   (in budget)
            exp - 40,   // dev -40   (in budget)
            exp + 700,  // dev +700  (late)
            exp + 120,  // dev +120  (in budget)
            exp - 6300, // dev -6300 (late, early spike)
        ];
        let t = TimingState::compute(
            exp,
            10_000_000.0,
            HACKRF_SAMPLES_PER_TRANSFER,
            &[],
            &gaps,
            50,
            10_000_000,
            0,
            19.5,
            0.2,
        );
        assert_eq!(t.deadline_budget_us, 603, "budget = round(0.046 * 13107)");
        // Signed deviations preserved, newest last, early spike negative.
        assert_eq!(t.cb_deviations_us, vec![50, -40, 700, 120, -6300]);
        assert_eq!(t.late_window, 5);
        assert_eq!(
            t.late_callbacks, 2,
            "only the +700 and -6300 exceed the budget"
        );
        assert_eq!(t.dev_peak_us, 6300, "peak is over the absolute deviation");
    }

    #[test]
    fn deadline_view_decouples_chart_from_late_window() {
        // More gaps than the deadline window: the chart series keeps them all, but
        // the late count / percentiles only consider the last STRIP_WINDOW of them.
        let exp = 13_107u64;
        let mut gaps: Vec<u64> = vec![exp + 5_000; 40]; // 40 old, very late
        gaps.extend(std::iter::repeat_n(exp, STRIP_WINDOW)); // STRIP_WINDOW on time
        let t = TimingState::compute(
            exp,
            10_000_000.0,
            HACKRF_SAMPLES_PER_TRANSFER,
            &[],
            &gaps,
            50,
            10_000_000,
            0,
            19.5,
            0.2,
        );
        // Whole ring is available to plot.
        assert_eq!(t.cb_deviations_us.len(), 40 + STRIP_WINDOW);
        // But the deadline figures see only the last STRIP_WINDOW (all on time).
        assert_eq!(t.late_window, STRIP_WINDOW as u32);
        assert_eq!(
            t.late_callbacks, 0,
            "the 40 late gaps scrolled out of the window"
        );
        assert_eq!(t.dev_peak_us, 0);
    }

    #[test]
    fn deadline_view_floor_when_no_period() {
        // No configured rate → budget falls back to the floor, never zero.
        let t = TimingState::compute(
            0,
            0.0,
            HACKRF_SAMPLES_PER_TRANSFER,
            &[],
            &[100, 200],
            0,
            0,
            0,
            0.0,
            0.0,
        );
        assert_eq!(t.deadline_budget_us, DEADLINE_BUDGET_FLOOR_US);
    }
}
