// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Deterministic test signals, shared by every step in this layer.
//!
//! Compiled only under `cfg(test)`. It is here rather than inside one module's
//! test block because many modules need noise at a stated signal-to-noise
//! ratio, and two modules that each roll their own definition of "0 dB SNR"
//! will disagree about what they measured while both looking right.
//!
//! **Deterministic on purpose.** A test that fails one run in fifty is a test
//! nobody trusts and everybody re-runs. Every generator here takes a seed, so a
//! failure is reproducible from the seed alone.

use num_complex::Complex;
use std::f64::consts::TAU;

/// SplitMix64. Steele, Lea and Flood, "Fast Splittable Pseudorandom Number
/// Generators", OOPSLA 2014; the constants are Vigna's public-domain reference
/// implementation.
///
/// Chosen because it is eight lines, passes the usual test batteries, and has no
/// state to seed badly: any 64-bit seed is as good as any other.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform on `(0, 1]`. Open at zero because the logarithm in Box-Muller
    /// would otherwise be handed a zero once every few billion draws, which is
    /// exactly the kind of failure that appears the week after a release.
    pub fn unit(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64 + 1.0) / 9_007_199_254_740_993.0
    }

    /// A pair of independent standard normals, by the Box-Muller transform.
    pub fn normal_pair(&mut self) -> (f64, f64) {
        let r = (-2.0 * self.unit().ln()).sqrt();
        let theta = TAU * self.unit();
        (r * theta.cos(), r * theta.sin())
    }

    /// Circularly symmetric complex Gaussian noise of the given total power,
    /// meaning `E[|z|^2] = power`, split evenly between the two components.
    pub fn noise(&mut self, n: usize, power: f64) -> Vec<Complex<f32>> {
        let s = (power / 2.0).sqrt();
        (0..n)
            .map(|_| {
                let (a, b) = self.normal_pair();
                Complex::new((s * a) as f32, (s * b) as f32)
            })
            .collect()
    }

    /// A unit-magnitude QPSK sequence: a stand-in for a preamble, with the flat
    /// spectrum and the sharp autocorrelation a real one is chosen to have.
    pub fn qpsk(&mut self, n: usize) -> Vec<Complex<f32>> {
        const R: f32 = std::f32::consts::FRAC_1_SQRT_2;
        (0..n)
            .map(|_| {
                let b = self.next_u64();
                let re = if b & 1 == 0 { R } else { -R };
                let im = if b & 2 == 0 { R } else { -R };
                Complex::new(re, im)
            })
            .collect()
    }
}

/// Mean power, `E[|x|^2]`, accumulated in `f64`.
pub fn power(x: &[Complex<f32>]) -> f64 {
    if x.is_empty() {
        return 0.0;
    }
    x.iter()
        .map(|s| (s.re as f64).powi(2) + (s.im as f64).powi(2))
        .sum::<f64>()
        / x.len() as f64
}

/// Add noise to reach a stated signal-to-noise ratio.
///
/// **The definition, spelled out because every SNR argument is really an
/// argument about the definition:** the ratio is of the mean power of the signal
/// passed in to the mean power of the noise added, both over the whole array and
/// over the whole band. Not per symbol, not per bit, not in a measurement
/// bandwidth narrower than the sample rate. A caller who wants Eb/N0 converts.
pub fn at_snr(signal: &[Complex<f32>], snr_db: f64, rng: &mut Rng) -> Vec<Complex<f32>> {
    let noise_power = power(signal) / 10f64.powf(snr_db / 10.0);
    let n = rng.noise(signal.len(), noise_power);
    signal.iter().zip(n.iter()).map(|(s, z)| s + z).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The generator is a safeguard for every later step, so it answers to its
    /// own statistics before anything is allowed to lean on it.
    #[test]
    fn the_noise_has_the_power_it_was_asked_for() {
        let mut rng = Rng::new(1);
        for want in [1.0, 0.01, 100.0] {
            let z = rng.noise(200_000, want);
            let got = power(&z);
            assert!(
                (got / want - 1.0).abs() < 0.02,
                "asked for power {want}, measured {got}"
            );
        }
    }

    #[test]
    fn the_noise_is_circular_and_centred() {
        let mut rng = Rng::new(2);
        let z = rng.noise(200_000, 1.0);
        let n = z.len() as f64;
        let mean_re = z.iter().map(|s| s.re as f64).sum::<f64>() / n;
        let mean_im = z.iter().map(|s| s.im as f64).sum::<f64>() / n;
        assert!(mean_re.abs() < 0.01 && mean_im.abs() < 0.01, "not centred");
        // Circular symmetry: the two components carry equal power and are
        // uncorrelated. A Box-Muller transform with a reused uniform would fail
        // the second of these while passing everything else.
        let pr = z.iter().map(|s| (s.re as f64).powi(2)).sum::<f64>() / n;
        let pi = z.iter().map(|s| (s.im as f64).powi(2)).sum::<f64>() / n;
        assert!(
            (pr / pi - 1.0).abs() < 0.03,
            "unequal components: {pr} {pi}"
        );
        let cross = z.iter().map(|s| s.re as f64 * s.im as f64).sum::<f64>() / n;
        assert!(cross.abs() < 0.01, "components are correlated: {cross}");
    }

    #[test]
    fn a_stated_snr_is_the_ratio_of_the_two_powers() {
        let mut rng = Rng::new(3);
        let s = rng.qpsk(100_000);
        for snr in [-10.0, 0.0, 20.0] {
            let mut r2 = Rng::new(4);
            let y = at_snr(&s, snr, &mut r2);
            let noise: Vec<_> = y.iter().zip(s.iter()).map(|(a, b)| a - b).collect();
            let got = 10.0 * (power(&s) / power(&noise)).log10();
            assert!((got - snr).abs() < 0.1, "asked {snr} dB, measured {got} dB");
        }
    }

    #[test]
    fn the_same_seed_gives_the_same_signal() {
        let a = Rng::new(7).noise(1000, 1.0);
        let b = Rng::new(7).noise(1000, 1.0);
        assert_eq!(a, b);
        assert_ne!(a, Rng::new(8).noise(1000, 1.0));
    }
}
