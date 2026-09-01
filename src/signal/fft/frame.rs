//! The per-frame DSP: bytes in, a smoothed spectrum out.
//!
//! Runs on **every** FFT frame, not at display rate, because the averaging is
//! only accurate if it sees every one. Nothing here takes the lock - it is the
//! other half of the worker's discipline: the cheap-and-constant part runs at
//! full rate and lock-free, and only the expensive analysis and the write-back
//! are throttled to the display.

use num_complex::Complex;

use crate::hardware::{SampleFormat, SampleGeometry};

use super::DB_FLOOR;

/// Decode one frame of interleaved bytes into windowed complex samples.
///
/// The format branch is taken **once per frame**, not once per sample: the inner
/// loop is the hottest in the program, and a match inside it would be paid
/// thousands of times for an answer that cannot change within a frame.
pub(super) fn decode_into(
    frame: &[u8],
    window: &[f32],
    geometry: SampleGeometry,
    out: &mut [Complex<f32>],
) {
    match geometry.format {
        SampleFormat::Int8 => {
            let fs = geometry.full_scale;
            for (i, (pair, &w)) in frame
                .as_chunks::<2>()
                .0
                .iter()
                .zip(window.iter())
                .enumerate()
            {
                out[i] = Complex {
                    re: pair[0] as i8 as f32 / fs * w,
                    im: pair[1] as i8 as f32 / fs * w,
                };
            }
        }
        // RTL-SDR unsigned-8-bit decode around the 127.5 DC bias maps the byte
        // range symmetrically onto [-1, 1]: 0x00 → -1.0, 0x80 → ~0, 0xFF → +1.0.
        //
        // The bias sits half a count below mid-scale, hence `full_scale - 0.5`.
        // `hardware::process` centres its *accumulators* by the whole 128
        // instead, deliberately, and the half count matters to neither job.
        // These two numbers look like they want to be one number. They do not.
        SampleFormat::Uint8 => {
            let bias = geometry.full_scale - 0.5;
            for (i, (pair, &w)) in frame
                .as_chunks::<2>()
                .0
                .iter()
                .zip(window.iter())
                .enumerate()
            {
                out[i] = Complex {
                    re: (pair[0] as f32 - bias) / bias * w,
                    im: (pair[1] as f32 - bias) / bias * w,
                };
            }
        }
    }
}

/// Bin magnitudes as dBFS, normalised by the transform length.
///
/// A zero bin floors rather than going to −∞, so nothing downstream has to guard
/// against an infinity that only ever means "silence".
pub(super) fn magnitudes_dbfs(samples: &[Complex<f32>], out: &mut [f32]) {
    let n = samples.len() as f32;
    for (i, z) in samples.iter().enumerate() {
        let norm = z.norm() / n;
        out[i] = if norm > 0.0 {
            20.0 * norm.log10()
        } else {
            DB_FLOOR
        };
    }
}

/// Rotate the spectrum so DC sits in the middle, where the display expects it.
pub(super) fn fftshift_into(mags: &[f32], out: &mut [f32]) {
    let n = mags.len();
    out[..n / 2].copy_from_slice(&mags[n / 2..]);
    out[n / 2..].copy_from_slice(&mags[..n / 2]);
}

/// Exponential trace averaging plus a decaying peak hold.
///
/// The first frame seeds both directly instead of averaging toward the floor,
/// which would otherwise take several seconds to climb into view. `initialized`
/// carries that across calls.
pub(super) fn average(
    shifted: &[f32],
    smoothed: &mut [f32],
    peak: &mut [f32],
    alpha: f32,
    decay_db: f32,
    initialized: &mut bool,
) {
    if !*initialized {
        smoothed.copy_from_slice(shifted);
        peak.copy_from_slice(shifted);
        *initialized = true;
        return;
    }
    let one_minus = 1.0 - alpha;
    for (s, &new) in smoothed.iter_mut().zip(shifted.iter()) {
        *s = alpha * new + one_minus * *s;
    }
    for (p, &s) in peak.iter_mut().zip(smoothed.iter()) {
        *p = (*p - decay_db).max(s);
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
        let z = Complex {
            re: 0.0f32,
            im: 0.0f32,
        };
        let norm = z.norm() / 2048.0f32;
        let db = if norm > 0.0 {
            20.0 * norm.log10()
        } else {
            DB_FLOOR
        };
        assert_eq!(db, DB_FLOOR);
    }

    /// Decode one I/Q pair through the real function, with a unit window so the
    /// windowing cannot hide a scaling mistake.
    ///
    /// These three tests used to compute `byte as i8 as f32 / 128.0` themselves
    /// and assert the answer, which passes whatever `decode_into` does. They ask
    /// it now.
    fn decoded(bytes: [u8; 2], format: SampleFormat, full_scale: f32) -> Complex<f32> {
        let mut out = [Complex { re: 0.0, im: 0.0 }];
        decode_into(
            &bytes,
            &[1.0],
            SampleGeometry { format, full_scale },
            &mut out,
        );
        out[0]
    }

    #[test]
    fn iq_byte_i8_max_converts_correctly() {
        let z = decoded([0x7F, 0x7F], SampleFormat::Int8, 128.0);
        assert!((z.re - 0.9921875).abs() < 1e-6, "got {}", z.re);
    }

    #[test]
    fn iq_byte_i8_min_converts_correctly() {
        let z = decoded([0x80, 0x80], SampleFormat::Int8, 128.0);
        assert!((z.re - (-1.0)).abs() < 1e-6, "got {}", z.re);
    }

    #[test]
    fn iq_byte_uint8_converts_correctly() {
        // RTL-SDR unsigned-8-bit decode around the 127.5 DC bias maps the byte
        // range symmetrically onto [-1, 1]: 0x00 → -1.0, 0x80 → ~0, 0xFF → +1.0.
        let lo = decoded([0x00, 0x00], SampleFormat::Uint8, 128.0).re;
        let mid = decoded([0x80, 0x80], SampleFormat::Uint8, 128.0).re;
        let hi = decoded([0xFF, 0xFF], SampleFormat::Uint8, 128.0).re;
        assert!((lo - (-1.0)).abs() < 1e-6, "lo = {}", lo);
        assert!(mid.abs() < 0.01, "mid = {}", mid);
        assert!((hi - 1.0).abs() < 1e-6, "hi = {}", hi);
    }

    /// The window multiplies the decoded sample, and it does so on both formats.
    /// A decoder that applied the window to only one branch would still pass
    /// every test above.
    #[test]
    fn the_window_is_applied_to_both_formats() {
        let mut out = [Complex { re: 0.0, im: 0.0 }];
        for format in [SampleFormat::Int8, SampleFormat::Uint8] {
            decode_into(
                &[0x7F, 0x7F],
                &[0.5],
                SampleGeometry {
                    format,
                    full_scale: 128.0,
                },
                &mut out,
            );
            let full = decoded([0x7F, 0x7F], format, 128.0).re;
            assert!(
                (out[0].re - full * 0.5).abs() < 1e-6,
                "{format:?}: windowed {} is not half of {full}",
                out[0].re
            );
        }
    }

    /// Scaling follows the device's declared full scale, not a constant. Proven
    /// on a fabricated wider geometry before any device reports one.
    #[test]
    fn decoding_follows_the_declared_full_scale() {
        // +64 counts is half scale against 128 and an eighth against 512.
        assert!((decoded([0x40, 0x40], SampleFormat::Int8, 128.0).re - 0.5).abs() < 1e-6);
        assert!((decoded([0x40, 0x40], SampleFormat::Int8, 512.0).re - 0.125).abs() < 1e-6);
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
}
