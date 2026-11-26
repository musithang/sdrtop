// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! A carrier's shift against the first's, with our oscillator's error gone.
//!
//! Each carrier at `fᵢ` is seen shifted by `Δfᵢ = Dᵢ − fᵢ·e`: its path's
//! Doppler `Dᵢ`, less our oscillator's fractional error `e` scaled to its
//! frequency (the sign `signal::reference` sets out). So
//! `Δfᵢ − (fᵢ/f1)·Δf1 = Dᵢ − (fᵢ/f1)·D1` has no `e` in it: the one figure here
//! offered as free of our oscillator. Its variance is `σᵢ² + (fᵢ/f1)²·σ1²`,
//! the two estimates being independent.

use super::settings::CarrierSetting;
use super::track::Line;

/// The shift of the carrier at `fi_hz` against the first's at `f1_hz`, or why
/// there is none: the reason of whichever line was refused, named.
pub fn differential(
    f1_hz: u64,
    first: &Result<Line, String>,
    fi_hz: u64,
    other: &Result<Line, String>,
) -> Result<Line, String> {
    let a = first
        .as_ref()
        .map_err(|why| format!("first carrier: {why}"))?;
    let b = other
        .as_ref()
        .map_err(|why| format!("{:.3} MHz: {why}", fi_hz as f64 / 1e6))?;
    let k = fi_hz as f64 / f1_hz as f64;
    Ok(Line {
        hz: b.hz - k * a.hz,
        sigma_hz: (b.sigma_hz.powi(2) + (k * a.sigma_hz).powi(2)).sqrt(),
    })
}

/// Whether these carriers can be differenced: their windows must end
/// together, which carriers of one resolution do.
pub fn pair(settings: &[CarrierSetting]) -> Result<(), String> {
    match settings.split_first() {
        Some((first, rest)) if rest.iter().any(|s| s.resolution_hz != first.resolution_hz) => Err(format!(
            "carriers differ in resolution ({}), so their windows do not end together: no differential",
            settings.iter().map(|s| format!("{} Hz", s.resolution_hz)).collect::<Vec<_>>().join(", ")
        )),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::doppler::{CarrierSetting, Line};

    const F1: u64 = 9_996_000;
    const F2: u64 = 10_000_000;

    /// **Our oscillator cancels.** Two paths' true shifts, our error applied
    /// to both as the radio applies it, and the differential is the paths'
    /// alone.
    #[test]
    fn our_oscillator_cancels() {
        let (d1, d2, e) = (0.30, -0.10, 0.1e-6);
        let seen = |d: f64, f: u64| {
            Ok(Line {
                hz: d - f as f64 * e,
                sigma_hz: 0.003,
            })
        };
        let got = differential(F1, &seen(d1, F1), F2, &seen(d2, F2)).unwrap();
        let want = d2 - (F2 as f64 / F1 as f64) * d1;
        assert!((got.hz - want).abs() < 1e-9, "{} vs {want}", got.hz);
    }

    #[test]
    fn its_sigma_combines_both() {
        let a = Ok(Line {
            hz: 0.0,
            sigma_hz: 0.003,
        });
        let b = Ok(Line {
            hz: 0.0,
            sigma_hz: 0.004,
        });
        let got = differential(F1, &a, F2, &b).unwrap();
        let k = F2 as f64 / F1 as f64;
        assert!((got.sigma_hz - (0.004f64.powi(2) + (k * 0.003).powi(2)).sqrt()).abs() < 1e-12);
    }

    #[test]
    fn a_refused_line_refuses_the_differential_with_its_reason() {
        let refused = Err("no line above the noise".to_string());
        let ok = Ok(Line {
            hz: 0.0,
            sigma_hz: 0.004,
        });
        let why = differential(F1, &refused, F2, &ok).unwrap_err();
        assert!(why.contains("first carrier: no line above"), "{why}");
        let why = differential(F1, &ok, F2, &refused).unwrap_err();
        assert!(why.contains("10.000 MHz: no line above"), "{why}");
    }

    #[test]
    fn carriers_differ_in_resolution() {
        let s = |hz, resolution_hz| CarrierSetting {
            hz,
            span_hz: 25.0,
            resolution_hz,
        };
        assert_eq!(pair(&[s(F1, 0.1), s(F2, 0.1)]), Ok(()));
        let why = pair(&[s(F1, 0.1), s(F2, 0.2)]).unwrap_err();
        assert!(why.contains("carriers differ in resolution"), "{why}");
    }
}
