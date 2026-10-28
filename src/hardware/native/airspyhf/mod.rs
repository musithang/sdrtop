// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The Airspy HF+ Discovery, driven through libairspyhf.
//!
//! Found, identified, opened and described here, for now only when `--device
//! airspyhf` names this backend; it does not stream yet.
//!
//! **Its gains are what they say.** The attenuator's 6 dB steps and the LNA's
//! 6 dB were measured on the bench against steady carriers, at HF and VHF,
//! and neither is made up digitally: both move the level the samples carry.
//! The attenuator is a negative gain (`-48..0 dB`), front of the LNA.
//!
//! **Where its LO is.** At its zero-IF rates libairspyhf tunes the radio 5 kHz
//! above the asked frequency and rotates the samples back, and below the
//! lowest LO it holds the LO there; [`lo_offset_hz`] says where that leaves
//! the LO in the delivered spectrum.
//!
//! **The Discovery, and only the Discovery.** The HF+ Dual Port and the
//! Ranger answer on the same USB id and through the same library; the part id
//! their firmware reports tells them apart (1, 2, 3). Only the Discovery has
//! been held and measured here, so the others are named and not taken: their
//! front ends, attenuators and rates would be guesses.

pub mod ffi;

use std::ffi::CStr;
use std::sync::{Arc, Mutex};

use libc::{c_char, c_int, c_void};

use crate::hardware::traits::RateSet;
use crate::hardware::{
    AcquisitionKind, DeliveryModel, DeviceCapabilities, DeviceInfo, DeviceKind, DeviceListing,
    GainModel, LevelUnit, RxContext, SampleFormat, SampleGeometry, SdrDevice, StageSpec,
    IQ_TRACE_STALE_MS,
};

/// Which HF+ a firmware is, by the part id it reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Board {
    DualPort,
    Discovery,
    Ranger,
    /// A part id none of the above has.
    Unknown,
}

impl Board {
    pub fn from_part_id(id: u32) -> Self {
        match id {
            1 => Board::DualPort,
            2 => Board::Discovery,
            3 => Board::Ranger,
            _ => Board::Unknown,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Board::DualPort => "Airspy HF+ Dual Port",
            Board::Discovery => "Airspy HF+ Discovery",
            Board::Ranger => "Airspy Ranger",
            Board::Unknown => "Airspy HF+",
        }
    }
}

/// What libairspyhf said about one radio.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    /// As SoapyAirspyHF spells it: lowercase hex, no prefix.
    pub serial: String,
    pub board: Board,
    pub firmware: Option<String>,
    pub rates: Vec<u32>,
    /// Listed on the bus but would not open: another program has claimed it,
    /// or this user may not open it. Nothing else is known about it.
    pub held: bool,
}

/// Every HF+ on the bus, each opened just long enough to identify it. A radio
/// that is listed but will not open is kept, held and described by its serial
/// alone.
pub fn find(api: &ffi::AirspyHfApi) -> Vec<Found> {
    // SAFETY: a null buffer asks only for the count.
    let count = unsafe { (api.airspyhf_list_devices)(std::ptr::null_mut(), 0) };
    let Ok(count) = usize::try_from(count) else {
        return Vec::new();
    };
    let mut serials = vec![0u64; count];
    // SAFETY: the buffer holds `count` serials, and the call writes at most
    // that many.
    let got = unsafe {
        (api.airspyhf_list_devices)(serials.as_mut_ptr(), c_int::try_from(count).unwrap_or(0))
    };
    serials.truncate(usize::try_from(got).unwrap_or(0));
    serials
        .into_iter()
        .map(|serial| identify(api, serial))
        .collect()
}

/// One radio, opened by its serial, read and closed; or, when it would not
/// open, held.
fn identify(api: &ffi::AirspyHfApi, serial: u64) -> Found {
    let mut device: *mut c_void = std::ptr::null_mut();
    // SAFETY: libairspyhf writes the handle it opened, or fails.
    if unsafe { (api.airspyhf_open_sn)(&mut device, serial) } != 0 || device.is_null() {
        return Found {
            serial: format!("{serial:x}"),
            board: Board::Unknown,
            firmware: None,
            rates: Vec::new(),
            held: true,
        };
    }
    // SAFETY: `device` was opened just above and is closed once read.
    let found = unsafe { read_found(api, device, serial) };
    // SAFETY: as above.
    unsafe { (api.airspyhf_close)(device) };
    found
}

/// What an open radio says about itself. A board whose part id cannot be read
/// is `Unknown`, and so not taken.
///
/// # Safety
/// `device` must be a handle libairspyhf opened and has not closed.
unsafe fn read_found(api: &ffi::AirspyHfApi, device: *mut c_void, serial: u64) -> Found {
    // SAFETY: each call writes only into the buffer it is given, of the size
    // it is given; the version buffer is the size libairspyhf copies from.
    unsafe {
        let mut part = ffi::PartSerial::default();
        let board = if (api.airspyhf_board_partid_serialno_read)(device, &mut part) == 0 {
            Board::from_part_id(part.part_id)
        } else {
            Board::Unknown
        };
        let mut text = [0 as c_char; ffi::VERSION_STRING_SIZE as usize];
        let firmware = ((api.airspyhf_version_string_read)(
            device,
            text.as_mut_ptr(),
            ffi::VERSION_STRING_SIZE,
        ) == 0)
            .then(|| text_of(&text))
            .flatten();
        let mut n = 0u32;
        let rates = if (api.airspyhf_get_samplerates)(device, &mut n, 0) == 0 && n > 0 {
            let mut rates = vec![0u32; n as usize];
            if (api.airspyhf_get_samplerates)(device, rates.as_mut_ptr(), n) == 0 {
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
            rates,
            held: false,
        }
    }
}

/// The text up to the first NUL of a buffer libairspyhf filled, trimmed;
/// `None` when that is empty.
fn text_of(buf: &[c_char]) -> Option<String> {
    // SAFETY: c_char and u8 have the same size and alignment.
    let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), buf.len()) };
    let text = CStr::from_bytes_until_nul(bytes).ok()?.to_string_lossy();
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
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
            kind: DeviceKind::AirspyHf,
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
    if found.held {
        return format!(
            "{} · {} · in use by another program, or not ours to open",
            Board::Unknown.name(),
            found.serial
        );
    }
    let base = format!("{} · {}", found.board.name(), found.serial);
    match found.board {
        Board::Discovery => base,
        _ => format!("{base} · not driven natively: nobody here has held one"),
    }
}

/// The lowest LO libairspyhf tunes the radio to at a zero-IF rate, Hz.
const MIN_ZERO_IF_LO_HZ: u64 = 180_000;
/// The same at a low-IF rate.
const MIN_LOW_IF_LO_HZ: u64 = 84_000;
/// How far above the asked frequency the library puts the LO at a zero-IF
/// rate with its DSP on, so the radio's own DC lands off the centre.
const IF_SHIFT_HZ: u64 = 5_000;
/// Buffers libairspyhf queues between its USB thread and the consumer thread
/// that calls back (`RAW_BUFFER_COUNT`, 1.6.8 and later alike).
const QUEUE_BLOCKS: u32 = 8;

/// Where the radio's LO sits in the delivered spectrum, Hz from its centre,
/// for the frequency asked, the rate's architecture, the library's DSP and the
/// calibration in the radio's flash: libairspyhf's own arithmetic
/// (`airspyhf_set_freq`), which tunes the firmware in whole kHz to the asked
/// frequency corrected by `ppb`, 5 kHz higher at a zero-IF rate with its DSP
/// on, and never below the rate's lowest LO. The rotator then moves the asked
/// frequency to the centre, and the LO lands this far from it.
pub fn lo_offset_hz(freq_hz: u64, low_if: bool, dsp: bool, ppb: i32) -> i64 {
    let shift = if dsp && !low_if { IF_SHIFT_HZ } else { 0 } as f64;
    let correction = (1e9 + f64::from(ppb)) * 1e-9;
    let lowest_khz = if low_if {
        MIN_LOW_IF_LO_HZ
    } else {
        MIN_ZERO_IF_LO_HZ
    } / 1000;
    let lo_khz = (((freq_hz as f64 * correction + shift) * 1e-3).round() as u64).max(lowest_khz);
    // The LO in the radio's own reference, back on the true scale the
    // rotator corrected the asked frequency onto.
    let lo_hz = lo_khz as f64 * 1e3 / correction;
    (lo_hz - freq_hz as f64).round() as i64
}

/// The HF+ Discovery's two stages front to back: the attenuator, nine 6 dB
/// steps of negative gain, and the LNA, off or 6 dB. Both measured on the
/// bench at HF and VHF to within 0.2 dB of those figures. The firmware's AGC
/// sets the attenuator alone; the LNA stays where the keys put it. A first
/// run and `[R]` start with no attenuation and the LNA off: on any antenna
/// whose band noise is above the radio's own, the LNA adds noise and level
/// alike and takes 6 dB of headroom.
pub fn gain_model() -> GainModel {
    GainModel::new(
        vec![
            StageSpec::tabled("ATT", (0..=8).map(|i| f64::from(i) * 6.0 - 48.0).collect()),
            StageSpec::tabled("LNA", vec![0.0, 6.0]),
        ],
        "ATT",
        "ATT",
    )
    .with_second_stage()
    .with_no_cascade_reason("each stage's own noise figure is not published")
    .with_boost(crate::hardware::Boost::GainMode)
    .with_auto_stages(&[0])
    .with_start_gains(vec![0.0, 0.0])
}

/// What a Discovery can do: Airspy's published bands, the rates its firmware
/// lists, and the block its library hands over.
pub fn caps(rates: &[u32], samples_per_transfer: u64) -> DeviceCapabilities {
    let min = rates.iter().copied().min().unwrap_or(768_000);
    let max = rates.iter().copied().max().unwrap_or(768_000);
    let default_rate = if rates.contains(&768_000) {
        768_000
    } else {
        max
    };
    DeviceCapabilities {
        level_unit: LevelUnit::Dbfs,
        level_min_db: -150.0,
        level_max_db: 0.0,
        trace_stale_ms: IQ_TRACE_STALE_MS,
        acquisition: AcquisitionKind::IqSamples,
        sample_rate_is_span: false,
        // The HF front end to 31 MHz, the VHF one from 60: nothing in between.
        freq_min_hz: 9_000,
        freq_max_hz: 260_000_000,
        freq_gaps: vec![std::ops::RangeInclusive::new(31_000_001, 59_999_999)],
        sample_rate_min_hz: f64::from(min),
        sample_rate_max_hz: f64::from(max),
        // The 40 m band: a choice, somewhere there is always something.
        default_frequency_hz: 7_100_000,
        default_sample_rate_hz: f64::from(default_rate),
        // The radio's 16-bit USB words, carried with eight fraction bits so
        // the library's correction and fine tuning are not rounded away: 0 dBFS
        // is where the words clip.
        sample_geometry: SampleGeometry {
            format: SampleFormat::Int32,
            full_scale: 8_388_608.0,
            converter_bits: Some(16),
        },
        gain: gain_model(),
        samples_per_transfer,
        has_bb_filter: false,
        friis_applicable: false,
        delivery: DeliveryModel::Push,
        driver_queue_blocks: Some(QUEUE_BLOCKS),
    }
}

/// Where the radio is tuned, as the device last set it: libairspyhf has no
/// getters.
struct Tuning {
    freq_hz: u64,
    rate: u32,
    low_if: bool,
}

/// An open Airspy HF+ Discovery.
pub struct AirspyHfDevice {
    api: &'static ffi::AirspyHfApi,
    ptr: *mut c_void,
    caps: DeviceCapabilities,
    info: DeviceInfo,
    notes: Vec<String>,
    rates: Vec<u32>,
    /// The flash's reference correction, read at open.
    ppb: i32,
    tuning: Mutex<Tuning>,
}

// Safety: the handle is only used through libairspyhf, which serialises its
// own control transfers; the tuning is behind its mutex.
unsafe impl Send for AirspyHfDevice {}
unsafe impl Sync for AirspyHfDevice {}

impl AirspyHfDevice {
    /// Opens the radio a listing names, by its serial.
    pub fn open(listing: &DeviceListing) -> anyhow::Result<Self> {
        Self::open_with_api(ffi::api()?, listing)
    }

    fn open_with_api(
        api: &'static ffi::AirspyHfApi,
        listing: &DeviceListing,
    ) -> anyhow::Result<Self> {
        let serial = listing
            .serial
            .as_deref()
            .and_then(|s| u64::from_str_radix(s, 16).ok())
            .ok_or_else(|| anyhow::anyhow!("{}: no serial to open it by", listing.label))?;
        let mut ptr: *mut c_void = std::ptr::null_mut();
        // SAFETY: libairspyhf writes the handle it opened, or fails.
        if unsafe { (api.airspyhf_open_sn)(&mut ptr, serial) } != 0 || ptr.is_null() {
            anyhow::bail!(
                "{}: libairspyhf could not open it: it is on the bus, so another \
                 program is most likely using it",
                listing.label
            );
        }
        // SAFETY: the handle was just opened.
        let found = unsafe { read_found(api, ptr, serial) };
        if found.board != Board::Discovery {
            // SAFETY: opened above, not kept.
            unsafe { (api.airspyhf_close)(ptr) };
            anyhow::bail!(
                "{}: only the Airspy HF+ Discovery is driven natively",
                label(&found)
            );
        }
        // SAFETY: the handle is open; each call fills what it is given.
        let (block, ppb, version) = unsafe {
            let block = (api.airspyhf_get_output_size)(ptr);
            let mut ppb = 0i32;
            let ppb = ((api.airspyhf_get_calibration)(ptr, &mut ppb) == 0).then_some(ppb);
            let mut v = ffi::LibVersion::default();
            (api.airspyhf_lib_version)(&mut v);
            (block, ppb, v)
        };
        let firmware = found.firmware.clone();
        let mut device = Self {
            api,
            ptr,
            caps: caps(&found.rates, u64::try_from(block).unwrap_or(0)),
            info: DeviceInfo {
                board_name: Board::Discovery.name().to_string(),
                serial: format!("{serial:x}"),
                fw_version: firmware.clone(),
                board_rev: None,
                usb_api_version: None,
                tuner_name: None,
                stack: firmware.map(|fw| crate::hardware::SoftwareStack {
                    label: "airspyhf fw ",
                    value: Arc::from(fw.as_str()),
                }),
            },
            notes: Vec::new(),
            rates: found.rates,
            ppb: ppb.unwrap_or(0),
            tuning: Mutex::new(Tuning {
                freq_hz: 0,
                rate: 0,
                low_if: false,
            }),
        };
        // What the firmware and the library would otherwise keep from the last
        // program, stated: the library's correction on, the AGC off at its low
        // threshold, no attenuation, the LNA off.
        // SAFETY: the handle is open and owned by `device`, whose Drop closes it.
        unsafe {
            device.check(
                (api.airspyhf_set_lib_dsp)(ptr, 1),
                "switch the library's DSP on",
            )?;
            device.check((api.airspyhf_set_hf_agc)(ptr, 0), "switch the AGC off")?;
            device.check(
                (api.airspyhf_set_hf_agc_threshold)(ptr, 0),
                "set the AGC's threshold",
            )?;
            device.check((api.airspyhf_set_hf_att)(ptr, 0), "set the attenuator")?;
            device.check((api.airspyhf_set_hf_lna)(ptr, 0), "switch the LNA off")?;
        }
        let name = Board::Discovery.name();
        device.notes = vec![
            format!(
                "{name}: libairspyhf {}.{}.{}, {} pairs a block",
                version.major, version.minor, version.revision, block
            ),
            match ppb {
                Some(ppb) => {
                    format!("{name}: reference correction from the radio's flash: {ppb} ppb")
                }
                None => format!("{name}: the radio's flash correction could not be read"),
            },
            format!("{name}: library DSP on, AGC off, no attenuation, LNA off, set at open"),
        ];
        Ok(device)
    }

    /// A libairspyhf call's result as an error naming what was being done.
    fn check(&self, code: c_int, doing: &str) -> anyhow::Result<()> {
        if code == 0 {
            Ok(())
        } else {
            anyhow::bail!(
                "{}: could not {doing} (libairspyhf {code})",
                Board::Discovery.name()
            )
        }
    }

    fn tuning(&self) -> std::sync::MutexGuard<'_, Tuning> {
        self.tuning.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl SdrDevice for AirspyHfDevice {
    fn capabilities(&self) -> &DeviceCapabilities {
        &self.caps
    }

    fn info(&self) -> DeviceInfo {
        self.info.clone()
    }

    fn start_rx(&self, _ctx: Arc<RxContext>) -> anyhow::Result<()> {
        anyhow::bail!(
            "the {}'s own backend opens and describes the radio but cannot stream yet",
            Board::Discovery.name()
        )
    }

    fn stop_rx(&self) -> anyhow::Result<()> {
        Ok(())
    }

    fn is_streaming(&self) -> bool {
        false
    }

    fn set_frequency(&self, hz: u64) -> anyhow::Result<()> {
        if !self.caps.tunes(hz) {
            anyhow::bail!(
                "the {} tunes {}, not {:.3} MHz",
                Board::Discovery.name(),
                self.caps.bands_text(),
                hz as f64 / 1e6
            );
        }
        // SAFETY: the handle is open; the bands keep `hz` inside u32.
        self.check(
            unsafe { (self.api.airspyhf_set_freq)(self.ptr, hz as u32) },
            "tune",
        )?;
        self.tuning().freq_hz = hz;
        Ok(())
    }

    /// One of the rates the firmware lists, set as asked; whether it is a
    /// low-IF one is read back, because where the LO sits depends on it.
    fn set_sample_rate(&self, hz: f64) -> anyhow::Result<RateSet> {
        let Some(&rate) = self
            .rates
            .iter()
            .find(|&&r| (f64::from(r) - hz).abs() < 0.5)
        else {
            let listed: Vec<String> = self.rates.iter().map(|r| format!("{}", r / 1000)).collect();
            anyhow::bail!(
                "the {} runs at {} kS/s",
                Board::Discovery.name(),
                listed.join(", ")
            );
        };
        // SAFETY: the handle is open.
        self.check(
            unsafe { (self.api.airspyhf_set_samplerate)(self.ptr, rate) },
            "set the rate",
        )?;
        // SAFETY: the handle is open.
        let low_if = unsafe { (self.api.airspyhf_is_low_if)(self.ptr) } != 0;
        let mut tuning = self.tuning();
        tuning.rate = rate;
        tuning.low_if = low_if;
        Ok(RateSet::new(f64::from(rate), None, 0))
    }

    fn set_lna_gain(&self, db: u32) -> anyhow::Result<()> {
        self.set_stage_gain(0, "ATT", f64::from(db))
    }

    /// The attenuator by its index (0 none .. 8 the most) or the LNA on or
    /// off, from the stage's own table.
    fn set_stage_gain(&self, index: usize, _name: &str, db: f64) -> anyhow::Result<()> {
        let stages = self.caps.gain.stages();
        let Some(position) = stages.get(index).and_then(|s| s.position_of(db)) else {
            anyhow::bail!("the {} has no gain stage {index}", Board::Discovery.name());
        };
        let position = u8::try_from(position).unwrap_or(u8::MAX);
        // SAFETY: the handle is open; positions come from the stage's table.
        let code = unsafe {
            match index {
                // The table runs -48 .. 0 dB; the attenuator's index the other way.
                0 => (self.api.airspyhf_set_hf_att)(self.ptr, 8u8.saturating_sub(position)),
                _ => (self.api.airspyhf_set_hf_lna)(self.ptr, position),
            }
        };
        self.check(code, "set the gain")
    }

    fn open_notes(&self) -> &[String] {
        &self.notes
    }

    /// The firmware's AGC, which sets the attenuator while it is on.
    fn set_tuner_agc(&self, on: bool) -> anyhow::Result<()> {
        // SAFETY: the handle is open.
        self.check(
            unsafe { (self.api.airspyhf_set_hf_agc)(self.ptr, u8::from(on)) },
            "switch the AGC",
        )
    }

    fn lo_offset_hz(&self) -> i64 {
        let t = self.tuning();
        if t.rate == 0 || t.freq_hz == 0 {
            return 0;
        }
        lo_offset_hz(t.freq_hz, t.low_if, true, self.ppb)
    }
}

impl Drop for AirspyHfDevice {
    fn drop(&mut self) {
        // SAFETY: the handle was opened in `open` and is closed once, here.
        unsafe { (self.api.airspyhf_close)(self.ptr) };
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::TestLibrary;
    use super::*;

    #[test]
    fn loader_rejects_missing_required_symbol() {
        let fixture = TestLibrary::new(true);
        let err = match super::super::loader::load("libairspyhf", &[fixture.path()], ffi::resolve) {
            Ok(_) => panic!("accepted an incomplete libairspyhf"),
            Err(err) => err,
        };
        assert!(err.contains(fixture.path()));
        assert!(
            err.contains("missing required symbol airspyhf_get_samplerates"),
            "{err}"
        );
    }

    /// The part ids the firmware reports, each its own board; anything else
    /// is no board of ours.
    #[test]
    fn a_board_is_told_by_its_part_id() {
        assert_eq!(Board::from_part_id(1), Board::DualPort);
        assert_eq!(Board::from_part_id(2), Board::Discovery);
        assert_eq!(Board::from_part_id(3), Board::Ranger);
        assert_eq!(Board::from_part_id(0), Board::Unknown);
        assert_eq!(Board::from_part_id(0xff), Board::Unknown);
    }

    /// A Discovery is found with its serial spelled as SoapyAirspyHF spells
    /// it, so the same radio reached both ways is one radio; its board,
    /// firmware and rates as the library reads them; and it is closed again.
    #[test]
    fn a_discovery_is_found_and_identified() {
        let fixture = TestLibrary::new(false);
        let api = ffi::resolve(fixture.library()).unwrap();
        assert_eq!(
            find(&api),
            vec![Found {
                serial: "dd528080ff444630".into(),
                board: Board::Discovery,
                firmware: Some("R5.0.1-CD".into()),
                rates: vec![912_000, 768_000, 650_000, 456_000, 384_000, 228_000, 192_000],
                held: false,
            }]
        );
        assert_eq!(fixture.calls(3), 1, "opened to identify it, then closed");
        // libairspyhf's own buffer is 64 bytes (`MAX_VERSION_STRING_SIZE`),
        // a fact of the library, so it is written out here rather than read
        // from the constant under test.
        assert!(
            (1..=64).contains(&fixture.airspyhf_state(0)),
            "asked for no more of the version than libairspyhf holds: {}",
            fixture.airspyhf_state(0)
        );
    }

    /// Another board is named as itself; a radio another program holds is
    /// kept by its serial alone; no radio is no radio.
    #[test]
    fn another_board_is_named_and_a_held_radio_kept() {
        let fixture = TestLibrary::new(false);
        let api = ffi::resolve(fixture.library()).unwrap();
        fixture.mode(30);
        assert_eq!(find(&api)[0].board, Board::Ranger);
        fixture.mode(31);
        assert_eq!(
            find(&api),
            vec![Found {
                serial: "dd528080ff444630".into(),
                board: Board::Unknown,
                firmware: None,
                rates: vec![],
                held: true,
            }]
        );
        fixture.mode(32);
        assert_eq!(find(&api), vec![]);
    }

    /// The selector says which HF+ it is and by which serial; another board
    /// says it is not driven natively, and a held one that it is held.
    #[test]
    fn the_label_names_the_board_and_serial() {
        let discovery = Found {
            serial: "dd528080ff444630".into(),
            board: Board::Discovery,
            firmware: None,
            rates: vec![768_000],
            held: false,
        };
        assert_eq!(label(&discovery), "Airspy HF+ Discovery · dd528080ff444630");
        let ranger = Found {
            board: Board::Ranger,
            ..discovery.clone()
        };
        assert!(
            label(&ranger).starts_with("Airspy Ranger · dd528080ff444630 · not driven natively"),
            "{}",
            label(&ranger)
        );
        let held = Found {
            board: Board::Unknown,
            held: true,
            ..discovery
        };
        assert!(label(&held).contains("in use by another program"));
    }

    /// The C structs have the layout libairspyhf writes into.
    #[test]
    fn the_structs_match_the_c_layout() {
        let fixture = TestLibrary::new(false);
        assert_eq!(
            fixture.airspyhf_layout(0),
            std::mem::size_of::<ffi::PartSerial>()
        );
        assert_eq!(
            fixture.airspyhf_layout(1),
            std::mem::offset_of!(ffi::PartSerial, serial_no)
        );
    }

    fn listing() -> DeviceListing {
        DeviceListing {
            kind: DeviceKind::AirspyHf,
            index: 0,
            label: "Airspy HF+ Discovery · dd528080ff444630".into(),
            serial: Some("dd528080ff444630".into()),
            args: None,
            path: None,
            tiny_sa_input: None,
        }
    }

    /// A fixture library that lives as long as the test process, as the real
    /// one does once loaded, and the Discovery opened through it.
    fn opened() -> (TestLibrary, AirspyHfDevice) {
        let fixture = TestLibrary::new(false);
        let api: &'static ffi::AirspyHfApi =
            Box::leak(Box::new(ffi::resolve(fixture.library()).unwrap()));
        let device = AirspyHfDevice::open_with_api(api, &listing()).expect("the Discovery opens");
        (fixture, device)
    }

    /// **Where the LO is**, by libairspyhf's own arithmetic: 5 kHz up at a
    /// zero-IF rate with the DSP on, the kHz grid's remainder at a low-IF
    /// rate, the rate's lowest LO below it, and the flash's correction folded
    /// back onto the true scale.
    #[test]
    fn the_lo_is_where_the_library_puts_it() {
        assert_eq!(lo_offset_hz(7_100_000, false, true, 0), 5_000);
        assert_eq!(lo_offset_hz(7_100_300, false, true, 0), 4_700);
        assert_eq!(lo_offset_hz(7_100_000, true, true, 0), 0);
        assert_eq!(lo_offset_hz(7_100_300, true, true, 0), -300);
        assert_eq!(
            lo_offset_hz(7_100_300, false, false, 0),
            -300,
            "no shift with DSP off"
        );
        assert_eq!(
            lo_offset_hz(9_000, false, true, 0),
            171_000,
            "held at 180 kHz"
        );
        assert_eq!(lo_offset_hz(9_000, true, true, 0), 75_000, "held at 84 kHz");
        // +250 ppb: tuned 100.000025 MHz in the radio's reference, LO at
        // 100.005 MHz there, 100.004975 MHz on the true scale.
        assert_eq!(lo_offset_hz(100_000_000, false, true, 250), 4_975);
    }

    /// The two measured stages, front to back, starting with no attenuation
    /// and the LNA off; the AGC sets the attenuator alone.
    #[test]
    fn the_gain_model_is_the_measured_one() {
        let g = gain_model();
        let stages = g.stages();
        let names: Vec<&str> = stages.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["ATT", "LNA"]);
        assert_eq!(stages[0].table.first(), Some(&-48.0));
        assert_eq!(stages[0].table.last(), Some(&0.0));
        assert_eq!(stages[0].table.len(), 9);
        assert_eq!(stages[1].table, [0.0, 6.0]);
        assert_eq!(g.start_gains(None), [0.0, 0.0]);
        assert!(g.auto_governs(0) && !g.auto_governs(1));
        assert!(g.has_second_stage());
    }

    /// What a Discovery is: two bands with the gap, the firmware's rates, the
    /// USB word with eight fraction bits (16 bits, 256 counts a word), the
    /// library's block, Push, its queue of eight.
    #[test]
    fn a_discovery_describes_itself() {
        let c = caps(
            &[
                912_000, 768_000, 650_000, 456_000, 384_000, 228_000, 192_000,
            ],
            2048,
        );
        assert_eq!(
            c.bands(),
            vec![9_000..=31_000_000, 60_000_000..=260_000_000]
        );
        assert_eq!((c.sample_rate_min_hz, c.sample_rate_max_hz), (192e3, 912e3));
        assert_eq!(c.default_sample_rate_hz, 768e3);
        assert!(c.tunes(c.default_frequency_hz));
        assert_eq!(c.sample_geometry.format, SampleFormat::Int32);
        assert_eq!(c.sample_geometry.bits(), 16);
        assert_eq!(c.sample_geometry.counts_per_word(), 256);
        assert_eq!(c.samples_per_transfer, 2048);
        assert_eq!(c.delivery, DeliveryModel::Push);
        assert_eq!(c.driver_queue_blocks, Some(8));
        assert!(!c.friis_applicable && !c.has_bb_filter);
    }

    /// Opening states what the last program might have left: the library's
    /// DSP on, the AGC off at its low threshold, no attenuation, the LNA off;
    /// and says who it is, with the library and the flash's correction.
    #[test]
    fn opening_states_the_dsp_agc_and_gains() {
        let (fixture, device) = opened();
        for (field, want, what) in [
            (3, 1, "DSP"),
            (6, 0, "AGC"),
            (7, 0, "threshold"),
            (4, 0, "attenuator"),
            (5, 0, "LNA"),
        ] {
            assert_eq!(fixture.airspyhf_state(field), want, "{what}");
        }
        let info = device.info();
        assert_eq!(info.board_name, "Airspy HF+ Discovery");
        assert_eq!(info.fw_version.as_deref(), Some("R5.0.1-CD"));
        assert_eq!(info.stack.map(|s| s.label), Some("airspyhf fw "));
        let notes = device.open_notes().join("\n");
        assert!(
            notes.contains("libairspyhf 1.6.8, 2048 pairs a block"),
            "{notes}"
        );
        assert!(
            notes.contains("reference correction from the radio's flash: 0 ppb"),
            "{notes}"
        );
        assert_eq!(device.capabilities().samples_per_transfer, 2048);
    }

    /// Another board and a held radio are refused by name, and a held one says
    /// whose it most likely is.
    #[test]
    fn another_board_or_a_held_radio_is_refused_by_name() {
        let fixture = TestLibrary::new(false);
        let api: &'static ffi::AirspyHfApi =
            Box::leak(Box::new(ffi::resolve(fixture.library()).unwrap()));
        fixture.mode(30);
        let why = AirspyHfDevice::open_with_api(api, &listing())
            .err()
            .unwrap()
            .to_string();
        assert!(why.starts_with("Airspy Ranger · dd528080ff444630"), "{why}");
        assert!(
            why.contains("only the Airspy HF+ Discovery is driven natively"),
            "{why}"
        );
        assert_eq!(fixture.calls(3), 1, "closed again");
        fixture.mode(31);
        let why = AirspyHfDevice::open_with_api(api, &listing())
            .err()
            .unwrap()
            .to_string();
        assert!(
            why.contains("another program is most likely using it"),
            "{why}"
        );
    }

    /// A frequency in the gap is refused with the bands and reaches nothing;
    /// one in a band is tuned. An unlisted rate is refused with the firmware's
    /// list; a listed one is set, and the LO moves with its architecture.
    #[test]
    fn tuning_and_rate_reach_the_radio_and_move_the_lo() {
        let (fixture, device) = opened();
        let why = device.set_frequency(45_000_000).unwrap_err().to_string();
        assert!(why.contains("9 kHz to 31 MHz, 60 to 260 MHz"), "{why}");
        assert_eq!(fixture.airspyhf_state(1), -1, "not tuned");
        device.set_frequency(7_100_300).unwrap();
        assert_eq!(fixture.airspyhf_state(1), 7_100);
        let why = device.set_sample_rate(1_000_000.0).unwrap_err().to_string();
        assert!(
            why.contains("912, 768, 650, 456, 384, 228, 192 kS/s"),
            "{why}"
        );
        device.set_sample_rate(768_000.0).unwrap();
        assert_eq!(fixture.airspyhf_state(2), 768_000);
        assert_eq!(
            device.lo_offset_hz(),
            4_700,
            "zero-IF: 5 kHz up, less the grid's 300 Hz"
        );
        device.set_sample_rate(192_000.0).unwrap();
        assert_eq!(
            device.lo_offset_hz(),
            -300,
            "low-IF: the grid's remainder only"
        );
    }

    /// Each stage reaches its own control: the attenuator by its index the
    /// other way round from the table, the LNA on or off; the AGC its own.
    #[test]
    fn each_gain_reaches_its_own_control() {
        let (fixture, device) = opened();
        for (db, index) in [(-12.0, 2), (0.0, 0), (-48.0, 8), (-30.0, 5)] {
            device.set_stage_gain(0, "ATT", db).unwrap();
            assert_eq!(fixture.airspyhf_state(4), index, "ATT {db} dB");
        }
        device.set_stage_gain(1, "LNA", 6.0).unwrap();
        assert_eq!(fixture.airspyhf_state(5), 1);
        device.set_stage_gain(1, "LNA", 0.0).unwrap();
        assert_eq!(fixture.airspyhf_state(5), 0);
        device.set_tuner_agc(true).unwrap();
        assert_eq!(fixture.airspyhf_state(6), 1);
    }

    /// It does not stream yet, and says so.
    #[test]
    fn starting_rx_says_it_cannot_stream_yet() {
        let (_fixture, device) = opened();
        let (ctx, _s, _d, _n) = crate::hardware::process::tests::rx_ctx();
        let why = device.start_rx(ctx).unwrap_err().to_string();
        assert!(why.contains("cannot stream yet"), "{why}");
        assert!(!device.is_streaming());
    }
}
