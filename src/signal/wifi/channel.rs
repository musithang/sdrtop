// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The channel a frame came through, subcarrier by subcarrier, from its long
//! training field, each tone with the uncertainty the noise leaves on it.
//!
//! **Two symbols, so the noise is measured, not assumed.** The long training
//! field sends one symbol twice (17.3.3): their mean is the estimate, their
//! difference holds only noise, and that noise is what each tone's
//! uncertainty is made of.
//!
//! **The floor's shape is noise, not a gain.** Our receiver's noise floor is
//! not flat at every gain: on the HackRF at high gain it rises towards the
//! band edges, while the frames heard through it do not, so the rise is the
//! noise's (folded in from beyond the band, most likely) and dividing it out
//! of the channel would correct something the signal never had. [`Floor`]
//! measures that shape from quiet stretches and it scales each tone's noise
//! variance: where the floor is higher, the tone is less certain, and says
//! so. Our own filter's gain shape is not claimed at all: the air cannot tell
//! it from the channel and the transmitter's filter without a reference
//! signal, and nothing here transmits one.

use num_complex::Complex;
use rustfft::FftPlanner;

/// The 52 occupied subcarriers, -26 to 26 without DC (17.3.2.4).
pub const TONES: [i32; 52] = {
    let mut t = [0i32; 52];
    let mut i = 0;
    let mut k = -26;
    while k <= 26 {
        if k != 0 {
            t[i] = k;
            i += 1;
        }
        k += 1;
    }
    t
};

/// The long training field's value on subcarrier `k` (Equation 17-8): +-1.
fn long(k: i32) -> f64 {
    super::ofdm::LONG[(k + 26) as usize] as f64
}

/// The sample rate the transform's subcarriers are 312.5 kHz apart at.
const FS: f64 = 20e6;

/// The noise floor's shape over the 52 tones, relative to its centre, and
/// each value's standard error.
#[derive(Clone, Debug, PartialEq)]
pub struct Floor {
    pub shape: Vec<f64>,
    pub error: Vec<f64>,
}

impl Floor {
    /// A floor with no shape: what is assumed only where none was measured.
    pub fn flat() -> Self {
        Self {
            shape: vec![1.0; 52],
            error: vec![0.0; 52],
        }
    }

    /// The floor from the quietest 64-sample stretches of `x` (those within
    /// half again of the tenth percentile's power), each subcarrier's mean
    /// power relative to the mean of the six nearest the centre. `None`
    /// with too few stretches to say anything (fewer than 32).
    #[cfg(test)]
    pub fn measure(x: &[Complex<f32>]) -> Option<Self> {
        let blocks: Vec<&[Complex<f32>; 64]> = x.as_chunks::<64>().0.iter().collect();
        let power: Vec<f64> = blocks
            .iter()
            .map(|b| b.iter().map(|z| z.norm_sqr() as f64).sum::<f64>())
            .collect();
        let mut sorted = power.clone();
        sorted.sort_by(f64::total_cmp);
        let cut = sorted.get(sorted.len() / 10)? * 1.5;
        let fft = FftPlanner::new().plan_fft_forward(64);
        let mut sum = vec![0.0f64; 64];
        let mut used = 0usize;
        for (b, p) in blocks.iter().zip(&power) {
            if *p > cut {
                continue;
            }
            let mut v: Vec<Complex<f64>> = b
                .iter()
                .map(|z| Complex::new(z.re as f64, z.im as f64))
                .collect();
            fft.process(&mut v);
            for (s, z) in sum.iter_mut().zip(&v) {
                *s += z.norm_sqr();
            }
            used += 1;
        }
        if used < 32 {
            return None;
        }
        let bin = |k: i32| sum[k.rem_euclid(64) as usize] / used as f64;
        let centre = [-4, -3, -2, 2, 3, 4].iter().map(|&k| bin(k)).sum::<f64>() / 6.0;
        if centre <= 0.0 {
            return None;
        }
        // A bin's power over `used` stretches of noise is a mean of that many
        // exponential readings: its relative standard error is 1/sqrt(used).
        let shape: Vec<f64> = TONES.iter().map(|&k| bin(k) / centre).collect();
        let error = shape.iter().map(|s| s / (used as f64).sqrt()).collect();
        Some(Self { shape, error })
    }
}

/// A frame's channel: each tone's value (in `TONES` order) and the variance
/// the noise leaves on it.
#[derive(Clone, Debug)]
pub struct Estimate {
    pub h: Vec<Complex<f64>>,
    pub variance: Vec<f64>,
}

/// The channel from the long training field whose first symbol starts at
/// stream pair `ltf_start`, the samples derotated by the frame's
/// `offset_hz`, from `samples` starting at `first_pair`. `None` unless both
/// symbols are held.
pub fn estimate(
    samples: &[Complex<f32>],
    first_pair: u64,
    ltf_start: u64,
    offset_hz: f64,
    floor: &Floor,
) -> Option<Estimate> {
    let at = ltf_start.checked_sub(first_pair)? as usize;
    let both = samples.get(at..at + 128)?;
    let turn = -std::f64::consts::TAU * offset_hz / FS;
    let fft = FftPlanner::new().plan_fft_forward(64);
    let symbol = |half: usize| {
        let mut v: Vec<Complex<f64>> = both[64 * half..64 * half + 64]
            .iter()
            .enumerate()
            .map(|(n, z)| {
                Complex::new(z.re as f64, z.im as f64)
                    * Complex::from_polar(1.0, turn * (64 * half + n) as f64)
            })
            .collect();
        fft.process(&mut v);
        v
    };
    let (y1, y2) = (symbol(0), symbol(1));
    let bin = |v: &[Complex<f64>], k: i32| v[k.rem_euclid(64) as usize];
    let h: Vec<Complex<f64>> = TONES
        .iter()
        .map(|&k| (bin(&y1, k) + bin(&y2, k)) / (2.0 * long(k)))
        .collect();
    // The difference of the two symbols is noise alone, twice a symbol's;
    // with the floor's shape taken out, its mean over the tones is one
    // symbol's noise at the centre.
    let centre_noise = TONES
        .iter()
        .zip(&floor.shape)
        .map(|(&k, s)| (bin(&y1, k) - bin(&y2, k)).norm_sqr() / (2.0 * s))
        .sum::<f64>()
        / TONES.len() as f64;
    // The mean of two symbols halves it, and the floor shapes it back.
    let variance = floor.shape.iter().map(|s| centre_noise * s / 2.0).collect();
    Some(Estimate { h, variance })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::dsp::testkit::Rng;
    use crate::signal::wifi::annex::MESSAGE;
    use crate::signal::wifi::detect::StfDetector;
    use crate::signal::wifi::ppdu::ppdu;
    use crate::signal::wifi::signal_field::Rate;
    use crate::signal::wifi::sync::{examine, Verdict};

    const FS: f64 = 20e6;

    /// The annex's frame through `taps` (one a sample), `lead` samples into
    /// noise `snr_db` below it, offset by `offset_hz`; and the factor its
    /// samples were scaled by.
    fn on_air(
        taps: &[Complex<f64>],
        snr_db: f64,
        offset_hz: f64,
        seed: u64,
    ) -> (Vec<Complex<f32>>, f64) {
        let f = ppdu(&MESSAGE, Rate::R36, 54);
        let power = f[..320].iter().map(|z| z.norm_sqr()).sum::<f64>() / 320.0;
        let scale = 1.0 / power.sqrt();
        let lead = 700;
        let mut y = vec![Complex::new(0.0, 0.0); lead + f.len() + 600];
        for (n, z) in f.iter().enumerate() {
            for (d, t) in taps.iter().enumerate() {
                y[lead + n + d] += z * t * scale;
            }
        }
        let noise = Rng::new(seed).noise(y.len(), 10f64.powf(-snr_db / 10.0));
        let x = y
            .iter()
            .enumerate()
            .map(|(n, z)| {
                let z =
                    z * Complex::from_polar(1.0, std::f64::consts::TAU * offset_hz * n as f64 / FS);
                Complex::new(z.re as f32, z.im as f32) + noise[n]
            })
            .collect();
        (x, scale)
    }

    /// Synchronised and estimated, through the receive chain as it runs.
    fn estimated(x: &[Complex<f32>], floor: &Floor) -> Estimate {
        let peak = StfDetector::new().push(0, x)[0].peak;
        let Some(Verdict::ThisChannel(s)) = examine(x, 0, peak) else {
            panic!("not judged this channel's");
        };
        estimate(x, 0, s.ltf_start, s.offset.value(), floor).expect("held")
    }

    /// The channel each tone should read for `taps`, scaled as the frame was.
    fn truth(taps: &[Complex<f64>], scale: f64) -> Vec<Complex<f64>> {
        TONES
            .iter()
            .map(|&k| {
                taps.iter()
                    .enumerate()
                    .map(|(d, t)| {
                        t * Complex::from_polar(
                            1.0,
                            -std::f64::consts::TAU * (k * d as i32) as f64 / 64.0,
                        )
                    })
                    .sum::<Complex<f64>>()
                    * scale
            })
            .collect()
    }

    /// An echo 250 ns late (5 samples) at half amplitude and a phase of its
    /// own, read off every tone, and the two taps found again from them.
    #[test]
    fn a_two_tap_channel_is_recovered() {
        let taps = [
            Complex::new(1.0, 0.0),
            Complex::new(0.0, 0.0),
            Complex::new(0.0, 0.0),
            Complex::new(0.0, 0.0),
            Complex::new(0.0, 0.0),
            Complex::from_polar(0.5, 1.0),
        ];
        let (x, scale) = on_air(&taps, 30.0, 20e3, 1);
        let e = estimated(&x, &Floor::flat());
        let want = truth(&taps, scale);
        // The carrier's phase when the frame began is in every tone alike,
        // and no receiver can know it: compare up to that one rotation.
        let common =
            e.h.iter()
                .zip(&want)
                .map(|(h, w)| h * w.conj())
                .sum::<Complex<f64>>()
                .arg();
        let turn = Complex::from_polar(1.0, -common);
        for (i, (h, w)) in e.h.iter().zip(&want).enumerate() {
            assert!(
                (h * turn - w).norm() < 0.08 * scale,
                "tone {}: {h} against {w}",
                TONES[i]
            );
        }
        // The taps, by matched filter over the 52 tones, with the same
        // rotation taken out: against what the same filter makes of the
        // true channel, at every delay up to 15 samples. The twelve tones
        // never sent leave the filter sidelobes (about -0.19 two samples
        // from a tap), so the truth is put through it too rather than
        // compared with bare taps.
        let filtered = |values: &[Complex<f64>], d: usize| -> Complex<f64> {
            values
                .iter()
                .zip(TONES)
                .map(|(h, k)| {
                    h * Complex::from_polar(
                        1.0,
                        std::f64::consts::TAU * (k * d as i32) as f64 / 64.0,
                    )
                })
                .sum::<Complex<f64>>()
                / (52.0 * scale)
        };
        let rotated: Vec<Complex<f64>> = e.h.iter().map(|h| h * turn).collect();
        for d in 0..16 {
            let (got, should) = (filtered(&rotated, d), filtered(&want, d));
            assert!(
                (got - should).norm() < 0.05,
                "delay {d}: {got} against {should}"
            );
        }
        // And the echo stands out where it is, at half the direct path.
        let echo = filtered(&rotated, 5).norm() / filtered(&rotated, 0).norm();
        assert!((echo - 0.5).abs() < 0.08, "{echo}");
    }

    /// **The radio's floor, by hand**, from a recording (`SDRTOP_REPLAY=
    /// path.sigmf-data`, ci8 at 20 Msps): its shape over the 52 tones,
    /// relative to the centre, as a bar a tone. `cargo test --release
    /// the_radios_floor -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn the_radios_floor() {
        let Ok(path) = std::env::var("SDRTOP_REPLAY") else {
            return;
        };
        let bytes = std::fs::read(&path).unwrap();
        let n = (bytes.len() / 2).min(20_000_000 * 4);
        let x: Vec<Complex<f32>> = bytes[..2 * n]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| Complex::new(p[0] as i8 as f32 / 128.0, p[1] as i8 as f32 / 128.0))
            .collect();
        let floor = Floor::measure(&x).expect("quiet stretches");
        for (i, &k) in TONES.iter().enumerate() {
            let db = 10.0 * floor.shape[i].log10();
            let bar = "#".repeat(((db + 1.0).max(0.0) * 10.0) as usize);
            eprintln!(
                "{k:+3}  {db:+5.2} dB +- {:.2}  {bar}",
                10.0 * (1.0 + floor.error[i] / floor.shape[i]).log10()
            );
        }
    }

    /// The noise variance read from the two long symbols is the noise put
    /// in, on average over many frames, at 10 dB.
    #[test]
    fn the_noise_is_read_without_bias() {
        let taps = [Complex::new(1.0, 0.0)];
        let mut ratio = Vec::new();
        for seed in 0..40 {
            let (x, scale) = on_air(&taps, 10.0, 0.0, 100 + seed);
            let e = estimated(&x, &Floor::flat());
            // Noise of power 0.1 a sample is 6.4 a bin through the 64-point
            // transform, and half that once the two symbols are averaged.
            let _ = scale;
            let expected = 0.1 * 64.0 / 2.0;
            ratio.push(e.variance[10] / expected);
        }
        let mean = ratio.iter().sum::<f64>() / ratio.len() as f64;
        assert!((0.8..1.2).contains(&mean), "{mean}");
    }

    /// A floor shaped by the radio (+2 dB at the edges here) is measured
    /// back from quiet stretches, within its stated error.
    #[test]
    fn a_shaped_floor_is_recovered() {
        let mut rng = Rng::new(7);
        // White noise through a filter that lifts the edge subcarriers: a
        // 64-point frequency-domain shaping of each 64-sample block.
        let lift = |k: i32| if k.abs() >= 20 { 10f64.powf(0.2) } else { 1.0 };
        let mut x = Vec::new();
        for _ in 0..4000 {
            let mut b: Vec<Complex<f64>> = rng
                .noise(64, 1.0)
                .iter()
                .map(|z| Complex::new(z.re as f64, z.im as f64))
                .collect();
            rustfft::FftPlanner::new()
                .plan_fft_forward(64)
                .process(&mut b);
            for (i, v) in b.iter_mut().enumerate() {
                let k = if i < 32 { i as i32 } else { i as i32 - 64 };
                *v *= lift(k).sqrt();
            }
            rustfft::FftPlanner::new()
                .plan_fft_inverse(64)
                .process(&mut b);
            x.extend(
                b.iter()
                    .map(|z| Complex::new((z.re / 64.0) as f32, (z.im / 64.0) as f32)),
            );
        }
        let floor = Floor::measure(&x).expect("quiet stretches");
        for (i, &k) in TONES.iter().enumerate() {
            let want = lift(k);
            let (got, se) = (floor.shape[i], floor.error[i]);
            assert!(
                (got - want).abs() < 4.0 * se + 0.02,
                "{k}: {got} against {want} +- {se}"
            );
        }
    }

    /// Under that shaped floor a flat channel reads flat, each tone within
    /// its own stated uncertainty, and the edge tones say they are the less
    /// certain ones.
    #[test]
    fn a_flat_channel_reads_flat_within_each_tones_uncertainty() {
        let mut shape = vec![1.0; 52];
        for (i, &k) in TONES.iter().enumerate() {
            if k.abs() >= 20 {
                shape[i] = 10f64.powf(0.2);
            }
        }
        let floor = Floor {
            shape,
            error: vec![0.0; 52],
        };
        let taps = [Complex::new(1.0, 0.0)];
        let mut outside = 0;
        let mut total = 0;
        for seed in 0..20 {
            let (x, scale) = on_air(&taps, 15.0, 0.0, 300 + seed);
            let e = estimated(&x, &floor);
            for (i, h) in e.h.iter().enumerate() {
                let sigma = e.variance[i].sqrt();
                total += 1;
                if (h - Complex::new(scale, 0.0)).norm() > 3.0 * sigma {
                    outside += 1;
                }
            }
            let edge = TONES.iter().position(|&k| k == 24).unwrap();
            let middle = TONES.iter().position(|&k| k == 4).unwrap();
            assert!(e.variance[edge] > e.variance[middle] * 1.5);
        }
        // A complex Gaussian beyond 3 sigma: about 1 in 8000 per tone.
        assert!(
            outside * 100 <= total,
            "{outside} of {total} beyond 3 sigma"
        );
    }
}
