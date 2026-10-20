// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Where a radio's runtime library is looked for, on the host sdrtop runs on.
//!
//! The radio libraries are opened at run time, so a missing one disables only
//! its own backend. Each backend owns its library's names; this module only
//! says where to try them. On Linux that is the names as they are, for the
//! dynamic loader's own search. On macOS the names are `.dylib`s, and
//! Homebrew on Apple Silicon installs them under `/opt/homebrew/lib`, which
//! the loader's default fallback does not search (MacPorts' `/opt/local/lib`
//! neither), so each name is tried bare first, leaving `DYLD_LIBRARY_PATH`
//! and the default fallback the first word, then under each known prefix.
//!
//! The host is a parameter, so the macOS lists are held by tests that run on
//! Linux, with no Mac and no library anywhere.

/// The host's family, as far as finding a library goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Host {
    Linux,
    MacOs,
}

impl Host {
    /// The host this build runs on.
    pub fn current() -> Host {
        if cfg!(target_os = "macos") {
            Host::MacOs
        } else {
            Host::Linux
        }
    }
}

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
