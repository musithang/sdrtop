// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! HackRF One backend: implements [`SdrDevice`] over libhackrf. The raw
//! per-sample math lives in [`crate::hardware::process::process_block`]; this module owns
//! the FFI, the device lifecycle, and the HackRF-specific capability descriptor.

pub mod ffi;

use libc::{c_int, c_void};
use std::ffi::CStr;
use std::sync::{Arc, Mutex};

use crate::state::{DEFAULT_FREQUENCY, DEFAULT_SAMPLE_RATE};

use crate::hardware::process::process_block;
use crate::hardware::{
    Boost, DeliveryModel, DeviceCapabilities, DeviceInfo, DeviceKind, DeviceListing, GainModel,
    RxContext, SampleFormat, SampleGeometry, SdrDevice, StageSpec,
};
// Not re-exported from `hardware`: what a backend answers a rate change with is
// between the backend and the trait, and no call site outside names the type.
use crate::hardware::traits::RateSet;
use ffi::*;

pub struct HackRfDevice {
    api: &'static HackrfApi,
    ptr: *mut c_void,
    caps: DeviceCapabilities,
    info: DeviceInfo,
    /// Keeps the streaming `RxContext` alive for the session, so the raw pointer
    /// handed to libhackrf stays valid until the device is told to stop.
    rx_ctx: Mutex<Option<Arc<RxContext>>>,
}

// Safety: libhackrf is thread-safe for status polling and streaming control.
unsafe impl Send for HackRfDevice {}
unsafe impl Sync for HackRfDevice {}

// ── RX callback (libhackrf's thread) ───────────────────────────────────────

extern "C" fn rx_callback(transfer: *mut hackrf_transfer) -> c_int {
    // Catch any Rust panic before it crosses the C FFI boundary. With
    // panic=abort this won't unwind, but the guard keeps the intent explicit and
    // protects debug builds from UB through C frames.
    let result =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| rx_callback_safe(transfer)));
    result.unwrap_or(0)
}

fn rx_callback_safe(transfer: *mut hackrf_transfer) -> c_int {
    unsafe {
        // Capture the timestamp immediately so jitter measures the true
        // inter-callback interval, not callback-entry-plus-processing time.
        let now = std::time::Instant::now();

        if transfer.is_null() {
            return 0;
        }
        let t = &*transfer;
        let ctx_ptr = t.rx_ctx as *const RxContext;
        if ctx_ptr.is_null() {
            return 0;
        }
        let ctx = &*ctx_ptr;

        // Guard against malformed USB transfers - libhackrf uses i32 and can
        // return error codes (negative) or zero-length transfers on instability.
        if t.buffer.is_null() {
            return 0;
        }
        if t.valid_length < 0 {
            return 0;
        }
        if t.valid_length == 0 {
            if let Ok(mut m) = ctx.metrics.lock() {
                m.signal.usb_errors_session += 1;
            }
            return 0;
        }

        let buf = std::slice::from_raw_parts(t.buffer as *const u8, t.valid_length as usize);
        let dropped_pairs = if t.valid_length < t.buffer_length {
            ((t.buffer_length - t.valid_length) / 2) as u64
        } else {
            0
        };

        process_block(buf, ctx.geometry, dropped_pairs, ctx, now);
    }
    0
}

// ── SdrDevice impl ──────────────────────────────────────────────────────────

impl SdrDevice for HackRfDevice {
    fn capabilities(&self) -> &DeviceCapabilities {
        &self.caps
    }
    fn info(&self) -> DeviceInfo {
        self.info.clone()
    }

    fn start_rx(&self, ctx: Arc<RxContext>) -> anyhow::Result<()> {
        let user_param = Arc::as_ptr(&ctx) as *mut c_void;
        unsafe {
            if (self.api.hackrf_start_rx)(self.ptr, rx_callback, user_param) != 0 {
                anyhow::bail!("Failed to start RX streaming");
            }
        }
        *self.rx_ctx.lock().unwrap_or_else(|e| e.into_inner()) = Some(ctx);
        Ok(())
    }

    fn stop_rx(&self) -> anyhow::Result<()> {
        unsafe {
            if (self.api.hackrf_stop_rx)(self.ptr) != 0 {
                anyhow::bail!("Failed to stop RX streaming");
            }
        }
        // hackrf_stop_rx joins libhackrf's transfer thread before returning, so
        // no further callback can fire - safe to release the context here.
        *self.rx_ctx.lock().unwrap_or_else(|e| e.into_inner()) = None;
        Ok(())
    }

    fn is_streaming(&self) -> bool {
        unsafe { (self.api.hackrf_is_streaming)(self.ptr) == 1 }
    }

    fn set_frequency(&self, hz: u64) -> anyhow::Result<()> {
        unsafe {
            if (self.api.hackrf_set_freq)(self.ptr, hz) != 0 {
                anyhow::bail!("Failed to set frequency");
            }
        }
        Ok(())
    }

    /// Sets the sample rate and programs the nearest valid BB filter BW,
    /// returning the bandwidth applied.
    /// libhackrf has no `get_sample_rate`, so there is nothing to ask back and
    /// the requested rate stands. The baseband width is a different matter: the
    /// value below is the one that was programmed, not a guess about it.
    fn set_sample_rate(&self, hz: f64) -> anyhow::Result<RateSet> {
        let bw = compute_bb_filter_bw(hz);
        unsafe {
            if (self.api.hackrf_set_sample_rate)(self.ptr, hz) != 0 {
                anyhow::bail!("Failed to set sample rate");
            }
            if (self.api.hackrf_set_baseband_filter_bandwidth)(self.ptr, bw) != 0 {
                anyhow::bail!("Failed to set baseband filter bandwidth");
            }
        }
        Ok(RateSet::new(hz, None, bw))
    }

    fn set_lna_gain(&self, db: u32) -> anyhow::Result<()> {
        unsafe {
            if (self.api.hackrf_set_lna_gain)(self.ptr, db) != 0 {
                anyhow::bail!("Failed to set LNA gain");
            }
        }
        Ok(())
    }

    fn set_vga_gain(&self, db: u32) -> anyhow::Result<()> {
        unsafe {
            if (self.api.hackrf_set_vga_gain)(self.ptr, db) != 0 {
                anyhow::bail!("Failed to set VGA gain");
            }
        }
        Ok(())
    }

    fn set_amp_enable(&self, on: bool) -> anyhow::Result<()> {
        unsafe {
            if (self.api.hackrf_set_amp_enable)(self.ptr, on as u8) != 0 {
                anyhow::bail!("Failed to set AMP enable");
            }
        }
        Ok(())
    }
}

impl Drop for HackRfDevice {
    fn drop(&mut self) {
        unsafe {
            if (self.api.hackrf_is_streaming)(self.ptr) == 1 {
                let _ = (self.api.hackrf_stop_rx)(self.ptr);
            }
            (self.api.hackrf_close)(self.ptr);
            (self.api.hackrf_exit)();
        }
    }
}

// ── Open / enumerate ─────────────────────────────────────────────────────────

impl HackRfDevice {
    /// Opens the HackRF at `index` and reads its metadata once.
    /// The library must export every required symbol. Firmware metadata read
    /// failures leave the corresponding fields unavailable.
    pub fn open(index: usize) -> anyhow::Result<Self> {
        Self::open_with_api(api()?, index)
    }

    fn open_with_api(api: &'static HackrfApi, index: usize) -> anyhow::Result<Self> {
        unsafe {
            let init_res = (api.hackrf_init)();
            if init_res != 0 {
                let err = CStr::from_ptr((api.hackrf_error_name)(init_res)).to_string_lossy();
                anyhow::bail!("Failed to initialize libhackrf: {}", err);
            }

            let list_ptr = (api.hackrf_device_list)();
            if list_ptr.is_null() {
                (api.hackrf_exit)();
                anyhow::bail!("Failed to retrieve HackRF device list.");
            }

            let list = &*list_ptr;
            let count = list.devicecount as usize;
            if count == 0 {
                (api.hackrf_device_list_free)(list_ptr);
                (api.hackrf_exit)();
                anyhow::bail!("No HackRF device found. Please connect your device and try again.");
            }
            if index >= count {
                (api.hackrf_device_list_free)(list_ptr);
                (api.hackrf_exit)();
                anyhow::bail!(
                    "Device index {} out of range ({} device(s) found).",
                    index,
                    count
                );
            }

            let mut ptr = std::ptr::null_mut();
            let res = (api.hackrf_device_list_open)(list_ptr, index as c_int, &mut ptr);
            (api.hackrf_device_list_free)(list_ptr);
            if res != 0 || ptr.is_null() {
                let err = CStr::from_ptr((api.hackrf_error_name)(res)).to_string_lossy();
                (api.hackrf_exit)();
                anyhow::bail!("Failed to open HackRF device: {} (code {})", err, res);
            }

            let board_id = read_board_id(api, ptr).unwrap_or(0);
            let info = DeviceInfo {
                board_name: read_board_name(api, board_id),
                serial: read_serial(api, ptr).unwrap_or_else(|| "unknown".into()),
                fw_version: read_version(api, ptr),
                board_rev: read_board_rev(api, ptr),
                usb_api_version: read_usb_api(api, ptr),
                tuner_name: None,
                // A HackRF reports its own firmware, so the header shows that.
                stack: None,
            };

            Ok(Self {
                api,
                ptr,
                caps: caps(),
                info,
                rx_ctx: Mutex::new(None),
            })
        }
    }
}

/// Enumerates connected HackRF devices with a readable serial. Swallows
/// enumeration errors (returns an empty list) - the caller unions backends and
/// reports "no device" only when every backend is empty.
pub fn list() -> Vec<DeviceListing> {
    let Ok(api) = api() else {
        return Vec::new();
    };
    list_with_api(api)
}

fn list_with_api(api: &HackrfApi) -> Vec<DeviceListing> {
    let mut out = Vec::new();
    unsafe {
        if (api.hackrf_init)() != 0 {
            return out;
        }
        let list_ptr = (api.hackrf_device_list)();
        if list_ptr.is_null() {
            (api.hackrf_exit)();
            return out;
        }
        let list = &*list_ptr;
        let count = list.devicecount as usize;
        if !list.serial_numbers.is_null() {
            for i in 0..count {
                let serial_ptr = *list.serial_numbers.add(i);
                if serial_ptr.is_null() {
                    continue;
                }
                let serial = CStr::from_ptr(serial_ptr).to_string_lossy().into_owned();
                if serial.is_empty() {
                    continue;
                }
                out.push(DeviceListing {
                    kind: DeviceKind::HackRf,
                    index: i,
                    label: format!("HackRF One · {}", serial),
                    args: None,
                    serial: Some(serial.clone()),
                    path: None,
                    tiny_sa_input: None,
                });
            }
        }
        (api.hackrf_device_list_free)(list_ptr);
        (api.hackrf_exit)();
    }
    out
}

/// The HackRF One's gain chain, from the datasheet.
///
/// Baseband LNA in 8 dB steps, VGA in 2 dB steps, and an RF amp that is a
/// two-position switch rather than a stage: 0 or +14 dB, on its own key. The
/// mixer sits between the two stages and has no gain to set, which is why the
/// diagram names it and the stage list does not.
///
/// This is the one place these numbers live. Everything device-generic asks the
/// model rather than knowing what a HackRF is.
pub fn gain_model() -> GainModel {
    GainModel::new(
        vec![
            StageSpec::ranged("LNA", 0.0, 40.0, 8.0),
            StageSpec::ranged("VGA", 0.0, 62.0, 2.0),
        ],
        "LNA",
        "LNA",
    )
    .with_second_stage()
    .with_boost(Boost::Element(StageSpec::ranged("AMP", 0.0, 14.0, 14.0)))
    .with_chain_diagram("LNA\u{25b8}MIX\u{25b8}VGA")
    // Never shown: this device has a modelled cascade, so `friis_applicable`
    // is true and the panels never ask why it is not.
    .with_no_cascade_reason("no cascade")
}

/// HackRF One capability descriptor - also used as the observer-mode default.
pub fn caps() -> DeviceCapabilities {
    DeviceCapabilities {
        level_unit: crate::hardware::LevelUnit::Dbfs,
        level_min_db: -120.0,
        level_max_db: 0.0,
        trace_stale_ms: crate::hardware::IQ_TRACE_STALE_MS,
        acquisition: crate::hardware::AcquisitionKind::IqSamples,
        sample_rate_is_span: false,
        freq_min_hz: 1_000_000,
        freq_max_hz: 6_000_000_000,
        sample_rate_min_hz: 2_000_000.0,
        sample_rate_max_hz: 20_000_000.0,
        default_frequency_hz: DEFAULT_FREQUENCY,
        default_sample_rate_hz: DEFAULT_SAMPLE_RATE,
        sample_geometry: SampleGeometry {
            format: SampleFormat::Int8,
            full_scale: 128.0,
        },
        gain: gain_model(),
        samples_per_transfer: crate::state::HACKRF_SAMPLES_PER_TRANSFER,
        has_bb_filter: true,
        friis_applicable: true,
        // libhackrf owns the thread and calls `rx_callback`. The interval
        // between callbacks is the link's own cadence.
        delivery: DeliveryModel::Push,
    }
}

// ── Metadata readers (open-time only) ─────────────────────────────────────────

unsafe fn read_board_id(api: &HackrfApi, ptr: *mut c_void) -> Option<u8> {
    let mut id = 0u8;
    ((api.hackrf_board_id_read)(ptr, &mut id) == 0).then_some(id)
}

unsafe fn read_board_name(api: &HackrfApi, id: u8) -> String {
    let p = (api.hackrf_board_id_name)(c_int::from(id));
    if p.is_null() {
        "Unknown".to_string()
    } else {
        CStr::from_ptr(p).to_string_lossy().into_owned()
    }
}

unsafe fn read_version(api: &HackrfApi, ptr: *mut c_void) -> Option<String> {
    // u8 buffer + .cast() so the pointer converts to *mut c_char on both glibc
    // (c_char = i8) and Android Bionic (c_char = u8).
    let mut buf = [0u8; 64];
    ((api.hackrf_version_string_read)(ptr, buf.as_mut_ptr().cast(), 63) == 0).then(|| {
        CStr::from_ptr(buf.as_ptr().cast())
            .to_string_lossy()
            .into_owned()
    })
}

unsafe fn read_serial(api: &HackrfApi, ptr: *mut c_void) -> Option<String> {
    let mut data = ReadPartidSerialno {
        part_id: [0; 2],
        serial_no: [0; 4],
    };
    ((api.hackrf_board_partid_serialno_read)(ptr, &mut data) == 0).then(|| {
        let s = data.serial_no;
        format!("{:08x}{:08x}{:08x}{:08x}", s[0], s[1], s[2], s[3])
    })
}

unsafe fn read_board_rev(api: &HackrfApi, ptr: *mut c_void) -> Option<u8> {
    let mut rev = 0u8;
    ((api.hackrf_board_rev_read)(ptr, &mut rev) == 0).then_some(rev)
}

unsafe fn read_usb_api(api: &HackrfApi, ptr: *mut c_void) -> Option<u16> {
    let mut ver = 0u16;
    ((api.hackrf_usb_api_version_read)(ptr, &mut ver) == 0).then_some(ver)
}

/// Maps a HackRF board-revision code to a human label.
pub fn board_rev_name(rev: u8) -> &'static str {
    match rev {
        0 => "HackRF One (old)",
        6 => "HackRF One r6",
        7 => "HackRF One r7",
        8 => "HackRF One r8",
        9 => "HackRF One r9",
        10 => "HackRF One r10",
        0xFE => "Undetected",
        0xFF => "Unrecognized",
        _ => "Unknown",
    }
}

/// Nearest valid HackRF baseband-filter bandwidth for a given sample rate.
pub fn compute_bb_filter_bw(sample_rate_hz: f64) -> u32 {
    const STEPS: &[u32] = &[
        1_750_000, 2_500_000, 3_500_000, 5_000_000, 5_500_000, 6_000_000, 7_000_000, 8_000_000,
        9_000_000, 10_000_000, 12_000_000, 14_000_000, 15_000_000, 20_000_000, 24_000_000,
        28_000_000,
    ];
    let target = sample_rate_hz as u32;
    STEPS
        .iter()
        .copied()
        .min_by_key(|&bw| (bw as i64 - target as i64).unsigned_abs())
        .unwrap_or(10_000_000)
}

#[cfg(test)]
mod tests {
    use super::super::test_support::TestLibrary;
    use super::*;

    #[test]
    fn loader_rejects_missing_required_symbol() {
        let fixture = TestLibrary::new(true);
        let err = match super::super::loader::load("libhackrf", &[fixture.path()], ffi::resolve) {
            Ok(_) => panic!("accepted an incomplete libhackrf"),
            Err(err) => err,
        };
        assert!(err.contains(fixture.path()));
        assert!(err.contains("missing required symbol hackrf_usb_api_version_read"));
    }

    #[test]
    fn loaded_api_owns_its_library() {
        let fixture = TestLibrary::new(false);
        let incompatible = TestLibrary::new(true);
        let api = super::super::loader::load(
            "libhackrf",
            &[
                "/sdrtop-nonexistent-test-directory/libmissing.so",
                incompatible.path(),
                fixture.path(),
            ],
            ffi::resolve,
        )
        .unwrap();
        drop(fixture);
        assert_eq!(unsafe { (api.hackrf_init)() }, 0);
        assert_eq!(unsafe { (api.hackrf_exit)() }, 0);
    }

    #[test]
    fn discovery_uses_hackrf_count_and_balances_initialization() {
        let fixture = TestLibrary::new(false);
        assert_eq!(
            std::mem::size_of::<HackrfDeviceList>(),
            fixture.list_layout(0)
        );
        assert_eq!(
            std::mem::offset_of!(HackrfDeviceList, usb_device_index),
            fixture.list_layout(1)
        );
        assert_eq!(
            std::mem::offset_of!(HackrfDeviceList, devicecount),
            fixture.list_layout(2)
        );
        assert_eq!(
            std::mem::offset_of!(HackrfDeviceList, usb_devices),
            fixture.list_layout(3)
        );
        assert_eq!(
            std::mem::offset_of!(HackrfDeviceList, usb_devicecount),
            fixture.list_layout(4)
        );
        let api = ffi::resolve(fixture.library()).unwrap();
        let devices = list_with_api(&api);
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].index, 0);
        assert_eq!(
            devices[0].serial.as_deref(),
            Some("0000000000000000123456789abcdef0")
        );
        assert_eq!(fixture.calls(0), 1);
        assert_eq!(fixture.calls(1), 1);
        assert_eq!(fixture.calls(2), 1);
    }

    #[test]
    fn open_errors_release_lists_and_initialized_library() {
        let fixture = TestLibrary::new(false);
        let api = Box::leak(Box::new(ffi::resolve(fixture.library()).unwrap()));
        for (mode, message, exits, freed) in [
            (1, "Failed to initialize", 0, 0),
            (2, "Failed to retrieve", 1, 0),
            (3, "No HackRF device found", 2, 1),
            (4, "Failed to open HackRF", 3, 2),
            (5, "Failed to open HackRF", 4, 3),
        ] {
            fixture.mode(mode);
            let err = match HackRfDevice::open_with_api(api, 0) {
                Ok(_) => panic!("accepted failing mode {mode}"),
                Err(err) => err,
            };
            assert!(err.to_string().contains(message), "{err}");
            assert_eq!(fixture.calls(1), exits);
            assert_eq!(fixture.calls(2), freed);
        }
        fixture.mode(0);
        assert!(HackRfDevice::open_with_api(api, 1).is_err());
        assert_eq!(fixture.calls(1), 5);
        assert_eq!(fixture.calls(2), 4);
        assert_eq!(fixture.calls(3), 0);
    }

    #[test]
    fn loaded_device_preserves_optional_metadata_and_control_errors() {
        let fixture = TestLibrary::new(false);
        let api = Box::leak(Box::new(ffi::resolve(fixture.library()).unwrap()));
        let device = HackRfDevice::open_with_api(api, 0).unwrap();
        assert_eq!(device.info().board_name, "Fixture HackRF");
        assert_eq!(device.info().fw_version, None);
        assert_eq!(device.info().board_rev, None);
        assert_eq!(device.info().usb_api_version, None);
        let rate = device.set_sample_rate(10_000_000.0).unwrap();
        assert_eq!(rate.rate_hz, 10_000_000.0);
        assert_eq!(rate.bb_filter_hz, 10_000_000);
        fixture.mode(6);
        assert!(device.set_sample_rate(10_000_000.0).is_err());
        assert!(device.set_frequency(100_000_000).is_err());
        assert!(device.set_lna_gain(8).is_err());
        assert!(device.set_vga_gain(2).is_err());
        assert!(device.set_amp_enable(true).is_err());
        drop(device);
        assert_eq!(fixture.calls(3), 1);
        assert_eq!(fixture.calls(1), 1);
    }

    #[test]
    fn drop_detection_arithmetic() {
        let buffer_length: i32 = 262144;
        let valid_length: i32 = 262144 - 128;
        let dropped_pairs = ((buffer_length - valid_length) / 2) as u64;
        assert_eq!(dropped_pairs, 64);
    }

    #[test]
    fn board_rev_name_known_revisions() {
        assert_eq!(board_rev_name(9), "HackRF One r9");
        assert_eq!(board_rev_name(0xFF), "Unrecognized");
        assert_eq!(board_rev_name(0xFE), "Undetected");
        assert_eq!(board_rev_name(0), "HackRF One (old)");
    }

    #[test]
    fn bb_filter_bw_exact_match() {
        assert_eq!(compute_bb_filter_bw(10_000_000.0), 10_000_000);
        assert_eq!(compute_bb_filter_bw(20_000_000.0), 20_000_000);
        assert_eq!(compute_bb_filter_bw(28_000_000.0), 28_000_000);
    }

    #[test]
    fn bb_filter_bw_rounds_to_nearest() {
        assert_eq!(compute_bb_filter_bw(11_500_000.0), 12_000_000);
        assert_eq!(compute_bb_filter_bw(4_000_000.0), 3_500_000);
    }

    #[test]
    fn bb_filter_bw_clamps_to_valid_range() {
        assert_eq!(compute_bb_filter_bw(500_000.0), 1_750_000);
        assert_eq!(compute_bb_filter_bw(30_000_000.0), 28_000_000);
    }

    #[test]
    fn hackrf_caps_match_legacy_constants() {
        let c = caps();
        assert_eq!(c.freq_min_hz, 1_000_000);
        assert_eq!(c.freq_max_hz, 6_000_000_000);
        assert_eq!(c.sample_rate_min_hz, 2_000_000.0);
        assert_eq!(c.sample_rate_max_hz, 20_000_000.0);
        assert_eq!(c.samples_per_transfer, 131_072);
        assert!(c.has_bb_filter && c.friis_applicable);
        assert_eq!(
            c.delivery,
            DeliveryModel::Push,
            "libhackrf drives rx_callback, so the callback interval is the link's"
        );
        assert_eq!(c.sample_geometry.format, SampleFormat::Int8);
        assert_eq!(c.sample_geometry.full_scale, 128.0);
    }
}
