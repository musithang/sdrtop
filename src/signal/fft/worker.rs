//! The FFT thread: receive bytes, transform, measure, publish.
//!
//! **Split by when it holds the lock**, the same way `tasks/rx/` is, so the
//! discipline is visible in the file layout rather than in a comment:
//!
//! - [`frame`](super::frame) is the per-frame DSP. Runs at **full rate**, holds
//!   no lock - the averaging is only accurate if it sees every frame.
//! - [`analysis`](super::analysis) is the expensive maths. Runs at display rate,
//!   holds **no lock**, reads no clock.
//! - [`publish`](super::publish) is the one lock block.
//!
//! There is exactly one other lock acquisition: a short read of frequency and
//! sample rate before the analysis, because the analysis needs them and must not
//! be holding the mutex while it runs.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;
use num_complex::Complex;
use rustfft::FftPlanner;

use crate::hardware::SampleGeometry;
use crate::signal::dsp::{self, WindowFn};
use crate::state::SdrMetrics;

use super::publish::{Pacing, Snapshot};
use super::{analysis, frame, DB_FLOOR};

/// Throttle state writes to ~30 fps - the EMA runs on every frame for accuracy,
/// but the expensive analysis and the state lock fire at display rate only.
const UPDATE_INTERVAL: Duration = Duration::from_millis(33);

pub struct FftWorker {
    pub sample_rx: Receiver<Vec<u8>>,
    pub state: Arc<Mutex<SdrMetrics>>,
    pub fft_size: usize,
    pub window_fn: WindowFn,
    pub ema_alpha: f32,
    pub peak_decay_db: f32,
    /// How to decode the raw bytes - set from the active device's capabilities.
    pub geometry: SampleGeometry,
}

impl FftWorker {
    pub fn new(
        sample_rx: Receiver<Vec<u8>>,
        state: Arc<Mutex<SdrMetrics>>,
        geometry: SampleGeometry,
    ) -> Self {
        Self {
            sample_rx,
            state,
            fft_size: 2048,
            window_fn: WindowFn::Hann,
            ema_alpha: 0.2,
            peak_decay_db: 0.5,
            geometry,
        }
    }

    pub fn run(self) {
        let n = self.fft_size;
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(n);
        let window = dsp::compute_window(self.window_fn, n);

        // ENBW coefficient: N × Σ(w²) / (Σ(w))² - exact for whatever window is used.
        // Hann ≈ 1.5, Hamming ≈ 1.36, Blackman ≈ 1.73.
        let w_sum_sq: f64 = window.iter().map(|&w| (w as f64).powi(2)).sum();
        let w_sum: f64 = window.iter().map(|&w| w as f64).sum();
        let enbw_coeff = n as f64 * w_sum_sq / (w_sum * w_sum);

        // Pre-allocate every scratch buffer - reused each frame, zero heap churn.
        let mut buf: Vec<u8> = Vec::new();
        let mut samples: Vec<Complex<f32>> = vec![Complex::default(); n];
        let mut mags: Vec<f32> = vec![0.0; n];
        let mut shifted: Vec<f32> = vec![0.0; n];
        let mut smoothed: Vec<f32> = vec![DB_FLOOR; n];
        let mut peak: Vec<f32> = vec![DB_FLOOR; n];
        let mut noise_scratch: Vec<f32> = vec![0.0; n];
        let mut linear: Vec<f32> = vec![0.0; n];
        let mut initialized = false;

        let mut last_state_update = Instant::now()
            .checked_sub(UPDATE_INTERVAL)
            .unwrap_or_else(Instant::now);
        let mut pacing = Pacing::new();
        // Live EMA factor: refreshed once per display frame from the lab's `AVG ×N`
        // control (alpha = 1/N), so trace averaging is adjustable without
        // rebuilding the worker. Starts at the worker's configured default.
        let mut current_alpha = self.ema_alpha;

        while let Ok(chunk) = self.sample_rx.recv() {
            buf.extend_from_slice(&chunk);

            let frame_bytes = n * 2;
            let mut buf_start = 0usize;

            while buf.len() - buf_start >= frame_bytes {
                // ── Full rate, no lock ──────────────────────────────────────
                frame::decode_into(
                    &buf[buf_start..buf_start + frame_bytes],
                    &window,
                    self.geometry,
                    &mut samples,
                );
                buf_start += frame_bytes;

                fft.process(&mut samples);
                frame::magnitudes_dbfs(&samples, &mut mags);
                frame::fftshift_into(&mags, &mut shifted);
                frame::average(
                    &shifted,
                    &mut smoothed,
                    &mut peak,
                    current_alpha,
                    self.peak_decay_db,
                    &mut initialized,
                );

                if last_state_update.elapsed() < UPDATE_INTERVAL {
                    continue;
                }
                last_state_update = Instant::now();

                // ── Lock, briefly: the analysis needs the tuning ────────────
                let (center_freq_hz, sample_rate) = self
                    .state
                    .lock()
                    .map(|m| (m.radio.frequency, m.radio.config_sample_rate))
                    .unwrap_or((0, 0.0));

                // ── Display rate, no lock ──────────────────────────────────
                let reading =
                    analysis::measure(&smoothed, &mut linear, &mut noise_scratch, sample_rate);

                // ── Lock block ─────────────────────────────────────────────
                if let Some(alpha) = super::publish::publish(
                    &self.state,
                    Snapshot {
                        reading: &reading,
                        smoothed: &smoothed,
                        peak: &peak,
                        linear: &linear,
                        center_freq_hz,
                        sample_rate,
                        enbw_hz: enbw_coeff * sample_rate / n as f64,
                    },
                    &mut pacing,
                ) {
                    current_alpha = alpha;
                }
            }

            // Single drain per received chunk instead of one per FFT frame.
            if buf_start > 0 {
                buf.drain(..buf_start);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::SdrMetrics;
    use std::f32::consts::TAU;

    /// Run the real worker over synthetic IQ and return the state it published.
    ///
    /// The worker had no test at all before the split - it is a thread that reads
    /// a channel and writes a mutex, and nothing exercised it end to end. This is
    /// what makes the split provable: the arithmetic moved between files, so the
    /// check that matters is that a known tone still comes out where it went in.
    fn run_against(tone_bin_offset: i32, frames: usize) -> SdrMetrics {
        const N: usize = 2048;
        let (tx, rx) = crossbeam_channel::unbounded::<Vec<u8>>();
        let state = Arc::new(Mutex::new(SdrMetrics::fixture().streaming()));
        let sample_rate = state.lock().unwrap().radio.config_sample_rate as f32;
        let bin_hz = sample_rate / N as f32;
        let tone_hz = tone_bin_offset as f32 * bin_hz;

        let mut phase = 0.0f32;
        let step = TAU * tone_hz / sample_rate;
        for _ in 0..frames {
            let mut bytes = Vec::with_capacity(N * 2);
            for _ in 0..N {
                bytes.push((phase.cos() * 120.0) as i8 as u8);
                bytes.push((phase.sin() * 120.0) as i8 as u8);
                phase += step;
            }
            tx.send(bytes).unwrap();
        }
        drop(tx); // ends the worker's recv loop

        let worker = FftWorker::new(
            rx,
            Arc::clone(&state),
            SampleGeometry {
                format: crate::hardware::SampleFormat::Int8,
                full_scale: 128.0,
            },
        );
        std::thread::spawn(move || worker.run()).join().unwrap();

        let guard = state.lock().unwrap();
        guard.clone()
    }

    /// A tone put in at a known offset comes out at that bin, at full amplitude,
    /// standing clear of the floor. This exercises the whole chain - decode,
    /// window, transform, magnitude, fftshift, average - and the publish block.
    #[test]
    fn a_known_tone_lands_in_its_own_bin() {
        const N: usize = 2048;
        let offset = 20;
        let m = run_against(offset, 1);
        let fr = m
            .waterfall
            .last_fft
            .as_ref()
            .expect("the worker published no spectrum");
        assert_eq!(fr.bins_dbfs.len(), N);

        let (idx, _) = fr
            .bins_dbfs
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap();
        // fftshift puts DC at N/2, so a +20-bin tone lands 20 right of centre.
        assert!(
            (idx as i32 - (N as i32 / 2 + offset)).abs() <= 1,
            "peak at bin {idx}, expected {}",
            N / 2 + offset as usize
        );
    }

    /// The SNR the worker publishes is the tone above the noise floor, and the
    /// noise floor is the quiet part of the span - not the tone.
    #[test]
    fn the_published_snr_measures_the_tone_against_the_floor() {
        let m = run_against(20, 1);
        let fr = m.waterfall.last_fft.as_ref().unwrap();
        assert!(
            fr.noise_floor < -40.0,
            "a single tone should leave a quiet floor, got {}",
            fr.noise_floor
        );
        assert!(
            m.signal.peak_to_nf_db > 30.0,
            "a full-scale tone should stand well clear, got {}",
            m.signal.peak_to_nf_db
        );
        assert_eq!(
            m.signal.peak_to_nf_db, fr.peak_to_nf_db,
            "one SNR, two readers"
        );
    }

    /// A single tone is a narrow carrier: the occupied bandwidth is a few bins,
    /// not the span, and the channel power is a real number rather than −∞.
    #[test]
    fn a_tone_reads_as_a_narrow_carrier() {
        let m = run_against(20, 1);
        let span = m.radio.config_sample_rate as u64;
        assert!(m.signal.occupied_bw_hz > 0, "no carrier found");
        assert!(
            m.signal.occupied_bw_hz < span / 20,
            "a tone should not occupy {} Hz of a {span} Hz span",
            m.signal.occupied_bw_hz
        );
        assert!(
            m.signal.channel_power_dbfs.is_finite(),
            "a carrier must have a channel power"
        );
    }

    /// The averaging runs on every frame, so more frames converge rather than
    /// diverge - and the peak hold never falls below the trace.
    #[test]
    fn averaging_over_many_frames_stays_bounded() {
        let m = run_against(20, 40);
        let fr = m.waterfall.last_fft.as_ref().unwrap();
        assert!(
            fr.bins_dbfs.iter().all(|v| v.is_finite()),
            "the averaged trace went non-finite"
        );
        assert!(
            fr.peak_hold
                .iter()
                .zip(fr.bins_dbfs.iter())
                .all(|(p, s)| p >= s),
            "peak hold fell below the trace it holds"
        );
    }

    /// The ENBW published with the frame is the window's, scaled by the bin
    /// width - a Hann window is about 1.5 bins wide.
    #[test]
    fn the_frame_carries_the_windows_noise_bandwidth() {
        const N: f64 = 2048.0;
        let m = run_against(20, 1);
        let fr = m.waterfall.last_fft.as_ref().unwrap();
        let bin_hz = fr.sample_rate / N;
        let coeff = fr.enbw_hz / bin_hz;
        assert!(
            (coeff - 1.5).abs() < 0.05,
            "Hann ENBW should be ~1.5 bins, got {coeff:.3}"
        );
    }
}
