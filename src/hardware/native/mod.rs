// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The backends sdrtop drives itself: HackRF One and RTL-SDR.
//!
//! Each is a thin FFI wrapper ([`hackrf::ffi`], [`rtlsdr::ffi`]) plus a device
//! struct, linked at build time and described from its own datasheet. What they
//! have in common is that **support here lands only after physical testing**,
//! which is what separates them from [`super::soapy`], where that rule is
//! suspended and replaced.
//!
//! Neither backend knows the other exists, and neither knows about SoapySDR.
//! Deciding between them, when the same radio is reachable two ways, is
//! [`super::discovery`]'s job.

pub mod hackrf;
pub mod rtlsdr;
