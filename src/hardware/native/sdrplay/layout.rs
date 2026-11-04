// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The C structs of SDRplay's API that sdrtop reads or writes, laid out as
//! the API's 3.15 headers lay them out.
//!
//! SDRplay's API hands its state over as structs the caller reads and writes
//! in place, so a field one offset off is not a crash but a value written
//! into the wrong field. The layout is therefore a checked fact, tied to the
//! API version it was checked against: [`super::ffi::CHECKED_VERSIONS`] lists
//! the versions whose headers produced exactly these offsets, and the backend
//! refuses any other version by name. The tests hold the offsets twice: against
//! the fixture library's copy of the declarations (in every test run), and
//! against the installed headers themselves, compiled by the C compiler (an
//! ignored test, for a machine that has them).

use libc::{c_char, c_int, c_void};

/// The longest serial the API writes, NUL included (`SDRPLAY_MAX_SER_NO_LEN`).
pub const SERIAL_LEN: usize = 64;

/// How many radios `sdrplay_api_GetDevices` may describe at once
/// (`SDRPLAY_MAX_DEVICES`).
pub const MAX_DEVICES: usize = 16;

/// `sdrplay_api_DeviceT`: one radio as `sdrplay_api_GetDevices` lists it.
/// The two enums (`tuner`, `rspDuoMode`) are C `int`s.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct DeviceT {
    pub ser_no: [c_char; SERIAL_LEN],
    /// Which RSP: 6 for the RSP1B (`SDRPLAY_RSP1B_ID`).
    pub hw_ver: u8,
    pub tuner: c_int,
    pub rsp_duo_mode: c_int,
    /// Whether the entry describes a radio this process may select.
    pub valid: u8,
    pub rsp_duo_sample_freq: f64,
    /// The handle, filled in by `sdrplay_api_SelectDevice`.
    pub dev: *mut c_void,
}

impl Default for DeviceT {
    fn default() -> Self {
        Self {
            ser_no: [0; SERIAL_LEN],
            hw_ver: 0,
            tuner: 0,
            rsp_duo_mode: 0,
            valid: 0,
            rsp_duo_sample_freq: 0.0,
            dev: std::ptr::null_mut(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::test_support::TestLibrary;
    use super::*;
    use std::mem::{offset_of, size_of};

    /// The offsets of [`DeviceT`] in the order the fixture and the probe
    /// report them: size, then each field after the serial.
    fn rust_device_layout() -> [usize; 7] {
        [
            size_of::<DeviceT>(),
            offset_of!(DeviceT, hw_ver),
            offset_of!(DeviceT, tuner),
            offset_of!(DeviceT, rsp_duo_mode),
            offset_of!(DeviceT, valid),
            offset_of!(DeviceT, rsp_duo_sample_freq),
            offset_of!(DeviceT, dev),
        ]
    }

    /// The fixture library's copy of `sdrplay_api_DeviceT` and ours agree.
    #[test]
    fn the_device_struct_matches_the_fixtures_c_layout() {
        let fixture = TestLibrary::new(false);
        let c: Vec<usize> = (0..7).map(|field| fixture.sdrplay_layout(field)).collect();
        assert_eq!(c, rust_device_layout());
    }

    /// Against the API's own headers, compiled: the version they declare is
    /// one sdrtop checked, and every offset of [`DeviceT`] is theirs. Needs
    /// SDRplay's API installed (its headers under the compiler's include
    /// path), which no package manager provides, so it is never run by CI.
    #[test]
    #[ignore = "requires SDRplay's API headers (sdrplay_api.h) installed"]
    fn the_installed_headers_lay_the_device_struct_out_as_we_do() {
        let dir = std::env::temp_dir().join(format!("sdrtop-sdrplay-probe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("probe.c");
        let binary = dir.join("probe");
        std::fs::write(
            &source,
            r#"#include <stdio.h>
#include <stddef.h>
#include <sdrplay_api.h>
int main(void) {
    printf("%.2f %d\n", (double)SDRPLAY_API_VERSION, SDRPLAY_RSP1B_ID);
    printf("%zu %zu %zu %zu %zu %zu %zu\n", sizeof(sdrplay_api_DeviceT),
        offsetof(sdrplay_api_DeviceT, hwVer), offsetof(sdrplay_api_DeviceT, tuner),
        offsetof(sdrplay_api_DeviceT, rspDuoMode), offsetof(sdrplay_api_DeviceT, valid),
        offsetof(sdrplay_api_DeviceT, rspDuoSampleFreq), offsetof(sdrplay_api_DeviceT, dev));
    return 0;
}
"#,
        )
        .unwrap();
        let built = std::process::Command::new("cc")
            .args(["-std=c11", "-Wall", "-Werror", "-o"])
            .arg(&binary)
            .arg(&source)
            .output()
            .expect("a C compiler");
        assert!(
            built.status.success(),
            "{}",
            String::from_utf8_lossy(&built.stderr)
        );
        let out = std::process::Command::new(&binary).output().unwrap();
        let text = String::from_utf8(out.stdout).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let mut lines = text.lines();
        let mut head = lines.next().unwrap().split_whitespace();
        let version = head.next().unwrap();
        assert!(
            super::super::ffi::CHECKED_VERSIONS.contains(&version),
            "the installed headers are {version}, not a version sdrtop checked"
        );
        assert_eq!(head.next(), Some("6"), "the RSP1B's hardware id");
        let c: Vec<usize> = lines
            .next()
            .unwrap()
            .split_whitespace()
            .map(|n| n.parse().unwrap())
            .collect();
        assert_eq!(c, rust_device_layout());
    }
}
