// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Classic Bluetooth (BR/EDR)'s own RF channel plan: 79 channels, 1 MHz
//! apart, `f(k) = 2402 + k` MHz for `k` = 0 to 78. Unlike
//! [`crate::signal::ble::channel`], numeric order and frequency order agree
//! here - nothing about BR/EDR's hop sequence, unknown to a passive receiver
//! (design section 1.4), needs any channel pulled out of line the way BLE's
//! three advertising channels are.
//!
//! **Not read from the Bluetooth Core Specification itself this session**
//! (Vol 2, Part B, Section 2, "RF Channels" - design section 6's own
//! facts-to-verify table does not yet name this fact, and should, alongside
//! the ones already there). The same discipline `signal::ble::channel`
//! already carries for its own table: cross-checked against independent,
//! widely reproduced secondary sources (the Bluetooth SIG's own public
//! channel-map explainer, Nordic's and Silicon Labs' BR/EDR primers) that
//! agree with each other on every frequency below, plus the internal
//! consistency this module's own tests assert - 79 channels, 1 MHz apart,
//! covering 2402 to 2480 MHz with no gap and no overlap.
//!
//! **A second thing this module owns that BLE's channel plan does not
//! need:** which of the 79 channels a *capture* can actually see at once,
//! not just which frequency one channel index means. Design section 1.4:
//! "with 20 MHz of capture we see 20 of 79 channels" - [`channels_in_span`]
//! makes that count exact, for [`super::receive`] to build one receiver per
//! channel it actually names, the piece B15 exists to add.

/// The band classic BT's 79 channels cover exactly.
pub const LOW_HZ: u64 = 2_402_000_000;
/// No consumer from `main` yet - `LOW_HZ`'s own pairing, kept for the same
/// reason `signal::ble::channel::HIGH_HZ` is: the band's own upper edge, for
/// whichever future reader needs it stated rather than recomputed.
#[allow(dead_code)]
pub const HIGH_HZ: u64 = 2_480_000_000;

/// Every channel is this far from its neighbour, and (unlike BLE) this is
/// also each channel's own nominal width.
const SPACING_HZ: u64 = 1_000_000;

/// Channels 0 to 78 - one more than BLE's 40, and none of them set aside as
/// BLE's three advertising channels are.
const CHANNEL_COUNT: u8 = 79;

/// How far from a channel's centre a tuning still counts as being on it. An
/// eighth of the spacing rather than BLE's own quarter - these channels sit
/// twice as close together, so the same fractional tolerance would let two
/// neighbours' windows overlap. Only [`channel_of`] uses this; see its own
/// "no consumer yet" note.
#[allow(dead_code)]
const TOLERANCE_HZ: u64 = SPACING_HZ / 8;

/// The centre frequency of classic BT RF channel `k`, 0 to 78.
pub fn centre_hz(channel: u8) -> Option<u64> {
    if channel < CHANNEL_COUNT {
        Some(LOW_HZ + SPACING_HZ * channel as u64)
    } else {
        None
    }
}

/// The channel a frequency is the centre of, if it is one.
///
/// No consumer from `main` yet: [`super::receive::Receiver`] is built from a
/// channel *index* `signal::net::worker` already has from
/// [`channels_in_span`], not from a frequency it would need to look one up
/// from. Kept as [`centre_hz`]'s own inverse and exercised directly by this
/// module's own round-trip test, the same standing
/// `signal::bt::access_code::find_access_code` had after B14.
#[allow(dead_code)]
pub fn channel_of(freq_hz: u64) -> Option<u8> {
    (0..CHANNEL_COUNT).find(|&channel| {
        centre_hz(channel).is_some_and(|centre| centre.abs_diff(freq_hz) <= TOLERANCE_HZ)
    })
}

/// Every channel whose *whole* nominal width sits inside a capture centred on
/// `tuned_centre_hz` and spanning `span_hz` - not merely a channel whose
/// centre happens to land inside it, which would count one the front end's
/// own rolloff has already started to eat into. Ascending by channel index,
/// which on this arc is also ascending by frequency.
///
/// An invalid span (non-finite, zero or negative) answers with no channels
/// rather than guessing at one - the same refusal-not-invention rule 2
/// already holds `signal::net::worker`'s own span calculation to.
pub fn channels_in_span(tuned_centre_hz: f64, span_hz: f64) -> Vec<u8> {
    if !tuned_centre_hz.is_finite() || !span_hz.is_finite() || span_hz <= 0.0 {
        return Vec::new();
    }
    let half_span = span_hz / 2.0;
    let (low, high) = (tuned_centre_hz - half_span, tuned_centre_hz + half_span);
    let half_width = SPACING_HZ as f64 / 2.0;
    (0..CHANNEL_COUNT)
        .filter(|&channel| {
            centre_hz(channel).is_some_and(|centre| {
                let centre = centre as f64;
                centre - half_width >= low && centre + half_width <= high
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The band's own two edges, at the channels the design document and
    /// every cross-checked secondary source name.
    #[test]
    fn channel_zero_and_the_last_channel_sit_at_the_bands_edges() {
        assert_eq!(centre_hz(0), Some(LOW_HZ));
        assert_eq!(centre_hz(78), Some(HIGH_HZ));
    }

    /// B15's own exit condition for this table: every one of the 79 channels
    /// maps to a frequency, and channel 79 and beyond do not exist.
    #[test]
    fn every_one_of_the_seventy_nine_channels_maps_to_a_frequency() {
        for channel in 0..79u8 {
            assert!(
                centre_hz(channel).is_some(),
                "channel {channel} has no frequency"
            );
        }
        assert_eq!(centre_hz(79), None);
        assert_eq!(centre_hz(255), None);
    }

    /// The internal consistency check the module doc promises: 79 distinct
    /// channels, none sharing a frequency, tiling 2402 to 2480 MHz exactly
    /// with 1 MHz steps and no gap - the same shape
    /// `signal::ble::channel::the_forty_channels_tile_the_band_with_no_gap_
    /// and_no_overlap` checks for BLE's own table.
    #[test]
    fn the_seventy_nine_channels_tile_the_band_with_no_gap_and_no_overlap() {
        let mut frequencies: Vec<u64> = (0..79u8).map(|c| centre_hz(c).unwrap()).collect();
        frequencies.sort_unstable();
        frequencies.dedup();
        assert_eq!(frequencies.len(), 79, "two channels share a frequency");
        assert_eq!(*frequencies.first().unwrap(), LOW_HZ);
        assert_eq!(*frequencies.last().unwrap(), HIGH_HZ);
        for pair in frequencies.windows(2) {
            assert_eq!(
                pair[1] - pair[0],
                SPACING_HZ,
                "a gap or an overlap at {pair:?}"
            );
        }
    }

    #[test]
    fn every_channel_round_trips_through_its_centre() {
        for channel in 0..79u8 {
            let hz = centre_hz(channel).unwrap();
            assert_eq!(channel_of(hz), Some(channel), "channel {channel}");
        }
    }

    /// Between two channels there is no channel, the same honesty
    /// `signal::ble::channel::a_frequency_between_channels_is_not_on_one`
    /// asserts for BLE.
    #[test]
    fn a_frequency_between_channels_is_not_on_one() {
        assert_eq!(channel_of(2_402_500_000), None);
        assert_eq!(channel_of(2_350_000_000), None);
        assert_eq!(channel_of(5_180_000_000), None);
    }

    /// A worked example: a 10 MHz capture centred on channel 10 (2412 MHz)
    /// fully contains channels 6 through 14 - the ones whose own half-width
    /// margin still clears the capture's own edges - and no others.
    #[test]
    fn channels_in_span_finds_exactly_the_channels_that_fully_fit() {
        let found = channels_in_span(2_412_000_000.0, 10_000_000.0);
        assert_eq!(found, (6..=14).collect::<Vec<u8>>());
    }

    /// Design section 1.4's own figure: a 20 MHz capture centred mid-band
    /// sees on the order of 20 of the 79 channels, not all of them.
    #[test]
    fn a_twenty_megahertz_capture_sees_about_twenty_channels() {
        let found = channels_in_span(2_441_000_000.0, 20_000_000.0);
        assert!(
            (18..=20).contains(&found.len()),
            "expected about 20 channels, found {}",
            found.len()
        );
    }

    /// A span narrower than one channel's own width finds nothing to fit
    /// wholly inside it, rather than rounding a partial channel up to a
    /// whole one.
    #[test]
    fn a_span_narrower_than_one_channel_finds_none() {
        assert!(channels_in_span(2_440_000_000.0, 500_000.0).is_empty());
    }

    /// An invalid span is refused, not guessed at.
    #[test]
    fn an_invalid_span_finds_no_channels() {
        assert!(channels_in_span(2_440_000_000.0, 0.0).is_empty());
        assert!(channels_in_span(2_440_000_000.0, -5.0).is_empty());
        assert!(channels_in_span(2_440_000_000.0, f64::NAN).is_empty());
        assert!(channels_in_span(f64::INFINITY, 20_000_000.0).is_empty());
    }
}
