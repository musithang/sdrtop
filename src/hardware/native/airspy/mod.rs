// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The Airspy R2, driven through libairspy.
//!
//! Found, identified, opened and described here; it is reached only when
//! `--device airspy` asks for it by name, and until it can stream, starting
//! RX says so and points at the SoapySDR path, which keeps offering the radio
//! as before.
//!
//! **Its gains are measured, not read off a datasheet.** libairspy takes
//! register positions, 0..14 or 0..15, and says nothing about decibels; the
//! tables in [`gain_model`] are what each position gave on this bench, with
//! the LNA's valid only over the VHF band it was measured in (see
//! `user_docs/hardware.md`).
//!
//! **The R2, and only the R2.** The Airspy Mini answers on the same USB id
//! and board id; the rates its firmware lists tell them apart (R2: 10 and 2.5
//! Msps; Mini: 6 and 3). A Mini is named and not taken: nobody here has held
//! one, and its own rates and gains would be guesses.

pub mod ffi;

use std::ffi::CStr;

use libc::{c_char, c_int, c_void};

use std::sync::Arc;

use crate::hardware::traits::RateSet;
use crate::hardware::{
    AcquisitionKind, DeliveryModel, DeviceCapabilities, DeviceInfo, DeviceKind, DeviceListing,
    GainModel, LevelUnit, RxContext, SampleFormat, SampleGeometry, SdrDevice, StageSpec,
    IQ_TRACE_STALE_MS,
};

/// Which Airspy a firmware is, by the rates it offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Model {
    R2,
    Mini,
    /// Rates neither of the above lists.
    Unknown,
}

/// The model the firmware's rate list (complex rates, Hz) belongs to, in any
/// order.
pub fn model_from_rates(rates: &[u32]) -> Model {
    let mut sorted = rates.to_vec();
    sorted.sort_unstable();
    match sorted.as_slice() {
        [2_500_000, 10_000_000] => Model::R2,
        [3_000_000, 6_000_000] => Model::Mini,
        _ => Model::Unknown,
    }
}

/// What libairspy said about one radio.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    /// As SoapyAirspy spells it: lowercase hex, no prefix.
    pub serial: String,
    /// The board's name as libairspy gives it, when the board could be read.
    pub board: Option<String>,
    pub firmware: Option<String>,
    pub rates: Vec<u32>,
    pub model: Model,
}

impl Model {
    fn name(self) -> &'static str {
        match self {
            Model::R2 => "Airspy R2",
            Model::Mini => "Airspy Mini",
            Model::Unknown => "Airspy",
        }
    }
}

/// Every Airspy on the bus, each opened just long enough to identify it. A
/// radio that will not open (another app holding it) is left out rather than
/// described from nothing.
pub fn find(api: &ffi::AirspyApi) -> Vec<Found> {
    // SAFETY: a null buffer asks only for the count.
    let count = unsafe { (api.airspy_list_devices)(std::ptr::null_mut(), 0) };
    let Ok(count) = usize::try_from(count) else {
        return Vec::new();
    };
    let mut serials = vec![0u64; count];
    // SAFETY: the buffer holds `count` serials, and the call writes at most
    // that many.
    let got = unsafe {
        (api.airspy_list_devices)(serials.as_mut_ptr(), c_int::try_from(count).unwrap_or(0))
    };
    serials.truncate(usize::try_from(got).unwrap_or(0));
    serials
        .into_iter()
        .filter_map(|serial| identify(api, serial))
        .collect()
}

/// One radio, opened by its serial, read and closed.
fn identify(api: &ffi::AirspyApi, serial: u64) -> Option<Found> {
    let mut device: *mut c_void = std::ptr::null_mut();
    // SAFETY: libairspy writes the handle it opened, or fails.
    if unsafe { (api.airspy_open_sn)(&mut device, serial) } != 0 || device.is_null() {
        return None;
    }
    // SAFETY: `device` was opened just above and is closed once read.
    let found = unsafe { read_found(api, device, serial) };
    // SAFETY: as above.
    unsafe { (api.airspy_close)(device) };
    Some(found)
}

/// What an open radio says about itself.
///
/// # Safety
/// `device` must be a handle libairspy opened and has not closed.
unsafe fn read_found(api: &ffi::AirspyApi, device: *mut c_void, serial: u64) -> Found {
    // SAFETY: each call writes only into the buffer it is given, of the size
    // it is given.
    unsafe {
        let mut id = 0u8;
        let board = ((api.airspy_board_id_read)(device, &mut id) == 0)
            .then(|| c_string((api.airspy_board_id_name)(c_int::from(id))))
            .flatten();
        let mut text = [0 as c_char; 128];
        let firmware = ((api.airspy_version_string_read)(device, text.as_mut_ptr(), 128) == 0)
            .then(|| c_string(text.as_ptr()))
            .flatten()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
        let mut n = 0u32;
        let rates = if (api.airspy_get_samplerates)(device, &mut n, 0) == 0 && n > 0 {
            let mut rates = vec![0u32; n as usize];
            if (api.airspy_get_samplerates)(device, rates.as_mut_ptr(), n) == 0 {
                rates
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };
        Found {
            serial: format!("{serial:x}"),
            board,
            firmware,
            model: model_from_rates(&rates),
            rates,
        }
    }
}

/// The device selector's entries for what [`find`] returned.
pub fn list() -> Vec<DeviceListing> {
    let Ok(api) = ffi::api() else {
        return Vec::new();
    };
    find(api)
        .into_iter()
        .enumerate()
        .map(|(index, found)| DeviceListing {
            kind: DeviceKind::Airspy,
            index,
            label: label(&found),
            serial: Some(found.serial),
            args: None,
            path: None,
            tiny_sa_input: None,
        })
        .collect()
}

fn label(found: &Found) -> String {
    let base = format!("{} · {}", found.model.name(), found.serial);
    match found.model {
        Model::R2 => base,
        Model::Mini => format!("{base} · not driven natively, use soapy=driver=airspy"),
        Model::Unknown => format!("{base} · rates not recognised, use soapy=driver=airspy"),
    }
}

/// The rates the R2's firmware offers, complex samples per second.
const R2_RATES: [u32; 2] = [2_500_000, 10_000_000];

/// The R2's three stages as this bench measured them: what each register
/// position gave, in dB above position 0, from back-to-back differences
/// against steady carriers with the other stages held (FM 92.8 MHz, 465 MHz,
/// GSM 943.4 MHz). The mixer and VGA agree within half a dB across those; the
/// LNA's gain falls with frequency (36 dB in all at 93 MHz, about 31 at
/// 929 MHz), so its table is the 92.8 MHz one, checked at 120 and 168 MHz,
/// and holds from 88 to 170 MHz only. The mixer's positions 13 to 15 add
/// nothing measurable; they are kept, because they are settings the radio
/// has, and they read as the same dB.
pub fn gain_model() -> GainModel {
    GainModel::new(
        vec![
            StageSpec::tabled(
                "LNA",
                vec![
                    0.0, 4.38, 9.09, 11.28, 11.75, 13.52, 16.74, 19.96, 23.40, 26.96, 27.76, 28.19,
                    28.64, 32.34, 36.02,
                ],
            )
            .measured_between(88_000_000, 170_000_000),
            StageSpec::tabled(
                "MIX",
                vec![
                    0.0, 1.14, 2.59, 4.34, 5.65, 6.94, 8.25, 9.41, 10.53, 12.03, 13.37, 14.67,
                    15.90, 16.16, 16.17, 16.24,
                ],
            ),
            StageSpec::tabled(
                "VGA",
                vec![
                    0.0, 3.40, 6.80, 10.16, 13.64, 16.91, 20.40, 23.52, 26.79, 30.17, 33.51, 36.86,
                    40.20, 43.65, 46.89, 49.96,
                ],
            ),
        ],
        "LNA",
        "LNA",
    )
    .with_second_stage()
    .with_no_cascade_reason("each stage's own noise figure is not published")
}

/// What an R2 can do, from its own firmware's rates and the bench.
pub fn caps() -> DeviceCapabilities {
    DeviceCapabilities {
        level_unit: LevelUnit::Dbfs,
        level_min_db: -120.0,
        level_max_db: 0.0,
        trace_stale_ms: IQ_TRACE_STALE_MS,
        acquisition: AcquisitionKind::IqSamples,
        sample_rate_is_span: false,
        // Airspy's own figures for the R2.
        freq_min_hz: 24_000_000,
        freq_max_hz: 1_800_000_000,
        sample_rate_min_hz: R2_RATES[0] as f64,
        sample_rate_max_hz: R2_RATES[1] as f64,
        default_frequency_hz: 100_000_000,
        default_sample_rate_hz: 10_000_000.0,
        // libairspy shifts the 12-bit samples up by four and filters them into
        // IQ at about half amplitude: a full-scale converter sine arrives as
        // ~16384 counts (modelled line for line from `iqconverter_int16.c`).
        sample_geometry: SampleGeometry {
            format: SampleFormat::Int16,
            full_scale: 16384.0,
            converter_bits: Some(12),
        },
        gain: gain_model(),
        // An unpacked USB buffer is 262144 bytes of 16-bit real samples,
        // halved by the IQ conversion: 65536 pairs per callback.
        samples_per_transfer: 65_536,
        has_bb_filter: false,
        friis_applicable: false,
        delivery: DeliveryModel::Push,
    }
}

/// An open Airspy R2.
pub struct AirspyDevice {
    api: &'static ffi::AirspyApi,
    ptr: *mut c_void,
    caps: DeviceCapabilities,
    info: DeviceInfo,
    notes: Vec<String>,
}

// Safety: the handle is only used through libairspy, which serialises its
// own control transfers, from the control path; the streaming thread it will
// start is its own.
unsafe impl Send for AirspyDevice {}
unsafe impl Sync for AirspyDevice {}

impl AirspyDevice {
    /// Opens the radio a listing names, by its serial.
    pub fn open(listing: &DeviceListing) -> anyhow::Result<Self> {
        Self::open_with_api(ffi::api()?, listing)
    }

    fn open_with_api(
        api: &'static ffi::AirspyApi,
        listing: &DeviceListing,
    ) -> anyhow::Result<Self> {
        let serial = listing
            .serial
            .as_deref()
            .and_then(|s| u64::from_str_radix(s, 16).ok())
            .ok_or_else(|| anyhow::anyhow!("{}: no serial to open it by", listing.label))?;
        let mut ptr: *mut c_void = std::ptr::null_mut();
        // SAFETY: libairspy writes the handle it opened, or fails.
        let code = unsafe { (api.airspy_open_sn)(&mut ptr, serial) };
        if code != 0 || ptr.is_null() {
            // SAFETY: returns a static string for any code.
            let name = c_string(unsafe { (api.airspy_error_name)(code) })
                .unwrap_or_else(|| code.to_string());
            anyhow::bail!("{}: libairspy could not open it ({name})", listing.label);
        }
        // From here the handle is the device's, and its Drop closes it.
        let mut device = Self {
            api,
            ptr,
            caps: caps(),
            info: DeviceInfo {
                board_name: Model::R2.name().to_string(),
                serial: format!("{serial:x}"),
                fw_version: None,
                board_rev: None,
                usb_api_version: None,
                tuner_name: Some("R820T2".to_string()),
                stack: None,
            },
            notes: Vec::new(),
        };
        // SAFETY: the handle is open and owned by `device`.
        let found = unsafe { read_found(api, ptr, serial) };
        if found.model != Model::R2 {
            anyhow::bail!(
                "{}: only the Airspy R2 is driven natively; open this one with \
                 --device soapy=driver=airspy",
                label(&found)
            );
        }
        device.info.fw_version = found.firmware.as_deref().map(firmware_version);
        // Shown where a HackRF shows its firmware, under the R2's own label,
        // so nothing has to guess whose firmware a version string is.
        device.info.stack =
            device
                .info
                .fw_version
                .as_deref()
                .map(|fw| crate::hardware::SoftwareStack {
                    label: "airspy fw ",
                    value: Arc::from(fw),
                });
        // What the firmware would otherwise decide for itself, stated: IQ
        // samples, both AGCs off so the stages are what the keys set, and the
        // bias tee off whatever the last program left it at.
        device.check(
            // SAFETY: the handle is open.
            unsafe { (api.airspy_set_sample_type)(ptr, ffi::SAMPLE_INT16_IQ) },
            "set the sample type",
        )?;
        device.check(
            unsafe { (api.airspy_set_rf_bias)(ptr, 0) },
            "switch the bias tee off",
        )?;
        device.check(
            unsafe { (api.airspy_set_lna_agc)(ptr, 0) },
            "switch the LNA AGC off",
        )?;
        device.check(
            unsafe { (api.airspy_set_mixer_agc)(ptr, 0) },
            "switch the mixer AGC off",
        )?;
        let mut v = ffi::LibVersion::default();
        // SAFETY: fills the struct it is given.
        unsafe { (api.airspy_lib_version)(&mut v) };
        device.notes = vec![
            format!(
                "Airspy R2: libairspy {}.{}.{}",
                v.major, v.minor, v.revision
            ),
            "Airspy R2: tuner AGC off and bias tee off, set at open".to_string(),
        ];
        Ok(device)
    }

    /// A libairspy call's result as an error naming what was being done.
    fn check(&self, code: c_int, doing: &str) -> anyhow::Result<()> {
        if code == 0 {
            Ok(())
        } else {
            anyhow::bail!("Airspy R2: could not {doing} ({})", self.error(code))
        }
    }

    /// Why RX cannot start yet, and the road that works.
    fn start_rx_refusal(&self) -> String {
        "The Airspy R2's own backend opens and describes the radio but cannot stream \
         yet. For samples, open it with --device soapy=driver=airspy"
            .to_string()
    }

    /// libairspy's name for an error code.
    fn error(&self, code: c_int) -> String {
        // SAFETY: returns a static string for any code.
        c_string(unsafe { (self.api.airspy_error_name)(code) }).unwrap_or_else(|| code.to_string())
    }
}

impl SdrDevice for AirspyDevice {
    fn capabilities(&self) -> &DeviceCapabilities {
        &self.caps
    }

    fn info(&self) -> DeviceInfo {
        self.info.clone()
    }

    fn start_rx(&self, ctx: Arc<RxContext>) -> anyhow::Result<()> {
        let _ = ctx;
        anyhow::bail!(self.start_rx_refusal())
    }

    fn stop_rx(&self) -> anyhow::Result<()> {
        Ok(())
    }

    fn is_streaming(&self) -> bool {
        false
    }

    fn set_frequency(&self, hz: u64) -> anyhow::Result<()> {
        if !(self.caps.freq_min_hz..=self.caps.freq_max_hz).contains(&hz) {
            anyhow::bail!(
                "the Airspy R2 tunes 24 MHz to 1.8 GHz, not {} MHz",
                hz / 1_000_000
            );
        }
        // SAFETY: the handle is open; the range check keeps `hz` inside u32.
        self.check(
            unsafe { (self.api.airspy_set_freq)(self.ptr, hz as u32) },
            "tune",
        )
    }

    /// One of the two rates the firmware offers, set as asked: libairspy has
    /// no getter to read it back with.
    fn set_sample_rate(&self, hz: f64) -> anyhow::Result<RateSet> {
        let Some(&rate) = R2_RATES.iter().find(|&&r| (f64::from(r) - hz).abs() < 0.5) else {
            anyhow::bail!("the Airspy R2 runs at 2.5 or 10 Msps");
        };
        // SAFETY: the handle is open.
        self.check(
            unsafe { (self.api.airspy_set_samplerate)(self.ptr, rate) },
            "set the rate",
        )?;
        Ok(RateSet::new(f64::from(rate), None, 0))
    }

    fn set_lna_gain(&self, db: u32) -> anyhow::Result<()> {
        self.set_stage_gain(0, "LNA", f64::from(db))
    }

    /// The stage's register position for `db`, by its measured table.
    fn set_stage_gain(&self, index: usize, _name: &str, db: f64) -> anyhow::Result<()> {
        let stages = self.caps.gain.stages();
        let Some(position) = stages.get(index).and_then(|s| s.position_of(db)) else {
            anyhow::bail!("the Airspy R2 has no gain stage {index}");
        };
        let position = u8::try_from(position).unwrap_or(u8::MAX);
        // SAFETY: the handle is open; positions come from the stage's own table.
        let code = unsafe {
            match index {
                0 => (self.api.airspy_set_lna_gain)(self.ptr, position),
                1 => (self.api.airspy_set_mixer_gain)(self.ptr, position),
                _ => (self.api.airspy_set_vga_gain)(self.ptr, position),
            }
        };
        self.check(code, "set the gain")
    }

    fn open_notes(&self) -> &[String] {
        &self.notes
    }
}

impl Drop for AirspyDevice {
    fn drop(&mut self) {
        // SAFETY: the handle was opened in `open` and is closed once, here.
        unsafe { (self.api.airspy_close)(self.ptr) };
    }
}

/// The version in the firmware's own string: `v1.0.0-rc10-0-g946184a` from
/// `AirSpy NOS v1.0.0-rc10-0-g946184a 2016-09-19`, or the whole string when
/// there is no such word in it.
fn firmware_version(text: &str) -> String {
    text.split_whitespace()
        .find(|w| w.starts_with('v') && w.contains('.'))
        .unwrap_or(text)
        .to_string()
}

fn c_string(ptr: *const c_char) -> Option<String> {
    // SAFETY: a NUL-terminated string libairspy returned or wrote.
    (!ptr.is_null()).then(|| {
        unsafe { CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned()
    })
}

#[cfg(test)]
mod tests {
    use super::super::test_support::TestLibrary;
    use super::*;

    /// Each model by the rates its firmware lists, whatever their order; a
    /// list that is neither is no model of ours.
    #[test]
    fn a_model_is_told_by_its_rates() {
        assert_eq!(model_from_rates(&[10_000_000, 2_500_000]), Model::R2);
        assert_eq!(model_from_rates(&[2_500_000, 10_000_000]), Model::R2);
        assert_eq!(model_from_rates(&[6_000_000, 3_000_000]), Model::Mini);
        assert_eq!(model_from_rates(&[10_000_000]), Model::Unknown);
        assert_eq!(model_from_rates(&[]), Model::Unknown);
    }

    #[test]
    fn loader_rejects_missing_required_symbol() {
        let fixture = TestLibrary::new(true);
        let err = match super::super::loader::load("libairspy", &[fixture.path()], ffi::resolve) {
            Ok(_) => panic!("accepted an incomplete libairspy"),
            Err(err) => err,
        };
        assert!(err.contains(fixture.path()));
        assert!(
            err.contains("missing required symbol airspy_get_samplerates"),
            "{err}"
        );
    }

    /// An R2 is found with its serial spelled as SoapyAirspy spells it, so
    /// the same radio reached both ways is one radio; its board, firmware
    /// and rates as the library reads them; and it is closed again.
    #[test]
    fn an_r2_is_found_and_identified() {
        let fixture = TestLibrary::new(false);
        let api = ffi::resolve(fixture.library()).unwrap();
        let found = find(&api);
        assert_eq!(
            found,
            vec![Found {
                serial: "637862dc2e1182d7".into(),
                board: Some("AIRSPY".into()),
                firmware: Some("AirSpy NOS v1.0.0-rc10-0-g946184a 2016-09-19".into()),
                rates: vec![10_000_000, 2_500_000],
                model: Model::R2,
            }]
        );
        assert_eq!(fixture.calls(3), 1, "opened to identify it, then closed");
    }

    /// A Mini is named as one; a radio another app holds is left out; no
    /// radio is no radio.
    #[test]
    fn a_mini_is_named_and_a_held_radio_left_out() {
        let fixture = TestLibrary::new(false);
        let api = ffi::resolve(fixture.library()).unwrap();
        fixture.mode(20);
        assert_eq!(find(&api)[0].model, Model::Mini);
        fixture.mode(21);
        assert_eq!(find(&api), vec![]);
        fixture.mode(22);
        assert_eq!(find(&api), vec![]);
    }

    /// The selector says which Airspy it is and by which serial; a Mini says
    /// it is not driven natively.
    #[test]
    fn the_label_names_the_model_and_serial() {
        let r2 = Found {
            serial: "637862dc2e1182d7".into(),
            board: Some("AIRSPY".into()),
            firmware: None,
            rates: vec![10_000_000, 2_500_000],
            model: Model::R2,
        };
        assert_eq!(label(&r2), "Airspy R2 · 637862dc2e1182d7");
        let mini = Found {
            model: Model::Mini,
            rates: vec![6_000_000, 3_000_000],
            ..r2
        };
        assert!(label(&mini).starts_with("Airspy Mini · 637862dc2e1182d7"));
        assert!(
            label(&mini).contains("not driven natively"),
            "{}",
            label(&mini)
        );
    }

    fn listing() -> DeviceListing {
        DeviceListing {
            kind: DeviceKind::Airspy,
            index: 0,
            label: "Airspy R2 · 637862dc2e1182d7".into(),
            serial: Some("637862dc2e1182d7".into()),
            args: None,
            path: None,
            tiny_sa_input: None,
        }
    }

    /// A fixture library that lives as long as the test process, as the real
    /// one does once loaded.
    fn opened() -> (TestLibrary, AirspyDevice) {
        let fixture = TestLibrary::new(false);
        let api: &'static ffi::AirspyApi =
            Box::leak(Box::new(ffi::resolve(fixture.library()).unwrap()));
        let device = AirspyDevice::open_with_api(api, &listing()).expect("the R2 opens");
        (fixture, device)
    }

    /// The C structs have the layout libairspy writes into.
    #[test]
    fn the_structs_match_the_c_layout() {
        let fixture = TestLibrary::new(false);
        assert_eq!(
            fixture.airspy_layout(2),
            std::mem::size_of::<ffi::LibVersion>()
        );
    }

    /// What an R2 is, from its firmware and the bench: 24 MHz to 1.8 GHz,
    /// 2.5 or 10 Msps, int16 at 16384 counts full scale from a 12-bit
    /// converter, the driver's thread delivering, no baseband filter and no
    /// noise model.
    #[test]
    fn an_r2_describes_itself() {
        let c = caps();
        assert_eq!((c.freq_min_hz, c.freq_max_hz), (24_000_000, 1_800_000_000));
        assert_eq!((c.sample_rate_min_hz, c.sample_rate_max_hz), (2.5e6, 10e6));
        assert_eq!(c.sample_geometry.full_scale, 16384.0);
        assert_eq!(c.sample_geometry.bits(), 12);
        assert_eq!(c.samples_per_transfer, 65_536);
        assert_eq!(c.delivery, DeliveryModel::Push);
        assert!(!c.friis_applicable && !c.has_bb_filter);
    }

    /// The three measured stages, front to back, the LNA in dB only over the
    /// VHF band it was measured in; the LNA the primary key and the mixer
    /// the second.
    #[test]
    fn the_gain_model_is_the_measured_one() {
        let g = gain_model();
        let stages = g.stages();
        let names: Vec<_> = stages.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["LNA", "MIX", "VGA"]);
        assert_eq!(
            stages.iter().map(|s| s.positions()).collect::<Vec<_>>(),
            [Some(15), Some(16), Some(16)]
        );
        assert_eq!(stages[0].table[14], 36.02);
        assert!(stages[0].db_holds_at(92_800_000) && !stages[0].db_holds_at(943_400_000));
        assert!(stages[1].db_holds_at(1_800_000_000) && stages[2].db_holds_at(24_000_000));
        assert!(g.has_second_stage());
        assert_eq!(g.unmodelled_stages(), "LNA\u{25b8}MIX\u{25b8}VGA");
    }

    /// **Opening states what the firmware would otherwise choose**: the IQ
    /// sample type, both AGCs off, the bias tee off. And it says so.
    #[test]
    fn opening_states_the_sample_type_agc_and_bias_tee() {
        let (fixture, device) = opened();
        assert_eq!(fixture.airspy_state(0), ffi::SAMPLE_INT16_IQ, "sample type");
        assert_eq!(fixture.airspy_state(1), 0, "bias tee off");
        assert_eq!(fixture.airspy_state(2), 0, "LNA AGC off");
        assert_eq!(fixture.airspy_state(3), 0, "mixer AGC off");
        let info = device.info();
        assert_eq!(info.board_name, "Airspy R2");
        assert_eq!(info.serial, "637862dc2e1182d7");
        assert_eq!(info.fw_version.as_deref(), Some("v1.0.0-rc10-0-g946184a"));
        // The header shows the firmware under the radio's own label, never
        // guessing a HackRF firmware family from the version's shape.
        let stack = info.stack.expect("the R2 names its firmware");
        assert_eq!(stack.label, "airspy fw ");
        assert_eq!(&*stack.value, "v1.0.0-rc10-0-g946184a");
        let notes = device.open_notes().join("\n");
        assert!(notes.contains("libairspy 1.0.10"), "{notes}");
        assert!(
            notes.contains("AGC off") && notes.contains("bias tee off"),
            "{notes}"
        );
    }

    /// A Mini, or a radio another app holds, is not opened; each says why.
    #[test]
    fn a_mini_or_a_held_radio_is_refused_by_name() {
        let fixture = TestLibrary::new(false);
        let api: &'static ffi::AirspyApi =
            Box::leak(Box::new(ffi::resolve(fixture.library()).unwrap()));
        fixture.mode(20);
        let why = AirspyDevice::open_with_api(api, &listing())
            .err()
            .unwrap()
            .to_string();
        assert!(
            why.contains("Airspy Mini") && why.contains("soapy=driver=airspy"),
            "{why}"
        );
        fixture.mode(21);
        let why = AirspyDevice::open_with_api(api, &listing())
            .err()
            .unwrap()
            .to_string();
        assert!(why.contains("AIRSPY_ERROR_BUSY"), "{why}");
    }

    /// A gain in dB lands on the register position the bench measured it
    /// at, for each stage by its own table.
    #[test]
    fn a_gain_lands_on_its_measured_position() {
        let (fixture, device) = opened();
        device.set_stage_gain(0, "LNA", 27.76).unwrap();
        device.set_stage_gain(1, "MIX", 16.24).unwrap();
        device.set_stage_gain(2, "VGA", 26.79).unwrap();
        assert_eq!(
            (
                fixture.airspy_state(4),
                fixture.airspy_state(5),
                fixture.airspy_state(6)
            ),
            (10, 15, 8)
        );
        device.set_lna_gain(36).unwrap();
        assert_eq!(fixture.airspy_state(4), 14);
    }

    /// Tuning inside the range reaches the radio; outside it is refused, as
    /// is a rate the firmware does not offer. The two it offers are set as
    /// asked, there being no getter to read back.
    #[test]
    fn tuning_and_rates_stay_inside_what_the_r2_does() {
        let (fixture, device) = opened();
        device.set_frequency(92_800_000).unwrap();
        assert_eq!(fixture.airspy_state(7), 92_800);
        assert!(device.set_frequency(2_400_000_000).is_err());
        assert_eq!(device.set_sample_rate(10e6).unwrap().rate_hz, 10e6);
        assert_eq!(fixture.airspy_state(8), 10_000_000);
        assert!(device.set_sample_rate(6e6).is_err());
        fixture.mode(23);
        let why = device.set_frequency(100_000_000).unwrap_err().to_string();
        assert!(why.contains("AIRSPY_ERROR"), "{why}");
    }

    /// Until it streams, starting RX says so and names the road that works.
    #[test]
    fn starting_rx_says_it_is_not_finished_and_points_at_soapy() {
        let (_fixture, device) = opened();
        let why = device.start_rx_refusal();
        assert!(why.contains("--device soapy=driver=airspy"), "{why}");
        assert!(!device.is_streaming());
    }
}
