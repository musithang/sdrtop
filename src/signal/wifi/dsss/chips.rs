// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The Barker chips as our capture hears them.
//!
//! **A symbol is 20 samples.** Eleven chips at 11 Mchip/s make a 1 us
//! symbol (15.4.4.4), exactly 20 samples at 20 Msps, so the chips are never
//! resampled: a chip is 20/11 of a sample, and the symbol's 20 samples are
//! what its 11 chips leave after our capture cuts them to +-10 MHz. That
//! cut is modelled once, as an ideal filter: the chips drawn at 220 Msps
//! (20 samples a chip, 11 times our rate), everything beyond +-10 MHz
//! removed, every 11th sample kept.

use num_complex::Complex;
use rustfft::FftPlanner;

/// The 11-chip Barker sequence, first chip first (15.4.4.4).
pub const BARKER: [f64; 11] = [1.0, -1.0, 1.0, 1.0, -1.0, 1.0, 1.0, 1.0, -1.0, -1.0, -1.0];

/// Samples a symbol at 20 Msps.
pub const SYMBOL: usize = 20;

/// How many times finer than our rate the chips are drawn.
const FINE: usize = 11;

/// `symbols` as a transmitter's chips, drawn at 220 Msps, cut to +-10 MHz
/// when `cut`, and taken at 20 Msps: symbol k's 20 samples from 20 k,
/// plus `pad` symbols of tail either side.
pub fn chips(symbols: &[Complex<f64>], pad: usize, cut: bool) -> Vec<Complex<f64>> {
    let per_symbol = SYMBOL * FINE;
    let n = (symbols.len() + 2 * pad) * per_symbol;
    let mut fine = vec![Complex::new(0.0, 0.0); n];
    for (k, s) in symbols.iter().enumerate() {
        for (c, chip) in BARKER.iter().enumerate() {
            let from = (pad + k) * per_symbol + c * SYMBOL;
            for z in &mut fine[from..from + SYMBOL] {
                *z = s * chip;
            }
        }
    }
    if cut {
        let mut planner = FftPlanner::new();
        planner.plan_fft_forward(n).process(&mut fine);
        // Bins beyond +-10 MHz of 220 MHz: keep |f| < n / 22.
        let edge = n / (2 * FINE);
        for (i, z) in fine.iter_mut().enumerate() {
            let f = if i <= n / 2 { i } else { n - i };
            if f >= edge {
                *z = Complex::new(0.0, 0.0);
            }
        }
        planner.plan_fft_inverse(n).process(&mut fine);
        for z in &mut fine {
            *z /= n as f64;
        }
    }
    fine.into_iter().step_by(FINE).collect()
}

/// One symbol of +1 as our capture hears it, its own 20 samples, less
/// their mean: the matched filter's template. Real, since the chips are and
/// the cut is symmetric. The Barker chips sum to one, not zero, so the
/// symbol has a little DC; taken out, the template is blind to whatever DC
/// the radio leaves (which, left in, passed the gate thousands of times a
/// recording), at the cost of that little of the symbol's energy. DC left
/// in the window's energy only makes the gate harder to pass.
pub fn template() -> &'static [f32; SYMBOL] {
    static T: std::sync::OnceLock<[f32; SYMBOL]> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        let pad = 8;
        let heard = chips(&[Complex::new(1.0, 0.0)], pad, true);
        let at = pad * SYMBOL;
        let mean = heard[at..at + SYMBOL].iter().map(|z| z.re).sum::<f64>() / SYMBOL as f64;
        std::array::from_fn(|i| (heard[at + i].re - mean) as f32)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Uncut, a symbol is its 11 chips, each held for 20/11 of a sample:
    /// every sample is a chip's value.
    #[test]
    fn uncut_a_symbol_is_its_chips() {
        let s = chips(&[Complex::new(1.0, 0.0)], 0, false);
        assert_eq!(s.len(), SYMBOL);
        for (n, z) in s.iter().enumerate() {
            let chip = BARKER[n * 11 / 20];
            assert_eq!(z.re, chip, "{n}");
        }
    }

    /// Cut to +-10 MHz the symbol keeps most of its energy, and the
    /// template is that symbol without its mean: real, summing to nothing,
    /// and correlated with the uncut chips above 0.9.
    #[test]
    fn the_template_is_the_cut_symbol() {
        let t = template();
        let uncut = chips(&[Complex::new(1.0, 0.0)], 0, false);
        let dot: f64 = t.iter().zip(&uncut).map(|(a, b)| *a as f64 * b.re).sum();
        let et: f64 = t.iter().map(|a| (*a as f64).powi(2)).sum();
        let eu: f64 = uncut.iter().map(|b| b.norm_sqr()).sum();
        let r = dot / (et * eu).sqrt();
        assert!(r > 0.9, "{r}");
        assert!(t.iter().sum::<f32>().abs() < 1e-5);
    }

    /// **What our +-10 MHz costs a 22 MHz signal, measured.** Against a
    /// receiver that hears the whole chip, whose matched filter's SNR is
    /// the symbol's energy over the noise density: ours keeps what of the
    /// cut symbol its template holds (20 a symbol uncut; the template also
    /// leaves out the symbol's DC), and the cut's
    /// tails spill into the next symbols, a floor of their own, measured
    /// on random symbols at the right timing with no noise at all.
    #[test]
    fn what_the_cut_costs() {
        let t = template();
        // What the matched filter collects of one symbol of the cut chips,
        // squared, over the template's energy, against 20 uncut.
        let one = chips(&[Complex::new(1.0, 0.0)], 8, true);
        let dot: f64 = one[8 * SYMBOL..9 * SYMBOL]
            .iter()
            .zip(t)
            .map(|(z, v)| z.re * *v as f64)
            .sum();
        let et: f64 = t.iter().map(|v| (*v as f64).powi(2)).sum();
        let kept = dot * dot / et / SYMBOL as f64;
        let loss_db = -10.0 * kept.log10();
        let mut rng = crate::signal::dsp::testkit::Rng::new(140);
        let sent: Vec<Complex<f64>> = (0..400)
            .map(|_| {
                if rng.next_u64() & 1 == 0 {
                    Complex::new(1.0, 0.0)
                } else {
                    Complex::new(-1.0, 0.0)
                }
            })
            .collect();
        let pad = 8;
        let heard = chips(&sent, pad, true);
        let out: Vec<Complex<f64>> = (10..390)
            .map(|k| {
                let at = (pad + k) * SYMBOL;
                let c: Complex<f64> = heard[at..at + SYMBOL]
                    .iter()
                    .zip(t)
                    .map(|(z, v)| z * *v as f64)
                    .sum();
                c * sent[k].conj()
            })
            .collect();
        let mean = out.iter().sum::<Complex<f64>>() / out.len() as f64;
        let isi = out.iter().map(|o| (o - mean).norm_sqr()).sum::<f64>() / out.len() as f64;
        let floor_db = 10.0 * (mean.norm_sqr() / isi).log10();
        eprintln!("the cut keeps {kept:.3} of a symbol's energy, {loss_db:.2} dB lost; its tails leave a floor {floor_db:.1} dB down");
        assert!((0.2..1.5).contains(&loss_db), "{loss_db}");
        assert!(floor_db > 15.0, "{floor_db}");
    }
}
