// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The front end: the radio's raw stream mixed to the channel and brought
//! down to the working rate every later stage runs at, with the
//! anti-alias filter that decimation needs.

/// Samples per symbol the receiver demodulates at. Not a specification
/// requirement - a PHY's own symbol rate ([`Phy::symbol_rate_hz`]) is fixed,
/// and this is comfortably enough resolution for the matched filter and the
/// discriminator slice both, without decimating a wide capture further than
/// it has to. The same for both uncoded PHYs: `LOOKBACK_SAMPLES`,
/// `HEADER_SEARCH_SYMBOLS` and every other constant counted in symbols below
/// stays correct unchanged when the PHY changes, because it is this figure -
/// not the PHY's own absolute rate - that fixes the conversion between the
/// two units.
use super::*;

pub(super) const WORKING_SPS: usize = 4;

/// The actual working rate a receiver on `phy` runs at, once decimated: one
/// number per PHY, since [`Phy::TwoM`] transmits at twice [`Phy::OneM`]'s
/// symbol rate and is received by the same chain at twice the rate.
pub(super) fn working_rate_hz(phy: Phy) -> f64 {
    phy.symbol_rate_hz() * WORKING_SPS as f64
}

/// The anti-alias filter's passband edge, in Hz: comfortably beyond this
/// PHY's own occupied bandwidth (`Phy::deviation_hz`'s own peak deviation on
/// a BT=0.5 Gaussian-shaped symbol rate) so the matched filter's own
/// waveform correlation is not the thing narrowed, and comfortably inside
/// the working Nyquist ([`working_rate_hz`] / 2) so there is real stopband
/// left before that boundary. See `front_end`'s own doc for why LE 1M's
/// figure, not a narrower one, is the one that was tried first; LE 2M's own
/// is the identical reasoning at twice the numbers, not separately measured
/// against real hardware.
pub(super) fn anti_alias_cutoff_hz(phy: Phy) -> f64 {
    match phy {
        Phy::OneM | Phy::Coded(_) => 1_500_000.0,
        Phy::TwoM => 3_000_000.0,
    }
}
/// Transition width, in Hz, either side of the cutoff.
pub(super) fn anti_alias_transition_hz(phy: Phy) -> f64 {
    match phy {
        Phy::OneM | Phy::Coded(_) => 500_000.0,
        Phy::TwoM => 1_000_000.0,
    }
}
/// Stopband attenuation the transition band settles to. Chosen for real
/// rejection of what a wideband capture actually carries - neighbouring BLE
/// channels, Wi-Fi - without the tap count a much deeper stopband would cost
/// for no measured benefit here. The same figure for both PHYs: it is a
/// property of how much rejection is worth paying for, not of the signal's
/// own bandwidth.
pub(super) const ANTI_ALIAS_STOPBAND_DB: f64 = 40.0;

/// Build the decimator from `raw_rate` to [`working_rate_hz`], or say why it
/// cannot be built.
///
/// Refuses rather than approximating when `raw_rate` is narrower than the
/// working rate, or is not close to a whole multiple of it: a decode running
/// against a rate it silently disagreed with about would scale every
/// deviation and timing figure downstream by exactly the mismatch, with
/// nothing on screen to say so.
///
/// **An anti-alias filter, because plain decimation let the band fold in.**
/// Keeping every `d`th sample and filtering nothing worked on synthetic
/// packets, and on a real HackRF at 20 Msps on channel 37 it produced a
/// flood of detector triggers, several a second, every one failing its CRC
/// with no two decoding to consistent fields: the detector matching noise
/// and out-of-channel energy aliased into the working band.
///
/// **The reference goes through the same filter.** A filtered signal
/// correlated against an unfiltered reference collapses the matched
/// filter's coherence however generous the passband: even a bare 19-tap
/// filter at 0.375 cycles/sample took a clean packet's peak from 0.9998 to
/// 0.22, under the threshold of 0.35. A coherence is an inner product, far
/// less forgiving of a shape mismatch between its two sides than an
/// ordinary demodulator. Filtering the reference through the identical
/// pipeline restored it to 0.99999997; [`matched_reference`] builds it so.
///
/// [`anti_alias_cutoff_hz`] is reasoned rather than swept: wide enough to
/// pass the PHY's own waveform (the tests confirm it for LE 1M) and narrow
/// enough to leave real stopband before the working Nyquist, a defensible
/// number rather than a measured best.
/// `a_strong_out_of_channel_interferer_no_longer_defeats_detection` puts a
/// GFSK signal at an offset that folds onto the passband under decimation
/// and checks detection survives it.
pub fn front_end(raw_rate: f64, phy: Phy) -> Result<StreamingDecimator, String> {
    // LE Coded has a chain of its own (`coded_rx`), with a filter chosen for
    // its sensitivity; this one is LE 1M's and LE 2M's.
    if matches!(phy, Phy::Coded(_)) {
        return Err("LE Coded is received by its own chain, not this one".to_string());
    }
    let rate_hz = working_rate_hz(phy);
    if raw_rate < rate_hz {
        return Err(format!(
            "BLE decode needs at least {:.1} Msps; the radio is at {:.3} Msps",
            rate_hz / 1e6,
            raw_rate / 1e6
        ));
    }
    let d = (raw_rate / rate_hz).round().max(1.0) as usize;
    let achieved = raw_rate / d as f64;
    if (achieved - rate_hz).abs() > rate_hz * 0.01 {
        return Err(format!(
            "BLE decode needs a sample rate near a whole multiple of {:.1} Msps; {:.3} Msps is not one",
            rate_hz / 1e6,
            raw_rate / 1e6
        ));
    }
    let taps = design_lowpass_to_spec(
        anti_alias_cutoff_hz(phy) / raw_rate,
        anti_alias_transition_hz(phy) / raw_rate,
        ANTI_ALIAS_STOPBAND_DB,
    );
    Ok(StreamingDecimator::new(taps, d))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::ble::Phy;

    /// A sample rate below the working rate is refused, not silently
    /// mis-scaled.
    #[test]
    fn a_sample_rate_below_the_working_rate_is_refused() {
        assert!(front_end(2_000_000.0, Phy::OneM).is_err());
    }

    /// A sample rate that decimates cleanly to the working rate is accepted,
    /// at a few realistic HackRF rates.
    #[test]
    fn clean_multiples_of_the_working_rate_are_accepted() {
        for rate in [4_000_000.0, 8_000_000.0, 20_000_000.0] {
            assert!(
                front_end(rate, Phy::OneM).is_ok(),
                "rate {rate} should be accepted"
            );
        }
    }

    /// LE 2M needs twice LE 1M's own minimum sample rate - the same
    /// refusal shape, at the doubled working rate.
    #[test]
    fn le_2m_needs_twice_the_minimum_sample_rate() {
        assert!(front_end(4_000_000.0, Phy::TwoM).is_err());
        assert!(front_end(8_000_000.0, Phy::TwoM).is_ok());
    }
}
