// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Where FT8 is on the air: the conventional dial frequencies, upper
//! sideband, the transmissions 0 to 3 kHz above.
//!
//! **A convention, not a definition.** The protocol says nothing about
//! frequencies; operators agree on them. The list is the FT8 rows of WSJT-X's
//! default Working Frequencies table (`models/FrequencyList.cpp`,
//! `default_frequency_list`; GPL-3, the same licence as sdrtop), taken as
//! data. 4 m (70.154 MHz) is a Region 1 allocation only; 6 m has a second
//! channel at 50.323 MHz.
//!
//! Nothing here tunes on its own: opening the view retunes nothing, and the
//! band keys offer only what the open radio can reach.

/// A band's name and its FT8 dial frequency, Hz, low to high.
pub const DIALS: [(&str, u64); 13] = [
    ("160 m", 1_840_000),
    ("80 m", 3_573_000),
    ("40 m", 7_074_000),
    ("30 m", 10_136_000),
    ("20 m", 14_074_000),
    ("17 m", 18_100_000),
    ("15 m", 21_074_000),
    ("12 m", 24_915_000),
    ("10 m", 28_074_000),
    ("6 m", 50_313_000),
    ("6 m", 50_323_000),
    ("4 m", 70_154_000),
    ("2 m", 144_174_000),
];

/// The band a dial frequency is FT8's on, if it is one.
pub fn band_of(hz: u64) -> Option<&'static str> {
    DIALS.iter().find(|d| d.1 == hz).map(|d| d.0)
}

/// The next dial above (`up`) or below `from` that `reaches` admits.
pub fn step(from: u64, up: bool, reaches: impl Fn(u64) -> bool) -> Option<(&'static str, u64)> {
    let ok = DIALS.iter().copied().filter(|d| reaches(d.1));
    if up {
        ok.filter(|d| d.1 > from).min_by_key(|d| d.1)
    } else {
        ok.filter(|d| d.1 < from).max_by_key(|d| d.1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The HF+ Discovery's reach: 9 kHz to 31 MHz and 60 to 260 MHz.
    fn hf_plus(hz: u64) -> bool {
        (9_000..=31_000_000).contains(&hz) || (60_000_000..=260_000_000).contains(&hz)
    }

    #[test]
    fn the_list_climbs_and_names_its_bands() {
        assert!(DIALS.windows(2).all(|w| w[0].1 < w[1].1));
        assert_eq!(band_of(14_074_000), Some("20 m"));
        assert_eq!(band_of(14_075_000), None);
    }

    #[test]
    fn stepping_offers_only_what_the_radio_reaches() {
        // Up from 10 m on the HF+: 6 m falls in its gap, 4 m does not.
        assert_eq!(step(28_074_000, true, hf_plus), Some(("4 m", 70_154_000)));
        assert_eq!(step(70_154_000, false, hf_plus), Some(("10 m", 28_074_000)));
        // From a frequency that is no dial, the nearest each way.
        assert_eq!(step(14_200_000, false, hf_plus), Some(("20 m", 14_074_000)));
        assert_eq!(step(14_200_000, true, hf_plus), Some(("17 m", 18_100_000)));
        // The ends stop.
        assert_eq!(step(144_174_000, true, hf_plus), None);
        assert_eq!(step(1_840_000, false, hf_plus), None);
    }
}
