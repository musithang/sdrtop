// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! RTL-SDR backend: implements [`SdrDevice`] over librtlsdr. The per-sample math
//! is shared with HackRF via [`crate::hardware::process::process_block`]; what differs is
//! the unsigned-8-bit sample format, the single discrete tuner-gain model, and
//! the blocking `rtlsdr_read_async` loop (which we drive on an owned thread and
//! stop with `rtlsdr_cancel_async`).

pub mod ffi;

use std::ffi::CStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use libc::{c_int, c_void};

use crate::hardware::process::process_block;
use crate::hardware::{
    Boost, DeliveryModel, DeviceCapabilities, DeviceInfo, DeviceKind, DeviceListing, GainModel,
    RxContext, SampleFormat, SampleGeometry, SdrDevice, StageSpec,
};
// Not re-exported from `hardware`: what a backend answers a rate change with is
// between the backend and the trait, and no call site outside names the type.
use crate::hardware::traits::RateSet;
use ffi::*;

/// Bytes per async transfer (must be a multiple of 512). 64 KiB ≈ 32 768 IQ
/// pairs - ~73 callbacks/s at 2.4 Msps, matching HackRF's cadence and keeping
/// lock contention and latency moderate.
const RTL_BUF_LEN: u32 = 65_536;
/// Number of USB transfer buffers (0 = librtlsdr default of 15).
const RTL_BUF_NUM: u32 = 0;

pub struct RtlDevice {
    api: &'static RtlSdrApi,
    ptr: *mut c_void,
    caps: DeviceCapabilities,
    info: DeviceInfo,
    /// Raw tuner gains in tenths of a dB, as reported by the device. The set
    /// path snaps a requested whole-dB value to the nearest entry here.
    gains_tenths: Vec<i32>,
    streaming: Arc<AtomicBool>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

// Safety: the device pointer is only touched from the main thread (control) and
// the owned read thread (async read), the same split libusb tolerates for HackRF.
unsafe impl Send for RtlDevice {}
unsafe impl Sync for RtlDevice {}

/// Serializes the fd-2 redirect dance so two control calls on different threads
/// (e.g. the input handler and the sweep task) can't clobber each other's saved
/// descriptor and leave stderr pointing at /dev/null permanently.
#[cfg(not(test))]
static STDERR_LOCK: Mutex<()> = Mutex::new(());

/// Run `f` with the process's stderr redirected to /dev/null, then restore it.
///
/// librtlsdr chatters to stderr on open, on tuning, and on gain changes - "Found
/// Rafael Micro R820T tuner", "Detached kernel driver", "[R82XX] PLL not
/// locked!" - which would scramble the TUI's alternate screen. Every librtlsdr
/// control call is wrapped in this; the long-lived async read is not (it runs on
/// its own thread). Best-effort: if the redirect can't be set up, `f` runs
/// unsilenced.
#[cfg(not(test))]
fn with_stderr_silenced<R>(f: impl FnOnce() -> R) -> R {
    let _guard = STDERR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    unsafe {
        let saved = libc::dup(libc::STDERR_FILENO);
        if saved < 0 {
            return f();
        }
        let devnull = libc::open(c"/dev/null".as_ptr(), libc::O_WRONLY);
        if devnull < 0 {
            libc::close(saved);
            return f();
        }
        libc::dup2(devnull, libc::STDERR_FILENO);
        libc::close(devnull);
        let result = f();
        libc::dup2(saved, libc::STDERR_FILENO);
        libc::close(saved);
        result
    }
}

#[cfg(test)]
fn with_stderr_silenced<R>(f: impl FnOnce() -> R) -> R {
    f()
}

// ── Async read callback (our read thread) ─────────────────────────────────────

extern "C" fn rtl_rx_callback(buf: *mut libc::c_uchar, len: u32, ctx: *mut c_void) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let now = std::time::Instant::now();
        if buf.is_null() || len == 0 || ctx.is_null() {
            return;
        }
        // Safety: `ctx` is the `Arc<RxContext>` the read thread keeps alive for
        // the whole `rtlsdr_read_async` call (see `start_rx`).
        let rx = unsafe { &*(ctx as *const RxContext) };
        let slice = unsafe { std::slice::from_raw_parts(buf as *const u8, len as usize) };
        process_block(slice, rx.geometry, 0, rx, now);
    }));
}

// ── SdrDevice impl ─────────────────────────────────────────────────────────────

impl SdrDevice for RtlDevice {
    fn capabilities(&self) -> &DeviceCapabilities {
        &self.caps
    }
    fn info(&self) -> DeviceInfo {
        self.info.clone()
    }

    fn start_rx(&self, ctx: Arc<RxContext>) -> anyhow::Result<()> {
        if self.streaming.swap(true, Ordering::SeqCst) {
            return Ok(()); // already streaming
        }
        with_stderr_silenced(|| unsafe {
            (self.api.rtlsdr_reset_buffer)(self.ptr);
        });

        // `rtlsdr_read_async` blocks until cancelled, so it gets its own thread.
        // The thread owns `cb_ctx`, keeping the pointer handed to the callback
        // valid for the entire call; `stop_rx` joins before that Arc drops.
        let ptr_usize = self.ptr as usize;
        let api = self.api;
        let flag = Arc::clone(&self.streaming);
        let cb_ctx = ctx;
        let handle = std::thread::spawn(move || {
            let user = Arc::as_ptr(&cb_ctx) as *mut c_void;
            unsafe {
                (api.rtlsdr_read_async)(
                    ptr_usize as *mut c_void,
                    rtl_rx_callback,
                    user,
                    RTL_BUF_NUM,
                    RTL_BUF_LEN,
                );
            }
            // read_async returned (cancelled, or a USB error): no longer streaming.
            flag.store(false, Ordering::SeqCst);
            drop(cb_ctx);
        });
        *self.thread.lock().unwrap_or_else(|e| e.into_inner()) = Some(handle);
        Ok(())
    }

    fn stop_rx(&self) -> anyhow::Result<()> {
        unsafe {
            (self.api.rtlsdr_cancel_async)(self.ptr);
        }
        // Join so the read thread (and the Arc<RxContext> it holds) is fully gone
        // before we return - no callback can fire afterward.
        if let Some(h) = self.thread.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = h.join();
        }
        self.streaming.store(false, Ordering::SeqCst);
        Ok(())
    }

    fn is_streaming(&self) -> bool {
        self.streaming.load(Ordering::SeqCst)
    }

    fn set_frequency(&self, hz: u64) -> anyhow::Result<()> {
        let res = with_stderr_silenced(|| unsafe {
            (self.api.rtlsdr_set_center_freq)(self.ptr, hz as u32)
        });
        if res != 0 {
            anyhow::bail!("Failed to set RTL-SDR frequency");
        }
        Ok(())
    }

    /// RTL-SDR has no programmable baseband filter, so this only sets the rate
    /// and returns 0 (no filter bandwidth).
    /// Sets the rate, then asks what it became.
    ///
    /// The R820T's clock is 28.8 MHz over an integer divider, so a request is a
    /// wish: 2.5 Msps is not on the grid and librtlsdr rounds it without
    /// comment. `rtlsdr_get_sample_rate` is the only place that rounding is
    /// visible, and every frequency the app prints is derived from it.
    ///
    /// No baseband filter on this radio, so the width is zero rather than a
    /// number that would have to mean something.
    fn set_sample_rate(&self, hz: f64) -> anyhow::Result<RateSet> {
        let res = with_stderr_silenced(|| unsafe {
            (self.api.rtlsdr_set_sample_rate)(self.ptr, hz as u32)
        });
        if res != 0 {
            anyhow::bail!("Failed to set RTL-SDR sample rate");
        }
        let got = with_stderr_silenced(|| unsafe { (self.api.rtlsdr_get_sample_rate)(self.ptr) });
        Ok(RateSet::new(hz, Some(got as f64), 0))
    }

    /// The single tuner gain. Forces manual gain mode, then snaps `db` to the
    /// nearest entry in the device's discrete table (stored in tenths of a dB).
    fn set_lna_gain(&self, db: u32) -> anyhow::Result<()> {
        let target = db as i32 * 10;
        let nearest = self
            .gains_tenths
            .iter()
            .copied()
            .min_by_key(|&t| (t - target).abs())
            .unwrap_or(target);
        // Force manual gain mode, then set the nearest table value (both chatter).
        let res = with_stderr_silenced(|| unsafe {
            (self.api.rtlsdr_set_tuner_gain_mode)(self.ptr, 1);
            (self.api.rtlsdr_set_tuner_gain)(self.ptr, nearest)
        });
        if res != 0 {
            anyhow::bail!("Failed to set RTL-SDR tuner gain");
        }
        Ok(())
    }

    /// Tuner AGC: on → automatic gain (mode 0), off → manual (mode 1).
    fn set_tuner_agc(&self, on: bool) -> anyhow::Result<()> {
        let res = with_stderr_silenced(|| unsafe {
            (self.api.rtlsdr_set_tuner_gain_mode)(self.ptr, if on { 0 } else { 1 })
        });
        if res != 0 {
            anyhow::bail!("Failed to set RTL-SDR tuner AGC");
        }
        Ok(())
    }
}

impl Drop for RtlDevice {
    fn drop(&mut self) {
        if self.streaming.load(Ordering::SeqCst) {
            unsafe {
                (self.api.rtlsdr_cancel_async)(self.ptr);
            }
        }
        if let Some(h) = self.thread.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = h.join();
        }
        unsafe {
            (self.api.rtlsdr_close)(self.ptr);
        }
    }
}

// ── Open / enumerate ──────────────────────────────────────────────────────────

impl RtlDevice {
    pub fn open(index: usize) -> anyhow::Result<Self> {
        Self::open_with_api(api()?, index)
    }

    fn open_with_api(api: &'static RtlSdrApi, index: usize) -> anyhow::Result<Self> {
        let mut ptr: *mut c_void = std::ptr::null_mut();
        // Open + tuner probe print kernel-driver / tuner lines to stderr; silence
        // them so they don't corrupt the TUI we've already switched into.
        let (res, tuner, gains_tenths) = with_stderr_silenced(|| {
            let res = unsafe { (api.rtlsdr_open)(&mut ptr, index as u32) };
            if res != 0 || ptr.is_null() {
                return (res, 0, Vec::new());
            }
            let tuner = unsafe { (api.rtlsdr_get_tuner_type)(ptr) };
            let gains = unsafe { read_tuner_gains(api, ptr) };
            // Default to manual gain so the gain controls take effect immediately.
            unsafe {
                (api.rtlsdr_set_tuner_gain_mode)(ptr, 1);
            }
            (res, tuner, gains)
        });
        if res != 0 || ptr.is_null() {
            anyhow::bail!("Failed to open RTL-SDR device {} (code {})", index, res);
        }

        let info = DeviceInfo {
            board_name: device_name(api, index as u32),
            serial: device_serial(api, index as u32).unwrap_or_else(|| format!("rtlsdr-{index}")),
            fw_version: None,
            board_rev: None,
            usb_api_version: None,
            tuner_name: tuner_name(tuner),
            // No on-device firmware: an RTL-SDR is driven entirely from the
            // host, so the header names the library instead of a version the
            // dongle does not have.
            stack: Some(crate::hardware::SoftwareStack {
                label: "rtl-sdr   ",
                value: std::sync::Arc::from("librtlsdr"),
            }),
        };

        Ok(Self {
            api,
            ptr,
            caps: rtl_caps(tuner, &gains_tenths),
            info,
            gains_tenths,
            streaming: Arc::new(AtomicBool::new(false)),
            thread: Mutex::new(None),
        })
    }
}

/// Enumerates connected RTL-SDR dongles. Never fails - returns an empty list
/// when librtlsdr finds none.
pub fn list() -> Vec<DeviceListing> {
    let Ok(api) = api() else {
        return Vec::new();
    };
    list_with_api(api)
}

fn list_with_api(api: &RtlSdrApi) -> Vec<DeviceListing> {
    let mut out = Vec::new();
    let count = unsafe { (api.rtlsdr_get_device_count)() };
    for i in 0..count {
        let name = device_name(api, i);
        let serial = device_serial(api, i);
        let shown = serial.clone().unwrap_or_else(|| format!("#{i}"));
        out.push(DeviceListing {
            kind: DeviceKind::RtlSdr,
            index: i as usize,
            label: format!("RTL-SDR · {} · {}", name, shown),
            args: None,
            serial,
            path: None,
            tiny_sa_input: None,
        });
    }
    out
}

/// Capability profile for observer mode, where no device handle is available to
/// query the tuner. Assumes the common R820T span with an empty gain table.
pub fn observer_caps() -> DeviceCapabilities {
    rtl_caps(5, &[])
}

/// An RTL-SDR's gain chain: one tuner, over the values the driver read out of
/// the device, plus the tuner AGC as its boost.
///
/// **A table rather than a grid**, because the real list is irregular and a
/// nearest-step answer would offer settings the tuner refuses.
///
/// A tuner that named no values has no describable stage, and saying so is not
/// the same as saying it sits at zero: `clamp_gains` then leaves a gain alone
/// rather than snapping it to a table that does not exist. The gauge still needs
/// a full scale in that case, and 49 dB is the answer it has always given.
pub fn gain_model(steps_db: &[u32]) -> GainModel {
    let stages = if steps_db.is_empty() {
        Vec::new()
    } else {
        vec![StageSpec::tabled(
            "Tuner",
            steps_db.iter().map(|&g| g as f64).collect(),
        )]
    };
    GainModel::new(stages, "Tuner", "TUN")
        // The tuner AGC: a flag the driver owns, not a switch with a name.
        .with_boost(Boost::GainMode)
        .with_chain_diagram("TUNER")
        .with_no_cascade_reason("single tuner, no cascade")
        .with_gauge_fallback(49)
}

fn rtl_caps(tuner: c_int, gains_tenths: &[i32]) -> DeviceCapabilities {
    // Frequency span depends on the tuner; the dominant R820T/R828D case covers
    // 24 MHz–1.766 GHz. E4000 reaches higher (with an internal gap we ignore).
    let (freq_min_hz, freq_max_hz) = match tuner {
        1 => (52_000_000, 2_200_000_000), // E4000
        _ => (24_000_000, 1_766_000_000), // R820T / R828D / others
    };

    // Round each gain step to whole dB for display, collapsing duplicates. The
    // exact tenths stay in `RtlDevice::gains_tenths` for the actual set.
    let mut steps: Vec<u32> = gains_tenths
        .iter()
        .map(|&t| ((t + 5) / 10).max(0) as u32)
        .collect();
    steps.dedup();
    if steps.is_empty() {
        steps = vec![0];
    }

    DeviceCapabilities {
        level_unit: crate::hardware::LevelUnit::Dbfs,
        level_min_db: -120.0,
        level_max_db: 0.0,
        trace_stale_ms: crate::hardware::IQ_TRACE_STALE_MS,
        acquisition: crate::hardware::AcquisitionKind::IqSamples,
        sample_rate_is_span: false,
        freq_min_hz,
        freq_max_hz,
        // RTL-SDR's usable upper band is 900_001..=3_200_000 Hz (the lower
        // 225_001..=300_000 band is excluded as it can't be a single range).
        // 900_001 is the true floor; the sample-rate input clamps into this
        // range, so entering "0.9" (900_000) snaps up to a valid 900_001.
        sample_rate_min_hz: 900_001.0,
        sample_rate_max_hz: 3_200_000.0,
        default_frequency_hz: 100_000_000,
        default_sample_rate_hz: 2_400_000.0,
        sample_geometry: SampleGeometry {
            format: SampleFormat::Uint8,
            // Centered by 128 rather than the true 127.5 bias, deliberately.
            // See `process::decode`.
            full_scale: 128.0,
        },
        gain: gain_model(&steps),
        samples_per_transfer: (RTL_BUF_LEN / 2) as u64,
        has_bb_filter: false,
        friis_applicable: false,
        // We own the thread, but `rtlsdr_read_async` drives `rtl_rx_callback`
        // from inside it. The pacing is still the driver's, not ours.
        delivery: DeliveryModel::Push,
    }
}

unsafe fn read_tuner_gains(api: &RtlSdrApi, ptr: *mut c_void) -> Vec<i32> {
    let n = (api.rtlsdr_get_tuner_gains)(ptr, std::ptr::null_mut());
    if n <= 0 {
        return Vec::new();
    }
    let mut buf = vec![0i32; n as usize];
    let got = (api.rtlsdr_get_tuner_gains)(ptr, buf.as_mut_ptr());
    if got <= 0 {
        return Vec::new();
    }
    buf.truncate(got as usize);
    buf
}

fn device_name(api: &RtlSdrApi, index: u32) -> String {
    unsafe {
        let p = (api.rtlsdr_get_device_name)(index);
        if p.is_null() {
            "RTL-SDR".to_string()
        } else {
            CStr::from_ptr(p).to_string_lossy().into_owned()
        }
    }
}

fn device_serial(api: &RtlSdrApi, index: u32) -> Option<String> {
    // `libc::c_char` is `i8` on x86_64 but `u8` on ARM Linux; tying the buffer
    // element type to it keeps `.as_ptr()` matching the FFI's `*mut c_char`
    // (and `CStr::from_ptr`'s `*const c_char`) on every platform.
    let mut manufact = [0 as libc::c_char; 256];
    let mut product = [0 as libc::c_char; 256];
    let mut serial = [0 as libc::c_char; 256];
    unsafe {
        if (api.rtlsdr_get_device_usb_strings)(
            index,
            manufact.as_mut_ptr(),
            product.as_mut_ptr(),
            serial.as_mut_ptr(),
        ) != 0
        {
            return None;
        }
        let s = CStr::from_ptr(serial.as_ptr())
            .to_string_lossy()
            .into_owned();
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    }
}

fn tuner_name(tuner: c_int) -> Option<String> {
    let name = match tuner {
        1 => "E4000",
        2 => "FC0012",
        3 => "FC0013",
        4 => "FC2580",
        5 => "R820T",
        6 => "R828D",
        _ => return None,
    };
    Some(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::super::test_support::TestLibrary;
    use super::*;

    #[test]
    fn loader_rejects_missing_required_symbol() {
        let fixture = TestLibrary::new(true);
        let err = match super::super::loader::load("librtlsdr", &[fixture.path()], ffi::resolve) {
            Ok(_) => panic!("accepted an incomplete librtlsdr"),
            Err(err) => err,
        };
        assert!(err.contains(fixture.path()));
        assert!(err.contains("missing required symbol rtlsdr_cancel_async"));
    }

    #[test]
    fn loaded_api_owns_its_library() {
        let fixture = TestLibrary::new(false);
        let api = ffi::resolve(fixture.library()).unwrap();
        drop(fixture);
        assert_eq!(unsafe { (api.rtlsdr_get_device_count)() }, 1);
    }

    #[test]
    fn loaded_device_preserves_discovery_metadata_and_control_errors() {
        let fixture = TestLibrary::new(false);
        let api = Box::leak(Box::new(ffi::resolve(fixture.library()).unwrap()));
        let devices = list_with_api(api);
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].index, 0);
        assert_eq!(devices[0].serial.as_deref(), Some("00000001"));
        assert_eq!(fixture.calls(0), 1, "enumeration reads USB strings once");
        let device = RtlDevice::open_with_api(api, 0).unwrap();
        assert_eq!(device.info().board_name, "Fixture RTL-SDR");
        assert_eq!(device.info().tuner_name.as_deref(), Some("R820T"));
        assert_eq!(device.gains_tenths, vec![0, 197, 496]);
        let rate = device.set_sample_rate(2_400_000.0).unwrap();
        assert_eq!(rate.rate_hz, 2_399_999.0);
        assert_eq!(rate.bb_filter_hz, 0);
        fixture.mode(6);
        assert!(device.set_sample_rate(2_400_000.0).is_err());
        assert!(device.set_frequency(100_000_000).is_err());
        assert!(device.set_lna_gain(20).is_err());
        assert!(device.set_tuner_agc(true).is_err());
        drop(device);
        assert_eq!(fixture.calls(3), 1);
    }

    #[test]
    fn open_rejects_error_and_null_handles() {
        let fixture = TestLibrary::new(false);
        let api = Box::leak(Box::new(ffi::resolve(fixture.library()).unwrap()));
        for mode in [4, 5] {
            fixture.mode(mode);
            let err = match RtlDevice::open_with_api(api, 0) {
                Ok(_) => panic!("accepted failing mode {mode}"),
                Err(err) => err,
            };
            assert!(err.to_string().contains("Failed to open RTL-SDR device"));
        }
        assert_eq!(fixture.calls(3), 0);
    }

    #[test]
    fn drop_joins_reader_even_after_streaming_flag_clears() {
        let fixture = TestLibrary::new(false);
        let api = Box::leak(Box::new(ffi::resolve(fixture.library()).unwrap()));
        let device = RtlDevice::open_with_api(api, 0).unwrap();
        let finished = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&finished);
        *device.thread.lock().unwrap() = Some(std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(20));
            flag.store(true, Ordering::SeqCst);
        }));
        drop(device);
        assert!(finished.load(Ordering::SeqCst));
        assert_eq!(fixture.calls(3), 1);
    }

    #[test]
    fn caps_round_and_dedupe_gain_steps() {
        // R820T-style raw tenths → rounded whole dB, duplicates collapsed.
        let raw = [0, 9, 14, 27, 37, 496];
        let caps = rtl_caps(5, &raw);
        // 0.0→0, 0.9→1, 1.4→1 (dup of 1), 2.7→3, 3.7→4, 49.6→50
        let stages = caps.gain.stages();
        assert_eq!(stages.len(), 1, "one tuner");
        assert_eq!(stages[0].table, vec![0.0, 1.0, 3.0, 4.0, 50.0]);
        assert_eq!(caps.sample_geometry.format, SampleFormat::Uint8);
        assert_eq!(caps.sample_geometry.full_scale, 128.0);
        assert!(!caps.has_bb_filter && !caps.friis_applicable);
        assert_eq!(
            caps.delivery,
            DeliveryModel::Push,
            "we own the thread, but rtlsdr_read_async paces the callback inside it"
        );
        assert_eq!(caps.freq_min_hz, 24_000_000);
        assert_eq!(caps.freq_max_hz, 1_766_000_000);
        assert_eq!(caps.samples_per_transfer, 32_768);
    }

    #[test]
    fn e4000_has_higher_freq_ceiling() {
        let caps = rtl_caps(1, &[0, 100, 200]);
        assert_eq!(caps.freq_min_hz, 52_000_000);
        assert_eq!(caps.freq_max_hz, 2_200_000_000);
    }

    #[test]
    fn nearest_tuner_gain_snaps_to_table() {
        // Mirrors set_lna_gain's snapping: 20 dB → 200 tenths → nearest of {197,207} = 197.
        let gains: [i32; 4] = [0, 197, 207, 496];
        let target: i32 = 20 * 10;
        let nearest = gains
            .iter()
            .copied()
            .min_by_key(|&t| (t - target).abs())
            .unwrap();
        assert_eq!(nearest, 197);
    }

    #[test]
    fn tuner_name_maps_known_types() {
        assert_eq!(tuner_name(5).as_deref(), Some("R820T"));
        assert_eq!(tuner_name(6).as_deref(), Some("R828D"));
        assert_eq!(tuner_name(0), None);
    }
}
