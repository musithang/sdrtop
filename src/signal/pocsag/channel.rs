// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! One paging channel out of the radio's whole stream: mixed to zero, filtered
//! and decimated to near [`CHANNEL_HZ`], continuously across blocks.
//!
//! **Continuous, unlike the demod bench's channel.** `signal::demod` restarts
//! its mixer every block, because a deviation statistic does not care. A
//! POCSAG page does: its codewords run across blocks, and a phase step at a
//! block boundary is a frequency spike the discriminator reads as a bit. So
//! the oscillator is [`crate::signal::dsp::nco::Nco`], whose phase carries,
//! and both filters are `StreamingDecimator`s, whose history carries;
//! [`Channel::reset`] is for a real break in the stream.
//!
//! **Two stages at high rates.** One filter from 20 Msps straight to 38 kHz
//! would need thousands of taps to be sharp, and `demod::tap_count` stops at
//! 511, where such a filter lets the neighbouring channels in. The first stage
//! comes down to about eight times the channel rate with a filter that only
//! has to protect what the second stage keeps; the second makes the channel.
//! Below sixteen times the channel rate one stage does both.
//!
//! The decimations are integers, so the channel rate is whatever they give:
//! [`Channel::rate`] reports it, never [`CHANNEL_HZ`].
//!
//! **The radio's DC offset is taken off.** A channel at the tuned centre sits
//! on the tuner's DC, a constant added to every sample, which bends the
//! discriminator's reading toward it. A slow mean ([`DC_SECONDS`]) is
//! subtracted: the two FSK tones at ±4.5 kHz average to nothing over it, the
//! offset does not.

use num_complex::Complex;

use crate::signal::demod::tap_count;
use crate::signal::dsp::fir::{design_lowpass, StreamingDecimator};
use crate::signal::dsp::nco::Nco;

/// The channel rate aimed at: 32 samples a bit at 1200 baud, 16 at 2400 and
/// 75 at 512, so the slowest and fastest rates both have room.
pub const CHANNEL_HZ: f64 = 38_400.0;

/// The channel filter's passband edge: the ±4.5 kHz shift and half the 2400
/// baud bit rate's main lobe, with room for a transmitter some kHz off.
const PASS_HZ: f64 = 9_000.0;

/// How long the DC mean remembers: long against a bit at 512 baud, short
/// against a page.
const DC_SECONDS: f64 = 0.1;

pub struct Channel {
    nco: Nco,
    stages: Vec<StreamingDecimator>,
    rate: f64,
    mixed: Vec<Complex<f32>>,
    between: Vec<Complex<f32>>,
    dc: Option<Complex<f64>>,
}

impl Channel {
    /// The channel `offset_hz` from the tuned centre of a stream at
    /// `input_hz`.
    pub fn new(input_hz: f64, offset_hz: f64) -> Self {
        let mut stages = Vec::new();
        let mut rate = input_hz;
        let total = (input_hz / CHANNEL_HZ).floor().max(1.0) as usize;
        let factors: Vec<usize> = if total <= 16 {
            vec![total]
        } else {
            let first = (input_hz / (CHANNEL_HZ * 8.0)).floor().max(1.0) as usize;
            let second = ((input_hz / first as f64) / CHANNEL_HZ).floor().max(1.0) as usize;
            vec![first, second]
        };
        let last = factors.len() - 1;
        for (i, &d) in factors.iter().enumerate() {
            let out_rate = rate / d as f64;
            // The last stage makes the channel; an earlier one only has to
            // stop what would fold into it, so its edge sits just inside its
            // own output's Nyquist, less what the last stage keeps.
            let edge = if i == last {
                PASS_HZ
            } else {
                out_rate / 2.0 - PASS_HZ
            };
            let fc = (edge / rate).min(0.45 / d as f64);
            stages.push(StreamingDecimator::new(design_lowpass(tap_count(d), fc), d));
            rate = out_rate;
        }
        Self {
            nco: Nco::new(-offset_hz, input_hz),
            stages,
            rate,
            mixed: Vec::new(),
            between: Vec::new(),
            dc: None,
        }
    }

    /// The channel's actual sample rate.
    pub fn rate(&self) -> f64 {
        self.rate
    }

    /// One block of the radio's stream in, the channel's samples out.
    pub fn push(&mut self, iq: &[Complex<f32>], out: &mut Vec<Complex<f32>>) {
        self.nco.mix_into(iq, &mut self.mixed);
        let last = self.stages.len() - 1;
        let mut input = std::mem::take(&mut self.mixed);
        for (i, stage) in self.stages.iter_mut().enumerate() {
            if i == last {
                stage.process(&input, out);
            } else {
                stage.process(&input, &mut self.between);
                std::mem::swap(&mut input, &mut self.between);
            }
        }
        self.mixed = input;
        self.take_off_dc(out);
    }

    /// Subtract the slow mean. Seeded from the first block's own mean, so
    /// the stream does not start a tenth of a second off while it settles.
    fn take_off_dc(&mut self, out: &mut [Complex<f32>]) {
        if out.is_empty() {
            return;
        }
        let alpha = 1.0 / (DC_SECONDS * self.rate);
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

    /// A break in the stream: start again with no history.
    pub fn reset(&mut self) {
        self.nco.reset();
        self.dc = None;
        for s in &mut self.stages {
            s.reset();
        }
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

    fn power(x: &[Complex<f32>]) -> f64 {
        x.iter().map(|z| z.norm_sqr() as f64).sum::<f64>() / x.len().max(1) as f64
    }

    /// The rate is the integer decimations' own, near the target at the
    /// rates the radios run.
    #[test]
    fn the_rate_is_what_the_decimation_gives() {
        for input in [2.0e6, 2.4e6, 2.5e6, 8.0e6, 10.0e6, 20.0e6] {
            let c = Channel::new(input, 0.0);
            assert!(
                (CHANNEL_HZ..CHANNEL_HZ * 1.2).contains(&c.rate()),
                "{input}: {}",
                c.rate()
            );
        }
    }

    /// The channel at its offset comes through at full level, from many
    /// blocks as from one; a station 50 kHz away is gone.
    #[test]
    fn the_channel_passes_and_its_neighbour_does_not() {
        let input = 2.4e6;
        let offset = 250e3;
        let mut c = Channel::new(input, offset);
        let signal = tone(offset + 2_000.0, input, 240_000);
        let mut out = Vec::new();
        let mut all = Vec::new();
        for block in signal.chunks(16_384) {
            c.push(block, &mut out);
            all.extend_from_slice(&out);
        }
        let settled = &all[200..];
        assert!((power(settled) - 1.0).abs() < 0.05, "{}", power(settled));

        let mut c = Channel::new(input, offset);
        c.push(&tone(offset + 50_000.0, input, 240_000), &mut out);
        assert!(power(&out[200..]) < 1e-3, "{}", power(&out[200..]));
    }

    /// A constant offset at the tuned centre is taken off, and a tone beside
    /// it comes through at its level.
    #[test]
    fn the_dc_offset_is_taken_off() {
        let input = 2.4e6;
        let dc = Complex::new(0.08f32, -0.05);
        let x: Vec<Complex<f32>> = tone(4_500.0, input, 480_000)
            .iter()
            .map(|z| z * 0.5 + dc)
            .collect();
        let mut c = Channel::new(input, 0.0);
        let mut out = Vec::new();
        let mut all = Vec::new();
        for b in x.chunks(65_536) {
            c.push(b, &mut out);
            all.extend_from_slice(&out);
        }
        let late = &all[all.len() / 2..];
        let mean = late.iter().sum::<Complex<f32>>() / late.len() as f32;
        assert!(mean.norm() < 0.005, "{mean}");
        assert!((power(late) - 0.25).abs() < 0.02, "{}", power(late));
    }

    /// After a reset the channel starts again as new: the same input gives
    /// the same output as a fresh channel's.
    #[test]
    fn a_reset_is_a_fresh_start() {
        let input = 2.0e6;
        let signal = tone(100e3 + 3_000.0, input, 100_000);
        let mut fresh = Channel::new(input, 100e3);
        let mut used = Channel::new(input, 100e3);
        let (mut a, mut b) = (Vec::new(), Vec::new());
        used.push(&tone(5_000.0, input, 33_333), &mut b);
        used.reset();
        fresh.push(&signal, &mut a);
        used.push(&signal, &mut b);
        assert_eq!(a, b);
    }

    /// Across block boundaries the channel is one tone: no phase step where
    /// one block ends and the next begins. Read once the DC mean has
    /// settled, a few of its time constants in: before that the mean of a
    /// short first block is not zero over a few cycles of a tone, and the
    /// tone wobbles while it settles.
    #[test]
    fn blocks_join_without_a_step() {
        let input = 2.0e6;
        let mut c = Channel::new(input, 100e3);
        let signal = tone(100e3 + 1_200.0, input, 2_000_000);
        let mut out = Vec::new();
        let mut all = Vec::new();
        for block in signal.chunks(7_777) {
            c.push(block, &mut out);
            all.extend_from_slice(&out);
        }
        let rate = c.rate();
        let settled = (3.0 * DC_SECONDS * rate) as usize;
        let worst = all[settled..]
            .windows(2)
            .map(|w| {
                let f = crate::signal::dsp::discriminate::instantaneous_freq_hz(w[0], w[1], rate);
                (f - 1_200.0).abs()
            })
            .fold(0.0f32, f32::max);
        assert!(worst < 20.0, "{worst} Hz");
    }
}
