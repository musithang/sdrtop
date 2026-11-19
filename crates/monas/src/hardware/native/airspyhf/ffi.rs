// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! libairspyhf, loaded at run time. The declarations follow `airspyhf.h`
//! from libairspyhf 1.6.8, the release every distribution ships; every symbol
//! required here is in it and is resolved before the backend is offered, so a
//! library missing one is refused whole rather than failing halfway through a
//! session. What later versions add is looked up as optional, by the feature
//! that needs it.

use libc::{c_char, c_int, c_void};
use std::sync::OnceLock;

use crate::hardware::runtime_lib;
use crate::host::Host;

use super::super::loader::{load, symbol};

/// `airspyhf_read_partid_serialno_t`. The part id is the board: 1 the HF+
/// Dual Port, 2 the HF+ Discovery, 3 the Ranger (the 1.8.1 header's
/// `airspyhf_board_id`; 1.6.8's names none of them, but the firmware answers
/// the same either way).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct PartSerial {
    pub part_id: u32,
    pub serial_no: [u32; 4],
}

/// The size of the buffer libairspyhf copies a firmware version out of
/// (`MAX_VERSION_STRING_SIZE`). `airspyhf_version_string_read` copies
/// `length - 1` bytes from a buffer of this size whatever `length` is, so a
/// longer request would read past it.
pub const VERSION_STRING_SIZE: u8 = 64;

/// `airspyhf_transfer_t`: one block, handed to the callback on libairspyhf's
/// consumer thread.
#[repr(C)]
pub struct Transfer {
    pub device: *mut c_void,
    pub ctx: *mut c_void,
    /// `sample_count` complex floats, `{re, im}`: the radio's 16-bit words
    /// times the firmware's filter gain over 32768, corrected and turned by
    /// the library while its DSP is on.
    pub samples: *mut c_void,
    pub sample_count: c_int,
    /// Samples lost in libairspyhf's own queue just before this block: whole
    /// buffers it had no room for, each counted at this block's size.
    pub dropped_samples: u64,
}

/// `airspyhf_sample_block_cb_fn`. A non-zero return asks libairspyhf to stop.
pub type SampleBlockFn = extern "C" fn(*mut Transfer) -> c_int;

/// `airspyhf_lib_version_t`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct LibVersion {
    pub major: u32,
    pub minor: u32,
    pub revision: u32,
}

pub struct AirspyHfApi {
    // Keeps the library mapped for as long as the API is reachable
    _lib: libloading::Library,
    /// With a null buffer, the number of HF+ radios on the bus; otherwise
    /// fills up to `count` serials and returns how many. Negative on error.
    pub airspyhf_list_devices: unsafe extern "C" fn(*mut u64, c_int) -> c_int,
    /// Fails, with `AIRSPYHF_ERROR`, for a radio another program has
    /// claimed: listing it needs no claim, opening it does.
    pub airspyhf_open_sn: unsafe extern "C" fn(*mut *mut c_void, u64) -> c_int,
    pub airspyhf_close: unsafe extern "C" fn(*mut c_void) -> c_int,
    pub airspyhf_board_partid_serialno_read:
        unsafe extern "C" fn(*mut c_void, *mut PartSerial) -> c_int,
    /// See [`VERSION_STRING_SIZE`] for the longest `length` that is safe.
    pub airspyhf_version_string_read: unsafe extern "C" fn(*mut c_void, *mut c_char, u8) -> c_int,
    pub airspyhf_lib_version: unsafe extern "C" fn(*mut LibVersion),
    /// The correction the radio's flash holds for its reference, parts per
    /// billion; the library applies it to every tuning.
    pub airspyhf_get_calibration: unsafe extern "C" fn(*mut c_void, *mut i32) -> c_int,
    /// Hz. The firmware tunes in whole kHz; with the library's DSP on, the
    /// rest is a rotation of the samples.
    pub airspyhf_set_freq: unsafe extern "C" fn(*mut c_void, u32) -> c_int,
    /// A rate in Hz from the firmware's list, or an index into it.
    pub airspyhf_set_samplerate: unsafe extern "C" fn(*mut c_void, u32) -> c_int,
    /// Whether the current rate is a low-IF one (1) or zero-IF (0).
    pub airspyhf_is_low_if: unsafe extern "C" fn(*mut c_void) -> c_int,
    /// Pairs per callback: 2048 in 1.6.8, 4096 in later versions.
    pub airspyhf_get_output_size: unsafe extern "C" fn(*mut c_void) -> c_int,
    /// The library's IQ correction, IF shift and fine tuning, on (1) or off.
    pub airspyhf_set_lib_dsp: unsafe extern "C" fn(*mut c_void, u8) -> c_int,
    /// The attenuator by its index, 0..8, 6 dB a step.
    pub airspyhf_set_hf_att: unsafe extern "C" fn(*mut c_void, u8) -> c_int,
    /// The LNA, 0 or 1: 6 dB on the level when on (measured; the header's
    /// "compensated in digital" is not what the firmware does).
    pub airspyhf_set_hf_lna: unsafe extern "C" fn(*mut c_void, u8) -> c_int,
    /// The firmware's AGC, which sets the attenuator while on.
    pub airspyhf_set_hf_agc: unsafe extern "C" fn(*mut c_void, u8) -> c_int,
    pub airspyhf_set_hf_agc_threshold: unsafe extern "C" fn(*mut c_void, u8) -> c_int,
    /// Starts libairspyhf's USB and consumer threads; the callback runs on the
    /// consumer thread with `ctx` in each transfer.
    pub airspyhf_start: unsafe extern "C" fn(*mut c_void, SampleBlockFn, *mut c_void) -> c_int,
    /// Joins both threads, in 1.6.8 only for a stream it still calls running.
    pub airspyhf_stop: unsafe extern "C" fn(*mut c_void) -> c_int,
    pub airspyhf_is_streaming: unsafe extern "C" fn(*mut c_void) -> c_int,
    /// With `len` zero, writes the number of rates the firmware offers into
    /// the buffer's first element; otherwise copies that many rates in Hz.
    pub airspyhf_get_samplerates: unsafe extern "C" fn(*mut c_void, *mut u32, u32) -> c_int,
}

// Upstream's soname (the AUR build, a source install), Debian's and Ubuntu's
// renamed one, then the development link.
const LINUX: &[&str] = &["libairspyhf.so.0", "libairspyhf.so.1", "libairspyhf.so"];
// Homebrew and MacPorts build upstream's CMake: libairspyhf.0.dylib.
const MACOS: &[&str] = &["libairspyhf.0.dylib", "libairspyhf.dylib"];

/// The names libairspyhf is tried by on `host`, in order.
fn names(host: Host) -> Vec<String> {
    runtime_lib::candidates(host, LINUX, MACOS)
}

/// What to install on `host` when none of them opened.
fn hint(host: Host) -> &'static str {
    match host {
        Host::Linux => {
            "Install libairspyhf1 on Debian and Ubuntu, or airspyhf-git from the AUR on Arch."
        }
        Host::MacOs => "Install it with Homebrew: brew install airspyhf.",
    }
}

pub fn api() -> anyhow::Result<&'static AirspyHfApi> {
    static API: OnceLock<Result<AirspyHfApi, String>> = OnceLock::new();
    API.get_or_init(|| {
        let names = names(Host::current());
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        load("libairspyhf", &names, resolve)
    })
    .as_ref()
    .map_err(|err| {
        anyhow::anyhow!(
            "{err}\nAirspy HF+ needs the libairspyhf runtime. {}",
            hint(Host::current())
        )
    })
}

pub(super) fn resolve(lib: libloading::Library) -> Result<AirspyHfApi, String> {
    macro_rules! sym {
        ($name:ident) => {
            unsafe { symbol(&lib, concat!(stringify!($name), "\0").as_bytes())? }
        };
    }
    Ok(AirspyHfApi {
        airspyhf_list_devices: sym!(airspyhf_list_devices),
        airspyhf_open_sn: sym!(airspyhf_open_sn),
        airspyhf_close: sym!(airspyhf_close),
        airspyhf_board_partid_serialno_read: sym!(airspyhf_board_partid_serialno_read),
        airspyhf_version_string_read: sym!(airspyhf_version_string_read),
        airspyhf_lib_version: sym!(airspyhf_lib_version),
        airspyhf_get_calibration: sym!(airspyhf_get_calibration),
        airspyhf_set_freq: sym!(airspyhf_set_freq),
        airspyhf_set_samplerate: sym!(airspyhf_set_samplerate),
        airspyhf_is_low_if: sym!(airspyhf_is_low_if),
        airspyhf_get_output_size: sym!(airspyhf_get_output_size),
        airspyhf_set_lib_dsp: sym!(airspyhf_set_lib_dsp),
        airspyhf_set_hf_att: sym!(airspyhf_set_hf_att),
        airspyhf_set_hf_lna: sym!(airspyhf_set_hf_lna),
        airspyhf_set_hf_agc: sym!(airspyhf_set_hf_agc),
        airspyhf_set_hf_agc_threshold: sym!(airspyhf_set_hf_agc_threshold),
        airspyhf_start: sym!(airspyhf_start),
        airspyhf_stop: sym!(airspyhf_stop),
        airspyhf_is_streaming: sym!(airspyhf_is_streaming),
        airspyhf_get_samplerates: sym!(airspyhf_get_samplerates),
        _lib: lib,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On Linux upstream's soname first, then the one Debian and Ubuntu
    /// renamed it to, then the development link; on macOS Homebrew's and
    /// MacPorts' `.dylib`s.
    #[test]
    fn the_library_is_looked_for_by_the_hosts_names() {
        assert_eq!(
            names(Host::Linux),
            ["libairspyhf.so.0", "libairspyhf.so.1", "libairspyhf.so"].map(String::from)
        );
        let mac = names(Host::MacOs);
        assert_eq!(mac[0], "libairspyhf.0.dylib");
        assert!(
            mac.contains(&"/opt/homebrew/lib/libairspyhf.0.dylib".to_string()),
            "{mac:?}"
        );
        assert!(!mac.iter().any(|n| n.contains(".so")), "{mac:?}");
    }

    /// The install hint names the package each system calls it by.
    #[test]
    fn the_install_hint_names_the_hosts_package() {
        assert!(hint(Host::Linux).contains("libairspyhf1"));
        assert!(hint(Host::Linux).contains("airspyhf-git from the AUR"));
        assert!(hint(Host::MacOs).contains("brew install airspyhf"));
    }
}
