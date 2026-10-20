// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

use libc::{c_char, c_int, c_uchar, c_void};
use std::sync::OnceLock;

use crate::hardware::runtime_lib::{self, Host};

use super::super::loader::{load, symbol};

/// `void (*)(unsigned char *buf, uint32_t len, void *ctx)` - the async read sink.
pub type RtlSdrReadAsyncCb = extern "C" fn(*mut c_uchar, u32, *mut c_void);

pub struct RtlSdrApi {
    // The static API must keep the library mapped through reader-thread cleanup
    _lib: libloading::Library,
    pub rtlsdr_get_device_count: unsafe extern "C" fn() -> u32,
    pub rtlsdr_get_device_name: unsafe extern "C" fn(u32) -> *const c_char,
    pub rtlsdr_get_device_usb_strings:
        unsafe extern "C" fn(u32, *mut c_char, *mut c_char, *mut c_char) -> c_int,
    pub rtlsdr_open: unsafe extern "C" fn(*mut *mut c_void, u32) -> c_int,
    pub rtlsdr_close: unsafe extern "C" fn(*mut c_void) -> c_int,
    pub rtlsdr_set_center_freq: unsafe extern "C" fn(*mut c_void, u32) -> c_int,
    pub rtlsdr_set_sample_rate: unsafe extern "C" fn(*mut c_void, u32) -> c_int,
    /// The rate the device is actually running at: 28.8 MHz over an integer
    /// divider, which is rarely the rate that was asked for. Returns 0 on
    /// failure.
    pub rtlsdr_get_sample_rate: unsafe extern "C" fn(*mut c_void) -> u32,
    pub rtlsdr_get_tuner_type: unsafe extern "C" fn(*mut c_void) -> c_int,
    /// With a null buffer, returns the number of gains; otherwise fills `gains`
    /// (in tenths of a dB) and returns the count.
    pub rtlsdr_get_tuner_gains: unsafe extern "C" fn(*mut c_void, *mut c_int) -> c_int,
    /// A mode of 1 selects manual gain. A mode of 0 selects tuner AGC
    pub rtlsdr_set_tuner_gain_mode: unsafe extern "C" fn(*mut c_void, c_int) -> c_int,
    /// The gain must be a table value in tenths of a dB
    pub rtlsdr_set_tuner_gain: unsafe extern "C" fn(*mut c_void, c_int) -> c_int,
    pub rtlsdr_reset_buffer: unsafe extern "C" fn(*mut c_void) -> c_int,
    pub rtlsdr_read_async:
        unsafe extern "C" fn(*mut c_void, RtlSdrReadAsyncCb, *mut c_void, u32, u32) -> c_int,
    pub rtlsdr_cancel_async: unsafe extern "C" fn(*mut c_void) -> c_int,
}

// Ubuntu uses .so.2 for the same C API that Debian packages as .so.0
const LINUX: &[&str] = &["librtlsdr.so.0", "librtlsdr.so.2", "librtlsdr.so"];
// Homebrew and MacPorts install librtlsdr.0.dylib and its development link.
const MACOS: &[&str] = &["librtlsdr.0.dylib", "librtlsdr.dylib"];

/// The names librtlsdr is tried by on `host`, in order.
fn names(host: Host) -> Vec<String> {
    runtime_lib::candidates(host, LINUX, MACOS)
}

/// What to install on `host` when none of them opened.
fn hint(host: Host) -> &'static str {
    match host {
        Host::Linux => {
            "Install librtlsdr0 on Debian, librtlsdr2 on Ubuntu or rtl-sdr on Arch/Fedora."
        }
        Host::MacOs => "Install it with Homebrew: brew install rtl-sdr.",
    }
}

pub fn api() -> anyhow::Result<&'static RtlSdrApi> {
    static API: OnceLock<Result<RtlSdrApi, String>> = OnceLock::new();
    API.get_or_init(|| {
        let names = names(Host::current());
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        load("librtlsdr", &names, resolve)
    })
    .as_ref()
    .map_err(|err| {
        anyhow::anyhow!(
            "{err}\nRTL-SDR needs the librtlsdr runtime. {}",
            hint(Host::current())
        )
    })
}

pub(super) fn resolve(lib: libloading::Library) -> Result<RtlSdrApi, String> {
    macro_rules! sym {
        ($name:ident) => {
            unsafe { symbol(&lib, concat!(stringify!($name), "\0").as_bytes())? }
        };
    }
    Ok(RtlSdrApi {
        rtlsdr_get_device_count: sym!(rtlsdr_get_device_count),
        rtlsdr_get_device_name: sym!(rtlsdr_get_device_name),
        rtlsdr_get_device_usb_strings: sym!(rtlsdr_get_device_usb_strings),
        rtlsdr_open: sym!(rtlsdr_open),
        rtlsdr_close: sym!(rtlsdr_close),
        rtlsdr_set_center_freq: sym!(rtlsdr_set_center_freq),
        rtlsdr_set_sample_rate: sym!(rtlsdr_set_sample_rate),
        rtlsdr_get_sample_rate: sym!(rtlsdr_get_sample_rate),
        rtlsdr_get_tuner_type: sym!(rtlsdr_get_tuner_type),
        rtlsdr_get_tuner_gains: sym!(rtlsdr_get_tuner_gains),
        rtlsdr_set_tuner_gain_mode: sym!(rtlsdr_set_tuner_gain_mode),
        rtlsdr_set_tuner_gain: sym!(rtlsdr_set_tuner_gain),
        rtlsdr_reset_buffer: sym!(rtlsdr_reset_buffer),
        rtlsdr_read_async: sym!(rtlsdr_read_async),
        rtlsdr_cancel_async: sym!(rtlsdr_cancel_async),
        _lib: lib,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On Linux the names are the ones Debian, Ubuntu and the rest ship,
    /// unchanged; on macOS the `.dylib`s Homebrew and MacPorts install,
    /// Homebrew's Apple Silicon prefix among the places tried.
    #[test]
    fn the_library_is_looked_for_by_the_hosts_names() {
        assert_eq!(
            names(Host::Linux),
            ["librtlsdr.so.0", "librtlsdr.so.2", "librtlsdr.so"].map(String::from)
        );
        let mac = names(Host::MacOs);
        assert_eq!(mac[0], "librtlsdr.0.dylib");
        assert!(
            mac.contains(&"/opt/homebrew/lib/librtlsdr.0.dylib".to_string()),
            "{mac:?}"
        );
        assert!(!mac.iter().any(|n| n.contains(".so")), "{mac:?}");
    }

    /// The install hint names the host's package manager.
    #[test]
    fn the_install_hint_names_the_hosts_package_manager() {
        assert!(hint(Host::MacOs).contains("brew install rtl-sdr"));
        assert!(hint(Host::Linux).contains("librtlsdr0 on Debian"));
    }
}
