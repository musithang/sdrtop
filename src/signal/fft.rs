use std::sync::{Arc, Mutex};

use crossbeam_channel::Receiver;
use num_complex::Complex;
use rustfft::FftPlanner;

use super::dsp::{self, WindowFn};
use crate::hardware::SampleFormat;
use crate::state::{FftFrame, SdrMetrics, ACPR_OFFSET_HZ};

const DB_FLOOR: f32 = -160.0;
/// Ratio floor for the ACPR measurement itself: an adjacent band this far below
/// the in-channel power (or lower) reports as "clean" rather than chasing
/// floating-point underflow toward -inf. Distinct from the panel's own display
/// scale — this is a measurement clamp, not a UI concern.
const ACPR_MEASURE_FLOOR_DB: f32 = -100.0;

/// How far a bin must stand above the noise floor to be counted as part of the
/// carrier when the occupied-bandwidth window is drawn.
///
/// The 99 % cumulative method answers "where does the power sit", and handed the
/// whole capture it answers "spread across the span" — a noise floor several MHz
/// wide carries real power, so the 0.5 % / 99.5 % cut-offs land near the span edges
/// and the result describes the sample rate rather than the signal. Bounding the
/// method to the carrier first is what makes the number a property of the signal.
///
/// 10 dB is the same judgement `spectrum::PEAK_PROMINENCE_DB` already makes about
/// what separates a solid carrier from FFT ripple.
const OBW_CARRIER_THRESHOLD_DB: f32 = 10.0;

/// How far the carrier window steps over bins below the threshold before it gives
/// up. A broadcast signal's own spectrum is ragged, and a momentary null inside the
/// channel must not amputate the measurement at that bin. In Hz, so it means the
/// same thing at 2 Msps as at 20.
const OBW_GAP_TOLERANCE_HZ: f64 = 10_000.0;

/// 99 % occupied bandwidth of the carrier the spectrum is centred on, in two steps:
///
/// 1. **Bound the carrier.** From the strongest bin, walk outward while bins stay
///    above `noise_floor + OBW_CARRIER_THRESHOLD_DB`, stepping over dips shorter
///    than [`OBW_GAP_TOLERANCE_HZ`] and trimming back to the last bin that actually
///    cleared the threshold.
/// 2. **Apply ITU-R SM.328 inside that window.** Exclude the bottom 0.5 % and the
///    top 0.5 % of the window's power; the span between the cut-offs is the answer.
///
/// `0` when no bin clears the threshold — the same "nothing to measure" sentinel
/// the caller already treats as `---`, rather than a guess off the noise floor.
fn occupied_bw(linear: &[f32], sample_rate: f64, noise_floor_db: f32) -> u64 {
    let n = linear.len();
    if n == 0 || sample_rate <= 0.0 { return 0; }
    let bin_hz = sample_rate / n as f64;

    let (peak_idx, peak_lin) = linear.iter().enumerate().fold(
        (0usize, f32::NEG_INFINITY),
        |(bi, bv), (i, &v)| if v > bv { (i, v) } else { (bi, bv) },
    );
    let threshold = 10f32.powf((noise_floor_db + OBW_CARRIER_THRESHOLD_DB) / 10.0);
    if !peak_lin.is_finite() || peak_lin <= threshold { return 0; }

    // Bins of slack, at least one, so a single ragged bin never ends the walk.
    let gap = (OBW_GAP_TOLERANCE_HZ / bin_hz).ceil().max(1.0) as usize;
    let mut lo = peak_idx;
    let mut miss = 0usize;
    for i in (0..peak_idx).rev() {
        if linear[i] > threshold { lo = i; miss = 0; }
        else { miss += 1; if miss > gap { break; } }
    }
    let mut hi = peak_idx;
    miss = 0;
    for (i, &v) in linear.iter().enumerate().skip(peak_idx + 1) {
        if v > threshold { hi = i; miss = 0; }
        else { miss += 1; if miss > gap { break; } }
    }

    let window = &linear[lo..=hi];
    let total: f32 = window.iter().sum();
    if !total.is_finite() || total <= 0.0 { return 0; }
    let lo_thresh = total * 0.005;
    let hi_thresh = total * 0.995;
    let mut acc = 0f32;
    let mut lo_b = 0usize;
    let mut hi_b = window.len() - 1;
    for (i, &v) in window.iter().enumerate() {
        acc += v;
        if acc < lo_thresh { lo_b = i; }
        if acc < hi_thresh { hi_b = i; }
    }
    ((hi_b.saturating_sub(lo_b) + 1) as f64 * bin_hz) as u64
}

/// Adjacent-channel power ratio: the lower/upper adjacent-band power relative to
/// the in-channel power, plus the absolute level (dBFS) of the louder — worse —
/// adjacent band. Each band is the same width as `occupied_bw_hz` (the standard
/// ACPR convention: compare like-sized channels), centred at ±`offset_hz` from
/// the spectrum's centre bin (`n/2`, already fftshifted). `None` when there is no
/// measured channel to compare against, or a band would fall outside the
/// captured span (too close to the edge, or the offset exceeds the span).
fn acpr_bands(linear: &[f32], sample_rate: f64, occupied_bw_hz: u64, offset_hz: f64)
    -> Option<(f32, f32, f32)>
{
    let n = linear.len();
    if occupied_bw_hz == 0 || sample_rate <= 0.0 || n == 0 { return None; }
    let bin_hz = sample_rate / n as f64;
    let half_bw_bins = ((occupied_bw_hz as f64 / bin_hz) / 2.0).round() as i64;
    let offset_bins  = (offset_hz / bin_hz).round() as i64;
    let center = n as i64 / 2;

    let band_power = |c: i64| -> Option<f32> {
        let lo = c - half_bw_bins;
        let hi = c + half_bw_bins;
        if lo < 0 || hi >= n as i64 || lo > hi { return None; }
        Some(linear[lo as usize..=hi as usize].iter().sum())
    };

    let ic = band_power(center)?;
    if ic <= 0.0 { return None; }
    let lower = band_power(center - offset_bins)?;
    let upper = band_power(center + offset_bins)?;

    let ratio_db = |band: f32| {
        if band > 0.0 { (10.0 * (band / ic).log10()).max(ACPR_MEASURE_FLOOR_DB) }
        else          { ACPR_MEASURE_FLOOR_DB }
    };
    let lower_db = ratio_db(lower);
    let upper_db = ratio_db(upper);
    let worse_lin = if lower_db >= upper_db { lower } else { upper };
    let adj_dbfs = if worse_lin > 0.0 { 10.0 * worse_lin.log10() } else { DB_FLOOR };
    Some((lower_db, upper_db, adj_dbfs))
}

pub struct FftWorker {
    pub sample_rx: Receiver<Vec<u8>>,
    pub state: Arc<Mutex<SdrMetrics>>,
    pub fft_size: usize,
    pub window_fn: WindowFn,
    pub ema_alpha: f32,
    pub peak_decay_db: f32,
    /// How to decode the raw bytes — set from the active device's capabilities.
    pub format: SampleFormat,
}

impl FftWorker {
    pub fn new(sample_rx: Receiver<Vec<u8>>, state: Arc<Mutex<SdrMetrics>>, format: SampleFormat) -> Self {
        Self {
            sample_rx,
            state,
            fft_size: 2048,
            window_fn: WindowFn::Hann,
            ema_alpha: 0.2,
            peak_decay_db: 0.5,
            format,
        }
    }

    pub fn run(self) {
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(self.fft_size);
        let window = dsp::compute_window(self.window_fn, self.fft_size);
        let n = self.fft_size;

        // ENBW coefficient: N × Σ(w²) / (Σ(w))² — exact for whatever window is used.
        // Hann ≈ 1.5, Hamming ≈ 1.36, Blackman ≈ 1.73.
        let w_sum_sq: f64 = window.iter().map(|&w| (w as f64).powi(2)).sum();
        let w_sum:    f64 = window.iter().map(|&w| w as f64).sum();
        let enbw_coeff = n as f64 * w_sum_sq / (w_sum * w_sum);

        // Pre-allocate all scratch buffers — reused every frame, zero heap churn
        let mut buf: Vec<u8> = Vec::new();
        let mut samples: Vec<Complex<f32>> = vec![Complex::default(); n];
        let mut mags:    Vec<f32>          = vec![0.0; n];
        let mut shifted: Vec<f32>          = vec![0.0; n];
        let mut smoothed: Vec<f32>         = vec![DB_FLOOR; n];
        let mut peak: Vec<f32>             = vec![DB_FLOOR; n];
        // scratch for noise floor partial sort (avoids O(n log n) full sort)
        let mut noise_scratch: Vec<f32>    = vec![0.0; n];
        // linear power per bin — computed once per display frame, reused for channel
        // power, occupied BW, and per-marker BW (avoids repeated 10^(x/10) powf calls)
        let mut linear: Vec<f32>           = vec![0.0; n];
        let mut initialized = false;

        // Throttle state writes to ~30 fps — EMA runs on every frame for accuracy,
        // but the expensive analysis + state lock fires at display rate only.
        const UPDATE_INTERVAL: std::time::Duration = std::time::Duration::from_millis(33);
        // Pace the drawn spectrum to the *visible* waterfall: each waterfall
        // character row packs this many data rows (half-block ▀), so the spectrum
        // refreshes once per visible line rather than every FFT frame, keeping the
        // two panels moving in lockstep. Signal metrics stay at full rate.
        const ROWS_PER_WATERFALL_LINE: u32 = 2;
        // Never let the drawn frame age past the panels' 500 ms STALE threshold,
        // even at large frames/row strides.
        const SPECTRUM_STALE_GUARD: std::time::Duration = std::time::Duration::from_millis(400);
        let mut last_state_update = std::time::Instant::now()
            .checked_sub(UPDATE_INTERVAL)
            .unwrap_or_else(std::time::Instant::now);
        let mut rows_since_spectrum: u32 = 0;
        let mut last_spectrum_update = std::time::Instant::now()
            .checked_sub(SPECTRUM_STALE_GUARD)
            .unwrap_or_else(std::time::Instant::now);
        // Live EMA factor: refreshed once per display frame from the lab's `AVG ×N`
        // control (alpha = 1/N), so trace averaging is adjustable without rebuilding
        // the worker. Starts at the worker's configured default.
        let mut current_alpha = self.ema_alpha;

        while let Ok(chunk) = self.sample_rx.recv() {
            buf.extend_from_slice(&chunk);

            let frame_bytes = n * 2;
            let mut buf_start = 0usize;

            while buf.len() - buf_start >= frame_bytes {
                let frame = &buf[buf_start..buf_start + frame_bytes];

                // Convert to windowed complex samples in-place. Branch the
                // byte→sample decode once per frame, not per sample.
                match self.format {
                    SampleFormat::Int8 => {
                        for (i, (pair, &w)) in frame.chunks_exact(2).zip(window.iter()).enumerate() {
                            samples[i] = Complex {
                                re: pair[0] as i8 as f32 / 128.0 * w,
                                im: pair[1] as i8 as f32 / 128.0 * w,
                            };
                        }
                    }
                    SampleFormat::Uint8 => {
                        for (i, (pair, &w)) in frame.chunks_exact(2).zip(window.iter()).enumerate() {
                            samples[i] = Complex {
                                re: (pair[0] as f32 - 127.5) / 127.5 * w,
                                im: (pair[1] as f32 - 127.5) / 127.5 * w,
                            };
                        }
                    }
                }
                buf_start += frame_bytes;

                fft.process(&mut samples);

                // Magnitude → dBFS in-place
                let n_f32 = n as f32;
                for (i, z) in samples.iter().enumerate() {
                    let norm = z.norm() / n_f32;
                    mags[i] = if norm > 0.0 { 20.0 * norm.log10() } else { DB_FLOOR };
                }

                // fftshift in-place
                shifted[..n / 2].copy_from_slice(&mags[n / 2..]);
                shifted[n / 2..].copy_from_slice(&mags[..n / 2]);

                // EMA smoothing + peak hold — run on every frame for accurate averaging
                if !initialized {
                    smoothed.copy_from_slice(&shifted);
                    peak.copy_from_slice(&shifted);
                    initialized = true;
                } else {
                    let alpha = current_alpha;
                    let one_minus = 1.0 - alpha;
                    for (s, &new) in smoothed.iter_mut().zip(shifted.iter()) {
                        *s = alpha * new + one_minus * *s;
                    }
                    let decay = self.peak_decay_db;
                    for (p, &s) in peak.iter_mut().zip(smoothed.iter()) {
                        *p = (*p - decay).max(s);
                    }
                }

                // Throttle: skip expensive analysis + state update until next display frame
                if last_state_update.elapsed() < UPDATE_INTERVAL {
                    continue;
                }
                last_state_update = std::time::Instant::now();

                // Noise floor: mean of bottom 10% via partial sort — O(n) average
                noise_scratch.copy_from_slice(&smoothed);
                let nf_count = (n / 10).max(1);
                noise_scratch.select_nth_unstable_by(nf_count - 1, |a, b| {
                    a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
                });
                let noise_floor = noise_scratch[..nf_count].iter().sum::<f32>() / nf_count as f32;

                // Read freq/rate while unlocked from the FFT loop
                let (center_freq_hz, sample_rate) = self
                    .state
                    .lock()
                    .map(|m| (m.radio.frequency, m.radio.config_sample_rate))
                    .unwrap_or((0, 0.0));

                // SNR: peak minus noise floor
                let peak_dbfs = smoothed.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
                let peak_to_nf_db = (peak_dbfs - noise_floor).max(0.0);

                // Single linear-power pass — 10^(x/10) is expensive; compute once,
                // reuse for channel power, occupied BW, and per-marker BW below.
                for (l, &s) in linear.iter_mut().zip(smoothed.iter()) {
                    *l = 10f32.powf(s / 10.0);
                }
                let total_linear: f32 = linear.iter().sum();

                // Channel power: integrate all bins → dBFS
                let channel_power_dbfs = if total_linear > 0.0 {
                    10.0 * total_linear.log10()
                } else {
                    f32::NEG_INFINITY
                };

                // 99% occupied BW of the carrier — ITU-R SM.328 cumulative method,
                // applied inside a window bounded to the carrier rather than to the
                // whole capture. See `occupied_bw`.
                let occupied_bw_hz = occupied_bw(&linear, sample_rate, noise_floor);
                // TEMP DEBUG — remove before commit
                if let Ok(p) = std::env::var("SDRTOP_DUMP_FFT") {
                    use std::io::Write;
                    let tmp = format!("{p}.tmp");
                    if let Ok(mut f) = std::fs::File::create(&tmp) {
                        let _ = writeln!(f, "# sample_rate={sample_rate} n={n} noise_floor={noise_floor} obw={occupied_bw_hz}");
                        for v in smoothed.iter() { let _ = writeln!(f, "{v}"); }
                        drop(f);
                        let _ = std::fs::rename(&tmp, &p);
                    }
                }

                // Adjacent-channel power ratio: L/R bands at ±ACPR_OFFSET_HZ, same
                // width as the measured in-channel occupancy. `None` becomes the
                // undefined sentinel — never a guessed ratio.
                let (acpr_lower_db, acpr_upper_db, adj_carrier_dbfs) =
                    acpr_bands(&linear, sample_rate, occupied_bw_hz, ACPR_OFFSET_HZ)
                        .unwrap_or((f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY));

                if let Ok(mut m) = self.state.lock() {
                    // Refresh the averaging factor from the lab control (cheap read
                    // under the lock we already hold for the result write-back).
                    current_alpha = m.lab.ema_alpha();
                    m.signal.peak_to_nf_db      = peak_to_nf_db;
                    m.signal.channel_power_dbfs = channel_power_dbfs;
                    m.signal.occupied_bw_hz     = occupied_bw_hz;
                    m.signal.modulation         = crate::state::classify(peak_to_nf_db, occupied_bw_hz);
                    m.signal.acpr_lower_db      = acpr_lower_db;
                    m.signal.acpr_upper_db      = acpr_upper_db;
                    m.signal.adj_carrier_dbfs   = adj_carrier_dbfs;

                    // Per-marker occupied BW within each marker's channel window
                    if sample_rate > 0.0 {
                        let bin_hz  = sample_rate / n as f64;
                        let left_hz = center_freq_hz as f64 - sample_rate / 2.0;
                        let right_hz = left_hz + sample_rate;
                        for mk in m.spectrum.markers.iter_mut() {
                            if let Some(ch_bw) = mk.channel_bw_hz {
                                let mf = mk.freq_hz as f64;
                                // Skip markers outside the current band
                                if mf < left_hz || mf > right_hz {
                                    mk.measured_bw_hz = None;
                                    continue;
                                }
                                let lo_hz  = mf - ch_bw as f64 / 2.0;
                                let hi_hz  = mf + ch_bw as f64 / 2.0;
                                let lo_bin = ((lo_hz - left_hz) / bin_hz).floor().max(0.0) as usize;
                                let hi_bin = ((hi_hz - left_hz) / bin_hz).ceil().min((n - 1) as f64) as usize;
                                if lo_bin <= hi_bin && hi_bin < n {
                                    let lin_slice = &linear[lo_bin..=hi_bin];
                                    let tot: f32 = lin_slice.iter().sum();
                                    if tot > 0.0 {
                                        let lo_t = tot * 0.005;
                                        let hi_t = tot * 0.995;
                                        let mut acc = 0f32;
                                        let mut lo_b = 0usize;
                                        let mut hi_b = lin_slice.len() - 1;
                                        for (i, &lin) in lin_slice.iter().enumerate() {
                                            acc += lin;
                                            if acc < lo_t { lo_b = i; }
                                            if acc < hi_t { hi_b = i; }
                                        }
                                        mk.measured_bw_hz = Some(((hi_b.saturating_sub(lo_b) + 1) as f64 * bin_hz) as u64);
                                    }
                                } else {
                                    mk.measured_bw_hz = None;
                                }
                            }
                        }
                    }

                    // Advance the waterfall every display frame.
                    let row_materialized = m.waterfall.buffer.push(&smoothed);
                    if row_materialized {
                        rows_since_spectrum += 1;
                    }

                    // Refresh the drawn spectrum once per visible waterfall line
                    // (or sooner if it would otherwise age toward STALE). This is
                    // the only display-paced write; the metrics above ran at full
                    // rate.
                    if m.waterfall.last_fft.is_none()
                        || rows_since_spectrum >= ROWS_PER_WATERFALL_LINE
                        || last_spectrum_update.elapsed() >= SPECTRUM_STALE_GUARD
                    {
                        rows_since_spectrum = 0;
                        last_spectrum_update = std::time::Instant::now();

                        // Reclaim the Vec allocations from the previous FftFrame
                        // before overwriting it.  We hold the mutex, so refcount == 1
                        // and try_unwrap is guaranteed to succeed — no heap alloc.
                        let (mut bins_vec, mut peak_vec) = match m.waterfall.last_fft.take() {
                            Some(old) => (
                                Arc::try_unwrap(old.bins_dbfs).unwrap_or_else(|_| vec![0.0_f32; n]),
                                Arc::try_unwrap(old.peak_hold).unwrap_or_else(|_| vec![0.0_f32; n]),
                            ),
                            None => (vec![0.0_f32; n], vec![0.0_f32; n]),
                        };
                        bins_vec.copy_from_slice(&smoothed);
                        peak_vec.copy_from_slice(&peak);
                        let bins_arc = Arc::new(bins_vec);
                        let peak_arc = Arc::new(peak_vec);

                        m.waterfall.last_fft = Some(FftFrame {
                            bins_dbfs: bins_arc,
                            peak_hold: peak_arc,
                            noise_floor,
                            center_freq_hz,
                            sample_rate,
                            timestamp: std::time::Instant::now(),
                            peak_to_nf_db,
                            channel_power_dbfs,
                            occupied_bw_hz,
                            enbw_hz: enbw_coeff * sample_rate / n as f64,
                        });
                    }
                }
            }

            // Single drain per received chunk instead of one per FFT frame
            if buf_start > 0 {
                buf.drain(..buf_start);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fftshift_dc_at_center() {
        let n = 8usize;
        let mags: Vec<f32> = (0..n).map(|i| i as f32).collect();
        let mut shifted = Vec::with_capacity(n);
        shifted.extend_from_slice(&mags[n / 2..]);
        shifted.extend_from_slice(&mags[..n / 2]);
        // shifted = [4,5,6,7,0,1,2,3]; DC (was 0) is now at index 4 = N/2
        assert_eq!(shifted[n / 2], 0.0, "DC should be at index N/2 after shift");
        assert_eq!(shifted[0], 4.0);
    }

    #[test]
    fn magnitude_floor_for_zero_input() {
        let z = Complex { re: 0.0f32, im: 0.0f32 };
        let norm = z.norm() / 2048.0f32;
        let db = if norm > 0.0 { 20.0 * norm.log10() } else { DB_FLOOR };
        assert_eq!(db, DB_FLOOR);
    }

    #[test]
    fn iq_byte_i8_max_converts_correctly() {
        let byte: u8 = 0x7F;
        let f = byte as i8 as f32 / 128.0;
        assert!((f - 0.9921875).abs() < 1e-6, "got {}", f);
    }

    #[test]
    fn iq_byte_i8_min_converts_correctly() {
        let byte: u8 = 0x80;
        let f = byte as i8 as f32 / 128.0;
        assert!((f - (-1.0)).abs() < 1e-6, "got {}", f);
    }

    #[test]
    fn iq_byte_uint8_converts_correctly() {
        // RTL-SDR unsigned-8-bit decode around the 127.5 DC bias maps the byte
        // range symmetrically onto [-1, 1]: 0x00 → -1.0, 0x80 → ~0, 0xFF → +1.0.
        let lo = (0x00u8 as f32 - 127.5) / 127.5;
        let mid = (0x80u8 as f32 - 127.5) / 127.5;
        let hi = (0xFFu8 as f32 - 127.5) / 127.5;
        assert!((lo - (-1.0)).abs() < 1e-6, "lo = {}", lo);
        assert!(mid.abs() < 0.01, "mid = {}", mid);
        assert!((hi - 1.0).abs() < 1e-6, "hi = {}", hi);
    }

    #[test]
    fn ema_converges_to_new_value() {
        let mut s = 0.0f32;
        let target = 1.0f32;
        let alpha = 0.5f32;
        for _ in 0..20 {
            s = alpha * target + (1.0 - alpha) * s;
        }
        assert!(s > 0.99, "EMA should converge to target, got {}", s);
    }

    #[test]
    fn snr_is_peak_minus_noise() {
        let peak_dbfs: f32 = -30.0;
        let noise_floor: f32 = -90.0;
        let snr = (peak_dbfs - noise_floor).max(0.0);
        assert!((snr - 60.0).abs() < 0.001);
    }

    #[test]
    fn channel_power_two_equal_bins() {
        let bins = [-60.0f32, -60.0];
        let total: f32 = bins.iter().map(|&b| 10f32.powf(b / 10.0)).sum();
        let power = 10.0 * total.log10();
        // Two -60 dBFS bins → -60 + 10*log10(2) ≈ -56.99 dBFS
        assert!((power - (-56.99)).abs() < 0.02);
    }

    #[test]
    fn channel_power_zero_signal_is_neg_inf() {
        let total_linear: f32 = 0.0;
        let power = if total_linear > 0.0 { 10.0 * total_linear.log10() } else { f32::NEG_INFINITY };
        assert!(power.is_infinite() && power < 0.0);
    }

    /// A linear-power spectrum: `n` bins of flat noise at `nf_db`, with one flat
    /// carrier `bw_hz` wide at `level_db`, offset `offset_hz` from the centre bin.
    fn spectrum(n: usize, sample_rate: f64, nf_db: f32, bw_hz: f64, level_db: f32, offset_hz: f64)
        -> Vec<f32>
    {
        let lin = |db: f32| 10f32.powf(db / 10.0);
        let mut bins = vec![lin(nf_db); n];
        let bin_hz = sample_rate / n as f64;
        let half = ((bw_hz / bin_hz) / 2.0).round() as i64;
        let c = n as i64 / 2 + (offset_hz / bin_hz).round() as i64;
        for i in (c - half)..(c + half) {
            if (0..n as i64).contains(&i) { bins[i as usize] = lin(level_db); }
        }
        bins
    }

    #[test]
    fn occupied_bw_measures_the_carrier_not_the_span() {
        // The bug this replaces: the 99 % method over the whole capture reported
        // 7.49 MHz for this signal at 10 Msps, because a noise floor that wide
        // carries enough power to push the cut-offs out to the span edges.
        let bins = spectrum(2048, 2_000_000.0, -90.0, 180_000.0, -40.0, 0.0);
        let bw = occupied_bw(&bins, 2_000_000.0, -90.0);
        assert!((170_000..=190_000).contains(&bw), "expected ~180 kHz, got {bw}");
    }

    #[test]
    fn occupied_bw_is_a_property_of_the_signal_not_the_sample_rate() {
        // The same 180 kHz carrier seen through a 2 MHz and a 10 MHz window must
        // measure the same, to within the coarser window's bin size.
        let narrow = occupied_bw(
            &spectrum(2048, 2_000_000.0, -90.0, 180_000.0, -40.0, 0.0), 2_000_000.0, -90.0);
        let wide = occupied_bw(
            &spectrum(2048, 10_000_000.0, -90.0, 180_000.0, -40.0, 0.0), 10_000_000.0, -90.0);
        let bin_hz = 10_000_000.0 / 2048.0;
        assert!((narrow as f64 - wide as f64).abs() < 3.0 * bin_hz,
                "narrow={narrow} wide={wide}, bin={bin_hz:.0} Hz");
    }

    #[test]
    fn occupied_bw_follows_a_carrier_that_is_not_centred() {
        // The window is drawn around the strongest bin, so an off-centre station
        // still reports its own bandwidth rather than its distance from centre.
        let bins = spectrum(2048, 2_000_000.0, -90.0, 180_000.0, -40.0, 300_000.0);
        let bw = occupied_bw(&bins, 2_000_000.0, -90.0);
        assert!((170_000..=190_000).contains(&bw), "expected ~180 kHz, got {bw}");
    }

    #[test]
    fn occupied_bw_reads_a_narrow_carrier_as_narrow() {
        // The wide/narrow split is what `classify` turns into the MOD badge, so a
        // 15 kHz channel must not come back looking like broadcast FM.
        let bins = spectrum(2048, 2_000_000.0, -90.0, 15_000.0, -40.0, 0.0);
        let bw = occupied_bw(&bins, 2_000_000.0, -90.0);
        assert!((10_000..=20_000).contains(&bw), "expected ~15 kHz, got {bw}");
        assert_eq!(crate::state::classify(50.0, bw), crate::state::Modulation::Nfm);
    }

    #[test]
    fn occupied_bw_steps_over_a_null_inside_the_channel() {
        // A broadcast signal's own spectrum is ragged. A momentary null a few bins
        // wide must not amputate the window at that bin.
        let mut bins = spectrum(2048, 2_000_000.0, -90.0, 180_000.0, -40.0, 0.0);
        let notch = 2048 / 2 - 40;
        for b in bins.iter_mut().skip(notch).take(5) { *b = 10f32.powf(-90.0 / 10.0); }
        let bw = occupied_bw(&bins, 2_000_000.0, -90.0);
        assert!((170_000..=190_000).contains(&bw), "null truncated the window: {bw}");
    }

    #[test]
    fn occupied_bw_is_zero_without_a_carrier() {
        // Noise with a few dB of ripple is still noise: no window, no measurement,
        // and `---` on screen rather than a bandwidth invented from the floor.
        let mut bins = vec![10f32.powf(-90.0 / 10.0); 2048];
        for (i, b) in bins.iter_mut().enumerate() {
            *b = 10f32.powf((-90.0 + (i % 7) as f32 - 3.0) / 10.0);
        }
        assert_eq!(occupied_bw(&bins, 2_000_000.0, -90.0), 0);
    }

    #[test]
    fn occupied_bw_declines_degenerate_input() {
        assert_eq!(occupied_bw(&[], 2_000_000.0, -90.0), 0);
        assert_eq!(occupied_bw(&[1.0, 1.0, 1.0], 0.0, -90.0), 0);
    }

    #[test]
    fn acpr_bands_computes_relative_and_absolute_adjacent_levels() {
        // 100 bins over 1 MHz (10 kHz/bin). A strong 100 kHz in-channel signal at
        // centre, with both adjacent bands (±200 kHz) 40 dB weaker.
        let n = 100;
        let sample_rate = 1_000_000.0;
        let mut linear = vec![1e-8f32; n];
        let center = n / 2;
        for i in center - 5..center + 5 { linear[i] = 1.0; }       // in-channel: sum = 10.0
        for i in center - 25..center - 15 { linear[i] = 1e-4; }    // lower adjacent: sum = 1e-3
        for i in center + 15..center + 25 { linear[i] = 1e-4; }    // upper adjacent: sum = 1e-3

        let (lo_db, up_db, adj_dbfs) = acpr_bands(&linear, sample_rate, 100_000, 200_000.0).unwrap();
        // ratio = 1e-3 / 10.0 = 1e-4 → -40 dB
        assert!((lo_db - (-40.0)).abs() < 0.5, "lo_db={lo_db}");
        assert!((up_db - (-40.0)).abs() < 0.5, "up_db={up_db}");
        // absolute adjacent level: 10*log10(1e-3) = -30 dBFS
        assert!((adj_dbfs - (-30.0)).abs() < 0.5, "adj_dbfs={adj_dbfs}");
    }

    #[test]
    fn acpr_bands_none_when_band_falls_outside_span() {
        let linear = vec![1.0f32; 20];
        // A 900 kHz offset on a 1 MHz / 20-bin span pushes the adjacent band
        // clean off the array — must report "can't measure", not a wrong number.
        assert!(acpr_bands(&linear, 1_000_000.0, 100_000, 900_000.0).is_none());
    }

    #[test]
    fn acpr_bands_none_without_occupied_bandwidth() {
        let linear = vec![1.0f32; 100];
        assert!(acpr_bands(&linear, 1_000_000.0, 0, 200_000.0).is_none());
    }

    #[test]
    fn acpr_bands_clean_adjacent_clamps_to_measure_floor() {
        // In-channel signal, but genuinely silent adjacent bands (linear ~ 0).
        let n = 100;
        let mut linear = vec![0.0f32; n];
        let center = n / 2;
        for i in center - 5..center + 5 { linear[i] = 1.0; }
        let (lo_db, up_db, _) = acpr_bands(&linear, 1_000_000.0, 100_000, 200_000.0).unwrap();
        assert_eq!(lo_db, ACPR_MEASURE_FLOOR_DB);
        assert_eq!(up_db, ACPR_MEASURE_FLOOR_DB);
    }
}
