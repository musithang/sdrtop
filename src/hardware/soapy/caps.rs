// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! What a driver said about itself, turned into a [`DeviceCapabilities`].
//!
//! **This file never sees a device pointer.** It takes [`DriverAnswers`], which
//! is plain Rust data, and returns capabilities or a refusal. That is what lets
//! the actual thinking here be tested on a machine with no radio, no driver and
//! no libSoapySDR, which is what CI has and what most contributors have.
//!
//! The rule this file exists to enforce, from `dev_docs/soapy-design.md`: **ask,
//! do not tabulate; and what cannot be asked is refused, not guessed.** Every
//! number below comes from the driver. Nothing is a constant somebody typed off
//! a datasheet, because we do not have the datasheets for the radios this
//! backend is for.

use std::fmt;

use crate::hardware::{
    Boost, DeliveryModel, DeviceCapabilities, GainModel, SampleFormat, SampleGeometry, StageSpec,
};

/// Pairs read out of one `readStream` call.
///
/// sdrtop picks the read size rather than following `getStreamMTU`, so the
/// timing panel's expected-callback-period maths has a number to work from
/// before a stream exists. The MTU only bounds a read; it does not dictate one.
/// **`stream.rs` must use this same constant** or the jitter expectation will be
/// measuring one thing and predicting another.
pub const READ_PAIRS: u64 = 16_384;

/// Everything sdrtop asks a device about itself, as plain data.
///
/// One struct rather than a dozen arguments so the tests below can be written as
/// "here is an Airspy, here is what it should come out as", which is the only
/// form of hardware testing available for radios nobody here owns.
#[derive(Clone, Debug, Default)]
pub struct DriverAnswers {
    /// `getFrequencyRange`, which returns a **list**: a tuner with a gap reports
    /// each side separately.
    pub freq_ranges: Vec<(f64, f64)>,
    /// `getSampleRateRange`. Drivers that only support discrete rates report
    /// them as zero-width ranges.
    pub rate_ranges: Vec<(f64, f64)>,
    /// `getGainRange`, the overall gain across every element.
    pub gain_range: (f64, f64),
    /// `listGains` in order, each name paired with its own
    /// `getGainElementRange`. The order is the driver's statement about its
    /// chain and is preserved exactly: nothing here sorts or renames.
    pub gain_elements: Vec<StageSpec>,
    /// `hasGainMode`: whether there is an automatic gain mode to toggle.
    pub has_gain_mode: bool,
    /// `getBandwidthRange`. Empty means no programmable baseband filter.
    pub bandwidth_ranges: Vec<(f64, f64)>,
    /// `getNativeStreamFormat`, and the full scale it reports alongside.
    pub native_format: String,
    pub native_full_scale: f64,
}

/// Why a device cannot be offered. Every variant names the thing that was wrong,
/// because this text goes straight into the log and then into a bug report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unsupported {
    /// The native format is one sdrtop's integer pipeline cannot decode.
    Format(String),
    /// No usable tuning range.
    FrequencyRange,
    /// No usable sample rate.
    SampleRate,
}

impl fmt::Display for Unsupported {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Unsupported::Format(name) => write!(
                f,
                "its native sample format is {name}, and sdrtop decodes CS8, CU8 and CS16"
            ),
            Unsupported::FrequencyRange => write!(f, "it reports no usable frequency range"),
            Unsupported::SampleRate => write!(f, "it reports no usable sample rate"),
        }
    }
}

/// Preferred startup tuning, clamped into whatever the device actually allows.
const WANT_FREQ_HZ: f64 = 100_000_000.0;
const WANT_RATE_HZ: f64 = 2_400_000.0;

/// What `capabilities` produced, plus anything the caller should say out loud.
///
/// `caps.rs` has no logger and should not have one: it is the file that has to
/// be testable with no library and no radio. So a refusal it makes about one
/// element travels back as text and `device.rs` logs it, which also makes the
/// wording itself a thing a test can assert.
#[derive(Debug)]
pub struct Built {
    pub caps: DeviceCapabilities,
    pub notes: Vec<String>,
}

/// Split the driver's elements into stages to distribute across and a boost to
/// toggle, dropping anything it described unusably.
///
/// Three outcomes, and each is the driver's own answer rather than a guess:
///
/// - **unusable** (an inverted or non-finite range) is refused by name. A stage
///   silently pinned at its minimum looks exactly like one the user turned down.
/// - **two positions** is a switch, so it becomes the boost. `SoapyHackRF`
///   reports `AMP [0, 14, 14]`, which is the same physical amp the native
///   backend drives with its own key.
/// - anything else is a stage.
///
/// Only the **first** switch becomes the boost. sdrtop has one boost concept and
/// one key for it; a second switch stays in the list as a stage it can still set,
/// rather than being silently dropped or quietly stealing the key.
fn split_elements(elements: &[StageSpec]) -> (Vec<StageSpec>, Option<Boost>, Vec<String>) {
    let mut stages = Vec::new();
    let mut boost = None;
    let mut notes = Vec::new();
    for e in elements {
        if !e.is_usable() {
            notes.push(format!(
                "SoapySDR: ignoring gain element {:?}, the driver reports its range as \
                 [{}, {}] which is not a range.",
                e.name, e.min_db, e.max_db
            ));
            continue;
        }
        if e.is_switch() && boost.is_none() {
            boost = Some(Boost::Element(e.clone()));
            continue;
        }
        stages.push(e.clone());
    }
    (stages, boost, notes)
}

pub fn capabilities(a: &DriverAnswers) -> Result<Built, Unsupported> {
    let geometry = geometry_for(&a.native_format, a.native_full_scale)?;
    let (freq_min, freq_max) = hull(&a.freq_ranges).ok_or(Unsupported::FrequencyRange)?;
    let (rate_min, rate_max) = hull(&a.rate_ranges).ok_or(Unsupported::SampleRate)?;

    let (gain_lo, gain_hi) = a.gain_range;
    let min_db = gain_lo.max(0.0).round() as u32;
    let max_db = gain_hi.max(gain_lo).max(0.0).round() as u32;

    let (mut stages, element_boost, mut notes) = split_elements(&a.gain_elements);
    // A driver that names no elements, or names only unusable ones, still has
    // the whole-chain range. One unnamed stage over it is exactly what this
    // backend did before it could ask per element, so nothing regresses.
    if stages.is_empty() {
        stages.push(StageSpec::ranged("RF", gain_lo, gain_hi.max(gain_lo), 0.0));
        if !a.gain_elements.is_empty() {
            notes.push(
                "SoapySDR: no usable gain element, falling back to the whole-chain range."
                    .to_string(),
            );
        }
    }
    // `hasGainMode` is automatic gain control; a two-position element is a
    // manual switch. Both land on sdrtop's one boost concept, and the element
    // wins because it is the one the user can actually set.
    let boost = element_boost.or(a.has_gain_mode.then_some(Boost::GainMode));

    let caps = DeviceCapabilities {
        freq_min_hz: freq_min.max(0.0) as u64,
        freq_max_hz: freq_max.max(0.0) as u64,
        sample_rate_min_hz: rate_min,
        sample_rate_max_hz: rate_max,
        default_frequency_hz: WANT_FREQ_HZ.clamp(freq_min, freq_max) as u64,
        default_sample_rate_hz: WANT_RATE_HZ.clamp(rate_min, rate_max),
        sample_geometry: geometry,
        gain: GainModel::Soapy {
            min_db,
            max_db,
            stages,
            boost,
        },
        samples_per_transfer: READ_PAIRS,
        has_bb_filter: hull(&a.bandwidth_ranges).is_some(),
        // The Friis cascade models the HackRF's known three-stage chain. We do
        // not know a Soapy device's chain, and a plausible-looking noise figure
        // for a chain we invented is the worst output this program could
        // produce. S10 is where the panels learn to say so.
        friis_applicable: false,
        // `readStream` is a pull: it returns as soon as the driver has data,
        // so our loop sets the rhythm rather than measuring one.
        delivery: DeliveryModel::Pull,
    };
    Ok(Built { caps, notes })
}

/// The lowest minimum and the highest maximum across a list of ranges.
///
/// `getFrequencyRange` returns a list because a tuner can have a gap, and
/// `DeviceCapabilities` holds one span. The hull is the honest summary of it: a
/// tune into a gap is refused by the device and logged, rather than being
/// prevented by a range we made up.
///
/// Zero-width entries are kept on purpose. Drivers that support only discrete
/// sample rates report each one as `min == max`, and a hull over those is still
/// the right outer bound.
fn hull(ranges: &[(f64, f64)]) -> Option<(f64, f64)> {
    let usable: Vec<(f64, f64)> = ranges
        .iter()
        .copied()
        .filter(|(lo, hi)| lo.is_finite() && hi.is_finite() && *hi >= *lo && *hi > 0.0)
        .collect();
    let lo = usable
        .iter()
        .map(|(lo, _)| *lo)
        .fold(f64::INFINITY, f64::min);
    let hi = usable
        .iter()
        .map(|(_, hi)| *hi)
        .fold(f64::NEG_INFINITY, f64::max);
    (lo.is_finite() && hi.is_finite() && hi > lo).then_some((lo, hi))
}

/// The native format and its full scale, if sdrtop can decode it.
///
/// **Only the native format is accepted.** SoapySDR will happily convert to
/// another one, but then the full scale is the converter's target rather than
/// the converter's own, and the ADC bench would be describing an ADC that is not
/// there. Refusing names the format, which is one log line away from a community
/// issue that tells us exactly which driver to add next.
fn geometry_for(native: &str, full_scale: f64) -> Result<SampleGeometry, Unsupported> {
    let format = match native {
        "CS8" => SampleFormat::Int8,
        "CU8" => SampleFormat::Uint8,
        "CS16" => SampleFormat::Int16,
        other => return Err(Unsupported::Format(other.to_string())),
    };
    // A driver that reports a nonsense scale would otherwise divide the whole
    // pipeline by zero. Fall back to the container's own full scale, which is
    // the one thing about it we do know.
    let full_scale = if full_scale.is_finite() && full_scale >= 1.0 {
        full_scale as f32
    } else {
        match format {
            SampleFormat::Int8 | SampleFormat::Uint8 => 128.0,
            SampleFormat::Int16 => 32768.0,
        }
    };
    Ok(SampleGeometry { format, full_scale })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verbatim from `SoapySDRUtil --probe="driver=hackrf"` on this machine.
    /// A HackRF through SoapyHackRF is the one Soapy device that can actually be
    /// tested here, which makes it the reference shape.
    /// One element as the driver describes it: name, then the `SoapySDRRange`
    /// it answers `getGainElementRange` with.
    fn el(name: &str, min: f64, max: f64, step: f64) -> StageSpec {
        StageSpec::ranged(name, min, max, step)
    }

    fn soapy_hackrf() -> DriverAnswers {
        DriverAnswers {
            freq_ranges: vec![(0.0, 7_250e6)],
            rate_ranges: (1..=20).map(|m| (m as f64 * 1e6, m as f64 * 1e6)).collect(),
            gain_range: (0.0, 116.0),
            gain_elements: vec![
                el("LNA", 0.0, 40.0, 8.0),
                el("AMP", 0.0, 14.0, 14.0),
                el("VGA", 0.0, 62.0, 2.0),
            ],
            has_gain_mode: false,
            bandwidth_ranges: vec![(1.75e6, 28e6)],
            native_format: "CS8".into(),
            native_full_scale: 128.0,
        }
    }

    /// Also verbatim, from `--probe="driver=audio"`. Kept because it is the
    /// shape that breaks lazy rules: a zero-width gain range, an AGC, and a full
    /// scale that does not fit its own container.
    fn built_in_audio() -> DriverAnswers {
        DriverAnswers {
            freq_ranges: vec![(0.0, 6_000e6)],
            rate_ranges: vec![(8e3, 8e3), (44.1e3, 44.1e3), (192e3, 192e3)],
            gain_range: (0.0, 0.0),
            gain_elements: vec![],
            has_gain_mode: true,
            bandwidth_ranges: vec![],
            native_format: "CS16".into(),
            native_full_scale: 65536.0,
        }
    }

    /// An Airspy R2, from its published specification rather than from a probe,
    /// because nobody here has one. Exactly the case this backend exists for.
    fn airspy_r2() -> DriverAnswers {
        DriverAnswers {
            freq_ranges: vec![(24e6, 1_800e6)],
            rate_ranges: vec![(2.5e6, 2.5e6), (10e6, 10e6)],
            gain_range: (0.0, 45.0),
            gain_elements: vec![
                el("LNA", 0.0, 15.0, 1.0),
                el("MIX", 0.0, 15.0, 1.0),
                el("VGA", 0.0, 15.0, 1.0),
            ],
            has_gain_mode: true,
            bandwidth_ranges: vec![],
            native_format: "CS16".into(),
            // 12-bit converter in a 16-bit container.
            native_full_scale: 2048.0,
        }
    }

    #[test]
    fn a_soapy_hackrf_comes_out_the_way_the_probe_describes_it() {
        let c = capabilities(&soapy_hackrf()).unwrap().caps;
        assert_eq!(c.freq_min_hz, 0);
        assert_eq!(c.freq_max_hz, 7_250_000_000);
        assert_eq!(c.sample_rate_min_hz, 1e6);
        assert_eq!(
            c.sample_rate_max_hz, 20e6,
            "hull over twenty discrete rates"
        );
        assert_eq!(c.sample_geometry.format, SampleFormat::Int8);
        assert_eq!(c.sample_geometry.full_scale, 128.0);
        assert_eq!(c.sample_geometry.bits(), 8);
        assert!(c.has_bb_filter, "1.75 to 28 MHz of filter bandwidths");
        assert!(!c.friis_applicable, "we do not know a Soapy device's chain");
        assert_eq!(
            c.delivery,
            DeliveryModel::Pull,
            "readStream returns as soon as there is data, so we set the rhythm"
        );
        match c.gain {
            GainModel::Soapy {
                min_db,
                max_db,
                ref stages,
                ref boost,
            } => {
                assert_eq!((min_db, max_db), (0, 116), "LNA 40 + AMP 14 + VGA 62");
                // AMP is a switch and became the boost, so two stages are left,
                // in the order the driver listed them.
                let names: Vec<&str> = stages.iter().map(|s| s.name.as_str()).collect();
                assert_eq!(names, ["LNA", "VGA"], "listGains order, minus the switch");
                assert_eq!(stages[0].step_db, 8.0, "the driver's own grid");
                assert_eq!(stages[1].max_db, 62.0);
                match boost {
                    Some(Boost::Element(e)) => {
                        assert_eq!(e.name, "AMP");
                        assert_eq!((e.min_db, e.max_db), (0.0, 14.0));
                    }
                    other => panic!("AMP [0, 14, 14] should be the boost, got {other:?}"),
                }
            }
            other => panic!("expected a Soapy gain model, got {other:?}"),
        }
    }

    /// The number that matters most on the RF bench, and the one a rule reading
    /// only the format name would get wrong for every 12-bit radio.
    #[test]
    fn a_twelve_bit_radio_in_a_sixteen_bit_container_reports_twelve() {
        let c = capabilities(&airspy_r2()).unwrap().caps;
        assert_eq!(c.sample_geometry.format, SampleFormat::Int16);
        assert_eq!(c.sample_geometry.full_scale, 2048.0);
        assert_eq!(c.sample_geometry.bits(), 12);
    }

    /// The sound card's `full-scale=65536` in a 16-bit container. Capped, not
    /// believed. Found by probing, not by imagining.
    #[test]
    fn a_full_scale_larger_than_its_container_is_capped() {
        let c = capabilities(&built_in_audio()).unwrap().caps;
        assert_eq!(c.sample_geometry.bits(), 16);
    }

    /// A device with no gain control at all, which the sound card really is.
    /// It must not become a device whose gain bar goes to 0 and whose keys
    /// misbehave.
    #[test]
    fn a_zero_width_gain_range_is_carried_through_intact() {
        let c = capabilities(&built_in_audio()).unwrap().caps;
        match c.gain {
            GainModel::Soapy { min_db, max_db, .. } => assert_eq!((min_db, max_db), (0, 0)),
            other => panic!("{other:?}"),
        }
        assert_eq!(c.gain.primary_max_db(), 0);
        // Clamping into an empty range must return something, not panic.
        assert_eq!(c.gain.clamp_gains(30, 20), (0, 20));
    }

    /// An element the driver described unusably is refused **by name**, not
    /// kept at zero. A stage pinned at its minimum looks exactly like one the
    /// user turned down, and the log line is the only way anyone would know.
    #[test]
    fn an_unusable_element_is_named_and_dropped() {
        let mut a = soapy_hackrf();
        a.gain_elements.insert(1, el("BROKEN", 40.0, 0.0, 1.0));
        let built = capabilities(&a).unwrap();

        let names: Vec<&str> = match &built.caps.gain {
            GainModel::Soapy { stages, .. } => stages.iter().map(|s| s.name.as_str()).collect(),
            other => panic!("{other:?}"),
        };
        assert_eq!(names, ["LNA", "VGA"], "the bad one is gone");
        assert_eq!(built.notes.len(), 1);
        assert!(
            built.notes[0].contains("BROKEN"),
            "the refusal names it: {:?}",
            built.notes[0]
        );
    }

    /// A driver that names no element still has a whole-chain range, and that
    /// is exactly what this backend used before it could ask per element. One
    /// unnamed stage over it, and no note, because nothing was refused.
    #[test]
    fn a_driver_that_names_nothing_still_gets_one_stage() {
        let built = capabilities(&built_in_audio()).unwrap();
        match &built.caps.gain {
            GainModel::Soapy { stages, .. } => {
                assert_eq!(stages.len(), 1);
                assert_eq!(stages[0].step_db, 0.0, "continuous: no grid was given");
            }
            other => panic!("{other:?}"),
        }
        assert!(built.notes.is_empty(), "{:?}", built.notes);
    }

    /// Every element unusable is not the same as no elements: the fallback is
    /// the same, but it happened for a reason worth saying.
    #[test]
    fn every_element_unusable_falls_back_and_says_so() {
        let mut a = soapy_hackrf();
        a.gain_elements = vec![el("A", f64::NAN, 10.0, 1.0), el("B", 5.0, 1.0, 1.0)];
        let built = capabilities(&a).unwrap();
        match &built.caps.gain {
            GainModel::Soapy { stages, boost, .. } => {
                assert_eq!(stages.len(), 1, "the whole-chain fallback");
                assert_eq!(stages[0].max_db, 116.0);
                assert!(boost.is_none(), "no switch survived either");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            built.notes.len(),
            3,
            "two refusals and the fallback: {:?}",
            built.notes
        );
    }

    /// Only the first switch takes the key. sdrtop has one boost concept, and a
    /// second switch is still a stage it can set rather than something silently
    /// dropped.
    #[test]
    fn a_second_switch_stays_a_stage() {
        let mut a = soapy_hackrf();
        a.gain_elements.push(el("PREAMP", 0.0, 20.0, 20.0));
        let built = capabilities(&a).unwrap();
        match &built.caps.gain {
            GainModel::Soapy { stages, boost, .. } => {
                let names: Vec<&str> = stages.iter().map(|s| s.name.as_str()).collect();
                assert_eq!(names, ["LNA", "VGA", "PREAMP"]);
                assert!(matches!(boost, Some(Boost::Element(e)) if e.name == "AMP"));
            }
            other => panic!("{other:?}"),
        }
    }

    /// Two mechanisms can be a boost, and a device with neither has none.
    ///
    /// A `SoapyHackRF` reports `Supports AGC: NO`, and the first version of this
    /// backend concluded it had no boost at all. It has: `AMP [0, 14, 14]` is a
    /// two-position element, the same physical switch the native backend drives
    /// with its own key. Reading only `hasGainMode` cost that radio its amp.
    #[test]
    fn a_boost_is_either_a_gain_mode_or_a_switch() {
        let hackrf = capabilities(&soapy_hackrf()).unwrap().caps;
        assert!(hackrf.gain.has_boost(), "AMP is a two-position element");
        assert_eq!(
            hackrf.gain.boost_label(),
            "AMP",
            "labelled with the driver's own name for it"
        );

        // No elements at all, but the driver offers automatic gain control.
        assert_eq!(
            capabilities(&built_in_audio())
                .unwrap()
                .caps
                .gain
                .boost_label(),
            "AGC"
        );

        // Neither: three continuous elements and no gain mode.
        let mut plain = airspy_r2();
        plain.has_gain_mode = false;
        assert!(!capabilities(&plain).unwrap().caps.gain.has_boost());
        assert!(capabilities(&built_in_audio())
            .unwrap()
            .caps
            .gain
            .has_boost());
    }

    /// Refusing rather than guessing, with the format named so the log answers
    /// the question on its own.
    #[test]
    fn a_format_we_cannot_decode_is_refused_by_name() {
        let mut a = airspy_r2();
        a.native_format = "CF32".into();
        let err = capabilities(&a).unwrap_err();
        assert_eq!(err, Unsupported::Format("CF32".into()));
        assert!(err.to_string().contains("CF32"), "{err}");

        // The packed 12-bit formats are refused the same way, and for the same
        // reason: the pipeline is integer and byte aligned.
        a.native_format = "CS12".into();
        assert!(matches!(capabilities(&a), Err(Unsupported::Format(_))));
    }

    /// A tuner with a gap reports both sides. The hull is the outer bound, and
    /// the comment on `hull` says why that is honest rather than lazy.
    #[test]
    fn a_split_tuner_range_becomes_its_outer_bound() {
        let mut a = airspy_r2();
        // An E4000-shaped answer: two bands with a hole in the middle.
        a.freq_ranges = vec![(52e6, 1_100e6), (1_250e6, 2_200e6)];
        let c = capabilities(&a).unwrap().caps;
        assert_eq!(c.freq_min_hz, 52_000_000);
        assert_eq!(c.freq_max_hz, 2_200_000_000);
    }

    /// A driver that answers nothing at all is refused, and the two reasons are
    /// distinguishable so the log can say which.
    #[test]
    fn a_driver_that_reports_nothing_is_refused() {
        let mut a = airspy_r2();
        a.freq_ranges = vec![];
        assert_eq!(capabilities(&a).unwrap_err(), Unsupported::FrequencyRange);

        let mut a = airspy_r2();
        a.rate_ranges = vec![];
        assert_eq!(capabilities(&a).unwrap_err(), Unsupported::SampleRate);
    }

    /// Garbage in the ranges must not survive into a capability the rest of the
    /// app trusts. A NaN would poison every clamp downstream.
    #[test]
    fn nonsense_ranges_are_filtered_before_the_hull() {
        assert_eq!(hull(&[(f64::NAN, 1e6)]), None);
        assert_eq!(hull(&[(1e6, f64::INFINITY)]), None);
        assert_eq!(hull(&[(1e6, 0.0)]), None, "inverted");
        assert_eq!(hull(&[(0.0, 0.0)]), None, "nothing above zero");
        // One good entry among the rubbish is still a usable answer.
        assert_eq!(hull(&[(f64::NAN, 1.0), (10.0, 20.0)]), Some((10.0, 20.0)));
    }

    /// A nonsense full scale falls back to the container's, because dividing the
    /// whole pipeline by zero is not an option and the container width is the
    /// one thing we do know.
    #[test]
    fn a_nonsense_full_scale_falls_back_to_the_container() {
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(geometry_for("CS16", bad).unwrap().full_scale, 32768.0);
            assert_eq!(geometry_for("CS8", bad).unwrap().full_scale, 128.0);
        }
    }

    /// The startup tuning has to be legal for this device, whatever it is. A
    /// sound card cannot be opened at 100 MHz and 2.4 Msps.
    #[test]
    fn the_startup_tuning_is_clamped_into_the_devices_own_range() {
        let c = capabilities(&built_in_audio()).unwrap().caps;
        assert!(c.default_sample_rate_hz <= c.sample_rate_max_hz);
        assert!(c.default_sample_rate_hz >= c.sample_rate_min_hz);
        assert_eq!(
            c.default_sample_rate_hz, 192_000.0,
            "clamped to its ceiling"
        );

        let c = capabilities(&airspy_r2()).unwrap().caps;
        assert_eq!(c.default_frequency_hz, 100_000_000, "in range, so kept");
        assert_eq!(
            c.default_sample_rate_hz, 2_500_000.0,
            "clamped to its floor"
        );
    }
}
