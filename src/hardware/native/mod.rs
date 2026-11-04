// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The backends sdrtop drives itself: HackRF One, RTL-SDR, the Airspy R2 and
//! the Airspy HF+ Discovery, with the SDRplay RSP1B on its way in ([`sdrplay`]).
//!
//! Each is a thin FFI wrapper ([`hackrf::ffi`], [`rtlsdr::ffi`]) plus a device
//! struct, loaded at runtime and described from its own datasheet. What they
//! have in common is that **support here lands only after physical testing**,
//! which is what separates them from [`super::soapy`], where that rule is
//! suspended and replaced.
//!
//! Neither backend knows the other exists, and neither knows about SoapySDR.
//! Deciding between them, when the same radio is reachable two ways, is
//! [`super::discovery`]'s job.

pub mod airspy;
pub mod airspyhf;
pub mod hackrf;
mod loader;
pub mod rtlsdr;
pub mod sdrplay;

#[cfg(test)]
mod test_support;

#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "requires installed libhackrf, librtlsdr, libairspy and libairspyhf runtimes"]
    fn installed_native_libraries_resolve() {
        super::airspy::ffi::api().expect("the installed libairspy must resolve");
        super::airspyhf::ffi::api().expect("the installed libairspyhf must resolve");
        super::hackrf::ffi::api().expect("the installed libhackrf must resolve");
        super::rtlsdr::ffi::api().expect("the installed librtlsdr must resolve");
    }

    /// Apart from the others because no package manager carries SDRplay's
    /// API, so CI, which installs the others, cannot run it.
    #[test]
    #[ignore = "requires SDRplay's API installed"]
    fn installed_sdrplay_api_resolves() {
        super::sdrplay::ffi::api().expect("the installed libsdrplay_api must resolve");
    }
}
