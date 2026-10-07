// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The carrier under a GFSK burst: where it starts and how it moves, read as
//! the Bluetooth test suites define them, from whatever bits were sent.
//!
//! **The suites' definitions.** The initial carrier f0 is the mean
//! frequency from the centre of the first preamble bit to the centre of the
//! bit after the preamble: 4 bits on BR (RF.TS.p35 RF/TRM/CA/BV-08-C d), 8 on
//! LE (RF-PHY.TS.4.2.1 TP/TRM-LE/CA/BV-06-C step 4). The drift readings fk
//! are the mean frequency over every ten bits from the second payload bit
//! (RF/TRM/CA/BV-09-C e, TP/TRM-LE/CA/BV-06-C step 6). The preamble
//! alternates, so its mean is the carrier as it stands.
//!
//! **Ten bits of traffic are not ten bits of `1010`.** The suites send an
//! alternation for the drift test, and five whole periods of one average to
//! the carrier exactly. Whitened traffic does not balance over ten bits: six
//! ones and four zeros put a BR block's mean some 30 kHz off the carrier,
//! which the suites' definition would call drift. So each bit's own
//! modulation is taken out before the blocks are averaged ([`by_bit_from`]): a
//! bit's mean reading depends on the bit and its two neighbours and nothing
//! further (a symbol two bits away moves it by about 1e-8 of the deviation,
//! `deviation::suite_readings` has the figure), so the burst itself shows
//! what each of the eight three-bit contexts adds, as the mean over every bit
//! with that context. What is left of a bit's reading is the carrier under
//! it. No pulse shape is assumed: the transmitter's own is what is measured.
//! `signal::net::conformance` holds the blocks this gives on random traffic
//! to the carrier the reference transmitter was given.

use super::deviation::{BitReadings, READINGS_PER_BIT};
use super::uncertainty::Uncertain;

/// The initial carrier f0: the mean of `at` (the frequency at `x` bit
/// periods from the start of bit 0) from the centre of bit `first` to the
/// centre of bit `first + preamble_bits`, read [`READINGS_PER_BIT`] times a
/// bit. The suites integrate; this is the same integral, sampled finely.
pub fn initial(at: impl Fn(f64) -> f32, first: usize, preamble_bits: usize) -> f64 {
    let n = READINGS_PER_BIT * preamble_bits;
    let start = first as f64 + 0.5;
    (0..n)
        .map(|j| at(start + (j as f64 + 0.5) / READINGS_PER_BIT as f64) as f64)
        .sum::<f64>()
        / n as f64
}

/// The carrier under each of `bits`, from its mean reading less what its
/// three-bit context adds, as the burst shows it; `None` for the two end
/// bits, which have no context, and for every bit unless both settled
/// contexts (`000` and `111`) occur, since the carrier is placed midway
/// between them.
///
/// **Fitted together with a straight-line carrier.** A context's mean is
/// taken at its members' mean time, and a carrier that drifts puts that
/// mean where the drift stood then: some 200 Hz on a 4 kHz drift over 200
/// bits, left in every bit of that context. So the readings are fitted as a
/// linear carrier plus one offset per context, by least squares (alternating
/// the two, which converges to it), and a bit's carrier is its reading less
/// its context's offset: a linear drift then leaves nothing behind, and a
/// curved one only its bend.
#[cfg(test)]
pub fn by_bit(bits: &[bool], at: impl Fn(f64) -> f32) -> Vec<Option<f64>> {
    by_bit_from(bits, &BitReadings::read(bits.len(), at))
}

/// Each bit's carrier, its own modulation taken out, from readings already
/// taken.
pub fn by_bit_from(bits: &[bool], readings: &BitReadings) -> Vec<Option<f64>> {
    const ROUNDS: usize = 12;
    let n = bits.len();
    let context =
        |k: usize| (bits[k - 1] as usize) << 2 | (bits[k] as usize) << 1 | bits[k + 1] as usize;
    let inner: Vec<(usize, usize, f64)> = (1..n.saturating_sub(1))
        .map(|k| (k, context(k), readings.mean(k)))
        .collect();
    let counts = inner.iter().fold([0usize; 8], |mut c, &(_, ctx, _)| {
        c[ctx] += 1;
        c
    });
    if counts[0b000] == 0 || counts[0b111] == 0 || inner.len() < 3 {
        return vec![None; n];
    }
    let mut offset = [0.0f64; 8];
    let (mut c0, mut c1) = (0.0f64, 0.0f64);
    for _ in 0..ROUNDS {
        // The contexts' offsets, less the line.
        let mut sums = [0.0f64; 8];
        for &(k, ctx, m) in &inner {
            sums[ctx] += m - (c0 + c1 * k as f64);
        }
        for c in 0..8 {
            if counts[c] > 0 {
                offset[c] = sums[c] / counts[c] as f64;
            }
        }
        // The line, less the contexts' offsets.
        let (mut sx, mut sy, mut sxx, mut sxy) = (0.0, 0.0, 0.0, 0.0);
        for &(k, ctx, m) in &inner {
            let (x, y) = (k as f64, m - offset[ctx]);
            sx += x;
            sy += y;
            sxx += x * x;
            sxy += x * y;
        }
        let count = inner.len() as f64;
        c1 = (count * sxy - sx * sy) / (count * sxx - sx * sx);
        c0 = (sy - c1 * sx) / count;
    }
    // The carrier sits midway between the settled ones and zeros.
    let shift = (offset[0b000] + offset[0b111]) / 2.0;
    let mut out = vec![None; n];
    for &(k, ctx, m) in &inner {
        out[k] = Some(m - offset[ctx] + shift);
    }
    out
}

/// The suites' drift readings over `carrier` (from [`by_bit_from`]): the mean of
/// every whole block of ten bits from `from`, up to `to`. A block missing a
/// bit's carrier is skipped rather than averaged over nine.
pub fn ten_bit_blocks(carrier: &[Option<f64>], from: usize, to: usize) -> Vec<f64> {
    let mut out = Vec::new();
    let mut k = from;
    while k + 10 <= to.min(carrier.len()) {
        let block: Option<Vec<f64>> = carrier[k..k + 10].iter().copied().collect();
        if let Some(b) = block {
            out.push(b.iter().sum::<f64>() / 10.0);
        }
        k += 10;
    }
    out
}

/// A packet's carrier as the test suites read it: where it started, where
/// it ended, and the two drift figures the Core Specification limits, each
/// with the noise it carries.
///
/// **The suites' maxima, with a `±` from the packet's own blocks.** The
/// suites take the largest `|fn - f0|` and `|fn - fn-5|` over the payload's
/// ten-bit blocks (RF-PHY.TS.4.2.1 TP/TRM-LE/CA/BV-06-C). On a cable that is
/// the transmitter; over the air a maximum of noisy blocks also finds the
/// noise, and with nothing drifting noise alone reads 0.9, 2.9 and 9.2 kHz
/// at 40, 30 and 20 dB in 1 MHz (measured through the chain's own pieces
/// in 200 trials, BR and LE alike). The same for BR (RF.TS.p35
/// RF/TRM/CA/BV-09-C) and LE. Kept as the
/// suites define it (Viktor, 2026-09-26), with a `±` from the blocks'
/// scatter about a straight line, which the panels print it against: a
/// reading whose `±` is past their resolution shows as a dash.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Drift {
    /// f0, the preamble's mean frequency (TP/TRM-LE/CA/BV-06-C step 4), or
    /// the first block's where the preamble is not held, in Hz.
    pub initial_hz: Uncertain,
    /// The last ten-bit block before the CRC, in Hz.
    pub final_hz: Uncertain,
    /// The block furthest from f0, as `fk - f0`, signed: the suites'
    /// `|f0 - fn|`, which the Core's "frequency drift during any packet"
    /// is held to.
    pub drift_hz: Uncertain,
    /// The largest change over five blocks, `fn - fn-5`, signed, over the
    /// time five blocks span: the suites' drift rate, in Hz/us.
    pub drift_rate_hz_per_us: Uncertain,
}

/// The ten-bit blocks' scatter about a straight line through them: the
/// noise one block carries, with any curve in the drift counted in, which
/// only makes it more cautious. `None` below three blocks.
fn block_sigma(blocks: &[f64]) -> Option<f64> {
    let n = blocks.len();
    if n < 3 {
        return None;
    }
    let nf = n as f64;
    let (sx, sy) = (nf * (nf - 1.0) / 2.0, blocks.iter().sum::<f64>());
    let sxx = (0..n).map(|i| (i * i) as f64).sum::<f64>();
    let sxy = blocks
        .iter()
        .enumerate()
        .map(|(i, y)| i as f64 * y)
        .sum::<f64>();
    let slope = (nf * sxy - sx * sy) / (nf * sxx - sx * sx);
    let icept = (sy - slope * sx) / nf;
    let ss = blocks
        .iter()
        .enumerate()
        .map(|(i, y)| (y - icept - slope * i as f64).powi(2))
        .sum::<f64>();
    Some((ss / (nf - 2.0)).sqrt())
}

/// A burst's carrier from its f0 (the preamble's mean and how many bits it
/// was read over, or `None` where the preamble is not held, and the first
/// block stands in) and its ten-bit blocks ([`ten_bit_blocks`]), at
/// `symbol_rate_hz`. `None` under six blocks, where the suites' five-block
/// rate cannot be read.
pub fn drift_from(
    initial: Option<(f64, usize)>,
    blocks: &[f64],
    symbol_rate_hz: f64,
) -> Option<Drift> {
    const SPAN: usize = 5;
    if blocks.len() <= SPAN {
        return None;
    }
    let sigma = block_sigma(blocks)?;
    // f0 is read from the same per-bit noise over fewer bits.
    let (f0, f0_sigma, from) = match initial {
        Some((f0, bits)) => (f0, sigma * (10.0 / bits.max(1) as f64).sqrt(), 0),
        None => (blocks[0], sigma, 1),
    };
    let furthest = *blocks[from..]
        .iter()
        .max_by(|a, b| (*a - f0).abs().total_cmp(&(*b - f0).abs()))?;
    let initial_hz = Uncertain::from_sigma(f0, f0_sigma);
    let steepest = blocks
        .windows(SPAN + 1)
        .map(|w| w[SPAN] - w[0])
        .max_by(|a, b| a.abs().total_cmp(&b.abs()))?;
    let span_us = (SPAN * 10) as f64 / symbol_rate_hz * 1e6;
    Some(Drift {
        initial_hz,
        final_hz: Uncertain::from_sigma(*blocks.last()?, sigma),
        drift_hz: Uncertain::from_sigma(furthest, sigma).difference(&initial_hz),
        drift_rate_hz_per_us: Uncertain::from_sigma(
            steepest / span_us,
            sigma * std::f64::consts::SQRT_2 / span_us,
        ),
    })
}

/// LE Coded (S=8)'s carrier as RFPHY/TRM/BV-14-C reads it, from its
/// 16-symbol groups: `preamble`, f0 to f3, and `payload`, f4 on, each the
/// mean frequency over 16 symbols. The same [`Drift`] LE 1M's carrier is,
/// because the limits are the same ones in other units: 19.2 kHz over 48 us
/// is LE 1M's 20 kHz over 50 us, 400 Hz/us, and 50 kHz from f0 is both.
///
/// The steepest change is over three groups, 48 us: f3 against f0 in the
/// preamble, and every fn against fn-3 in the payload. Never across the gap
/// between them (the access address, the CI and TERM1 lie there), whose
/// groups are not 48 us apart. The furthest from f0 is over f2 on, as the
/// suite reads `|f0 - fn|` for n from 2. The `±` is the payload groups'
/// scatter about a straight line, as LE 1M's is its blocks'. `None` under
/// four payload groups, where no 48 us step lies within the payload.
pub fn coded_drift_from(preamble: [f64; 4], payload: &[f64]) -> Option<Drift> {
    const SPAN: usize = 3;
    const SPAN_US: f64 = 48.0;
    if payload.len() <= SPAN {
        return None;
    }
    let sigma = block_sigma(payload)?;
    let f0 = preamble[0];
    let initial_hz = Uncertain::from_sigma(f0, sigma);
    let furthest = preamble[2..]
        .iter()
        .chain(payload)
        .copied()
        .max_by(|a, b| (a - f0).abs().total_cmp(&(b - f0).abs()))?;
    let steepest = std::iter::once(preamble[SPAN] - preamble[0])
        .chain(payload.windows(SPAN + 1).map(|w| w[SPAN] - w[0]))
        .max_by(|a, b| a.abs().total_cmp(&b.abs()))?;
    Some(Drift {
        initial_hz,
        final_hz: Uncertain::from_sigma(*payload.last()?, sigma),
        drift_hz: Uncertain::from_sigma(furthest, sigma).difference(&initial_hz),
        drift_rate_hz_per_us: Uncertain::from_sigma(
            steepest / SPAN_US,
            sigma * std::f64::consts::SQRT_2 / SPAN_US,
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A trace of carrier plus a constant step per bit value: every bit's
    /// carrier comes back as the carrier, whatever the balance of ones and
    /// zeros in a block, and a drifting carrier is followed.
    #[test]
    fn each_bit_gives_the_carrier_under_it() {
        let mut rng = crate::signal::dsp::testkit::Rng::new(4);
        let bits: Vec<bool> = (0..200).map(|_| rng.next_u64() & 1 == 1).collect();
        let carrier = |x: f64| 10_000.0 + 20.0 * x;
        let at = |x: f64| {
            let k = (x.floor() as usize).min(bits.len() - 1);
            (carrier(x) + if bits[k] { 150_000.0 } else { -150_000.0 }) as f32
        };
        let got = by_bit(&bits, at);
        assert!(got[0].is_none() && got[199].is_none());
        for (k, c) in got.iter().enumerate().skip(1).take(198) {
            let want = carrier(k as f64 + 0.5);
            assert!((c.unwrap() - want).abs() < 1.0, "{k}: {c:?} vs {want}");
        }
        let blocks = ten_bit_blocks(&got, 1, 199);
        assert_eq!(blocks.len(), 19);
        assert!((initial(at, 0, 4) - carrier(2.5)).abs() < 150_000.0);
    }

    /// Without both settled contexts there is nothing to place the carrier
    /// between: refused, not guessed.
    #[test]
    fn no_settled_bits_no_carrier() {
        let bits: Vec<bool> = (0..50).map(|i| i % 2 == 0).collect();
        assert!(by_bit(&bits, |_| 0.0).iter().all(Option::is_none));
    }

    /// LE Coded's carrier from its 16-symbol groups: f0 from the preamble,
    /// the furthest group from it, and the steepest change over three groups
    /// (48 us), never across the gap between the preamble's groups and the
    /// payload's, which are not 48 us apart.
    #[test]
    fn coded_drift_reads_the_bv_14_c_figures() {
        // A step between the preamble and the payload: furthest, not steep.
        let flat: Vec<f64> = vec![30_000.0; 10];
        let d = coded_drift_from([0.0; 4], &flat).unwrap();
        assert_eq!(d.initial_hz.value(), 0.0);
        assert_eq!(d.drift_hz.value(), 30_000.0);
        assert_eq!(d.drift_rate_hz_per_us.value(), 0.0);
        // A payload ramp of 1.2 kHz a group: 3.6 kHz over 48 us.
        let ramp: Vec<f64> = (0..10).map(|i| 1_200.0 * i as f64).collect();
        let d = coded_drift_from([0.0; 4], &ramp).unwrap();
        assert!(
            (d.drift_rate_hz_per_us.value() - 3_600.0 / 48.0).abs() < 1e-9,
            "{d:?}"
        );
        assert_eq!(d.final_hz.value(), 10_800.0);
        // The preamble's own f3 - f0 counts as a 48 us change.
        let d = coded_drift_from([0.0, 0.0, 0.0, 9_600.0], &[9_600.0; 10]).unwrap();
        assert!(
            (d.drift_rate_hz_per_us.value() - 200.0).abs() < 1e-9,
            "{d:?}"
        );
        // Too few payload groups for a 48 us step: refused.
        assert!(coded_drift_from([0.0; 4], &[0.0; 3]).is_none());
    }
}
