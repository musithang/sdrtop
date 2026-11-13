// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! What one decoded transmission looked like on the way in, kept beside its
//! numbers so a view can show the symbols themselves.
//!
//! Everything here is read from the same slice and the same fit the decode
//! used ([`super::demod::measured`]); nothing is recomputed differently and
//! nothing feeds back into the decode. A trace is a copy of what was there.
//!
//! - **Tone energies**: each symbol correlated with each tone at the
//!   frequency the fit found, drift followed, and with one tone either side
//!   of the eight (tone -1 and tone 8). Those two are guards, where a
//!   neighbour or a dirty transmitter would show. They are whole tones, not
//!   half: over one symbol a tone half a spacing away is not orthogonal to
//!   the sent one and reads the signal itself about 4 dB down.
//! - **Bits**: what was received (the sign of the soft decisions that
//!   decoded) beside the codeword it decoded to, so the bits the code
//!   repaired are known by position, not only by count.
//! - **The frequency track**: tone 0's frequency through the transmission,
//!   group by group, each point with the spread its own samples showed.
//! - **The envelope**: the signal's power half symbol by half symbol, the
//!   noise's share taken out, over its mean.

use std::f64::consts::{PI, TAU};

use num_complex::Complex;

use super::demod::{self, Measured};
use super::ldpc;
use super::modulate::{data_positions, SYMBOLS, SYMBOL_SECONDS, TONE_SPACING};
use super::search::{DOWN_RATE, DOWN_SYMBOL};

/// Rows of [`Trace::energies`]: tone -1, the eight tones, tone 8.
pub const TRACE_TONES: usize = 10;

/// Symbols in one point of the frequency track: ten groups over the 79,
/// the last of seven. Long enough that each point stands above the noise of
/// a signal a few dB over the decoding threshold, short enough to show a
/// chirp at key-up or a warble from a transmitter's supply.
pub const TRACK_GROUP: usize = 8;

/// One point of the frequency track.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackPoint {
    /// Seconds into the transmission, the middle of the group.
    pub at: f64,
    /// Tone 0's frequency there, Hz above the dial.
    pub hz: f64,
    /// One standard deviation, Hz, from the scatter of the group's own
    /// symbol-to-symbol phase steps.
    pub sigma: f64,
}

/// What one decoded transmission looked like.
#[derive(Clone, Debug, PartialEq)]
pub struct Trace {
    /// The tones the codeword was sent as.
    pub tones: [u8; SYMBOLS],
    /// Each symbol's energy on tone -1, 0..=7 and 8 (rows 0 to 9).
    pub energies: [[f32; TRACE_TONES]; SYMBOLS],
    /// The energy of one cell with noise alone, so a cell reads as dB over
    /// the noise.
    pub noise: f32,
    /// The bits as received: the sign of the soft decisions that decoded.
    pub received: [bool; ldpc::N],
    /// The codeword they decoded to.
    pub codeword: [bool; ldpc::N],
    pub f0_track: Vec<TrackPoint>,
    /// Power per half symbol, the noise's share taken out, over the
    /// transmission's mean (1 is average; noise can take a piece below 0).
    pub envelope: Vec<f32>,
}

impl Trace {
    /// Bits the codeword disagrees with what was received.
    pub fn repaired_bits(&self) -> u32 {
        self.received
            .iter()
            .zip(&self.codeword)
            .filter(|(a, b)| a != b)
            .count() as u32
    }

    /// The symbols, of 79, carrying a bit the code repaired. The Costas
    /// symbols carry none.
    pub fn repaired_symbols(&self) -> [bool; SYMBOLS] {
        let mut out = [false; SYMBOLS];
        for (k, pos) in data_positions().enumerate() {
            out[pos] = (3 * k..3 * k + 3).any(|b| self.received[b] != self.codeword[b]);
        }
        out
    }

    /// Each symbol's strongest of the eight tones over its runner-up, dB.
    pub fn margin_db(&self) -> [f32; SYMBOLS] {
        std::array::from_fn(|s| {
            let mut e: Vec<f32> = self.energies[s][1..=8].to_vec();
            e.sort_by(|a, b| b.total_cmp(a));
            10.0 * (e[0].max(f32::MIN_POSITIVE) / e[1].max(f32::MIN_POSITIVE)).log10()
        })
    }

    /// The least of the data symbols' margins, dB: the symbol the decision
    /// was closest on. The Costas symbols carry no data and are left out.
    pub fn least_margin_db(&self) -> f32 {
        let m = self.margin_db();
        data_positions().map(|s| m[s]).fold(f32::MAX, f32::min)
    }

    /// How much what was received speaks against the tones the codeword was
    /// sent as: over the 58 data symbols, the energy a symbol's strongest
    /// tone holds beyond the sent one's, summed where the two differ, over
    /// the signal's energy on the sent tones (each less a cell's noise).
    ///
    /// A true decode differs from the strongest tones where the signal faded
    /// into the noise, by noise-sized amounts. A wrong codeword the code and
    /// the check let through on a real signal differs in several symbols
    /// wherever they fall, the strong ones too, where the signal itself
    /// stands on another tone.
    pub fn contradiction(&self) -> f32 {
        let (mut against, mut signal) = (0f32, 0f32);
        for s in data_positions() {
            let row = &self.energies[s][1..=8];
            let sent = row[usize::from(self.tones[s])];
            let best = row.iter().copied().fold(f32::MIN, f32::max);
            signal += (sent - self.noise).max(0.0);
            if best > sent {
                against += best - sent;
            }
        }
        if signal > 0.0 {
            against / signal
        } else {
            f32::INFINITY
        }
    }

    /// A trace of nothing, for tests that build a decode by hand.
    #[cfg(test)]
    pub fn empty() -> Self {
        Self {
            tones: [0; SYMBOLS],
            energies: [[0.0; TRACE_TONES]; SYMBOLS],
            noise: 0.0,
            received: [false; ldpc::N],
            codeword: [false; ldpc::N],
            f0_track: Vec::new(),
            envelope: Vec::new(),
        }
    }

    /// The trace of a decode: its slice `y` (tone 0 near the measured
    /// frequency, which is in the slice's own terms), what [`demod::measured`]
    /// found, the soft decisions that decoded and the codeword. `at_zero` is
    /// the frequency the slice put at zero, added back to the track as it is
    /// to the decode's own frequency.
    pub fn build(
        y: &[Complex<f32>],
        m: &Measured,
        tones: &[u8; SYMBOLS],
        llr: &[f32; ldpc::N],
        codeword: &[u8; ldpc::N],
        at_zero: f64,
    ) -> Self {
        let p = &m.physics;
        Self {
            tones: *tones,
            energies: tone_energies(y, p.start, p.f0, p.drift),
            noise: (DOWN_SYMBOL as f64 * m.sigma2) as f32,
            received: std::array::from_fn(|v| llr[v] > 0.0),
            codeword: std::array::from_fn(|v| codeword[v] == 1),
            f0_track: track(y, m, tones, at_zero),
            envelope: envelope(m),
        }
    }
}

/// Tone 0's frequency `t` seconds into the transmission on the fitted line:
/// `f0` in the middle, moving by `drift` from the first symbol to the last.
fn on_the_line(f0: f64, drift: f64, t: f64) -> f64 {
    let length = SYMBOLS as f64 * SYMBOL_SECONDS;
    f0 + drift * (t - length / 2.0) / length
}

/// Each symbol's energy on tones -1 to 8, at the fitted frequency for that
/// symbol.
fn tone_energies(
    y: &[Complex<f32>],
    start: f64,
    f0: f64,
    drift: f64,
) -> [[f32; TRACE_TONES]; SYMBOLS] {
    let first = (start * DOWN_RATE).ceil() as i64;
    std::array::from_fn(|s| {
        let hz = on_the_line(f0, drift, (s as f64 + 0.5) * SYMBOL_SECONDS);
        let a = first + (s * DOWN_SYMBOL) as i64;
        std::array::from_fn(|row| {
            let f = hz + (row as f64 - 1.0) * TONE_SPACING;
            let mut acc = Complex::new(0.0f64, 0.0);
            for n in 0..DOWN_SYMBOL as i64 {
                let i = a + n;
                if i < 0 || i as usize >= y.len() {
                    continue;
                }
                let v = y[i as usize];
                let ph = -TAU * f * n as f64 / DOWN_RATE;
                acc +=
                    Complex::new(f64::from(v.re), f64::from(v.im)) * Complex::from_polar(1.0, ph);
            }
            acc.norm_sqr() as f32
        })
    })
}

/// The frequency track: for each group of [`TRACK_GROUP`] symbols, the mean
/// phase step from one symbol's correlation with the rebuilt waveform to the
/// next is the frequency the fitted line left over there.
fn track(y: &[Complex<f32>], m: &Measured, tones: &[u8; SYMBOLS], at_zero: f64) -> Vec<TrackPoint> {
    let p = &m.physics;
    let (per, _) = demod::per_part(y, p.start, p.f0, p.drift, tones, 1);
    let mut out = Vec::new();
    for group in (0..SYMBOLS).collect::<Vec<_>>().chunks(TRACK_GROUP) {
        // Steps into each symbol of the group from the one before.
        let steps: Vec<(f64, Complex<f64>)> = group
            .iter()
            .filter(|&&k| k > 0)
            .map(|&k| (k as f64 * SYMBOL_SECONDS, per[k] * per[k - 1].conj()))
            .filter(|(_, d)| d.norm() > 0.0)
            .collect();
        if steps.len() < 2 {
            continue;
        }
        let n = steps.len() as f64;
        let mean: Complex<f64> = steps.iter().map(|(_, d)| d / d.norm()).sum();
        if mean.norm() == 0.0 {
            continue;
        }
        let dir = mean / mean.norm();
        let scatter: f64 = steps
            .iter()
            .map(|(_, d)| (d * dir.conj()).arg().powi(2))
            .sum::<f64>()
            / (n - 1.0);
        let at = steps.iter().map(|(t, _)| t).sum::<f64>() / n;
        let to_hz = 1.0 / (2.0 * PI * SYMBOL_SECONDS);
        out.push(TrackPoint {
            at,
            hz: on_the_line(p.f0, p.drift, at) + at_zero + dir.arg() * to_hz,
            sigma: (scatter / n).sqrt() * to_hz,
        });
    }
    out
}

/// Power per piece of the measurement, the noise's share taken out, over
/// the mean.
fn envelope(m: &Measured) -> Vec<f32> {
    if m.power <= 0.0 {
        return vec![0.0; m.pieces.len()];
    }
    m.pieces
        .iter()
        .zip(&m.piece_energy)
        .map(|(c, &e)| {
            if e <= 0.0 {
                return 0.0;
            }
            ((c.norm_sqr() - m.sigma2 * e) / (e * e) / m.power) as f32
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::dsp::testkit::Rng;
    use crate::signal::ft8::conformance::{self, Sent, CYCLE_SAMPLES};
    use crate::signal::ft8::cycle::{self, Decode};
    use crate::signal::ft8::hash::HashBook;
    use crate::signal::ft8::modulate::{self, Waveform, COSTAS, COSTAS_AT};
    use crate::signal::ft8::search::RATE;

    /// One standard CQ with a locator, at 1000 Hz, 0.1 s late.
    fn sent(snr_db: f64, drift: f64) -> Sent {
        Sent {
            f0: 1000.0,
            dt: 0.1,
            drift,
            ..conformance::readable(1, snr_db, &mut Rng::new(5))[0]
        }
    }

    fn the_one(x: &[Complex<f32>]) -> Decode {
        let payload = sent(0.0, 0.0).payload;
        let report = cycle::decode(x, &mut HashBook::new());
        report
            .decodes
            .into_iter()
            .find(|d| d.payload == payload)
            .expect("the transmission decodes")
    }

    #[test]
    fn a_clean_trace_puts_every_symbol_on_its_tone() {
        let mut rng = Rng::new(7);
        let x = conformance::cycle(&[sent(-6.0, 0.0)], &mut rng);
        let d = the_one(&x);
        let t = &d.trace;
        assert_eq!(t.tones, sent(-6.0, 0.0).tones());
        for s in 0..SYMBOLS {
            let best = (1..=8)
                .max_by(|&a, &b| t.energies[s][a].total_cmp(&t.energies[s][b]))
                .unwrap();
            assert_eq!(best - 1, usize::from(t.tones[s]), "symbol {s}");
        }
        for &at in &COSTAS_AT {
            assert_eq!(t.tones[at..at + 7], COSTAS);
        }
        assert_eq!(t.repaired_bits(), d.repaired);
        assert!(t.noise > 0.0);
    }

    /// Measured on this cycle, 40 dB over the noise: the strongest guard cell
    /// of the 79 stands 22.75 dB under its symbol's sent tone (the smoothed
    /// tone changes spill a little past the eight). The bar leaves 2.7 dB.
    #[test]
    fn the_guard_tones_stay_quiet_on_a_clean_signal() {
        let mut rng = Rng::new(11);
        let x = conformance::cycle(&[sent(40.0, 0.0)], &mut rng);
        let t = the_one(&x).trace;
        let worst = (0..SYMBOLS)
            .map(|s| {
                let sent = t.energies[s][usize::from(t.tones[s]) + 1];
                let guard = t.energies[s][0].max(t.energies[s][9]);
                10.0 * (guard / sent).log10()
            })
            .fold(f32::MIN, f32::max);
        assert!(worst < -20.0, "a guard read {worst} dB under the sent tone");
    }

    #[test]
    fn a_drifting_signal_tracks_its_drift() {
        let mut rng = Rng::new(13);
        let x = conformance::cycle(&[sent(-6.0, 2.0)], &mut rng);
        let tr = the_one(&x).trace.f0_track.clone();
        assert_eq!(tr.len(), SYMBOLS.div_ceil(TRACK_GROUP));
        let (a, b) = (tr[0], tr[tr.len() - 1]);
        let length = SYMBOLS as f64 * SYMBOL_SECONDS;
        let expected = 2.0 * (b.at - a.at) / length;
        let sigma = a.sigma.hypot(b.sigma);
        assert!(
            ((b.hz - a.hz) - expected).abs() < 3.0 * sigma,
            "moved {} Hz, expected {expected} within 3 x {sigma}",
            b.hz - a.hz
        );
        for p in &tr {
            assert!(p.sigma < 0.625, "a point at -6 dB with sigma {}", p.sigma);
        }
    }

    /// A transmitter that jumps by 1.5 Hz halfway: the fitted line can only
    /// call it drift, but the track shows two flat halves.
    #[test]
    fn a_frequency_step_shows_in_the_track() {
        let mut rng = Rng::new(17);
        let s = sent(-4.0, 0.0);
        let amplitude = (2500.0 / RATE * 10f64.powf(s.snr_db / 10.0)).sqrt();
        let wave = |f0| {
            let mut one = vec![Complex::new(0.0f32, 0.0); CYCLE_SAMPLES];
            let w = Waveform {
                f0,
                start: s.start(),
                amplitude,
                drift: 0.0,
                phase: 0.3,
            };
            modulate::add(&s.tones(), &w, RATE, &mut one);
            one
        };
        let (low, high) = (wave(1000.0), wave(1001.5));
        let step_at = s.start() + 40.0 * SYMBOL_SECONDS;
        let cut = (step_at * RATE) as usize;
        let mut x = rng.noise(CYCLE_SAMPLES, 1.0);
        for (i, v) in x.iter_mut().enumerate() {
            *v += if i < cut { low[i] } else { high[i] };
        }
        let tr = the_one(&x).trace.f0_track.clone();
        let into = 40.0 * SYMBOL_SECONDS;
        let half = TRACK_GROUP as f64 * SYMBOL_SECONDS / 2.0;
        for p in &tr {
            let want = if p.at + half < into {
                1000.0
            } else if p.at - half > into {
                1001.5
            } else {
                continue;
            };
            assert!(
                (p.hz - want).abs() < 3.0 * p.sigma + 0.1,
                "at {:.2} s read {:.3} Hz, sent {want} (sigma {:.3})",
                p.at,
                p.hz,
                p.sigma
            );
        }
    }

    #[test]
    fn a_fade_shows_in_the_envelope() {
        let mut rng = Rng::new(19);
        let (x, _) = conformance::faded(&[sent(-4.0, 0.0)], 2.0, &mut rng);
        let env = the_one(&x).trace.envelope.clone();
        assert_eq!(env.len(), SYMBOLS * demod::POWER_PARTS);
        let max = env.iter().copied().fold(f32::MIN, f32::max);
        let min = env.iter().copied().fold(f32::MAX, f32::min);
        assert!(min < 0.5 * max, "a 2 Hz fade from {min} to {max}");
    }

    #[test]
    fn a_steady_envelope_sits_near_one() {
        let mut rng = Rng::new(23);
        let x = conformance::cycle(&[sent(10.0, 0.0)], &mut rng);
        let env = the_one(&x).trace.envelope.clone();
        let mean = env.iter().sum::<f32>() / env.len() as f32;
        assert!((mean - 1.0).abs() < 0.05, "mean {mean}");
    }

    #[test]
    fn repaired_symbols_follow_the_repaired_bits() {
        let mut t = Trace::empty();
        assert_eq!(t.repaired_bits(), 0);
        assert!(t.repaired_symbols().iter().all(|r| !r));
        // The sixth data symbol's middle bit: data symbol 5 sits at 12.
        t.received[3 * 5 + 1] = true;
        assert_eq!(t.repaired_bits(), 1);
        let r = t.repaired_symbols();
        assert!(r[12]);
        assert_eq!(r.iter().filter(|&&x| x).count(), 1);
    }

    #[test]
    fn the_margin_is_the_winner_over_the_runner_up() {
        let mut t = Trace::empty();
        t.energies[3][1..=8].copy_from_slice(&[1.0, 10.0, 1.0, 1.0, 100.0, 1.0, 1.0, 1.0]);
        let m = t.margin_db();
        assert!((m[3] - 10.0).abs() < 1e-4, "{}", m[3]);
    }

    /// The least margin is over the data symbols alone: a Costas symbol
    /// closer to a tie does not count.
    #[test]
    fn the_least_margin_leaves_the_costas_symbols_out() {
        let mut t = Trace::empty();
        for row in &mut t.energies {
            row[1..=8].copy_from_slice(&[100.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0]);
        }
        // Symbol 2 is a Costas symbol, 50 a data symbol.
        t.energies[2][1..=8].copy_from_slice(&[1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0]);
        t.energies[50][1..=8].copy_from_slice(&[10.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0]);
        assert!(
            (t.least_margin_db() - 10.0).abs() < 1e-4,
            "{}",
            t.least_margin_db()
        );
    }
}
