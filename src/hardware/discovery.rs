// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Which radios exist, and which of them to offer.
//!
//! The one module that legitimately sees both worlds. A backend describes only
//! itself; nothing in `native/` knows that `soapy/` exists and nothing in
//! `soapy/` knows about the native pair. Deciding what to show when two of them
//! found the same radio is a question neither can answer alone, so it is asked
//! here.
//!
//! Everything in this file is about identity and offering. Opening a device is
//! the backend's job; [`open_device`] only dispatches to it.

use std::sync::Arc;

use super::native::{hackrf, rtlsdr};
use super::{soapy, sysfs, DeviceCapabilities, SdrDevice};

/// Which backend a [`DeviceListing`] / open request targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceKind {
    HackRf,
    RtlSdr,
    Soapy,
}

/// What observer mode needs in order to describe a device it cannot open.
///
/// **One answer rather than two questions.** A backend either has both a sysfs
/// scan and a capability profile or it has neither, and two methods that must
/// agree are two facts that eventually will not.
#[derive(Clone, Copy)]
pub struct ObserverProfile {
    /// Finds this backend's device in sysfs, by USB vendor and product id.
    pub scan: fn() -> Option<sysfs::HackRfSysInfo>,
    /// Placeholder capabilities, so the panels keep this radio's labels rather
    /// than another radio's.
    ///
    /// Placeholders in the literal sense: `rtlsdr::observer_caps` carries an
    /// empty gain table, which is why `Boot::observer` does not clamp against
    /// it. See the comment there.
    pub caps: fn() -> DeviceCapabilities,
}

impl DeviceKind {
    /// How observer mode describes this backend, when it can.
    ///
    /// Observer mode reads sysfs for a USB device sdrtop knows by vendor and
    /// product id. There is no generic equivalent for "whatever SoapySDR was
    /// talking to", so a Soapy device that will not open simply will not open,
    /// and says why.
    ///
    /// `None` **is** that refusal, and it is why nothing downstream carries an
    /// unreachable match arm any more: a caller that has a profile got it from
    /// here, and one that did not never reached the code that needs it.
    pub fn observer_profile(&self) -> Option<ObserverProfile> {
        match self {
            DeviceKind::HackRf => Some(ObserverProfile {
                scan: sysfs::find_hackrf,
                caps: hackrf::caps,
            }),
            DeviceKind::RtlSdr => Some(ObserverProfile {
                scan: sysfs::find_rtlsdr,
                caps: rtlsdr::observer_caps,
            }),
            DeviceKind::Soapy => None,
        }
    }
}

/// One enumerated device, before it is opened. `index` is the per-backend index
/// the backend's `open(index)` expects. `label` is the human string shown in the
/// device selector.
#[derive(Clone, Debug)]
pub struct DeviceListing {
    pub kind: DeviceKind,
    pub index: usize,
    pub label: String,
    /// The device's serial as its own backend spells it, when it has one.
    ///
    /// Kept so the same radio reached two ways can be recognised as one radio.
    /// See [`list_all_devices`].
    pub serial: Option<String>,
    /// SoapySDR device arguments, `None` for the two native backends.
    ///
    /// Soapy identifies a device by key/value arguments, not by a position in a
    /// list, and an index into a list that may have been re-enumerated since is
    /// a race. The two native backends genuinely are index addressed, so they
    /// carry nothing here.
    pub args: Option<String>,
}

/// Normalise a serial so two backends' spellings of the same radio compare
/// equal.
///
/// Lowercased and stripped of leading zeros. All-zero collapses to `None`,
/// because an unprogrammed serial is not an identity and matching on it would
/// make every such device look like every other one.
///
/// **Device identity, not driver argument parsing**, which is why it lives here
/// rather than beside the SoapySDR argument helpers it used to sit with. Both
/// its callers are in this file and one of them runs it over a *native*
/// backend's serial.
pub fn normalise_serial(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_start_matches('0').to_ascii_lowercase();
    (!trimmed.is_empty()).then_some(trimmed)
}

/// Parse what `--device` named: a backend, and for SoapySDR the device
/// arguments that may ride along, as in `--device soapy=driver=airspy`.
///
/// Those arguments are both a filter and the way to ask for a driver sdrtop
/// hides by default, which is why they travel together with the backend rather
/// than as a flag of their own.
///
/// Returns an error rather than exiting, so the parse is testable and the
/// process only ends in `main`, where ending it is the job.
pub fn parse_device_arg(spec: &str) -> anyhow::Result<(DeviceKind, Option<String>)> {
    let (name, filter) = match spec.split_once('=') {
        Some((n, f)) => (n, Some(f.to_string())),
        None => (spec, None),
    };
    let kind = match name.to_ascii_lowercase().as_str() {
        "hackrf" => DeviceKind::HackRf,
        "rtlsdr" | "rtl-sdr" | "rtl" => DeviceKind::RtlSdr,
        "soapy" | "soapysdr" => DeviceKind::Soapy,
        other => anyhow::bail!(
            "Unknown --device '{other}' (use 'hackrf', 'rtlsdr', or \
             'soapy', optionally as 'soapy=driver=airspy')"
        ),
    };
    Ok((kind, filter))
}

/// What else the operator could look at, when enumeration found nothing.
///
/// Empty when there is nothing useful to add. Mentioning SoapySDR to someone who
/// does not have it installed is a wild goose chase, and they have enough to
/// check already.
///
/// Asked here rather than in `main` so that finding out costs the caller no
/// knowledge of which backends exist or how one reports itself present.
pub fn no_device_hint() -> &'static str {
    if soapy::api::api().is_some() {
        " SoapySDR is installed: `SoapySDRUtil --find` lists what it can see."
    } else {
        ""
    }
}

/// Every connected device across all compiled-in backends. Never fails: a
/// backend with no devices (or an enumeration error) simply contributes nothing.
///
/// `want` is the backend `--device` named, and `soapy_filter` the argument
/// string from `--device soapy=...`. Both are applied here rather than by the
/// caller, because they interact: asking for SoapySDR by name turns off the
/// deduplication that would otherwise leave nothing to return.
pub fn list_all_devices(
    want: Option<DeviceKind>,
    soapy_filter: Option<&str>,
) -> Vec<DeviceListing> {
    let mut out = Vec::new();
    out.extend(hackrf::list());
    out.extend(rtlsdr::list());
    // A radio the native backend also found is normally dropped. Not when the
    // user asked for SoapySDR by name: then the Soapy path is the thing they
    // wanted, and hiding it would leave `--device soapy` finding nothing at all
    // on the one machine where it is most obviously supposed to work.
    let native = if want == Some(DeviceKind::Soapy) {
        &[][..]
    } else {
        &out[..]
    };
    let soapy = offer_soapy(soapy::device::list(), native, soapy_filter);
    out.extend(soapy);
    if let Some(want) = want {
        out.retain(|d| d.kind == want);
    }
    out
}

/// Which Soapy listings to actually show, given what the native backends already
/// found.
///
/// Pure, and separated for that reason: the two rules here are the ones that
/// would be maddening to get wrong and impossible to test through a real
/// enumeration.
fn offer_soapy(
    found: Vec<DeviceListing>,
    native: &[DeviceListing],
    filter: Option<&str>,
) -> Vec<DeviceListing> {
    // Serials the native backends have already claimed. `None` serials are not
    // collected: an empty identity must not match another empty identity, or one
    // unprogrammed radio would hide every other one.
    let claimed: Vec<String> = native
        .iter()
        .filter_map(|d| d.serial.as_deref())
        .filter_map(normalise_serial)
        .collect();

    found
        .into_iter()
        .filter(|d| {
            let args = d.args.as_deref().unwrap_or("");
            match filter {
                // Asked for by name: the user gets exactly what they asked for,
                // including anything that would otherwise hide itself.
                Some(f) if !f.is_empty() => soapy::args::matches_filter(args, f),
                // Otherwise, everything that does not hide itself. Which
                // listings those are is SoapySDR's own business and is answered
                // in `soapy::args`; nothing here knows why one would.
                _ => !soapy::args::hidden_by_default(args),
            }
        })
        // **The native backend wins.** sdrtop's own HackRF and RTL-SDR paths know
        // more about those radios than the generic one does, and that is the
        // whole reason they exist. Showing the same radio twice, behaving
        // differently, would be worse than not supporting Soapy at all.
        .filter(|d| {
            d.serial
                .as_deref()
                .and_then(normalise_serial)
                .is_none_or(|s| !claimed.contains(&s))
        })
        .collect()
}

/// Opens the device a listing points at, as a trait object.
pub fn open_device(listing: &DeviceListing) -> anyhow::Result<Arc<dyn SdrDevice>> {
    match listing.kind {
        DeviceKind::HackRf => Ok(Arc::new(hackrf::HackRfDevice::open(listing.index)?)),
        DeviceKind::RtlSdr => Ok(Arc::new(rtlsdr::RtlDevice::open(listing.index)?)),
        DeviceKind::Soapy => {
            let Some(args) = listing.args.as_deref() else {
                anyhow::bail!("a SoapySDR listing with no device arguments cannot be opened");
            };
            Ok(Arc::new(soapy::device::SoapyDevice::open(args)?))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listing(
        kind: DeviceKind,
        label: &str,
        serial: Option<&str>,
        args: Option<&str>,
    ) -> DeviceListing {
        DeviceListing {
            kind,
            index: 0,
            label: label.to_string(),
            serial: serial.map(str::to_string),
            args: args.map(str::to_string),
        }
    }

    /// The real pair from this machine: `hackrf_info` and `SoapySDRUtil --find`
    /// report the same serial, so the same radio is enumerated twice.
    fn native_hackrf() -> DeviceListing {
        listing(
            DeviceKind::HackRf,
            "HackRF One \u{00b7} 0000000000000000955c64dc2a3d89c3",
            Some("0000000000000000955c64dc2a3d89c3"),
            None,
        )
    }

    fn soapy_hackrf() -> DeviceListing {
        listing(
            DeviceKind::Soapy,
            "SoapySDR \u{00b7} hackrf \u{00b7} HackRF One #0 955c64dc2a3d89c3",
            Some("0000000000000000955c64dc2a3d89c3"),
            Some("driver=hackrf, serial=0000000000000000955c64dc2a3d89c3"),
        )
    }

    /// A listing that hides itself. The sound card is the only driver that does
    /// so today; which ones do is answered by `soapy::args::hidden_by_default`
    /// and tested there. These tests are about what the offering policy does
    /// with such a listing, not about which driver it is.
    fn soapy_hidden() -> DeviceListing {
        listing(
            DeviceKind::Soapy,
            "SoapySDR \u{00b7} audio \u{00b7} Built-in Audio",
            None,
            Some("driver=audio, device_id=0"),
        )
    }

    fn soapy_airspy() -> DeviceListing {
        listing(
            DeviceKind::Soapy,
            "SoapySDR \u{00b7} airspy \u{00b7} AirSpy R2",
            Some("644064DC3639AF31"),
            Some("driver=airspy, serial=644064DC3639AF31"),
        )
    }

    /// One radio, two backends, one row. The native path knows more about a
    /// HackRF than the generic one does, so it is the one that survives.
    #[test]
    fn a_radio_reached_two_ways_is_listed_once() {
        let native = vec![native_hackrf()];
        let out = offer_soapy(vec![soapy_hackrf(), soapy_airspy()], &native, None);
        assert_eq!(out.len(), 1, "the duplicate HackRF is gone");
        assert!(out[0].label.contains("airspy"));
    }

    /// The case that would be maddening and is easy to write by accident: two
    /// devices with no serial are not the same device.
    ///
    /// An implementation that compares `None == None`, or normalises a missing
    /// serial to an empty string, hides every unidentified Soapy device behind
    /// the first unidentified native one.
    #[test]
    fn devices_with_no_serial_are_not_duplicates_of_each_other() {
        let native = vec![listing(
            DeviceKind::RtlSdr,
            "RTL-SDR \u{00b7} #0",
            None,
            None,
        )];
        let out = offer_soapy(
            vec![soapy_airspy(), soapy_hidden()],
            &native,
            Some("driver=audio"),
        );
        assert_eq!(out.len(), 1, "the audio device must survive: {out:?}");
        assert!(out[0].label.contains("audio"));
    }

    /// An all-zero serial is padding, not an identity, and must not match
    /// another all-zero one.
    #[test]
    fn an_unprogrammed_serial_does_not_swallow_other_devices() {
        let native = vec![listing(
            DeviceKind::HackRf,
            "HackRF One",
            Some("00000000"),
            None,
        )];
        let blank = listing(
            DeviceKind::Soapy,
            "SoapySDR \u{00b7} mystery",
            Some("0000"),
            Some("driver=mystery"),
        );
        let out = offer_soapy(vec![blank], &native, None);
        assert_eq!(out.len(), 1, "two unprogrammed radios are still two radios");
    }

    /// A listing that hides itself is skipped by default, and reachable by name.
    ///
    /// Without the first half, sdrtop starts on any laptop with the audio module
    /// installed and no radio attached at all. Naming a driver is what overrides
    /// the hiding, and it overrides it for any driver, not for a special-cased
    /// one.
    #[test]
    fn a_self_hiding_listing_is_skipped_unless_it_is_asked_for() {
        let out = offer_soapy(vec![soapy_hidden(), soapy_airspy()], &[], None);
        assert_eq!(out.len(), 1);
        assert!(out[0].label.contains("airspy"), "{out:?}");

        let asked = offer_soapy(
            vec![soapy_hidden(), soapy_airspy()],
            &[],
            Some("driver=audio"),
        );
        assert_eq!(asked.len(), 1);
        assert!(asked[0].label.contains("audio"), "{asked:?}");
    }

    /// Asking for SoapySDR by name has to hand back the Soapy path even for a
    /// radio the native backend also found.
    ///
    /// Found by running it: `--device soapy` printed "No SDR device found" on a
    /// machine with a HackRF, because the deduplication had already dropped the
    /// only Soapy device before the backend filter ran.
    #[test]
    fn asking_for_soapy_by_name_turns_off_the_deduplication() {
        // The dedup as it applies by default.
        let deduped = offer_soapy(vec![soapy_hackrf()], &[native_hackrf()], None);
        assert!(deduped.is_empty(), "the native path wins by default");
        // And with no native list to compare against, which is what
        // `list_all_devices` passes when SoapySDR was asked for.
        let asked = offer_soapy(vec![soapy_hackrf()], &[], None);
        assert_eq!(asked.len(), 1, "asking by name must find it: {asked:?}");
    }

    /// A filter narrows the list to what it names, and an empty one is not a
    /// filter at all.
    #[test]
    fn a_device_filter_narrows_to_what_it_names() {
        let all = vec![soapy_hackrf(), soapy_airspy()];
        let out = offer_soapy(all.clone(), &[], Some("driver=airspy"));
        assert_eq!(out.len(), 1);
        assert!(out[0].label.contains("airspy"));

        let empty = offer_soapy(all, &[], Some(""));
        assert_eq!(empty.len(), 2, "an empty filter is the default, not a ban");
    }

    /// The deduplication compares this against the native backend's serial. On
    /// this machine libhackrf and SoapyHackRF agree byte for byte, but agreeing
    /// by luck is not a rule.
    #[test]
    fn serials_compare_after_normalising_padding_and_case() {
        assert_eq!(
            normalise_serial("0000000000000000955c64dc2a3d89c3").unwrap(),
            "955c64dc2a3d89c3",
            "leading zeros are padding, not identity"
        );
        assert_eq!(
            normalise_serial("955C64DC2A3D89C3"),
            normalise_serial("0000000000000000955c64dc2a3d89c3"),
            "and the two backends may not agree on case"
        );
    }

    /// The four shapes `--device` comes in. `main` never tested any of them,
    /// because the parse lived inside it and ended in `process::exit`.
    #[test]
    fn a_device_argument_names_a_backend_and_may_carry_its_arguments() {
        assert_eq!(
            parse_device_arg("hackrf").unwrap(),
            (DeviceKind::HackRf, None)
        );
        assert_eq!(
            parse_device_arg("soapy=driver=airspy").unwrap(),
            (DeviceKind::Soapy, Some("driver=airspy".to_string())),
            "only the first = separates the name from the arguments"
        );
        assert!(parse_device_arg("airspy").is_err(), "not a backend name");
        assert!(parse_device_arg("").is_err(), "nor is nothing");
    }

    /// Spellings that have to keep working, and the case fold.
    #[test]
    fn the_backend_names_have_the_aliases_the_help_text_promises() {
        for s in ["rtlsdr", "rtl-sdr", "rtl", "RTL-SDR"] {
            assert_eq!(parse_device_arg(s).unwrap().0, DeviceKind::RtlSdr, "{s}");
        }
        for s in ["soapy", "soapysdr", "SoapySDR"] {
            assert_eq!(parse_device_arg(s).unwrap().0, DeviceKind::Soapy, "{s}");
        }
    }

    /// `--device soapy=` is a backend with an empty filter, and an empty filter
    /// is not a filter: it must offer the same list as `--device soapy`.
    ///
    /// Worth pinning across the two functions rather than in either, because
    /// the parse produces `Some("")` and the offering treats it as absent. One
    /// of those changing alone is the bug.
    #[test]
    fn an_empty_argument_string_is_not_a_filter() {
        let (kind, filter) = parse_device_arg("soapy=").unwrap();
        assert_eq!(kind, DeviceKind::Soapy);
        assert_eq!(filter.as_deref(), Some(""));
        let all = vec![soapy_hackrf(), soapy_airspy()];
        assert_eq!(offer_soapy(all, &[], filter.as_deref()).len(), 2);
    }

    /// Observer mode reads sysfs for a USB device sdrtop knows by vendor and
    /// product id, and only the two native backends are such devices.
    ///
    /// This used to be a comment in three files and a `debug_assert!` in one of
    /// them. A refusal that only fires in a debug build is not a tested fact.
    #[test]
    fn only_the_native_backends_can_be_observed() {
        assert!(DeviceKind::HackRf.observer_profile().is_some());
        assert!(DeviceKind::RtlSdr.observer_profile().is_some());
        assert!(
            DeviceKind::Soapy.observer_profile().is_none(),
            "there is no sysfs profile for whatever SoapySDR was talking to"
        );
    }

    /// The failure the deleted assertion existed to prevent: a user staring at
    /// one radio's gain labels while looking at another.
    ///
    /// A profile pairs a scan with the capabilities that belong beside it, so
    /// the way to get this wrong is to swap the two halves of one arm. Naming
    /// the labels is what catches that; asserting merely that both are `Some`
    /// would not.
    #[test]
    fn an_observer_profile_carries_its_own_backends_labels() {
        let hackrf = (DeviceKind::HackRf.observer_profile().unwrap().caps)();
        let rtl = (DeviceKind::RtlSdr.observer_profile().unwrap().caps)();
        assert_eq!(hackrf.gain.primary_label(), "LNA");
        assert!(
            hackrf.gain.has_second_stage(),
            "a HackRF has an LNA and a VGA"
        );
        assert_eq!(rtl.gain.primary_label(), "Tuner");
        assert!(!rtl.gain.has_second_stage(), "an RTL-SDR is one tuner");
    }

    /// The case that would otherwise make every unprogrammed device look like
    /// every other one, and quietly hide all but the first.
    #[test]
    fn an_all_zero_or_blank_serial_is_no_serial() {
        assert_eq!(normalise_serial("00000000"), None, "all padding");
        assert_eq!(normalise_serial("   "), None);
        assert_eq!(normalise_serial(""), None);
    }
}
