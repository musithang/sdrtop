// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Symbol timing recovery and bit slicing for LE 1M: turning a discriminator's
//! instantaneous-frequency output into bits, once [`super::detect`] has
//! already said a packet starts near here.
//!
//! Uses `dsp::timing::find_phase`, not `dsp::timing::recover` (Gardner).
//! Design section 1.1 offers both and says to use whichever the test vectors
//! say is enough; building this module's own test vectors found Gardner's
//! detector converging to a phase measurably biased from the true one on a
//! Gaussian-filtered GFSK discriminator's output - a real S-curve bias from
//! feeding it a pulse shape its derivation does not assume, not a bug, and
//! `dsp::timing`'s own module doc records the measurement. A BLE advertising
//! packet is a short burst with a static phase, not a stream whose timing
//! drifts, which is exactly what a search-once-and-hold method fits, so that
//! is what this module uses.

use crate::signal::dsp::timing::{find_phase, interpolate};

/// How many candidate phases [`slice`] tries per symbol period. 16 puts the
/// worst-case phase error at 1/32 of a symbol - a sixteenth either side of
/// the best candidate tried - comfortably finer than the bias
/// `dsp::timing`'s own tests measured Gardner settling to on this signal.
const PHASE_RESOLUTION: usize = 16;

/// Recover one bit per symbol from a discriminator's instantaneous-frequency
/// output, finding the symbol phase once over the whole span given and
/// slicing every symbol against `threshold`.
///
/// A bit is `true` when the recovered sample is above `threshold` - `0.0`
/// for a caller with nothing else to go on, matching the convention
/// [`super::gfsk::modulate`] uses to map a `true` bit to positive deviation.
/// `signal::ble::receive::Receiver` passes its own capture's mean instead:
/// real hardware measured a real device's crystal offset (or this radio's
/// own LO leakage) sitting exactly at the tuned centre this arc never mixes
/// off, which shifts every discriminator sample by a constant a fixed zero
/// would slice against wrongly. See `dsp::uncertainty::mean_with_uncertainty`
/// for how that same mean becomes B7's reported frequency offset.
///
/// **Returns the raw sample at each symbol alongside the bit it became.**
/// The threshold decision throws away exactly the number
/// `signal::ble::measure::modulation_quality` needs - how far the
/// discriminator actually swung, not just which side of the line it landed
/// on - so this hands both back rather than making a second caller re-run
/// [`find_phase`] and [`interpolate`] to recover what this call already
/// computed.
pub fn slice(
    discriminator: &[f32],
    sps: f64,
    symbols: usize,
    threshold: f32,
) -> (Vec<bool>, Vec<f32>) {
    let phase = find_phase(discriminator, sps, symbols, PHASE_RESOLUTION);
    let raw: Vec<f32> = (0..symbols)
        .map(|k| interpolate(discriminator, phase + k as f64 * sps))
        .collect();
    let bits = raw.iter().map(|&s| s > threshold).collect();
    (bits, raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::ble::detect::Le1mParams;
    use crate::signal::ble::gfsk::modulate;
    use crate::signal::dsp::discriminate::discriminate;
    use crate::signal::dsp::testkit::{at_snr, Rng};

    /// The ideal noncoherent binary orthogonal FSK bit error rate.
    ///
    /// Source: J.G. Proakis, "Digital Communications", the standard result
    /// for noncoherent detection of binary orthogonal signals,
    /// `Pb = (1/2) exp(-Eb / (2 N0))`. This is a bound on the best any
    /// receiver for this signal class can do, not a prediction of what this
    /// one measures - GFSK's Gaussian filtering is not the rectangular pulse
    /// the bound assumes, and slicing a single discriminator sample per
    /// symbol is not the optimal (matched, integrate-and-dump) receiver the
    /// bound is for. `the_measured_ber_never_beats_the_ideal_bound` is the
    /// half of that gap this module can assert without a second citation for
    /// exactly how large the other half is; the margin in
    /// `bit_error_rate_tracks_the_ideal_curve_within_a_measured_margin` is
    /// measured, not looked up.
    fn ideal_noncoherent_fsk_ber(eb_n0_db: f64) -> f64 {
        let eb_n0 = 10f64.powf(eb_n0_db / 10.0);
        0.5 * (-eb_n0 / 2.0).exp()
    }

    /// `dsp::testkit::at_snr`'s SNR is per complex sample, over the whole
    /// sampled bandwidth; `Eb/N0` is energy per bit over noise spectral
    /// density. Oversampling by `sps` samples per bit spreads the same total
    /// noise power over `sps` times as many samples, which is exactly the
    /// `sps` factor between the two: `Eb/N0 = SNR * sps` linearly, derived
    /// from `Eb = S / R_b` and `N0 = N / F_s` with `F_s = sps * R_b`.
    fn snr_db_for_eb_n0_db(eb_n0_db: f64, sps: usize) -> f64 {
        eb_n0_db - 10.0 * (sps as f64).log10()
    }

    /// B4's exit condition: bit error rate against a generated packet
    /// matches the theoretical GFSK curve within a stated margin, over a
    /// spread of SNRs wide enough to see the curve's own shape.
    ///
    /// **The margin is wide, and the reason is worth recording rather than
    /// hiding.** [`ideal_noncoherent_fsk_ber`] is the bound for a receiver
    /// that integrates a full symbol's energy (a matched filter or
    /// integrate-and-dump). This module's [`slice`] does neither - it reads
    /// one instantaneous discriminator sample per symbol, out of `sps`
    /// available, and throws the rest away. Measured on this signal, that
    /// costs on the order of 12-15 dB against the ideal bound, not the couple
    /// of dB a shaping-filter penalty alone would - a real, substantial
    /// implementation loss from the deliberately simple detector, and not a
    /// number this arc can cite a receiver-specific formula for. What is
    /// still true, and still worth asserting precisely: the curve's shape
    /// (bit errors fall as Eb/N0 rises), the physical floor (never better
    /// than the ideal bound), and that it actually gets good at high enough
    /// Eb/N0 rather than plateauing at some broken floor.
    #[test]
    fn bit_error_rate_tracks_the_ideal_curve_within_a_measured_margin() {
        let params = Le1mParams::at(4);
        let n_bits = 60_000;
        let mut rng = Rng::new(1);
        let bits: Vec<bool> = (0..n_bits).map(|_| rng.next_u64() & 1 == 1).collect();
        let clean = modulate(
            &bits,
            params.sps,
            params.deviation_hz,
            params.sample_rate,
            params.bt,
        );

        let guard = 5;
        let mut previous = f64::INFINITY;
        for eb_n0_db in [6.0, 12.0, 18.0, 24.0] {
            let snr_db = snr_db_for_eb_n0_db(eb_n0_db, params.sps);
            let noisy = at_snr(&clean, snr_db, &mut Rng::new(2));
            let mut inst = Vec::new();
            discriminate(&noisy, params.sample_rate, &mut inst);

            let (recovered, _) = slice(&inst, params.sps as f64, n_bits - guard, 0.0);

            let errors = recovered
                .iter()
                .zip(bits.iter())
                .filter(|(a, b)| a != b)
                .count();
            let measured = errors as f64 / recovered.len() as f64;
            let ideal = ideal_noncoherent_fsk_ber(eb_n0_db);

            assert!(
                measured >= ideal * 0.5,
                "eb/n0={eb_n0_db} dB: measured BER {measured:e} is implausibly below the ideal bound {ideal:e}"
            );
            // Twenty dB of implementation loss as the ceiling: generous
            // against the roughly 12-15 dB actually measured, wide enough to
            // absorb run-to-run statistical noise, still tight enough that a
            // detector reading pure noise (BER near 0.5 at every point)
            // would fail it at the higher Eb/N0 values below.
            let degraded = ideal_noncoherent_fsk_ber(eb_n0_db - 20.0);
            assert!(
                measured <= degraded,
                "eb/n0={eb_n0_db} dB: measured BER {measured:e} exceeds the ideal curve degraded by 20 dB ({degraded:e})"
            );
            assert!(
                measured <= previous + 1e-9,
                "eb/n0={eb_n0_db} dB: BER {measured:e} rose above the previous point's {previous:e} - Eb/N0 increasing must not make bits worse"
            );
            previous = measured;
        }
        assert!(
            previous < 0.01,
            "at the highest Eb/N0 tested the receiver should be working well: BER was {previous:e}"
        );
    }

    /// The physical floor: no receiver for this signal beats the ideal
    /// noncoherent bound, so a measurement under it would mean the test's
    /// own accounting is wrong, not that the receiver is unusually good.
    #[test]
    fn the_measured_ber_never_beats_the_ideal_bound() {
        let params = Le1mParams::at(4);
        let n_bits = 20_000;
        let mut rng = Rng::new(3);
        let bits: Vec<bool> = (0..n_bits).map(|_| rng.next_u64() & 1 == 1).collect();
        let clean = modulate(
            &bits,
            params.sps,
            params.deviation_hz,
            params.sample_rate,
            params.bt,
        );
        let eb_n0_db = 3.0;
        let snr_db = snr_db_for_eb_n0_db(eb_n0_db, params.sps);
        let noisy = at_snr(&clean, snr_db, &mut Rng::new(4));
        let mut inst = Vec::new();
        discriminate(&noisy, params.sample_rate, &mut inst);
        let (recovered, _) = slice(&inst, params.sps as f64, n_bits - 5, 0.0);

        let errors = recovered
            .iter()
            .zip(bits.iter())
            .filter(|(a, b)| a != b)
            .count();
        let measured = errors as f64 / recovered.len() as f64;
        let ideal = ideal_noncoherent_fsk_ber(eb_n0_db);
        assert!(
            measured >= ideal * 0.5,
            "measured BER {measured:e} is implausibly below the ideal bound {ideal:e} at {eb_n0_db} dB"
        );
    }

    /// A noiseless packet slices back to exactly its own bits - the floor
    /// this module has to clear before any BER-versus-noise claim means
    /// anything, and cheap enough to keep as its own test.
    #[test]
    fn a_noiseless_packet_slices_to_exactly_its_own_bits() {
        let params = Le1mParams::at(4);
        let mut rng = Rng::new(5);
        let bits: Vec<bool> = (0..2000).map(|_| rng.next_u64() & 1 == 1).collect();
        let clean = modulate(
            &bits,
            params.sps,
            params.deviation_hz,
            params.sample_rate,
            params.bt,
        );
        let mut inst = Vec::new();
        discriminate(&clean, params.sample_rate, &mut inst);
        let (recovered, _) = slice(&inst, params.sps as f64, bits.len() - 5, 0.0);
        assert_eq!(&recovered[..], &bits[..recovered.len()]);
    }
}
