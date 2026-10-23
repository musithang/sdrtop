// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The covariance ellipse drawn over the cloud: pure geometry, what makes
//! amplitude imbalance (`a≠b`) and phase imbalance (a non-zero tilt) legible
//! as a shape. It is drawn and not read out: the imbalance figures are IQ
//! Diagnostics', measured there from every sample rather than from the 1024
//! drawn here.

/// Fit a covariance ("RMS") ellipse to the cloud. Returns `(cx, cy, a, b, theta)`:
/// centre, semi-axes and tilt. The semi-axes use `sqrt(2·λ)` so a balanced ring is
/// traced exactly; amplitude imbalance then shows as `a≠b`, phase imbalance as a
/// non-zero tilt. `None` for too few points or a degenerate spread.
pub(super) fn fit_ellipse(coords: &[(f64, f64)]) -> Option<(f64, f64, f64, f64, f64)> {
    let n = coords.len();
    if n < 16 {
        return None;
    }
    let nf = n as f64;
    let (mut sx, mut sy) = (0.0, 0.0);
    for &(x, y) in coords {
        sx += x;
        sy += y;
    }
    let (mx, my) = (sx / nf, sy / nf);

    let (mut cxx, mut cyy, mut cxy) = (0.0, 0.0, 0.0);
    for &(x, y) in coords {
        let (dx, dy) = (x - mx, y - my);
        cxx += dx * dx;
        cyy += dy * dy;
        cxy += dx * dy;
    }
    cxx /= nf;
    cyy /= nf;
    cxy /= nf;

    let tr = cxx + cyy;
    let det = cxx * cyy - cxy * cxy;
    let disc = (tr * tr / 4.0 - det).max(0.0).sqrt();
    let l1 = tr / 2.0 + disc;
    let l2 = (tr / 2.0 - disc).max(0.0);
    if l1 <= 1e-9 {
        return None;
    }

    let a = (2.0 * l1).sqrt();
    let b = (2.0 * l2).sqrt();
    let theta = 0.5 * (2.0 * cxy).atan2(cxx - cyy);
    Some((mx, my, a, b, theta))
}

#[cfg(test)]
mod tests {
    use super::super::tests_support::ring;
    use super::*;
    use std::f64::consts::PI;

    #[test]
    fn fit_ellipse_balanced_ring_is_circular() {
        let (_, _, a, b, _) = fit_ellipse(&ring(256, 1.0, 1.0)).unwrap();
        assert!((a - 1.0).abs() < 0.05, "a≈1, got {a:.3}");
        assert!((b - 1.0).abs() < 0.05, "b≈1, got {b:.3}");
        assert!((a - b).abs() < 0.05, "balanced → a≈b");
    }

    #[test]
    fn fit_ellipse_amplitude_imbalance_stretches_axes() {
        // I stretched 2×, Q unchanged → major axis ~2× the minor, tilt ~0.
        let (_, _, a, b, th) = fit_ellipse(&ring(256, 2.0, 1.0)).unwrap();
        assert!(a > b, "major > minor");
        assert!(
            (a / b - 2.0).abs() < 0.1,
            "axis ratio ≈ 2, got {:.2}",
            a / b
        );
        assert!(
            th.abs() < 0.05 || (th.abs() - PI).abs() < 0.05,
            "tilt ≈ 0 along I"
        );
    }

    #[test]
    fn fit_ellipse_too_few_points_is_none() {
        assert!(fit_ellipse(&ring(8, 1.0, 1.0)).is_none());
        assert!(fit_ellipse(&[]).is_none());
    }
}
