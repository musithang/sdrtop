// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The symbols after the long training field, as a receiver first meets
//! them: the L-SIG, and whatever signalling a later format puts after it.
//!
//! **Every one is read the same way.** Cut 80 samples apart from where the
//! channel was measured, with the estimate's timing and derotation, so a
//! timing error inside the guard turns symbol and channel alike; equalised
//! by the long field's estimate; and righted by its four pilots, which in
//! every format here are BPSK at the polarity of the symbol's place,
//! p_n for the n-th symbol after the long field (17.3.5.10; the L-SIG
//! p0, HT-SIG p1 and p2, an RL-SIG p1, a greenfield HT-SIG p0 and p1).
//!
//! **The pilots are weighed against what the offset allows.** What they
//! correct is the phase the offset's error has turned since the long
//! field, and the synchronisation states that error's spread. Four pilots
//! at a few dB measure the phase worse than that spread already bounds it:
//! taken alone they tipped one BPSK symbol in 30 past 45 degrees at 3 dB,
//! where leaving the phase alone read every one. So the pilots' phase is
//! combined with the drift's own prior, each by its information (the
//! pilots' |H|^2 over their noise, against one over the drift's variance):
//! at high SNR or a wide offset spread the pilots decide, at low SNR with a
//! sure offset the synchronisation does.
//!
//! **What a symbol carries is read two ways.** As BPSK readings at a given
//! rotation (1 for BPSK, j for the QBPSK of HT-SIG and VHT-SIG-A's second
//! symbol), each weighted by its tone's |H|^2 over its noise; and as a
//! score for which axis its data power is on, the test the standard's own
//! receiver makes to tell the formats apart (19.3.9.4.3, 27.3.24).

use num_complex::Complex;
use rustfft::FftPlanner;

use super::channel::{estimate, Estimate, Floor};
use super::map::demap_soft;
use super::ofdm::{data_subcarriers, polarity, wide_data_subcarriers, EDGES, PILOTS};
use super::sync::Sync;

/// The sample rate everything here is at (a 20 MHz channel, 17.3.2.4).
const FS: f64 = 20e6;

/// How many symbols after the long training field a frame's format and
/// signalling are read from: the L-SIG, the RL-SIG and an extended-range
/// SIG field's four symbols.
pub const HELD: usize = 6;

/// The first symbol's transform starts here after the long field's first
/// sample: its two symbols and the symbol's 0.8 us guard. Each symbol after
/// it is 80 samples on.
pub const FIRST_AT: usize = 128 + 16;

/// Where tone `k`, -26 to 26 without 0, sits in [`super::channel::TONES`]
/// order.
fn tone(k: i32) -> usize {
    if k < 0 {
        (k + 26) as usize
    } else {
        (k + 25) as usize
    }
}

/// A synchronised frame's long field measured, ready to read the symbols
/// after it.
pub struct Header<'a> {
    samples: &'a [Complex<f32>],
    /// Where the long field's first sample is in `samples`.
    ltf: usize,
    offset_hz: f64,
    /// The offset's standard uncertainty: how far its error can have
    /// turned the carrier by any later symbol.
    offset_sigma_hz: f64,
    channel: Estimate,
}

/// The channel measured from the long training field of the frame `sync`
/// describes. `None` unless the long field is held.
pub fn header<'a>(samples: &'a [Complex<f32>], first_pair: u64, sync: &Sync) -> Option<Header<'a>> {
    let offset_hz = sync.offset.value();
    let channel = estimate(
        samples,
        first_pair,
        sync.ltf_start,
        offset_hz,
        &Floor::flat(),
    )?;
    Some(Header {
        samples,
        ltf: sync.ltf_start.checked_sub(first_pair)? as usize,
        offset_hz,
        offset_sigma_hz: sync.offset.sigma(),
        channel,
    })
}

/// One symbol's 48 data tones, righted by its pilots: each received value
/// with the channel it came through and that tone's noise variance, in the
/// order the data subcarriers are filled (Equation 17-23).
pub struct Symbol {
    tones: Vec<(Complex<f64>, Complex<f64>, f64)>,
}

impl Header<'_> {
    /// The `n`-th symbol after the long field (the L-SIG's place is 0),
    /// its 48 data tones. `None` unless it is held.
    pub fn symbol(&self, n: usize) -> Option<Symbol> {
        let y = self.righted(n)?;
        let tones = data_subcarriers()
            .into_iter()
            .map(|k| {
                let t = tone(k);
                (y[bin(k)], self.channel.h[t], self.channel.variance[t])
            })
            .collect();
        Some(Symbol { tones })
    }

    /// The channel on the four edge tones, -28, -27, 27 and 28, which the
    /// long field does not train: from the L-SIG and the RL-SIG, where HE
    /// and EHT put known values on them ([`EDGES`]), each righted by its
    /// pilots, the two averaged as the long field's two symbols are.
    pub fn edges(&self) -> Option<[Complex<f64>; 4]> {
        let (a, b) = (self.righted(0)?, self.righted(1)?);
        Some(EDGES.map(|(k, v)| (a[bin(k)] + b[bin(k)]) / (2.0 * v)))
    }

    /// The `n`-th symbol's 52 data tones, as HE-SIG-A and U-SIG fill them,
    /// the edge ones through `edges`. Their noise is taken as the long
    /// field's at +-26, the nearest it measured, both being the mean of two
    /// symbols.
    pub fn wide_symbol(&self, n: usize, edges: &[Complex<f64>; 4]) -> Option<Symbol> {
        let y = self.righted(n)?;
        let tones = wide_data_subcarriers()
            .into_iter()
            .map(|k| {
                let (h, t) = match EDGES.iter().position(|&(e, _)| e == k) {
                    Some(i) => (edges[i], tone(k.clamp(-26, 26))),
                    None => (self.channel.h[tone(k)], tone(k)),
                };
                (y[bin(k)], h, self.channel.variance[t])
            })
            .collect();
        Some(Symbol { tones })
    }

    /// The `n`-th symbol's 64 bins, derotated by the offset and righted by
    /// its pilots. `None` unless it is held.
    fn righted(&self, n: usize) -> Option<Vec<Complex<f64>>> {
        let from = FIRST_AT + 80 * n;
        let at = self.ltf.checked_add(from)?;
        let held = self.samples.get(at..at + 64)?;
        let turn = -std::f64::consts::TAU * self.offset_hz / FS;
        let mut y: Vec<Complex<f64>> = held
            .iter()
            .enumerate()
            .map(|(i, z)| {
                Complex::new(z.re as f64, z.im as f64)
                    * Complex::from_polar(1.0, turn * (from + i) as f64)
            })
            .collect();
        FftPlanner::new().plan_fft_forward(64).process(&mut y);
        let h = &self.channel.h;

        // The common phase, from the pilots against what they should read
        // through their channel, and how much they know of it.
        let p = polarity(n);
        let common = PILOTS
            .iter()
            .map(|&(k, v)| y[bin(k)] * (h[tone(k)] * v * p).conj())
            .sum::<Complex<f64>>();
        let pilots: f64 = PILOTS
            .iter()
            .map(|&(k, _)| {
                let v = self.channel.variance[tone(k)];
                if v > 0.0 {
                    h[tone(k)].norm_sqr() / v
                } else {
                    0.0
                }
            })
            .sum();
        // The drift the offset's error allows, from the long field's centre
        // (64 samples in) to this symbol's (32 into its transform).
        let spread = std::f64::consts::TAU * self.offset_sigma_hz * (from + 32 - 64) as f64 / FS;
        let prior = if spread > 0.0 {
            1.0 / (spread * spread)
        } else {
            f64::INFINITY
        };
        let phase = if common.norm() > 0.0 && pilots > 0.0 {
            common.arg() * pilots / (pilots + prior)
        } else {
            0.0
        };
        let right = Complex::from_polar(1.0, -phase);
        Some(y.into_iter().map(|z| z * right).collect())
    }
}

/// Where subcarrier `k` sits in the transform's output.
fn bin(k: i32) -> usize {
    k.rem_euclid(64) as usize
}

impl Symbol {
    /// BPSK readings of its 48 data tones at `rotation` (1, or j for
    /// QBPSK), positive for a 1, each weighted by its tone's certainty, in
    /// subcarrier order (not yet deinterleaved).
    pub fn readings(&self, rotation: Complex<f64>) -> Vec<f32> {
        self.tones
            .iter()
            .map(|&(y, h, v)| {
                let gain = h.norm_sqr();
                if gain == 0.0 || v <= 0.0 {
                    return 0.0;
                }
                let point = y * rotation.conj() / h;
                demap_soft(&[point], 1)[0] * (gain / v) as f32
            })
            .collect()
    }

    /// Positive when its data power is on the quadrature axis (QBPSK),
    /// negative on the in-phase one (BPSK): each tone's value times its
    /// channel's conjugate, over its noise, squared part against part.
    pub fn quadrature(&self) -> f64 {
        self.tones
            .iter()
            .filter(|&&(_, _, v)| v > 0.0)
            .map(|&(y, h, v)| {
                let z = y * h.conj() / v;
                z.im * z.im - z.re * z.re
            })
            .sum()
    }
}
