// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! libairspy, loaded at run time. The declarations follow `airspy.h` from
//! libairspy 1.0.10; every symbol is resolved before the backend is offered,
//! so a library missing one is refused whole rather than failing halfway
//! through a session.

use libc::{c_char, c_int, c_void};
use std::sync::OnceLock;

use crate::hardware::runtime_lib;
use crate::host::Host;

use super::super::loader::{load, symbol};

/// `airspy_lib_version_t`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct LibVersion {
    pub major: u32,
    pub minor: u32,
    pub revision: u32,
}

/// `AIRSPY_SAMPLE_INT16_IQ`: interleaved I/Q, int16, the converter's 12 bits
/// shifted up by four and filtered.
pub const SAMPLE_INT16_IQ: c_int = 2;

/// `airspy_transfer_t`: one block, handed to the callback on libairspy's
/// consumer thread.
#[repr(C)]
pub struct Transfer {
    pub device: *mut c_void,
    pub ctx: *mut c_void,
    pub samples: *mut c_void,
    /// I/Q pairs in `samples` for the IQ sample types.
    pub sample_count: c_int,
    /// Samples lost in libairspy's own queue just before this block: whole
    /// USB buffers it had no room for, each counted at this block's size.
    pub dropped_samples: u64,
    pub sample_type: c_int,
}

/// `airspy_sample_block_cb_fn`. A non-zero return asks libairspy to stop.
pub type SampleBlockFn = extern "C" fn(*mut Transfer) -> c_int;

pub struct AirspyApi {
    // Keeps the library mapped for as long as the API is reachable
    _lib: libloading::Library,
    /// With a null buffer, the number of Airspy devices on the bus; otherwise
    /// fills up to `count` serials and returns how many. Negative on error.
    pub airspy_lib_version: unsafe extern "C" fn(*mut LibVersion),
    pub airspy_list_devices: unsafe extern "C" fn(*mut u64, c_int) -> c_int,
    pub airspy_open_sn: unsafe extern "C" fn(*mut *mut c_void, u64) -> c_int,
    pub airspy_close: unsafe extern "C" fn(*mut c_void) -> c_int,
    pub airspy_board_id_read: unsafe extern "C" fn(*mut c_void, *mut u8) -> c_int,
    pub airspy_board_id_name: unsafe extern "C" fn(c_int) -> *const c_char,
    pub airspy_version_string_read: unsafe extern "C" fn(*mut c_void, *mut c_char, u8) -> c_int,
    pub airspy_error_name: unsafe extern "C" fn(c_int) -> *const c_char,
    pub airspy_set_sample_type: unsafe extern "C" fn(*mut c_void, c_int) -> c_int,
    /// An index into the firmware's rate list, or a rate in Hz from 1 MHz up.
    pub airspy_set_samplerate: unsafe extern "C" fn(*mut c_void, u32) -> c_int,
    pub airspy_set_freq: unsafe extern "C" fn(*mut c_void, u32) -> c_int,
    /// Register positions, not decibels: LNA 0..14, mixer and VGA 0..15.
    pub airspy_set_lna_gain: unsafe extern "C" fn(*mut c_void, u8) -> c_int,
    pub airspy_set_mixer_gain: unsafe extern "C" fn(*mut c_void, u8) -> c_int,
    pub airspy_set_vga_gain: unsafe extern "C" fn(*mut c_void, u8) -> c_int,
    pub airspy_set_lna_agc: unsafe extern "C" fn(*mut c_void, u8) -> c_int,
    pub airspy_set_mixer_agc: unsafe extern "C" fn(*mut c_void, u8) -> c_int,
    /// 4.5 V on the antenna port. Only ever switched off here.
    pub airspy_set_rf_bias: unsafe extern "C" fn(*mut c_void, u8) -> c_int,
    /// With `len` zero, writes the number of rates the firmware offers into
    /// the buffer's first element; otherwise copies that many rates in Hz,
    /// complex rates for the IQ sample types (the default at open).
    /// Starts libairspy's USB and consumer threads; the callback runs on the
    /// consumer thread with `ctx` in each transfer.
    pub airspy_start_rx: unsafe extern "C" fn(*mut c_void, SampleBlockFn, *mut c_void) -> c_int,
    /// Joins both threads before returning when the stream was running.
    pub airspy_stop_rx: unsafe extern "C" fn(*mut c_void) -> c_int,
    pub airspy_is_streaming: unsafe extern "C" fn(*mut c_void) -> c_int,
    /// 12-bit packing on the USB link; refused while streaming.
    pub airspy_set_packing: unsafe extern "C" fn(*mut c_void, u8) -> c_int,
    pub airspy_get_samplerates: unsafe extern "C" fn(*mut c_void, *mut u32, u32) -> c_int,
}

// The soname Debian, Ubuntu and Arch ship, then the development link.
const LINUX: &[&str] = &["libairspy.so.0", "libairspy.so"];
// Homebrew installs libairspy.0.dylib and its development link.
const MACOS: &[&str] = &["libairspy.0.dylib", "libairspy.dylib"];

/// The names libairspy is tried by on `host`, in order.
fn names(host: Host) -> Vec<String> {
    runtime_lib::candidates(host, LINUX, MACOS)
}

/// What to install on `host` when none of them opened.
fn hint(host: Host) -> &'static str {
    match host {
        Host::Linux => "Install libairspy0 on Debian and Ubuntu, or airspy on Arch.",
        Host::MacOs => "Install it with Homebrew: brew install airspy.",
    }
}

pub fn api() -> anyhow::Result<&'static AirspyApi> {
    static API: OnceLock<Result<AirspyApi, String>> = OnceLock::new();
    API.get_or_init(|| {
        let names = names(Host::current());
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        load("libairspy", &names, resolve)
    })
    .as_ref()
    .map_err(|err| {
        anyhow::anyhow!(
            "{err}\nAirspy needs the libairspy runtime. {}",
            hint(Host::current())
        )
    })
}

pub(super) fn resolve(lib: libloading::Library) -> Result<AirspyApi, String> {
    macro_rules! sym {
        ($name:ident) => {
            unsafe { symbol(&lib, concat!(stringify!($name), "\0").as_bytes())? }
        };
    }
    Ok(AirspyApi {
        airspy_lib_version: sym!(airspy_lib_version),
        airspy_list_devices: sym!(airspy_list_devices),
        airspy_open_sn: sym!(airspy_open_sn),
        airspy_close: sym!(airspy_close),
        airspy_board_id_read: sym!(airspy_board_id_read),
        airspy_board_id_name: sym!(airspy_board_id_name),
        airspy_version_string_read: sym!(airspy_version_string_read),
        airspy_error_name: sym!(airspy_error_name),
        airspy_set_sample_type: sym!(airspy_set_sample_type),
        airspy_set_samplerate: sym!(airspy_set_samplerate),
        airspy_set_freq: sym!(airspy_set_freq),
        airspy_set_lna_gain: sym!(airspy_set_lna_gain),
        airspy_set_mixer_gain: sym!(airspy_set_mixer_gain),
        airspy_set_vga_gain: sym!(airspy_set_vga_gain),
        airspy_set_lna_agc: sym!(airspy_set_lna_agc),
        airspy_set_mixer_agc: sym!(airspy_set_mixer_agc),
        airspy_set_rf_bias: sym!(airspy_set_rf_bias),
        airspy_start_rx: sym!(airspy_start_rx),
        airspy_stop_rx: sym!(airspy_stop_rx),
        airspy_is_streaming: sym!(airspy_is_streaming),
        airspy_set_packing: sym!(airspy_set_packing),
        airspy_get_samplerates: sym!(airspy_get_samplerates),
        _lib: lib,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On Linux the soname Debian, Ubuntu and Arch install, then the
    /// development link; on macOS Homebrew's and MacPorts' `.dylib`s.
    #[test]
    fn the_library_is_looked_for_by_the_hosts_names() {
        assert_eq!(
            names(Host::Linux),
            ["libairspy.so.0", "libairspy.so"].map(String::from)
        );
        let mac = names(Host::MacOs);
        assert_eq!(mac[0], "libairspy.0.dylib");
        assert!(
            mac.contains(&"/opt/homebrew/lib/libairspy.0.dylib".to_string()),
            "{mac:?}"
        );
        assert!(!mac.iter().any(|n| n.contains(".so")), "{mac:?}");
    }

    /// The install hint names the package each system calls it by.
    #[test]
    fn the_install_hint_names_the_hosts_package() {
        assert!(hint(Host::Linux).contains("libairspy0"));
        assert!(hint(Host::Linux).contains("airspy on Arch"));
        assert!(hint(Host::MacOs).contains("brew install airspy"));
    }
}
