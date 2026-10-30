// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Special functions the closed-form signals need.
//!
//! The error function shapes every Gaussian-filtered frequency pulse: the
//! test transmitters of the NET conformance harness and the FT8 waveform,
//! which FT8 also rebuilds at run time to subtract what it decoded.

use std::f64::consts::PI;

/// The error function, to `f64` rounding.
///
/// Summed as `2/sqrt(pi) * exp(-x^2) * sum_n 2^n x^(2n+1) / (1*3*...*(2n+1))`,
/// a series whose terms are all positive, so nothing cancels, and which stops
/// when a term no longer moves the sum. Past 6 the true value is within
/// `3e-17` of one, below `f64`'s resolution there. Held by a test to its own
/// defining derivative and limits, not to remembered values.
pub fn erf(x: f64) -> f64 {
    if x.is_nan() {
        return f64::NAN;
    }
    if x < 0.0 {
        return -erf(-x);
    }
    if x > 6.0 {
        return 1.0;
    }
    let x2 = x * x;
    let (mut term, mut sum, mut n) = (x, x, 0.0f64);
    loop {
        n += 1.0;
        term *= 2.0 * x2 / (2.0 * n + 1.0);
        sum += term;
        if term <= sum * f64::EPSILON * 0.25 {
            break;
        }
    }
    2.0 / PI.sqrt() * (-x2).exp() * sum
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `erf` is odd, zero at zero, one at infinity, and its slope is
    /// `2/sqrt(pi) exp(-x^2)` everywhere: the definition, which pins it down
    /// without a single remembered value.
    #[test]
    fn erf_is_the_integral_of_the_gaussian() {
        assert_eq!(erf(0.0), 0.0);
        assert_eq!(erf(7.0), 1.0);
        assert!(1.0 - erf(5.99) < 1e-15);
        let d = 1e-5;
        let mut x = -5.0;
        while x < 5.0 {
            assert_eq!(erf(-x), -erf(x));
            let slope = (erf(x + d) - erf(x - d)) / (2.0 * d);
            let exact = 2.0 / PI.sqrt() * (-x * x).exp();
            assert!((slope - exact).abs() < 1e-9, "{x}: {slope} vs {exact}");
            x += 0.01;
        }
    }
}
