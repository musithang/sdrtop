// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! A synthetic GFSK transmitter, for testing this arc against a signal whose
//! bits are known - the role [`crate::signal::dsp::testkit`] plays for the
//! shared layer, and the reason this exists separately from it: that module
//! knows no protocol, and a GFSK modulator with a modulation index and a
//! Gaussian filter baked in is BLE-specific policy, not a shared primitive.
//!
//! Compiled only under `cfg(test)`, and reused from B2 onward: every later
//! step that checks a bit error rate, a timing recovery, or a decode against
//! a known signal starts here rather than re-deriving its own.

use num_complex::Complex;

use crate::signal::dsp::fir::gaussian_taps;

/// How many symbol periods the shaping filter spans. 4 is the common choice
/// in the references [`gaussian_taps`] cites; nothing in this file depends on
/// the exact figure, only on the filter it produces being the one B2's own
/// tests measure.
const FILTER_SPAN_SYMBOLS: usize = 4;

/// A GFSK-modulated baseband signal for the given bits, at `sps` samples per
/// symbol.
///
/// `deviation_hz` is the peak frequency deviation - 250 kHz for BLE LE 1M's
/// nominal modulation index of 0.5 at a 1 Mb/s symbol rate, since `h = 2 *
/// deviation / symbol_rate`. Design section 2.1's measurement 1 is this same
/// `h`, measured the other way around from a captured signal. `bt` is the
/// Gaussian filter's bandwidth-time product, 0.5 for BLE.
///
/// NRZ maps each bit to ±1, held for `sps` samples, Gaussian-filtered to
/// shape the transitions, then frequency-modulated: the textbook GFSK
/// construction, and the reason [`gaussian_taps`]'s own unit-sum
/// normalisation matters here - an unnormalised kernel would scale the
/// deviation along with it.
pub fn modulate(
    bits: &[bool],
    sps: usize,
    deviation_hz: f64,
    sample_rate: f64,
    bt: f64,
) -> Vec<Complex<f32>> {
    let n = bits.len() * sps;
    let mut nrz = vec![0.0f32; n];
    for (i, &b) in bits.iter().enumerate() {
        let v = if b { 1.0 } else { -1.0 };
        for s in nrz[i * sps..(i + 1) * sps].iter_mut() {
            *s = v;
        }
    }

    let taps = gaussian_taps(bt, sps, FILTER_SPAN_SYMBOLS);
    let half = (taps.len() as isize - 1) / 2;
    let filtered: Vec<f64> = (0..n as isize)
        .map(|i| {
            taps.iter()
                .enumerate()
                .map(|(k, &h)| {
                    let j = i - half + k as isize;
                    let x = if j >= 0 && (j as usize) < n {
                        nrz[j as usize] as f64
                    } else {
                        0.0
                    };
                    h as f64 * x
                })
                .sum::<f64>()
        })
        .collect();

    let mut phase = 0.0f64;
    filtered
        .iter()
        .map(|&f| {
            phase += std::f64::consts::TAU * deviation_hz * f / sample_rate;
            Complex::new(phase.cos() as f32, phase.sin() as f32)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::dsp::discriminate::discriminate;
    use crate::signal::dsp::testkit::{at_snr, Rng};

    const SPS: usize = 4;
    const SYMBOL_RATE: f64 = 1_000_000.0;
    const SAMPLE_RATE: f64 = SYMBOL_RATE * SPS as f64;
    const DEVIATION_HZ: f64 = 250_000.0;
    const BT: f64 = 0.5;

    /// Slice the discriminator output at each symbol's own centre. No timing
    /// recovery yet - that is B4's job - so this leans on the fact that the
    /// synthetic transmitter and this reader agree exactly on where a symbol
    /// starts.
    fn recover_bits(iq: &[Complex<f32>], n_bits: usize) -> Vec<bool> {
        let mut inst = Vec::new();
        discriminate(iq, SAMPLE_RATE, &mut inst);
        (0..n_bits)
            .map(|k| {
                let centre = k * SPS + SPS / 2;
                let idx = centre.saturating_sub(1).min(inst.len() - 1);
                inst[idx] > 0.0
            })
            .collect()
    }

    /// B2's exit condition: a generated packet demodulates back to the bits
    /// that went in, at high SNR. Guarded at both ends because a finite
    /// kernel tapers the very first and last symbol towards zero deviation -
    /// zero-padding at the edge of the filter, not a defect in the round trip
    /// - and a symbol sitting in that taper is not what this test checks.
    #[test]
    fn a_generated_packet_demodulates_back_to_its_own_bits_at_high_snr() {
        let mut rng = Rng::new(1);
        let bits: Vec<bool> = (0..64).map(|_| rng.next_u64() & 1 == 1).collect();
        let iq = modulate(&bits, SPS, DEVIATION_HZ, SAMPLE_RATE, BT);
        let noisy = at_snr(&iq, 25.0, &mut rng);
        let recovered = recover_bits(&noisy, bits.len());
        for i in 2..bits.len() - 2 {
            assert_eq!(recovered[i], bits[i], "bit {i} did not round-trip");
        }
    }

    /// The same, with no noise at all - the cleanest possible statement of
    /// the round trip, kept separate so a failure here is never confused with
    /// an SNR margin problem.
    #[test]
    fn a_generated_packet_demodulates_back_to_its_own_bits_noiseless() {
        let mut rng = Rng::new(2);
        let bits: Vec<bool> = (0..64).map(|_| rng.next_u64() & 1 == 1).collect();
        let iq = modulate(&bits, SPS, DEVIATION_HZ, SAMPLE_RATE, BT);
        let recovered = recover_bits(&iq, bits.len());
        for i in 2..bits.len() - 2 {
            assert_eq!(recovered[i], bits[i], "bit {i} did not round-trip");
        }
    }

    /// The modulator deviates by about what was asked: on a run of
    /// same-valued bits, once the Gaussian filter has settled, the
    /// instantaneous frequency at the centre of the run should sit close to
    /// the full deviation rather than some other scale entirely.
    #[test]
    fn the_modulator_deviates_by_about_what_was_asked() {
        let bits = vec![true; 20];
        let iq = modulate(&bits, SPS, DEVIATION_HZ, SAMPLE_RATE, BT);
        let mut inst = Vec::new();
        discriminate(&iq, SAMPLE_RATE, &mut inst);
        let settled = inst[inst.len() / 2];
        assert!(
            (settled - DEVIATION_HZ as f32).abs() < DEVIATION_HZ as f32 * 0.1,
            "settled deviation {settled}, want about {DEVIATION_HZ}"
        );
    }

    /// Flip every bit and the deviation flips sign with it - the other half
    /// of "about what was asked", since a scale error alone would not catch a
    /// sign error in the phase integration.
    #[test]
    fn a_run_of_the_other_bit_deviates_the_other_way() {
        let bits = vec![false; 20];
        let iq = modulate(&bits, SPS, DEVIATION_HZ, SAMPLE_RATE, BT);
        let mut inst = Vec::new();
        discriminate(&iq, SAMPLE_RATE, &mut inst);
        let settled = inst[inst.len() / 2];
        assert!(
            (settled + DEVIATION_HZ as f32).abs() < DEVIATION_HZ as f32 * 0.1,
            "settled deviation {settled}, want about {}",
            -DEVIATION_HZ
        );
    }
}
