// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The measuring half of sdrtop's spectrum: one frame's DSP and what is
//! measured from it, with no lock, no clock and no idea who reads it.
//!
//! - [`frame`]: per-frame DSP - decode, transform, average. Full rate.
//! - [`analysis`]: the measurements. Display rate.
//! - [`carrier`]: the carrier at centre, its occupied bandwidth and channel
//!   power, which the panels share.
//! - [`acpr`]: the adjacent channels, and the offset a carrier of each kind
//!   is measured at.
//!
//! Published only so that the programs built on it can be installed from
//! crates.io. It has no stability promise of its own: its version is
//! sdrtop's, and a version of it works with the same version of the other
//! `monas` crates and no other.

// The documentation is its developers', so a public item's comment may
// point at the private one that explains it. rustdoc still resolves every
// such link, and a broken one fails the doc build as it always did.
#![allow(rustdoc::private_intra_doc_links)]

pub mod acpr;
pub mod analysis;
pub mod carrier;
pub mod frame;

pub use acpr::acpr_offset_hz;
pub use carrier::{centre_radius_bins, strongest_real_bin};

/// Floor for a magnitude of zero, so silence reads as a number rather than −∞.
///
/// Public because the NET receivers report powers in the same dBFS, and one
/// scale for one quantity everywhere it appears is rule 5.
pub const DB_FLOOR: f32 = -160.0;
