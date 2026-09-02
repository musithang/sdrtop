// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

pub mod hackrf;
pub mod process;
pub mod rtlsdr;
pub mod soapy;
pub mod sysfs;
pub mod traits;

pub use hackrf::{board_rev_name, compute_bb_filter_bw};
pub use traits::{
    DeviceCapabilities, DeviceInfo, GainModel, RxContext, SampleFormat, SampleGeometry, SdrDevice,
    SoftwareStack,
};

use std::sync::Arc;

/// Which backend a [`DeviceListing`] / open request targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceKind {
    HackRf,
    RtlSdr,
    Soapy,
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

/// The driver key SoapySDR gives a sound card.
///
/// Skipped unless asked for by name. On any desktop with `soapysdr-module-audio`
/// installed, enumeration otherwise reports the built-in microphone as an SDR,
/// and sdrtop would start on a laptop with no radio attached at all. It is a
/// real SDR source for anyone running a soundcard receiver, so it is one flag
/// away rather than gone: `--device soapy=driver=audio`.
const AUDIO_DRIVER: &str = "audio";

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
        .filter_map(soapy::args::normalise_serial)
        .collect();

    found
        .into_iter()
        .filter(|d| {
            let args = d.args.as_deref().unwrap_or("");
            match filter {
                // Asked for by name: the user gets exactly what they asked for,
                // audio included.
                Some(f) if !f.is_empty() => soapy::args::matches_filter(args, f),
                // Otherwise every driver but the sound card.
                _ => soapy::args::value_of(args, "driver") != Some(AUDIO_DRIVER),
            }
        })
        // **The native backend wins.** sdrtop's own HackRF and RTL-SDR paths know
        // more about those radios than the generic one does, and that is the
        // whole reason they exist. Showing the same radio twice, behaving
        // differently, would be worse than not supporting Soapy at all.
        .filter(|d| {
            d.serial
                .as_deref()
                .and_then(soapy::args::normalise_serial)
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

    fn soapy_audio() -> DeviceListing {
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
            vec![soapy_airspy(), soapy_audio()],
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

    /// The sound card is skipped by default, and reachable by name.
    ///
    /// Without the first half, sdrtop starts on any laptop with the audio module
    /// installed and no radio attached at all.
    #[test]
    fn the_audio_driver_is_skipped_unless_it_is_asked_for() {
        let out = offer_soapy(vec![soapy_audio(), soapy_airspy()], &[], None);
        assert_eq!(out.len(), 1);
        assert!(out[0].label.contains("airspy"), "{out:?}");

        let asked = offer_soapy(
            vec![soapy_audio(), soapy_airspy()],
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
}
