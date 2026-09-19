// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The header every export begins with.
//!
//! Design section 15.1: **an export carries its provenance or it is worthless.**
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
fn civil_from_days(z: i64) -> (i64, u32, u32) {
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
pub fn block(state: &SdrMetrics, unix_secs: i64) -> Vec<String> {
    let now = std::time::Instant::now();
    let geometry = state.caps.sample_geometry;
    let session = state
        .radio
        .rx_start_time
        .map(|t| clock(now.saturating_duration_since(t).as_secs()))
        .unwrap_or_else(|| "not streaming".to_string());
    let field = |name: &str, value: String| format!("{COMMENT} {name:<12} {value}");

    vec![
        format!("{COMMENT} sdrtop {}", env!("CARGO_PKG_VERSION")),
        field("exported", iso8601(unix_secs)),
        field(
            "device",
            format!(
                "{}  serial {}",
                state.system.board_name, state.system.serial
            ),
        ),
        field(
            "tuning",
            format!(
                "{:.3} MHz   {:.3} Msps   {} bit  fs={}",
                state.radio.frequency as f64 / 1e6,
                state.radio.config_sample_rate / 1e6,
                geometry.bits(),
                geometry.full_scale,
            ),
        ),
        field("mode", format!("NET / {}", state.net.mode.label())),
        // The rows below show addresses in this mode, so the file says which.
        field("addresses", state.net.address_display.label().to_string()),
        field("reference", reference_line(state, now)),
        field("session", session),
    ]
}

/// How the frequency reference is described, or why it is not.
fn reference_line(state: &SdrMetrics, now: std::time::Instant) -> String {
    let Some(r) = state.radio.reference.as_ref() else {
        return "unreferenced, no reference established".to_string();
    };
    // **What it is worth now, not what it was worth when it was taken.** A stale
    // reference exports the way the panel draws it: as no reference at all. The
    // figure is deliberately left out - a number in a file outlives the sentence
    // beside it, and this is the one number most likely to be read later as an
    // absolute claim.
    if r.is_stale(now) {
        return format!(
            "unreferenced, a {} reference against {} expired after {}",
            r.provenance.label().to_lowercase(),
            r.source,
            clock(crate::state::REFERENCE_STALE_S)
        );
    }
    format!(
        "{}, {:.2} ppm +/- {:.2} against {}, measured {} ago",
        r.provenance.label().to_lowercase(),
        r.ppm,
        r.sigma_ppm,
        r.source,
        clock(r.age(now).as_secs())
    )
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

    /// Every field design section 15.1 lists, from a fixture rather than a live
    /// radio.
    #[test]
    fn the_block_carries_every_field_the_design_lists() {
        let m = SdrMetrics::fixture().streaming();
        let lines = block(&m, 1_788_632_561);
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
        assert!(joined.contains(env!("CARGO_PKG_VERSION")), "{joined}");
        assert!(joined.contains("2026-09-05T18:22:41Z"), "{joined}");
        assert!(joined.contains("HackRF"), "{joined}");
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
