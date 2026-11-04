// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! SDRplay's API (`libsdrplay_api`), loaded at run time.
//!
//! The library is closed: what it does is known from SDRplay's API
//! specification and from measuring it, not from its source. It talks to a
//! service, `sdrplay_apiService`, which owns the radios; without the service
//! running, `sdrplay_api_Open` fails and nothing else can be asked.
//!
//! Its structs are read and written in place, so which API versions are
//! accepted is a layout question first ([`super::layout`]): only
//! [`CHECKED_VERSIONS`] are, and every other version is refused by name.
//!
//! The declarations follow SDRplay's `sdrplay_api.h` 3.15. The macOS names
//! are written from SDRplay's macOS installer as others describe it (a
//! `.so`, under `/usr/local/lib`); nobody here has run it on a Mac.

use libc::{c_char, c_int};
use std::sync::OnceLock;

use crate::hardware::runtime_lib;
use crate::host::Host;

use super::super::loader::{load, symbol};
use super::layout::DeviceT;

/// `sdrplay_api_ErrT`: success.
pub const SUCCESS: c_int = 0;

/// The API versions whose headers lay out every struct sdrtop touches as
/// [`super::layout`] does, spelled as `sdrplay_api_ApiVersion` reports them
/// to two decimals. 3.14 added the RSP1B but its headers could not be had to
/// check, so it is refused like any other version.
pub const CHECKED_VERSIONS: &[&str] = &["3.15"];

/// `sdrplay_api_ApiVersion` as two decimals, the way SDRplay numbers its API
/// ("3.15"); the library reports a float.
pub fn version_text(version: f32) -> String {
    format!("{version:.2}")
}

/// Whether sdrtop checked the layout of this API version.
pub fn is_checked(version: f32) -> bool {
    CHECKED_VERSIONS.contains(&version_text(version).as_str())
}

/// Each field is the C function of the same name with `sdrplay_api_` in
/// front (`open` is `sdrplay_api_Open`).
pub struct SdrplayApi {
    // Keeps the library mapped for as long as the API is reachable
    _lib: libloading::Library,
    /// The first call of a session: opens the channel to the service. Fails,
    /// with `sdrplay_api_Fail`, when the service is not running.
    pub open: unsafe extern "C" fn() -> c_int,
    /// The last call of a session, after every `Open` that succeeded;
    /// SDRplay's specification warns that skipping it can leave the service
    /// in an unknown state.
    pub close: unsafe extern "C" fn() -> c_int,
    pub api_version: unsafe extern "C" fn(*mut f32) -> c_int,
    /// Held around `GetDevices` (and later `SelectDevice`), as SDRplay's own
    /// examples do, so two programs do not pick the same radio at once.
    pub lock_device_api: unsafe extern "C" fn() -> c_int,
    pub unlock_device_api: unsafe extern "C" fn() -> c_int,
    /// Fills up to `max` entries and writes how many. A radio another program
    /// has selected is not among them at all.
    pub get_devices: unsafe extern "C" fn(*mut DeviceT, *mut u32, u32) -> c_int,
    /// The error's name, from a static table: never freed.
    pub get_error_string: unsafe extern "C" fn(c_int) -> *const c_char,
}

// SDRplay's installer and the AUR's `libsdrplay` both install
// libsdrplay_api.so.3 with a development link beside it.
const LINUX: &[&str] = &["libsdrplay_api.so.3", "libsdrplay_api.so"];
// SDRplay's macOS installer ships the same `.so` names.
const MACOS: &[&str] = &["libsdrplay_api.so.3", "libsdrplay_api.so"];

/// The names libsdrplay_api is tried by on `host`, in order.
fn names(host: Host) -> Vec<String> {
    runtime_lib::candidates(host, LINUX, MACOS)
}

/// What to install on `host` when none of them opened. No package manager
/// carries SDRplay's API: it comes from SDRplay.
fn hint(host: Host) -> &'static str {
    match host {
        Host::Linux => {
            "Install SDRplay's API from https://www.sdrplay.com/api/ (on Arch, libsdrplay from \
             the AUR), then start its service: sudo systemctl start sdrplay."
        }
        Host::MacOs => "Install SDRplay's API for macOS from https://www.sdrplay.com/api/.",
    }
}

pub fn api() -> anyhow::Result<&'static SdrplayApi> {
    static API: OnceLock<Result<SdrplayApi, String>> = OnceLock::new();
    API.get_or_init(|| {
        let names = names(Host::current());
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        load("libsdrplay_api", &names, resolve)
    })
    .as_ref()
    .map_err(|err| {
        anyhow::anyhow!(
            "{err}\nAn SDRplay RSP needs SDRplay's API. {}",
            hint(Host::current())
        )
    })
}

pub(super) fn resolve(lib: libloading::Library) -> Result<SdrplayApi, String> {
    macro_rules! sym {
        ($name:literal) => {
            unsafe { symbol(&lib, concat!($name, "\0").as_bytes())? }
        };
    }
    Ok(SdrplayApi {
        open: sym!("sdrplay_api_Open"),
        close: sym!("sdrplay_api_Close"),
        api_version: sym!("sdrplay_api_ApiVersion"),
        lock_device_api: sym!("sdrplay_api_LockDeviceApi"),
        unlock_device_api: sym!("sdrplay_api_UnlockDeviceApi"),
        get_devices: sym!("sdrplay_api_GetDevices"),
        get_error_string: sym!("sdrplay_api_GetErrorString"),
        _lib: lib,
    })
}

impl SdrplayApi {
    /// The API's own name for an error code, or the bare code when it gives
    /// none.
    pub fn error_name(&self, code: c_int) -> String {
        // SAFETY: the API returns a pointer into a static table, or null.
        let ptr = unsafe { (self.get_error_string)(code) };
        if ptr.is_null() {
            return format!("error {code}");
        }
        // SAFETY: a NUL-terminated string the API keeps for the process's life.
        unsafe { std::ffi::CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On Linux the soname, then the development link; on macOS the same
    /// `.so` names, bare and under each known prefix (`/usr/local/lib` among
    /// them, where SDRplay's installer is said to put them).
    #[test]
    fn the_library_is_looked_for_by_the_hosts_names() {
        assert_eq!(
            names(Host::Linux),
            ["libsdrplay_api.so.3", "libsdrplay_api.so"].map(String::from)
        );
        let mac = names(Host::MacOs);
        assert_eq!(mac[0], "libsdrplay_api.so.3");
        assert!(
            mac.contains(&"/usr/local/lib/libsdrplay_api.so.3".to_string()),
            "{mac:?}"
        );
    }

    /// No package manager carries it, so the hint says where it comes from,
    /// and on Linux that its service has to run.
    #[test]
    fn the_install_hint_names_sdrplay_and_the_service() {
        assert!(hint(Host::Linux).contains("sdrplay.com/api"));
        assert!(hint(Host::Linux).contains("libsdrplay from the AUR"));
        assert!(hint(Host::Linux).contains("systemctl start sdrplay"));
        assert!(hint(Host::MacOs).contains("sdrplay.com/api"));
    }

    /// 3.15 is the version whose headers were checked; 3.14, which first
    /// knew the RSP1B, is refused like any other, and so is anything newer.
    #[test]
    fn only_checked_versions_pass() {
        // The versions as the API reports them: floats, so read from text
        // (3.14 written as a literal reads to clippy as an attempt at pi).
        let v = |text: &str| text.parse::<f32>().unwrap();
        assert!(is_checked(v("3.15")));
        assert!(
            is_checked(v("3.15") + 1e-6),
            "a float a hair off is still 3.15"
        );
        assert!(!is_checked(v("3.14")));
        assert!(!is_checked(v("3.16")));
        assert!(!is_checked(v("4.0")));
        assert_eq!(version_text(v("3.14")), "3.14");
    }
}
