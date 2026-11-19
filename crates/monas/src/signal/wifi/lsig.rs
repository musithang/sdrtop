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
//! The symbol is received as every signalling symbol is ([`super::sig`]):
//! equalised by the long field's estimate, righted by its pilots, each
//! tone weighted by its own certainty.
//!
//! **The tail is a check.** Decoded unterminated, a garbled symbol has to
//! land six zeros besides even parity and a rate the standard lists: about
//! one in 200 does. A terminated decode forces the tail and loses the
//! check, and then a quarter of garbled symbols pass as a rate and a
//! length nobody sent. Held to as little noise accepted by a threshold on
//! how far its re-encoded word lies from the hard readings (within 7 of 48,
//! 0.5 % of noise accepted), the terminated decode reads about half a dB
//! lower (92 % at 0 dB against 86 %); but that threshold is a number found
//! by trying, where the tail is the standard's own check with a stated
//! rate. The tail it is (the ignored test at the foot measures it).

#[cfg(test)]
use num_complex::Complex;

use super::bcc;
use super::interleave::deinterleave;
use super::signal_field::{Coding, Rate};
#[cfg(test)]
use super::sync::Sync;

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
    pub fn from_bits(bits: &[bool]) -> Result<Self, Refused> {
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

/// The L-SIG from its symbol's readings in subcarrier order (BPSK,
/// [`super::sig::Symbol::readings`] at no rotation): deinterleaved,
/// decoded unterminated and checked.
#[cfg(test)]
pub fn from_readings(r: &[f32]) -> Result<LSig, Refused> {
    LSig::from_bits(&bits(r))
}

/// The 24 bits the readings decode to, unterminated, before any check.
pub fn bits(r: &[f32]) -> Vec<bool> {
    // A symbol's 48 readings always decode to 24 bits; the decoder refuses
    // only an empty or odd count, which no symbol gives, and a field of
    // ones fails the tail check.
    bcc::decode(&deinterleave(r, 1), Coding::Half, false).unwrap_or_else(|| vec![true; 24])
}

/// The SIGNAL symbol's 48 soft readings, in subcarrier order. `None`
/// unless the long field and the symbol are both held.
#[cfg(test)]
fn readings(samples: &[Complex<f32>], first_pair: u64, sync: &Sync) -> Option<Vec<f32>> {
    Some(
        super::sig::header(samples, first_pair, sync)?
            .symbol(0)?
            .readings(Complex::new(1.0, 0.0)),
    )
}

/// Read the L-SIG of the frame `sync` describes, from `samples` starting
/// at stream pair `first_pair`. `None` unless the long training field and
/// the SIGNAL symbol are both held.
#[cfg(test)]
pub fn read(
    samples: &[Complex<f32>],
    first_pair: u64,
    sync: &Sync,
) -> Option<Result<LSig, Refused>> {
    Some(from_readings(&readings(samples, first_pair, sync)?))
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
    /// long field's first symbol 192 samples in, no offset left, and the
    /// offset's uncertainty what the synchronisation states at 10 dB.
    fn placed() -> Sync {
        Sync {
            ltf_start: (LEAD + 192) as u64,
            offset: Uncertain::from_sigma(0.0, 2_000.0),
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

    /// A 50 kHz offset the synchronisation did not take out, but said it
    /// might have (its uncertainty 50 kHz): by the SIGNAL symbol it has
    /// turned the carrier 1.8 rad, and the pilots right it.
    #[test]
    fn the_pilots_right_what_the_offset_left() {
        let mut rng = Rng::new(82);
        let mut x = on_air(
            &header(&signal_field::bits(Rate::R24, 777)),
            &[Complex::new(1.0, 0.0)],
            25.0,
            &mut rng,
        );
        crate::signal::dsp::nco::Nco::new(50e3, 20e6).mix(&mut x);
        let sync = Sync {
            offset: Uncertain::from_sigma(0.0, 50e3),
            ..placed()
        };
        assert_eq!(
            read(&x, 0, &sync),
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
    /// decoded unterminated with the tail checked, against terminated and
    /// held to its word within a few disagreements; and on noise, how often
    /// each accepts, beside the union bound on the terminated decode's.
    #[test]
    #[ignore]
    fn what_the_tail_check_costs() {
        // Terminated, then held to its word: the re-encoded field against
        // the readings' signs, accepted within `d` disagreements.
        let within = |r: &[f32], d: usize| {
            bcc::decode(r, Coding::Half, true).and_then(|mut b| {
                b[18..].fill(false);
                let far = bcc::encode(&b)
                    .iter()
                    .zip(r)
                    .filter(|(c, v)| **c != (**v > 0.0))
                    .count();
                (far <= d).then(|| LSig::from_bits(&b))
            })
        };
        const DS: [usize; 3] = [5, 6, 7];
        let all = |r: &[f32]| {
            let open = bcc::decode(r, Coding::Half, false).map(|b| LSig::from_bits(&b));
            let closed: Vec<_> = DS.iter().map(|d| within(r, *d)).collect();
            (open, closed)
        };
        let mut rng = Rng::new(86);
        let trials = 4000;
        for snr in [-2.0, -1.0, 0.0, 1.0, 2.0, 4.0] {
            let mut open_ok = 0;
            let mut closed_ok = [0; 3];
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
                let r = deinterleave(&readings(&x, 0, &placed()).unwrap(), 1);
                let (open, closed) = all(&r);
                open_ok += (open == want) as usize;
                for (n, c) in closed.iter().enumerate() {
                    closed_ok[n] += (*c == want) as usize;
                }
            }
            eprintln!(
                "{snr:+.0} dB: tail checked {:.2} %; terminated within {DS:?}: {:?} %",
                100.0 * open_ok as f64 / trials as f64,
                closed_ok.map(|k| (10000.0 * k as f64 / trials as f64).round() / 100.0)
            );
        }
        let mut frame = header(&[false; 24]);
        frame.truncate(321);
        let mut open_yes = 0;
        let mut closed_yes = [0; 3];
        let noise_trials = 20000;
        for _ in 0..noise_trials {
            let x = on_air(&frame, &[Complex::new(1.0, 0.0)], 20.0, &mut rng);
            let r = deinterleave(&readings(&x, 0, &placed()).unwrap(), 1);
            let (open, closed) = all(&r);
            open_yes += matches!(open, Some(Ok(_))) as usize;
            for (n, c) in closed.iter().enumerate() {
                closed_yes[n] += matches!(c, Some(Ok(_))) as usize;
            }
        }
        // Union bound: 2^18 words, each within d of 48 coin flips, then
        // parity and a supported rate (a half each).
        let bound = DS.map(|d| {
            let tail: f64 = (0..=d)
                .map(|k| (0..k).fold(1.0, |c, j| c * (48 - j) as f64 / (j + 1) as f64))
                .sum::<f64>()
                / 2f64.powi(48);
            (10000.0 * (2f64.powi(18) * tail / 4.0)).round() / 100.0
        });
        eprintln!(
            "noise accepted: tail checked {:.2} %; terminated within {DS:?}: {:?} %, bound {bound:?} %",
            100.0 * open_yes as f64 / noise_trials as f64,
            closed_yes.map(|k| (10000.0 * k as f64 / noise_trials as f64).round() / 100.0)
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
