// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The Airspy HF+ Discovery, driven through libairspyhf.
//!
//! Found, identified, opened, described and streamed here. Offered unasked
//! for a Discovery (or a radio another program holds); the other boards only
//! when `--device airspyhf` names this backend ([`offered`]).
//!
//! **libairspyhf owns the threads.** Its USB thread queues whole buffers and
//! its consumer thread converts them to floats and calls [`rx_callback`],
//! which turns them back into the radio's words (with fraction bits, so the
//! library's correction is not rounded away) and hands the block to
//! [`crate::hardware::intake`]. The library hides the scale it divided the
//! words by; each stream reads it from its first block ([`filter_gain_db`]).
//!
//! **Its gains are what they say.** The attenuator's 6 dB steps and the LNA's
//! 6 dB were measured on the bench against steady carriers, at HF and VHF.
//! Both are digital gain after the converter, behind the radio's own AGCs,
//! which are always on: they place the level in the samples, not the signal
//! at the converter ([`gain_model`]). The attenuator is a negative gain
//! (`-48..0 dB`), ahead of the LNA.
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
    DeviceOption, GainModel, LevelUnit, RxContext, SampleFormat, SampleGeometry, SdrDevice,
    StageSpec, IQ_TRACE_STALE_MS,
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

/// Whether a radio [`find`] returned is offered without being asked for by
/// name: a Discovery, the one board driven natively, and a held radio, which
/// cannot say what it is and is only offered so that observer mode can show
/// it. The other HF+ boards stay with SoapyAirspyHF unless asked for, where
/// they work and here they would be refused.
pub fn offered(found: &Found, asked_by_name: bool) -> bool {
    asked_by_name || found.board == Board::Discovery || found.held
}

/// The device selector's entries for what [`find`] returned and [`offered`]
/// lets through.
pub fn list(asked_by_name: bool) -> Vec<DeviceListing> {
    let Ok(api) = ffi::api() else {
        return Vec::new();
    };
    find(api)
        .into_iter()
        .filter(|found| offered(found, asked_by_name))
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

/// Where the samples are centred after tuning to `freq_hz`: there, with the
/// library's correction on (it turns the samples onto it); on the LO, the
/// firmware's kHz grid point on the true scale, with it off.
pub fn tuned_centre_hz(freq_hz: u64, low_if: bool, dsp: bool, ppb: i32) -> u64 {
    if dsp {
        freq_hz
    } else {
        freq_hz.saturating_add_signed(lo_offset_hz(freq_hz, low_if, false, ppb))
    }
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
    // Both are digital gain after the converter, behind the radio's own RF,
    // IF and digital AGCs, which are always on (what every measurement here
    // shows): they place the level in the samples.
    .with_digital_stages("AGC")
    .with_no_cascade_reason("ATT and LNA are digital, after the converter")
    .with_boost(crate::hardware::Boost::GainMode)
    .with_auto_stages(&[0])
    .with_start_gains(vec![0.0, 0.0])
    // The radio's digital AGC, always on, turns its own gain down to hold a
    // carrier's sample peak, at the same level on the USB word at every rate
    // (measured on a cable, all seven rates, both bands, each within 0.2 dB).
    // Where it holds depends on what came before: a carrier that arrives well
    // past it is held at -13.5 dBFS on HF and -14.8 on VHF, and the next gain
    // command sent while it is there lifts the hold to -11.6 and -13.3. It
    // cuts gain cleanly and does not report how much, so from the lower level
    // up the level may no longer be the antenna's, and nothing on the word
    // ever clips.
    .with_held_ceiling(
        "digital AGC",
        vec![(0, -13.5, -11.6), (VHF_FROM_HZ, -14.8, -13.3)],
    )
}

/// Where the VHF front end starts.
const VHF_FROM_HZ: u64 = 60_000_000;

/// The rates a Discovery's firmware lists (R5.0.1-CD, read from the radio),
/// for describing one that is observed and so cannot be asked.
const DISCOVERY_RATES: [u32; 7] = [
    912_000, 768_000, 650_000, 456_000, 384_000, 228_000, 192_000,
];

/// A Discovery's capabilities as placeholders, for observer mode: the bands,
/// stages and ceiling are the board's, the rates its firmware lists. The
/// block is the library's, which only an open radio reports, so it is left
/// unknown.
pub fn observer_caps() -> DeviceCapabilities {
    caps(&DISCOVERY_RATES, 0)
}

/// The firmware slot of the header for an HF+ observed, not opened: under
/// the HF+'s own label, with the version it cannot read as a dash.
pub fn observer_stack() -> crate::hardware::SoftwareStack {
    crate::hardware::SoftwareStack {
        label: "airspyhf fw ",
        value: Arc::from("\u{2014}"),
    }
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
        freq_gaps: vec![std::ops::RangeInclusive::new(31_000_001, VHF_FROM_HZ - 1)],
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

// ── The stream (libairspyhf's consumer thread) ─────────────────────────────

/// The firmware's filter gain, whole dB, from a block taken with the
/// library's DSP off, or `None` when the block cannot tell.
///
/// libairspyhf hands each 16-bit word as `word * (1/32768 * 10^(-g/20))`,
/// computed in float, and never says what `g` is; it differs from rate to
/// rate and may from firmware to firmware. With the DSP off nothing else
/// touches the samples, so the right `g` is one that turns every sample back
/// into a whole word. A larger `g` by 20 dB turns them into whole words too
/// (ten times each), so the smallest that fits is taken, and only where some
/// word is not a multiple of ten: a block of zeros, or of tens, tells nothing.
/// Only ever given blocks taken with the DSP off: samples carrying decimal
/// fractions of a word would fit a `g` 20 or 40 dB too high, and the DSP's
/// are not whole words at any gain.
pub fn filter_gain_db(samples: &[f32]) -> Option<u8> {
    (0u8..=60).find(|&g| {
        let per_word = f64::from(word_gain(g));
        let mut telling = false;
        for &x in samples {
            let word = f64::from(x) / per_word;
            if (word - word.round()).abs() > 1e-3 {
                return false;
            }
            telling |= (word.round() as i64) % 10 != 0;
        }
        telling
    })
}

/// What libairspyhf multiplies a word by at filter gain `g`, in float as it
/// computes it (`airspyhf.c`, `convert_samples`).
fn word_gain(g: u8) -> f32 {
    (1.0f32 / 32768.0) * 10f32.powf(f32::from(g) * -0.05)
}

/// Counts per float at filter gain `g`: the USB word's scale with eight
/// fraction bits below it, the container's full scale at the word's.
fn scale_for(g: u8) -> f64 {
    256.0 / f64::from(word_gain(g))
}

/// What a stream carries from one block to the next. Only libairspyhf's
/// consumer thread converts, so `conversion` is never contended; `tuning` is
/// the device's own, locked once a stream to put the frequency back after the
/// measurement.
struct StreamCtx {
    rx: Arc<RxContext>,
    api: &'static ffi::AirspyHfApi,
    ptr: *mut c_void,
    rate: u32,
    tuning: Arc<Mutex<Tuning>>,
    conversion: Mutex<Conversion>,
}

// Safety: the handle is used from the consumer thread only for the two calls
// that end the measurement, under the device's tuning mutex, which every
// other tuning call takes too.
unsafe impl Send for StreamCtx {}
unsafe impl Sync for StreamCtx {}

#[derive(Default)]
struct Conversion {
    /// Counts per float once the filter gain is known; `None` while the
    /// stream is still measuring it, with the library's DSP off.
    scale: Option<f64>,
    gain_db: Option<u8>,
    /// Blocks that could not tell the gain.
    undecided: u32,
    /// Whether the log has been told what was measured.
    said: bool,
    words: Vec<u8>,
}

extern "C" fn rx_callback(transfer: *mut ffi::Transfer) -> c_int {
    // No Rust panic may cross into C.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: libairspyhf hands a transfer valid for this call, whose
        // `ctx` is the `StreamCtx` the device keeps alive (see `Stream`).
        unsafe { rx_block(transfer) }
    }));
    // Never asks libairspyhf to stop: 1.6.8 answers a non-zero return by
    // ending its consumer thread while still calling the stream running.
    0
}

/// # Safety
/// `transfer` is null or a transfer libairspyhf passed to the callback, with
/// `ctx` null or a live `StreamCtx`.
unsafe fn rx_block(transfer: *mut ffi::Transfer) {
    // Stamped first, so the timing bench measures the interval between
    // deliveries and not entry plus our own work.
    let now = std::time::Instant::now();
    // SAFETY: the caller's contract.
    let Some(t) = (unsafe { transfer.as_ref() }) else {
        return;
    };
    if t.ctx.is_null() || t.samples.is_null() {
        return;
    }
    let Ok(pairs) = usize::try_from(t.sample_count) else {
        return;
    };
    // SAFETY: the caller's contract.
    let ctx = unsafe { &*(t.ctx as *const StreamCtx) };
    // SAFETY: a transfer holds `sample_count` complex floats.
    let floats = unsafe { std::slice::from_raw_parts(t.samples as *const f32, pairs * 2) };
    let mut conv = ctx.conversion.lock().unwrap_or_else(|e| e.into_inner());
    let Some(scale) = conv.scale else {
        measure(ctx, &mut conv, floats);
        return;
    };
    conv.words.clear();
    conv.words.reserve(floats.len() * 4);
    for &x in floats {
        let word = (f64::from(x) * scale)
            .round()
            .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32;
        conv.words.extend_from_slice(&word.to_le_bytes());
    }
    crate::hardware::intake::deliver(&ctx.rx, &conv.words, t.dropped_samples, now);
    if !conv.said {
        conv.said = say_measured(ctx, &conv);
    }
}

/// A block taken while the filter gain is not known yet, with the library's
/// DSP off: read the gain from it if it can tell, and once it does, put the
/// DSP back on and the frequency back as it is meant to be. The block itself
/// is not delivered: uncorrected and unturned, it is not the stream the
/// rest is, so the stream's positions start after it.
fn measure(ctx: &StreamCtx, conv: &mut Conversion, floats: &[f32]) {
    let Some(g) = filter_gain_db(floats) else {
        conv.undecided += 1;
        return;
    };
    conv.gain_db = Some(g);
    conv.scale = Some(scale_for(g));
    let tuning = ctx.tuning.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: the handle is open while a stream runs; the tuning mutex keeps
    // this from crossing a tuning made from another thread. With the DSP back
    // on, a frequency set while it was off gets the IF shift it lacked; the
    // same frequency again moves nothing. With the correction chosen off, the
    // DSP stays off and the tuning is already what it should be.
    if tuning.dsp {
        unsafe {
            (ctx.api.airspyhf_set_lib_dsp)(ctx.ptr, 1);
            if tuning.freq_hz > 0 {
                (ctx.api.airspyhf_set_freq)(ctx.ptr, tuning.freq_hz as u32);
            }
        }
    }
}

/// Tell the log what the stream measured, without ever waiting for the lock:
/// `false` when it is busy, and the next block tries again.
fn say_measured(ctx: &StreamCtx, conv: &Conversion) -> bool {
    let Ok(mut m) = ctx.rx.metrics.try_lock() else {
        return false;
    };
    m.push_log(format!(
        "{}: the USB word's scale at {} kS/s read from the samples: filter gain {} dB",
        Board::Discovery.name(),
        ctx.rate / 1000,
        conv.gain_db.unwrap_or(0),
    ));
    true
}

/// What the device keeps while libairspyhf's threads may call back into it.
#[derive(Default)]
struct Stream {
    /// The context handed to the running stream, kept alive until
    /// libairspyhf has joined the threads that use it.
    ctx: Option<Arc<StreamCtx>>,
    /// Contexts of streams libairspyhf ended by itself (a USB transfer that
    /// came back short, the radio leaving the bus). 1.6.8 then joins nothing
    /// on stop and leaves its transfers live, so they are never let go; nor is
    /// the handle closed (see `Drop`).
    stranded: Vec<Arc<StreamCtx>>,
}

/// Where the radio is tuned, as the device last set it: libairspyhf has no
/// getters.
struct Tuning {
    freq_hz: u64,
    rate: u32,
    low_if: bool,
    /// The library's correction, IF shift and fine tuning, as chosen: on
    /// unless switched off for the raw words.
    dsp: bool,
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
    tuning: Arc<Mutex<Tuning>>,
    /// Held across every start, stop and restart, so the RX task's
    /// [`SdrDevice::is_streaming`] never sees the gap inside a rate change as
    /// a stream that stopped.
    stream: Mutex<Stream>,
}

// Safety: the handle is only used through libairspyhf, which serialises its
// own control transfers; the tuning is behind its mutex.
unsafe impl Send for AirspyHfDevice {}
unsafe impl Sync for AirspyHfDevice {}

impl AirspyHfDevice {
    /// Opens the radio a listing names, by its serial, with the `[airspyhf]`
    /// settings.
    pub fn open(
        listing: &DeviceListing,
        settings: &crate::config::AirspyHfSettings,
    ) -> anyhow::Result<Self> {
        Self::open_with_api(ffi::api()?, listing, settings)
    }

    fn open_with_api(
        api: &'static ffi::AirspyHfApi,
        listing: &DeviceListing,
        settings: &crate::config::AirspyHfSettings,
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
            tuning: Arc::new(Mutex::new(Tuning {
                freq_hz: 0,
                rate: 0,
                low_if: false,
                dsp: true,
            })),
            stream: Mutex::new(Stream::default()),
        };
        // What the firmware and the library would otherwise keep from the last
        // program, stated: the library's correction as saved (on unless the
        // raw words were chosen), the AGC off at its low threshold, no
        // attenuation, the LNA off.
        device.tuning().dsp = settings.dsp;
        // SAFETY: the handle is open and owned by `device`, whose Drop closes it.
        unsafe {
            device.check(
                (api.airspyhf_set_lib_dsp)(ptr, u8::from(settings.dsp)),
                "set the library's DSP",
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
            format!(
                "{name}: library DSP {}, AGC off, no attenuation, LNA off, set at open",
                if settings.dsp {
                    "on"
                } else {
                    "off (saved): the raw words, centred on the kHz grid"
                }
            ),
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

    fn stream(&self) -> std::sync::MutexGuard<'_, Stream> {
        self.stream.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Whether libairspyhf says its stream runs.
    fn library_streaming(&self) -> bool {
        // SAFETY: the handle is open.
        unsafe { (self.api.airspyhf_is_streaming)(self.ptr) != 0 }
    }

    /// Starts libairspyhf's threads on a new context for `rx`, with the
    /// library's DSP off until the first block has told the filter gain at
    /// this rate (see [`filter_gain_db`]).
    fn start_locked(&self, stream: &mut Stream, rx: Arc<RxContext>) -> anyhow::Result<()> {
        let ctx = Arc::new(StreamCtx {
            rx,
            api: self.api,
            ptr: self.ptr,
            rate: self.tuning().rate,
            tuning: Arc::clone(&self.tuning),
            conversion: Mutex::new(Conversion::default()),
        });
        // SAFETY: the handle is open.
        self.check(
            unsafe { (self.api.airspyhf_set_lib_dsp)(self.ptr, 0) },
            "switch the library's DSP off to measure",
        )?;
        let user = Arc::as_ptr(&ctx) as *mut c_void;
        stream.ctx = Some(ctx);
        // SAFETY: the handle is open; `user` stays valid while `stream` holds it.
        let code = unsafe { (self.api.airspyhf_start)(self.ptr, rx_callback, user) };
        if code != 0 {
            stream.ctx = None;
            let dsp = u8::from(self.tuning().dsp);
            // SAFETY: as above.
            unsafe { (self.api.airspyhf_set_lib_dsp)(self.ptr, dsp) };
        }
        self.check(code, "start streaming")
    }

    /// Stops libairspyhf's stream and lets the context go once nothing can
    /// call back with it.
    fn stop_locked(&self, stream: &mut Stream) -> anyhow::Result<()> {
        // 1.6.8 joins its threads only for a stream it still calls running;
        // one that ended by itself leaves its consumer thread behind.
        let joined = self.library_streaming();
        // SAFETY: the handle is open.
        let code = unsafe { (self.api.airspyhf_stop)(self.ptr) };
        if let Some(ctx) = stream.ctx.take() {
            if !joined {
                stream.stranded.push(ctx);
            }
        }
        // The DSP back as chosen whatever the stream left it at, so a tuning
        // made while stopped gets the IF shift it should.
        let dsp = u8::from(self.tuning().dsp);
        // SAFETY: as above.
        unsafe { (self.api.airspyhf_set_lib_dsp)(self.ptr, dsp) };
        self.check(code, "stop streaming")
    }
}

impl SdrDevice for AirspyHfDevice {
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
                    "the last stream ended by itself; if the radio left the USB bus, \
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

    /// With the library's correction on, tuned to the hertz: the firmware
    /// takes whole kHz and the library turns the rest. With it off, to the
    /// firmware's kHz grid, and the frequency returned is that grid point on
    /// the true scale (the flash's correction folded back), which is where the
    /// samples are then centred. Below 180 kHz the lowest LO depends on the
    /// rate's architecture, so with the correction off the centre would move
    /// with the rate; it is refused there instead.
    fn set_frequency(&self, hz: u64) -> anyhow::Result<u64> {
        if !self.caps.tunes(hz) {
            anyhow::bail!(
                "the {} tunes {}, not {:.3} MHz",
                Board::Discovery.name(),
                self.caps.bands_text(),
                hz as f64 / 1e6
            );
        }
        // Under the tuning mutex, which the stream also takes when it puts
        // the frequency back after measuring.
        let mut tuning = self.tuning();
        if !tuning.dsp && hz < MIN_ZERO_IF_LO_HZ {
            anyhow::bail!(
                "with the library's correction off the {} tunes from {} kHz, \
                 where its lowest LO is the same at every rate",
                Board::Discovery.name(),
                MIN_ZERO_IF_LO_HZ / 1000
            );
        }
        // SAFETY: the handle is open; the bands keep `hz` inside u32.
        self.check(
            unsafe { (self.api.airspyhf_set_freq)(self.ptr, hz as u32) },
            "tune",
        )?;
        tuning.freq_hz = hz;
        Ok(tuned_centre_hz(hz, tuning.low_if, tuning.dsp, self.ppb))
    }

    /// One of the rates the firmware lists, set as asked; whether it is a
    /// low-IF one is read back, because where the LO sits depends on it.
    ///
    /// **Never on a running stream.** A rate set while streaming, followed by
    /// a tuning below the lowest low-IF LO, left this radio sending nothing
    /// from its next start until it was unplugged (measured, and reported to
    /// Airspy, who traced it to the silicon). A running stream is stopped,
    /// the rate set, and the stream started again, measuring the new rate's
    /// filter gain afresh; its positions count from zero, as any new stream's
    /// do, so no block of it is joined to one at the old rate.
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
        let mut stream = self.stream();
        let running = stream.ctx.clone().filter(|_| self.library_streaming());
        if running.is_some() {
            self.stop_locked(&mut stream)?;
        }
        {
            let mut tuning = self.tuning();
            // SAFETY: the handle is open.
            self.check(
                unsafe { (self.api.airspyhf_set_samplerate)(self.ptr, rate) },
                "set the rate",
            )?;
            // SAFETY: the handle is open.
            tuning.low_if = unsafe { (self.api.airspyhf_is_low_if)(self.ptr) } != 0;
            tuning.rate = rate;
        }
        if let Some(ctx) = running {
            ctx.rx.begin_stream();
            self.start_locked(&mut stream, Arc::clone(&ctx.rx))
                .map_err(|e| e.context("the stream stopped for the rate change"))?;
        }
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

    fn options(&self) -> Vec<DeviceOption> {
        let on = self.tuning().dsp;
        vec![DeviceOption {
            id: "dsp".into(),
            label: "Library DSP".into(),
            choices: vec!["On".into(), "Off".into()],
            selected_choice: if on { "On" } else { "Off" }.into(),
            integer_range: None,
        }]
    }

    /// The library's DSP on or off. Takes effect from the next block; the
    /// frequency then has to be set again (the IF shift comes and goes with
    /// it, and off, the centre is the kHz grid), which the app does on
    /// [`SdrDevice::option_moves_tuning`]. Below 180 kHz switching it off is
    /// refused, as tuning there with it off is.
    fn set_option(&self, id: &str, choice: &str) -> anyhow::Result<()> {
        if id != "dsp" {
            anyhow::bail!("the {} has no option {id}", Board::Discovery.name());
        }
        let on = match choice {
            "On" => true,
            "Off" => false,
            _ => anyhow::bail!("{choice} is not a choice for {id}"),
        };
        let stream = self.stream();
        let mut tuning = self.tuning();
        if !on && tuning.freq_hz > 0 && tuning.freq_hz < MIN_ZERO_IF_LO_HZ {
            anyhow::bail!(
                "with the library's correction off the {} tunes from {} kHz; tune there first",
                Board::Discovery.name(),
                MIN_ZERO_IF_LO_HZ / 1000
            );
        }
        tuning.dsp = on;
        // A stream still measuring its scale keeps the DSP off until it is
        // done, and then sets it as chosen.
        let measuring = stream.ctx.as_ref().is_some_and(|ctx| {
            ctx.conversion
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .scale
                .is_none()
        });
        if !measuring {
            // SAFETY: the handle is open.
            self.check(
                unsafe { (self.api.airspyhf_set_lib_dsp)(self.ptr, u8::from(on)) },
                "set the library's DSP",
            )?;
        }
        Ok(())
    }

    fn option_moves_tuning(&self, id: &str) -> bool {
        id == "dsp"
    }

    fn update_config(&self, config: &mut crate::config::AppConfig) -> anyhow::Result<()> {
        config.airspyhf.dsp = self.tuning().dsp;
        Ok(())
    }

    /// The firmware's AGC, which sets the attenuator while it is on.
    fn set_tuner_agc(&self, on: bool) -> anyhow::Result<()> {
        // SAFETY: the handle is open.
        self.check(
            unsafe { (self.api.airspyhf_set_hf_agc)(self.ptr, u8::from(on)) },
            "switch the AGC",
        )
    }

    /// The radio's 16-bit I and Q words cross the link, four bytes a pair,
    /// whatever container sdrtop carries them in afterwards.
    fn wire_bytes_per_pair(&self) -> Option<f64> {
        Some(4.0)
    }

    /// With the correction off the samples are centred on the LO itself.
    fn lo_offset_hz(&self) -> i64 {
        let t = self.tuning();
        if t.rate == 0 || t.freq_hz == 0 || !t.dsp {
            return 0;
        }
        lo_offset_hz(t.freq_hz, t.low_if, true, self.ppb)
    }
}

impl Drop for AirspyHfDevice {
    fn drop(&mut self) {
        let mut stream = self.stream();
        if stream.ctx.is_some() {
            let _ = self.stop_locked(&mut stream);
        }
        // A stream libairspyhf ended by itself leaves transfers 1.6.8 never
        // reaped, and its close frees them after tearing down the USB context
        // they belong to. Such a handle is held, with its context, until the
        // process exits and the system releases both.
        if !stream.stranded.is_empty() {
            return;
        }
        drop(stream);
        // SAFETY: the handle was opened in `open` and is closed once, here,
        // after its stream.
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

    /// **Unasked, only a Discovery is offered natively**, and a held radio,
    /// which cannot say what it is, so observer mode can show it; another HF+
    /// board stays with SoapyAirspyHF, where it works, unless asked for.
    #[test]
    fn unasked_only_a_discovery_or_a_held_radio_is_offered() {
        let discovery = Found {
            serial: "dd528080ff444630".into(),
            board: Board::Discovery,
            firmware: None,
            rates: vec![768_000],
            held: false,
        };
        let ranger = Found {
            board: Board::Ranger,
            ..discovery.clone()
        };
        let held = Found {
            board: Board::Unknown,
            held: true,
            ..discovery.clone()
        };
        assert!(offered(&discovery, false));
        assert!(offered(&held, false));
        assert!(!offered(&ranger, false));
        assert!(offered(&ranger, true), "asked for by name");
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
        let device = AirspyHfDevice::open_with_api(api, &listing(), &Default::default())
            .expect("the Discovery opens");
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

    /// With the library's correction on the samples are centred where asked;
    /// off, on the LO: the firmware's kHz grid point, folded back onto the
    /// true scale by the flash's correction.
    #[test]
    fn the_centre_is_the_asked_frequency_or_the_grid() {
        assert_eq!(tuned_centre_hz(7_074_300, false, true, 0), 7_074_300);
        assert_eq!(tuned_centre_hz(7_074_300, false, false, 0), 7_074_000);
        assert_eq!(tuned_centre_hz(7_074_600, true, false, 0), 7_075_000);
        // +250 ppb: 100.000025 MHz in the radio's reference rounds to
        // 100.000 MHz there, 99.999975 MHz on the true scale.
        assert_eq!(tuned_centre_hz(100_000_000, false, false, 250), 99_999_975);
    }

    /// **With the DSP off** the radio opens without it, tunes onto the grid
    /// and says so, puts its LO on the centre, refuses below 180 kHz, and
    /// keeps it off through a stream's measuring: the frequency is not set
    /// again and the words carry no fraction. Switching it on reaches the
    /// library, asks the app to tune again, and is what the config keeps.
    #[test]
    fn the_dsp_off_tunes_on_the_grid_and_stays_off() {
        let fixture = TestLibrary::new(false);
        let api: &'static ffi::AirspyHfApi =
            Box::leak(Box::new(ffi::resolve(fixture.library()).unwrap()));
        let device = AirspyHfDevice::open_with_api(
            api,
            &listing(),
            &crate::config::AirspyHfSettings { dsp: false },
        )
        .expect("the Discovery opens");
        assert_eq!(fixture.airspyhf_state(3), 0, "opened with the DSP off");
        assert_eq!(device.options()[0].selected_choice, "Off");
        device.set_sample_rate(192_000.0).unwrap();
        assert_eq!(device.set_frequency(7_074_300).unwrap(), 7_074_000);
        assert_eq!(device.lo_offset_hz(), 0, "the LO is the centre");
        let why = device.set_frequency(150_000).unwrap_err().to_string();
        assert!(why.contains("from 180 kHz"), "{why}");

        let tuned = fixture.airspyhf_state(11);
        let (ctx, net) = streaming_ctx();
        device.start_rx(Arc::clone(&ctx)).unwrap();
        let block = net.recv_timeout(WAIT).unwrap();
        assert_eq!(fixture.airspyhf_state(3), 0, "the DSP still off");
        assert_eq!(
            fixture.airspyhf_state(11),
            tuned,
            "the frequency left alone"
        );
        assert_eq!(words(&block)[..2], [101 * 256, -(101 * 256)]);
        device.stop_rx().unwrap();
        assert_eq!(fixture.airspyhf_state(3), 0, "and off after stopping");

        assert!(device.option_moves_tuning("dsp"));
        device.set_option("dsp", "On").unwrap();
        assert_eq!(fixture.airspyhf_state(3), 1);
        assert_eq!(device.set_frequency(7_074_300).unwrap(), 7_074_300);
        let mut config = crate::config::AppConfig::default();
        device.update_config(&mut config).unwrap();
        assert!(config.airspyhf.dsp);
        device.set_option("dsp", "Off").unwrap();
        device.update_config(&mut config).unwrap();
        assert!(!config.airspyhf.dsp);
    }

    /// Below 180 kHz, where the lowest LO moves with the rate, switching the
    /// DSP off is refused and it stays on.
    #[test]
    fn the_dsp_stays_on_below_the_zero_if_floor() {
        let (fixture, device) = opened();
        assert_eq!(device.set_frequency(150_000).unwrap(), 150_000);
        let why = device.set_option("dsp", "Off").unwrap_err().to_string();
        assert!(why.contains("tune there first"), "{why}");
        assert_eq!(fixture.airspyhf_state(3), 1);
        assert_eq!(device.options()[0].selected_choice, "On");
        assert!(device.set_option("dsp", "Maybe").is_err());
        assert!(device.set_option("agc", "On").is_err());
    }

    /// The two measured stages, front to back, starting with no attenuation
    /// and the LNA off; the AGC sets the attenuator alone; the digital AGC
    /// holds the level within its measured span, lower on VHF.
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
        assert!(g.stages_are_digital());
        assert_eq!(
            g.chain_caption(),
            "ANT\u{25b8}AGC\u{25b8}ADC\u{25b8}ATT\u{25b8}LNA\u{25b8}word"
        );
        let c = g.ceiling().expect("the digital AGC's hold");
        assert_eq!(
            (c.part, c.kind),
            ("digital AGC", crate::hardware::CeilingKind::Holds)
        );
        assert_eq!(g.ceiling_dbfs_at(9_000), -13.5);
        assert_eq!(g.ceiling_dbfs_at(31_000_000), -13.5);
        assert_eq!(g.ceiling_dbfs_at(60_000_000), -14.8);
        assert_eq!(g.ceiling_dbfs_at(260_000_000), -14.8);
        assert_eq!(c.text_at(14_200_000), "-14 to -12");
        assert_eq!(c.text_at(100_000_000), "-15 to -13");
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
        let why = AirspyHfDevice::open_with_api(api, &listing(), &Default::default())
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
        let why = AirspyHfDevice::open_with_api(api, &listing(), &Default::default())
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

    const WAIT: std::time::Duration = std::time::Duration::from_secs(5);

    fn streaming_ctx() -> (
        Arc<RxContext>,
        crossbeam_channel::Receiver<crate::hardware::StreamBlock>,
    ) {
        let (ctx, _fft, _demod, net) = crate::hardware::process::tests::rx_ctx_of(
            caps(&[912_000, 768_000, 192_000], 2048).sample_geometry,
        );
        (ctx, net)
    }

    fn words(block: &crate::hardware::StreamBlock) -> Vec<i32> {
        block
            .bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&b| i32::from_le_bytes(b))
            .collect()
    }

    /// **The hidden gain is read back from the samples**: each word times the
    /// library's float gain at `g` gives `g` again, at every gain the
    /// Discovery's rates use; a block of zeros, of tens, or with the DSP's
    /// fractions in it tells nothing.
    #[test]
    fn the_filter_gain_is_read_from_the_words() {
        for g in [0u8, 3, 7, 9, 13, 16] {
            let block: Vec<f32> = (-700i32..700).map(|w| w as f32 * word_gain(g)).collect();
            assert_eq!(filter_gain_db(&block), Some(g), "g = {g}");
        }
        assert_eq!(filter_gain_db(&[0.0; 64]), None, "zeros tell nothing");
        let tens: Vec<f32> = (1..50).map(|w| (w * 10) as f32 * word_gain(0)).collect();
        assert_eq!(filter_gain_db(&tens), None, "nor do tens: 0 or 20 dB?");
        // A third of a word: no power of ten makes it whole, as no DSP
        // output is whole words at any gain.
        let turned: Vec<f32> = (1..50)
            .map(|w| (w as f32 + 1.0 / 3.0) * word_gain(0))
            .collect();
        assert_eq!(
            filter_gain_db(&turned),
            None,
            "the DSP's fractions are no words"
        );
    }

    /// The transfer struct has the layout libairspyhf hands over.
    #[test]
    fn the_transfer_matches_the_c_layout() {
        let fixture = TestLibrary::new(false);
        assert_eq!(
            fixture.airspyhf_transfer_layout(0),
            std::mem::size_of::<ffi::Transfer>()
        );
        assert_eq!(
            fixture.airspyhf_transfer_layout(1),
            std::mem::offset_of!(ffi::Transfer, samples)
        );
        assert_eq!(
            fixture.airspyhf_transfer_layout(2),
            std::mem::offset_of!(ffi::Transfer, sample_count)
        );
        assert_eq!(
            fixture.airspyhf_transfer_layout(3),
            std::mem::offset_of!(ffi::Transfer, dropped_samples)
        );
    }

    /// **A stream starts by measuring, then delivers the words.** It starts
    /// with the library's DSP off; the first block tells the filter gain (16
    /// dB at 192 kS/s), is not delivered, and the DSP goes back on with the
    /// frequency set again; the rest arrive as the radio's words with eight
    /// fraction bits (the fixture's quarter word is 64 counts), the buffer
    /// the library lost counted and placed. The log says what was measured.
    /// Stopping joins the threads and lets the context go.
    #[test]
    fn a_stream_measures_its_scale_then_delivers_words() {
        let (fixture, device) = opened();
        device.set_sample_rate(192_000.0).unwrap();
        device.set_frequency(7_100_000).unwrap();
        let tuned = fixture.airspyhf_state(11);
        let (ctx, net) = streaming_ctx();
        device.start_rx(Arc::clone(&ctx)).unwrap();
        assert_eq!(fixture.airspyhf_state(12), 0, "started with the DSP off");
        let blocks: Vec<_> = (0..3).map(|_| net.recv_timeout(WAIT).unwrap()).collect();
        assert_eq!(fixture.airspyhf_state(3), 1, "the DSP back on");
        assert_eq!(
            fixture.airspyhf_state(11),
            tuned + 1,
            "the frequency set again"
        );
        assert_eq!(
            blocks.iter().map(|b| b.first_pair).collect::<Vec<_>>(),
            [0, 8, 12],
            "the measuring block not delivered; four pairs lost before the third"
        );
        assert_eq!(
            words(&blocks[0]),
            [
                101 * 256 + 64,
                -(101 * 256 + 64),
                102 * 256 + 64,
                -(102 * 256 + 64),
                103 * 256 + 64,
                -(103 * 256 + 64),
                104 * 256 + 64,
                -(104 * 256 + 64)
            ]
        );
        assert_eq!(ctx.metrics.lock().unwrap().signal.driver_drops_session, 4);
        let log: Vec<String> = ctx
            .metrics
            .lock()
            .unwrap()
            .ui
            .log
            .iter()
            .map(|e| e.text.to_string())
            .collect();
        assert!(
            log.iter()
                .any(|l| l.contains("at 192 kS/s read from the samples: filter gain 16 dB")),
            "{log:?}"
        );
        device.stop_rx().unwrap();
        assert!(!device.is_streaming());
        assert_eq!(Arc::strong_count(&ctx), 1, "nothing can call back now");
    }

    /// **A rate change never lands on a running stream**: the stream is
    /// stopped, the rate set, and a new one started, measuring again.
    #[test]
    fn a_rate_change_while_streaming_restarts_the_stream() {
        let (fixture, device) = opened();
        device.set_sample_rate(768_000.0).unwrap();
        let (ctx, net) = streaming_ctx();
        device.start_rx(Arc::clone(&ctx)).unwrap();
        // The fixture's stream delivers three blocks and idles: take them all,
        // so what arrives next is the new stream's.
        for _ in 0..3 {
            net.recv_timeout(WAIT).unwrap();
        }
        device.set_sample_rate(192_000.0).unwrap();
        assert!(device.is_streaming());
        assert_eq!(fixture.airspyhf_state(8), 2, "started again");
        assert_eq!(fixture.airspyhf_state(9), 1, "stopped first");
        assert_eq!(fixture.airspyhf_state(10), 192_000, "at the new rate");
        assert_eq!(fixture.airspyhf_state(12), 0, "measuring afresh");
        let first = net.recv_timeout(WAIT).unwrap();
        assert_eq!(first.first_pair, 0, "a new stream's positions");
        device.stop_rx().unwrap();
    }

    /// The link carries the radio's int16 words, four bytes a pair, not the
    /// eight sdrtop's container takes: the timing bench's bus line counts the
    /// wire.
    #[test]
    fn the_wire_carries_four_bytes_a_pair() {
        let (_fixture, device) = opened();
        assert_eq!(device.wire_bytes_per_pair(), Some(4.0));
        assert_eq!(device.capabilities().sample_geometry.bytes_per_pair(), 8);
    }

    /// A stream that ended by itself keeps its context, and the handle is
    /// not closed under it; a new start works.
    #[test]
    fn a_stream_that_ended_by_itself_keeps_its_context() {
        let (fixture, device) = opened();
        let (ctx, net) = streaming_ctx();
        fixture.mode(35);
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
        device.stop_rx().unwrap();
    }
}
