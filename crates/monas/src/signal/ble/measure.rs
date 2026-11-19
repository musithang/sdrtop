// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Transmitter modulation quality: modulation index, delta-f1 average,
//! delta-f2 average and the ratio of the averages, from a packet the receiver
//! already recovered rather than a dedicated test transmission.
//!
//! The four numbers are specified as read from a *known*
//! symbol pattern: `00001111` repeated for delta-f1, `10101010` repeated for
//! delta-f2. Bluetooth's own conformance test procedure gets that known
//! pattern by putting the device under test into Direct Test Mode and
//! sending it a specific payload, whitening disabled. This receiver has no
//! way to do either - it is a passive listener on ordinary advertising
//! traffic from devices it does not control, and rule 8 (`POLICY.md`) rules
//! out ever transmitting to ask for one.
//!
//! **What makes those patterns' bits the right ones is their neighbours,
//! and ordinary traffic has plenty of them.** The suites read bits 2, 3, 6
//! and 7 of `00001111`, each with the same bit either side, and every bit of
//! `10101010`, each with the opposite bit either side. A GFSK symbol moves
//! the frequency two bits away by about 1e-8 of the deviation, so every
//! such bit in whitened traffic is read exactly as the suites read theirs:
//! `signal::dsp::deviation::suite_readings` takes them, and
//! `signal::net::conformance` holds its figures to the suites' own on their
//! own patterns. This module turns those readings into the four numbers.
//!
//! **Measured against the physically transmitted bits, not the decoded
//! ones.** Whitening is a logical operation applied before modulation and
//! undone after slicing; the Gaussian filter and the discriminator only
//! ever see the *whitened* symbol sequence, so what qualifies a bit is its
//! on-air neighbours, not its data neighbours. The receivers pass the air
//! bits (`pdu::Packet::air`).
//!
//! [`drift_from`] needs none of the pattern-search
//! machinery above - a frequency drift within a packet is a property of the
//! per-symbol discriminator readings themselves, not of any particular bit
//! pattern - but it reads one value per symbol, the reading at each bit's
//! centre (`dsp::carrier::by_bit_from`): the raw, oversampled trace turned
//! out to be the wrong input for it.

use super::Phy;
use crate::signal::dsp::uncertainty::Uncertain;

// The symbol rate both measurements scale by is the PHY's own
// (`Phy::symbol_rate_hz`: 1 Mb/s on LE 1M, 2 Mb/s on LE 2M), a specification
// fact, never derived from a receiver's own `sps`, which is a demodulator
// design choice, so LE 2M is measured as fully as LE 1M.

// The run shapes are GFSK's, not BLE's: `signal::dsp::deviation` holds
// them since classic Bluetooth reads the same.

/// RFPHY/TRM/BV-13-C's floor for Δf1max on LE Coded (S=8): 99.9 % of them
/// above 185 kHz.
pub const CODED_DELTA_F1_MAX_LIMIT_HZ: f64 = 185_000.0;

/// An LE Coded (S=8) packet's modulation as RFPHY/TRM/BV-13-C reads it:
/// Δf1 alone, because S=8 sends only `0011` and `1100` and so never the
/// alternating symbols Δf2 is read from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CodedModulation {
    /// The mean Δf1max over the packet's settled symbols, and its standard
    /// error. Pass: 225 to 275 kHz.
    pub delta_f1_avg_hz: Uncertain,
    /// The share of Δf1max readings above [`CODED_DELTA_F1_MAX_LIMIT_HZ`];
    /// the suite asks for 99.9 % over ten packets.
    pub share_f1max_above_limit: f64,
}

/// An LE Coded (S=8) packet's initial carrier as RFPHY/TRM/BV-14-C reads
/// it: the frequency integrated over four 16-symbol groups of the preamble
/// from its third symbol, f0 to f3, in Hz from the channel's nominal centre.
/// f0 is the initial carrier; f3 against f0 is the drift across 48 us
/// (pass: within 19.2 kHz).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CodedInitial {
    pub groups_hz: [f64; 4],
}

/// One packet's modulation quality, each figure carrying the uncertainty a
/// caller needs to judge it against a stated limit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModulationQuality {
    /// The average deviation of every on-air bit whose neighbours both
    /// equal it, each the mean over the bit (the suites' delta-f1), in Hz.
    pub delta_f1_avg_hz: Uncertain,
    /// The average deviation of every on-air bit whose neighbours both
    /// differ from it, each read at the bit's centre where an alternation
    /// peaks (the suites' delta-f2), in Hz, held
    /// against the floor Core 5.4 Vol 6 Part A 3.1 puts on the *minimum*
    /// deviation ("shall never be less than 185 kHz" at 1 Msym/s).
    ///
    /// **Why the average, when the floor is on the minimum.** Two other
    /// readings were tried on live traffic and both measured the noise
    /// instead of the transmitter. The largest reading could never fail:
    /// noise only pushed it further above the floor (668 kHz from a 7 dB
    /// packet). The smallest nearly always did: the least of twenty noisy
    /// readings sits about two of their sigma below their mean, so healthy
    /// transmitters read 150 kHz against a floor of 185. A GFSK transmitter's
    /// alternating peaks are all the same peak, shaped by the same filter,
    /// so its minimum is its average, and the average is what a packet can
    /// measure with a real uncertainty. What that costs is stated where it
    /// is shown: an average under the floor is a transmitter under it, and
    /// one over it could still dip on a run the average hides.
    pub delta_f2_avg_hz: Uncertain,
    /// `2 * delta_f1_avg_hz / symbol rate` - the modulation index the
    /// specification states a band for, derived from delta-f1 because
    /// that is the settled, filter-independent deviation a device's own
    /// deviation setting actually controls.
    pub modulation_index: Uncertain,
    /// The average peak deviation over alternating runs, over the same
    /// figure for settled runs: the specification's way of asking whether
    /// the Gaussian filter is right. A ratio well below one means the filter is narrower than it
    /// should be, closing the eye during continuous alternation more than
    /// the specification allows.
    pub ratio: Uncertain,
}

/// Modulation quality from one packet's delta-f1 and delta-f2 readings
/// (`signal::dsp::deviation::suite_readings`), in Hz.
///
/// `None` when either kind never occurred - a packet too short, or one
/// whose particular content happened to lack a qualifying bit of either
/// kind. Rule 2: a measurement with nothing behind it is refused, not
/// invented from zero occurrences.
pub fn modulation_from(
    settled: &[f32],
    alternating: &[f32],
    phy: Phy,
) -> Option<ModulationQuality> {
    if settled.is_empty() || alternating.is_empty() {
        return None;
    }

    let delta_f1_avg_hz = crate::signal::dsp::uncertainty::mean_with_uncertainty(settled);
    let delta_f2_avg_hz = crate::signal::dsp::uncertainty::mean_with_uncertainty(alternating);
    let modulation_index = delta_f1_avg_hz.scale(2.0 / phy.symbol_rate_hz());
    let ratio = delta_f2_avg_hz.ratio(&delta_f1_avg_hz);

    Some(ModulationQuality {
        delta_f1_avg_hz,
        delta_f2_avg_hz,
        modulation_index,
        ratio,
    })
}

/// The carrier figures, which are GFSK's and so `dsp::carrier`'s: classic
/// Bluetooth reads them too.
pub use crate::signal::dsp::carrier::Drift;

/// [`crate::signal::dsp::carrier::drift_from`] on `phy`'s clock.
pub fn drift_from(initial: Option<(f64, usize)>, blocks: &[f64], phy: Phy) -> Option<Drift> {
    crate::signal::dsp::carrier::drift_from(initial, blocks, phy.symbol_rate_hz())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::ble::detect::Le1mParams;
    use crate::signal::ble::gfsk::modulate;
    use crate::signal::dsp::discriminate::discriminate;
    use crate::signal::dsp::testkit::{at_snr, Rng};

    /// The suites' readings of per-symbol `samples`, each held across its
    /// bit, aggregated: what the receivers do with a rebuilt waveform, on
    /// the one reading a bit these tests build.
    fn modulation_quality(bits: &[bool], samples: &[f32], phy: Phy) -> Option<ModulationQuality> {
        let (settled, alternating) = crate::signal::dsp::deviation::suite_readings(bits, |x| {
            samples[(x as usize).min(samples.len() - 1)]
        })?;
        modulation_from(&settled, &alternating, phy)
    }

    /// A random on-air bit stream and the discriminator samples recovered
    /// from it at `deviation_hz`, sliced against zero - the same shape
    /// `receive.rs` hands this module, built without any of that module's
    /// own detection or timing-search machinery, which this measurement
    /// does not touch.
    fn symbols_and_samples(
        deviation_hz: f64,
        n_bits: usize,
        snr_db: f64,
        seed: u64,
    ) -> (Vec<bool>, Vec<f32>) {
        let params = Le1mParams {
            sps: 4,
            sample_rate: 4_000_000.0,
            deviation_hz,
            bt: 0.5,
        };
        let mut rng = Rng::new(seed);
        let bits: Vec<bool> = (0..n_bits).map(|_| rng.next_u64() & 1 == 1).collect();
        let clean = modulate(
            &bits,
            params.sps,
            deviation_hz,
            params.sample_rate,
            params.bt,
        );
        let iq = if snr_db.is_finite() {
            at_snr(&clean, snr_db, &mut Rng::new(seed + 1))
        } else {
            clean
        };
        let mut inst = Vec::new();
        discriminate(&iq, params.sample_rate, &mut inst);
        let (_, samples) = crate::signal::ble::sync::slice(&inst, params.sps as f64, n_bits, 0.0);
        (bits, samples)
    }

    /// The "measured as wrong" half: a deviation
    /// clearly outside the modulation index band measures outside it, and
    /// the measurement moves the *right way* as deviation rises - up for
    /// more, down for less - rather than only "close to the deviation
    /// asked for".
    ///
    /// **Not compared against `2 * deviation_hz / symbol rate` to a
    /// tight tolerance, and that is a finding of its own kind.** A first
    /// version of this test did exactly that, at three deviations, and
    /// failed at 200 kHz while passing at 250 kHz - not because the
    /// measurement is wrong, but because [`find_phase`](crate::signal::dsp::timing::find_phase)'s
    /// amplitude-based search has a broad, nearly flat maximum over a
    /// settled plateau, so two *independent* simulated captures at
    /// different deviations can legitimately settle on phases a fraction of
    /// a sample apart - benign in itself, but enough to move which exact
    /// sample a transition-region symbol reads, which this generalised
    /// measurement (any bit its neighbours qualify, not only a repeated
    /// specification octet) is more exposed to than a single hand-picked
    /// symbol would be. Comparing two independently-simulated captures to
    /// each other, rather than either to a fixed theoretical fraction, is
    /// not sensitive to that phase choice: both readings come from the same
    /// noiseless family and the underlying settling fraction is a property
    /// of the bit sequence and the filter, not of which capture found it.
    #[test]
    fn a_deliberately_wrong_deviation_measures_outside_the_band_and_the_right_way() {
        let readings: Vec<(f64, f64)> = [150_000.0, 250_000.0, 350_000.0]
            .into_iter()
            .map(|deviation_hz| {
                let (bits, samples) = symbols_and_samples(deviation_hz, 4000, f64::INFINITY, 10);
                let q = modulation_quality(&bits, &samples, Phy::OneM)
                    .expect("plenty of settled runs at 4000 bits");
                (deviation_hz, q.modulation_index.value())
            })
            .collect();

        // Monotonic: more deviation must never measure as less.
        for pair in readings.windows(2) {
            assert!(
                pair[1].1 > pair[0].1,
                "deviation {} measured {}, not more than deviation {}'s {}",
                pair[1].0,
                pair[1].1,
                pair[0].0,
                pair[0].1
            );
        }

        // 250 kHz is BLE's own nominal deviation and must land inside the band;
        // 150 kHz and 350 kHz are both far enough outside its 0.45 to 0.55 that
        // no plausible settling loss brings them back in - a genuinely wrong
        // transmitter, correctly read as one.
        let index = |d: f64| readings.iter().find(|r| r.0 == d).unwrap().1;
        assert!(
            index(250_000.0) > 0.45 && index(250_000.0) < 0.55,
            "nominal deviation measured index {}",
            index(250_000.0)
        );
        assert!(
            index(150_000.0) < 0.45,
            "150 kHz measured index {}, inside the band",
            index(150_000.0)
        );
        assert!(
            index(350_000.0) > 0.55,
            "350 kHz measured index {}, inside the band",
            index(350_000.0)
        );
    }

    /// The nominal BLE deviation lands inside the specification's own
    /// modulation index band, 0.45 to 0.55 - not asserted against the exact
    /// figure here (that belongs to the panel's stated `Limit`), just that
    /// this measurement's own number is the one a real conforming
    /// transmitter would be judged against.
    #[test]
    fn the_nominal_deviation_measures_inside_the_modulation_index_band() {
        let (bits, samples) = symbols_and_samples(250_000.0, 4000, 25.0, 11);
        let q = modulation_quality(&bits, &samples, Phy::OneM).unwrap();
        assert!(
            q.modulation_index.value() > 0.45 && q.modulation_index.value() < 0.55,
            "measured index {}",
            q.modulation_index.value()
        );
    }

    /// Continuous alternation gives the Gaussian filter the least time to
    /// settle, so delta-f2 must never exceed delta-f1 on the same signal -
    /// the physical relationship the ratio is built to expose a filter
    /// that violates.
    #[test]
    fn alternation_never_reaches_further_than_a_settled_run_does() {
        let (bits, samples) = symbols_and_samples(250_000.0, 4000, f64::INFINITY, 12);
        let q = modulation_quality(&bits, &samples, Phy::OneM).unwrap();
        assert!(
            q.ratio.value() <= 1.01,
            "ratio {} implies alternation reached further than settling does",
            q.ratio.value()
        );
        assert!(q.delta_f2_avg_hz.value() <= q.delta_f1_avg_hz.value());
    }

    /// The alternating deviation can fall below the specification's floor,
    /// which the maximum it replaced never could: a transmitter at 150 kHz
    /// deviation reads under 185 kHz, a nominal one over it.
    ///
    /// **And noise is either inside its sigma or too loud to print.**
    /// Discriminator clicks push an average of peak readings up, as they
    /// pushed the maximum: measured on this construction, about 220 kHz
    /// clean, 260 to 320 at 8 dB. At 20 dB the reading stays within two
    /// sigma of its clean value; at 8 dB its sigma is past the 10 kHz the
    /// panel prints a delta-f reading at, so it shows as a dash rather than
    /// as the inflated figure.
    #[test]
    fn the_alternating_deviation_can_fail_its_floor_and_noise_alone_does_not() {
        const FLOOR_HZ: f64 = 185_000.0;
        const PRINTS_BELOW_HZ: f64 = 10_000.0;
        let df2 = |dev: f64, snr: f64| {
            let (bits, samples) = symbols_and_samples(dev, 300, snr, 21);
            modulation_quality(&bits, &samples, Phy::OneM)
                .unwrap()
                .delta_f2_avg_hz
        };
        let clean = df2(250_000.0, f64::INFINITY);
        assert!(clean.value() > FLOOR_HZ, "{clean:?}");
        assert!(df2(150_000.0, f64::INFINITY).value() < FLOOR_HZ);
        let fair = df2(250_000.0, 20.0);
        assert!(fair.sigma() < PRINTS_BELOW_HZ, "{fair:?}");
        assert!(
            (fair.value() - clean.value()).abs() < 2.0 * fair.sigma(),
            "noise moved a printed reading: {fair:?} against {clean:?}"
        );
        let loud = df2(250_000.0, 8.0);
        assert!(loud.sigma() > PRINTS_BELOW_HZ, "{loud:?}");
    }

    /// A run too short to contain either pattern refuses rather than
    /// inventing a measurement from nothing.
    #[test]
    fn too_short_a_run_refuses_rather_than_inventing_a_reading() {
        let bits = vec![true, false, true];
        let samples = vec![1.0f32, -1.0, 1.0];
        assert!(modulation_quality(&bits, &samples, Phy::OneM).is_none());
    }

    /// A packet with settled runs but no alternation - or the reverse -
    /// still refuses, because the ratio and the panel's own display need
    /// both figures, not just one.
    #[test]
    fn one_pattern_present_without_the_other_still_refuses() {
        let all_settled = vec![true; 20];
        let samples = vec![1.0f32; 20];
        assert!(modulation_quality(&all_settled, &samples, Phy::OneM).is_none());
    }

    /// The suites' drift of per-symbol `samples`, each held across its bit,
    /// with no preamble held: what the receivers do with a rebuilt
    /// waveform, on the one reading a bit these tests build.
    fn drift(bits: &[bool], samples: &[f32], phy: Phy) -> Option<Drift> {
        use crate::signal::dsp::carrier::{by_bit, ten_bit_blocks};
        let carrier = by_bit(bits, |x| samples[(x as usize).min(samples.len() - 1)]);
        drift_from(None, &ten_bit_blocks(&carrier, 1, bits.len() - 1), phy)
    }

    /// A clean IQ signal with an added linear frequency ramp on top of
    /// whatever it is already modulating: `drift_rate_hz_per_s * t` more
    /// frequency at time `t`, the same effect a thermally pulling crystal or
    /// a still-settling PLL has on a real transmitter within one burst.
    /// Multiplying by a chirp is exact here because frequency is additive
    /// under complex multiplication: the discriminator recovers the sum of
    /// the two phase derivatives, not some mixture of them.
    fn with_drift(
        iq: &[num_complex::Complex<f32>],
        sample_rate_hz: f64,
        drift_rate_hz_per_s: f64,
    ) -> Vec<num_complex::Complex<f32>> {
        use num_complex::Complex;
        iq.iter()
            .enumerate()
            .map(|(n, &s)| {
                let t = n as f64 / sample_rate_hz;
                let phase = std::f64::consts::TAU * 0.5 * drift_rate_hz_per_s * t * t;
                s * Complex::new(phase.cos() as f32, phase.sin() as f32)
            })
            .collect()
    }

    /// A synthetic drift of a known rate is recovered
    /// to within its own uncertainty.
    #[test]
    fn a_known_drift_rate_is_recovered_within_its_own_uncertainty() {
        let params = Le1mParams {
            sps: 4,
            sample_rate: 4_000_000.0,
            deviation_hz: 250_000.0,
            bt: 0.5,
        };
        let mut rng = Rng::new(20);
        let bits: Vec<bool> = (0..3000).map(|_| rng.next_u64() & 1 == 1).collect();
        let clean = modulate(
            &bits,
            params.sps,
            params.deviation_hz,
            params.sample_rate,
            params.bt,
        );
        let drift_rate_hz_per_us = 20.0;
        let drifted = with_drift(&clean, params.sample_rate, drift_rate_hz_per_us * 1e6);
        let mut inst = Vec::new();
        discriminate(&drifted, params.sample_rate, &mut inst);
        let (_, samples) =
            crate::signal::ble::sync::slice(&inst, params.sps as f64, bits.len(), 0.0);

        let d = drift(&bits, &samples, Phy::OneM).expect("plenty of samples at 3000 bits");
        assert!(
            (d.drift_rate_hz_per_us.value() - drift_rate_hz_per_us).abs()
                < 4.0 * d.drift_rate_hz_per_us.sigma(),
            "measured {} +/- {}, injected {}",
            d.drift_rate_hz_per_us.value(),
            d.drift_rate_hz_per_us.sigma(),
            drift_rate_hz_per_us
        );
        // The two ends of the same measurement: final must read higher
        // than initial for a positive
        // drift, not just the rate derived from their difference.
        assert!(d.final_hz.value() > d.initial_hz.value());
    }

    /// No injected drift measures as no drift, within the same uncertainty
    /// a real one would have to clear - the null result this measurement
    /// has to get right before its own "yes, this moved" means anything.
    #[test]
    fn no_injected_drift_measures_as_none_within_uncertainty() {
        let params = Le1mParams {
            sps: 4,
            sample_rate: 4_000_000.0,
            deviation_hz: 250_000.0,
            bt: 0.5,
        };
        let mut rng = Rng::new(21);
        let bits: Vec<bool> = (0..3000).map(|_| rng.next_u64() & 1 == 1).collect();
        let clean = modulate(
            &bits,
            params.sps,
            params.deviation_hz,
            params.sample_rate,
            params.bt,
        );
        let mut inst = Vec::new();
        discriminate(&clean, params.sample_rate, &mut inst);
        let (_, samples) =
            crate::signal::ble::sync::slice(&inst, params.sps as f64, bits.len(), 0.0);

        let d = drift(&bits, &samples, Phy::OneM).unwrap();
        assert!(
            d.drift_rate_hz_per_us.value().abs() < 4.0 * d.drift_rate_hz_per_us.sigma(),
            "measured {} +/- {} with nothing injected",
            d.drift_rate_hz_per_us.value(),
            d.drift_rate_hz_per_us.sigma()
        );
    }

    /// Too few samples to give each half its own variance refuses rather
    /// than inventing a drift from a handful of points.
    #[test]
    fn too_few_samples_refuses_rather_than_inventing_a_reading() {
        assert!(drift(&[true, false, true], &[1.0, 2.0, 3.0], Phy::OneM).is_none());
    }

    /// Symbols and samples at LE 2M: 2 Mb/s, four samples a symbol, 8 Msps.
    fn le2m_symbols_and_samples(
        deviation_hz: f64,
        n_bits: usize,
        seed: u64,
    ) -> (Vec<bool>, Vec<f32>) {
        let (sps, rate) = (4usize, 8_000_000.0);
        let mut rng = Rng::new(seed);
        let bits: Vec<bool> = (0..n_bits).map(|_| rng.next_u64() & 1 == 1).collect();
        let clean = modulate(&bits, sps, deviation_hz, rate, 0.5);
        let iq = at_snr(&clean, 30.0, &mut Rng::new(seed + 1));
        let mut inst = Vec::new();
        discriminate(&iq, rate, &mut inst);
        let (_, samples) = crate::signal::ble::sync::slice(&inst, sps as f64, n_bits, 0.0);
        (bits, samples)
    }

    /// **LE 2M measured as fully as LE 1M**: the
    /// nominal 500 kHz deviation, twice LE 1M's at twice the symbol rate,
    /// reads inside the same 0.45 to 0.55 index band, with delta-f1 near
    /// 500 kHz; a transmitter still using LE 1M's 250 kHz reads far below
    /// it. The index is scaled by 2 Mb/s, not 1: read at 1 Mb/s the nominal
    /// signal would have shown an index near 1.
    #[test]
    fn le_2m_is_measured_on_its_own_symbol_rate() {
        let (bits, samples) = le2m_symbols_and_samples(500_000.0, 4000, 30);
        let q = modulation_quality(&bits, &samples, Phy::TwoM).unwrap();
        let h = q.modulation_index.value();
        assert!(h > 0.45 && h < 0.55, "LE 2M nominal index {h}");
        let df1 = q.delta_f1_avg_hz.value();
        assert!(df1 > 450e3 && df1 < 550e3, "delta-f1 {df1}");

        let (bits, samples) = le2m_symbols_and_samples(250_000.0, 4000, 31);
        let low = modulation_quality(&bits, &samples, Phy::TwoM).unwrap();
        assert!(
            low.modulation_index.value() < 0.45,
            "{:?}",
            low.modulation_index
        );
    }

    /// The drift, on LE 2M's clock: a known rate comes back within its own
    /// uncertainty, which it would not if the halves were timed at 1 Mb/s.
    #[test]
    fn le_2m_drift_is_timed_at_its_own_symbol_rate() {
        let (sps, rate) = (4usize, 8_000_000.0);
        let mut rng = Rng::new(32);
        let bits: Vec<bool> = (0..6000).map(|_| rng.next_u64() & 1 == 1).collect();
        let clean = modulate(&bits, sps, 500_000.0, rate, 0.5);
        let injected = 20.0;
        let drifted = with_drift(&clean, rate, injected * 1e6);
        let mut inst = Vec::new();
        discriminate(&drifted, rate, &mut inst);
        let (_, samples) = crate::signal::ble::sync::slice(&inst, sps as f64, bits.len(), 0.0);
        let d = drift(&bits, &samples, Phy::TwoM).unwrap();
        assert!(
            (d.drift_rate_hz_per_us.value() - injected).abs()
                < 4.0 * d.drift_rate_hz_per_us.sigma(),
            "measured {:?}, injected {injected}",
            d.drift_rate_hz_per_us
        );
    }
}
