// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Where a radio's runtime library is looked for, and how it is opened, on the
//! host sdrtop runs on.
//!
//! The radio libraries are opened at run time, so a missing one disables only
//! its own backend. Each backend owns its library's names; this module only
//! says where to try them and how to open them. On Linux the places are the
//! names as they are, for the dynamic loader's own search. On macOS the names
//! are `.dylib`s, and Homebrew on Apple Silicon installs them under
//! `/opt/homebrew/lib`, which
//! the loader's default fallback does not search (MacPorts' `/opt/local/lib`
//! neither), so each name is tried bare first, leaving `DYLD_LIBRARY_PATH`
//! and the default fallback the first word, then under each known prefix.
//!
//! The host is a parameter, so the macOS lists and flags are held by tests
//! that run on Linux, with no Mac and no library anywhere.

use crate::host::Host;
use std::os::raw::c_int;

/// Where macOS package managers put libraries the loader would not find by
/// name: Homebrew on Apple Silicon, Homebrew on Intel, MacPorts.
pub const MACOS_PREFIXES: [&str; 3] = ["/opt/homebrew/lib", "/usr/local/lib", "/opt/local/lib"];

/// The names to try on `host`, in order: `linux` as they are on Linux; on
/// macOS each of `macos` bare, then each under every prefix in
/// [`MACOS_PREFIXES`].
pub fn candidates(host: Host, linux: &[&str], macos: &[&str]) -> Vec<String> {
    match host {
        Host::Linux => linux.iter().map(|n| n.to_string()).collect(),
        Host::MacOs => macos
            .iter()
            .flat_map(|name| {
                std::iter::once(name.to_string())
                    .chain(MACOS_PREFIXES.iter().map(move |p| format!("{p}/{name}")))
            })
            .collect(),
    }
}

/// How a radio library is opened on `host`.
///
/// On macOS every one is opened global. SoapySDR's driver modules there are
/// built for flat-namespace lookup: SoapyRTLSDR and SoapyHackRF resolve
/// SoapySDR's own symbols and librtlsdr's or libhackrf's from the images the
/// process has made global. A library sdrtop opened local first stays hidden
/// from them even though the module names it as its own dependency, and the
/// module fails with "symbol not found in flat namespace" (`_rtlsdr_cancel_async`,
/// `_hackrf_board_id_name`) while `SoapySDRUtil`, which opens neither first,
/// loads it. Linux keeps the loader's default, which libloading's
/// `Library::new` has always used: there a module's own dependencies resolve
/// it, however sdrtop opened them.
pub fn open_flags(host: Host) -> c_int {
    use libloading::os::unix::{RTLD_GLOBAL, RTLD_LAZY, RTLD_LOCAL};
    match host {
        Host::Linux => RTLD_LAZY | RTLD_LOCAL,
        Host::MacOs => RTLD_LAZY | RTLD_GLOBAL,
    }
}

/// Opens the library `name` with the flags [`open_flags`] gives this host.
///
/// # Safety
/// Opening a library runs its initialisers, so `name` must be one of a
/// backend's fixed names, never user input.
pub unsafe fn open(name: &str) -> Result<libloading::Library, libloading::Error> {
    let flags = open_flags(Host::current());
    unsafe { libloading::os::unix::Library::open(Some(name), flags) }.map(libloading::Library::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On Linux the names are exactly the backend's own, in its order:
    /// nothing changes there.
    #[test]
    fn on_linux_the_names_are_the_backends_own() {
        let linux = ["librtlsdr.so.0", "librtlsdr.so.2", "librtlsdr.so"];
        assert_eq!(
            candidates(Host::Linux, &linux, &["librtlsdr.0.dylib"]),
            linux.map(String::from)
        );
    }

    /// **On macOS every radio library is opened global**, so a SoapySDR
    /// module built for flat-namespace lookup finds librtlsdr, libhackrf and
    /// libSoapySDR after sdrtop has opened them. Linux keeps the loader's
    /// default.
    #[test]
    fn on_macos_every_radio_library_is_opened_global() {
        use libloading::os::unix::{RTLD_GLOBAL, RTLD_LAZY, RTLD_LOCAL};
        assert_eq!(open_flags(Host::Linux), RTLD_LAZY | RTLD_LOCAL);
        assert_eq!(open_flags(Host::MacOs), RTLD_LAZY | RTLD_GLOBAL);
    }

    /// **On macOS each name is tried bare first**, so `DYLD_LIBRARY_PATH`
    /// and the loader's fallback still have the first word, then under
    /// Homebrew's Apple Silicon prefix, Intel's, and MacPorts', in that
    /// order; the Linux names are not tried.
    #[test]
    fn on_macos_each_name_is_tried_bare_then_under_each_prefix() {
        let got = candidates(
            Host::MacOs,
            &["librtlsdr.so.0"],
            &["librtlsdr.0.dylib", "librtlsdr.dylib"],
        );
        assert_eq!(
            got,
            [
                "librtlsdr.0.dylib",
                "/opt/homebrew/lib/librtlsdr.0.dylib",
                "/usr/local/lib/librtlsdr.0.dylib",
                "/opt/local/lib/librtlsdr.0.dylib",
                "librtlsdr.dylib",
                "/opt/homebrew/lib/librtlsdr.dylib",
                "/usr/local/lib/librtlsdr.dylib",
                "/opt/local/lib/librtlsdr.dylib",
            ]
            .map(String::from)
        );
    }
}
