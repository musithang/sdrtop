//! RTL-SDR backend: implements [`SdrDevice`] over librtlsdr. The per-sample math
//! is shared with HackRF via [`super::process::process_block`]; what differs is
//! the unsigned-8-bit sample format, the single discrete tuner-gain model, and
//! the blocking `rtlsdr_read_async` loop (which we drive on an owned thread and
//! stop with `rtlsdr_cancel_async`).

pub mod ffi;

use std::ffi::CStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use libc::{c_int, c_void};

use super::process::process_block;
use super::traits::{
    DeviceCapabilities, DeviceInfo, GainModel, RxContext, SampleFormat, SampleGeometry, SdrDevice,
};
use super::{DeviceKind, DeviceListing};
use ffi::*;

/// Bytes per async transfer (must be a multiple of 512). 64 KiB ≈ 32 768 IQ
/// pairs - ~73 callbacks/s at 2.4 Msps, matching HackRF's cadence and keeping
/// lock contention and latency moderate.
const RTL_BUF_LEN: u32 = 65_536;
/// Number of USB transfer buffers (0 = librtlsdr default of 15).
const RTL_BUF_NUM: u32 = 0;

pub struct RtlDevice {
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
static STDERR_LOCK: Mutex<()> = Mutex::new(());

/// Run `f` with the process's stderr redirected to /dev/null, then restore it.
///
/// librtlsdr chatters to stderr on open, on tuning, and on gain changes - "Found
/// Rafael Micro R820T tuner", "Detached kernel driver", "[R82XX] PLL not
/// locked!" - which would scramble the TUI's alternate screen. Every librtlsdr
/// control call is wrapped in this; the long-lived async read is not (it runs on
/// its own thread). Best-effort: if the redirect can't be set up, `f` runs
/// unsilenced.
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
            rtlsdr_reset_buffer(self.ptr);
        });

        // `rtlsdr_read_async` blocks until cancelled, so it gets its own thread.
        // The thread owns `cb_ctx`, keeping the pointer handed to the callback
        // valid for the entire call; `stop_rx` joins before that Arc drops.
        let ptr_usize = self.ptr as usize;
        let flag = Arc::clone(&self.streaming);
        let cb_ctx = ctx;
        let handle = std::thread::spawn(move || {
            let user = Arc::as_ptr(&cb_ctx) as *mut c_void;
            unsafe {
                rtlsdr_read_async(
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
            rtlsdr_cancel_async(self.ptr);
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
        let res = with_stderr_silenced(|| unsafe { rtlsdr_set_center_freq(self.ptr, hz as u32) });
        if res != 0 {
            anyhow::bail!("Failed to set RTL-SDR frequency");
        }
        Ok(())
    }

    /// RTL-SDR has no programmable baseband filter, so this only sets the rate
    /// and returns 0 (no filter bandwidth).
    fn set_sample_rate(&self, hz: f64) -> anyhow::Result<u32> {
        let res = with_stderr_silenced(|| unsafe { rtlsdr_set_sample_rate(self.ptr, hz as u32) });
        if res != 0 {
            anyhow::bail!("Failed to set RTL-SDR sample rate");
        }
        Ok(0)
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
            rtlsdr_set_tuner_gain_mode(self.ptr, 1);
            rtlsdr_set_tuner_gain(self.ptr, nearest)
        });
        if res != 0 {
            anyhow::bail!("Failed to set RTL-SDR tuner gain");
        }
        Ok(())
    }

    /// Tuner AGC: on → automatic gain (mode 0), off → manual (mode 1).
    fn set_tuner_agc(&self, on: bool) -> anyhow::Result<()> {
        let res = with_stderr_silenced(|| unsafe {
            rtlsdr_set_tuner_gain_mode(self.ptr, if on { 0 } else { 1 })
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
                rtlsdr_cancel_async(self.ptr);
            }
            if let Some(h) = self.thread.lock().unwrap_or_else(|e| e.into_inner()).take() {
                let _ = h.join();
            }
        }
        unsafe {
            rtlsdr_close(self.ptr);
        }
    }
}

// ── Open / enumerate ──────────────────────────────────────────────────────────

impl RtlDevice {
    pub fn open(index: usize) -> anyhow::Result<Self> {
        let mut ptr: *mut c_void = std::ptr::null_mut();
        // Open + tuner probe print kernel-driver / tuner lines to stderr; silence
        // them so they don't corrupt the TUI we've already switched into.
        let (res, tuner, gains_tenths) = with_stderr_silenced(|| {
            let res = unsafe { rtlsdr_open(&mut ptr, index as u32) };
            if res != 0 || ptr.is_null() {
                return (res, 0, Vec::new());
            }
            let tuner = unsafe { rtlsdr_get_tuner_type(ptr) };
            let gains = unsafe { read_tuner_gains(ptr) };
            // Default to manual gain so the gain controls take effect immediately.
            unsafe {
                rtlsdr_set_tuner_gain_mode(ptr, 1);
            }
            (res, tuner, gains)
        });
        if res != 0 || ptr.is_null() {
            anyhow::bail!("Failed to open RTL-SDR device {} (code {})", index, res);
        }

        let info = DeviceInfo {
            board_name: device_name(index as u32),
            serial: device_serial(index as u32).unwrap_or_else(|| format!("rtlsdr-{index}")),
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
    let mut out = Vec::new();
    let count = unsafe { rtlsdr_get_device_count() };
    for i in 0..count {
        let name = device_name(i);
        let serial = device_serial(i).unwrap_or_default();
        let shown = if serial.is_empty() {
            format!("#{i}")
        } else {
            serial
        };
        out.push(DeviceListing {
            kind: DeviceKind::RtlSdr,
            index: i as usize,
            label: format!("RTL-SDR · {} · {}", name, shown),
            args: None,
            serial: device_serial(i),
        });
    }
    out
}

/// Capability profile for observer mode, where no device handle is available to
/// query the tuner. Assumes the common R820T span with an empty gain table.
pub fn observer_caps() -> DeviceCapabilities {
    rtl_caps(5, &[])
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
        gain: GainModel::RtlSingle {
            gain_steps_db: steps,
        },
        samples_per_transfer: (RTL_BUF_LEN / 2) as u64,
        has_bb_filter: false,
        friis_applicable: false,
    }
}

unsafe fn read_tuner_gains(ptr: *mut c_void) -> Vec<i32> {
    let n = rtlsdr_get_tuner_gains(ptr, std::ptr::null_mut());
    if n <= 0 {
        return Vec::new();
    }
    let mut buf = vec![0i32; n as usize];
    let got = rtlsdr_get_tuner_gains(ptr, buf.as_mut_ptr());
    if got <= 0 {
        return Vec::new();
    }
    buf.truncate(got as usize);
    buf
}

fn device_name(index: u32) -> String {
    unsafe {
        let p = rtlsdr_get_device_name(index);
        if p.is_null() {
            "RTL-SDR".to_string()
        } else {
            CStr::from_ptr(p).to_string_lossy().into_owned()
        }
    }
}

fn device_serial(index: u32) -> Option<String> {
    // `libc::c_char` is `i8` on x86_64 but `u8` on ARM Linux; tying the buffer
    // element type to it keeps `.as_ptr()` matching the FFI's `*mut c_char`
    // (and `CStr::from_ptr`'s `*const c_char`) on every platform.
    let mut manufact = [0 as libc::c_char; 256];
    let mut product = [0 as libc::c_char; 256];
    let mut serial = [0 as libc::c_char; 256];
    unsafe {
        if rtlsdr_get_device_usb_strings(
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
    use super::*;

    #[test]
    fn caps_round_and_dedupe_gain_steps() {
        // R820T-style raw tenths → rounded whole dB, duplicates collapsed.
        let raw = [0, 9, 14, 27, 37, 496];
        let caps = rtl_caps(5, &raw);
        match caps.gain {
            GainModel::RtlSingle { gain_steps_db, .. } => {
                // 0.0→0, 0.9→1, 1.4→1 (dup of 1), 2.7→3, 3.7→4, 49.6→50
                assert_eq!(gain_steps_db, vec![0, 1, 3, 4, 50]);
            }
            _ => panic!("expected RtlSingle gain model"),
        }
        assert_eq!(caps.sample_geometry.format, SampleFormat::Uint8);
        assert_eq!(caps.sample_geometry.full_scale, 128.0);
        assert!(!caps.has_bb_filter && !caps.friis_applicable);
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
