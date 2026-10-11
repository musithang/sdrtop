// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The 2.4 GHz band's own numbering.
//!
//! **Everyone thinks about this band in channels, not in hertz.** A header that
//! says 2437.000 MHz is correct and useless; one that says channel 6 is what the
//! person in front of it is actually holding in their head. So the conversion
//! lives here, once, and every panel that needs it asks rather than deriving it
//! again with its own rounding.
//!
//! Wi-Fi numbering only, for now. Bluetooth's own channel maps arrive with the
//! arc that needs them, because a header showing "ch 6" for Wi-Fi and "ch 6" for
//! BLE would be naming two different frequencies with one label.

/// The 2.4 GHz ISM band itself, 2400 to 2483.5 MHz.
///
/// **Wi-Fi channel 14 is centred above this**, at 2484 MHz, half a megahertz
/// past the top. That is the plan and not a mistake: the channel is Japan-only
/// and 802.11b-only, and most of its emission falls below the edge. A radio
/// tuned there is off the right end of any rail drawn over the band, which is
/// the honest picture rather than a band stretched to hide it.
///
/// **Not the same as the range the gate requires.** The gate asks whether a
/// radio can reach every centre frequency this section tunes to, and the lowest
/// of those is 2402 MHz, not 2400. These two are the band; those are the
/// requirement. Keeping them separate is what stops a rail drawn over the band
/// from quietly becoming a claim about what a radio must cover.
pub const LOW_HZ: u64 = 2_400_000_000;
pub const HIGH_HZ: u64 = 2_483_500_000;

/// Centre of Wi-Fi channel 1 in the 2.4 GHz band.
///
/// Source: IEEE 802.11-2020, Annex E, Table E-1. Channels 1 to 13 are spaced
/// 5 MHz apart from here; channel 14 is the exception below.
const CHANNEL_1_HZ: u64 = 2_412_000_000;

/// Channel spacing for 1 to 13.
const SPACING_HZ: u64 = 5_000_000;

/// The highest regularly spaced channel. 14 exists but does not follow the
/// spacing: it sits at 2484 MHz, 12 MHz above 13, and is Japan-only and
/// 802.11b-only. It is included because a radio tuned there is on a real
/// channel, and leaving it out would report "no channel" for a legal one. It is
/// also centred above [`HIGH_HZ`]; see that constant.
const LAST_SPACED: u8 = 13;
const CHANNEL_14_HZ: u64 = 2_484_000_000;

/// How far from a centre a tuning still counts as being "on" that channel.
///
/// Half the spacing would tile the band with no gaps and would name a channel
/// for every frequency in it, including the ones exactly between two. A quarter
/// leaves those unnamed, which is the honest answer: a survey stepping across
/// the band is genuinely not on a channel most of the time, and saying so is
/// better than rounding to the nearest one.
const TOLERANCE_HZ: u64 = SPACING_HZ / 4;

/// The Wi-Fi channel this frequency is the centre of, if it is one.
pub fn wifi_channel(freq_hz: u64) -> Option<u8> {
    for n in 1..=LAST_SPACED {
        let centre = CHANNEL_1_HZ + SPACING_HZ * (n as u64 - 1);
        if freq_hz.abs_diff(centre) <= TOLERANCE_HZ {
            return Some(n);
        }
    }
    if freq_hz.abs_diff(CHANNEL_14_HZ) <= TOLERANCE_HZ {
        return Some(14);
    }
    None
}

/// The channels a Wi-Fi view steps through and its survey visits: 1 to 13.
/// Fourteen is a real channel but 802.11b-only and Japan-only, and centred
/// above the band (see [`HIGH_HZ`]), so nothing takes the radio there; a
/// tuning that lands on it is still named by [`wifi_channel`].
pub const WIFI_STEPPED: std::ops::RangeInclusive<u8> = 1..=LAST_SPACED;

/// The centres of [`WIFI_STEPPED`], lowest first.
pub fn wifi_stepped_centres() -> Vec<u64> {
    WIFI_STEPPED.filter_map(wifi_centre_hz).collect()
}

/// The centre frequency of a Wi-Fi channel.
pub fn wifi_centre_hz(channel: u8) -> Option<u64> {
    match channel {
        1..=LAST_SPACED => Some(CHANNEL_1_HZ + SPACING_HZ * (channel as u64 - 1)),
        14 => Some(CHANNEL_14_HZ),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every regularly spaced channel is inside the band, and channel 14 is
    /// not: its centre sits half a megahertz above the nominal top. Written as
    /// an assertion rather than a comment because it is the sort of half
    /// megahertz that gets "corrected" by someone tidying up later.
    #[test]
    fn channel_fourteen_is_centred_above_the_band_and_the_rest_are_not() {
        for n in 1..=LAST_SPACED {
            let hz = wifi_centre_hz(n).unwrap();
            assert!(
                (LOW_HZ..=HIGH_HZ).contains(&hz),
                "channel {n} at {hz} is outside the band"
            );
        }
        assert!(wifi_centre_hz(14).unwrap() > HIGH_HZ);
        assert_eq!(wifi_centre_hz(14).unwrap() - HIGH_HZ, 500_000);
    }

    /// The three anchors of the plan, from the standard: 1, 6 and 11 are the
    /// non-overlapping trio everyone uses, and 13 is the top of the spaced run.
    #[test]
    fn the_channels_sit_where_the_standard_puts_them() {
        assert_eq!(wifi_centre_hz(1), Some(2_412_000_000));
        assert_eq!(wifi_centre_hz(6), Some(2_437_000_000));
        assert_eq!(wifi_centre_hz(11), Some(2_462_000_000));
        assert_eq!(wifi_centre_hz(13), Some(2_472_000_000));
        assert_eq!(wifi_centre_hz(14), Some(2_484_000_000));
        assert_eq!(wifi_centre_hz(0), None);
        assert_eq!(wifi_centre_hz(15), None);
    }

    #[test]
    fn every_channel_round_trips_through_its_centre() {
        for n in 1..=14u8 {
            let hz = wifi_centre_hz(n).unwrap();
            assert_eq!(wifi_channel(hz), Some(n), "channel {n}");
        }
    }

    /// Channel 14 is 12 MHz above 13, not 5, and a plan that assumed the spacing
    /// held would put it at 2477 and name it for a frequency nothing uses.
    #[test]
    fn channel_fourteen_does_not_follow_the_spacing() {
        assert_eq!(
            wifi_centre_hz(14).unwrap() - wifi_centre_hz(13).unwrap(),
            12_000_000
        );
        assert_eq!(wifi_channel(2_477_000_000), None);
    }

    /// Between two channels there is no channel, and saying so is the point.
    #[test]
    fn a_frequency_between_channels_is_not_on_one() {
        // Halfway between 6 and 7.
        assert_eq!(wifi_channel(2_439_500_000), None);
        // Inside the tolerance of 6, which a real tuning rarely misses by more.
        assert_eq!(wifi_channel(2_437_000_000 + 1_000_000), Some(6));
        assert_eq!(wifi_channel(2_437_000_000 - 1_250_000), Some(6));
        assert_eq!(wifi_channel(2_437_000_000 + 1_500_000), None);
        // Outside the band entirely.
        assert_eq!(wifi_channel(100_000_000), None);
        assert_eq!(wifi_channel(5_180_000_000), None);
    }
}
