// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The bit chain both ways: a PSDU to the constellation points of its DATA
//! symbols and back, through noise. What the receiver's decoding half will
//! do once its earlier stages (finding the frame, its timing, its offset,
//! the channel) hand it points; here the points come straight from the
//! transmit side, so what is tested is the coding, not the radio.

use num_complex::Complex;

use super::bcc::{decode, encode, puncture};
use super::interleave::{deinterleave, interleave};
use super::map::{demap_soft, map};
use super::scrambler::{apply, scrambled, sequence};
use super::signal_field::Rate;

/// The DATA field's points, one symbol's 48 at a time.
pub fn points(psdu: &[u8], rate: Rate, initial: u8) -> Vec<Vec<Complex<f64>>> {
    let coded = puncture(&encode(&scrambled(psdu, rate, initial)), rate.coding());
    coded
        .chunks(rate.n_cbps())
        .map(|c| map(&interleave(c, rate.n_bpsc()), rate.n_bpsc()))
        .collect()
}

/// The PSDU of `length` octets back from its symbols' points: soft
/// readings, deinterleaved, decoded, then descrambled from the seed the
/// SERVICE field's first seven bits give away (they are zeros scrambled,
/// so they are the sequence itself, 17.3.5.2).
pub fn psdu(symbols: &[Vec<Complex<f64>>], rate: Rate, length: usize) -> Option<Vec<u8>> {
    let readings: Vec<f32> = symbols
        .iter()
        .flat_map(|s| deinterleave(&demap_soft(s, rate.n_bpsc()), rate.n_bpsc()))
        .collect();
    let bits = decode(&readings, rate.coding(), false)?;
    let initial = bits[..7]
        .iter()
        .enumerate()
        .fold(0u8, |v, (i, &b)| v | (b as u8) << i);
    let plain = apply(&bits, &sequence(initial, bits.len()));
    let body = plain.get(16..16 + 8 * length)?;
    Some(
        body.chunks(8)
            .map(|o| {
                o.iter()
                    .enumerate()
                    .fold(0u8, |v, (i, &b)| v | (b as u8) << i)
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::dsp::code::conv::{correlation, hamming};
    use crate::signal::dsp::testkit::Rng;
    use crate::signal::wifi::bcc::CODE;

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

    /// Liszt's Second Hungarian Rhapsody, by name: our own first frame.
    const LISZT: &str = "Liszt Ferenc: II. magyar rapszódia, S.244/2";

    /// Points with complex Gaussian noise `snr_db` below their unit power.
    fn noisy(symbols: &[Vec<Complex<f64>>], snr_db: f64, rng: &mut Rng) -> Vec<Vec<Complex<f64>>> {
        let power = 10f64.powf(-snr_db / 10.0);
        symbols
            .iter()
            .map(|s| {
                let n = rng.noise(s.len(), power);
                s.iter()
                    .zip(n)
                    .map(|(p, n)| p + Complex::new(n.re as f64, n.im as f64))
                    .collect()
            })
            .collect()
    }

    /// Liszt's name goes out and comes back at every rate, clean and
    /// through noise 25 dB down.
    #[test]
    fn liszt_round_trips_at_every_rate() {
        let mut rng = Rng::new(244);
        for rate in RATES {
            let sent = points(LISZT.as_bytes(), rate, 0x2a);
            let clean = psdu(&sent, rate, LISZT.len()).unwrap();
            assert_eq!(String::from_utf8(clean).unwrap(), LISZT, "{rate:?}");
            let heard = psdu(&noisy(&sent, 25.0, &mut rng), rate, LISZT.len()).unwrap();
            assert_eq!(heard, LISZT.as_bytes(), "{rate:?} through noise");
        }
    }

    /// Random frames, from one octet to a full Ethernet payload, at every
    /// rate and from every seed in turn, through noise 30 dB down.
    #[test]
    fn random_frames_round_trip_at_every_rate() {
        let mut rng = Rng::new(7);
        for (i, rate) in RATES.into_iter().enumerate() {
            for length in [1usize, 37, 100, 1500] {
                let frame: Vec<u8> = (0..length).map(|_| rng.next_u64() as u8).collect();
                let seed = 1 + (i * 17 + length) as u8 % 127;
                let sent = points(&frame, rate, seed);
                let heard = psdu(&noisy(&sent, 30.0, &mut rng), rate, length);
                assert_eq!(heard.as_deref(), Some(&frame[..]), "{rate:?} {length}");
            }
        }
    }

    /// Bit errors at `eb_n0_db` over `n` information bits in frames of 500,
    /// BPSK, uncoded and at rate 1/2 decoded soft and hard.
    fn errors(eb_n0_db: f64, n: usize, seed: u64) -> (usize, usize, usize) {
        let mut rng = Rng::new(seed);
        let eb_n0 = 10f64.powf(eb_n0_db / 10.0);
        // A coded bit carries half an information bit's energy at rate 1/2;
        // the noise is per real dimension, BPSK using one.
        let sigma_coded = (1.0 / (2.0 * 0.5 * eb_n0)).sqrt();
        let sigma_plain = (1.0 / (2.0 * eb_n0)).sqrt();
        let (mut plain, mut soft, mut hard) = (0, 0, 0);
        for _ in 0..n / 500 {
            let mut bits: Vec<bool> = (0..494).map(|_| rng.next_u64() & 1 == 1).collect();
            bits.extend([false; 6]);
            for &b in &bits[..494] {
                let (x, _) = rng.normal_pair();
                plain += usize::from(((if b { 1.0 } else { -1.0 }) + sigma_plain * x > 0.0) != b);
            }
            let r: Vec<f32> = encode(&bits)
                .iter()
                .map(|&c| {
                    let (x, _) = rng.normal_pair();
                    ((if c { 1.0 } else { -1.0 }) + sigma_coded * x) as f32
                })
                .collect();
            let h: Vec<bool> = r.iter().map(|&v| v > 0.0).collect();
            let s = CODE
                .decode(500, true, |k, a, b| correlation(&r, k, a, b))
                .unwrap();
            let d = CODE
                .decode(500, true, |k, a, b| hamming(&h, k, a, b))
                .unwrap();
            soft += bits[..494].iter().zip(&s).filter(|(a, b)| a != b).count();
            hard += bits[..494].iter().zip(&d).filter(|(a, b)| a != b).count();
        }
        (plain, soft, hard)
    }

    /// **The code earns its rate.** At Eb/N0 = 4 dB uncoded BPSK makes about
    /// one error in eighty; the code, decoded soft, at least fifty times
    /// fewer over the same 20 000 bits and the same noise generator.
    #[test]
    fn the_code_cuts_errors_fifty_fold_at_four_db() {
        let (plain, soft, _) = errors(4.0, 20_000, 1);
        assert!(plain > 150, "uncoded {plain}");
        assert!(soft * 50 < plain, "uncoded {plain}, coded {soft}");
    }

    /// **Soft decisions beat hard ones by a measured margin**: at 3 dB, the
    /// decoder that weighs each reading makes at most a third of the errors
    /// of the one that slices it first.
    #[test]
    fn soft_decisions_beat_hard_ones_at_three_db() {
        let (_, soft, hard) = errors(3.0, 20_000, 2);
        assert!(hard >= 10, "hard {hard}");
        assert!(soft * 3 <= hard, "soft {soft}, hard {hard}");
    }

    /// The code's free distance, the least weight of a path that leaves the
    /// zero state and returns, found by search over its own trellis: a
    /// state is the last six inputs, newest highest; a step with `input`
    /// reads the window `input << 6 | state` through the two generators.
    fn free_distance() -> u32 {
        let weight =
            |window: u32| (window & 0o133).count_ones() % 2 + (window & 0o171).count_ones() % 2;
        // Leaving zero: a one in, from state 0.
        let mut best = vec![u32::MAX; 64];
        best[32] = weight(1 << 6);
        let mut free = u32::MAX;
        for _ in 0..200 {
            let mut next = vec![u32::MAX; 64];
            for (s, &w) in best.iter().enumerate() {
                if w == u32::MAX {
                    continue;
                }
                for input in 0..2u32 {
                    let window = input << 6 | s as u32;
                    let total = w + weight(window);
                    let to = (window >> 1) as usize;
                    if to == 0 {
                        free = free.min(total);
                    } else {
                        next[to] = next[to].min(total);
                    }
                }
            }
            best = next;
        }
        free
    }

    /// The free distance is 10 (so the asymptotic coding gain of soft
    /// decoding, 10 log10(R d_free), is 7.0 dB at rate 1/2).
    #[test]
    fn the_free_distance_is_ten() {
        assert_eq!(free_distance(), 10);
    }

    /// **The curve, by hand**: bit error rate against Eb/N0 for uncoded
    /// BPSK and the code decoded soft and hard, and the coding gain read
    /// off at 1e-4 beside the asymptotic 10 log10(R d_free); then what one
    /// decoded bit costs, the figure the DATA field's budget is set from.
    /// `cargo test --release ber_curve -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn ber_curve_and_cost() {
        let n = 400_000;
        eprintln!("Eb/N0 dB   uncoded     soft        hard");
        let mut rows = Vec::new();
        for tenth in (10..=90).step_by(5) {
            let db = tenth as f64 / 10.0;
            let (p, s, h) = errors(db, n, 100 + tenth as u64);
            let ber = |e: usize| e as f64 / (n / 500 * 494) as f64;
            eprintln!(
                "{db:5.1}     {:9.2e}   {:9.2e}   {:9.2e}",
                ber(p),
                ber(s),
                ber(h)
            );
            rows.push((db, ber(p), ber(s)));
        }
        let at = |f: &dyn Fn(&(f64, f64, f64)) -> f64| -> Option<f64> {
            rows.windows(2).find_map(|w| {
                let (a, b) = (f(&w[0]), f(&w[1]));
                (a >= 1e-4 && b < 1e-4 && b > 0.0).then(|| {
                    let t = (a.log10() - (-4.0)) / (a.log10() - b.log10());
                    w[0].0 + t * (w[1].0 - w[0].0)
                })
            })
        };
        let gain = at(&|r| r.1).zip(at(&|r| r.2)).map(|(u, c)| u - c);
        let asymptotic = 10.0 * (0.5 * free_distance() as f64).log10();
        eprintln!("coding gain at 1e-4: {gain:?} dB; asymptotic {asymptotic:.1} dB");

        let frame: Vec<bool> = (0..1500 * 8).map(|i| i % 3 == 0).collect();
        let r: Vec<f32> = encode(&frame)
            .iter()
            .map(|&b| if b { 1.0 } else { -1.0 })
            .collect();
        let start = std::time::Instant::now();
        for _ in 0..20 {
            std::hint::black_box(CODE.decode(frame.len(), false, |k, a, b| {
                correlation(std::hint::black_box(&r), k, a, b)
            }));
        }
        let per_bit = start.elapsed().as_secs_f64() / (20 * frame.len()) as f64;
        eprintln!(
            "decode: {:.1} ns a bit, {:.1} Mbit/s; 54 Mb/s needs {:.2} of a core",
            per_bit * 1e9,
            1e-6 / per_bit,
            54e6 * per_bit
        );
    }
}
