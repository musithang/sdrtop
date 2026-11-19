// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! A rough modulation estimate for the carrier at centre, from its occupied
//! bandwidth: a heuristic that says what it is, not a demodulating classifier.

/// A rough modulation estimate for the signal at centre. Honest by design: a
/// bandwidth heuristic (see [`classify`]), not a demodulating classifier. The
/// demod phase refines it (e.g. WFM confirmed by a 19 kHz pilot lock).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Modulation {
    /// No clear carrier to characterize (weak signal), or a shape that does not
    /// fit the known bands.
    #[default]
    Unknown,
    /// Wide-band FM broadcast. Allocated a 200 kHz channel, but see
    /// [`WFM_MIN_BW_HZ`] for what it actually *measures*.
    Wfm,
    /// Narrow-band FM voice / data (~11–30 kHz).
    Nfm,
    /// Amplitude modulation / narrow voice (< 11 kHz).
    Am,
}

impl Modulation {
    /// Short badge label for the banner / headline: `WFM` / `NFM` / `AM` / `—`.
    pub fn label(self) -> &'static str {
        match self {
            Modulation::Wfm => "WFM",
            Modulation::Nfm => "NFM",
            Modulation::Am => "AM",
            Modulation::Unknown => "\u{2014}",
        }
    }

    /// Whether a modulation was confidently classified (not the no-signal fallback).
    pub fn is_known(self) -> bool {
        !matches!(self, Modulation::Unknown)
    }
}

/// Minimum peak-to-noise (dB) for [`classify`] to commit to a modulation. Below
/// this there is no clear carrier at centre, so it reports [`Modulation::Unknown`]
/// rather than labelling noise.
pub const CLASSIFY_MIN_SNR_DB: f32 = 10.0;

/// Occupied bandwidth at or above which the carrier reads as broadcast FM.
///
/// Not the 180 kHz of Carson's rule, and not the 200 kHz channel allocation. The
/// 99 % occupied bandwidth of a real WFM broadcast measures far less than either,
/// because the time-averaged spectrum of FM is strongly peaked at the carrier -
/// 92.8 MHz on a stub antenna measures ~85 kHz, with 91 % of its power inside
/// ±25 kHz. A weaker signal reads narrower still, since its skirts sink under the
/// noise floor before the threshold that bounds the measurement window can see them.
///
/// So the boundary sits well below the observed figure, and still far above narrow
/// FM, which tops out near 16 kHz. There is a factor of five of empty space between
/// the two populations; this is the middle of it.
pub const WFM_MIN_BW_HZ: u64 = 50_000;

/// Occupied bandwidth above which the energy at centre is not one signal at all.
///
/// Carson's rule for broadcast FM with its widest subcarrier, the 92 kHz SCA:
/// 2 x (75 + 92) = 334 kHz. No single broadcast reaches past it, and its 99 %
/// occupied bandwidth sits far inside it (~85 kHz measured). Wider than this,
/// the carrier window has walked through station after station: on the 40 m
/// band a 768 kHz slice of SSB, CW and broadcast read as one 657 kHz "clean
/// WFM signal". That is many signals, and no modulation is named for them.
pub const WFM_MAX_BW_HZ: u64 = 334_000;

/// Occupied bandwidth at or above which the carrier reads as narrow-band FM rather
/// than AM. The honest weak point of the heuristic: narrow FM data bursts and AM
/// voice genuinely overlap here (a 447 MHz data burst measures 6–8 kHz, AM
/// broadcast 9 kHz), and bandwidth alone cannot separate them. `[T]` on the demod
/// bench is the override, and a demodulator that confirms the mode is the real fix.
pub const NFM_MIN_BW_HZ: u64 = 11_000;

/// Estimate the modulation of the signal at centre from its 99% occupied
/// bandwidth, gated on signal presence. Deliberately conservative: a bandwidth
/// heuristic, so the wide/narrow split is trustworthy while the AM vs NFM boundary
/// is a best guess the demod phase can sharpen.
pub fn classify(snr_db: f32, occupied_bw_hz: u64) -> Modulation {
    if snr_db < CLASSIFY_MIN_SNR_DB || occupied_bw_hz == 0 {
        return Modulation::Unknown;
    }
    match occupied_bw_hz {
        bw if bw > WFM_MAX_BW_HZ => Modulation::Unknown,
        bw if bw >= WFM_MIN_BW_HZ => Modulation::Wfm,
        bw if bw >= NFM_MIN_BW_HZ => Modulation::Nfm,
        _ => Modulation::Am,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_gates_on_signal_presence() {
        // Weak carrier or no occupancy → no guess.
        assert_eq!(classify(5.0, 180_000), Modulation::Unknown);
        assert_eq!(classify(40.0, 0), Modulation::Unknown);
    }

    #[test]
    fn classify_bands_by_occupied_bandwidth() {
        assert_eq!(classify(40.0, 180_000), Modulation::Wfm);
        assert_eq!(classify(40.0, WFM_MIN_BW_HZ), Modulation::Wfm); // wide boundary
        assert_eq!(classify(40.0, 15_000), Modulation::Nfm);
        assert_eq!(classify(40.0, NFM_MIN_BW_HZ), Modulation::Nfm); // narrow-FM boundary
        assert_eq!(classify(40.0, 8_000), Modulation::Am);
    }

    /// **Wider than any one broadcast is not one signal**: a band of stations
    /// walked into one window (657 kHz on 40 m) names no modulation; the widest
    /// FM broadcast Carson's rule allows still reads as WFM.
    #[test]
    fn classify_names_no_modulation_for_a_band_of_stations() {
        assert_eq!(classify(53.0, 657_000), Modulation::Unknown);
        assert_eq!(classify(53.0, WFM_MAX_BW_HZ + 1), Modulation::Unknown);
        assert_eq!(classify(53.0, WFM_MAX_BW_HZ), Modulation::Wfm);
    }
}
