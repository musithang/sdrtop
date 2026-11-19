// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The three amplitude zones, and the counts that fall in them.
//!
//! The histogram's 32 bins split into Low / Mid / Clip, and before the split that
//! boundary was written out **five** times as bare `8` and `24` - as slice
//! ranges, as canvas x-comparisons twice, and as an axis-width fraction. Moving
//! one of them and not the others would have coloured a column one zone while the
//! percentage under it counted another.
//!
//! Bin mapping, from the RX callback: bin `i` holds `max(|I|,|Q|)` in
//! `[4i, 4(i+1)) / 128`, so bin 8 is a quarter of full scale and bin 24 is
//! three quarters.

/// First bin of the Mid zone: a quarter of full scale. Below it the ADC is
/// barely being used.
pub(super) const MID_FIRST_BIN: usize = 8;
/// First bin of the Clip zone: three quarters of full scale. At or above it,
/// samples are close enough to the rails to be at risk.
pub(super) const CLIP_FIRST_BIN: usize = 24;
pub(super) const BINS: usize = 32;

/// Bins in each of the two outer zones, which is what the axis row spends its
/// width on. Equal today only because the boundaries happen to be symmetric.
pub(super) const LOW_BINS: usize = MID_FIRST_BIN;
pub(super) const CLIP_BINS: usize = BINS - CLIP_FIRST_BIN;

/// Which zone a bin index belongs to. The one place the boundaries are compared.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Zone {
    Low,
    Mid,
    Clip,
}

pub(super) fn zone_of(bin: usize) -> Zone {
    if bin >= CLIP_FIRST_BIN {
        Zone::Clip
    } else if bin >= MID_FIRST_BIN {
        Zone::Mid
    } else {
        Zone::Low
    }
}

/// The histogram reduced to what the four rows below the chart need.
pub(super) struct Zones {
    pub total: u64,
    pub low: u64,
    pub mid: u64,
    pub clip: u64,
}

impl Zones {
    pub(super) fn of(hist: &[u64; BINS]) -> Self {
        Self {
            total: hist.iter().sum(),
            low: hist[..MID_FIRST_BIN].iter().sum(),
            mid: hist[MID_FIRST_BIN..CLIP_FIRST_BIN].iter().sum(),
            clip: hist[CLIP_FIRST_BIN..].iter().sum(),
        }
    }

    /// Share of samples in a zone, as a whole percent. Zero when nothing has been
    /// counted yet, rather than a division by zero.
    pub(super) fn pct(&self, count: u64) -> u64 {
        (count * 100).checked_div(self.total).unwrap_or(0)
    }
}

/// Per-bin heights for the chart, log-scaled so a bin with a hundredth of the
/// samples is still visible beside the mode.
///
/// `+1` inside the log keeps an empty bin at zero height rather than at −∞.
pub(super) fn heights(hist: &[u64; BINS], n_bins: usize) -> Vec<(f64, f64)> {
    let max_count = hist.iter().copied().max().unwrap_or(1).max(1);
    let log_max = ((max_count + 1) as f64).log2();
    hist.iter()
        .take(n_bins)
        .enumerate()
        .map(|(i, &count)| {
            let h = if log_max > 0.0 {
                ((count + 1) as f64).log2() / log_max
            } else {
                0.0
            };
            (i as f64, h)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_zones_tile_the_histogram_without_gap_or_overlap() {
        let mut seen = [false; BINS];
        let mut counts = (0, 0, 0);
        for (bin, s) in seen.iter_mut().enumerate() {
            *s = true;
            match zone_of(bin) {
                Zone::Low => counts.0 += 1,
                Zone::Mid => counts.1 += 1,
                Zone::Clip => counts.2 += 1,
            }
        }
        assert!(seen.iter().all(|&s| s));
        assert_eq!(
            counts,
            (LOW_BINS, CLIP_FIRST_BIN - MID_FIRST_BIN, CLIP_BINS)
        );
        assert_eq!(counts.0 + counts.1 + counts.2, BINS);
    }

    /// The counts must add up to the total, or the three percentages will not
    /// sum to 100 and the row lies about where the samples are.
    #[test]
    fn the_zone_counts_partition_the_samples() {
        let mut hist = [0u64; BINS];
        for (i, b) in hist.iter_mut().enumerate() {
            *b = i as u64 + 1;
        }
        let z = Zones::of(&hist);
        assert_eq!(z.low + z.mid + z.clip, z.total);
        assert_eq!(z.total, (1..=BINS as u64).sum::<u64>());
    }

    #[test]
    fn an_empty_histogram_is_zero_percent_rather_than_a_division_by_zero() {
        let z = Zones::of(&[0u64; BINS]);
        assert_eq!(z.total, 0);
        assert_eq!(z.pct(z.low), 0);
        assert_eq!(z.pct(0), 0);
    }

    /// The log scale exists so a small bin beside a huge one is still drawn. A
    /// linear scale would render it as nothing.
    #[test]
    fn a_rare_bin_is_still_visible_next_to_the_mode() {
        let mut hist = [0u64; BINS];
        hist[4] = 1_000_000;
        hist[20] = 10;
        let h = heights(&hist, BINS);
        assert!(
            (h[4].1 - 1.0).abs() < 1e-9,
            "the mode should be full height"
        );
        assert!(
            h[20].1 > 0.15,
            "a 10-sample bin beside a million vanished: {}",
            h[20].1
        );
        assert_eq!(h[0].1, 0.0, "an empty bin has no height");
    }

    #[test]
    fn heights_never_exceed_the_canvas() {
        let mut hist = [0u64; BINS];
        for (i, b) in hist.iter_mut().enumerate() {
            *b = (i as u64).pow(3) + 1;
        }
        assert!(heights(&hist, BINS)
            .iter()
            .all(|&(_, h)| (0.0..=1.0).contains(&h)));
    }

    /// A narrow panel draws fewer bins; it must take them from the left rather
    /// than panicking or wrapping.
    #[test]
    fn a_narrow_chart_takes_the_leading_bins() {
        let hist = [7u64; BINS];
        assert_eq!(heights(&hist, 10).len(), 10);
        assert_eq!(heights(&hist, 0).len(), 0);
    }
}
