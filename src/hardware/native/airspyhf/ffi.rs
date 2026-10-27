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
