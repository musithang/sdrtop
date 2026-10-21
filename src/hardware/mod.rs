// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The hardware layer, and the map of it.
//!
//! [`native`] holds the radios sdrtop drives itself. [`soapy`] holds everything
//! reachable through libSoapySDR. [`tinysa`] holds swept spectrum analyzers.
//! [`discovery`] is the only module that sees all three groups.
//!
//! The rest is shared and backend-neutral: [`traits`] is the vocabulary,
//! [`process`] the per-sample decode all three feed, [`intake`] the queue that
//! keeps it off the driver's thread, [`record_tap`] the
//! recorder's feed out of it, [`gain`] the placement policy, [`usb`] the
//! read-only USB scan behind observer mode, [`runtime_lib`] where a radio's
//! runtime library is looked for on the host.
//!
//! **What this file re-exports is what the rest of the app may know about the
//! hardware**, so nothing backend-specific belongs in the list below. A fact
//! about one radio is reached through the module that owns it, spelled out at
//! the call site: `native::hackrf::board_rev_name` says whose board revision it
//! is in a way that `hardware::board_rev_name` did not.

pub mod discovery;
pub mod gain;
pub mod intake;
pub mod native;
pub mod process;
pub mod record_tap;
pub mod runtime_lib;
pub mod soapy;
pub mod tinysa;
mod traits;
pub mod usb;

pub use discovery::{list_all_devices, open_device, DeviceKind, DeviceListing};
pub use record_tap::RecordTap;
#[cfg(test)]
pub(crate) use traits::RateSet;
pub use traits::{
    AcquisitionKind, Boost, DeliveryModel, DeviceCapabilities, DeviceInfo, DeviceOption,
    DirectSweepConfig, FeedHealth, GainModel, LevelUnit, PowerTrace, PowerTraceTarget, RadioDrops,
    RxContext, SampleFormat, SampleGeometry, SdrDevice, SoftwareStack, StageReading, StageSpec,
    StreamBlock, IQ_TRACE_STALE_MS,
};

pub(crate) fn debug_assert_device_options(options: &[DeviceOption]) {
    #[cfg(debug_assertions)]
    {
        let mut ids = std::collections::HashSet::with_capacity(options.len());
        for option in options {
            debug_assert!(
                !option.choices.is_empty(),
                "device option '{}' must advertise a choice",
                option.id
            );
            debug_assert!(
                option.choices.contains(&option.selected_choice),
                "device option '{}' selected an unavailable choice",
                option.id
            );
            debug_assert!(
                ids.insert(option.id.as_str()),
                "device option IDs must be unique within a snapshot"
            );
            debug_assert!(
                option.integer_range.is_none()
                    || option.choices.iter().any(|choice| {
                        option.integer_choice(choice).ok() == Some(choice.as_str())
                    }),
                "numeric device option '{}' must advertise a canonical integer within its bounds",
                option.id
            );
        }
    }
    #[cfg(not(debug_assertions))]
    let _ = options;
}

#[cfg(all(test, debug_assertions))]
mod tests {
    use super::*;

    fn option(id: &str, choices: &[&str], selected: &str) -> DeviceOption {
        DeviceOption {
            id: id.into(),
            label: "Bandwidth".into(),
            choices: choices.iter().map(|choice| (*choice).into()).collect(),
            selected_choice: selected.into(),
            integer_range: None,
        }
    }

    #[test]
    #[should_panic(expected = "must advertise a choice")]
    fn option_contract_rejects_an_empty_choice_list() {
        debug_assert_device_options(&[option("bandwidth", &[], "")]);
    }

    #[test]
    #[should_panic(expected = "selected an unavailable choice")]
    fn option_contract_rejects_an_unavailable_selection() {
        debug_assert_device_options(&[option("bandwidth", &["Narrow", "Wide"], "Missing")]);
    }

    #[test]
    #[should_panic(expected = "must be unique")]
    fn option_contract_rejects_duplicate_ids() {
        debug_assert_device_options(&[
            option("bandwidth", &["Narrow"], "Narrow"),
            option("bandwidth", &["Wide"], "Wide"),
        ]);
    }

    #[test]
    fn numeric_contract_accepts_sparse_choices_and_auto() {
        let mut numeric = option("level", &["auto", "0", "10"], "auto");
        numeric.integer_range = Some(0..=31);
        debug_assert_device_options(&[numeric]);
    }

    #[test]
    #[should_panic(expected = "must advertise a canonical integer within its bounds")]
    fn numeric_contract_rejects_choices_outside_bounds() {
        let mut numeric = option("level", &["auto", "32"], "auto");
        numeric.integer_range = Some(0..=31);
        debug_assert_device_options(&[numeric]);
    }

    #[test]
    #[should_panic(expected = "must advertise a canonical integer within its bounds")]
    fn numeric_contract_rejects_noncanonical_integers() {
        let mut numeric = option("level", &["auto", "+1", "01", "-0", "1.0"], "auto");
        numeric.integer_range = Some(0..=31);
        debug_assert_device_options(&[numeric]);
    }

    #[test]
    #[should_panic(expected = "must advertise a canonical integer within its bounds")]
    fn numeric_contract_rejects_reversed_bounds() {
        let mut numeric = option("level", &["0"], "0");
        numeric.integer_range = Some(std::ops::RangeInclusive::new(31, 0));
        debug_assert_device_options(&[numeric]);
    }
}
