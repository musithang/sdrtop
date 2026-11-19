// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! How big a decimation filter is: the tap count for a decimate-by-`d`
//! channel filter, and where it stops sharpening.

/// Tap count for a decimate-by-`d` channel filter.
///
/// A Hamming-windowed sinc has a transition width of roughly `3.3 / taps`
/// (normalised to the input rate). Holding that to about a fifth of the channel
/// bandwidth needs ~16.5 × `d` taps. Clamped: below 31 the filter is too soft to
/// reject a neighbouring station, and above 511 the cost stops buying quality -
/// at very high sample rates the filter degrades gracefully, which is why the
/// panel advises dropping the sample rate rather than pretending otherwise.
pub fn tap_count(d: usize) -> usize {
    let n = ((d as f64) * 16.5).round() as usize;
    let n = n.clamp(31, 511);
    n | 1 // odd → a symmetric filter with an exact centre tap
}

/// Decimation factor beyond which [`tap_count`] saturates at its 511-tap cap and
/// the channel filter stops sharpening. Past this the panel advises a lower
/// sample rate rather than silently returning a softer measurement.
pub const FILTER_QUALITY_D_LIMIT: usize = 31;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tap_count_is_odd_and_clamped() {
        assert_eq!(tap_count(1) % 2, 1);
        assert!(tap_count(1) >= 31);
        assert!(tap_count(1000) <= 511);
        for d in [2usize, 8, 40, 80] {
            assert_eq!(tap_count(d) % 2, 1, "d={d} must give an odd tap count");
        }
    }
}
