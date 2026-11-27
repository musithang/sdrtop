// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Samples to frames to the two sums per bin.
//!
//! A frame is Hann windowed and transformed, its power per bin added to the
//! sums. Bins are in frequency order from the band's low edge, so bin `k`
//! sits `k · bin_hz` above `centre − rate / 2`. A gap in the stream throws the
//! partial frame away and counts it: frames are independent, so losing one
//! costs depth, never a wrong statistic.

use std::sync::Arc;

use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};

use super::settings::Derived;

/// The two running sums per bin, and the frames they hold.
#[derive(Clone, Debug, Default)]
pub struct Sums {
    /// Power per bin, summed over frames.
    pub s1: Vec<f64>,
    /// Power squared per bin, summed over frames.
    pub s2: Vec<f64>,
    /// Frames summed.
    pub frames: u64,
    /// Partial frames thrown away at a gap.
    pub dropped: u64,
}

impl Sums {
    pub fn new(d: &Derived) -> Sums {
        Sums {
            s1: vec![0.0; d.bins],
            s2: vec![0.0; d.bins],
            frames: 0,
            dropped: 0,
        }
    }

    /// Start again, keeping the size.
    pub fn clear(&mut self) {
        self.s1.iter_mut().for_each(|v| *v = 0.0);
        self.s2.iter_mut().for_each(|v| *v = 0.0);
        self.frames = 0;
        self.dropped = 0;
    }
}

/// The frame under way, and what transforms it.
pub struct Frames {
    window: Vec<f32>,
    fft: Arc<dyn Fft<f32>>,
    held: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
}

impl Frames {
    pub fn new(d: &Derived) -> Frames {
        let n = d.frame_len;
        let tau = std::f64::consts::TAU;
        let fft = FftPlanner::<f32>::new().plan_fft_forward(n);
        Frames {
            window: (0..n)
                .map(|i| (0.5 - 0.5 * (tau * i as f64 / n as f64).cos()) as f32)
                .collect(),
            scratch: vec![Complex::new(0.0, 0.0); fft.get_inplace_scratch_len()],
            fft,
            held: Vec::with_capacity(n),
        }
    }

    /// Throw away the frame under way.
    pub fn clear(&mut self) {
        self.held.clear();
    }

    /// Add `iq` to the frame under way, summing every frame it completes;
    /// the number completed. A gap before `iq` throws away what was held.
    pub fn push(&mut self, iq: &[Complex<f32>], gap_before: bool, sums: &mut Sums) -> usize {
        if gap_before && !self.held.is_empty() {
            self.held.clear();
            sums.dropped += 1;
        }
        let n = self.window.len();
        let mut done = 0;
        let mut rest = iq;
        while !rest.is_empty() {
            let take = (n - self.held.len()).min(rest.len());
            self.held.extend_from_slice(&rest[..take]);
            rest = &rest[take..];
            if self.held.len() == n {
                for (x, w) in self.held.iter_mut().zip(&self.window) {
                    *x *= *w;
                }
                self.fft
                    .process_with_scratch(&mut self.held, &mut self.scratch);
                let half = n / 2;
                for (k, x) in self.held.iter().enumerate() {
                    let bin = (k + n - half) % n;
                    let p = x.norm_sqr() as f64;
                    sums.s1[bin] += p;
                    sums.s2[bin] += p * p;
                }
                sums.frames += 1;
                self.held.clear();
                done += 1;
            }
        }
        done
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::floor::settings::{derive, FloorSetting};
    use crate::signal::floor::testing::{add, noise, tone};

    const RATE: f64 = 192_000.0;

    fn setup() -> (Derived, Frames, Sums) {
        let d = derive(
            &FloorSetting {
                bin_hz: 1.0,
                zoom: None,
            },
            RATE,
        )
        .unwrap();
        let f = Frames::new(&d);
        let s = Sums::new(&d);
        (d, f, s)
    }

    #[test]
    fn a_tone_lands_in_its_bin() {
        let (_, mut f, mut s) = setup();
        let mut phase = 0.0;
        let mut seed = 7;
        let n = 3 * RATE as usize;
        let iq = add(
            tone(RATE, 12_345.0, 0.01, 0, n, &mut phase),
            &noise(n, 0.001, &mut seed),
        );
        assert_eq!(f.push(&iq, false, &mut s), 3);
        let peak = (0..s.s1.len())
            .max_by(|&a, &b| s.s1[a].total_cmp(&s.s1[b]))
            .unwrap();
        assert_eq!(peak, 96_000 + 12_345);
    }

    /// Chance's fingerprint: an exponential's second moment is twice its
    /// mean squared.
    #[test]
    fn noise_power_per_bin_is_exponential() {
        let (d, mut f, mut s) = setup();
        let mut seed = 11;
        for _ in 0..200 {
            f.push(&noise(RATE as usize, 0.01, &mut seed), false, &mut s);
        }
        assert_eq!(s.frames, 200);
        let t = s.frames as f64;
        let mut ratios: Vec<f64> = d
            .usable
            .clone()
            .map(|k| (s.s2[k] / t) / (s.s1[k] / t).powi(2))
            .collect();
        ratios.sort_by(f64::total_cmp);
        let median = ratios[ratios.len() / 2];
        assert!((median - 2.0).abs() < 0.04, "{median}");
    }

    #[test]
    fn a_gap_drops_the_partial_frame_and_counts_it() {
        let (_, mut f, mut s) = setup();
        let mut seed = 3;
        let half = RATE as usize / 2;
        f.push(&noise(3 * half, 0.01, &mut seed), false, &mut s);
        assert_eq!((s.frames, s.dropped), (1, 0));
        // Half a frame is held; a gap throws it away.
        f.push(&noise(2 * half, 0.01, &mut seed), true, &mut s);
        assert_eq!((s.frames, s.dropped), (2, 1));
    }
}
