// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

use crate::hardware::DeviceCapabilities;

use super::{CENTER_FREQUENCY_HZ, RTL_SAMPLE_RATE_HZ};

pub fn refusal(caps: &DeviceCapabilities) -> Option<String> {
    if caps.freq_min_hz > CENTER_FREQUENCY_HZ || caps.freq_max_hz < CENTER_FREQUENCY_HZ {
        return Some(format!(
            "ADS-B needs 1090 MHz; this radio tunes {:.3}-{:.3} MHz",
            caps.freq_min_hz as f64 / 1e6,
            caps.freq_max_hz as f64 / 1e6,
        ));
    }
    if caps.sample_rate_min_hz > RTL_SAMPLE_RATE_HZ || caps.sample_rate_max_hz < RTL_SAMPLE_RATE_HZ
    {
        return Some(format!(
            "ADS-B needs 2.4 MS/s; this radio supports {:.3}-{:.3} MS/s",
            caps.sample_rate_min_hz / 1e6,
            caps.sample_rate_max_hz / 1e6,
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rtl_and_hackrf_ranges_admit_1090_mhz_at_2400_ksps() {
        let hackrf = crate::hardware::native::hackrf::caps();
        assert_eq!(refusal(&hackrf), None);
    }

    #[test]
    fn a_radio_that_cannot_tune_1090_mhz_is_refused() {
        let mut caps = crate::hardware::native::hackrf::caps();
        caps.freq_max_hz = 1_000_000_000;
        assert!(refusal(&caps).unwrap().contains("1090 MHz"));
    }

    #[test]
    fn a_radio_without_the_required_sample_rate_is_refused() {
        let mut caps = crate::hardware::native::hackrf::caps();
        caps.sample_rate_min_hz = 3_000_000.0;
        assert!(refusal(&caps).unwrap().contains("2.4 MS/s"));
    }
}
