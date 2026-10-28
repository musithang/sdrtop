// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! What the panel measures, computed once.
//!
//! The blocks draw from the same numbers, and two of them (the verdict and
//! the image bar) need figures the DC block derived. Computing them here rather
//! than in each section is what lets [`super::verdict::decide`] be a pure
//! function of a `Reading` instead of a function of the whole metrics snapshot:
//! the decision then has nothing to do with drawing, and can be tested by
//! writing down a handful of numbers.

use crate::signal::image_rejection_db;
use crate::state::SdrMetrics;

/// The IQ front-end's condition as one set of readings.
///
/// These are the **residual** after any active correction. A lit `[C]` chip with
/// a still-bad reading therefore means the correction is not keeping up, which
/// is what the verdict says in words.
pub(super) struct Reading {
    pub dc_i: f32,
    pub dc_q: f32,
    /// Distance of the I/Q centroid from zero.
    pub dc_mag: f64,
    /// How tall the LO spike is, or `None` when there is no offset at all.
    pub spike_dbfs: Option<f64>,
    /// How far the DC stands over (positive) or under the signal's magnitude
    /// rms (DC excluded), dB.
    /// `None` with no offset at all.
    pub dc_over_signal_db: Option<f64>,
    pub amp_db: f32,
    pub phase_deg: f32,
    pub irr_db: f64,
    /// Where the LO sits, Hz from the centre (`RadioState::lo_offset_hz`).
    /// Zero on a radio that mixes straight to 0 Hz, where the DC above is
    /// the LO's leak; anything else puts the leak somewhere the DC is not.
    pub lo_offset_hz: i64,
    /// The spectrum's level at the LO, dBFS per bin, where the LO is off the
    /// centre and inside the frame; `None` otherwise.
    pub lo_line_dbfs: Option<f64>,
}

impl Reading {
    pub(super) fn of(state: &SdrMetrics) -> Self {
        let dc_i = state.iq.dc_offset_i;
        let dc_q = state.iq.dc_offset_q;
        let dc_mag = (dc_i as f64).hypot(dc_q as f64);
        let spike_dbfs = dc_spike_dbfs(dc_mag);
        let rms_dbfs = state.signal.adc_rms_dbfs as f64;
        Self {
            dc_i,
            dc_q,
            dc_mag,
            spike_dbfs,
            dc_over_signal_db: spike_dbfs.map(|s| s - rms_dbfs),
            amp_db: state.iq.iq_imbalance_db,
            phase_deg: state.iq.phase_imbalance_deg,
            irr_db: image_rejection_db(state.iq.iq_imbalance_db, state.iq.phase_imbalance_deg),
            lo_offset_hz: state.radio.lo_offset_hz,
            lo_line_dbfs: lo_line_dbfs(state),
        }
    }

    /// Where the DC reading sits, in words: the LO on a radio whose LO is at
    /// 0 Hz, plain 0 Hz on one whose LO is not.
    pub(super) fn dc_place(&self) -> &'static str {
        if self.lo_offset_hz == 0 {
            "the LO"
        } else {
            "0 Hz"
        }
    }

    /// IRR as the verdict and the bar both want to print it: capped, because the
    /// formula runs away to hundreds of dB as the imbalance approaches zero and
    /// no front end measures that.
    pub(super) fn irr_text(&self, decimals: usize) -> String {
        if self.irr_db >= 60.0 {
            if decimals == 0 {
                "> 60".to_string()
            } else {
                "> 60 dB".to_string()
            }
        } else {
            format!(
                "{:.*}{}",
                decimals,
                self.irr_db,
                if decimals == 0 { "" } else { " dB" }
            )
        }
    }
}

/// The spectrum's level at an LO off the centre: the highest of the five bins
/// round it in the last FFT frame, dBFS per bin. `None` with the LO at the
/// centre (the DC reading is the LO there), with no frame, or with the LO
/// outside the frame.
fn lo_line_dbfs(state: &SdrMetrics) -> Option<f64> {
    let offset = state.radio.lo_offset_hz;
    if offset == 0 {
        return None;
    }
    let frame = state.waterfall.last_fft.as_ref()?;
    let n = frame.bins_dbfs.len();
    if n == 0 || frame.sample_rate <= 0.0 {
        return None;
    }
    let at = (n as f64 / 2.0 + offset as f64 * n as f64 / frame.sample_rate).round();
    if at < 0.0 || at >= n as f64 {
        return None;
    }
    let at = at as usize;
    let lo = at.saturating_sub(2);
    let hi = (at + 2).min(n - 1);
    frame.bins_dbfs[lo..=hi]
        .iter()
        .copied()
        .fold(None, |best: Option<f32>, v| {
            Some(best.map_or(v, |b| b.max(v)))
        })
        .map(f64::from)
}

/// DC spike level in dBFS: how tall the centre-frequency spike is in the spectrum.
///   DC spike = 20·log₁₀(dc_magnitude)
/// Returns None when dc_mag is zero (no spike).
fn dc_spike_dbfs(dc_mag: f64) -> Option<f64> {
    if dc_mag <= 0.0 {
        return None;
    }
    Some(20.0 * dc_mag.log10())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dc_spike_typical_values() {
        // dc_mag = 0.005 → spike = 20*log10(0.005) ≈ -46 dBFS
        let s = dc_spike_dbfs(0.005).unwrap();
        assert!(
            (s - (-46.0)).abs() < 0.2,
            "expected ~-46 dBFS, got {:.1}",
            s
        );
    }

    #[test]
    fn dc_spike_zero_is_none() {
        assert!(dc_spike_dbfs(0.0).is_none());
    }

    /// The magnitude is the distance from zero, not the sum: an offset of the
    /// same size in both channels is 1.41× one of them, not 2×.
    #[test]
    fn dc_magnitude_is_the_distance_from_centre() {
        let mut m = SdrMetrics::fixture();
        m.iq.dc_offset_i = 0.03;
        m.iq.dc_offset_q = 0.04;
        let r = Reading::of(&m);
        assert!((r.dc_mag - 0.05).abs() < 1e-6, "got {}", r.dc_mag);
    }

    /// The cap exists because the IRR formula diverges as imbalance → 0, and a
    /// panel that printed "312.4 dB" would be reporting arithmetic, not a radio.
    #[test]
    fn a_perfect_front_end_reads_as_capped_not_as_infinity() {
        let mut m = SdrMetrics::fixture();
        m.iq.iq_imbalance_db = 0.0;
        m.iq.phase_imbalance_deg = 0.0;
        let r = Reading::of(&m);
        assert_eq!(r.irr_text(1), "> 60 dB");
        assert_eq!(r.irr_text(0), "> 60");
    }

    #[test]
    fn a_measurable_imbalance_prints_its_value() {
        let mut m = SdrMetrics::fixture();
        m.iq.iq_imbalance_db = -4.2;
        m.iq.phase_imbalance_deg = 7.5;
        let r = Reading::of(&m);
        assert!(r.irr_db < 60.0 && r.irr_db > 0.0, "got {}", r.irr_db);
        assert!(r.irr_text(1).ends_with(" dB"));
        assert!(!r.irr_text(0).contains('.'), "0 decimals means no point");
    }
}
