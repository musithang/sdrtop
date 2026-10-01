// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

use crate::hardware::DeviceCapabilities;

use super::{CENTER_FREQUENCY_HZ, HIGH_RATE_SAMPLE_RATE_HZ, RTL_SAMPLE_RATE_HZ};

pub fn refusal(caps: &DeviceCapabilities) -> Option<String> {
    if caps.freq_min_hz > CENTER_FREQUENCY_HZ || caps.freq_max_hz < CENTER_FREQUENCY_HZ {
        return Some(format!(
            "ADS-B needs 1090 MHz; this radio tunes {:.3}-{:.3} MHz",
            caps.freq_min_hz as f64 / 1e6,
            caps.freq_max_hz as f64 / 1e6,
        ));
    }

    let supports_rtl_rate =
        (caps.sample_rate_min_hz..=caps.sample_rate_max_hz).contains(&RTL_SAMPLE_RATE_HZ);
    let supports_high_rate =
        (caps.sample_rate_min_hz..=caps.sample_rate_max_hz).contains(&HIGH_RATE_SAMPLE_RATE_HZ);
    if !supports_rtl_rate && !supports_high_rate {
        return Some(format!(
            "ADS-B needs 2.4 MS/s or 6.0 MS/s; this radio supports {:.3}-{:.3} MS/s",
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
    fn a_hackrf_is_admitted() {
        let caps = crate::hardware::native::hackrf::caps();
        assert_eq!(refusal(&caps), None);
    }

    #[test]
    fn a_radio_that_cannot_tune_1090_mhz_is_refused() {
        let mut caps = crate::hardware::native::hackrf::caps();
        caps.freq_max_hz = 1_000_000_000;
        assert!(refusal(&caps).unwrap().contains("1090 MHz"));
    }

    #[test]
    fn a_radio_without_either_supported_rate_is_refused() {
        let mut caps = crate::hardware::native::hackrf::caps();
        caps.sample_rate_min_hz = 3_000_000.0;
        caps.sample_rate_max_hz = 5_000_000.0;
        assert!(refusal(&caps).unwrap().contains("2.4 MS/s or 6.0 MS/s"));
    }
}
