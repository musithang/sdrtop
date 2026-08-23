//! The measurement behind the picture: fit a ring to the cloud, then say how
//! tightly the cloud sits on it.
//!
//! The two steps are separate on purpose. [`fit_ellipse`] is pure geometry — the
//! covariance ellipse of the points — and it is what makes amplitude imbalance
//! (`a≠b`) and phase imbalance (a non-zero tilt) legible as a shape.
//! [`cloud_stats`] then measures the cloud *against* that fit, which is why
//! imbalance does not inflate the EVM figure: the ellipse has already absorbed it.

/// Fit a covariance ("RMS") ellipse to the cloud. Returns `(cx, cy, a, b, theta)`:
/// centre, semi-axes and tilt. The semi-axes use `sqrt(2·λ)` so a balanced ring is
/// traced exactly; amplitude imbalance then shows as `a≠b`, phase imbalance as a
/// non-zero tilt. `None` for too few points or a degenerate spread.
pub(super) fn fit_ellipse(coords: &[(f64, f64)]) -> Option<(f64, f64, f64, f64, f64)> {
    let n = coords.len();
    if n < 16 { return None; }
    let nf = n as f64;
    let (mut sx, mut sy) = (0.0, 0.0);
    for &(x, y) in coords { sx += x; sy += y; }
    let (mx, my) = (sx / nf, sy / nf);

    let (mut cxx, mut cyy, mut cxy) = (0.0, 0.0, 0.0);
    for &(x, y) in coords {
        let (dx, dy) = (x - mx, y - my);
        cxx += dx * dx; cyy += dy * dy; cxy += dx * dy;
    }
    cxx /= nf; cyy /= nf; cxy /= nf;

    let tr = cxx + cyy;
    let det = cxx * cyy - cxy * cxy;
    let disc = (tr * tr / 4.0 - det).max(0.0).sqrt();
    let l1 = tr / 2.0 + disc;
    let l2 = (tr / 2.0 - disc).max(0.0);
    if l1 <= 1e-9 { return None; }

    let a = (2.0 * l1).sqrt();
    let b = (2.0 * l2).sqrt();
    let theta = 0.5 * (2.0 * cxy).atan2(cxx - cyy);
    Some((mx, my, a, b, theta))
}

/// Scalar quality read-outs derived from the cloud, for the corner stats box.
/// `evm_*` / `mer_db` / `ecc` / `tilt_deg` are `None` until there are enough
/// points for a stable ellipse fit.
pub(super) struct CloudStats {
    pub n:        usize,
    pub cx:       f64,
    pub cy:       f64,
    pub sigma:    f64,
    pub evm_rms:  Option<f64>,
    pub evm_pk:   Option<f64>,
    pub mer_db:   Option<f64>,
    pub ecc:      Option<f64>,
    pub tilt_deg: Option<f64>,
}

/// Derive the corner stats from the cloud and its fitted ellipse.
///
/// `sigma` is the radial spread (std of point radius about the centroid).
/// `evm_*` is a **scatter-derived proxy**, not symbol-referenced EVM: each point's
/// normalised radius `ρ = sqrt((u/a)² + (v/b)²)` in the fitted-ellipse frame is `1`
/// on the ellipse, so `ρ−1` measures how tightly the cloud hugs its own fitted ring
/// (amplitude/phase imbalance is captured separately by `ecc`/`tilt`, not here).
/// `mer_db = −20·log10(EVM_rms)`.
pub(super) fn cloud_stats(coords: &[(f64, f64)], ellipse: Option<(f64, f64, f64, f64, f64)>) -> CloudStats {
    let n  = coords.len();
    let nf = n.max(1) as f64;
    let (mut sx, mut sy) = (0.0, 0.0);
    for &(x, y) in coords { sx += x; sy += y; }
    let (cx, cy) = (sx / nf, sy / nf);

    let (mut sr, mut sr2) = (0.0, 0.0);
    for &(x, y) in coords {
        let r = (x - cx).hypot(y - cy);
        sr += r; sr2 += r * r;
    }
    let mean_r = sr / nf;
    let sigma  = (sr2 / nf - mean_r * mean_r).max(0.0).sqrt();

    let mut s = CloudStats {
        n, cx, cy, sigma,
        evm_rms: None, evm_pk: None, mer_db: None, ecc: None, tilt_deg: None,
    };

    if let Some((ex, ey, a, b, th)) = ellipse {
        if a > 1e-9 && b > 1e-9 {
            let (ct, st) = (th.cos(), th.sin());
            let (mut acc, mut pk) = (0.0, 0.0f64);
            for &(x, y) in coords {
                let (dx, dy) = (x - ex, y - ey);
                let u =  dx * ct + dy * st;   // rotate into the ellipse's own frame
                let v = -dx * st + dy * ct;
                let rho = ((u / a).powi(2) + (v / b).powi(2)).sqrt();
                let dev = rho - 1.0;
                acc += dev * dev;
                pk = pk.max(dev.abs());
            }
            let evm_rms = (acc / nf).sqrt();
            let mer = if evm_rms > 1e-6 { -20.0 * evm_rms.log10() } else { 60.0 };
            s.evm_rms = Some(evm_rms);
            s.evm_pk  = Some(pk);
            s.mer_db  = Some(mer.min(60.0));
            // Cap the ratio: a near-collinear cloud (b→0, e.g. 90° phase imbalance)
            // would otherwise print a ~1e9 eccentricity.
            s.ecc     = Some((a / b.max(1e-9)).min(99.0));
            let mut tilt = th.to_degrees();
            while tilt >   90.0 { tilt -= 180.0; }
            while tilt <= -90.0 { tilt += 180.0; }
            s.tilt_deg = Some(tilt);
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::tests_support::ring;
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
        assert!((a / b - 2.0).abs() < 0.1, "axis ratio ≈ 2, got {:.2}", a / b);
        assert!(th.abs() < 0.05 || (th.abs() - PI).abs() < 0.05, "tilt ≈ 0 along I");
    }

    #[test]
    fn fit_ellipse_too_few_points_is_none() {
        assert!(fit_ellipse(&ring(8, 1.0, 1.0)).is_none());
        assert!(fit_ellipse(&[]).is_none());
    }

    #[test]
    fn cloud_stats_balanced_ring_is_tight_and_round() {
        let coords = ring(256, 0.5, 0.5);
        let s = cloud_stats(&coords, fit_ellipse(&coords));
        assert_eq!(s.n, 256);
        // Points lie exactly on the fitted ring → near-zero EVM, high MER.
        assert!(s.evm_rms.unwrap() < 0.02, "evm {:?}", s.evm_rms);
        assert!(s.mer_db.unwrap() > 30.0, "mer {:?}", s.mer_db);
        assert!((s.ecc.unwrap() - 1.0).abs() < 0.05, "ecc {:?}", s.ecc);
        // Centroid of a centred ring is ~origin.
        assert!(s.cx.abs() < 1e-3 && s.cy.abs() < 1e-3);
    }

    #[test]
    fn cloud_stats_amplitude_imbalance_does_not_inflate_evm() {
        // A clean 2:1 elliptical ring: ecc≈2 but EVM stays low because the fit
        // captures the ellipse (imbalance is reported by ecc, not EVM).
        let coords = ring(256, 1.0, 0.5);
        let s = cloud_stats(&coords, fit_ellipse(&coords));
        assert!(s.evm_rms.unwrap() < 0.03, "evm {:?}", s.evm_rms);
        assert!((s.ecc.unwrap() - 2.0).abs() < 0.15, "ecc {:?}", s.ecc);
    }

    #[test]
    fn cloud_stats_scatter_raises_evm() {
        // Two concentric rings can't both lie on one ellipse → real radial scatter.
        let mut coords = ring(128, 0.4, 0.4);
        coords.extend(ring(128, 0.6, 0.6));
        let s = cloud_stats(&coords, fit_ellipse(&coords));
        assert!(s.evm_rms.unwrap() > 0.1, "expected scatter, evm {:?}", s.evm_rms);
        assert!(s.mer_db.unwrap() < 25.0, "mer {:?}", s.mer_db);
    }

    #[test]
    fn cloud_stats_too_few_points_has_no_evm() {
        let s = cloud_stats(&ring(8, 0.5, 0.5), None);
        assert!(s.evm_rms.is_none() && s.mer_db.is_none() && s.ecc.is_none());
        assert_eq!(s.n, 8);
    }
}
