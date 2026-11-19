// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

use libc::{c_char, c_int, c_void};
use std::sync::OnceLock;

use crate::hardware::runtime_lib;
use crate::host::Host;

use super::super::loader::{load, optional_symbol, symbol};

#[repr(C)]
pub struct hackrf_transfer {
    pub device: *mut c_void,
    pub buffer: *mut u8,
    pub buffer_length: i32,
    pub valid_length: i32,
    pub rx_ctx: *mut c_void,
    pub tx_ctx: *mut c_void,
}

// Matches hackrf_device_list_t in hackrf.h exactly
#[repr(C)]
pub struct HackrfDeviceList {
    pub serial_numbers: *mut *mut c_char,
    pub usb_board_ids: *mut c_int,
    pub usb_device_index: *mut c_int,
    pub devicecount: c_int,
    pub usb_devices: *mut *mut c_void,
    pub usb_devicecount: c_int,
}

/// `hackrf_m0_state` in hackrf.h: the SGPIO loop on the radio's M0 core,
/// field for field.
#[repr(C)]
#[derive(Default)]
pub struct HackrfM0State {
    pub requested_mode: u16,
    pub request_flag: u16,
    pub active_mode: u32,
    pub m0_count: u32,
    pub m4_count: u32,
    pub num_shortfalls: u32,
    pub longest_shortfall: u32,
    pub shortfall_limit: u32,
    pub threshold: u32,
    pub next_mode: u32,
    pub error: u32,
}

#[repr(C)]
pub struct ReadPartidSerialno {
    pub part_id: [u32; 2],
    pub serial_no: [u32; 4],
}

pub type HackrfTransferCallback = extern "C" fn(*mut hackrf_transfer) -> c_int;

pub struct HackrfApi {
    // The static API must keep the library mapped through device and callback cleanup
    _lib: libloading::Library,
    pub hackrf_init: unsafe extern "C" fn() -> c_int,
    pub hackrf_exit: unsafe extern "C" fn() -> c_int,
    pub hackrf_close: unsafe extern "C" fn(*mut c_void) -> c_int,
    pub hackrf_device_list: unsafe extern "C" fn() -> *mut HackrfDeviceList,
    pub hackrf_device_list_free: unsafe extern "C" fn(*mut HackrfDeviceList),
    pub hackrf_device_list_open:
        unsafe extern "C" fn(*mut HackrfDeviceList, c_int, *mut *mut c_void) -> c_int,
    pub hackrf_version_string_read: unsafe extern "C" fn(*mut c_void, *mut c_char, u8) -> c_int,
    pub hackrf_is_streaming: unsafe extern "C" fn(*mut c_void) -> c_int,
    pub hackrf_set_sample_rate: unsafe extern "C" fn(*mut c_void, f64) -> c_int,
    pub hackrf_set_baseband_filter_bandwidth: unsafe extern "C" fn(*mut c_void, u32) -> c_int,
    pub hackrf_set_freq: unsafe extern "C" fn(*mut c_void, u64) -> c_int,
    pub hackrf_set_amp_enable: unsafe extern "C" fn(*mut c_void, u8) -> c_int,
    pub hackrf_start_rx:
        unsafe extern "C" fn(*mut c_void, HackrfTransferCallback, *mut c_void) -> c_int,
    pub hackrf_stop_rx: unsafe extern "C" fn(*mut c_void) -> c_int,
    pub hackrf_set_lna_gain: unsafe extern "C" fn(*mut c_void, u32) -> c_int,
    pub hackrf_set_vga_gain: unsafe extern "C" fn(*mut c_void, u32) -> c_int,
    pub hackrf_board_partid_serialno_read:
        unsafe extern "C" fn(*mut c_void, *mut ReadPartidSerialno) -> c_int,
    pub hackrf_board_id_read: unsafe extern "C" fn(*mut c_void, *mut u8) -> c_int,
    pub hackrf_board_id_name: unsafe extern "C" fn(c_int) -> *const c_char,
    pub hackrf_error_name: unsafe extern "C" fn(c_int) -> *const c_char,
    pub hackrf_board_rev_read: unsafe extern "C" fn(*mut c_void, *mut u8) -> c_int,
    pub hackrf_usb_api_version_read: unsafe extern "C" fn(*mut c_void, *mut u16) -> c_int,
    // Optional: only the radio's own drop count needs these, so a library
    // without them loses that count and nothing else.
    pub hackrf_get_m0_state: Option<unsafe extern "C" fn(*mut c_void, *mut HackrfM0State) -> c_int>,
    pub hackrf_get_transfer_buffer_size: Option<unsafe extern "C" fn(*mut c_void) -> usize>,
    pub hackrf_get_transfer_queue_depth: Option<unsafe extern "C" fn(*mut c_void) -> u32>,
}

const LINUX: &[&str] = &["libhackrf.so.0", "libhackrf.so"];
// Homebrew and MacPorts install libhackrf.0.dylib and its development link.
const MACOS: &[&str] = &["libhackrf.0.dylib", "libhackrf.dylib"];

/// The names libhackrf is tried by on `host`, in order.
fn names(host: Host) -> Vec<String> {
    runtime_lib::candidates(host, LINUX, MACOS)
}

/// What to install on `host` when none of them opened.
fn hint(host: Host) -> &'static str {
    match host {
        Host::Linux => "Install libhackrf0 on Debian/Ubuntu or hackrf on Arch/Fedora.",
        Host::MacOs => "Install it with Homebrew: brew install hackrf.",
    }
}

pub fn api() -> anyhow::Result<&'static HackrfApi> {
    static API: OnceLock<Result<HackrfApi, String>> = OnceLock::new();
    API.get_or_init(|| {
        let names = names(Host::current());
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        load("libhackrf", &names, resolve)
    })
    .as_ref()
    .map_err(|err| {
        anyhow::anyhow!(
            "{err}\nHackRF needs libhackrf 2023.01.1 or newer. {}",
            hint(Host::current())
        )
    })
}

pub(super) fn resolve(lib: libloading::Library) -> Result<HackrfApi, String> {
    macro_rules! sym {
        ($name:ident) => {
            unsafe { symbol(&lib, concat!(stringify!($name), "\0").as_bytes())? }
        };
    }
    Ok(HackrfApi {
        hackrf_init: sym!(hackrf_init),
        hackrf_exit: sym!(hackrf_exit),
        hackrf_close: sym!(hackrf_close),
        hackrf_device_list: sym!(hackrf_device_list),
        hackrf_device_list_free: sym!(hackrf_device_list_free),
        hackrf_device_list_open: sym!(hackrf_device_list_open),
        hackrf_version_string_read: sym!(hackrf_version_string_read),
        hackrf_is_streaming: sym!(hackrf_is_streaming),
        hackrf_set_sample_rate: sym!(hackrf_set_sample_rate),
        hackrf_set_baseband_filter_bandwidth: sym!(hackrf_set_baseband_filter_bandwidth),
        hackrf_set_freq: sym!(hackrf_set_freq),
        hackrf_set_amp_enable: sym!(hackrf_set_amp_enable),
        hackrf_start_rx: sym!(hackrf_start_rx),
        hackrf_stop_rx: sym!(hackrf_stop_rx),
        hackrf_set_lna_gain: sym!(hackrf_set_lna_gain),
        hackrf_set_vga_gain: sym!(hackrf_set_vga_gain),
        hackrf_board_partid_serialno_read: sym!(hackrf_board_partid_serialno_read),
        hackrf_board_id_read: sym!(hackrf_board_id_read),
        hackrf_board_id_name: sym!(hackrf_board_id_name),
        hackrf_error_name: sym!(hackrf_error_name),
        hackrf_board_rev_read: sym!(hackrf_board_rev_read),
        hackrf_usb_api_version_read: sym!(hackrf_usb_api_version_read),
        hackrf_get_m0_state: unsafe { optional_symbol(&lib, b"hackrf_get_m0_state\0") },
        hackrf_get_transfer_buffer_size: unsafe {
            optional_symbol(&lib, b"hackrf_get_transfer_buffer_size\0")
        },
        hackrf_get_transfer_queue_depth: unsafe {
            optional_symbol(&lib, b"hackrf_get_transfer_queue_depth\0")
        },
        _lib: lib,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On Linux the names are unchanged; on macOS the `.dylib`s Homebrew
    /// and MacPorts install, Homebrew's Apple Silicon prefix among the
    /// places tried.
    #[test]
    fn the_library_is_looked_for_by_the_hosts_names() {
        assert_eq!(
            names(Host::Linux),
            ["libhackrf.so.0", "libhackrf.so"].map(String::from)
        );
        let mac = names(Host::MacOs);
        assert_eq!(mac[0], "libhackrf.0.dylib");
        assert!(
            mac.contains(&"/opt/homebrew/lib/libhackrf.0.dylib".to_string()),
            "{mac:?}"
        );
        assert!(!mac.iter().any(|n| n.contains(".so")), "{mac:?}");
    }

    /// The install hint names the host's package manager.
    #[test]
    fn the_install_hint_names_the_hosts_package_manager() {
        assert!(hint(Host::MacOs).contains("brew install hackrf"));
        assert!(hint(Host::Linux).contains("libhackrf0 on Debian/Ubuntu"));
    }
}
