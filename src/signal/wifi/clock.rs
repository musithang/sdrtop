// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! A transmitter's symbol clock, from the pilots across its DATA field.
//!
//! **Carrier and clock are one oscillator, by requirement** ("the error in
//! ppm for the carrier and the symbol timing shall be the same", IEEE
//! 802.11-2024 17.3.9.5, 18.4.7.5, 19.3.18.4, 27.3.21.3). The carrier's
//! offset is measured at synchronisation; this is the other half, so the
//! two can be held against each other.
//!
//! **A fast clock slides the symbols under our transform.** A transmitter
//! whose sample clock runs e fast against ours sends each 80-sample symbol
//! in 80 (1 - e) of ours, so our window, placed at fixed steps, drifts
//! later by about 80 e samples a symbol; a delay of d samples turns
//! subcarrier k by 2 pi k d / 64. The product of the pilots at +k and -k,
//! each times its known sign, cancels everything common to the two (the
//! channel's phase where they share it, the carrier's residual, the
//! polarity) and keeps 2 pi (2k) d / 64, rising with the window's start.
//! Its slope over the DATA field, fitted by least squares for k = 21 and
//! k = 7 with each standard error from its own residuals, is the clock;
//! the pairs' constant phases (each pilot's own channel) are absorbed by
//! the fit, so no channel estimate is needed at all.
//!
//! **Only frames whose pilots are known**: non-HT (Clause 17's +1, +1, +1,
//! -1 at p_n), and HT-mixed with one space-time stream (Psi = 1, 1, 1, -1
//! rotated a place each symbol, Equation 19-54), its DATA symbols where its
//! HT-SIG puts them. More streams put other patterns on each, mixed at one
//! antenna; their clock is not read.

use num_complex::Complex;

use super::format::{Format, Heard};
use super::sig::FIRST_AT;
use super::sync::Sync;
use crate::signal::dsp::uncertainty::Uncertain;

/// The fewest DATA symbols a clock is read from. The uncertainty comes
/// from the fit's own residuals, and with few of them it is itself
/// uncertain: on the recordings, frames of 8 to 29 symbols spread twice
/// as wide as they said (chi-square 2 to 4 a frame), and they weigh almost
/// nothing beside one long frame anyway.
pub const MIN_SYMBOLS: usize = 30;

/// Which pilot signs a frame's DATA symbols carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pilots {
    /// Clause 17 and 18's +1, +1, +1, -1.
    NonHt,
    /// HT's one-stream Psi, rotated a place each symbol.
    Ht,
}

/// Where a frame's DATA symbols are, counted from the long training
/// field's first sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    /// Where the first DATA symbol's transform starts.
    pub first: usize,
    /// From one symbol's transform to the next's.
    pub step: usize,
    pub count: usize,
    pub pilots: Pilots,
}

impl Plan {
    /// The samples after the long field's first that the last DATA
    /// symbol's transform ends at.
    pub fn end(&self) -> usize {
        self.first + self.step * (self.count - 1) + 64
    }
}

/// The DATA field a clock can be read from, by what the frame said: a
/// non-HT frame's or a one-stream BCC HT-mixed one's, of
/// [`MIN_SYMBOLS`] or more.
pub fn plan(heard: &Heard) -> Option<Plan> {
    let plan = match heard.format? {
        Format::NonHt => Plan {
            first: FIRST_AT + 80,
            step: 80,
            count: heard.lsig?.ok()?.n_sym(),
            pilots: Pilots::NonHt,
        },
        Format::HtMixed(Ok(sig)) => {
            let d = sig.data().ok()?;
            if d.n_sts != 1 || sig.cbw40 {
                return None;
            }
            // After the L-SIG, HT-SIG's two symbols, HT-STF and the
            // HT-LTFs, each 80 samples; then the DATA symbols.
            let guard = if sig.short_gi { 8 } else { 16 };
            Plan {
                first: 128 + 80 * (4 + d.n_ltf as usize) + guard,
                step: 64 + guard,
                count: d.n_sym as usize,
                pilots: Pilots::Ht,
            }
        }
        _ => return None,
    };
    (plan.count >= MIN_SYMBOLS).then_some(plan)
}

/// The sample rate everything here is at (a 20 MHz channel, 17.3.2.4).
const FS: f64 = 20e6;

/// HT's one-stream pilot pattern (Table 19-19).
const PSI: [f64; 4] = [1.0, 1.0, 1.0, -1.0];

impl Pilots {
    /// For DATA symbol `n` (from 0), the products of the signs of the
    /// pilots at 21 and -21, and at 7 and -7. The polarity, common to all
    /// four, squares away.
    fn pairs(self, n: usize) -> [f64; 2] {
        match self {
            // +1 on -21, -7, 7 and -1 on 21.
            Pilots::NonHt => [-1.0, 1.0],
            // Psi_n on -21, Psi_(n+1) on -7, Psi_(n+2) on 7, Psi_(n+3) on 21.
            Pilots::Ht => [
                PSI[(n + 3) % 4] * PSI[n % 4],
                PSI[(n + 2) % 4] * PSI[(n + 1) % 4],
            ],
        }
    }
}

/// The slope of `phases` against `at`, unwrapped in order, with its
/// standard error from the residuals.
fn slope(at: &[f64], z: &[Complex<f64>]) -> Option<Uncertain> {
    let n = at.len();
    if n < 3 {
        return None;
    }
    let mut phases = Vec::with_capacity(n);
    let mut last = z[0].arg();
    phases.push(last);
    for w in z.windows(2) {
        last += (w[1] * w[0].conj()).arg();
        phases.push(last);
    }
    let (mx, my) = (
        at.iter().sum::<f64>() / n as f64,
        phases.iter().sum::<f64>() / n as f64,
    );
    let sxx: f64 = at.iter().map(|x| (x - mx).powi(2)).sum();
    let sxy: f64 = at
        .iter()
        .zip(&phases)
        .map(|(x, y)| (x - mx) * (y - my))
        .sum();
    let b = sxy / sxx;
    let rss: f64 = at
        .iter()
        .zip(&phases)
        .map(|(x, y)| (y - my - b * (x - mx)).powi(2))
        .sum();
    Some(Uncertain::from_variance(b, rss / (n - 2) as f64 / sxx))
}

/// The transmitter's sample clock against ours, in ppm, positive when
/// theirs runs fast, with its uncertainty: from `samples` starting at
/// stream pair `first_pair`, the frame `sync` describes laid out as
/// `plan` says. `None` unless its DATA field is held.
pub fn measure(
    samples: &[Complex<f32>],
    first_pair: u64,
    sync: &Sync,
    plan: &Plan,
) -> Option<Uncertain> {
    let base = sync.ltf_start.checked_sub(first_pair)? as usize;
    samples.get(base..base + plan.end())?;
    let turn = -std::f64::consts::TAU * sync.offset.value() / FS;
    // Each pilot's bin by a direct transform: four of 64.
    let bin = |start: usize, k: i32| -> Complex<f64> {
        (0..64)
            .map(|i| {
                let z = samples[base + start + i];
                let phase = turn * (start + i) as f64
                    - std::f64::consts::TAU * (k * i as i32) as f64 / 64.0;
                Complex::new(z.re as f64, z.im as f64) * Complex::from_polar(1.0, phase)
            })
            .sum()
    };
    let mut at = Vec::with_capacity(plan.count);
    let (mut outer, mut inner) = (
        Vec::with_capacity(plan.count),
        Vec::with_capacity(plan.count),
    );
    for n in 0..plan.count {
        let start = plan.first + plan.step * n;
        let [s21, s7] = plan.pilots.pairs(n);
        at.push(start as f64);
        outer.push(bin(start, 21) * bin(start, -21).conj() * s21);
        inner.push(bin(start, 7) * bin(start, -7).conj() * s7);
    }
    // Each pair's phase rises 2 pi (2k) e / 64 a sample of window start: e
    // from each slope, then the two by inverse variance.
    let read = |k: f64, z: &[Complex<f64>]| {
        let b = slope(&at, z)?;
        let scale = 64.0 / (4.0 * std::f64::consts::PI * k) * 1e6;
        Some(Uncertain::from_sigma(b.value() * scale, b.sigma() * scale))
    };
    let (a, b) = (read(21.0, &outer)?, read(7.0, &inner)?);
    let (wa, wb) = (1.0 / a.sigma().powi(2), 1.0 / b.sigma().powi(2));
    if !(wa.is_finite() && wb.is_finite()) {
        return None;
    }
    Some(Uncertain::from_variance(
        (a.value() * wa + b.value() * wb) / (wa + wb),
        1.0 / (wa + wb),
    ))
}

/// **A diagnostic, for the tests**: the carrier offset the LTF's estimate
/// left, from the pilots' common phase drifting across the DATA field, in
/// hertz. Each pilot against itself in the first DATA symbol, times both
/// symbols' known signs and polarities, summed over the four; its phase
/// rises 2 pi (residual) / FS a sample. A third reading of the carrier,
/// independent of the training.
#[cfg(test)]
pub fn residual_carrier_hz(
    samples: &[Complex<f32>],
    first_pair: u64,
    sync: &Sync,
    plan: &Plan,
) -> Option<Uncertain> {
    use super::ofdm::{polarity, PILOTS};
    let base = sync.ltf_start.checked_sub(first_pair)? as usize;
    samples.get(base..base + plan.end())?;
    let turn = -std::f64::consts::TAU * sync.offset.value() / FS;
    let bin = |start: usize, k: i32| -> Complex<f64> {
        (0..64)
            .map(|i| {
                let z = samples[base + start + i];
                let phase = turn * (start + i) as f64
                    - std::f64::consts::TAU * (k * i as i32) as f64 / 64.0;
                Complex::new(z.re as f64, z.im as f64) * Complex::from_polar(1.0, phase)
            })
            .sum()
    };
    let signs = |n: usize| -> [f64; 4] {
        match plan.pilots {
            Pilots::NonHt => PILOTS.map(|(_, v)| v * polarity(n + 1)),
            Pilots::Ht => [0, 1, 2, 3].map(|j| PSI[(n + j) % 4] * polarity(n + 3)),
        }
    };
    let first: Vec<Complex<f64>> = PILOTS.iter().map(|&(k, _)| bin(plan.first, k)).collect();
    let s0 = signs(0);
    let mut at = Vec::new();
    let mut z = Vec::new();
    for n in 0..plan.count {
        let start = plan.first + plan.step * n;
        let sn = signs(n);
        at.push(start as f64);
        z.push(
            PILOTS
                .iter()
                .enumerate()
                .map(|(j, &(k, _))| bin(start, k) * first[j].conj() * sn[j] * s0[j])
                .sum::<Complex<f64>>(),
        );
    }
    let b = slope(&at, &z)?;
    let scale = FS / std::f64::consts::TAU;
    Some(Uncertain::from_sigma(b.value() * scale, b.sigma() * scale))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::dsp::testkit::Rng;
    use crate::signal::wifi::detect::StfDetector;
    use crate::signal::wifi::format::{read, Format};
    use crate::signal::wifi::htsig::{HtSig, SigFault};
    use crate::signal::wifi::lsig::LSig;
    use crate::signal::wifi::ppdu::{clocked, ht_mixed, ppdu};
    use crate::signal::wifi::signal_field::Rate;
    use crate::signal::wifi::sync::{examine, Verdict};

    const LEAD: usize = 700;
    const CENTRE_HZ: f64 = 2_437e6;

    fn non_ht(rate: Rate, length: u16) -> Heard {
        Heard {
            lsig: Some(Ok(LSig { rate, length })),
            format: Some(Format::NonHt),
        }
    }

    fn ht(mcs: u8, short_gi: bool) -> HtSig {
        HtSig {
            mcs,
            cbw40: false,
            length: 1500,
            stbc: 0,
            ldpc: false,
            short_gi,
            ness: 0,
        }
    }

    fn mixed(sig: HtSig) -> Heard {
        Heard {
            lsig: Some(Ok(LSig {
                rate: Rate::R6,
                length: 999,
            })),
            format: Some(Format::HtMixed(Ok(sig))),
        }
    }

    /// Where the DATA symbols are: a non-HT frame's from the symbol after
    /// the L-SIG, 80 samples apart (100 octets at 6 Mb/s: ceil(822 / 24) =
    /// 35); HT-mixed's after HT-SIG, HT-STF and its HT-LTF, 80 or 72 apart
    /// as its guard is long or short.
    #[test]
    fn the_plan_is_where_the_data_symbols_are() {
        assert_eq!(
            plan(&non_ht(Rate::R6, 100)),
            Some(Plan {
                first: 144 + 80,
                step: 80,
                count: 35,
                pilots: Pilots::NonHt,
            })
        );
        // MCS 7, 1500 octets: ceil(12022 / 260) = 47 symbols.
        assert_eq!(
            plan(&mixed(ht(7, false))),
            Some(Plan {
                first: 128 + 80 * 5 + 16,
                step: 80,
                count: 47,
                pilots: Pilots::Ht,
            })
        );
        assert_eq!(
            plan(&mixed(ht(7, true))),
            Some(Plan {
                first: 128 + 80 * 5 + 8,
                step: 72,
                count: 47,
                pilots: Pilots::Ht,
            })
        );
    }

    /// Not measured: a frame of fewer than 30 DATA symbols (an ACK at
    /// 54 Mb/s, 100 octets at 54), more than one stream, LDPC, a refused
    /// HT-SIG, greenfield, HE.
    #[test]
    fn some_frames_give_no_clock() {
        let ldpc = HtSig {
            ldpc: true,
            ..ht(7, false)
        };
        let stbc = HtSig {
            stbc: 1,
            ..ht(7, false)
        };
        let none = [
            non_ht(Rate::R54, 14),
            non_ht(Rate::R54, 100),
            mixed(ht(12, false)),
            mixed(ldpc),
            mixed(stbc),
            Heard {
                lsig: Some(Ok(LSig {
                    rate: Rate::R6,
                    length: 999,
                })),
                format: Some(Format::HtMixed(Err(SigFault::Crc))),
            },
            Heard {
                lsig: None,
                format: Some(Format::Greenfield(Ok(ht(7, false)))),
            },
        ];
        for heard in none {
            assert_eq!(plan(&heard), None, "{heard:?}");
        }
    }

    /// `frame` with its sample clock `ppm` fast and, when `locked`, its
    /// carrier as fast, `LEAD` samples into noise `snr_db` below it.
    fn on_air(
        frame: &[Complex<f64>],
        ppm: f64,
        locked: bool,
        snr_db: f64,
        rng: &mut Rng,
    ) -> Vec<Complex<f32>> {
        let f = clocked(frame, ppm);
        let power = f[..320].iter().map(|z| z.norm_sqr()).sum::<f64>() / 320.0;
        let mut x = rng.noise(LEAD + f.len() + 600, 10f64.powf(-snr_db / 10.0));
        let mut signal = vec![Complex::new(0.0f32, 0.0); x.len()];
        for (o, z) in signal[LEAD..].iter_mut().zip(&f) {
            let z = z / power.sqrt();
            *o = Complex::new(z.re as f32, z.im as f32);
        }
        if locked {
            crate::signal::dsp::nco::Nco::new(ppm * 1e-6 * CENTRE_HZ, 20e6).mix(&mut signal);
        }
        for (o, s) in x.iter_mut().zip(signal) {
            *o += s;
        }
        x
    }

    /// Found, synchronised, its format read, planned and measured; and the
    /// carrier's ppm beside it.
    fn clock_of(x: &[Complex<f32>]) -> Option<(Uncertain, Uncertain)> {
        let peak = StfDetector::new().push(0, x).first()?.peak;
        let Some(Verdict::ThisChannel(s)) = examine(x, 0, peak) else {
            return None;
        };
        let p = plan(&read(x, 0, &s)?)?;
        let clock = measure(x, 0, &s, &p)?;
        let carrier = Uncertain::from_sigma(
            s.offset.value() / CENTRE_HZ * 1e6,
            s.offset.sigma() / CENTRE_HZ * 1e6,
        );
        Some((clock, carrier))
    }

    /// Bias and spread of `readings` against `truth`, and the spread over
    /// the stated sigma.
    fn judge(readings: &[Uncertain], truth: f64) -> (f64, f64, f64) {
        let n = readings.len() as f64;
        let mean = readings.iter().map(|r| r.value()).sum::<f64>() / n;
        let spread = (readings
            .iter()
            .map(|r| (r.value() - mean).powi(2))
            .sum::<f64>()
            / (n - 1.0))
            .sqrt();
        let sigma = readings.iter().map(|r| r.sigma()).sum::<f64>() / n;
        (mean - truth, spread, spread / sigma)
    }

    /// Non-HT frames at 6 Mb/s, 400 octets (134 DATA symbols), at 20 dB:
    /// the clock unbiased at -20, 0, 20 and 40 ppm, and spread as its own
    /// stated uncertainty says.
    #[test]
    fn a_non_ht_clock_is_unbiased_and_as_uncertain_as_it_says() {
        let mut rng = Rng::new(110);
        let psdu: Vec<u8> = (0..400).map(|i| (i * 37 + 11) as u8).collect();
        let frame = ppdu(&psdu, Rate::R6, 0x5d);
        for ppm in [-20.0, 0.0, 20.0, 40.0] {
            let readings: Vec<Uncertain> = (0..30)
                .filter_map(|_| clock_of(&on_air(&frame, ppm, true, 20.0, &mut rng)))
                .map(|(c, _)| c)
                .collect();
            assert!(readings.len() >= 29, "{ppm}: {} read", readings.len());
            let (bias, spread, ratio) = judge(&readings, ppm);
            eprintln!("{ppm:+} ppm: bias {bias:+.3}, spread {spread:.3}, ratio {ratio:.2}");
            assert!(
                bias.abs() < 3.0 * spread / (readings.len() as f64).sqrt() + 0.05,
                "{ppm}: bias {bias}"
            );
            assert!(
                (0.6..1.5).contains(&ratio),
                "{ppm}: spread {spread} against stated, ratio {ratio}"
            );
        }
    }

    /// HT-mixed, MCS 7, 1500 octets, long and short guard, 30 ppm at 20 dB:
    /// read where its HT-SIG puts the DATA symbols, with HT's rotating
    /// pilots.
    #[test]
    fn an_ht_clock_is_read_with_either_guard() {
        let mut rng = Rng::new(111);
        for short_gi in [false, true] {
            let sig = ht(7, short_gi);
            let readings: Vec<Uncertain> = (0..20)
                .filter_map(|_| {
                    let frame = ht_mixed(&sig, &mut rng);
                    clock_of(&on_air(&frame, 30.0, true, 20.0, &mut rng))
                })
                .map(|(c, _)| c)
                .collect();
            assert!(readings.len() >= 19, "{short_gi}: {} read", readings.len());
            let (bias, spread, ratio) = judge(&readings, 30.0);
            eprintln!(
                "short guard {short_gi}: bias {bias:+.3}, spread {spread:.3}, ratio {ratio:.2}"
            );
            assert!(
                bias.abs() < 3.0 * spread / (readings.len() as f64).sqrt() + 0.1,
                "{short_gi}: bias {bias}"
            );
            assert!((0.5..1.6).contains(&ratio), "{short_gi}: ratio {ratio}");
        }
    }

    /// **The cross-check.** Locked, the clock and the carrier agree within
    /// their uncertainties; a transmitter whose clock runs 30 ppm fast
    /// while its carrier does not is told apart by many sigma.
    #[test]
    fn locked_clocks_agree_and_unlocked_ones_do_not() {
        let mut rng = Rng::new(112);
        let psdu: Vec<u8> = (0..1500).map(|i| (i * 13 + 5) as u8).collect();
        let frame = ppdu(&psdu, Rate::R6, 0x2a);
        for _ in 0..5 {
            let (clock, carrier) = clock_of(&on_air(&frame, 30.0, true, 20.0, &mut rng)).unwrap();
            let sigma = clock.sigma().hypot(carrier.sigma());
            assert!(
                (clock.value() - carrier.value()).abs() < 4.0 * sigma,
                "{clock:?} {carrier:?}"
            );
            let (clock, carrier) = clock_of(&on_air(&frame, 30.0, false, 20.0, &mut rng)).unwrap();
            let sigma = clock.sigma().hypot(carrier.sigma());
            assert!(
                (clock.value() - carrier.value()).abs() > 10.0 * sigma,
                "{clock:?} {carrier:?}"
            );
        }
    }

    /// The diagnostic reads a carrier error the synchronisation was told
    /// to leave: a frame whose sync is told it is 2 kHz lower than it
    /// measured.
    #[test]
    fn the_residual_carrier_is_read_from_the_pilots() {
        let mut rng = Rng::new(114);
        let psdu: Vec<u8> = (0..400).map(|i| (i * 7) as u8).collect();
        let x = on_air(&ppdu(&psdu, Rate::R6, 0x5d), 0.0, true, 25.0, &mut rng);
        let peak = StfDetector::new().push(0, &x)[0].peak;
        let Some(Verdict::ThisChannel(s)) = examine(&x, 0, peak) else {
            panic!("not judged");
        };
        let p = plan(&read(&x, 0, &s).unwrap()).unwrap();
        let told = Sync {
            offset: Uncertain::from_sigma(s.offset.value() - 2_000.0, s.offset.sigma()),
            ..s
        };
        // The frame's carrier is truly 0 here; what the sync measured of it
        // is left as well as the 2 kHz.
        let left = 2_000.0 - s.offset.value();
        let r = residual_carrier_hz(&x, 0, &told, &p).unwrap();
        assert!(
            (r.value() - left).abs() < 4.0 * r.sigma() + 5.0,
            "{r:?} against {left}"
        );
    }

    /// Without its whole DATA field held, nothing is read.
    #[test]
    fn a_frame_not_held_to_its_end_is_not_read() {
        let mut rng = Rng::new(113);
        let psdu: Vec<u8> = (0..100).collect();
        let x = on_air(&ppdu(&psdu, Rate::R6, 0x5d), 10.0, true, 20.0, &mut rng);
        let peak = StfDetector::new().push(0, &x)[0].peak;
        let Some(Verdict::ThisChannel(s)) = examine(&x, 0, peak) else {
            panic!("not judged");
        };
        let p = plan(&read(&x, 0, &s).unwrap()).unwrap();
        let end = s.ltf_start as usize + p.end();
        assert!(measure(&x[..end], 0, &s, &p).is_some());
        assert!(measure(&x[..end - 1], 0, &s, &p).is_none());
    }
}
