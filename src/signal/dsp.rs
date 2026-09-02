// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

#[derive(Clone, Copy, Debug, PartialEq)]
#[allow(dead_code)]
pub enum WindowFn {
    Hann,
    Hamming,
    Blackman,
}

pub fn compute_window(fn_type: WindowFn, size: usize) -> Vec<f32> {
    use std::f64::consts::PI;
    let n = size as f64;
    (0..size)
        .map(|i| {
            let x = 2.0 * PI * i as f64 / (n - 1.0);
            match fn_type {
                WindowFn::Hann => (0.5 * (1.0 - x.cos())) as f32,
                WindowFn::Hamming => (0.54 - 0.46 * x.cos()) as f32,
                WindowFn::Blackman => (0.42 - 0.5 * x.cos() + 0.08 * (2.0 * x).cos()) as f32,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hann_endpoints_are_zero() {
        let w = compute_window(WindowFn::Hann, 1024);
        assert!(w[0].abs() < 1e-6, "first = {}", w[0]);
        assert!(w[1023].abs() < 1e-6, "last = {}", w[1023]);
    }

    #[test]
    fn hann_peak_near_center() {
        let w = compute_window(WindowFn::Hann, 1024);
        let peak_idx = w
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap())
            .map(|(i, _)| i)
            .unwrap();
        assert!((peak_idx as i64 - 511).abs() <= 2, "peak at {}", peak_idx);
    }

    #[test]
    fn hamming_endpoints_nonzero() {
        let w = compute_window(WindowFn::Hamming, 1024);
        assert!(
            w[0] > 0.05,
            "Hamming endpoint should not reach zero, got {}",
            w[0]
        );
    }

    #[test]
    fn blackman_endpoints_near_zero() {
        let w = compute_window(WindowFn::Blackman, 1024);
        assert!(w[0].abs() < 1e-4, "first = {}", w[0]);
        assert!(w[1023].abs() < 1e-4, "last = {}", w[1023]);
    }
}
