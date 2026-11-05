// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! What the SDRplay RSP1B is, as plain data: its rates and how each is made,
//! its converter's depth at each, its gain stages as measured, and the
//! capabilities built from them. Nothing here touches the API.
//!
//! **Measured, not taken from the table.** SDRplay publishes an LNA table
//! for the RSP1B; on a cable from a HackRF (every state read until two reads
//! agreed to 0.1 dB, going up and coming down, at fourteen frequencies) the
//! radio reduces up to 6 dB more than that table from state 4 on between 50
//! and 420 MHz, and in L-band the shape moves with frequency. So the tables
//! below are the measurements, each band's from the frequency it was
//! measured at, and a frequency no band covers reads as a position.
//!
//! **The API accepts what it cannot do.** It takes an 8 MHz IF bandwidth at
//! 2 MSPS, a decimation of 3 (and ignores it), an IF mode no combination
//! makes (and delivers real samples, Q all zero) and 500 Hz, without an error.
//! So every rate here is made one way, chosen here, and nothing else is ever
//! asked of it.

use crate::hardware::{
    AcquisitionKind, BandTable, Boost, DeliveryModel, DeviceCapabilities, GainModel, LevelUnit,
    SampleFormat, SampleGeometry, StageSpec, IQ_TRACE_STALE_MS,
};

/// The lowest and highest frequency SDRplay states for the RSP1B. The API
/// itself takes 500 Hz without complaint and refuses 2.5 GHz; the
/// datasheet's span is what is offered.
pub const FREQ_MIN_HZ: u64 = 1_000;
pub const FREQ_MAX_HZ: u64 = 2_000_000_000;

/// The ADC's rate range: `sdrplay_api_Init` refuses anything outside it
/// ("fsHz out of range (2000000:10000000)"), whatever the datasheet's 10.66.
pub const FS_MIN_HZ: f64 = 2_000_000.0;
pub const FS_MAX_HZ: f64 = 10_000_000.0;

/// How the radio is set up to deliver one output rate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RateMode {
    /// The ADC's rate, Hz (`fsHz`).
    pub fs_hz: f64,
    /// The API's decimation factor, 1 for none (2, 4, 8, 16 or 32 otherwise:
    /// the API refuses 64 and silently ignores 3).
    pub decimation: u8,
    /// The low IF, kHz, or 0 for zero IF (`ifType`).
    pub if_khz: u32,
    /// The IF filter, kHz (`bwType`): never wider than the output rate,
    /// except the 200 kHz filter, the narrowest there is, under a decimated
    /// rate, whose own filter then does the rest.
    pub bw_khz: u32,
}

impl RateMode {
    /// What the samples arrive at, Hz.
    pub fn output_hz(&self) -> f64 {
        if self.if_khz == 2048 {
            // 8.192 MSPS at a 2.048 MHz IF, down-converted by the API.
            self.fs_hz / 4.0
        } else {
            self.fs_hz / f64::from(self.decimation)
        }
    }
}

/// The IF filters the API has, kHz (`sdrplay_api_Bw_MHzT`).
const BANDWIDTHS_KHZ: [u32; 8] = [200, 300, 600, 1536, 5000, 6000, 7000, 8000];

/// The widest IF filter that fits in `rate_hz`, or the narrowest there is.
fn bandwidth_for(rate_hz: f64) -> u32 {
    BANDWIDTHS_KHZ
        .iter()
        .rev()
        .copied()
        .find(|&bw| f64::from(bw) * 1_000.0 <= rate_hz)
        .unwrap_or(BANDWIDTHS_KHZ[0])
}

/// How the radio makes the output rate `rate_hz`, or `None` for a rate it
/// cannot make honestly.
///
/// 2 to 10 MSPS: the ADC at that rate, zero IF. Under 2 MSPS: the ADC at 2
/// MSPS and the API's decimation, by a power of two up to 32. 2.048 MSPS:
/// the one rate there is no zero-IF way to, made as SDRplay documents it,
/// 8.192 MSPS at a 2.048 MHz low IF with the 1.536 MHz filter (measured at
/// 2.048 MS/s, its LO outside the delivered band).
pub fn mode_for(rate_hz: f64) -> Option<RateMode> {
    if (rate_hz - 2_048_000.0).abs() < 0.5 {
        return Some(RateMode {
            fs_hz: 8_192_000.0,
            decimation: 1,
            if_khz: 2048,
            bw_khz: 1536,
        });
    }
    if (FS_MIN_HZ..=FS_MAX_HZ).contains(&rate_hz) {
        return Some(RateMode {
            fs_hz: rate_hz,
            decimation: 1,
            if_khz: 0,
            bw_khz: bandwidth_for(rate_hz),
        });
    }
    [2u8, 4, 8, 16, 32]
        .into_iter()
        .find(|&d| (FS_MIN_HZ / f64::from(d) - rate_hz).abs() < 0.5)
        .map(|decimation| RateMode {
            fs_hz: FS_MIN_HZ,
            decimation,
            if_khz: 0,
            bw_khz: bandwidth_for(rate_hz),
        })
}

/// The rates offered: every decimation of 2 MSPS, the ADC from 2 to 10 MSPS
/// in whole MSPS, and 2.048 MSPS.
pub const RATES_HZ: [f64; 15] = [
    62_500.0,
    125_000.0,
    250_000.0,
    500_000.0,
    1_000_000.0,
    2_000_000.0,
    2_048_000.0,
    3_000_000.0,
    4_000_000.0,
    5_000_000.0,
    6_000_000.0,
    7_000_000.0,
    8_000_000.0,
    9_000_000.0,
    10_000_000.0,
];

/// The bits the ADC delivers at `fs_hz`, as the samples show them: with the
/// API's corrections off they step by 4 counts (14 bits) up to 6.0 MSPS, by
/// 16 (12 bits) from 6.048 to 9.0, by 256 (8 bits) from 9.216 MSPS, both
/// boundaries belonging to the higher rate. SDRplay's datasheet gives 10
/// bits from 8.064 to 9.216 MSPS; the samples there step by 16 as at 7 MSPS,
/// so they carry 12, and 12 is what is said.
pub fn converter_bits(fs_hz: f64) -> u8 {
    if fs_hz < 6_048_000.0 {
        14
    } else if fs_hz < 9_216_000.0 {
        12
    } else {
        8
    }
}

/// The LNA states' gain, dB above each band's least-gain state, measured on
/// a cable at the frequency named for each band. SDRplay states no absolute
/// gain for the RSP1B, so a stage's zero is its least setting in the band.
/// Under 50 MHz the table is the mean of 5, 14 and 40 MHz, which agree to
/// about 1 dB; the front end's low-pass path under 2 MHz was not measured,
/// so nothing is said there.
fn lna_bands() -> Vec<BandTable> {
    let band = |lo: u64, hi: u64, db: &[f64]| BandTable {
        band: lo..=hi,
        db: db.to_vec(),
    };
    vec![
        // 5, 14 and 40 MHz: 7 states.
        band(
            2_000_000,
            49_999_999,
            &[61.04, 55.07, 48.76, 42.78, 23.93, 18.82, 0.0],
        ),
        // 55 MHz.
        band(
            50_000_000,
            59_999_999,
            &[
                68.05, 61.66, 55.27, 49.26, 43.09, 37.52, 31.3, 25.24, 6.42, 0.0,
            ],
        ),
        // 100 MHz (the 60-120 MHz filter).
        band(
            60_000_000,
            119_999_999,
            &[
                66.34, 60.24, 53.89, 47.92, 41.47, 35.57, 29.23, 23.19, 4.31, 0.0,
            ],
        ),
        // 150 MHz (120-250).
        band(
            120_000_000,
            249_999_999,
            &[
                67.75, 61.65, 55.33, 49.32, 43.13, 36.5, 29.87, 23.75, 4.89, 0.0,
            ],
        ),
        // 300 MHz (250-300 and 300-380).
        band(
            250_000_000,
            379_999_999,
            &[
                65.97, 59.38, 53.3, 47.07, 42.43, 36.55, 30.27, 24.55, 5.97, 0.0,
            ],
        ),
        // 400 MHz (380-420).
        band(
            380_000_000,
            419_999_999,
            &[
                66.91, 60.93, 54.6, 48.55, 43.93, 37.57, 31.1, 24.99, 6.1, 0.0,
            ],
        ),
        // 450 MHz.
        band(
            420_000_000,
            574_999_999,
            &[
                68.29, 60.89, 54.62, 48.19, 45.11, 37.68, 31.64, 25.14, 18.98, 0.0,
            ],
        ),
        // 700 MHz.
        band(
            575_000_000,
            824_999_999,
            &[
                67.46, 59.53, 53.55, 47.13, 46.12, 38.23, 31.58, 25.1, 18.81, 0.0,
            ],
        ),
        // 950 MHz.
        band(
            825_000_000,
            999_999_999,
            &[
                67.38, 59.73, 53.44, 46.81, 45.44, 37.83, 32.07, 25.4, 18.71, 0.0,
            ],
        ),
        // 1200 MHz: 9 states from here up.
        band(
            1_000_000_000,
            1_399_999_999,
            &[60.32, 53.88, 47.13, 39.55, 33.75, 26.98, 20.12, 19.13, 0.0],
        ),
        // 1600 MHz.
        band(
            1_400_000_000,
            1_749_999_999,
            &[59.35, 52.77, 45.83, 43.54, 36.33, 29.27, 22.36, 19.05, 0.0],
        ),
        // 1900 MHz.
        band(
            1_750_000_000,
            FREQ_MAX_HZ,
            &[59.33, 52.16, 45.0, 44.3, 36.64, 29.8, 23.12, 19.02, 0.0],
        ),
    ]
}

/// The IF stage's gain, dB above gRdB 59, by gRdB from 59 down to 20, as
/// measured at 150 MHz (the mean of four sweeps; 14 and 950 MHz give the
/// same staircase to 0.3 dB): about 1 dB a step, every sixth step 1.25.
pub const IF_DB: [f64; 40] = [
    0.0, 1.02, 2.05, 3.1, 3.98, 4.92, 6.13, 7.07, 8.07, 9.11, 9.98, 10.95, 12.2, 13.15, 14.12,
    15.14, 16.0, 16.94, 18.2, 19.14, 20.12, 21.12, 21.98, 22.95, 24.16, 25.13, 26.09, 27.11, 27.98,
    28.93, 30.18, 31.13, 32.1, 33.11, 33.97, 34.93, 36.21, 37.13, 38.13, 39.13,
];

/// The gRdB the API takes for an IF gain from [`IF_DB`]: its position counted
/// down from 59.
pub fn grdb_for(if_db: f64) -> i32 {
    let at = IF_DB
        .iter()
        .enumerate()
        .min_by(|a, b| (a.1 - if_db).abs().total_cmp(&(b.1 - if_db).abs()))
        .map_or(0, |(i, _)| i);
    59 - at as i32
}

/// The two stages, front to back: the LNA, stored as the radio's state with
/// its dB per band, and the IF, in dB from its measured table. The API's AGC
/// moves the IF only (measured: the LNA's reduction stayed put in every
/// run), so it is the boost key's and governs the IF.
pub fn gain_model() -> GainModel {
    GainModel::new(
        vec![
            StageSpec::banded("LNA", 10, lna_bands()),
            StageSpec::tabled("IF", IF_DB.to_vec()),
        ],
        "LNA",
        "LNA",
    )
    .with_second_stage()
    .with_no_cascade_reason("each stage's own noise figure is not published")
    .with_boost(Boost::GainMode)
    .with_auto_stages(&[1])
    // A start in the middle of both, until the knob's curves say otherwise.
    .with_start_gains(vec![4.0, IF_DB[20]])
}

/// What an RSP1B can do.
pub fn caps() -> DeviceCapabilities {
    DeviceCapabilities {
        level_unit: LevelUnit::Dbfs,
        level_min_db: -150.0,
        level_max_db: 0.0,
        trace_stale_ms: IQ_TRACE_STALE_MS,
        acquisition: AcquisitionKind::IqSamples,
        sample_rate_is_span: false,
        freq_min_hz: FREQ_MIN_HZ,
        freq_max_hz: FREQ_MAX_HZ,
        freq_gaps: Vec::new(),
        sample_rate_min_hz: RATES_HZ[0],
        sample_rate_max_hz: RATES_HZ[RATES_HZ.len() - 1],
        sample_rates: RATES_HZ.to_vec(),
        default_frequency_hz: 100_000_000,
        default_sample_rate_hz: 2_000_000.0,
        // Left-aligned in int16: an overloaded capture spans -32768 to
        // +32764 (8191 times 4). The depth here is the best the ADC gives;
        // the device says the depth at the rate it runs.
        sample_geometry: SampleGeometry {
            format: SampleFormat::Int16,
            full_scale: 32_768.0,
            converter_bits: Some(14),
        },
        gain: gain_model(),
        // Deliveries are gathered from the API's blocks by the stream; the
        // device says how long once it streams.
        samples_per_transfer: 0,
        has_bb_filter: false,
        // No stage's own noise figure is published, so no cascade.
        friis_applicable: false,
        // The service passes the stream on in bursts (see the variant).
        delivery: DeliveryModel::Relayed,
        // The queue is inside SDRplay's service, which does not say.
        driver_queue_blocks: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every offered rate is made one way, and each way is one the API does
    /// what it says with: zero IF at 2 to 10 MSPS, decimation of 2 MSPS by a
    /// power of two, and 2.048 MSPS at the 2.048 MHz low IF.
    #[test]
    fn every_rate_has_one_honest_mode() {
        for rate in RATES_HZ {
            let m = mode_for(rate).unwrap_or_else(|| panic!("{rate}"));
            assert!((m.output_hz() - rate).abs() < 0.5, "{rate}: {m:?}");
            assert!((FS_MIN_HZ..=FS_MAX_HZ).contains(&m.fs_hz), "{m:?}");
            assert!([1, 2, 4, 8, 16, 32].contains(&m.decimation), "{m:?}");
            assert!(BANDWIDTHS_KHZ.contains(&m.bw_khz), "{m:?}");
            assert!(
                f64::from(m.bw_khz) * 1_000.0 <= rate.max(200_000.0),
                "a filter no wider than the rate: {m:?}"
            );
        }
        assert_eq!(
            mode_for(2_048_000.0),
            Some(RateMode {
                fs_hz: 8_192_000.0,
                decimation: 1,
                if_khz: 2048,
                bw_khz: 1536
            })
        );
        assert_eq!(mode_for(250_000.0).unwrap().decimation, 8);
        assert_eq!(mode_for(10_000_000.0).unwrap().bw_khz, 8000);
        assert_eq!(mode_for(6_000_000.0).unwrap().bw_khz, 6000);
        assert_eq!(mode_for(2_500_000.0).unwrap().bw_khz, 1536);
    }

    /// What the API would take and not do is never asked: a decimation of
    /// 3, a rate past 10 MSPS, a rate under 2 MSPS no power of two makes.
    #[test]
    fn a_rate_the_radio_cannot_make_honestly_is_refused() {
        assert_eq!(mode_for(666_667.0), None, "a decimation of 3");
        assert_eq!(mode_for(10_660_000.0), None, "the datasheet's top");
        assert_eq!(mode_for(1_500_000.0), None);
        assert_eq!(mode_for(31_250.0), None, "a decimation of 64");
    }

    /// The depth by the ADC's rate, as its samples' quantum showed it, both
    /// boundaries with the higher rate; 12 bits where the datasheet says 10.
    #[test]
    fn the_depth_is_the_samples_by_rate() {
        assert_eq!(converter_bits(2_000_000.0), 14);
        assert_eq!(converter_bits(6_000_000.0), 14);
        assert_eq!(converter_bits(6_048_000.0), 12);
        assert_eq!(converter_bits(8_192_000.0), 12, "the 2.048 MHz low IF");
        assert_eq!(converter_bits(8_500_000.0), 12, "not the datasheet's 10");
        assert_eq!(converter_bits(9_216_000.0), 8);
        assert_eq!(converter_bits(10_000_000.0), 8);
    }

    /// The LNA is the radio's state with its band's measured dB: state 4 at
    /// 150 MHz 43.13 dB above state 9, at 14 MHz 23.93 above state 6, the L-band
    /// shape apart at 1200 and 1900 MHz; no band under 2 MHz or past 2 GHz.
    #[test]
    fn the_lna_is_the_measured_tables_by_band() {
        let lna = &gain_model().stages()[0];
        assert!(lna.is_banded());
        assert_eq!(lna.positions(), Some(10));
        assert_eq!(lna.gain_db_at(4.0, 150_000_000), Some(43.13));
        assert_eq!(lna.gain_db_at(4.0, 14_000_000), Some(23.93));
        assert_eq!(lna.gain_db_at(9.0, 14_000_000), Some(0.0), "state 6 there");
        assert_eq!(lna.gain_db_at(3.0, 1_200_000_000), Some(39.55));
        assert_eq!(lna.gain_db_at(3.0, 1_900_000_000), Some(44.3));
        assert_eq!(
            lna.gain_db_at(9.0, 1_900_000_000),
            Some(0.0),
            "8 states there"
        );
        assert_eq!(lna.gain_db_at(4.0, 1_000_000), None);
        // Every band is in gain order from state 0 down, ends at 0 dB, and
        // the bands meet with no gap from 2 MHz to 2 GHz.
        let mut next = 2_000_000;
        for b in &lna.banded {
            assert_eq!(*b.band.start(), next, "{b:?}");
            next = b.band.end() + 1;
            assert!(b.db.windows(2).all(|w| w[0] > w[1]), "{b:?}");
            assert_eq!(b.db.last(), Some(&0.0));
        }
        assert_eq!(next, FREQ_MAX_HZ + 1);
    }

    /// The IF in dB from its measured staircase, and back to the gRdB the API
    /// takes: 0 dB is gRdB 59, the top 20.
    #[test]
    fn the_if_is_its_measured_staircase_and_back_to_grdb() {
        let if_stage = &gain_model().stages()[1];
        assert_eq!(if_stage.positions(), Some(40));
        assert_eq!(grdb_for(0.0), 59);
        assert_eq!(grdb_for(39.13), 20);
        assert_eq!(grdb_for(IF_DB[20]), 39);
        assert!(IF_DB.windows(2).all(|w| w[1] > w[0]));
    }

    /// **No cascade is modelled**, so nothing behind `friis_applicable` (the
    /// HackRF's cascade, which takes a stored value as a dB) can see the
    /// banded LNA; the AGC governs the IF only.
    #[test]
    fn the_capabilities_model_no_cascade_and_an_if_agc() {
        let c = caps();
        assert!(!c.friis_applicable);
        assert!(c.gain.auto_governs(1));
        assert!(!c.gain.auto_governs(0));
        assert_eq!(c.sample_geometry.bits(), 14);
        assert_eq!((c.freq_min_hz, c.freq_max_hz), (1_000, 2_000_000_000));
        assert_eq!(c.delivery, DeliveryModel::Relayed);
    }
}
