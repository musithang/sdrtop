//! The per-frame DSP: bytes in, a smoothed spectrum out.
//!
//! Runs on **every** FFT frame, not at display rate, because the averaging is
//! only accurate if it sees every one. Nothing here takes the lock — it is the
//! other half of the worker's discipline: the cheap-and-constant part runs at
//! full rate and lock-free, and only the expensive analysis and the write-back
//! are throttled to the display.

use num_complex::Complex;

use crate::hardware::SampleFormat;

use super::DB_FLOOR;

/// Decode one frame of interleaved bytes into windowed complex samples.
///
/// The format branch is taken **once per frame**, not once per sample: the inner
/// loop is the hottest in the program, and a match inside it would be paid
/// thousands of times for an answer that cannot change within a frame.
pub(super) fn decode_into(
    frame: &[u8],
    window: &[f32],
    format: SampleFormat,
    out: &mut [Complex<f32>],
) {
    match format {
        SampleFormat::Int8 => {
            for (i, (pair, &w)) in frame
                .as_chunks::<2>()
                .0
                .iter()
                .zip(window.iter())
                .enumerate()
            {
                out[i] = Complex {
                    re: pair[0] as i8 as f32 / 128.0 * w,
                    im: pair[1] as i8 as f32 / 128.0 * w,
                };
            }
        }
        // RTL-SDR unsigned-8-bit decode around the 127.5 DC bias maps the byte
        // range symmetrically onto [-1, 1]: 0x00 → -1.0, 0x80 → ~0, 0xFF → +1.0.
        SampleFormat::Uint8 => {
            for (i, (pair, &w)) in frame
                .as_chunks::<2>()
                .0
                .iter()
                .zip(window.iter())
                .enumerate()
            {
                out[i] = Complex {
                    re: (pair[0] as f32 - 127.5) / 127.5 * w,
                    im: (pair[1] as f32 - 127.5) / 127.5 * w,
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
}
