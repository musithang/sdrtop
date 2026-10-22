// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The Airspy R2, driven through libairspy.
//!
//! Found, identified, opened, described and streamed here; it is reached
//! only when `--device airspy` asks for it by name, and the SoapySDR path
//! keeps offering the radio as before.
//!
//! **libairspy owns the threads.** Its USB thread queues whole buffers and
//! its consumer thread converts them to I/Q and calls [`rx_callback`], which
//! hands the block to [`crate::hardware::intake`] and returns. What the
//! library's own queue had no room for arrives as `dropped_samples` on the
//! next block, and goes in as the driver's drop before it: counted, and a gap
//! at a known place in the stream.
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

use std::sync::{Arc, Mutex};

use crate::hardware::traits::RateSet;
use crate::hardware::{
    AcquisitionKind, DeliveryModel, DeviceCapabilities, DeviceInfo, DeviceKind, DeviceListing,
    DeviceOption, GainCurve, GainModel, LevelUnit, RxContext, SampleFormat, SampleGeometry,
    SdrDevice, StageSpec, IQ_TRACE_STALE_MS,
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

// libairspy's two presets, as `airspy.c` (1.0.10, lines 117-122) holds them:
// register positions, indexed from the top of the curve down.
const LINEARITY_VGA: [u32; 22] = [
    13, 12, 11, 11, 11, 11, 11, 10, 10, 10, 10, 10, 10, 10, 10, 10, 9, 8, 7, 6, 5, 4,
];
const LINEARITY_MIXER: [u32; 22] = [
    12, 12, 11, 9, 8, 7, 6, 6, 5, 0, 0, 1, 0, 0, 2, 2, 1, 1, 1, 1, 0, 0,
];
const LINEARITY_LNA: [u32; 22] = [
    14, 14, 14, 13, 12, 10, 9, 9, 8, 9, 8, 6, 5, 3, 1, 0, 0, 0, 0, 0, 0, 0,
];
const SENSITIVITY_VGA: [u32; 22] = [
    13, 12, 11, 10, 9, 8, 7, 6, 5, 5, 5, 5, 5, 4, 4, 4, 4, 4, 4, 4, 4, 4,
];
const SENSITIVITY_MIXER: [u32; 22] = [
    12, 12, 12, 12, 11, 10, 10, 9, 9, 8, 7, 4, 4, 4, 3, 2, 2, 1, 0, 0, 0, 0,
];
const SENSITIVITY_LNA: [u32; 22] = [
    14, 14, 14, 14, 14, 14, 14, 14, 14, 13, 12, 12, 9, 9, 8, 7, 6, 5, 3, 2, 1, 0,
];

/// A preset as a curve over the three stages. `airspy_set_*_gain(v)` reads
/// entry `21 - v` of each table, with both AGCs off, and does nothing else:
/// so step `v` here is that call's setting, made through the three stage
/// setters, and the state knows every position it put the radio at.
fn preset(
    name: &'static str,
    short: &'static str,
    lna: &[u32; 22],
    mixer: &[u32; 22],
    vga: &[u32; 22],
) -> GainCurve {
    let steps = (0..22)
        .map(|v| {
            let i = 21 - v;
            vec![lna[i], mixer[i], vga[i]]
        })
        .collect();
    GainCurve::new(name, short, steps)
}

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
    // `[A]` is the R820T's own AGC on the LNA and the mixer, both together;
    // the VGA has none and stays where the keys put it.
    .with_boost(crate::hardware::Boost::GainMode)
    .with_auto_stages(&[0, 1])
    // The knob walks a vendor curve unless a stage is picked by hand. It
    // starts on Linearity, which keeps the LNA low until the middle of the
    // curve and so is the harder to overdrive near strong stations, at step
    // 10, the middle: a choice, not a measurement.
    .with_curves(
        vec![
            preset(
                "Linearity",
                "LIN",
                &LINEARITY_LNA,
                &LINEARITY_MIXER,
                &LINEARITY_VGA,
            ),
            preset(
                "Sensitivity",
                "SEN",
                &SENSITIVITY_LNA,
                &SENSITIVITY_MIXER,
                &SENSITIVITY_VGA,
            ),
        ],
        "Linearity",
        10,
    )
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

// ── RX callback (libairspy's consumer thread) ──────────────────────────────

extern "C" fn rx_callback(transfer: *mut ffi::Transfer) -> c_int {
    // No Rust panic may cross into C.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: libairspy hands a transfer that is valid for this call,
        // whose `ctx` is the `RxContext` the device keeps alive (see
        // `Stream`).
        unsafe { rx_block(transfer) }
    }));
    // Never asks libairspy to stop: 1.0.10 answers a non-zero return by
    // ending its consumer thread while still reporting the stream as running,
    // which would read as a live stream that delivers nothing.
    0
}

/// # Safety
/// `transfer` is null or a transfer libairspy passed to the callback, with
/// `ctx` null or a live `RxContext`.
unsafe fn rx_block(transfer: *mut ffi::Transfer) {
    // Stamped first, so the timing bench measures the interval between
    // deliveries and not entry plus our own work.
    let now = std::time::Instant::now();
    // SAFETY: the caller's contract.
    let Some(t) = (unsafe { transfer.as_ref() }) else {
        return;
    };
    // Only the type set at open is understood; anything else is not samples
    // of ours, and is not read as if it were.
    if t.ctx.is_null() || t.samples.is_null() || t.sample_type != ffi::SAMPLE_INT16_IQ {
        return;
    }
    let Ok(pairs) = usize::try_from(t.sample_count) else {
        return;
    };
    // SAFETY: the caller's contract.
    let ctx = unsafe { &*(t.ctx as *const RxContext) };
    let len = pairs * ctx.geometry.bytes_per_pair();
    // SAFETY: an INT16_IQ transfer holds `sample_count` pairs of two int16.
    let buf = unsafe { std::slice::from_raw_parts(t.samples as *const u8, len) };
    crate::hardware::intake::deliver(ctx, buf, t.dropped_samples, now);
}

/// What the device keeps while libairspy's threads may call back into it.
#[derive(Default)]
struct Stream {
    /// The context handed to the running stream, kept alive until
    /// libairspy has joined the threads that use it.
    ctx: Option<Arc<RxContext>>,
    /// Contexts of streams libairspy ended by itself (a USB transfer that came
    /// back short, or the radio leaving the bus). 1.0.10 then joins nothing
    /// on stop, and its consumer thread can be left waiting with the pointer,
    /// so they are never let go; nor is the handle closed (see `Drop`).
    stranded: Vec<Arc<RxContext>>,
    /// The rate last set, Hz: libairspy has no getter, and a change while
    /// streaming costs a restart, which the same rate does not need.
    rate: Option<u32>,
    /// What the two options are set to; libairspy has no getter for either.
    bias_tee: bool,
    packing: bool,
}

/// An open Airspy R2.
pub struct AirspyDevice {
    api: &'static ffi::AirspyApi,
    ptr: *mut c_void,
    caps: DeviceCapabilities,
    info: DeviceInfo,
    notes: Vec<String>,
    /// Held across every start, stop and restart, so the RX task's
    /// [`SdrDevice::is_streaming`] never sees the gap inside a rate change as
    /// a stream that stopped.
    stream: Mutex<Stream>,
}

// Safety: the handle is only used through libairspy, which serialises its
// own control transfers; starting, stopping and the rate are serialised by
// `stream`, and the threads libairspy starts are its own.
unsafe impl Send for AirspyDevice {}
unsafe impl Sync for AirspyDevice {}

impl AirspyDevice {
    /// Opens the radio a listing names, by its serial, with the `[airspy]`
    /// settings.
    pub fn open(
        listing: &DeviceListing,
        settings: &crate::config::AirspySettings,
    ) -> anyhow::Result<Self> {
        Self::open_with_api(ffi::api()?, listing, settings)
    }

    fn open_with_api(
        api: &'static ffi::AirspyApi,
        listing: &DeviceListing,
        settings: &crate::config::AirspySettings,
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
            stream: Mutex::new(Stream::default()),
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
        device.check(
            unsafe { (api.airspy_set_packing)(ptr, u8::from(settings.packing)) },
            "set packing",
        )?;
        device.stream().packing = settings.packing;
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

    fn stream(&self) -> std::sync::MutexGuard<'_, Stream> {
        self.stream.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Whether libairspy says its stream runs.
    fn library_streaming(&self) -> bool {
        // SAFETY: the handle is open.
        unsafe { (self.api.airspy_is_streaming)(self.ptr) != 0 }
    }

    /// Starts libairspy's threads on `ctx`, which `stream` keeps.
    fn start_locked(&self, stream: &mut Stream, ctx: Arc<RxContext>) -> anyhow::Result<()> {
        let user = Arc::as_ptr(&ctx) as *mut c_void;
        stream.ctx = Some(ctx);
        // SAFETY: the handle is open; `user` stays valid while `stream` holds it.
        let code = unsafe { (self.api.airspy_start_rx)(self.ptr, rx_callback, user) };
        if code != 0 {
            stream.ctx = None;
        }
        self.check(code, "start streaming")
    }

    /// Makes `change` with the stream stopped, when one is running, and starts
    /// it again after: for the settings libairspy refuses, or should not be
    /// given, on a running stream. The restarted stream counts its positions
    /// from zero, as any new stream does, so no block of it is joined to one
    /// from before the change.
    fn restart_around(
        &self,
        stream: &mut Stream,
        change: impl FnOnce(&mut Stream) -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        let running = stream.ctx.clone().filter(|_| self.library_streaming());
        if running.is_some() {
            self.stop_locked(stream)?;
        }
        change(stream)?;
        if let Some(ctx) = running {
            ctx.begin_stream();
            self.start_locked(stream, ctx)
                .map_err(|e| e.context("the stream stopped for the change"))?;
        }
        Ok(())
    }

    /// Stops libairspy's stream and lets the context go once nothing can
    /// call back with it.
    fn stop_locked(&self, stream: &mut Stream) -> anyhow::Result<()> {
        // 1.0.10 joins its threads only for a stream it still calls running;
        // one that ended by itself leaves its consumer thread behind.
        let joined = self.library_streaming();
        // SAFETY: the handle is open.
        let code = unsafe { (self.api.airspy_stop_rx)(self.ptr) };
        if let Some(ctx) = stream.ctx.take() {
            if !joined {
                stream.stranded.push(ctx);
            }
        }
        self.check(code, "stop streaming")
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
        let mut stream = self.stream();
        if stream.ctx.is_some() && self.library_streaming() {
            return Ok(());
        }
        // A stream that ended by itself is stopped before a new one starts,
        // so its context is accounted for.
        if stream.ctx.is_some() {
            self.stop_locked(&mut stream)?;
        }
        let started = self.start_locked(&mut stream, ctx);
        // A radio that fell off the bus comes back as a new USB device, which
        // this handle no longer reaches.
        if started.is_err() && !stream.stranded.is_empty() {
            return started.map_err(|e| {
                e.context(
                    "the last stream ended by itself; if the R2 left the USB bus, \
                     open it again from the menu or restart sdrtop",
                )
            });
        }
        started
    }

    fn stop_rx(&self) -> anyhow::Result<()> {
        self.stop_locked(&mut self.stream())
    }

    fn is_streaming(&self) -> bool {
        let _stream = self.stream();
        self.library_streaming()
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
    ///
    /// **A change while streaming restarts the stream.** libairspy does not
    /// refuse it, but sets the rate by clearing the bulk endpoint the running
    /// transfers are on; stopped first, nothing is in flight. The restarted
    /// stream counts its positions from zero, as any new stream does, so no
    /// block of it is joined to one at the old rate.
    fn set_sample_rate(&self, hz: f64) -> anyhow::Result<RateSet> {
        let Some(&rate) = R2_RATES.iter().find(|&&r| (f64::from(r) - hz).abs() < 0.5) else {
            anyhow::bail!("the Airspy R2 runs at 2.5 or 10 Msps");
        };
        let mut stream = self.stream();
        let running = stream.ctx.is_some() && self.library_streaming();
        if !running || stream.rate != Some(rate) {
            self.restart_around(&mut stream, |stream| {
                // SAFETY: the handle is open.
                self.check(
                    unsafe { (self.api.airspy_set_samplerate)(self.ptr, rate) },
                    "set the rate",
                )?;
                stream.rate = Some(rate);
                Ok(())
            })?;
        }
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

    /// The LNA and mixer AGC, together.
    fn set_tuner_agc(&self, on: bool) -> anyhow::Result<()> {
        // SAFETY: the handle is open.
        self.check(
            unsafe { (self.api.airspy_set_lna_agc)(self.ptr, u8::from(on)) },
            "switch the LNA AGC",
        )?;
        self.check(
            unsafe { (self.api.airspy_set_mixer_agc)(self.ptr, u8::from(on)) },
            "switch the mixer AGC",
        )
    }

    fn options(&self) -> Vec<DeviceOption> {
        let stream = self.stream();
        let on_off = |on: bool| if on { "On" } else { "Off" }.to_string();
        vec![
            DeviceOption {
                id: "bias_tee".into(),
                label: "Bias tee (4.5 V)".into(),
                choices: vec!["Off".into(), "On".into()],
                selected_choice: on_off(stream.bias_tee),
                integer_range: None,
            },
            DeviceOption {
                id: "packing".into(),
                label: "12-bit packing".into(),
                choices: vec!["Off".into(), "On".into()],
                selected_choice: on_off(stream.packing),
                integer_range: None,
            },
        ]
    }

    fn set_option(&self, id: &str, choice: &str) -> anyhow::Result<()> {
        let on = match choice {
            "On" => true,
            "Off" => false,
            _ => anyhow::bail!("{choice} is not a choice for {id}"),
        };
        let mut stream = self.stream();
        match id {
            "bias_tee" => {
                // SAFETY: the handle is open.
                self.check(
                    unsafe { (self.api.airspy_set_rf_bias)(self.ptr, u8::from(on)) },
                    "switch the bias tee",
                )?;
                stream.bias_tee = on;
                Ok(())
            }
            // libairspy refuses packing on a running stream, so it restarts.
            "packing" => self.restart_around(&mut stream, |stream| {
                // SAFETY: the handle is open.
                self.check(
                    unsafe { (self.api.airspy_set_packing)(self.ptr, u8::from(on)) },
                    "set packing",
                )?;
                stream.packing = on;
                Ok(())
            }),
            _ => anyhow::bail!("the Airspy R2 has no option {id}"),
        }
    }

    /// Packing only: the bias tee starts off every session, whatever the last
    /// one ended with.
    fn update_config(&self, config: &mut crate::config::AppConfig) -> anyhow::Result<()> {
        config.airspy.packing = self.stream().packing;
        Ok(())
    }

    /// Smaller while packed: libairspy 1.0.10 fills a 147456-byte packed
    /// buffer (6144 x 24) instead of 262144, which unpacks to 98304 real
    /// samples, 49152 I/Q pairs.
    fn samples_per_transfer(&self) -> u64 {
        if self.stream().packing {
            49_152
        } else {
            self.caps.samples_per_transfer
        }
    }
}

impl Drop for AirspyDevice {
    fn drop(&mut self) {
        let mut stream = self.stream();
        if stream.ctx.is_some() {
            let _ = self.stop_locked(&mut stream);
        }
        // A stream libairspy ended by itself leaves transfers it never reaped,
        // and 1.0.10's close frees them after tearing down the USB context
        // they belong to: on an R2 that fell off the bus, that crashed. Such
        // a handle is held, with its context, until the process exits and the
        // system releases both.
        if !stream.stranded.is_empty() {
            return;
        }
        drop(stream);
        // SAFETY: the handle was opened in `open` and is closed once, here,
        // after its stream.
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

/// An R2 on the fixture libairspy, for tests elsewhere that need to see
/// what a key or a reset actually wrote into the radio.
#[cfg(test)]
pub(crate) mod bench {
    use super::super::test_support::TestLibrary;
    use super::*;

    pub(crate) struct BenchR2 {
        fixture: TestLibrary,
        pub device: Arc<dyn SdrDevice>,
    }

    impl BenchR2 {
        pub(crate) fn new() -> Self {
            let fixture = TestLibrary::new(false);
            // Lives as long as the test process, as the real library does once
            // loaded.
            let api: &'static ffi::AirspyApi =
                Box::leak(Box::new(ffi::resolve(fixture.library()).unwrap()));
            let listing = DeviceListing {
                kind: DeviceKind::Airspy,
                index: 0,
                label: "Airspy R2 · 637862dc2e1182d7".into(),
                serial: Some("637862dc2e1182d7".into()),
                args: None,
                path: None,
                tiny_sa_input: None,
            };
            let device = AirspyDevice::open_with_api(api, &listing, &Default::default())
                .expect("the R2 opens");
            Self {
                fixture,
                device: Arc::new(device),
            }
        }

        /// The register positions last written to the LNA, mixer and VGA;
        /// -1 for one never written.
        pub(crate) fn registers(&self) -> [i32; 3] {
            [4, 5, 6].map(|field| self.fixture.airspy_state(field))
        }
    }
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
        let device = AirspyDevice::open_with_api(api, &listing(), &Default::default())
            .expect("the R2 opens");
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

    /// **The presets are libairspy's own tables**, step `v` reading entry
    /// `21 - v`: the top of Linearity is LNA 14, mixer 12, VGA 13, its bottom
    /// LNA 0, mixer 0, VGA 4, and Sensitivity 12 is LNA 13, mixer 8, VGA 5.
    /// The knob starts on Linearity 10.
    #[test]
    fn the_presets_are_libairspys_tables() {
        let g = gain_model();
        let stages = g.stages();
        let curves = g.curves();
        assert_eq!(
            curves
                .iter()
                .map(|c| (c.name, c.short, c.last_step()))
                .collect::<Vec<_>>(),
            [("Linearity", "LIN", 21), ("Sensitivity", "SEN", 21)]
        );
        let lin = &curves[0];
        assert_eq!(lin.values(21, &stages), [36.02, 15.90, 43.65]);
        assert_eq!(lin.values(0, &stages), [0.0, 0.0, 13.64]);
        assert_eq!(curves[1].values(12, &stages), [32.34, 10.53, 16.91]);
        assert_eq!(g.default_curve(), Some(0));
        assert_eq!(g.start_gains(Some(0)), [16.74, 1.14, 33.51]);
        assert_eq!(lin.step_of(&stages, &[16.74, 1.14, 33.51]), Some(10));
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
        let why = AirspyDevice::open_with_api(api, &listing(), &Default::default())
            .err()
            .unwrap()
            .to_string();
        assert!(
            why.contains("Airspy Mini") && why.contains("soapy=driver=airspy"),
            "{why}"
        );
        fixture.mode(21);
        let why = AirspyDevice::open_with_api(api, &listing(), &Default::default())
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

    /// The libairspy transfer, as the C compiler lays it out.
    #[test]
    fn the_transfer_matches_the_c_layout() {
        use std::mem::{offset_of, size_of};
        let fixture = TestLibrary::new(false);
        assert_eq!(fixture.airspy_layout(3), size_of::<ffi::Transfer>());
        assert_eq!(fixture.airspy_layout(4), offset_of!(ffi::Transfer, samples));
        assert_eq!(
            fixture.airspy_layout(5),
            offset_of!(ffi::Transfer, sample_count)
        );
        assert_eq!(
            fixture.airspy_layout(6),
            offset_of!(ffi::Transfer, dropped_samples)
        );
        assert_eq!(
            fixture.airspy_layout(7),
            offset_of!(ffi::Transfer, sample_type)
        );
    }

    const WAIT: std::time::Duration = std::time::Duration::from_secs(5);

    fn streaming_ctx() -> (
        Arc<RxContext>,
        crossbeam_channel::Receiver<crate::hardware::StreamBlock>,
    ) {
        let (ctx, _fft, _demod, net) =
            crate::hardware::process::tests::rx_ctx_of(caps().sample_geometry);
        (ctx, net)
    }

    /// **Blocks arrive from libairspy's thread with its losses placed**: the
    /// samples as the library converted them, and the buffer its queue had
    /// no room for counted as a drop and left as a gap before the block it
    /// preceded. Stopping joins the threads and lets the context go.
    #[test]
    fn blocks_arrive_with_the_librarys_losses_placed() {
        let (fixture, device) = opened();
        let (ctx, net) = streaming_ctx();
        device.start_rx(Arc::clone(&ctx)).unwrap();
        assert!(device.is_streaming());
        let blocks: Vec<_> = (0..3).map(|_| net.recv_timeout(WAIT).unwrap()).collect();
        assert_eq!(
            blocks.iter().map(|b| b.first_pair).collect::<Vec<_>>(),
            [0, 8, 12],
            "four pairs lost before the second block"
        );
        assert_eq!(
            blocks.iter().map(|b| b.gap_before).collect::<Vec<_>>(),
            [false, true, false]
        );
        let first: Vec<i16> = blocks[0]
            .bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&b| i16::from_le_bytes(b))
            .collect();
        assert_eq!(first, [0, 0, 1, -1, 2, -2, 3, -3]);
        assert_eq!(ctx.metrics.lock().unwrap().acc.drops, 4);
        device.stop_rx().unwrap();
        assert!(!device.is_streaming());
        assert_eq!(fixture.airspy_state(10), 1);
        assert_eq!(Arc::strong_count(&ctx), 1, "nothing can call back now");
    }

    /// **A rate change while streaming restarts the stream at the new rate**,
    /// counting from zero; the same rate again restarts nothing, and before
    /// a stream there is nothing to restart.
    #[test]
    fn a_rate_change_while_streaming_restarts_the_stream() {
        let (fixture, device) = opened();
        let (ctx, net) = streaming_ctx();
        device.set_sample_rate(10e6).unwrap();
        assert_eq!(fixture.airspy_state(9), 0, "not streaming, nothing started");
        device.start_rx(Arc::clone(&ctx)).unwrap();
        for _ in 0..3 {
            net.recv_timeout(WAIT).unwrap();
        }
        device.set_sample_rate(10e6).unwrap();
        assert_eq!((fixture.airspy_state(9), fixture.airspy_state(10)), (1, 0));
        device.set_sample_rate(2.5e6).unwrap();
        assert_eq!((fixture.airspy_state(9), fixture.airspy_state(10)), (2, 1));
        assert_eq!(
            fixture.airspy_state(11),
            2_500_000,
            "started at the new rate"
        );
        assert!(device.is_streaming());
        assert_eq!(net.recv_timeout(WAIT).unwrap().first_pair, 0);
        device.stop_rx().unwrap();
    }

    /// **A stream libairspy ended by itself keeps its context**: the
    /// library's consumer thread may still hold it. The RX task sees the
    /// stream stopped, and a new start begins cleanly.
    #[test]
    fn a_stream_that_ended_by_itself_keeps_its_context() {
        let (fixture, device) = opened();
        let (ctx, net) = streaming_ctx();
        fixture.mode(24);
        device.start_rx(Arc::clone(&ctx)).unwrap();
        for _ in 0..3 {
            net.recv_timeout(WAIT).unwrap();
        }
        let deadline = std::time::Instant::now() + WAIT;
        while device.is_streaming() {
            assert!(
                std::time::Instant::now() < deadline,
                "the stream never ended"
            );
            std::thread::yield_now();
        }
        device.stop_rx().unwrap();
        assert_eq!(Arc::strong_count(&ctx), 2, "kept, not let go");
        fixture.mode(0);
        device.start_rx(Arc::clone(&ctx)).unwrap();
        assert!(device.is_streaming());
        assert_eq!(fixture.airspy_state(9), 2);
        device.stop_rx().unwrap();
    }

    /// **A radio whose stream libairspy ended by itself is never closed.**
    /// 1.0.10's close tears the USB context down before freeing transfers
    /// the dead stream never reaped, and crashed doing it when the R2 fell
    /// off the bus. A radio stopped cleanly is closed as usual.
    #[test]
    fn a_radio_whose_stream_died_is_not_closed() {
        let (fixture, device) = opened();
        let (ctx, net) = streaming_ctx();
        device.start_rx(Arc::clone(&ctx)).unwrap();
        net.recv_timeout(WAIT).unwrap();
        device.stop_rx().unwrap();
        drop(device);
        assert_eq!(fixture.calls(3), 1, "stopped cleanly, closed");

        let (fixture, device) = opened();
        let (ctx, net) = streaming_ctx();
        fixture.mode(24);
        device.start_rx(Arc::clone(&ctx)).unwrap();
        for _ in 0..3 {
            net.recv_timeout(WAIT).unwrap();
        }
        let deadline = std::time::Instant::now() + WAIT;
        while device.is_streaming() {
            assert!(
                std::time::Instant::now() < deadline,
                "the stream never ended"
            );
            std::thread::yield_now();
        }
        device.stop_rx().unwrap();
        fixture.mode(23);
        let why = format!("{:#}", device.start_rx(Arc::clone(&ctx)).unwrap_err());
        assert!(
            why.contains("ended by itself") && why.contains("restart"),
            "{why}"
        );
        drop(device);
        assert_eq!(fixture.calls(3), 0, "held, not closed");
    }

    fn option(device: &AirspyDevice, id: &str) -> String {
        device
            .options()
            .into_iter()
            .find(|o| o.id == id)
            .map(|o| o.selected_choice)
            .unwrap_or_default()
    }

    /// **Two options, both off at first**: the bias tee and packing.
    /// Switching the bias tee reaches the radio both ways; a choice it does
    /// not offer is refused.
    #[test]
    fn the_bias_tee_is_an_option_that_reaches_the_radio() {
        let (fixture, device) = opened();
        let ids: Vec<String> = device.options().into_iter().map(|o| o.id).collect();
        assert_eq!(ids, ["bias_tee", "packing"]);
        assert_eq!(
            (option(&device, "bias_tee"), option(&device, "packing")),
            ("Off".into(), "Off".into())
        );
        device.set_option("bias_tee", "On").unwrap();
        assert_eq!(
            (fixture.airspy_state(1), option(&device, "bias_tee")),
            (1, "On".into())
        );
        device.set_option("bias_tee", "Off").unwrap();
        assert_eq!(fixture.airspy_state(1), 0);
        assert!(device.set_option("bias_tee", "Maybe").is_err());
        // The Options pane shows 18 columns of a label, and a cut one read
        // "Bias tee, 4.5 V on" live.
        for o in device.options() {
            assert!(o.label.chars().count() <= 18, "{}", o.label);
        }
        assert!(device.set_option("turbo", "On").is_err());
    }

    /// **Packing while streaming restarts the stream with it**, since
    /// libairspy refuses it on a running one; the block the timing bench
    /// expects shrinks to the packed transfer's 49152 pairs.
    #[test]
    fn packing_while_streaming_restarts_the_stream() {
        let (fixture, device) = opened();
        let (ctx, net) = streaming_ctx();
        device.start_rx(Arc::clone(&ctx)).unwrap();
        for _ in 0..3 {
            net.recv_timeout(WAIT).unwrap();
        }
        assert_eq!(device.samples_per_transfer(), 65_536);
        device.set_option("packing", "On").unwrap();
        assert_eq!((fixture.airspy_state(9), fixture.airspy_state(10)), (2, 1));
        assert_eq!(fixture.airspy_state(13), 1, "the new stream is packed");
        assert!(device.is_streaming());
        assert_eq!(device.samples_per_transfer(), 49_152);
        device.stop_rx().unwrap();
    }

    /// **Packing is kept, the bias tee never**: opened with packing on, the
    /// radio is packed at open; the config takes packing back, and has
    /// nowhere to keep the bias tee.
    #[test]
    fn packing_is_kept_and_the_bias_tee_is_not() {
        let fixture = TestLibrary::new(false);
        let api: &'static ffi::AirspyApi =
            Box::leak(Box::new(ffi::resolve(fixture.library()).unwrap()));
        let packed = crate::config::AirspySettings { packing: true };
        let device = AirspyDevice::open_with_api(api, &listing(), &packed).unwrap();
        assert_eq!(fixture.airspy_state(12), 1);
        device.set_option("bias_tee", "On").unwrap();
        let mut cfg = crate::config::AppConfig::default();
        device.update_config(&mut cfg).unwrap();
        assert_eq!(cfg.airspy, packed);
    }

    /// **`[A]` is both AGCs**, on and off together; the VGA has none.
    #[test]
    fn the_agc_switches_the_lna_and_mixer_agc_together() {
        let (fixture, device) = opened();
        device.set_tuner_agc(true).unwrap();
        assert_eq!((fixture.airspy_state(2), fixture.airspy_state(3)), (1, 1));
        device.set_tuner_agc(false).unwrap();
        assert_eq!((fixture.airspy_state(2), fixture.airspy_state(3)), (0, 0));
        let g = gain_model();
        assert!(matches!(g.boost(), Some(crate::hardware::Boost::GainMode)));
        assert_eq!(
            (0..3).map(|i| g.auto_governs(i)).collect::<Vec<_>>(),
            [true, true, false]
        );
    }

    /// A stream that would not start leaves nothing behind and says why.
    #[test]
    fn a_refused_start_says_so_and_keeps_nothing() {
        let (fixture, device) = opened();
        let (ctx, _net) = streaming_ctx();
        fixture.mode(23);
        let why = device.start_rx(Arc::clone(&ctx)).unwrap_err().to_string();
        assert!(why.contains("could not start streaming"), "{why}");
        assert_eq!(Arc::strong_count(&ctx), 1);
        assert!(!device.is_streaming());
    }
}
