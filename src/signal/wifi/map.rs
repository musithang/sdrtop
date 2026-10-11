// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Coded bits to constellation points, and back.
//!
//! Gray-coded on each axis (17.3.5.8, Figure 17-10, Tables 17-15 to 17-18):
//! a point's bits split in two, the first half naming the I level and the
//! second the Q level, the first bit of each half the most significant.
//! BPSK puts its one bit on I. Every constellation is scaled by K_MOD to
//! unit average power (Table 17-14).

use num_complex::Complex;

/// K_MOD for `n_bpsc` bits a point (Table 17-14).
fn k_mod(n_bpsc: usize) -> f64 {
    match n_bpsc {
        1 => 1.0,
        2 => 1.0 / 2f64.sqrt(),
        4 => 1.0 / 10f64.sqrt(),
        6 => 1.0 / 42f64.sqrt(),
        _ => panic!("no constellation of {n_bpsc} bits"),
    }
}

/// One axis's level from its Gray bits, most significant first: -1 and +1
/// for one bit, -3 to +3 for two, -7 to +7 for three.
fn axis(bits: &[bool]) -> f64 {
    let n = bits.len();
    // Gray to binary, the first bit most significant.
    let mut value = 0usize;
    let mut bit = false;
    for &g in bits {
        bit ^= g;
        value = value << 1 | bit as usize;
    }
    (2 * value) as f64 - ((1 << n) - 1) as f64
}

/// The nearest level's Gray bits on one axis of `n` bits.
fn axis_bits(level: f64, n: usize) -> Vec<bool> {
    let top = (1usize << n) - 1;
    let value = ((level + top as f64) / 2.0).round().clamp(0.0, top as f64) as usize;
    let gray = value ^ (value >> 1);
    (0..n).rev().map(|i| gray >> i & 1 == 1).collect()
}

/// Coded bits to points, `n_bpsc` bits each.
pub fn map(bits: &[bool], n_bpsc: usize) -> Vec<Complex<f64>> {
    let k = k_mod(n_bpsc);
    bits.chunks(n_bpsc)
        .map(|b| {
            if n_bpsc == 1 {
                Complex::new(axis(b) * k, 0.0)
            } else {
                let (i, q) = b.split_at(n_bpsc / 2);
                Complex::new(axis(i) * k, axis(q) * k)
            }
        })
        .collect()
}

/// Points to the bits of the nearest constellation point: a hard decision.
pub fn demap(points: &[Complex<f64>], n_bpsc: usize) -> Vec<bool> {
    let k = k_mod(n_bpsc);
    points
        .iter()
        .flat_map(|p| {
            if n_bpsc == 1 {
                axis_bits(p.re / k, 1)
            } else {
                let mut b = axis_bits(p.re / k, n_bpsc / 2);
                b.extend(axis_bits(p.im / k, n_bpsc / 2));
                b
            }
        })
        .collect()
}

/// Points to one reading a bit, positive for a 1: for each bit, the
/// squared distance to the nearest point where it is 0 less that to the
/// nearest where it is 1 (the max-log approximation of its likelihood
/// ratio, up to a scale every reading shares). What the decoder weighs
/// instead of a hard decision.
pub fn demap_soft(points: &[Complex<f64>], n_bpsc: usize) -> Vec<f32> {
    let constellation: Vec<(Vec<bool>, Complex<f64>)> = (0..1usize << n_bpsc)
        .map(|v| {
            let bits: Vec<bool> = (0..n_bpsc).map(|i| v >> i & 1 == 1).collect();
            let p = map(&bits, n_bpsc)[0];
            (bits, p)
        })
        .collect();
    let mut out = Vec::with_capacity(points.len() * n_bpsc);
    for y in points {
        for i in 0..n_bpsc {
            let (mut zero, mut one) = (f64::INFINITY, f64::INFINITY);
            for (bits, p) in &constellation {
                let d = (y - p).norm_sqr();
                if bits[i] {
                    one = one.min(d);
                } else {
                    zero = zero.min(d);
                }
            }
            out.push((zero - one) as f32);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use num_complex::Complex;

    /// The annex's own example (I.1.6.3): the first four DATA bits, 0111,
    /// are the 16-QAM point -0.316 + 0.316j.
    #[test]
    fn zero_one_one_one_is_the_annexes_point() {
        let p = map(&[false, true, true, true], 4)[0];
        assert!((p - Complex::new(-0.316, 0.316)).norm() < 0.0005, "{p}");
    }

    /// Every constellation has unit average power (K_MOD, Table 17-14).
    #[test]
    fn every_constellation_has_unit_power() {
        for n_bpsc in [1, 2, 4, 6] {
            let points: Vec<Complex<f64>> = (0..1usize << n_bpsc)
                .map(|v| {
                    let bits: Vec<bool> = (0..n_bpsc).map(|i| v >> i & 1 == 1).collect();
                    map(&bits, n_bpsc)[0]
                })
                .collect();
            let power = points.iter().map(|p| p.norm_sqr()).sum::<f64>() / points.len() as f64;
            assert!((power - 1.0).abs() < 1e-12, "{n_bpsc}: {power}");
        }
    }

    /// Neighbouring levels differ in one bit (Gray), on every axis.
    #[test]
    fn neighbouring_levels_differ_in_one_bit() {
        for n in [1usize, 2, 3] {
            let mut levels: Vec<(f64, usize)> = (0..1usize << n)
                .map(|v| {
                    let bits: Vec<bool> = (0..n).map(|i| v >> i & 1 == 1).collect();
                    (axis(&bits), v)
                })
                .collect();
            levels.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
            for w in levels.windows(2) {
                assert_eq!((w[0].1 ^ w[1].1).count_ones(), 1, "{n}: {w:?}");
            }
        }
    }

    /// Soft readings carry each bit's sign and grow with its distance from
    /// the decision boundary: on a clean point every reading agrees with
    /// its bit, and a point nudged towards the boundary reads fainter.
    #[test]
    fn soft_readings_agree_with_the_bits_and_weaken_near_a_boundary() {
        for n_bpsc in [1, 2, 4, 6] {
            let bits: Vec<bool> = (0..n_bpsc * 30).map(|i| (i * 3 + i / 4) % 5 < 2).collect();
            let r = demap_soft(&map(&bits, n_bpsc), n_bpsc);
            assert_eq!(r.len(), bits.len());
            for (i, (&v, &b)) in r.iter().zip(&bits).enumerate() {
                assert_eq!(v > 0.0, b, "{n_bpsc}: bit {i} read {v}");
            }
        }
        let firm = demap_soft(&[Complex::new(1.0, 0.0)], 1)[0];
        let faint = demap_soft(&[Complex::new(0.2, 0.0)], 1)[0];
        assert!(firm > faint && faint > 0.0, "{firm} {faint}");
    }

    /// The nearest point's bits are the bits that made it, at every size.
    #[test]
    fn demapping_returns_the_bits() {
        for n_bpsc in [1, 2, 4, 6] {
            let bits: Vec<bool> = (0..n_bpsc * 40).map(|i| (i * 5 + i / 7) % 3 == 0).collect();
            assert_eq!(demap(&map(&bits, n_bpsc), n_bpsc), bits, "{n_bpsc}");
        }
    }
}
