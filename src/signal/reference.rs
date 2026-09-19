// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! What our own local oscillator is doing, measured against something that
//! knows better.
//!
//! Design section 7, and the decision it records is where this lives rather than
//! what it computes: **every ppm reading in the app contains our own oscillator's
//! error**, so the correction is a property of the radio on the desk and not of
//! any one feature. A device's carrier offset, its crystal error, the whole
//! "rank the clocks in the room" measurement - each of them is our error plus
//! theirs until this is established.
//!
//! # The sign, which is the part that gets written backwards
//!
//! We command the radio to `nominal`. Our reference is fast by a fraction `e`,
//! so the synthesiser lands at `nominal * (1 + e)`. The station transmits on
//! `nominal` exactly. What arrives at baseband is therefore
//! `nominal - nominal(1 + e)`, which is `-nominal * e`.
//!
//! So **a carrier that appears above the tuned centre means our oscillator is
//! slow**, and the minus sign in [`lo_error_ppm`] is the whole of that argument.
//! Getting it backwards would not crash anything; it would report every clock in
//! the room as wrong in the opposite direction, plausibly.
//!
//! # Two uncertainties, and only one of them is ours
//!
//! The estimator's spread is a Type A uncertainty - evaluated from the
//! measurement itself, which is what N6's variances are for. The station's own
//! accuracy is Type B: it comes from a specification rather than from anything
//! we observed, and the Guide to the Expression of Uncertainty in Measurement
//! (JCGM 100:2008, 4.3.7) says a quantity known only to lie within `±b` with no
//! reason to prefer any value inside it contributes a variance of `b²/3`.
//!
//! **No station in the table below carries that figure yet**, and the reason is
//! this repository's own rule rather than an oversight: design section 18 says
//! no constant reaches the code until someone has read the actual specification.
//! NIST's station page publishes the frequencies, which is why they are here; it
//! does not publish the carrier accuracy, which is why [`Standard::tolerance_ppm`]
//! is `None` for every row. The machinery that would use it is written and
//! tested, so filling one number in is all that remains.

use rustfft::num_complex::Complex;

use crate::signal::dsp::correlate::{threshold_for_false_alarm, DelayedAutocorrelator};
use crate::signal::dsp::estimate::{moose_offset, moose_variance, snr_from_metric};
use crate::signal::dsp::uncertainty::{crlb_frequency, efficiency, Uncertain};

/// A transmitter whose carrier we are willing to measure ourselves against.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Standard {
    /// What the panel calls it.
    pub name: &'static str,
    /// The carrier, as published.
    pub hz: u64,
    /// The station's own accuracy, as a bound in ppm.
    ///
    /// `None` means "not read from a primary source", not "perfect". A reading
    /// taken against such a station carries the estimator's uncertainty alone
    /// and the panel says the station's contribution is missing, which is a
    /// smaller lie than a figure nobody checked.
    pub tolerance_ppm: Option<f64>,
    /// Where the frequency came from, so the next person can check it.
    pub source: &'static str,
}

/// Stations this app can be pointed at to learn its own error.
///
/// Source for every row: NIST, "Radio Station WWV", which lists the transmitted
/// carriers and their powers. Only what that page states is here. WWVH shares
/// the lower four carriers and CHU, MSF and DCF77 are all real candidates, but
/// each needs its own primary source read before it earns a row.
///
/// **Reachability is not this table's business.** An RTL-SDR tunes from about
/// 24 MHz and cannot hear any of these; a HackRF from 1 MHz hears all of them.
/// That is a capability question and `DeviceCapabilities` is what answers it,
/// the same split `net::gate` makes.
pub const STANDARDS: &[Standard] = &[
    Standard {
        name: "WWV 2.5 MHz",
        hz: 2_500_000,
        tolerance_ppm: None,
        source: "NIST, Radio Station WWV",
    },
    Standard {
        name: "WWV 5 MHz",
        hz: 5_000_000,
        tolerance_ppm: None,
        source: "NIST, Radio Station WWV",
    },
    Standard {
        name: "WWV 10 MHz",
        hz: 10_000_000,
        tolerance_ppm: None,
        source: "NIST, Radio Station WWV",
    },
    Standard {
        name: "WWV 15 MHz",
        hz: 15_000_000,
        tolerance_ppm: None,
        source: "NIST, Radio Station WWV",
    },
    Standard {
        name: "WWV 20 MHz",
        hz: 20_000_000,
        tolerance_ppm: None,
        source: "NIST, Radio Station WWV",
    },
];

/// How far off a station's carrier a tuning may be and still be that station.
///
/// A kilohertz. Wide enough for any oscillator this app will meet - a hundred
/// ppm at 10 MHz is a kilohertz, and a crystal that bad would be remarkable -
/// and narrow enough that it cannot reach the next standard carrier, the
/// closest pair of which are two and a half megahertz apart.
pub const TUNING_TOLERANCE_HZ: u64 = 1_000;

/// The standard being tuned to, if any.
pub fn standard_at(hz: u64) -> Option<&'static Standard> {
    STANDARDS
        .iter()
        .find(|s| s.hz.abs_diff(hz) <= TUNING_TOLERANCE_HZ)
}

/// Our oscillator's fractional error, in ppm, from a carrier seen off centre.
///
/// `baseband_hz` is where the carrier turned up relative to the tuned centre,
/// with the uncertainty the estimator gave it. `standard` contributes its own
/// Type B term when it has one.
pub fn lo_error_ppm(baseband_hz: Uncertain, standard: &Standard) -> Uncertain {
    // The minus is the argument in the header: the carrier arriving above centre
    // means the synthesiser landed below the station, which means our reference
    // is slow.
    let ours = baseband_hz.scale(-1e6 / standard.hz as f64);
    let station = standard.tolerance_ppm.map_or(0.0, type_b_variance);
    Uncertain::from_variance(ours.value(), ours.sigma().powi(2) + station)
}

/// How often the estimator is allowed to call noise a carrier.
///
/// One in ten thousand captures. A reference is established once and then
/// believed by every ppm reading in the app until it expires, so the cost of a
/// false one is high and the cost of refusing is one keypress. N5's
/// [`threshold_for_false_alarm`] turns this into a coherence threshold through
/// the Beta law, which is why this is a rate here rather than a level.
const FALSE_ALARM: f64 = 1e-4;

/// The residual carrier offset in a block of baseband samples, and its spread.
///
/// **The estimator is the delayed autocorrelation and its variance is Moose's.**
/// N5 built the first and N6 the second, together with the Cramer-Rao bound that
/// says whether the pair are being honest. Nothing new is invented here: this is
/// the two of them pointed at a carrier instead of at a preamble.
///
/// **`max_offset_hz` is how far the caller is prepared to look, and it is a
/// parameter because it has to be.** The correlation's phase cannot tell an
/// angle from the same angle plus a turn, so the lag sets a range and outside it
/// the answer wraps into a wrong number that looks exactly like a right one -
/// `moose_offset`'s own documentation says the caller is responsible for
/// arranging that the offset fits, and this is that arrangement. The lag is
/// derived from the range asked for; a block too short to reach it is refused
/// rather than silently measured with a lag that wraps.
///
/// `None` when the block cannot hold the requested range, when there is no
/// energy in it, or when the coherence is below what noise alone would clear
/// [`FALSE_ALARM`] of the time - which is the case where a number would be
/// worst, because a weak carrier gives a confident-looking offset that is mostly
/// noise.
pub fn carrier_offset_hz(
    samples: &[Complex<f32>],
    sample_rate_hz: f64,
    max_offset_hz: f64,
) -> Option<CarrierOffset> {
    if !sample_rate_hz.is_finite() || sample_rate_hz <= 0.0 {
        return None;
    }
    if !max_offset_hz.is_finite() || max_offset_hz <= 0.0 {
        return None;
    }
    // The longest lag whose range still holds the offset asked for. Longest,
    // because range and precision trade one for one and the caller has already
    // said how much range it needs: anything shorter throws away precision it
    // was not asked to give up.
    let lag = (sample_rate_hz / (2.0 * max_offset_hz)).floor();
    if lag.is_nan() || lag < 1.0 {
        return None;
    }
    let lag = lag as usize;
    // A window of at least the lag, so the correlation rests on as many products
    // as the block can afford after paying for the range.
    let window = samples.len().checked_sub(lag).filter(|w| *w >= lag)?;

    let mut correlator = DelayedAutocorrelator::new(window, lag);
    let mut reading = None;
    for x in samples {
        if let Some(r) = correlator.push(*x) {
            reading = Some(r);
        }
    }
    // The last reading, not the first: it is the one whose window holds the most
    // of the block. Taking the first would measure the leading quarter and throw
    // the rest away.
    let reading = reading?;

    // A weak carrier is the dangerous case, not the empty one: it returns an
    // offset that looks like a measurement. `snr_from_metric` refusing is the
    // whole guard, and it refuses exactly when the coherence is too low to imply
    // a signal-to-noise ratio at all.
    let metric = reading.metric()?;
    if metric < threshold_for_false_alarm(window, FALSE_ALARM) {
        return None;
    }
    let snr = snr_from_metric(metric, window)?;
    let offset = moose_offset(reading.p, lag);
    let variance = moose_variance(snr, window, lag);
    // The floor for the same block at the same SNR: every sample the
    // correlator saw, `window + lag` of them, which is the whole block.
    let bound = crlb_frequency(snr, window + lag);
    Some(CarrierOffset {
        offset_hz: Uncertain::from_variance(offset, variance).scale(sample_rate_hz),
        efficiency: efficiency(variance, bound),
    })
}

/// A carrier's offset, and how close the estimate came to the physical limit.
#[derive(Clone, Copy, Debug)]
pub struct CarrierOffset {
    pub offset_hz: Uncertain,
    /// `crlb / variance`, in `(0, 1]` (`dsp::uncertainty::efficiency`): one
    /// is as good as the samples and the SNR allow. Well below one says the
    /// estimator is what limits the reference, not the signal, which is
    /// design section 5.4's reason to display the bound at all.
    pub efficiency: f64,
}

/// How far off our oscillator is allowed to be before the search gives up.
///
/// Two hundred parts per million, which is far worse than any crystal in a radio
/// anyone would put on a bench - the cheapest RTL-SDR dongles are specified at
/// fifty and drift to perhaps a hundred cold. It is a search bound rather than a
/// claim about hardware: too small and a genuinely bad oscillator is refused,
/// too large and the estimator gives up precision for range it never needed.
pub const SEARCH_PPM: f64 = 200.0;

/// The offset range to search for a carrier that should be on `nominal_hz`.
pub fn search_range_hz(nominal_hz: f64) -> f64 {
    nominal_hz * SEARCH_PPM / 1e6
}

/// One captured block, interpreted: bytes in, a reference out or a reason why
/// not.
///
/// **The reason is returned rather than logged here** so the caller decides
/// where it goes, and so this stays testable with no radio. Every failure is a
/// sentence a user can act on: a wrong tuning, a carrier too weak, an
/// oscillator further out than the search covers.
pub fn capture(
    bytes: &[u8],
    geometry: crate::hardware::SampleGeometry,
    tuned_hz: u64,
    sample_rate_hz: f64,
) -> Result<(Uncertain, f64, &'static Standard), String> {
    let Some(standard) = standard_at(tuned_hz) else {
        return Err(format!(
            "no standard station at {:.3} MHz: tune to one of {} first",
            tuned_hz as f64 / 1e6,
            STANDARDS
                .iter()
                .map(|s| s.name)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    };

    let pairs = bytes.len() / geometry.bytes_per_pair();
    // Flat, not Hann: a window is for keeping one tone's skirts off its
    // neighbours in a transform, and this is a correlation over a single
    // carrier. Tapering it would throw away the ends of the block for nothing.
    let flat = vec![1.0f32; pairs];
    let mut samples = vec![Complex::new(0.0, 0.0); pairs];
    crate::signal::fft::frame::decode_into(
        &bytes[..pairs * geometry.bytes_per_pair()],
        &flat,
        geometry,
        &mut samples,
    );

    let range = search_range_hz(standard.hz as f64);
    let offset = carrier_offset_hz(&samples, sample_rate_hz, range).ok_or_else(|| {
        format!(
            "no carrier within {:.0} Hz of centre on {}: too weak, or the \
             oscillator is more than {SEARCH_PPM:.0} ppm out",
            range, standard.name
        )
    })?;
    Ok((
        lo_error_ppm(offset.offset_hz, standard),
        offset.efficiency,
        standard,
    ))
}

/// A specification bound turned into a variance.
///
/// JCGM 100:2008 4.3.7: a quantity known only to lie within `±b`, with no reason
/// to prefer any value inside that interval, is a rectangular distribution whose
/// variance is `b²/3`. Not `b²`, which would treat the bound as a standard
/// deviation and overstate it by seventy percent, and not zero, which is what
/// leaving it out amounts to.
pub fn type_b_variance(bound: f64) -> f64 {
    if bound.is_finite() {
        bound * bound / 3.0
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_says_where_every_number_came_from() {
        assert!(!STANDARDS.is_empty());
        for s in STANDARDS {
            assert!(!s.source.is_empty(), "{} cites nothing", s.name);
            // The rule this table exists under: design section 18 says a
            // constant reaches the code only after a primary source has been
            // read. The frequencies have been; the accuracies have not, and an
            // invented one would be indistinguishable from a checked one.
            assert_eq!(
                s.tolerance_ppm, None,
                "{} carries an accuracy figure: has it been read from a primary \
                 source? If so, say where in `source` and delete this assertion",
                s.name
            );
        }
        // No two rows within a tuning tolerance of each other, or `standard_at`
        // would answer with whichever came first in the list.
        for (i, a) in STANDARDS.iter().enumerate() {
            for b in &STANDARDS[i + 1..] {
                assert!(
                    a.hz.abs_diff(b.hz) > 2 * TUNING_TOLERANCE_HZ,
                    "{} and {} are indistinguishable",
                    a.name,
                    b.name
                );
            }
        }
    }

    #[test]
    fn a_tuning_finds_the_station_it_is_on_and_no_other() {
        assert_eq!(standard_at(10_000_000).map(|s| s.name), Some("WWV 10 MHz"));
        // Inside the tolerance, which is where a real tuning lands.
        assert_eq!(standard_at(10_000_900).map(|s| s.name), Some("WWV 10 MHz"));
        assert_eq!(standard_at(9_999_100).map(|s| s.name), Some("WWV 10 MHz"));
        // Outside it there is no station, and saying so is the point: a
        // reference captured against "whatever was nearest" is not a reference.
        assert_eq!(standard_at(10_002_000), None);
        assert_eq!(standard_at(7_000_000), None);
        assert_eq!(standard_at(2_437_000_000), None);
    }

    /// **The sign, which is the one thing here that can be plausibly wrong.**
    ///
    /// A carrier that turns up *above* the tuned centre means the synthesiser
    /// landed *below* the station, which means our reference is slow. Backwards,
    /// this reports every clock in the room as wrong in the other direction and
    /// nothing looks broken.
    #[test]
    fn a_carrier_above_centre_means_our_oscillator_is_slow() {
        let wwv = standard_at(10_000_000).unwrap();
        // The carrier turns up 100 Hz above centre at 10 MHz: 10 ppm.
        let e = lo_error_ppm(Uncertain::from_sigma(100.0, 1.0), wwv);
        assert!((e.value() + 10.0).abs() < 1e-9, "got {}", e.value());
        // And below centre is the other way.
        let e = lo_error_ppm(Uncertain::from_sigma(-100.0, 1.0), wwv);
        assert!((e.value() - 10.0).abs() < 1e-9, "got {}", e.value());
        // Dead on is dead on, and not a small number that rounds to it.
        let e = lo_error_ppm(Uncertain::from_sigma(0.0, 1.0), wwv);
        assert_eq!(e.value(), 0.0);
    }

    #[test]
    fn the_uncertainty_scales_with_the_frequency_it_was_measured_at() {
        // The same 1 Hz of estimator spread is worth ten times more ppm at
        // 2.5 MHz than at 25 MHz, which is the whole reason to reference against
        // the highest carrier the radio can hear.
        let low = lo_error_ppm(
            Uncertain::from_sigma(0.0, 1.0),
            standard_at(2_500_000).unwrap(),
        );
        let high = lo_error_ppm(
            Uncertain::from_sigma(0.0, 1.0),
            standard_at(20_000_000).unwrap(),
        );
        assert!((low.sigma() - 0.4).abs() < 1e-9, "got {}", low.sigma());
        assert!((high.sigma() - 0.05).abs() < 1e-9, "got {}", high.sigma());
    }

    /// A tone at a known offset comes back, with a spread that covers the error
    /// it actually made.
    ///
    /// The second half is the part worth having: an estimator that returns the
    /// right number with an uncertainty it cannot support is the failure this
    /// whole layer exists to prevent, so the test asserts the claim as well as
    /// the value.
    #[test]
    fn a_tone_at_a_known_offset_comes_back_with_an_honest_spread() {
        use crate::signal::dsp::testkit::Rng;
        use std::f64::consts::TAU;

        const RATE: f64 = 2_000_000.0;
        const N: usize = 8_192;
        for want_hz in [0.0, 37.0, -37.0, 400.0, -1_500.0] {
            let mut worst = 0.0f64;
            let mut claimed = 0.0f64;
            for seed in 0..16u64 {
                let mut rng = Rng::new(seed * 104_729 + 5);
                let noise = rng.noise(N, 0.01);
                let samples: Vec<Complex<f32>> = (0..N)
                    .map(|n| {
                        let ph = TAU * want_hz * n as f64 / RATE;
                        Complex::new(ph.cos() as f32, ph.sin() as f32) + noise[n]
                    })
                    .collect();
                let got = carrier_offset_hz(&samples, RATE, 2_000.0)
                    .expect("a clean tone measures")
                    .offset_hz;
                worst = worst.max((got.value() - want_hz).abs());
                claimed = got.sigma();
            }
            assert!(
                worst < 3.0 * claimed,
                "{want_hz} Hz: worst error {worst:.3} against a claimed sigma of {claimed:.3}"
            );
            // And the claim is not so wide as to be useless: at 10 MHz this has
            // to resolve a part per million, which is 10 Hz.
            assert!(claimed < 3.0, "{want_hz} Hz: claimed sigma {claimed}");
        }
    }

    /// **The estimate never claims to beat its own floor.** Moose's variance
    /// and the Cramer-Rao bound are two formulas from two papers; an
    /// efficiency above one would mean they disagree about the same block,
    /// and whichever is wrong, the card would print a flattering lie.
    #[test]
    fn the_reference_never_claims_to_beat_the_bound() {
        use crate::signal::dsp::testkit::Rng;
        use std::f64::consts::TAU;
        const RATE: f64 = 2_000_000.0;
        for (n, range, amp) in [
            (8_192, 2_000.0, 0.01),
            (4_096, 20_000.0, 0.1),
            (32_768, 500.0, 0.3),
        ] {
            let mut rng = Rng::new(n as u64);
            let noise = rng.noise(n, amp);
            let samples: Vec<Complex<f32>> = (0..n)
                .map(|k| {
                    let ph = TAU * 120.0 * k as f64 / RATE;
                    Complex::new(ph.cos() as f32, ph.sin() as f32) + noise[k]
                })
                .collect();
            let got = carrier_offset_hz(&samples, RATE, range).expect("a tone measures");
            assert!(
                got.efficiency > 0.0 && got.efficiency <= 1.0,
                "{n} samples, {range} Hz: efficiency {}",
                got.efficiency
            );
        }
    }

    /// Refusals, in the three cases where a number would be worst.
    #[test]
    fn a_block_with_nothing_in_it_refuses_rather_than_answering() {
        use crate::signal::dsp::testkit::Rng;
        const RATE: f64 = 2_000_000.0;

        // Too short to fill the correlator at all.
        assert!(carrier_offset_hz(&[], RATE, 2_000.0).is_none());
        assert!(carrier_offset_hz(&[Complex::new(1.0, 0.0); 8], RATE, 2_000.0).is_none());
        // No energy: silence is not a carrier on zero hertz.
        assert!(carrier_offset_hz(&[Complex::new(0.0, 0.0); 4096], RATE, 2_000.0).is_none());
        // Noise alone. A weak carrier is the dangerous case, because the offset
        // it returns looks like a measurement.
        let mut rng = Rng::new(11);
        let noise = rng.noise(4096, 1.0);
        assert!(
            carrier_offset_hz(&noise, RATE, 2_000.0).is_none(),
            "noise alone must not produce a frequency"
        );
        // A nonsense rate is nothing to convert against.
        let mut rng = Rng::new(12);
        let tone = rng.noise(4096, 0.001);
        assert!(carrier_offset_hz(&tone, 0.0, 2_000.0).is_none());
        assert!(carrier_offset_hz(&tone, RATE, 0.0).is_none());
        assert!(carrier_offset_hz(&tone, RATE, f64::NAN).is_none());
    }

    /// A bound is not a sigma, and treating it as one overstates it by seventy
    /// percent.
    #[test]
    fn a_specification_bound_becomes_a_rectangular_variance() {
        assert!((type_b_variance(0.05) - 0.05f64.powi(2) / 3.0).abs() < 1e-18);
        assert_eq!(type_b_variance(0.0), 0.0);
        // Nonsense in, nothing added, rather than a NaN propagating into every
        // ppm reading in the app.
        assert_eq!(type_b_variance(f64::NAN), 0.0);
        assert_eq!(type_b_variance(-1.0), type_b_variance(1.0));
    }

    /// The station's own accuracy joins the estimator's, in quadrature, when
    /// there is one to join.
    #[test]
    fn a_station_with_a_stated_accuracy_widens_the_reading() {
        let bare = Standard {
            name: "test",
            hz: 10_000_000,
            tolerance_ppm: None,
            source: "the test below",
        };
        let stated = Standard {
            tolerance_ppm: Some(0.05),
            ..bare
        };
        let a = lo_error_ppm(Uncertain::from_sigma(0.0, 1.0), &bare);
        let b = lo_error_ppm(Uncertain::from_sigma(0.0, 1.0), &stated);
        assert!(b.sigma() > a.sigma(), "{} vs {}", b.sigma(), a.sigma());
        // In quadrature, not added: they are independent.
        let want = (a.sigma().powi(2) + type_b_variance(0.05)).sqrt();
        assert!((b.sigma() - want).abs() < 1e-12);
    }
}
