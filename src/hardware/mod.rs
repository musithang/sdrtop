// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The hardware layer, and the map of it.
//!
//! Three backends, in two groups. [`native`] holds the radios sdrtop drives
//! itself and has physically tested; [`soapy`] holds everything reachable
//! through libSoapySDR, under the replacement rule described there. Neither
//! group knows the other exists. [`discovery`] is the only module that sees
//! both, because deciding what to offer when they find the same radio is a
//! question neither can answer alone.
//!
//! The rest is shared and backend-neutral: [`traits`] is the vocabulary,
//! [`process`] the per-sample decode all three feed, [`gain`] the placement
//! policy, [`sysfs`] the read-only USB scan behind observer mode.
//!
//! **What this file re-exports is what the rest of the app may know about the
//! hardware**, so nothing backend-specific belongs in the list below. A fact
//! about one radio is reached through the module that owns it, spelled out at
//! the call site: `native::hackrf::board_rev_name` says whose board revision it
//! is in a way that `hardware::board_rev_name` did not.

pub mod discovery;
pub mod gain;
pub mod native;
pub mod process;
pub mod soapy;
pub mod sysfs;
mod traits;

pub use discovery::{list_all_devices, open_device, DeviceKind, DeviceListing};
pub use traits::{
    Boost, DeliveryModel, DemodBlock, DeviceCapabilities, DeviceInfo, GainModel, RxContext,
    SampleFormat, SampleGeometry, SdrDevice, SoftwareStack, StageSpec,
};
