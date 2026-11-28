// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The header every export begins with.
//!
//! **An export carries its provenance or it is worthless.**
//! A column of ppm values, six months later, with no record of what radio
//! measured them, in which mode, against which frequency reference and with what
//! uncertainty, is not data. It is a set of numbers someone will misread.
//!
//! Built from the same `SdrMetrics` the panels read, so the file and the screen
//! cannot describe different sessions.
//!
//! This is the reusable half of `export`, with `destination`. A later IQ-sample
//! export is a completely different body - binary, large, with its own metadata
//! sidecar - and wants exactly this header.

use crate::state::SdrMetrics;

/// The comment marker every provenance line begins with.
///
/// `#` because that is what a spreadsheet, gnuplot and `pandas.read_csv` all
/// skip by default. The header has to be ignorable by the tools people actually
/// open these files with, or it stops being provenance and starts being an
/// obstacle.
pub const COMMENT: &str = "#";

/// Seconds from the Unix epoch to a civil date and time, as `2026-09-05T18:22:41Z`.
///
/// **Hand-rolled because the alternative is a dependency for one line.** The
/// civil-from-days conversion is Howard Hinnant's, which is short, exact for
/// every date in the proleptic Gregorian calendar, and - the reason it is worth
/// using rather than inventing - has a published derivation to check against.
/// A date routine that is wrong is wrong for years at a time and nobody notices
/// until the data is old, so this one is tested against fixed points rather than
/// against itself.
pub fn iso8601(unix_secs: i64) -> String {
    // Floor division, so a time before the epoch takes the day below it rather
    // than truncating towards zero into the following day.
    let days = unix_secs.div_euclid(86_400);
    let secs = unix_secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        secs / 3600,
        (secs / 60) % 60,
        secs % 60
    )
}

/// Days since 1970-01-01 to `(year, month, day)`.
pub(crate) fn civil_from_days(z: i64) -> (i64, u32, u32) {
    // Shift the epoch to 0000-03-01, which puts the leap day at the end of the
    // year and makes the whole thing arithmetic rather than a table of months.
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// `00:41:12` - a duration in the form the header shows it.
pub fn clock(secs: u64) -> String {
    format!(
        "{:02}:{:02}:{:02}",
        secs / 3600,
        (secs / 60) % 60,
        secs % 60
    )
}

/// The provenance block, one line per fact.
///
/// `unix_secs` is passed in rather than read here so the block is a pure
/// function of the state and the clock, and can be asserted whole.
///
/// `writer` is the program writing the file and its version, which the
/// first line names: the terminal and the headless server each say their
/// own.
pub fn block(state: &SdrMetrics, unix_secs: i64, writer: &str) -> Vec<String> {
    std::iter::once(format!("{COMMENT} {writer}"))
        .chain(
            fields(state, unix_secs)
                .into_iter()
                .map(|(name, value)| format!("{COMMENT} {name:<12} {value}")),
        )
        .collect()
}

/// The header's fields, name and value, as [`block`] writes them and as the
/// API serves them: one account of where a measurement came from, whichever
/// way it leaves.
pub fn fields(state: &SdrMetrics, unix_secs: i64) -> Vec<(&'static str, String)> {
    let now = std::time::Instant::now();
    let geometry = state.caps.sample_geometry;
    let session = state
        .radio
        .rx_start_time
        .map(|t| clock(now.saturating_duration_since(t).as_secs()))
        .unwrap_or_else(|| "not streaming".to_string());
    let mut fields = vec![
        ("exported", iso8601(unix_secs)),
        (
            "device",
            format!(
                "{}  serial {}",
                state.system.board_name, state.system.serial
            ),
        ),
        (
            "tuning",
            format!(
                "{:.3} MHz   {:.3} Msps   {} bit  fs={}",
                state.radio.frequency as f64 / 1e6,
                state.radio.config_sample_rate / 1e6,
                state.converter_bits(),
                geometry.full_scale,
            ),
        ),
        ("mode", mode_text(state)),
    ];
    // NET's rows show addresses in this mode, so the file says which; FT8's
    // distances are measured from you, so it says from where. A mode with
    // neither has nothing to say about either.
    if state.mode.feeds.ft8 {
        fields.push(("station", station_line(state)));
    } else if state.mode.feeds.net {
        fields.push(("addresses", addresses_line(state)));
    }
    fields.push(("reference", reference_line(state, now)));
    fields.push(("session", session));
    fields
}

/// What the engine was doing: FT8 and the band it was on, NET's mode, or
/// the view whose work it did.
fn mode_text(state: &SdrMetrics) -> String {
    if state.mode.feeds.ft8 {
        let hz = state.radio.frequency;
        let band = crate::signal::ft8::bands::band_of(hz)
            .map_or("not an FT8 frequency".to_string(), str::to_string);
        format!("FT8 / {band}, dial {:.3} MHz", hz as f64 / 1e6)
    } else if state.mode.feeds.net {
        format!("NET / {}", state.net.mode.label())
    } else if state.mode.view.is_empty() {
        "spectrum".to_string()
    } else {
        state.mode.view.clone()
    }
}

/// Where the distances in an FT8 file are measured from, and whose call
/// marks a decode as yours: each said absent when it is.
fn station_line(state: &SdrMetrics) -> String {
    let at = state
        .home
        .position()
        .and_then(|(lat, lon)| crate::geo::maidenhead::locator(lat, lon, 3))
        .unwrap_or_else(|| "no position, so no distances".to_string());
    let call = state.home.callsign.as_deref().unwrap_or("no call");
    format!("{at}   {call}")
}

/// How the addresses below are shown, and, when a registrant's name can be
/// in them, which snapshot of the IEEE's listing it came from.
fn addresses_line(state: &SdrMetrics) -> String {
    let mode = state.net.address_display;
    match mode {
        crate::state::AddressDisplay::Full => mode.label().to_string(),
        _ => format!(
            "{}, registrants from the IEEE listing of {}, companies from the SIG registry of {}",
            mode.label(),
            crate::signal::net::vendor::fetched(),
            crate::signal::assigned::fetched()
        ),
    }
}

/// How the frequency reference is described, or why it is not.
pub(crate) fn reference_line(state: &SdrMetrics, now: std::time::Instant) -> String {
    // **What it is worth now, not what it was worth when it was taken**, as
    // the engine's view decides it: an expired reference exports as no
    // reference at all, its figure left out, because a number in a file
    // outlives the sentence beside it.
    match crate::engine::view::reference(state, now) {
        crate::engine::view::Reading::Value(r) => format!(
            "{}, {:.2} ppm +/- {:.2} against {}, measured {} ago",
            r.provenance,
            r.ppm,
            r.sigma_ppm,
            r.source,
            clock(r.age_s)
        ),
        crate::engine::view::Reading::Absent(why) => format!("unreferenced, {why}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fixed points, not a round trip against the same arithmetic.
    #[test]
    fn the_epoch_and_the_awkward_dates_come_out_right() {
        assert_eq!(iso8601(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso8601(1), "1970-01-01T00:00:01Z");
        // The design's own example line.
        assert_eq!(iso8601(1_788_632_561), "2026-09-05T18:22:41Z");
        // A leap day, and the day after it.
        assert_eq!(iso8601(1_709_164_800), "2024-02-29T00:00:00Z");
        assert_eq!(iso8601(1_709_251_200), "2024-03-01T00:00:00Z");
        // **The century rule, which is the one that catches out a hand-rolled
        // calendar.** A year divisible by 100 is not a leap year unless it is
        // also divisible by 400. 2000 was; 1900 and 2100 are not.
        //
        // Testing 2000 alone proves nothing: it is a leap year under both the
        // right rule and the naive every-fourth-year one, so a conversion
        // missing the correction passes it. This test claimed to cover the rule
        // while doing exactly that, and a deliberate break that removed the
        // correction went unnoticed. The dates below are the ones that bite.
        assert_eq!(iso8601(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(iso8601(4_107_456_000), "2100-02-28T00:00:00Z");
        assert_eq!(
            iso8601(4_107_542_400),
            "2100-03-01T00:00:00Z",
            "2100 is not a leap year"
        );
        assert_eq!(iso8601(-2_203_977_600), "1900-02-28T00:00:00Z");
        assert_eq!(
            iso8601(-2_203_891_200),
            "1900-03-01T00:00:00Z",
            "nor was 1900"
        );
        // And the four-hundred-year rule the other way, well past any date this
        // program will see, because the arithmetic either works or it does not.
        assert_eq!(iso8601(13_574_649_600), "2400-03-01T00:00:00Z");
        // Year boundaries either side of midnight.
        assert_eq!(iso8601(1_767_225_599), "2025-12-31T23:59:59Z");
        assert_eq!(iso8601(1_767_225_600), "2026-01-01T00:00:00Z");
        // Before the epoch, which a `u64` would have made impossible to express
        // and a wrong conversion turns into a date in the far future.
        assert_eq!(iso8601(-1), "1969-12-31T23:59:59Z");
    }

    #[test]
    fn a_duration_reads_as_a_clock() {
        assert_eq!(clock(0), "00:00:00");
        assert_eq!(clock(2472), "00:41:12");
        assert_eq!(clock(3600), "01:00:00");
        // Past a day it keeps counting hours rather than starting again, because
        // a bench session that ran overnight is a fact worth seeing.
        assert_eq!(clock(90_000), "25:00:00");
    }

    /// Every field the header promises, from a fixture rather than a live
    /// radio.
    #[test]
    fn the_block_carries_every_field_the_design_lists() {
        let mut m = SdrMetrics::fixture().streaming();
        m.show_section(crate::signal::net::SECTION);
        let lines = block(&m, 1_788_632_561, "sdrtop 0.0.0");
        let joined = lines.join("\n");

        for line in &lines {
            assert!(line.starts_with(COMMENT), "not a comment: {line:?}");
        }
        for field in [
            "sdrtop",
            "exported",
            "device",
            "tuning",
            "mode",
            "addresses",
            "reference",
            "session",
        ] {
            assert!(joined.contains(field), "no {field} line:\n{joined}");
        }
        // The first line names the program that wrote the file, as given.
        assert_eq!(lines[0], format!("{COMMENT} sdrtop 0.0.0"), "{joined}");
        assert!(joined.contains("2026-09-05T18:22:41Z"), "{joined}");
        assert!(joined.contains("HackRF"), "{joined}");
    }

    /// A header written outside NET and FT8 (the API's, in the spectrum's
    /// mode) names the view whose work was done, and says nothing about
    /// addresses or a station it had no use for.
    #[test]
    fn outside_net_and_ft8_the_header_names_the_view() {
        let mut m = SdrMetrics::fixture().streaming();
        let joined = block(&m, 1_788_632_561, "sdrtop 0.0.0").join("\n");
        assert!(joined.contains("# mode         spectrum"), "{joined}");
        m.mode = crate::engine::mode::Mode::named("wifi_survey").unwrap();
        assert!(fields(&m, 0).iter().any(|(n, _)| *n == "addresses"));
        m.mode = crate::engine::mode::Mode::for_view("lab", "lab_signal");
        let fields = fields(&m, 0);
        assert!(fields.contains(&("mode", "lab_signal".to_string())));
        assert!(!fields
            .iter()
            .any(|(n, _)| *n == "addresses" || *n == "station"));
    }

    /// **An FT8 file says FT8**, its band and where its distances are from,
    /// not the NET mode it was not in.
    #[test]
    fn an_ft8_file_names_its_band_and_its_station() {
        let mut m = SdrMetrics::fixture().streaming();
        m.show_section(crate::signal::ft8::SECTION);
        m.radio.frequency = 7_074_000;
        // Alcsútdoboz, where Hatvanpuszta is: a place for a test, not anyone's station.
        m.home.latitude = Some(47.4258);
        m.home.longitude = Some(18.6026);
        m.home.callsign = Some("HA5ZZT".into());
        let joined = block(&m, 1_788_632_561, "sdrtop 0.0.0").join("\n");
        assert!(joined.contains("FT8 / 40 m, dial 7.074 MHz"), "{joined}");
        assert!(joined.contains("JN97hk   HA5ZZT"), "{joined}");
        assert!(
            !joined.contains("NET /") && !joined.contains("addresses"),
            "{joined}"
        );
        m.home.latitude = None;
        let joined = block(&m, 1_788_632_561, "sdrtop 0.0.0").join("\n");
        assert!(joined.contains("no position, so no distances"), "{joined}");
    }

    /// **An unreferenced radio says so.** The reference line is the one most
    /// likely to be read as an absolute claim six months later, so its absence
    /// has to be as loud as its presence.
    #[test]
    fn the_reference_line_says_what_the_ppm_is_worth() {
        let now = std::time::Instant::now();
        let mut m = SdrMetrics::fixture().streaming();

        let none = reference_line(&m, now);
        assert!(none.contains("unreferenced"), "{none}");
        assert!(
            !none.contains("ppm"),
            "no number to attach a unit to: {none}"
        );

        m.radio.reference = Some(crate::state::FrequencyReference {
            ppm: -1.84,
            sigma_ppm: 0.12,
            provenance: crate::state::Provenance::Traceable,
            source: "WWV 10 MHz".to_string(),
            at: now - std::time::Duration::from_secs(843),
            efficiency: None,
            trusted: None,
        });
        let traceable = reference_line(&m, now);
        assert!(traceable.contains("traceable"), "{traceable}");
        assert!(traceable.contains("-1.84"), "{traceable}");
        assert!(traceable.contains("0.12"), "{traceable}");
        assert!(traceable.contains("WWV 10 MHz"), "{traceable}");
        assert!(traceable.contains("00:14:03"), "how old it is: {traceable}");

        // A stale one is worth what no reference is worth, and the file says the
        // same thing the panel does rather than carrying the old figure.
        m.radio.reference.as_mut().unwrap().at =
            now - std::time::Duration::from_secs(crate::state::REFERENCE_STALE_S + 1);
        let stale = reference_line(&m, now);
        assert!(stale.contains("unreferenced"), "{stale}");
        assert!(stale.contains("expired"), "{stale}");
        assert!(!stale.contains("-1.84"), "the dead figure is gone: {stale}");
    }
}
