// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The radio's stream to FT8's 12 000 samples a second, continuously across
//! blocks.
//!
//! **The dial is the tuned frequency.** FT8 lives 0 to 3 kHz above the dial,
//! in the upper sideband; in complex baseband that is simply 0 to +3 kHz, so
//! nothing is mixed and the frequency on screen is the one being read. The
//! radio's DC sits at 0 Hz, below the search's lowest tone, and a slow mean is
//! taken off as the pager's channel does.
//!
//! **Exactly 12 000, not near it.** The decoder's numbers are counted in
//! samples (1920 a symbol), so the rate is reached exactly: by integer
//! decimation alone where the radio's rate is a multiple of 12 000 (the HF+'s
//! 192 kS/s, 2.4 MS/s), otherwise by integer decimation to an intermediate
//! rate of 24 to 64 kS/s and a rational resampler from there (20 MS/s goes by
//! 625 to 32 kS/s, then by 3/8). A rate that offers neither is refused, not
//! approximated.
//!
//! **What must not fold in.** At 12 kS/s complex the band runs from -6 to +6
//! kHz; anything from 9 to 15 kHz would alias onto the search's 0 to 3 kHz.
//! Every stage keeps 3.5 kHz either side and stops what would land there.

use num_complex::Complex;

use crate::signal::dsp::fir::{design_lowpass_to_spec, StreamingDecimator};
use crate::signal::dsp::resample::Resampler;

/// FT8's rate.
pub const RATE_HZ: u64 = 12_000;

/// Kept flat either side of the dial: the search's 3 kHz and a margin.
const PASS_HZ: f64 = 3_500.0;

/// The intermediate rates a resampler may start from.
const MID_MIN: u64 = 24_000;
const MID_MAX: u64 = 64_000;

/// The resampler's rejection, dB.
const STOPBAND_DB: f64 = 80.0;

/// How long the DC mean remembers: long against the lowest tone searched
/// (100 Hz), short against a cycle.
const DC_SECONDS: f64 = 0.1;

/// How a radio rate reaches 12 000.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Route {
    /// Integer decimation first.
    pub decimate: u64,
    /// Then a resampler up by `up` and down by `down`, or none (1/1).
    pub up: u64,
    pub down: u64,
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

/// The route from `rate_hz` to [`RATE_HZ`], or `None` for a rate that is not
/// a whole number of hertz or offers no route.
pub fn route(rate_hz: f64) -> Option<Route> {
    let r = rate_hz.round();
    if (rate_hz - r).abs() > 1e-6 || r < RATE_HZ as f64 {
        return None;
    }
    let r = r as u64;
    if r.is_multiple_of(RATE_HZ) {
        return Some(Route {
            decimate: r / RATE_HZ,
            up: 1,
            down: 1,
        });
    }
    // The divisor whose intermediate rate is whole, in range, and makes the
    // simplest resampler.
    (1..=r / MID_MIN)
        .filter(|&d| r.is_multiple_of(d))
        .map(|d| (d, r / d))
        .filter(|&(_, mid)| (MID_MIN..=MID_MAX).contains(&mid))
        .map(|(d, mid)| {
            let g = gcd(RATE_HZ, mid);
            Route {
                decimate: d,
                up: RATE_HZ / g,
                down: mid / g,
            }
        })
        .min_by_key(|r| r.up + r.down)
}

/// Split an integer decimation into stages of at most 16, largest first, so
/// no single filter has to be sharp at a high rate.
fn stages_of(mut d: u64) -> Vec<u64> {
    let mut out = Vec::new();
    while d > 16 {
        match (2..=16).rev().find(|&f| d.is_multiple_of(f)) {
            Some(f) => {
                out.push(f);
                d /= f;
            }
            None => break,
        }
    }
    out.push(d);
    out
}

pub struct Channel {
    stages: Vec<StreamingDecimator>,
    resampler: Option<Resampler>,
    /// The chain's delay, seconds: a sample out stands for the input this long
    /// before its own place.
    delay: f64,
    between: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    dc: Option<Complex<f64>>,
}

impl Channel {
    /// The channel for a stream at `rate_hz`, or `None` where no route
    /// reaches 12 000.
    pub fn new(rate_hz: f64) -> Option<Self> {
        let route = route(rate_hz)?;
        let mut rate = rate_hz;
        let mut stages = Vec::new();
        let mut delay = 0.0;
        let factors = stages_of(route.decimate);
        for &d in &factors {
            let d = d as usize;
            let out_rate = rate / d as f64;
            // What this stage lets through must not fold onto the band any
            // later stage keeps: it passes PASS_HZ and stops from its own
            // output rate less PASS_HZ.
            let stop = out_rate - PASS_HZ;
            let fc = (PASS_HZ + stop) / 2.0 / rate;
            let transition = (stop - PASS_HZ) / rate;
            let s = StreamingDecimator::new(design_lowpass_to_spec(fc, transition, STOPBAND_DB), d);
            delay += s.delay() / rate;
            stages.push(s);
            rate = out_rate;
        }
        let resampler = (route.up > 1 || route.down > 1).then(|| {
            let r = Resampler::new(route.up as usize, route.down as usize, 0.1, STOPBAND_DB);
            delay += r.delay_input_samples() / rate;
            r
        });
        Some(Self {
            stages,
            resampler,
            delay,
            between: Vec::new(),
            scratch: Vec::new(),
            dc: None,
        })
    }

    /// Seconds between an input sample and the output sample that stands for
    /// it.
    pub fn delay(&self) -> f64 {
        self.delay
    }

    /// One block of the radio's stream in, FT8's samples appended to `out`.
    pub fn push(&mut self, iq: &[Complex<f32>], out: &mut Vec<Complex<f32>>) {
        let mut input = std::mem::take(&mut self.scratch);
        input.clear();
        input.extend_from_slice(iq);
        for stage in &mut self.stages {
            stage.process(&input, &mut self.between);
            std::mem::swap(&mut input, &mut self.between);
        }
        let from = out.len();
        match &mut self.resampler {
            Some(r) => {
                r.process(&input, &mut self.between);
                out.extend_from_slice(&self.between);
            }
            None => out.extend_from_slice(&input),
        }
        self.scratch = input;
        self.take_off_dc(&mut out[from..]);
    }

    /// Subtract the slow mean, seeded from the first block's own mean.
    fn take_off_dc(&mut self, out: &mut [Complex<f32>]) {
        if out.is_empty() {
            return;
        }
        let alpha = 1.0 / (DC_SECONDS * RATE_HZ as f64);
        let mut dc = self.dc.unwrap_or_else(|| {
            out.iter()
                .map(|z| Complex::new(f64::from(z.re), f64::from(z.im)))
                .sum::<Complex<f64>>()
                / out.len() as f64
        });
        for z in out.iter_mut() {
            let v = Complex::new(f64::from(z.re), f64::from(z.im));
            dc += (v - dc) * alpha;
            *z = Complex::new((v.re - dc.re) as f32, (v.im - dc.im) as f32);
        }
        self.dc = Some(dc);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::dsp::nco::Nco;

    fn tone(hz: f64, rate: f64, n: usize) -> Vec<Complex<f32>> {
        let mut nco = Nco::new(hz, rate);
        let mut out = vec![Complex::new(0.0, 0.0); n];
        nco.fill(&mut out);
        out
    }

    #[test]
    fn every_rate_the_radios_offer_has_a_route() {
        let cases = [
            (192_000.0, 16, 1, 1),
            (912_000.0, 76, 1, 1),
            (2_400_000.0, 200, 1, 1),
            (20_000_000.0, 625, 3, 8),
            (2_048_000.0, 64, 3, 8),
            (10_000_000.0, 250, 3, 10),
            (256_000.0, 8, 3, 8),
        ];
        for (rate, d, up, down) in cases {
            assert_eq!(
                route(rate),
                Some(Route {
                    decimate: d,
                    up,
                    down
                }),
                "{rate}"
            );
        }
        assert_eq!(route(11_025.0), None);
        assert_eq!(route(192_000.5), None);
    }

    #[test]
    fn stages_split_large_decimations() {
        assert_eq!(stages_of(16), vec![16]);
        assert_eq!(stages_of(200), vec![10, 10, 2]);
        assert_eq!(stages_of(625), vec![5, 5, 5, 5]);
        assert_eq!(stages_of(200).iter().product::<u64>(), 200);
    }

    /// Mean power of the channel's output after it settles.
    fn through(rate: f64, hz: f64) -> f64 {
        let mut ch = Channel::new(rate).unwrap();
        let mut out = Vec::new();
        let input = tone(hz, rate, (rate * 0.6) as usize);
        for block in input.chunks(8192) {
            ch.push(block, &mut out);
        }
        let tail = &out[out.len() / 2..];
        tail.iter().map(|z| f64::from(z.norm_sqr())).sum::<f64>() / tail.len() as f64
    }

    #[test]
    fn the_band_passes_and_what_would_fold_in_is_stopped() {
        for rate in [192_000.0, 256_000.0, 2_400_000.0] {
            for hz in [300.0, 1500.0, 2900.0] {
                let p = through(rate, hz);
                assert!((p - 1.0).abs() < 0.05, "{rate} {hz}: {p}");
            }
            // 12 kHz from the dial would land on it at 12 kS/s.
            let folded = 10.0 * through(rate, 12_000.0 + 1500.0).log10();
            assert!(folded < -60.0, "{rate}: {folded} dB");
        }
    }

    #[test]
    fn the_output_rate_is_exactly_12_000() {
        // Counted over the second of two seconds, past the filters' start.
        for rate in [192_000.0, 256_000.0, 2_048_000.0] {
            let mut ch = Channel::new(rate).unwrap();
            let mut out = Vec::new();
            let input = vec![Complex::new(0.0f32, 0.0); rate as usize];
            for block in input.chunks(4096) {
                ch.push(block, &mut out);
            }
            let first = out.len();
            for block in input.chunks(4096) {
                ch.push(block, &mut out);
            }
            assert_eq!(out.len() - first, 12_000, "{rate}");
        }
    }

    #[test]
    fn dc_is_taken_off() {
        let mut ch = Channel::new(192_000.0).unwrap();
        let mut out = Vec::new();
        ch.push(&vec![Complex::new(0.3f32, -0.2); 192_000], &mut out);
        let last = out.last().unwrap();
        assert!(last.norm() < 1e-3, "{last}");
    }
}
