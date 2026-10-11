// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! What is true of every Wi-Fi view before anything is decoded: the one rate
//! they run at.

/// Whether a Wi-Fi view can run at `rate_hz`: `None` when it can, the
/// sentence that says why not when it cannot.
///
/// **One rate, exactly.** A 20 MHz channel is taken whole at one sample per
/// 50 ns, the OFDM symbol's own clock (IEEE 802.11-2024, 17.3.2.4: the
/// subcarrier spacing is 20 MHz / 64), tuned to the channel's centre. A slower
/// rate cannot hold the channel; a faster one could, through a resampler,
/// but none is built yet, and a receiver fed a rate it was not written for
/// would be wrong without saying so. Within a hertz, since a rate is set,
/// not measured.
pub fn rate_refusal(rate_hz: f64) -> Option<String> {
    let need = super::gate::wifi_phy().rate_hz;
    if (rate_hz - need).abs() <= 1.0 {
        return None;
    }
    let tail = if rate_hz > need {
        "; a faster rate is not resampled down yet"
    } else {
        ""
    };
    Some(format!(
        "a Wi-Fi view needs {:.3} Msps, and the radio is at {:.3} (s){tail}",
        need / 1e6,
        rate_hz / 1e6
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Twenty megasamples fit; a hair off, a slow rate and a fast one are
    /// each refused with both numbers and the key that changes it.
    #[test]
    fn only_twenty_msps_runs_a_wifi_view() {
        assert_eq!(rate_refusal(20e6), None);
        for (rate, shown) in [(19.999e6, "19.999"), (8e6, "8.000"), (40e6, "40.000")] {
            let why = rate_refusal(rate).unwrap_or_else(|| panic!("{rate} was accepted"));
            assert!(why.contains("20.000 Msps"), "{why}");
            assert!(why.contains(shown), "{why}");
            assert!(why.contains("(s)"), "{why}");
        }
    }

    /// Above the rate is refused too, and says it is not resampled down
    /// yet rather than leaving the reader to guess why more is not better.
    #[test]
    fn a_faster_rate_says_it_is_not_resampled_yet() {
        let why = rate_refusal(40e6).unwrap();
        assert!(why.contains("not resampled"), "{why}");
    }
}
