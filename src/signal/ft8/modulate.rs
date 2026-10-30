// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The FT8 waveform, in closed form (the QEX paper, sections 4 and 5): the
//! 174 codeword bits as 58 Gray-mapped 8-ary symbols, three Costas arrays
//! around and between them, 79 symbols of 0.16 s sent as continuous-phase FSK
//! with tones 6.25 Hz apart (h = 1), the frequency steps smoothed by a
//! Gaussian filter of BT = 2, the envelope raised and lowered over T/8 with a
//! raised cosine.
//!
//! The receiver needs it twice: the tests send through it, and a decoded
//! signal is rebuilt with it to be subtracted, so a weaker one beneath it can
//! be read. It is written as a complex baseband signal, tone 0 at `f0`; the
//! paper's real audio is the real part of the same thing.

use std::f64::consts::{PI, TAU};

use num_complex::Complex;

use super::ldpc;
use crate::signal::dsp::special::erf;

/// Symbols in a transmission: 58 carrying data, 21 of sync.
pub const SYMBOLS: usize = 79;

/// The signalling interval T, in seconds (Table 4).
pub const SYMBOL_SECONDS: f64 = 0.16;

/// Tone spacing, h / T with h = 1 (Table 4).
pub const TONE_SPACING: f64 = 6.25;

/// The Costas array, sent at the beginning, middle and end (section 4).
pub const COSTAS: [u8; 7] = [3, 1, 4, 0, 6, 5, 2];

/// Where each Costas array starts: symbols S, M_A (29), S, M_B (29), S.
pub const COSTAS_AT: [usize; 3] = [0, 36, 72];

/// Seconds after the 15 s boundary a transmission nominally starts (section 8,
/// "the nominal start time, 0.5 s after the beginning of a 15 s interval").
pub const NOMINAL_START: f64 = 0.5;

/// Gaussian filter bandwidth-time product for FT8 (Table 4).
const BT: f64 = 2.0;

/// Table 3: the tone a triad of bits is sent as, by the triad's value read
/// as a binary number (000 is 0, 001 is 1, 011 is 2, 010 is 3, 110 is 4, 100
/// is 5, 101 is 6, 111 is 7).
pub const GRAY: [u8; 8] = [0, 1, 3, 2, 5, 6, 4, 7];

/// The data symbols' positions among the 79: everything but the Costas
/// arrays, in order.
pub fn data_positions() -> impl Iterator<Item = usize> {
    (7..36).chain(43..72)
}

/// The 79 tones of a codeword.
pub fn channel_symbols(codeword: &[u8; ldpc::N]) -> [u8; SYMBOLS] {
    let mut tones = [0u8; SYMBOLS];
    for &at in &COSTAS_AT {
        tones[at..at + 7].copy_from_slice(&COSTAS);
    }
    for (k, pos) in data_positions().enumerate() {
        let b = &codeword[3 * k..3 * k + 3];
        tones[pos] = GRAY[usize::from(b[0] << 2 | b[1] << 1 | b[2])];
    }
    tones
}

/// Equation (3): the Gaussian-smoothed frequency pulse, times T, at `u = t/T`
/// from the centre of its symbol. It has unit area in `u`, so a pulse of
/// height `b` moves the phase by `2 pi h b`.
pub fn pulse(u: f64) -> f64 {
    let k = PI * (2.0 / std::f64::consts::LN_2).sqrt();
    0.5 * (erf(k * BT * (u + 0.5)) - erf(k * BT * (u - 0.5)))
}

/// One transmission, as a receiver would meet it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Waveform {
    /// Tone 0's frequency in the middle of the transmission, Hz.
    pub f0: f64,
    /// The first symbol's start, seconds from the start of the buffer.
    pub start: f64,
    /// Peak amplitude of the complex signal.
    pub amplitude: f64,
    /// How far the frequency moves from the first symbol to the last, Hz,
    /// linearly, centred on `f0`.
    pub drift: f64,
    /// Carrier phase at the start, radians.
    pub phase: f64,
}

/// [`pulse`] tabulated from -1.5 to +1.5 symbols in steps of 1/[`TABLE_STEPS`]
/// of a symbol, so a waveform costs table lookups, not error functions.
/// Linear interpolation between entries is off by at most `h^2/8 max|p''|`,
/// about 2e-7 of a tone spacing here: a microhertz.
const TABLE_STEPS: usize = 8192;

fn pulse_table() -> &'static [f64] {
    static TABLE: std::sync::OnceLock<Vec<f64>> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        (0..=3 * TABLE_STEPS)
            .map(|i| pulse(i as f64 / TABLE_STEPS as f64 - 1.5))
            .collect()
    })
}

fn pulse_fast(u: f64) -> f64 {
    let x = (u + 1.5) * TABLE_STEPS as f64;
    if !(0.0..(3 * TABLE_STEPS) as f64).contains(&x) {
        return 0.0;
    }
    let i = x as usize;
    let t = pulse_table();
    t[i] + (t[i + 1] - t[i]) * (x - i as f64)
}

/// The frequency deviation, in tone spacings, at `s` symbols into the
/// transmission. The paper sums the nearest pulse and its two neighbours;
/// beyond the ends the first and last tones are held, our choice (a receiver
/// must not depend on the first and last half symbol).
fn deviation(tones: &[u8; SYMBOLS], s: f64) -> f64 {
    let n = (s - 0.5).round() as i64;
    (n - 1..=n + 1)
        .map(|m| {
            let tone = tones[m.clamp(0, SYMBOLS as i64 - 1) as usize];
            f64::from(tone) * pulse_fast(s - m as f64 - 0.5)
        })
        .sum()
}

/// The envelope at `s` symbols in: a raised cosine over the first and last
/// eighth of a symbol (section 5), one between, zero outside.
fn envelope(s: f64) -> f64 {
    let end = SYMBOLS as f64;
    if !(0.0..=end).contains(&s) {
        return 0.0;
    }
    let edge = s.min(end - s);
    if edge >= 0.125 {
        1.0
    } else {
        0.5 * (1.0 - (8.0 * PI * edge).cos())
    }
}

/// Add a transmission of `tones` to `out`, sampled at `rate` Hz. Samples the
/// transmission does not reach are left as they were.
pub fn add(tones: &[u8; SYMBOLS], w: &Waveform, rate: f64, out: &mut [Complex<f32>]) {
    let duration = SYMBOLS as f64 * SYMBOL_SECONDS;
    let first = ((w.start * rate).ceil().max(0.0)) as usize;
    let last = (((w.start + duration) * rate).floor() as usize).min(out.len().saturating_sub(1));
    if out.is_empty() || first > last {
        return;
    }
    // Integrate the instantaneous frequency sample by sample, from the
    // transmission's start (a buffer that begins later still gets the phase
    // the signal has there).
    let start_sample = (w.start * rate).ceil() as i64;
    let mut phase = w.phase;
    for i in start_sample..=last as i64 {
        let t = i as f64 / rate - w.start;
        let s = t / SYMBOL_SECONDS;
        let f = w.f0 + w.drift * (s / SYMBOLS as f64 - 0.5) + TONE_SPACING * deviation(tones, s);
        if i >= first as i64 {
            let a = w.amplitude * envelope(s);
            let z = Complex::from_polar(a, phase);
            out[i as usize] += Complex::new(z.re as f32, z.im as f32);
        }
        phase = (phase + TAU * f / rate) % TAU;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::ft8::crc;

    const RATE: f64 = 12_000.0;

    fn tones_of(payload: u128) -> [u8; SYMBOLS] {
        channel_symbols(&ldpc::encode(crc::with_crc(payload)))
    }

    #[test]
    fn costas_arrays_sit_at_the_start_middle_and_end() {
        let t = tones_of(0x1F2E_3D4C_5B6A_7988);
        for at in COSTAS_AT {
            assert_eq!(t[at..at + 7], COSTAS);
        }
        assert_eq!(data_positions().count(), 58);
    }

    #[test]
    fn bits_map_to_tones_by_table_3() {
        // Codeword bits arranged so data symbol k carries the triad k % 8.
        let mut cw = [0u8; ldpc::N];
        for k in 0..58 {
            let v = k % 8;
            cw[3 * k] = (v >> 2 & 1) as u8;
            cw[3 * k + 1] = (v >> 1 & 1) as u8;
            cw[3 * k + 2] = (v & 1) as u8;
        }
        let t = channel_symbols(&cw);
        // Table 3, symbol by bits: 000 0, 001 1, 010 3, 011 2, 100 5, 101 6,
        // 110 4, 111 7.
        let expect = [0, 1, 3, 2, 5, 6, 4, 7];
        for (k, pos) in data_positions().enumerate() {
            assert_eq!(t[pos], expect[k % 8], "data symbol {k}");
        }
    }

    #[test]
    fn the_pulse_has_unit_area_and_shifted_pulses_tile() {
        let du = 1e-3;
        let area: f64 = (-4000..4000).map(|i| pulse(i as f64 * du) * du).sum();
        assert!((area - 1.0).abs() < 1e-9, "{area}");
        for i in 0..100 {
            let u = i as f64 / 100.0;
            let sum: f64 = (-3..=3).map(|m| pulse(u - m as f64)).sum();
            assert!((sum - 1.0).abs() < 1e-9, "{u}: {sum}");
        }
        // Only the nearest pulse and its neighbours matter (section 5).
        assert!(pulse(1.5) < 1e-12);
        // The table the waveform reads agrees with the function.
        for i in 0..3000 {
            let u = -1.5 + i as f64 * 0.001 + 0.000_37;
            assert!((pulse_fast(u) - pulse(u)).abs() < 1e-6, "{u}");
        }
    }

    /// Instantaneous frequency of a complex signal between two samples.
    fn freq(x: &[Complex<f32>], i: usize) -> f64 {
        f64::from((x[i + 1] * x[i].conj()).arg()) * RATE / TAU
    }

    #[test]
    fn a_tone_held_reads_at_its_frequency() {
        // All-Costas would vary; build tones with long runs instead.
        let mut tones = [5u8; SYMBOLS];
        tones[40..50].fill(2);
        let w = Waveform {
            f0: 1000.0,
            start: 0.5,
            amplitude: 1.0,
            drift: 0.0,
            phase: 0.3,
        };
        let mut x = vec![Complex::new(0.0, 0.0); (15.0 * RATE) as usize];
        add(&tones, &w, RATE, &mut x);
        let at = |sym: f64| ((w.start + sym * SYMBOL_SECONDS) * RATE) as usize;
        assert!((freq(&x, at(20.5)) - (1000.0 + 5.0 * 6.25)).abs() < 1e-3);
        assert!((freq(&x, at(45.5)) - (1000.0 + 2.0 * 6.25)).abs() < 1e-3);
    }

    #[test]
    fn drift_moves_tone_zero_about_its_middle() {
        let tones = [0u8; SYMBOLS];
        let w = Waveform {
            f0: 800.0,
            start: 0.5,
            amplitude: 1.0,
            drift: 4.0,
            phase: 0.0,
        };
        let mut x = vec![Complex::new(0.0, 0.0); (15.0 * RATE) as usize];
        add(&tones, &w, RATE, &mut x);
        let at = |sym: f64| ((w.start + sym * SYMBOL_SECONDS) * RATE) as usize;
        assert!((freq(&x, at(39.5)) - 800.0).abs() < 0.01);
        assert!((freq(&x, at(1.0)) - (800.0 - 2.0 + 4.0 / 79.0)).abs() < 0.01);
    }

    #[test]
    fn the_envelope_rises_over_an_eighth_of_a_symbol_and_lasts_12_64_s() {
        let w = Waveform {
            f0: 1500.0,
            start: 0.5,
            amplitude: 2.0,
            drift: 0.0,
            phase: 0.0,
        };
        let mut x = vec![Complex::new(0.0, 0.0); (15.0 * RATE) as usize];
        add(&tones_of(42), &w, RATE, &mut x);
        let i0 = (0.5 * RATE) as usize;
        let ramp = (SYMBOL_SECONDS / 8.0 * RATE) as usize; // 240 samples, 20 ms
        let end = i0 + (12.64 * RATE) as usize;
        assert_eq!(x[i0 - 1].norm(), 0.0);
        assert!(x[i0].norm() < 1e-3);
        assert!((x[i0 + ramp / 2].norm() - 1.0).abs() < 0.01);
        assert!((x[i0 + ramp].norm() - 2.0).abs() < 1e-3);
        assert!((x[end - ramp - 1].norm() - 2.0).abs() < 1e-3);
        assert!(x[end].norm() < 1e-3);
        assert_eq!(x[end + 1].norm(), 0.0);
    }

    #[test]
    fn a_buffer_that_starts_mid_transmission_gets_the_same_samples() {
        let w = Waveform {
            f0: 1200.0,
            start: 0.5,
            amplitude: 1.0,
            drift: 1.0,
            phase: 1.0,
        };
        let t = tones_of(7);
        let mut whole = vec![Complex::new(0.0, 0.0); (15.0 * RATE) as usize];
        add(&t, &w, RATE, &mut whole);
        let late = Waveform { start: -2.5, ..w };
        let mut part = vec![Complex::new(0.0, 0.0); (12.0 * RATE) as usize];
        add(&t, &late, RATE, &mut part);
        let off = (3.0 * RATE) as usize;
        for i in [0, 1000, 50_000, 100_000] {
            assert!((part[i] - whole[i + off]).norm() < 1e-4, "{i}");
        }
    }
}
