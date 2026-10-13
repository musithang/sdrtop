// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The SIGNAL field (L-SIG): the one symbol after the long training field
//! that says a frame's rate and length, hence how long it is on the air.
//!
//! BPSK at rate 1/2, not scrambled, through the DATA field's interleaver
//! with the pilots at polarity p0 (IEEE 802.11-2024, 17.3.4). Every later
//! format sends one too, with a rate and length that give its whole
//! duration (10.27.4, 27.3.11.5), so what it says about time holds for
//! frames this bench cannot decode further.
//!
//! **Cut where the channel was measured.** The symbol is taken 144 samples
//! after the long field's first, with the timing and derotation the channel
//! estimate used: a timing error inside the guard interval turns both
//! alike and cancels when one is divided by the other.
//!
//! **The pilots take out what the offset's error left**, a phase common to
//! every tone, before any bit is read; and each tone's reading is weighted
//! by its own certainty, |H|^2 over its noise, so a tone an echo notched
//! counts for less.
//!
//! **The tail is a check.** Decoded unterminated, a garbled symbol has to
//! land six zeros besides even parity and a rate the standard lists: about
//! one in 200 does. A terminated decode forces the tail and loses the
//! check, and then a quarter of garbled symbols pass as a rate and a
//! length nobody sent. It reads more fields right near 0 dB (91 % against
//! 68 %), but only by accepting those; held to as little noise accepted
//! (its re-encoded word within 6 of the hard readings), it read no more
//! than the tail check does (63 % at 0 dB, 96 % at 2 dB against 95 %). So
//! the standard's own check it is, with no threshold to set (the ignored
//! test at the foot measures it).

use num_complex::Complex;
use rustfft::FftPlanner;

use super::bcc;
use super::channel::{estimate, Floor};
use super::interleave::deinterleave;
use super::map::demap_soft;
use super::ofdm::{data_subcarriers, polarity, PILOTS};
use super::signal_field::{Coding, Rate};
use super::sync::Sync;

/// The sample rate everything here is at (a 20 MHz channel, 17.3.2.4).
const FS: f64 = 20e6;

/// Where the SIGNAL symbol's transform starts, after the long training
/// field's first sample: its two symbols and the symbol's 0.8 us guard.
pub const SIGNAL_AT: usize = 128 + 16;

/// What a frame's L-SIG says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LSig {
    pub rate: Rate,
    /// LENGTH, in octets.
    pub length: u16,
}

/// Why an L-SIG was not accepted, in the order the checks are made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// Bits 0 to 17 do not have even parity (17.3.4.4).
    Parity,
    /// The RATE bits are none of Table 17-6's eight.
    Rate,
    /// The six tail bits are not all zero.
    Tail,
}

impl Refused {
    /// The check that refused it, in a few words.
    pub fn label(self) -> &'static str {
        match self {
            Refused::Parity => "parity",
            Refused::Rate => "no such rate",
            Refused::Tail => "tail not zero",
        }
    }
}

impl LSig {
    /// The DATA symbols it announces (Equation 17-11): SERVICE, the PSDU
    /// and the tail, in whole symbols.
    pub fn n_sym(&self) -> usize {
        (16 + 8 * self.length as usize + 6).div_ceil(self.rate.n_dbps())
    }

    /// The frame's time on the air, in microseconds (Equation 17-29): 16
    /// of training, 4 of SIGNAL, 4 a DATA symbol. The 6 us of silence the
    /// MAC counts after an ERP-OFDM frame (18.5.3.2) is not on the air and
    /// not in it.
    pub fn airtime_us(&self) -> u32 {
        20 + 4 * self.n_sym() as u32
    }

    /// The field's 24 bits, checked as a receiver checks them; the
    /// reserved bit is ignored on receive (17.3.4.4).
    fn from_bits(bits: &[bool]) -> Result<Self, Refused> {
        if bits[..18].iter().filter(|&&b| b).count() % 2 == 1 {
            return Err(Refused::Parity);
        }
        let rate = Rate::from_bits([bits[0], bits[1], bits[2], bits[3]]).ok_or(Refused::Rate)?;
        if bits[18..24].iter().any(|&b| b) {
            return Err(Refused::Tail);
        }
        let length = (0..12).fold(0u16, |l, i| l | (bits[5 + i] as u16) << i);
        Ok(Self { rate, length })
    }
}

/// Where tone `k`, -26 to 26 without 0, sits in [`super::channel::TONES`]
/// order.
fn tone(k: i32) -> usize {
    if k < 0 {
        (k + 26) as usize
    } else {
        (k + 25) as usize
    }
}

/// The SIGNAL symbol's 48 soft readings, deinterleaved into the order the
/// encoder sent them, positive for a 1. `None` unless the long field and
/// the symbol are both held.
fn readings(samples: &[Complex<f32>], first_pair: u64, sync: &Sync) -> Option<Vec<f32>> {
    let offset = sync.offset.value();
    let channel = estimate(samples, first_pair, sync.ltf_start, offset, &Floor::flat())?;
    let at = (sync.ltf_start.checked_sub(first_pair)? as usize).checked_add(SIGNAL_AT)?;
    let symbol = samples.get(at..at + 64)?;
    let turn = -std::f64::consts::TAU * offset / FS;
    let mut y: Vec<Complex<f64>> = symbol
        .iter()
        .enumerate()
        .map(|(n, z)| {
            Complex::new(z.re as f64, z.im as f64)
                * Complex::from_polar(1.0, turn * (SIGNAL_AT + n) as f64)
        })
        .collect();
    FftPlanner::new().plan_fft_forward(64).process(&mut y);
    let bin = |k: i32| y[k.rem_euclid(64) as usize];

    // The common phase, from the pilots against what they should read
    // through their channel.
    let p0 = polarity(0);
    let common = PILOTS
        .iter()
        .map(|&(k, v)| bin(k) * (channel.h[tone(k)] * v * p0).conj())
        .sum::<Complex<f64>>();
    let right = if common.norm() > 0.0 {
        common.conj() / common.norm()
    } else {
        Complex::new(1.0, 0.0)
    };

    let mut soft = Vec::with_capacity(48);
    for k in data_subcarriers() {
        let (h, v) = (channel.h[tone(k)], channel.variance[tone(k)]);
        let gain = h.norm_sqr();
        if gain == 0.0 || v <= 0.0 {
            soft.push(0.0);
            continue;
        }
        let point = bin(k) * right / h;
        soft.push(demap_soft(&[point], 1)[0] * (gain / v) as f32);
    }
    Some(deinterleave(&soft, 1))
}

/// Read the L-SIG of the frame `sync` describes, from `samples` starting
/// at stream pair `first_pair`. `None` unless the long training field and
/// the SIGNAL symbol are both held.
pub fn read(
    samples: &[Complex<f32>],
    first_pair: u64,
    sync: &Sync,
) -> Option<Result<LSig, Refused>> {
    let r = readings(samples, first_pair, sync)?;
    let bits = bcc::decode(&r, Coding::Half, false)?;
    Some(LSig::from_bits(&bits))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::dsp::testkit::Rng;
    use crate::signal::dsp::uncertainty::Uncertain;
    use crate::signal::wifi::detect::StfDetector;
    use crate::signal::wifi::ppdu::{long_training, ppdu, short_training, signal_from_bits};
    use crate::signal::wifi::signal_field::{self, Rate};
    use crate::signal::wifi::sync::{examine, Verdict};

    const RATES: [Rate; 8] = [
        Rate::R6,
        Rate::R9,
        Rate::R12,
        Rate::R18,
        Rate::R24,
        Rate::R36,
        Rate::R48,
        Rate::R54,
    ];

    /// Where frames start in the streams built here.
    const LEAD: usize = 700;

    /// The two training fields and a SIGNAL symbol carrying `bits`, with
    /// nothing after: the DATA field is not the L-SIG's business.
    fn header(bits: &[bool; 24]) -> Vec<Complex<f64>> {
        let mut out = vec![Complex::new(0.0, 0.0); 401];
        for (at, field) in [
            (0, short_training()),
            (160, long_training()),
            (320, signal_from_bits(bits)),
        ] {
            for (o, v) in out[at..].iter_mut().zip(&field) {
                *o += v;
            }
        }
        out
    }

    /// `frame` through `taps`, `LEAD` samples into noise `snr_db` below it.
    fn on_air(
        frame: &[Complex<f64>],
        taps: &[Complex<f64>],
        snr_db: f64,
        rng: &mut Rng,
    ) -> Vec<Complex<f32>> {
        let power = frame[..320].iter().map(|z| z.norm_sqr()).sum::<f64>() / 320.0;
        let scale = 1.0 / power.sqrt();
        let mut y = vec![Complex::new(0.0, 0.0); LEAD + frame.len() + 600];
        for (n, z) in frame.iter().enumerate() {
            for (d, t) in taps.iter().enumerate() {
                y[LEAD + n + d] += z * t * scale;
            }
        }
        let noise = rng.noise(y.len(), 10f64.powf(-snr_db / 10.0));
        y.iter()
            .zip(noise)
            .map(|(z, n)| Complex::new(z.re as f32, z.im as f32) + n)
            .collect()
    }

    /// Found, synchronised and read, through the receive chain as it runs.
    fn received(x: &[Complex<f32>]) -> Option<Result<LSig, Refused>> {
        let peak = StfDetector::new().push(0, x).first()?.peak;
        let Some(Verdict::ThisChannel(s)) = examine(x, 0, peak) else {
            return None;
        };
        read(x, 0, &s)
    }

    /// A frame at `LEAD`, synchronised as the timing tests show it is: the
    /// long field's first symbol 192 samples in, no offset left.
    fn placed() -> Sync {
        Sync {
            ltf_start: (LEAD + 192) as u64,
            offset: Uncertain::from_sigma(0.0, 1.0),
            snr: 100.0,
        }
    }

    /// Lengths that set every bit of the field once, and its ends.
    fn lengths() -> Vec<u16> {
        let mut out: Vec<u16> = (0..12).map(|i| 1 << i).collect();
        out.extend([3, 100, 1500, 2346, 4095]);
        out
    }

    /// Every rate and every length bit, read back at 10 dB through the
    /// chain as it runs.
    #[test]
    fn every_rate_and_length_is_read_back() {
        let mut rng = Rng::new(80);
        for rate in RATES {
            for length in lengths() {
                let x = on_air(
                    &header(&signal_field::bits(rate, length)),
                    &[Complex::new(1.0, 0.0)],
                    10.0,
                    &mut rng,
                );
                assert_eq!(
                    received(&x),
                    Some(Ok(LSig { rate, length })),
                    "{rate:?} {length}"
                );
            }
        }
    }

    /// Through a 250 ns echo at half amplitude, which notches some tones:
    /// the equalisation and the weights carry it at 12 dB.
    #[test]
    fn an_echo_is_equalised() {
        let mut taps = vec![Complex::new(0.0, 0.0); 6];
        taps[0] = Complex::new(1.0, 0.0);
        taps[5] = Complex::from_polar(0.5, 2.0);
        let mut rng = Rng::new(81);
        for rate in RATES {
            for length in [1u16, 100, 4095] {
                let x = on_air(
                    &header(&signal_field::bits(rate, length)),
                    &taps,
                    12.0,
                    &mut rng,
                );
                assert_eq!(
                    received(&x),
                    Some(Ok(LSig { rate, length })),
                    "{rate:?} {length}"
                );
            }
        }
    }

    /// The SIGNAL symbol turned by 2 rad against the long field (a phase
    /// the offset's estimate left, made large): the pilots right it.
    #[test]
    fn the_pilots_right_a_turned_symbol() {
        let mut rng = Rng::new(82);
        let mut x = on_air(
            &header(&signal_field::bits(Rate::R24, 777)),
            &[Complex::new(1.0, 0.0)],
            25.0,
            &mut rng,
        );
        let turn = Complex::from_polar(1.0f32, 2.0);
        for z in &mut x[LEAD + 320..] {
            *z *= turn;
        }
        assert_eq!(
            read(&x, 0, &placed()),
            Some(Ok(LSig {
                rate: Rate::R24,
                length: 777
            }))
        );
    }

    /// A field that fails a check is refused by that check: the parity,
    /// a rate code outside Table 17-6, a tail that is not zero. The
    /// reserved bit is ignored on receive (17.3.4.4).
    #[test]
    fn each_check_refuses_its_own_fault() {
        let mut rng = Rng::new(83);
        let good = signal_field::bits(Rate::R12, 321);
        let flip = |bits: &[bool; 24], i: usize| {
            let mut b = *bits;
            b[i] = !b[i];
            b
        };
        let parity_kept = |bits: [bool; 24]| {
            let mut b = bits;
            b[17] = b[..17].iter().filter(|&&x| x).count() % 2 == 1;
            b
        };
        // RATE 0000 is no rate; its parity made even again.
        let mut no_rate = good;
        no_rate[..4].copy_from_slice(&[false; 4]);
        let cases = [
            (flip(&good, 17), Err(Refused::Parity)),
            (parity_kept(no_rate), Err(Refused::Rate)),
            (flip(&good, 20), Err(Refused::Tail)),
            (
                parity_kept(flip(&good, 4)),
                Ok(LSig {
                    rate: Rate::R12,
                    length: 321,
                }),
            ),
        ];
        for (bits, want) in cases {
            let x = on_air(&header(&bits), &[Complex::new(1.0, 0.0)], 25.0, &mut rng);
            assert_eq!(read(&x, 0, &placed()), Some(want), "{bits:?}");
        }
    }

    /// Noise where the SIGNAL symbol should be, behind a real preamble, is
    /// refused: a garbled field passes parity, a rate and a zero tail about
    /// once in 256.
    #[test]
    fn noise_in_place_of_the_field_is_refused() {
        let mut rng = Rng::new(84);
        let mut frame = header(&[false; 24]);
        frame.truncate(321);
        let mut accepted = 0;
        let trials = 2000;
        for _ in 0..trials {
            let x = on_air(&frame, &[Complex::new(1.0, 0.0)], 20.0, &mut rng);
            if let Some(Ok(_)) = read(&x, 0, &placed()) {
                accepted += 1;
            }
        }
        assert!(accepted * 100 < trials, "{accepted} of {trials} accepted");
    }

    /// The symbols and the airtime as Equation 17-11 and Table 17-5 give
    /// them: the annex's frame is six DATA symbols, 44 us; every rate and
    /// length agrees with the transmitter's own count.
    #[test]
    fn the_airtime_is_the_equations() {
        let annex = LSig {
            rate: Rate::R36,
            length: 100,
        };
        assert_eq!((annex.n_sym(), annex.airtime_us()), (6, 44));
        for rate in RATES {
            for length in lengths() {
                let l = LSig { rate, length };
                let n = crate::signal::wifi::scrambler::Layout::new(rate, length as usize).n_sym;
                assert_eq!(l.n_sym(), n, "{rate:?} {length}");
                assert_eq!(l.airtime_us(), 20 + 4 * n as u32, "{rate:?} {length}");
            }
        }
    }

    /// **What checking the tail costs, measured.** Not a check: by hand,
    /// in release (`cargo test --release what_the_tail_check_costs --
    /// --ignored --nocapture`). At each SNR, the share of fields read right
    /// decoded unterminated with the tail checked, against terminated with
    /// only parity and rate to check; and on noise, how often each accepts.
    #[test]
    #[ignore]
    fn what_the_tail_check_costs() {
        let both = |r: &[f32]| {
            let open = bcc::decode(r, Coding::Half, false).map(|b| LSig::from_bits(&b));
            let closed = bcc::decode(r, Coding::Half, true).map(|mut b| {
                // Terminated, the tail is zero by construction.
                b[18..].fill(false);
                LSig::from_bits(&b)
            });
            (open, closed)
        };
        let mut rng = Rng::new(86);
        let trials = 4000;
        for snr in [-2.0, -1.0, 0.0, 1.0, 2.0, 4.0] {
            let (mut open_ok, mut closed_ok) = (0, 0);
            for i in 0..trials {
                let rate = RATES[i % 8];
                let length = (i * 37 % 4095 + 1) as u16;
                let want = Some(Ok(LSig { rate, length }));
                let x = on_air(
                    &header(&signal_field::bits(rate, length)),
                    &[Complex::new(1.0, 0.0)],
                    snr,
                    &mut rng,
                );
                let r = readings(&x, 0, &placed()).unwrap();
                let (open, closed) = both(&r);
                open_ok += (open == want) as usize;
                closed_ok += (closed == want) as usize;
            }
            eprintln!(
                "{snr:+.0} dB: unterminated, tail checked {:.2} %; terminated {:.2} %",
                100.0 * open_ok as f64 / trials as f64,
                100.0 * closed_ok as f64 / trials as f64
            );
        }
        let mut frame = header(&[false; 24]);
        frame.truncate(321);
        let (mut open_yes, mut closed_yes) = (0, 0);
        for _ in 0..trials {
            let x = on_air(&frame, &[Complex::new(1.0, 0.0)], 20.0, &mut rng);
            let r = readings(&x, 0, &placed()).unwrap();
            let (open, closed) = both(&r);
            open_yes += matches!(open, Some(Ok(_))) as usize;
            closed_yes += matches!(closed, Some(Ok(_))) as usize;
        }
        eprintln!(
            "noise accepted: unterminated, tail checked {:.2} %; terminated {:.2} %",
            100.0 * open_yes as f64 / trials as f64,
            100.0 * closed_yes as f64 / trials as f64
        );
    }

    /// **What a recording's L-SIGs say, by hand**
    /// (`SDRTOP_REPLAY=path.sigmf-data`, ci8 at 20 Msps on a channel
    /// centre): every frame of this channel read offline, counted by rate
    /// and by length, with the refusals by check. `cargo test --release
    /// a_recordings_l_sigs -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn a_recordings_l_sigs() {
        let Ok(path) = std::env::var("SDRTOP_REPLAY") else {
            return;
        };
        let bytes = std::fs::read(&path).unwrap();
        let pairs = bytes.len() / 2;
        let pair = |p: &[u8; 2]| Complex::new(p[0] as i8 as f32 / 128.0, p[1] as i8 as f32 / 128.0);
        // The radio's DC taken off, as the worker's feed does: the long
        // field has no DC tone, and the HackRF's spike would sit in every
        // window the gate fits.
        // Summed in f64: hundreds of millions of f32 additions lose it.
        let sum = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| {
                let z = pair(p);
                Complex::new(z.re as f64, z.im as f64)
            })
            .sum::<Complex<f64>>()
            / pairs as f64;
        let dc = Complex::new(sum.re as f32, sum.im as f32);
        // Converted a stretch at a time: the whole recording as floats
        // would be four times its size.
        let stretch = |from: usize, len: usize| -> Vec<Complex<f32>> {
            bytes[2 * from..2 * (from + len).min(pairs)]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|p| pair(p) - dc)
                .collect()
        };
        let mut by_rate = std::collections::BTreeMap::new();
        let mut by_length = std::collections::BTreeMap::new();
        let mut refused = std::collections::BTreeMap::new();
        let mut airtime = 0u64;
        let mut detector = StfDetector::new();
        for at in (0..pairs).step_by(131_072) {
            for d in detector.push(at as u64, &stretch(at, 131_072)) {
                let Some((from, len)) = crate::signal::wifi::sync::needs(d.peak) else {
                    continue;
                };
                let x = stretch(from as usize, len);
                if let Some(Verdict::ThisChannel(s)) = examine(&x, from, d.peak) {
                    match read(&x, from, &s) {
                        Some(Ok(l)) => {
                            *by_rate.entry(l.rate.mbps()).or_insert(0) += 1;
                            let bucket = match l.length {
                                0..=31 => "0-31",
                                32..=127 => "32-127",
                                128..=511 => "128-511",
                                512..=1599 => "512-1599",
                                _ => "1600+",
                            };
                            *by_length.entry((l.rate.mbps(), bucket)).or_insert(0) += 1;
                            airtime += l.airtime_us() as u64;
                        }
                        Some(Err(why)) => *refused.entry(why.label()).or_insert(0) += 1,
                        None => {}
                    }
                }
            }
        }
        let secs = pairs as f64 / FS;
        eprintln!("{secs:.1} s; L-SIG rates (Mb/s: frames) {by_rate:?}");
        eprintln!("by rate and length {by_length:?}");
        eprintln!("refused {refused:?}");
        eprintln!(
            "airtime {:.1} ms, {:.2} % of the recording",
            airtime as f64 / 1e3,
            airtime as f64 / 1e4 / secs
        );
    }

    /// Frames one after another, of every rate and assorted lengths, found
    /// in one stream: their airtimes add up to the time they took, exactly.
    #[test]
    fn a_sequence_s_airtime_adds_up() {
        let mut rng = Rng::new(85);
        let psdu = "Liszt Ferenc: II. magyar rapszódia, S.244/2".as_bytes();
        let mut stream: Vec<Complex<f64>> = vec![Complex::new(0.0, 0.0); 500];
        let mut took_us = 0u32;
        for (i, rate) in RATES.iter().enumerate() {
            let body = &psdu[..10 + 4 * i];
            let f = ppdu(body, *rate, 0x5d);
            // Each frame's samples are 20 a microsecond, less the window's
            // closing sample.
            took_us += (f.len() as u32 - 1) / 20;
            stream.extend(f);
            stream.extend(vec![Complex::new(0.0, 0.0); 300 + 37 * i]);
        }
        // At 20 dB, the frames scaled to unit power.
        let training = &stream[500..820];
        let power = training.iter().map(|z| z.norm_sqr()).sum::<f64>() / 320.0;
        let noise = rng.noise(stream.len(), 0.01);
        let x: Vec<Complex<f32>> = stream
            .iter()
            .zip(noise)
            .map(|(z, n)| {
                let z = z / power.sqrt();
                Complex::new(z.re as f32, z.im as f32) + n
            })
            .collect();
        let mut sum = 0u32;
        let mut frames = 0;
        for d in StfDetector::new().push(0, &x) {
            if let Some(Verdict::ThisChannel(s)) = examine(&x, 0, d.peak) {
                if let Some(Ok(l)) = read(&x, 0, &s) {
                    sum += l.airtime_us();
                    frames += 1;
                }
            }
        }
        assert_eq!(frames, RATES.len());
        assert_eq!(sum, took_us);
    }
}
