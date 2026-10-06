// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Which radios can work in this band at all, and which modes their sample rate
//! admits.
//!
//! **A radio that cannot get there does not get a greyed-out section, it gets no
//! section.** That is rule 2 and it is the same decision the RF bench's
//! noise-figure card already makes: absent is correct, and a screen of zeros
//! would be a claim we cannot support. One line in the log says why, once.
//!
//! Everything here branches on [`DeviceCapabilities`] and never on a device
//! name, so a SoapySDR radio nobody here has held is admitted or refused on
//! what its driver declares, exactly like the two native ones.

use crate::hardware::DeviceCapabilities;

/// The lowest centre frequency anything in this section tunes to: Bluetooth LE
/// advertising channel 37, at 2402 MHz.
///
/// The ISM band opens at 2400 MHz, but nothing here ever tunes there, and a gate
/// written against the band edge would refuse a radio that could do every mode
/// on offer.
pub const LOWEST_CENTRE_HZ: u64 = 2_402_000_000;

/// The top of the 2.4 GHz ISM band. Bluetooth LE channel 39 sits at 2480 MHz and
/// Wi-Fi channel 13 at 2472, so a radio that stops below this can still reach
/// some of the band; requiring the whole band is deliberate, because a survey
/// that silently covered three quarters of it would be worse than no survey.
pub const HIGHEST_CENTRE_HZ: u64 = super::band::HIGH_HZ;

/// A physical layer this section could work with, and what it costs to receive.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Phy {
    pub name: &'static str,
    /// The lowest sample rate at which this can honestly be received.
    pub rate_hz: f64,
    /// Occupied bandwidth, so a panel can show what the rate is buying.
    pub occupied_hz: f64,
}

/// Every mode this section could ever offer, cheapest first.
///
/// With complex (IQ) samples the sample rate is the bandwidth seen, so each
/// rate is the lowest that covers the PHY's occupied bandwidth, with room for a
/// filter's skirt where there is any. The 20 Msps entry is the one to look at
/// twice: 802.11a/g/n HT20's active subcarriers span 16.6 MHz, so 20 Msps
/// captures it with 1.7 MHz of skirt either side and no room for a clean
/// anti-aliasing roll-off. It works on strong signals and degrades on weak
/// ones, which is a statable limit rather than a reason to refuse.
pub const PHYS: &[Phy] = &[
    Phy {
        name: "BT BR/EDR",
        rate_hz: 2e6,
        occupied_hz: 1.0e6,
    },
    Phy {
        name: "BLE 1M",
        rate_hz: 2e6,
        occupied_hz: 1.1e6,
    },
    Phy {
        name: "BLE Coded",
        rate_hz: 2e6,
        occupied_hz: 1.1e6,
    },
    Phy {
        name: "BLE 2M",
        rate_hz: 4e6,
        occupied_hz: 2.2e6,
    },
    Phy {
        name: "802.11a/g/n HT20",
        rate_hz: 20e6,
        occupied_hz: 16.6e6,
    },
    Phy {
        name: "802.11b DSSS",
        rate_hz: 22e6,
        occupied_hz: 22e6,
    },
    Phy {
        name: "802.11n HT40",
        rate_hz: 40e6,
        occupied_hz: 36e6,
    },
    Phy {
        name: "802.11ac VHT80",
        rate_hz: 80e6,
        occupied_hz: 78e6,
    },
];

/// The cheapest mode on offer. A radio below this rate can do nothing here.
///
/// Read off [`PHYS`] rather than written down again, so the two cannot disagree
/// about which mode is cheapest when one is added.
pub fn minimum_rate_hz() -> f64 {
    PHYS.iter().fold(f64::INFINITY, |a, p| a.min(p.rate_hz))
}

/// Does the radio's declared tuning range cover the whole band this section
/// works in?
pub fn reaches_band(caps: &DeviceCapabilities) -> bool {
    caps.freq_min_hz <= LOWEST_CENTRE_HZ && caps.freq_max_hz >= HIGHEST_CENTRE_HZ
}

/// Can the radio's sample-rate ceiling receive this mode?
pub fn admits(caps: &DeviceCapabilities, phy: &Phy) -> bool {
    caps.sample_rate_max_hz >= phy.rate_hz
}

/// Whether the NET section appears, and if not, the one line that says why.
///
/// The sentence is written here rather than at the call site because the reason
/// and the decision must not be able to disagree: a section that vanished for
/// one reason while the log gave another would be worse than silence.
pub fn verdict(caps: &DeviceCapabilities) -> Result<(), String> {
    if !reaches_band(caps) {
        return Err(format!(
            "NET section hidden: this radio tunes {:.3} to {:.3} MHz, and the 2.4 GHz band needs {:.3} to {:.3}",
            caps.freq_min_hz as f64 / 1e6,
            caps.freq_max_hz as f64 / 1e6,
            LOWEST_CENTRE_HZ as f64 / 1e6,
            HIGHEST_CENTRE_HZ as f64 / 1e6,
        ));
    }
    let minimum = minimum_rate_hz();
    if caps.sample_rate_max_hz < minimum {
        return Err(format!(
            "NET section hidden: this radio reaches {:.3} Msps, and the cheapest mode here needs {:.3}",
            caps.sample_rate_max_hz / 1e6,
            minimum / 1e6,
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A capability record with the three fields this gate reads overridden.
    ///
    /// Built from the HackRF's own declaration rather than written out, so it
    /// stays a real `DeviceCapabilities` as the struct grows, and so the only
    /// numbers in this file are the ones under test.
    fn caps_with(
        freq_min_hz: u64,
        freq_max_hz: u64,
        sample_rate_max_hz: f64,
    ) -> DeviceCapabilities {
        let mut c = crate::hardware::native::hackrf::caps();
        c.freq_min_hz = freq_min_hz;
        c.freq_max_hz = freq_max_hz;
        c.sample_rate_max_hz = sample_rate_max_hz;
        c
    }

    /// Read off the backend, not copied from a table. If the HackRF's
    /// declared range ever changes, this is where it is noticed.
    #[test]
    fn the_hackrf_is_admitted_on_its_own_declaration() {
        let caps = crate::hardware::native::hackrf::caps();
        assert!(verdict(&caps).is_ok());
        assert!(reaches_band(&caps));
    }

    /// The RTL-SDR ranges are asserted against the driver in
    /// `hardware::native::rtlsdr`'s own tests; repeated here only as the input
    /// to this gate.
    #[test]
    fn neither_rtl_sdr_tuner_reaches_the_band() {
        let r820t = caps_with(24_000_000, 1_766_000_000, 3.2e6);
        let e4000 = caps_with(52_000_000, 2_200_000_000, 3.2e6);
        assert!(verdict(&r820t).is_err());
        assert!(verdict(&e4000).is_err());
    }

    /// The boundary is where a gate is worth testing. One hertz either side.
    #[test]
    fn the_band_edges_are_exact() {
        assert!(verdict(&caps_with(LOWEST_CENTRE_HZ, HIGHEST_CENTRE_HZ, 20e6)).is_ok());
        assert!(verdict(&caps_with(LOWEST_CENTRE_HZ + 1, HIGHEST_CENTRE_HZ, 20e6)).is_err());
        assert!(verdict(&caps_with(LOWEST_CENTRE_HZ, HIGHEST_CENTRE_HZ - 1, 20e6)).is_err());
    }

    #[test]
    fn a_radio_below_the_cheapest_mode_is_refused_by_rate_and_says_so() {
        let why = caps_with(1_000_000, 6_000_000_000, 1.5e6);
        let why = verdict(&why).unwrap_err();
        assert!(why.contains("1.500 Msps"), "{why}");
        assert!(why.contains("2.000"), "{why}");
        // Exactly at the minimum is in, because a minimum is a minimum.
        assert!(verdict(&caps_with(1_000_000, 6_000_000_000, minimum_rate_hz())).is_ok());
    }

    #[test]
    fn the_refusal_names_the_range_it_refused_for() {
        let why = verdict(&caps_with(24_000_000, 1_766_000_000, 3.2e6)).unwrap_err();
        assert!(why.contains("24.000"), "{why}");
        assert!(why.contains("1766.000"), "{why}");
        assert!(
            why.contains("2402.000") && why.contains("2483.500"),
            "{why}"
        );
    }

    #[test]
    fn the_sample_rate_decides_which_modes_are_on_offer() {
        let hackrf = crate::hardware::native::hackrf::caps();
        let admitted: Vec<&str> = PHYS
            .iter()
            .filter(|p| admits(&hackrf, p))
            .map(|p| p.name)
            .collect();
        // The HackRF's 20 Msps ceiling is the shape of the whole feature: all of
        // Bluetooth, HT20 exactly at the limit, and nothing wider.
        assert!(admitted.contains(&"BLE 2M"));
        assert!(admitted.contains(&"802.11a/g/n HT20"));
        assert!(!admitted.contains(&"802.11b DSSS"));
        assert!(!admitted.contains(&"802.11n HT40"));

        // A 61.44 Msps Soapy device lifts everything below VHT80.
        let pluto = caps_with(70_000_000, 6_000_000_000, 61.44e6);
        assert_eq!(
            PHYS.iter().filter(|p| admits(&pluto, p)).count(),
            PHYS.len() - 1
        );
    }

    #[test]
    fn the_cheapest_mode_is_read_off_the_table_and_not_written_twice() {
        assert_eq!(
            minimum_rate_hz(),
            PHYS.iter().map(|p| p.rate_hz).fold(f64::INFINITY, f64::min)
        );
        assert_eq!(minimum_rate_hz(), 2e6);
    }
}
