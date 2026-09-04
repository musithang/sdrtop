// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Device arguments: what a driver told us about a device, and what we tell it
//! back to open the same one again.
//!
//! **Entirely safe and entirely pure.** Everything here works on
//! `[(String, String)]`, which is what [`super::api::kwargs_to_vec`] produces,
//! so every decision in this file can be tested on a machine with no library, no
//! driver and no radio. That is the point of the split.
//!
//! The example data in the tests is real, copied from `SoapySDRUtil --find` on a
//! machine with a HackRF One and a sound card, rather than invented. Inventing
//! it is how you end up handling a shape no driver produces and missing the one
//! every driver does.

/// The separator the device selector already uses for the two native backends.
const SEP: &str = " \u{00b7} "; // ·

/// The value for `key`, if the driver reported one and it is not blank.
///
/// Blank is treated as absent on purpose: a driver that reports `serial=` has
/// not told us a serial, and letting an empty string through would make every
/// such device look like the same device to the deduplication in `list_all`.
pub fn get<'a>(args: &'a [(String, String)], key: &str) -> Option<&'a str> {
    args.iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
        .filter(|v| !v.trim().is_empty())
}

/// The line the device selector shows.
///
/// `SoapySDR · hackrf · HackRF One #0 955c64dc2a3d89c3`
///
/// Three parts, each earning its place: the backend, because the same radio can
/// appear on a native backend too and you need to know which one you picked; the
/// driver key, because that is the word you would type in
/// `--device soapy=driver=hackrf`; and the driver's own `label`, because it is
/// already a better human string than anything we would assemble.
///
/// Falling back through `serial` then `device_id` rather than inventing
/// something: a device with no label at all still needs to be distinguishable
/// from the one next to it.
pub fn label(args: &[(String, String)], index: usize) -> String {
    let driver = get(args, "driver").unwrap_or("soapy");
    let identity = get(args, "label")
        .or_else(|| get(args, "serial"))
        .or_else(|| get(args, "device_id"))
        .map(str::to_string)
        .unwrap_or_else(|| format!("device {index}"));
    format!("SoapySDR{SEP}{driver}{SEP}{identity}")
}

/// The smallest argument string that reopens this exact device, in the markup
/// `SoapySDRDevice_makeStrArgs` parses: `key=value, key=value`.
///
/// **Not the whole enumeration result.** SoapySDR's convention is that you can
/// hand an enumeration entry straight back to `make`, and that would be simpler,
/// but those entries carry free text: this HackRF reports
/// `label = HackRF One #0 955c64dc2a3d89c3`. The markup parser splits on commas
/// and equals signs and has no escaping, so echoing arbitrary driver prose back
/// through it is a bug waiting for the first driver whose label contains a
/// comma.
///
/// So: the driver key, plus the first identifier that is safe to round trip.
/// `serial` and `device_id` are hex or digits by convention and never contain a
/// separator; `label` is the last resort and the one that could.
pub fn open_markup(args: &[(String, String)], index: usize) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(driver) = get(args, "driver") {
        parts.push(format!("driver={driver}"));
    }
    match get(args, "serial")
        .map(|v| ("serial", v))
        .or_else(|| get(args, "device_id").map(|v| ("device_id", v)))
        .or_else(|| get(args, "label").map(|v| ("label", v)))
    {
        Some((key, value)) => parts.push(format!("{key}={value}")),
        // Nothing identifying at all. The enumeration index is the only handle
        // left, and it is a weak one: it is only stable until something is
        // unplugged. Better than refusing to open the device.
        None => parts.push(format!("device_id={index}")),
    }
    parts.join(", ")
}

/// One value out of an argument markup string, `driver=hackrf, serial=abc`.
///
/// The inverse of [`open_markup`], and the reason it exists is that the markup
/// is the only thing that survives from enumeration all the way to open: keeping
/// a second copy of the driver key or the serial beside it is how the two end up
/// disagreeing.
pub fn value_of<'a>(markup: &'a str, key: &str) -> Option<&'a str> {
    markup
        .split(',')
        .filter_map(|part| part.split_once('='))
        .find(|(k, _)| k.trim() == key)
        .map(|(_, v)| v.trim())
        .filter(|v| !v.is_empty())
}

/// Whether a device's arguments satisfy every `key=value` in a filter.
///
/// Used by `--device soapy=driver=airspy`. A filter naming a key the device does
/// not have matches nothing, which is the answer a person typing a filter wants:
/// silence, not everything.
pub fn matches_filter(markup: &str, filter: &str) -> bool {
    filter
        .split(',')
        .filter_map(|part| part.split_once('='))
        .all(|(k, v)| value_of(markup, k.trim()) == Some(v.trim()))
}

/// The driver key SoapySDR gives a sound card.
const AUDIO_DRIVER: &str = "audio";

/// Whether this device is one SoapySDR offers that sdrtop hides unless it is
/// asked for by name.
///
/// One driver, so far. On any desktop with `soapysdr-module-audio` installed,
/// enumeration reports the built-in microphone as an SDR, and sdrtop would
/// otherwise start on a laptop with no radio attached at all. It is a real SDR
/// source for anyone running a soundcard receiver, so it is one flag away
/// rather than gone: `--device soapy=driver=audio`.
///
/// **Which drivers behave like this is SoapySDR's business**, which is why the
/// question lives here. The module that decides what to offer asks whether a
/// listing hides itself; it does not know what a sound card is.
pub fn hidden_by_default(markup: &str) -> bool {
    value_of(markup, "driver") == Some(AUDIO_DRIVER)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kv(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// Verbatim from `SoapySDRUtil --find`.
    fn hackrf() -> Vec<(String, String)> {
        kv(&[
            ("device", "HackRF One"),
            ("driver", "hackrf"),
            ("label", "HackRF One #0 955c64dc2a3d89c3"),
            ("part_id", "a000cb3cbc4f433a"),
            ("serial", "0000000000000000955c64dc2a3d89c3"),
            ("version", "n_260829"),
        ])
    }

    /// Also verbatim, and a useful second shape: the audio driver has no serial
    /// at all, which is exactly the case that breaks a lazy identity rule.
    fn built_in_audio() -> Vec<(String, String)> {
        kv(&[
            ("default_input", "True"),
            ("default_output", "True"),
            ("device_id", "0"),
            ("driver", "audio"),
            ("label", "Built-in Audio"),
        ])
    }

    #[test]
    fn a_blank_value_reads_as_absent() {
        let args = kv(&[("serial", "   "), ("driver", "hackrf")]);
        assert_eq!(get(&args, "serial"), None);
        assert_eq!(get(&args, "driver"), Some("hackrf"));
        assert_eq!(get(&args, "nothing"), None);
    }

    #[test]
    fn the_label_names_the_backend_the_driver_and_the_device() {
        assert_eq!(
            label(&hackrf(), 1),
            "SoapySDR \u{00b7} hackrf \u{00b7} HackRF One #0 955c64dc2a3d89c3"
        );
        assert_eq!(
            label(&built_in_audio(), 0),
            "SoapySDR \u{00b7} audio \u{00b7} Built-in Audio"
        );
    }

    /// A device that reports nothing recognisable still gets a distinguishable
    /// line, because two of them side by side must not read identically.
    #[test]
    fn a_device_with_no_identity_still_gets_a_distinct_label() {
        let bare = kv(&[("driver", "mystery")]);
        assert_eq!(
            label(&bare, 0),
            "SoapySDR \u{00b7} mystery \u{00b7} device 0"
        );
        assert_ne!(label(&bare, 0), label(&bare, 1));
        // And with no driver either.
        assert_eq!(label(&[], 3), "SoapySDR \u{00b7} soapy \u{00b7} device 3");
    }

    #[test]
    fn the_open_markup_carries_the_driver_and_one_identifier() {
        assert_eq!(
            open_markup(&hackrf(), 1),
            "driver=hackrf, serial=0000000000000000955c64dc2a3d89c3"
        );
        assert_eq!(
            open_markup(&built_in_audio(), 0),
            "driver=audio, device_id=0"
        );
    }

    /// The reason `open_markup` does not simply echo the enumeration back: the
    /// markup has no escaping, so a value containing a comma would silently
    /// become two arguments. Serials and device ids never do; labels might.
    #[test]
    fn the_open_markup_prefers_identifiers_that_cannot_contain_a_separator() {
        let awkward = kv(&[
            ("driver", "weird"),
            ("label", "Radio, model 2 = the good one"),
            ("serial", "ABC123"),
        ]);
        let markup = open_markup(&awkward, 0);
        assert_eq!(markup, "driver=weird, serial=ABC123");
        assert!(!markup.contains("good one"), "the free text stayed out");
    }

    /// With no serial and no device id, the label is all that is left. It is
    /// used, and the comment above says why that is the last resort.
    #[test]
    fn the_open_markup_falls_back_to_the_label_then_to_the_index() {
        let only_label = kv(&[("driver", "x"), ("label", "Thing")]);
        assert_eq!(open_markup(&only_label, 2), "driver=x, label=Thing");
        let nothing = kv(&[("driver", "x")]);
        assert_eq!(open_markup(&nothing, 2), "driver=x, device_id=2");
    }

    /// Reading one value back out of the markup, which is how the driver key and
    /// the serial are recovered without keeping a second copy of either.
    #[test]
    fn a_value_can_be_read_back_out_of_the_markup() {
        let m = open_markup(&hackrf(), 0);
        assert_eq!(value_of(&m, "driver"), Some("hackrf"));
        assert_eq!(
            value_of(&m, "serial"),
            Some("0000000000000000955c64dc2a3d89c3")
        );
        assert_eq!(value_of(&m, "label"), None, "not in the open markup");
        assert_eq!(value_of("", "driver"), None);
    }

    /// The `--device soapy=...` filter. A filter naming a key the device does
    /// not have matches nothing, because silence is what someone typing a filter
    /// expects when it does not apply, not everything.
    #[test]
    fn a_filter_must_match_every_pair_it_names() {
        let m = open_markup(&hackrf(), 0);
        assert!(matches_filter(&m, "driver=hackrf"));
        assert!(matches_filter(
            &m,
            "driver=hackrf, serial=0000000000000000955c64dc2a3d89c3"
        ));
        assert!(!matches_filter(&m, "driver=airspy"));
        assert!(!matches_filter(&m, "driver=hackrf, serial=nope"));
        assert!(
            !matches_filter(&m, "label=anything"),
            "a key the device does not carry cannot match"
        );
        // The audio device is reachable by naming it, which is the whole point.
        let a = open_markup(&built_in_audio(), 0);
        assert!(matches_filter(&a, "driver=audio"));
    }

    /// Which drivers hide themselves is SoapySDR's business, and this is where
    /// that fact lives. The module that decides what to offer only asks the
    /// question.
    ///
    /// The sound card is the whole list. Without it sdrtop starts on any laptop
    /// with `soapysdr-module-audio` installed and no radio attached at all.
    #[test]
    fn the_sound_card_is_the_driver_that_hides_itself() {
        assert!(hidden_by_default(&open_markup(&built_in_audio(), 0)));
        assert!(!hidden_by_default(&open_markup(&hackrf(), 0)));
        assert!(
            !hidden_by_default(""),
            "a listing with no arguments names no driver, so it hides nothing"
        );
        assert!(
            !hidden_by_default("driver=audiocodec"),
            "the key is matched whole, not by prefix"
        );
    }
}
