// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The envelope: how far from the centre the points sit, `|z|`. The one
//! thing this panel measures that no other does, and the one its picture is
//! best at: a constant-envelope signal is a ring, noise is a disc, a clipped
//! one is pushed against the edge.

/// The cloud's envelope, from the points drawn.
pub(super) struct Envelope {
    /// The magnitude rms, dBFS.
    pub rms_dbfs: f64,
    /// The spread of `|z|` about its mean, as a fraction of the mean: 0 for a
    /// perfect ring, about 0.52 for Gaussian noise (Rayleigh's ratio).
    pub spread: f64,
    /// `|z|` binned from the centre to the outer ring, `bins` wide; points
    /// past the ring land in the last bin.
    pub profile: Vec<u32>,
}

/// Rayleigh's spread, `sqrt(4/π - 1)`: what Gaussian noise reads.
pub(super) const NOISE_SPREAD: f64 = 0.522_7;

impl Envelope {
    /// `None` for an empty cloud, which has no envelope to describe.
    pub(super) fn of(coords: &[(f64, f64)], outer: f64, bins: usize) -> Option<Self> {
        if coords.is_empty() || bins == 0 || outer <= 0.0 {
            return None;
        }
        let n = coords.len() as f64;
        let mags: Vec<f64> = coords.iter().map(|&(i, q)| i.hypot(q)).collect();
        let mean = mags.iter().sum::<f64>() / n;
        let ms = mags.iter().map(|m| m * m).sum::<f64>() / n;
        let var = (ms - mean * mean).max(0.0);
        let mut profile = vec![0u32; bins];
        for &m in &mags {
            profile[((m / outer * bins as f64) as usize).min(bins - 1)] += 1;
        }
        Some(Envelope {
            rms_dbfs: 10.0 * ms.max(1e-24).log10(),
            spread: if mean > 0.0 { var.sqrt() / mean } else { 0.0 },
            profile,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests_support::ring;
    use super::*;

    #[test]
    fn a_ring_has_no_spread_and_one_radius() {
        let e = Envelope::of(&ring(256, 0.55, 0.55), 1.0, 10).unwrap();
        assert!(e.spread < 1e-6, "{}", e.spread);
        assert!((e.rms_dbfs - 20.0 * 0.55f64.log10()).abs() < 1e-9);
        assert_eq!(e.profile[5], 256, "every point at 0.55 of the radius");
    }

    /// Gaussian noise reads Rayleigh's spread, which is the scale the panel
    /// prints beside the figure.
    #[test]
    fn noise_reads_rayleighs_spread() {
        let mut rng = crate::signal::dsp::testkit::Rng::new(3);
        let cloud: Vec<(f64, f64)> = (0..20_000)
            .map(|_| {
                let (a, b) = rng.normal_pair();
                (a * 0.1, b * 0.1)
            })
            .collect();
        let e = Envelope::of(&cloud, 1.0, 10).unwrap();
        assert!((e.spread - NOISE_SPREAD).abs() < 0.02, "{}", e.spread);
    }

    #[test]
    fn past_the_ring_lands_in_the_last_bin_and_empty_is_none() {
        let e = Envelope::of(&[(2.0, 0.0)], 1.0, 4).unwrap();
        assert_eq!(e.profile, vec![0, 0, 0, 1]);
        assert!(Envelope::of(&[], 1.0, 4).is_none());
    }
}
