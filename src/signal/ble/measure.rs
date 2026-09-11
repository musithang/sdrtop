// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Transmitter modulation quality: modulation index, delta-f1 average,
//! delta-f2 maximum and their ratio, from a packet this arc already
//! recovered rather than a dedicated test transmission.
//!
//! Design section 2.1's four numbers are specified as read from a *known*
//! symbol pattern: `00001111` repeated for delta-f1, `10101010` repeated for
//! delta-f2. Bluetooth's own conformance test procedure gets that known
//! pattern by putting the device under test into Direct Test Mode and
//! sending it a specific payload, whitening disabled. This receiver has no
//! way to do either - it is a passive listener on ordinary advertising
//! traffic from devices it does not control, and rule 8 (`POLICY.md`) rules
//! out ever transmitting to ask for one.
//!
//! **What the two patterns are *for* is what this measures instead of the
//! patterns themselves.** `00001111` is chosen because four symbol periods
//! of the same value is enough for a BT=0.5 Gaussian filter to reach
//! whatever deviation it settles at - the measurement point is the *last*
//! symbol of the run, as far from the last transition as the pattern gets.
//! `10101010` is chosen because continuous alternation is the opposite
//! extreme, the pattern that gives the filter the least time to settle
//! between transitions. Both conditions occur constantly in ordinary
//! whitened traffic, which looks like uniformly random bits at the symbol
//! level: [`SETTLED_RUN`] or more identical bits in a row, and
//! [`SETTLED_RUN`] or more bits that strictly alternate, both happen many
//! times in a packet of any real length. This measures at every such
//! occurrence and reports the same four numbers the specification's own
//! procedure does, built from data this receiver already has instead of
//! data it has no way to ask for.
//!
//! **Measured against the physically transmitted bits, not the decoded
//! ones.** Whitening is a logical operation applied before modulation and
//! undone after slicing; the Gaussian filter and the discriminator only
//! ever see the *whitened* symbol sequence, so a run of identical or
//! alternating *data* bits is not what settles or unsettles the filter - a
//! run of identical or alternating *on-air* bits is. `signal::ble::receive`
//! calls this before its own call to [`crate::signal::dsp::code::lfsr::whiten`],
//! on the same bits [`super::sync::slice`] sliced.

use crate::signal::dsp::uncertainty::Uncertain;

/// LE 1M's symbol rate, fixed at 1 Mb/s by the PHY itself. Not derived from
/// a receiver's own `sps` - that is a demodulator design choice, this is a
/// specification fact, and the two must never be confused into agreeing by
/// coincidence.
const SYMBOL_RATE_HZ: f64 = 1_000_000.0;

/// How many like, or alternating, symbols in a row counts as "settled" for
/// [`modulation_quality`]'s own purposes: `00001111`'s two four-symbol runs
/// and `10101010`'s continuous alternation both generalise to this one
/// number. See the module doc for why a literal search for either octet
/// pattern is not what real advertising traffic can supply.
const SETTLED_RUN: usize = 4;

/// One packet's modulation quality, each figure carrying the uncertainty a
/// caller needs to judge it against a stated limit - except
/// [`Self::delta_f2_max_hz`], a maximum of several noisy readings, which has
/// no closed-form uncertainty the way a mean does and is reported as read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModulationQuality {
    /// The average peak deviation reached at the end of a settled run of
    /// four or more identical on-air symbols, in Hz.
    pub delta_f1_avg_hz: Uncertain,
    /// The largest single peak deviation seen during a settled run of four
    /// or more alternating on-air symbols, in Hz. Not an [`Uncertain`]: see
    /// this struct's own doc for why a maximum does not get one.
    pub delta_f2_max_hz: f64,
    /// `2 * delta_f1_avg_hz / SYMBOL_RATE_HZ` - the modulation index
    /// design section 2.1 states a band for, derived from delta-f1 because
    /// that is the settled, filter-independent deviation a device's own
    /// deviation setting actually controls.
    pub modulation_index: Uncertain,
    /// The average peak deviation over alternating runs, over the same
    /// figure for settled runs - design section 2.1's fourth number, "the
    /// specification's own way of asking whether the Gaussian filter is
    /// right": a ratio well below one means the filter is narrower than it
    /// should be, closing the eye during continuous alternation more than
    /// the specification allows.
    pub ratio: Uncertain,
}

/// Whether the window of `SETTLED_RUN` bits ending at `i` (inclusive) is all
/// one value.
fn ends_settled_run(bits: &[bool], i: usize) -> bool {
    i + 1 >= SETTLED_RUN
        && bits[i + 1 - SETTLED_RUN..=i]
            .windows(2)
            .all(|w| w[0] == w[1])
}

/// Whether the window of `SETTLED_RUN` bits ending at `i` (inclusive)
/// strictly alternates.
fn ends_alternating_run(bits: &[bool], i: usize) -> bool {
    i + 1 >= SETTLED_RUN
        && bits[i + 1 - SETTLED_RUN..=i]
            .windows(2)
            .all(|w| w[0] != w[1])
}

/// Modulation quality from one packet's own on-air symbols (`bits`) and the
/// discriminator sample recovered at each one (`samples`, in Hz) - the same
/// two arrays [`super::sync::slice`] returns, before whitening is undone.
///
/// `None` when either pattern never occurred - a packet too short, or one
/// whose particular random content happened to lack a settled run of either
/// kind. Rule 2: a measurement with nothing behind it is refused, not
/// invented from zero occurrences.
pub fn modulation_quality(bits: &[bool], samples: &[f32]) -> Option<ModulationQuality> {
    debug_assert_eq!(bits.len(), samples.len());
    let n = bits.len().min(samples.len());

    let settled: Vec<f32> = (0..n)
        .filter(|&i| ends_settled_run(bits, i))
        .map(|i| samples[i].abs())
        .collect();
    let alternating: Vec<f32> = (0..n)
        .filter(|&i| ends_alternating_run(bits, i))
        .map(|i| samples[i].abs())
        .collect();

    if settled.is_empty() || alternating.is_empty() {
        return None;
    }

    let delta_f1_avg_hz = crate::signal::dsp::uncertainty::mean_with_uncertainty(&settled);
    let delta_f2_avg_hz = crate::signal::dsp::uncertainty::mean_with_uncertainty(&alternating);
    let delta_f2_max_hz = alternating.iter().cloned().fold(f32::MIN, f32::max) as f64;
    let modulation_index = delta_f1_avg_hz.scale(2.0 / SYMBOL_RATE_HZ);
    let ratio = delta_f2_avg_hz.ratio(&delta_f1_avg_hz);

    Some(ModulationQuality {
        delta_f1_avg_hz,
        delta_f2_max_hz,
        modulation_index,
        ratio,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::ble::detect::Le1mParams;
    use crate::signal::ble::gfsk::modulate;
    use crate::signal::dsp::discriminate::discriminate;
    use crate::signal::dsp::testkit::{at_snr, Rng};

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

    /// B8's exit condition, the "measured as wrong" half: a deviation
    /// clearly outside the modulation index band measures outside it, and
    /// the measurement moves the *right way* as deviation rises - up for
    /// more, down for less - rather than only "close to the deviation
    /// asked for".
    ///
    /// **Not compared against `2 * deviation_hz / SYMBOL_RATE_HZ` to a
    /// tight tolerance, and that is a finding of its own kind.** A first
    /// version of this test did exactly that, at three deviations, and
    /// failed at 200 kHz while passing at 250 kHz - not because the
    /// measurement is wrong, but because [`find_phase`](crate::signal::dsp::timing::find_phase)'s
    /// amplitude-based search has a broad, nearly flat maximum over a
    /// settled plateau, so two *independent* simulated captures at
    /// different deviations can legitimately settle on phases a fraction of
    /// a sample apart - benign in itself, but enough to move which exact
    /// sample a transition-region symbol reads, which this generalised
    /// measurement (any settled run of four, not only a repeated
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
                let q = modulation_quality(&bits, &samples)
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

        // 250 kHz is BLE's own nominal deviation and must land inside the
        // band; 150 kHz and 350 kHz are both far enough outside it (design
        // section 2.1's own 0.45-0.55) that no plausible settling loss
        // brings them back in - a genuinely wrong transmitter, correctly
        // read as one.
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
        let q = modulation_quality(&bits, &samples).unwrap();
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
        let q = modulation_quality(&bits, &samples).unwrap();
        assert!(
            q.ratio.value() <= 1.01,
            "ratio {} implies alternation reached further than settling does",
            q.ratio.value()
        );
        assert!(q.delta_f2_max_hz <= q.delta_f1_avg_hz.value() * 1.2);
    }

    /// A run too short to contain either pattern refuses rather than
    /// inventing a measurement from nothing.
    #[test]
    fn too_short_a_run_refuses_rather_than_inventing_a_reading() {
        let bits = vec![true, false, true];
        let samples = vec![1.0f32, -1.0, 1.0];
        assert!(modulation_quality(&bits, &samples).is_none());
    }

    /// A packet with settled runs but no alternation - or the reverse -
    /// still refuses, because the ratio and the panel's own display need
    /// both figures, not just one.
    #[test]
    fn one_pattern_present_without_the_other_still_refuses() {
        let all_settled = vec![true; 20];
        let samples = vec![1.0f32; 20];
        assert!(modulation_quality(&all_settled, &samples).is_none());
    }
}
