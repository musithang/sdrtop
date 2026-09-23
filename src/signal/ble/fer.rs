// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The frame error rate against SNR, accumulated over the session
//! (Bluetooth design measurement 16, net-ux-polish-plan 5.7): "given enough
//! traffic from one device this draws the actual waterfall curve of the link".
//!
//! **What is counted.** Every packet the receiver decoded to its full length
//! carries its SNR (read from the sync word, before any payload bit) and its
//! CRC result, so each lands in its SNR bin as good or failed. A trigger the
//! receiver gave up on is not here: it has no packet, no length and no SNR,
//! and the decode-health panel counts it. So this is the CRC failure rate
//! among packets whose length matched, which is what a link's curve is made
//! of; the text that shows it says so.
//!
//! **Counts in the record, rates on the screen.** A curve is two integer
//! arrays, updated inside the state lock with an increment (the `tasks/rx`
//! discipline), and a rate and its uncertainty are computed when read
//! ([`FerCurve::rate`]).
//!
//! **A thin bin is refused, not drawn** (rule 2): below [`MIN_PACKETS`] a
//! bin has no rate, and the reader sees how many packets it did have.
//!
//! **The uncertainty is never zero.** A bin of 400 good packets and none
//! failed has not shown a zero error rate, only a small one. The rate is the
//! plain fraction `failed / n`; its uncertainty is the Agresti-Coull one at
//! one sigma, `sqrt(p~(1 - p~) / (n + 1))` with `p~ = (failed + 1/2) / (n +
//! 1)`, which stays positive at both ends where the textbook `sqrt(p(1 -
//! p) / n)` collapses to a certainty the data does not have.

use crate::signal::dsp::uncertainty::Uncertain;

/// Bins, each [`BIN_DB`] wide from [`LOW_DB`]: 0 to 40 dB in 2 dB steps.
/// A packet below the first edge counts in the first bin and one above the
/// last in the last, so the ends read "below 2 dB" and "38 dB and up".
pub const BINS: usize = 20;
pub const LOW_DB: f64 = 0.0;
pub const BIN_DB: f64 = 2.0;

/// Packets a bin needs before it has a rate. Ten gives a rate a first
/// decimal that is not noise from one packet more or less, without keeping
/// a quiet room's curve empty for minutes.
pub const MIN_PACKETS: u32 = 10;

/// One curve: good and failed packets per SNR bin.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FerCurve {
    pub good: [u32; BINS],
    pub failed: [u32; BINS],
}

/// The bin an SNR falls in, the ends absorbing what is beyond them.
pub fn bin_of(snr_db: f64) -> usize {
    if !snr_db.is_finite() {
        return if snr_db > 0.0 { BINS - 1 } else { 0 };
    }
    (((snr_db - LOW_DB) / BIN_DB).floor().max(0.0) as usize).min(BINS - 1)
}

/// A bin's edges in dB, `(low, high)`: the first open below, the last above.
pub fn edges(bin: usize) -> (Option<f64>, Option<f64>) {
    let low = LOW_DB + bin as f64 * BIN_DB;
    (
        (bin > 0).then_some(low),
        (bin + 1 < BINS).then_some(low + BIN_DB),
    )
}

impl FerCurve {
    /// Count one packet at `snr_db`.
    pub fn record(&mut self, snr_db: f64, crc_ok: bool) {
        let b = bin_of(snr_db);
        let slot = if crc_ok {
            &mut self.good[b]
        } else {
            &mut self.failed[b]
        };
        *slot = slot.saturating_add(1);
    }

    /// Packets in `bin`.
    pub fn packets(&self, bin: usize) -> u32 {
        self.good[bin].saturating_add(self.failed[bin])
    }

    /// Packets in every bin.
    pub fn total(&self) -> u64 {
        (0..BINS).map(|b| self.packets(b) as u64).sum()
    }

    /// The failure fraction in `bin`, `0.0` to `1.0`, with its uncertainty;
    /// `None` below [`MIN_PACKETS`].
    pub fn rate(&self, bin: usize) -> Option<Uncertain> {
        let n = self.packets(bin);
        if n < MIN_PACKETS {
            return None;
        }
        let (x, n) = (self.failed[bin] as f64, n as f64);
        let p_tilde = (x + 0.5) / (n + 1.0);
        let sigma = (p_tilde * (1.0 - p_tilde) / (n + 1.0)).sqrt();
        Some(Uncertain::from_sigma(x / n, sigma))
    }

    /// The bins from the lowest to the highest that holds any packet: the
    /// span a curve is drawn over, empty bins inside it included.
    pub fn span(&self) -> Option<std::ops::RangeInclusive<usize>> {
        let first = (0..BINS).find(|&b| self.packets(b) > 0)?;
        let last = (0..BINS).rev().find(|&b| self.packets(b) > 0)?;
        Some(first..=last)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_packet_lands_in_the_bin_its_snr_names_and_the_ends_absorb() {
        assert_eq!(bin_of(0.0), 0);
        assert_eq!(bin_of(1.99), 0);
        assert_eq!(bin_of(2.0), 1);
        assert_eq!(bin_of(13.7), 6);
        assert_eq!(bin_of(-5.0), 0, "below the first edge");
        assert_eq!(bin_of(55.0), BINS - 1, "above the last");
        assert_eq!(bin_of(f64::NAN), 0);
        assert_eq!(edges(0), (None, Some(2.0)));
        assert_eq!(edges(6), (Some(12.0), Some(14.0)));
        assert_eq!(edges(BINS - 1), (Some(38.0), None));
    }

    /// **The rate is the fraction; its uncertainty never collapses.** 3 of
    /// 30 failed is 10 %; none of 400 is 0 % with a small, positive sigma;
    /// all of 20 is 100 % likewise; and nine packets have no rate at all.
    #[test]
    fn a_rate_and_its_uncertainty_behave_at_both_ends() {
        let mut c = FerCurve::default();
        for i in 0..30 {
            c.record(5.0, i >= 3);
        }
        let r = c.rate(bin_of(5.0)).unwrap();
        assert!((r.value() - 0.10).abs() < 1e-12, "{r:?}");
        assert!(r.sigma() > 0.04 && r.sigma() < 0.07, "{r:?}");

        let mut clean = FerCurve::default();
        for _ in 0..400 {
            clean.record(30.0, true);
        }
        let r = clean.rate(bin_of(30.0)).unwrap();
        assert_eq!(r.value(), 0.0);
        assert!(r.sigma() > 0.0 && r.sigma() < 0.01, "{r:?}");

        let mut lost = FerCurve::default();
        for _ in 0..20 {
            lost.record(1.0, false);
        }
        let r = lost.rate(0).unwrap();
        assert_eq!(r.value(), 1.0);
        assert!(r.sigma() > 0.0, "{r:?}");

        let mut thin = FerCurve::default();
        for _ in 0..9 {
            thin.record(10.0, false);
        }
        assert_eq!(thin.rate(bin_of(10.0)), None);
        assert_eq!(thin.packets(bin_of(10.0)), 9);
    }

    #[test]
    fn the_span_runs_from_the_lowest_to_the_highest_bin_with_packets() {
        let mut c = FerCurve::default();
        assert_eq!(c.span(), None);
        c.record(4.5, true);
        c.record(21.0, false);
        assert_eq!(c.span(), Some(2..=10));
        assert_eq!(c.total(), 2);
    }
}
