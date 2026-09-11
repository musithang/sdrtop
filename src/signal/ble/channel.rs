// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! BLE's own channel numbering: 40 RF channels, 2 MHz apart, indexed 0 to 39
//! in an order that is not the frequency order.
//!
//! **The three advertising channels sit apart from the data channels on
//! purpose.** 37, 38 and 39 are placed at 2402, 2426 and 2480 MHz - the low
//! edge, the middle, and the high edge of the band - specifically so that a
//! Wi-Fi network sitting on channel 1, 6 or 11 cannot cover all three
//! advertising channels at once. The 37 data channels fill in the rest, 0 to
//! 10 from 2404 to 2424 MHz and 11 to 36 from 2428 to 2478 MHz, stepping around
//! the three advertising frequencies rather than through them.
//!
//! Source: Bluetooth Core Specification, Vol 6, Part B, Section 1.4.1 (the
//! data physical channel index to frequency mapping) and Section 2.3.1 (the
//! three advertising channels' fixed placement). **Not checked against a copy
//! of the specification itself in this session** - Rule 1 of `POLICY.md`
//! applies, and this table has instead been cross-checked against three
//! independent secondary sources (Electronics Notes' published channel table,
//! SemFio Networks' and Nordic's own BLE primers) that agree with each other
//! on every frequency below, plus an internal consistency check this module's
//! own tests assert: 40 channels, 2 MHz apart, covering exactly 2402 to
//! 2480 MHz with no gap and no overlap. That agreement is why this table is a
//! constant rather than a `None`-shaped placeholder like
//! [`crate::signal::reference::Standard::tolerance_ppm`] - but the primary
//! source is still owed a real read before anything downstream (whitening,
//! CRC, hop timing) trusts a channel *index*, not just its frequency.

/// The 2.4 GHz ISM band, as BLE actually uses it: 2402 to 2480 MHz, the span
/// the 40 channels below cover exactly. Not [`crate::signal::net::band::LOW_HZ`]
/// and `HIGH_HZ`, which are the wider Wi-Fi framing of the same physical band;
/// BLE's own channel plan does not reach either edge of that wider range.
///
/// No consumer yet outside this module's own tests; a real one arrives with
/// B11's survey mode, the same way `net::band`'s edges feed the occupancy
/// ruler. Still true after B6 wired the rest of this arc into a live
/// capture: detection and decode both work at a single fixed channel, and
/// have no reason to know the band's own edges.
#[allow(dead_code)]
pub const LOW_HZ: u64 = 2_402_000_000;
#[allow(dead_code)]
pub const HIGH_HZ: u64 = 2_480_000_000;

/// Every RF channel is this wide and this far from its neighbour.
const SPACING_HZ: u64 = 2_000_000;

/// The three advertising channels, fixed and out of numeric order.
const ADV_37_HZ: u64 = 2_402_000_000;
const ADV_38_HZ: u64 = 2_426_000_000;
const ADV_39_HZ: u64 = 2_480_000_000;

/// Data channel 0's frequency. Channels 0 to 10 run from here in a straight
/// line, then jump the gap that channel 38 occupies.
const DATA_LOW_START_HZ: u64 = 2_404_000_000;
/// The last data channel before the jump. 0 to this index are contiguous.
const DATA_LOW_LAST: u8 = 10;

/// Data channel 11's frequency, immediately after the gap channel 38 occupies.
/// Channels 11 to 36 run from here to [`HIGH_HZ`] minus one spacing.
const DATA_HIGH_START_HZ: u64 = 2_428_000_000;
/// The first data channel after the jump.
const DATA_HIGH_FIRST: u8 = 11;
/// The last data channel of all. 37 to 39 are the advertising channels.
const DATA_LAST: u8 = 36;

/// How far from a channel's centre a tuning still counts as being on it.
/// A quarter of the spacing, the same fraction `net::band` uses for Wi-Fi and
/// for the same reason: it tiles the band with real gaps between channels
/// rather than rounding every frequency to its nearest neighbour.
const TOLERANCE_HZ: u64 = SPACING_HZ / 4;

/// The centre frequency of a BLE RF channel index, 0 to 39.
///
/// No consumer yet: every later step in this arc, from B3's burst detection
/// onward, needs a channel's frequency to tune to it or a hop event's
/// frequency to name its channel, but B1 lands the table alone.
pub fn centre_hz(channel: u8) -> Option<u64> {
    match channel {
        0..=DATA_LOW_LAST => Some(DATA_LOW_START_HZ + SPACING_HZ * channel as u64),
        DATA_HIGH_FIRST..=DATA_LAST => {
            Some(DATA_HIGH_START_HZ + SPACING_HZ * (channel - DATA_HIGH_FIRST) as u64)
        }
        37 => Some(ADV_37_HZ),
        38 => Some(ADV_38_HZ),
        39 => Some(ADV_39_HZ),
        _ => None,
    }
}

/// The BLE channel index a frequency is the centre of, if it is one.
///
/// No consumer yet, same reason as [`centre_hz`].
pub fn channel_of(freq_hz: u64) -> Option<u8> {
    (0..=39).find(|&channel| {
        centre_hz(channel).is_some_and(|centre| centre.abs_diff(freq_hz) <= TOLERANCE_HZ)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three anchors named in the spec, in the order and at the
    /// frequencies the design document cites.
    #[test]
    fn the_advertising_channels_sit_where_the_standard_puts_them() {
        assert_eq!(centre_hz(37), Some(2_402_000_000));
        assert_eq!(centre_hz(38), Some(2_426_000_000));
        assert_eq!(centre_hz(39), Some(2_480_000_000));
    }

    /// The data channels either side of the jump: 10 is the last of the low
    /// run, 11 is the first of the high run, and they are not 2 MHz apart from
    /// each other the way every other adjacent pair of data channels is -
    /// channel 38's frequency sits in between.
    #[test]
    fn the_data_channels_jump_around_channel_38_frequency() {
        assert_eq!(centre_hz(0), Some(2_404_000_000));
        assert_eq!(centre_hz(10), Some(2_424_000_000));
        assert_eq!(centre_hz(11), Some(2_428_000_000));
        assert_eq!(centre_hz(36), Some(2_478_000_000));
    }

    /// Every one of the 40 channels maps to a frequency, and channel 40 and
    /// beyond do not exist. This is B1's exit condition, written as an
    /// assertion rather than left to the eye.
    #[test]
    fn every_one_of_the_forty_channels_maps_to_a_frequency() {
        for channel in 0..=39u8 {
            assert!(
                centre_hz(channel).is_some(),
                "channel {channel} has no frequency"
            );
        }
        assert_eq!(centre_hz(40), None);
        assert_eq!(centre_hz(255), None);
    }

    /// The internal consistency check the module doc promises: 40 distinct
    /// channels, every one inside the band, none of them sharing a frequency
    /// with another, and the whole set covering the band exactly with 2 MHz
    /// steps and no gap.
    #[test]
    fn the_forty_channels_tile_the_band_with_no_gap_and_no_overlap() {
        let mut frequencies: Vec<u64> = (0..=39u8).map(|c| centre_hz(c).unwrap()).collect();
        frequencies.sort_unstable();
        frequencies.dedup();
        assert_eq!(frequencies.len(), 40, "two channels share a frequency");
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

    /// Every channel round-trips through its own centre.
    #[test]
    fn every_channel_round_trips_through_its_centre() {
        for channel in 0..=39u8 {
            let hz = centre_hz(channel).unwrap();
            assert_eq!(channel_of(hz), Some(channel), "channel {channel}");
        }
    }

    /// Between two channels there is no channel, the same honesty
    /// `net::band::a_frequency_between_channels_is_not_on_one` asserts for
    /// Wi-Fi.
    #[test]
    fn a_frequency_between_channels_is_not_on_one() {
        // Halfway between data channels 0 and 1.
        assert_eq!(channel_of(2_405_000_000), None);
        // Inside the tolerance of channel 0, which a real tuning rarely misses by more.
        assert_eq!(channel_of(2_404_000_000 + 400_000), Some(0));
        assert_eq!(channel_of(2_404_000_000 + 600_000), None);
        // Outside the band entirely.
        assert_eq!(channel_of(2_350_000_000), None);
        assert_eq!(channel_of(5_180_000_000), None);
    }
}
