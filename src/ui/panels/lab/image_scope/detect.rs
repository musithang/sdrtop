// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Where the carrier is, where its mirror is, and how far apart they sit.
//!
//! Pure and deterministic: [`detect_image`] takes a slice of bins and returns the
//! reading, with no theme, no widths and no frame in it. That is what lets the
//! Lab IQ marker bar and `input.rs`'s `[M]` pin call [`carrier_image`] and get the
//! same answer the scope is drawing.

use crate::state::{FftFrame, SdrMetrics};

/// How far above the noise floor (dB) the strongest bin must sit before the auto
/// path treats it as a carrier. Below this there is no signal to measure - only
/// noise - so the scope reports "no carrier" rather than a random noise peak.
/// A placed marker / pin bypasses this gate.
const CARRIER_MIN_SNR_DB: f32 = 10.0;

/// Over the floor, where a bin is taken as part of a signal rather than noise
/// when walking from the carrier to the LO. Lower than the carrier gate: the
/// skirt of a signal is weaker than its peak.
const SIGNAL_OVER_FLOOR_DB: f32 = 6.0;

/// Bins either side looked at together on that walk, so one deep bin in a
/// busy signal's spectrum is not taken for a gap.
const WALK_HALF_WIDTH: usize = 2;

/// A mirror within this of the carrier is more likely a signal of its own
/// than an image: no working front end lets an image through that strong.
const MIRROR_BUSY_DB: f32 = 6.0;

/// A mirror this close to the floor is the floor: the image is somewhere
/// under it.
const AT_FLOOR_DB: f32 = 3.0;

/// What the gap between the carrier and its mirror can honestly be called.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Reading {
    /// The image, measured above the floor: this far under the carrier, dB.
    Measured(f32),
    /// The mirror is the floor: the image is at least this far down, dB.
    AtLeast(f32),
    /// The mirror is within [`MIRROR_BUSY_DB`] of the carrier: too close to
    /// tell from a signal of its own. How far down it is, dB.
    MirrorBusy(f32),
    /// The carrier is part of a signal that runs across the LO, so its
    /// mirror is the signal's own other half and there is no image to see.
    OnTheLo,
}

impl Reading {
    /// The figure that can be printed as image rejection, and whether it is
    /// a lower bound; `None` where there is no image measurement at all.
    pub(crate) fn rejection(self) -> Option<(f32, bool)> {
        match self {
            Reading::Measured(db) => Some((db, false)),
            Reading::AtLeast(db) => Some((db, true)),
            Reading::MirrorBusy(_) | Reading::OnTheLo => None,
        }
    }
}

use super::CarrierImage;

/// Resolve the carrier and its LO-mirror image into absolute frequencies + levels,
/// honouring a placed marker / pin as the carrier (see [`carrier_hint_bin`]).
/// Shared by the scope panel and the marker bar so both tell the same story.
/// `None` when there is no frame yet or it is too small / silent.
pub(crate) fn carrier_image(state: &SdrMetrics) -> Option<CarrierImage> {
    let frame = state.waterfall.last_fft.as_ref()?;
    let hint = carrier_hint_bin(state, frame);
    let lo = lo_bin(state, frame);
    let r = detect_image(
        &frame.bins_dbfs,
        frame.sample_rate,
        frame.noise_floor,
        hint,
        lo,
    )?;
    let center = frame.center_freq_hz as f64;
    Some(CarrierImage {
        carrier_hz: (center + r.carrier_offset_hz).round() as u64,
        image_hz: (center + r.image_offset_hz).round() as u64,
        carrier_dbfs: r.carrier_dbfs,
        image_dbfs: r.image_dbfs,
        reading: r.reading,
    })
}

/// Carrier / image / DC read-out derived from one fftshifted FFT frame.
pub(super) struct ImageReadout {
    /// Which bin the carrier was found in. Nothing draws it - the panel works in
    /// frequencies - but it is the single most important thing `detect_image`
    /// decides, and every test in this module asserts on it. Kept as the
    /// detection's assertion surface, and as where a future cursor would read
    /// from. (This is what the old `let _ = r.carrier_idx;` in `render` was for;
    /// the reason belongs here, not in a no-op statement three screens away.)
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the detection tests read it, and a cursor would")
    )]
    pub carrier_idx: usize,
    pub carrier_dbfs: f32,
    pub image_dbfs: f32,
    pub reading: Reading,
    /// Carrier offset from the frame's centre, signed (Hz).
    pub carrier_offset_hz: f64,
    /// Its mirror's offset from the frame's centre, signed (Hz): the carrier
    /// reflected about the LO, which is the centre on most radios.
    pub image_offset_hz: f64,
}

/// The bin the LO sits in, in an fftshifted frame: the centre, moved by the
/// offset the device reports (`SdrDevice::lo_offset_hz`). The centre when that
/// falls outside the frame, since nothing then mirrors about a bin it has.
pub(super) fn lo_bin(state: &SdrMetrics, frame: &FftFrame) -> usize {
    let n = frame.bins_dbfs.len();
    let lo_hz = (frame.center_freq_hz as i64).saturating_add(state.radio.lo_offset_hz);
    u64::try_from(lo_hz)
        .ok()
        .and_then(|hz| freq_to_bin(hz, frame.center_freq_hz, frame.sample_rate, n))
        .unwrap_or(n / 2)
}

/// Locate the carrier and its mirror about the LO's bin `lo`, and say what
/// the gap between them is ([`Reading`]). The carrier is `carrier_hint` when
/// supplied and valid (a placed marker or pin); otherwise the strongest bin
/// outside a small DC guard, when it clears the noise floor by
/// [`CARRIER_MIN_SNR_DB`]. `None` when the frame is too small / silent. Pure +
/// deterministic for unit testing.
///
/// **Not every mirror is an image.** A station sitting on the LO has its
/// strongest bin just off the DC guard and its mirror on its own other side,
/// and the scope used to print the station's symmetry, a few dB, as the
/// front end's image rejection. A carrier whose signal runs unbroken to the
/// LO is [`Reading::OnTheLo`] and gets no figure.
pub(super) fn detect_image(
    bins: &[f32],
    sample_rate: f64,
    noise_floor: f32,
    carrier_hint: Option<usize>,
    lo: usize,
) -> Option<ImageReadout> {
    let n = bins.len();
    if n < 8 || lo >= n {
        return None;
    }
    let center = lo;
    let guard = (n / 64).max(2);
    let in_band = |i: usize| i < n && (i as isize - center as isize).unsigned_abs() > guard;

    let carrier_idx = match carrier_hint {
        // Honour a hinted carrier (marker / pin) when it lands in a usable band,
        // regardless of strength - the operator may be probing a deliberately
        // weak signal, so explicit intent overrides the noise gate below.
        Some(h) if in_band(h) => h,
        // Auto path: the strongest bin outside the DC guard, but only when it
        // stands clear of the noise floor. With no real carrier present (just
        // noise), the loudest bin is a random noise peak - reporting it as a
        // "carrier" would show a meaningless, alarming image-suppression figure
        // and make the scope's frequency axis jitter frame to frame. Treat that
        // as "no signal" instead.
        _ => {
            let mut idx = center;
            let mut best = f32::NEG_INFINITY;
            for (i, &v) in bins.iter().enumerate() {
                if !in_band(i) {
                    continue;
                }
                if v > best {
                    best = v;
                    idx = i;
                }
            }
            if best < noise_floor + CARRIER_MIN_SNR_DB {
                return None;
            }
            idx
        }
    };

    // A mirror past the frame's edge (an LO well off the centre) is measured
    // at the edge's bin, which then reads as the floor or a bound.
    let image_idx = (2 * center).saturating_sub(carrier_idx).min(n - 1);
    let bin_hz = sample_rate / n as f64;
    let (carrier, image) = (bins[carrier_idx], bins[image_idx]);
    let down = carrier - image;
    let reading = if runs_to_the_lo(bins, carrier_idx, center, guard, noise_floor) {
        Reading::OnTheLo
    } else if image <= noise_floor + AT_FLOOR_DB {
        // The image reads no higher than the bin, which is mostly noise.
        Reading::AtLeast(down)
    } else if down < MIRROR_BUSY_DB {
        Reading::MirrorBusy(down)
    } else {
        Reading::Measured(down)
    };
    Some(ImageReadout {
        carrier_idx,
        carrier_dbfs: carrier,
        image_dbfs: image,
        reading,
        carrier_offset_hz: (carrier_idx as f64 - (n / 2) as f64) * bin_hz,
        image_offset_hz: (image_idx as f64 - (n / 2) as f64) * bin_hz,
    })
}

/// Whether the signal the carrier belongs to runs, with no stretch of noise,
/// from the carrier all the way to the DC guard: every window of bins on the
/// way holds one over the floor by [`SIGNAL_OVER_FLOOR_DB`].
fn runs_to_the_lo(bins: &[f32], carrier: usize, center: usize, guard: usize, floor: f32) -> bool {
    let n = bins.len();
    let path = if carrier > center {
        center + guard + 1..carrier
    } else {
        carrier + 1..center - guard
    };
    path.into_iter().all(|i| {
        let lo = i.saturating_sub(WALK_HALF_WIDTH);
        let hi = (i + WALK_HALF_WIDTH).min(n - 1);
        bins[lo..=hi]
            .iter()
            .any(|&v| v >= floor + SIGNAL_OVER_FLOOR_DB)
    })
}

/// Resolve the carrier bin from operator intent, in priority order:
/// 1. an explicit `[M]` pin, 2. the strongest **placed spectrum marker**, else
///    `None` so [`detect_image`] auto-picks the strongest bin. This is what makes
///    a marker you set on the spectrum actually drive the image calculation.
pub(super) fn carrier_hint_bin(state: &SdrMetrics, frame: &FftFrame) -> Option<usize> {
    let n = frame.bins_dbfs.len();
    let to_bin = |f: u64| freq_to_bin(f, frame.center_freq_hz, frame.sample_rate, n);

    if let Some((carrier_hz, _)) = state.lab.iq_marker_pin {
        if let Some(b) = to_bin(carrier_hz) {
            return Some(b);
        }
    }
    state
        .spectrum
        .markers
        .iter()
        .filter_map(|m| to_bin(m.freq_hz).map(|b| (b, frame.bins_dbfs[b])))
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(b, _)| b)
}

/// Map an absolute frequency to a bin index in the fftshifted frame, or `None` if
/// it falls outside the captured span. Uses the canonical fftshift convention
/// (`bin = n/2 + (f − f_c)·n/rate`, bin `n/2` = DC), the exact inverse of the
/// per-bin frequency the scope and `carrier_offset_hz` use - so a frequency and
/// its bin round-trip without the off-by-one an `(n−1)` mapping introduces.
fn freq_to_bin(freq_hz: u64, center_freq_hz: u64, sample_rate: f64, n: usize) -> Option<usize> {
    if n == 0 || sample_rate <= 0.0 {
        return None;
    }
    let bin_hz = sample_rate / n as f64;
    let b = (n as f64 / 2.0 + (freq_hz as f64 - center_freq_hz as f64) / bin_hz).round();
    if b < 0.0 || b >= n as f64 {
        return None;
    }
    Some(b as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(n: usize) -> Vec<f32> {
        vec![-110.0; n]
    }

    /// The LO's bin in the 64-bin frames below: the centre, as on most radios.
    const LO: usize = 32;

    /// **An LO off the centre is the axis.** The HF+ at a zero-IF rate holds
    /// its LO 5 kHz above the centre: a carrier 8 bins above that LO has its
    /// mirror 8 bins below it, not 8 below the centre, and a carrier mirrored
    /// about the centre would be measured against an empty bin.
    #[test]
    fn the_mirror_is_taken_about_the_lo() {
        let mut b = frame(64);
        let lo = 36;
        b[44] = -8.0; // carrier, +8 bins from the LO
        b[28] = -64.0; // its mirror about the LO
        b[20] = -110.0; // where a mirror about the centre would be: the floor
        let r = detect_image(&b, 64.0, -110.0, None, lo).unwrap();
        assert_eq!(r.carrier_idx, 44);
        assert_eq!(r.reading, Reading::Measured(56.0));
        assert!((r.carrier_offset_hz - 12.0).abs() < 1e-6, "from the centre");
        assert!(
            (r.image_offset_hz - (-4.0)).abs() < 1e-6,
            "{}",
            r.image_offset_hz
        );
        // And the guard sits round the LO: a line on the LO is not a carrier.
        let mut on_lo = frame(64);
        on_lo[36] = -10.0;
        assert!(detect_image(&on_lo, 64.0, -110.0, None, lo).is_none());
    }

    #[test]
    fn detect_image_finds_carrier_and_mirror() {
        let mut b = frame(64);
        b[40] = -8.0; // carrier, +8 bins from centre (32)
        b[24] = -64.0; // its mirror, −8 bins
        b[32] = -20.0; // DC spike
        let r = detect_image(&b, 64.0, -110.0, None, LO).unwrap();
        assert_eq!(r.carrier_idx, 40);
        assert!((r.carrier_dbfs - (-8.0)).abs() < 1e-6);
        assert!((r.image_dbfs - (-64.0)).abs() < 1e-6);
        assert_eq!(r.reading, Reading::Measured(56.0));
        assert!(
            (r.carrier_offset_hz - 8.0).abs() < 1e-6,
            "off {}",
            r.carrier_offset_hz
        );
    }

    #[test]
    fn detect_image_ignores_dc_spike_as_carrier() {
        // A tall DC spike inside the guard band must not be picked as the carrier.
        let mut b = frame(64);
        b[32] = 0.0; // huge DC, at centre
        b[44] = -12.0; // the real carrier
        let r = detect_image(&b, 64.0, -110.0, None, LO).unwrap();
        assert_eq!(r.carrier_idx, 44, "carrier should skip the guarded DC bin");
    }

    #[test]
    fn detect_image_honours_carrier_hint() {
        // A weaker bin chosen by a marker must override the strongest-bin auto-pick.
        let mut b = frame(64);
        b[50] = -4.0; // strongest peak (would auto-win)
        b[40] = -18.0; // the bin the operator marked
        let r = detect_image(&b, 64.0, -110.0, Some(40), LO).unwrap();
        assert_eq!(r.carrier_idx, 40, "hint should drive the carrier");
        assert_eq!((2 * 32usize) - 40, 24);
        assert!((r.carrier_dbfs - (-18.0)).abs() < 1e-6);
    }

    #[test]
    fn detect_image_invalid_hint_falls_back_to_auto() {
        let mut b = frame(64);
        b[50] = -4.0;
        // A hint inside the DC guard is rejected → auto-pick the strongest bin.
        let r = detect_image(&b, 64.0, -110.0, Some(33), LO).unwrap();
        assert_eq!(r.carrier_idx, 50);
        // An out-of-range hint is likewise ignored.
        let r2 = detect_image(&b, 64.0, -110.0, Some(999), LO).unwrap();
        assert_eq!(r2.carrier_idx, 50);
    }

    /// **A signal on the LO has no image to measure**: its strongest bin
    /// just off the DC guard and the mirror on its own other side read a few
    /// dB apart, which used to be printed as the front end's image rejection.
    #[test]
    fn a_signal_running_across_the_lo_is_not_an_image() {
        let mut b = frame(64);
        // An FM station centred on the LO, a little lopsided.
        for (i, v) in b.iter_mut().enumerate().take(41).skip(24) {
            *v = -60.0 - (i as f32 - 32.0).abs();
        }
        b[37] = -50.0;
        let r = detect_image(&b, 64.0, -110.0, None, LO).unwrap();
        assert_eq!(r.carrier_idx, 37);
        assert_eq!(r.reading, Reading::OnTheLo);
        assert!(r.reading.rejection().is_none());
    }

    /// One deep bin inside a busy signal is not a gap that splits it from
    /// the LO; a real stretch of noise is.
    #[test]
    fn a_single_deep_bin_is_not_a_gap_but_noise_is() {
        let mut b = frame(64);
        for v in b.iter_mut().take(44).skip(20) {
            *v = -70.0;
        }
        b[44] = -50.0;
        b[38] = -110.0;
        let r = detect_image(&b, 64.0, -110.0, None, LO).unwrap();
        assert_eq!(r.reading, Reading::OnTheLo);
        for v in b.iter_mut().take(43).skip(36) {
            *v = -110.0;
        }
        let r = detect_image(&b, 64.0, -110.0, None, LO).unwrap();
        assert_ne!(r.reading, Reading::OnTheLo);
    }

    /// A mirror on the floor is a lower bound, not a measurement: the image
    /// is somewhere under it. A mirror within 6 dB is no image figure at all.
    #[test]
    fn a_mirror_on_the_floor_is_a_bound_and_one_close_is_not_an_image() {
        let mut b = frame(64);
        b[50] = -60.0;
        b[14] = -108.0;
        let r = detect_image(&b, 64.0, -110.0, None, LO).unwrap();
        assert_eq!(r.reading, Reading::AtLeast(48.0));
        assert_eq!(r.reading.rejection(), Some((48.0, true)));
        b[14] = -63.0;
        let r = detect_image(&b, 64.0, -110.0, None, LO).unwrap();
        assert_eq!(r.reading, Reading::MirrorBusy(3.0));
        assert!(r.reading.rejection().is_none());
    }

    #[test]
    fn freq_to_bin_maps_endpoints_and_centre() {
        // 64-bin frame, 64 Hz span centred on 1000 Hz → 1 Hz/bin, left edge = 968.
        assert_eq!(freq_to_bin(968, 1000, 64.0, 64), Some(0));
        assert_eq!(freq_to_bin(1000, 1000, 64.0, 64), Some(32)); // centre = n/2
        assert_eq!(freq_to_bin(2000, 1000, 64.0, 64), None); // out of span
    }

    #[test]
    fn detect_image_too_small_is_none() {
        assert!(detect_image(&frame(4), 64.0, -110.0, None, LO).is_none());
    }

    #[test]
    fn detect_image_gates_noise_when_no_carrier() {
        // Only noise: the loudest in-band bin sits a few dB over the floor, below
        // the SNR gate → auto-detection reports no carrier (not a noise peak).
        let mut b = frame(64); // floor -110
        b[40] = -104.0; // a 6 dB noise bump, under the 10 dB gate
        assert!(
            detect_image(&b, 64.0, -110.0, None, LO).is_none(),
            "a sub-gate noise peak must not be reported as a carrier"
        );
        // The same weak bin is still measured when the operator marks it explicitly.
        let r = detect_image(&b, 64.0, -110.0, Some(40), LO).unwrap();
        assert_eq!(
            r.carrier_idx, 40,
            "an explicit hint bypasses the noise gate"
        );
        // A real carrier well above the floor passes the auto gate.
        b[44] = -70.0;
        let r2 = detect_image(&b, 64.0, -110.0, None, LO).unwrap();
        assert_eq!(r2.carrier_idx, 44);
    }
}
