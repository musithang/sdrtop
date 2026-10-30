// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The (174,91) LDPC code that protects the 91-bit message-plus-CRC word
//! (the QEX paper, section 3), and our decoder for it.
//!
//! **The code is two tables, kept as published.** `ldpc_generator.dat` (83
//! rows of 91 bits: parity bit `i` is the modulo-2 sum of the message bits
//! row `i` marks) and `ldpc_parity.dat` (for each of the 174 codeword bits,
//! the three of the 83 checks it takes part in) are the paper's reference 14
//! files, public domain, unaltered; they are read at first use. That the two
//! agree, and that the codeword is laid out as this module assumes (the 91
//! message bits first, then the 83 parity bits), is not assumed: a test
//! encodes every unit message and checks it against every parity check.
//!
//! **Decoding, in two stages.**
//!
//! 1. Belief propagation (the sum-product algorithm, in the log domain) on
//!    the parity checks. It converges to a codeword or gives up after a
//!    bounded number of iterations.
//! 2. When it gives up, or converges to a codeword whose CRC fails, ordered
//!    statistics decoding (Fossorier and Lin's OSD, the method the paper's
//!    section 6 names): the 91 most reliable independent bits are taken as
//!    they were received and re-encoded, then the same with one, or two of
//!    the least reliable among them, flipped. Of the candidates whose CRC
//!    passes, the one closest to what was received wins, and only if it is
//!    close enough (see [`OSD_MAX_DISTANCE`]).
//!
//! **What a decode cost is reported** (POLICY.md, rule 3): the iterations
//! belief propagation took, how many of the 174 received bits the codeword
//! disagrees with, and the OSD order if OSD was needed.
//!
//! **The sign convention.** A log-likelihood ratio here is `ln P(1) / P(0)`:
//! positive means the bit is more likely a one.

use std::sync::OnceLock;

use super::crc;

/// Codeword bits.
pub const N: usize = 174;
/// Message-plus-CRC bits.
pub const K: usize = 91;
/// Parity bits, and parity checks.
pub const M: usize = 83;

/// A codeword as three words of bits: bit `i` of the codeword is bit `i % 64`
/// of word `i / 64`.
type Bits = [u64; 3];

fn get(b: &Bits, i: usize) -> bool {
    b[i / 64] >> (i % 64) & 1 == 1
}

fn set(b: &mut Bits, i: usize) {
    b[i / 64] |= 1 << (i % 64);
}

fn xor(a: &mut Bits, b: &Bits) {
    for (x, y) in a.iter_mut().zip(b) {
        *x ^= y;
    }
}

struct Code {
    /// Row `i`: the message bits (bit `j` for message bit `j`, the first
    /// transmitted being 0) whose sum is parity bit `i`.
    generator: [u128; M],
    /// Check `c`: the codeword bits it sums.
    checks: Vec<Vec<usize>>,
    /// Bit `v`: its three checks.
    bit_checks: [[usize; 3]; N],
    /// The systematic generator, one row per message bit: the codeword of a
    /// message with only that bit set.
    rows: [Bits; K],
}

fn code() -> &'static Code {
    static CODE: OnceLock<Code> = OnceLock::new();
    CODE.get_or_init(|| {
        let generator = parse_generator(include_str!("ldpc_generator.dat"));
        let bit_checks = parse_parity(include_str!("ldpc_parity.dat"));
        let mut checks = vec![Vec::new(); M];
        for (v, cs) in bit_checks.iter().enumerate() {
            for &c in cs {
                checks[c].push(v);
            }
        }
        let mut rows = [[0u64; 3]; K];
        for (j, row) in rows.iter_mut().enumerate() {
            set(row, j);
            for (i, g) in generator.iter().enumerate() {
                if g >> j & 1 == 1 {
                    set(row, K + i);
                }
            }
        }
        Code {
            generator,
            checks,
            bit_checks,
            rows,
        }
    })
}

/// `ldpc_generator.dat`: a header, then 83 lines of 91 `0`/`1`, column 1
/// first.
fn parse_generator(text: &str) -> [u128; M] {
    let rows: Vec<u128> = text
        .lines()
        .map(str::trim)
        .filter(|l| l.len() == K && l.bytes().all(|b| b == b'0' || b == b'1'))
        .map(|l| {
            l.bytes()
                .enumerate()
                .fold(0u128, |acc, (j, b)| acc | (u128::from(b - b'0') << j))
        })
        .collect();
    rows.try_into().expect("83 generator rows")
}

/// `ldpc_parity.dat`: a header, then 174 lines of three check numbers, 1 to
/// 83, one line per codeword bit.
fn parse_parity(text: &str) -> [[usize; 3]; N] {
    let cols: Vec<[usize; 3]> = text
        .lines()
        .filter_map(|l| {
            let n: Vec<usize> = l
                .split_whitespace()
                .map(str::parse)
                .collect::<Result<_, _>>()
                .ok()?;
            (n.len() == 3).then(|| [n[0] - 1, n[1] - 1, n[2] - 1])
        })
        .collect();
    cols.try_into().expect("174 parity columns")
}

/// The codeword of a 91-bit message-plus-CRC word (its first bit at bit 90),
/// as 174 bits, 0 or 1, the first transmitted first.
pub fn encode(word91: u128) -> [u8; N] {
    let c = code();
    let mut out = [0u8; N];
    let mut msg = 0u128;
    for (j, bit) in out.iter_mut().take(K).enumerate() {
        *bit = (word91 >> (K - 1 - j) & 1) as u8;
        msg |= u128::from(*bit) << j;
    }
    for (i, g) in c.generator.iter().enumerate() {
        out[K + i] = ((g & msg).count_ones() & 1) as u8;
    }
    out
}

/// How many checks a 174-bit word fails.
fn failed_checks(bits: &Bits) -> usize {
    code()
        .checks
        .iter()
        .filter(|vs| vs.iter().filter(|&&v| get(bits, v)).count() % 2 == 1)
        .count()
}

/// The 91-bit word at the head of a codeword, first bit at bit 90.
fn head(bits: &Bits) -> u128 {
    (0..K).fold(0u128, |acc, j| (acc << 1) | u128::from(get(bits, j)))
}

/// What was decoded, and what it took.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Decoded {
    /// The 77-bit payload, CRC checked.
    pub payload: u128,
    /// Belief propagation's iterations; the limit when OSD finished the job.
    pub iterations: u32,
    /// Received bits the codeword disagrees with (of 174).
    pub repaired: u32,
    /// The OSD order that found it: `None` when belief propagation did.
    pub osd: Option<u8>,
    /// How far the codeword lies from what was received: the reliability
    /// of the bits it disagrees with over the total (as
    /// [`OSD_MAX_DISTANCE`]).
    pub distance: f32,
}

/// How far an OSD candidate may lie from what was received: the sum of the
/// reliabilities of the bits it disagrees with, divided by the sum over all
/// 174. With a 14-bit CRC and a couple of hundred candidates tried, about one
/// noise input in a hundred has some candidate pass the CRC, so the CRC alone
/// does not make an OSD result a message; the distance does.
///
/// Measured (`measure_the_osd_thresholds`, order 2, Eb/N0 0.5 to 2 dB, 4000
/// transmissions, 20 000 pure-noise inputs): at 0.06, 525 of the 573
/// transmissions OSD can recover come back, no wrong message, no noise
/// decoded; at 0.07, 567 and one noise decode in 20 000; at 0.11, 573 with
/// eight wrong messages and 172 noise decodes. A false call on the map is
/// worse than a weak one missed, so 0.06.
pub const OSD_MAX_DISTANCE: f32 = 0.06;

/// How many of the least reliable information bits OSD flips in pairs at
/// order 2.
const OSD2_SPAN: usize = 16;

/// Decode 174 log-likelihood ratios (`ln P(1)/P(0)`), with at most
/// `max_iterations` of belief propagation and OSD up to `osd_order` (0, 1
/// or 2; anything above 2 is 2).
pub fn decode(llr: &[f32; N], max_iterations: u32, osd_order: u8) -> Option<Decoded> {
    let mut hard = [0u64; 3];
    for (v, &l) in llr.iter().enumerate() {
        if l > 0.0 {
            set(&mut hard, v);
        }
    }
    let repaired = |bits: &Bits| -> u32 {
        bits.iter()
            .zip(&hard)
            .map(|(a, b)| (a ^ b).count_ones())
            .sum()
    };
    let total: f32 = llr.iter().map(|l| l.abs()).sum();
    let distance = |bits: &Bits| -> f32 {
        let d: f32 = (0..N)
            .filter(|&v| get(bits, v) != get(&hard, v))
            .map(|v| llr[v].abs())
            .sum();
        if total > 0.0 {
            d / total
        } else {
            0.0
        }
    };

    if let Some((bits, iterations)) = belief_propagation(llr, max_iterations) {
        let word = head(&bits);
        if crc::check(word) {
            return Some(Decoded {
                payload: word >> 14,
                iterations,
                repaired: repaired(&bits),
                osd: None,
                distance: distance(&bits),
            });
        }
    }
    let (bits, order) = osd(llr, &hard, osd_order.min(2), OSD_MAX_DISTANCE)?;
    Some(Decoded {
        payload: head(&bits) >> 14,
        iterations: max_iterations,
        repaired: repaired(&bits),
        osd: Some(order),
        distance: distance(&bits),
    })
}

/// Sum-product on the parity checks. Internally the messages are
/// `ln P(0)/P(1)`, the textbook sign, so the check update is the plain tanh
/// rule. Returns the codeword and the iterations it took (0 when the hard
/// decisions were already one).
fn belief_propagation(llr: &[f32; N], max_iterations: u32) -> Option<(Bits, u32)> {
    let c = code();
    let prior: Vec<f32> = llr.iter().map(|l| -l).collect();
    // Bit-to-check and check-to-bit messages, indexed by the bit's position
    // in its three checks.
    let mut to_check = [[0f32; 3]; N];
    let mut to_bit = [[0f32; 3]; N];
    for v in 0..N {
        to_check[v] = [prior[v]; 3];
    }
    // Where bit v sits among check c's bits, to index the messages.
    let slot = |v: usize, check: usize| -> usize {
        c.bit_checks[v]
            .iter()
            .position(|&x| x == check)
            .expect("bit in its check")
    };

    for iteration in 0..=max_iterations {
        // Posterior and hard decision.
        let mut bits = [0u64; 3];
        for v in 0..N {
            let total = prior[v] + to_bit[v].iter().sum::<f32>();
            if total < 0.0 {
                set(&mut bits, v);
            }
        }
        if failed_checks(&bits) == 0 {
            return Some((bits, iteration));
        }
        if iteration == max_iterations {
            break;
        }
        // Check update: the tanh rule over the other bits of each check.
        for (check, vs) in c.checks.iter().enumerate() {
            let t: Vec<f32> = vs
                .iter()
                .map(|&v| (to_check[v][slot(v, check)] / 2.0).tanh())
                .collect();
            for (k, &v) in vs.iter().enumerate() {
                let p: f32 = t
                    .iter()
                    .enumerate()
                    .filter(|&(i, _)| i != k)
                    .map(|(_, x)| x)
                    .product();
                let p = p.clamp(-0.999_999, 0.999_999);
                to_bit[v][slot(v, check)] = 2.0 * p.atanh();
            }
        }
        // Bit update: everything the bit knows except what this check said.
        for v in 0..N {
            let total = prior[v] + to_bit[v].iter().sum::<f32>();
            for s in 0..3 {
                to_check[v][s] = total - to_bit[v][s];
            }
        }
    }
    None
}

/// Ordered statistics decoding to `order`: the codeword and the order that
/// found it, if one passes the CRC within `max_distance` (normalised, as
/// [`OSD_MAX_DISTANCE`]) of what was received.
fn osd(llr: &[f32; N], hard: &Bits, order: u8, max_distance: f32) -> Option<(Bits, u8)> {
    let c = code();
    // Columns by reliability, most reliable first.
    let mut by_reliability: Vec<usize> = (0..N).collect();
    by_reliability.sort_by(|&a, &b| llr[b].abs().total_cmp(&llr[a].abs()));

    // Gaussian elimination over GF(2): bring the generator to identity on
    // the most reliable 91 independent columns, kept in reliability order.
    let mut rows = c.rows;
    let mut pivots: Vec<usize> = Vec::with_capacity(K);
    for &col in &by_reliability {
        if pivots.len() == K {
            break;
        }
        let p = pivots.len();
        let Some(r) = (p..K).find(|&r| get(&rows[r], col)) else {
            continue;
        };
        rows.swap(p, r);
        let pivot = rows[p];
        for (r, row) in rows.iter_mut().enumerate() {
            if r != p && get(row, col) {
                xor(row, &pivot);
            }
        }
        pivots.push(col);
    }
    if pivots.len() < K {
        return None;
    }

    // Order 0: the received decisions on the information set.
    let mut base = [0u64; 3];
    for (p, &col) in pivots.iter().enumerate() {
        if get(hard, col) {
            xor(&mut base, &rows[p]);
        }
    }
    let total: f32 = llr.iter().map(|l| l.abs()).sum();
    let distance = |bits: &Bits| -> f32 {
        (0..N)
            .filter(|&v| get(bits, v) != get(hard, v))
            .map(|v| llr[v].abs())
            .sum::<f32>()
    };

    let mut best: Option<(f32, Bits, u8)> = None;
    let mut consider = |bits: Bits, ord: u8| {
        if !crc::check(head(&bits)) {
            return;
        }
        let d = distance(&bits);
        if best.as_ref().is_none_or(|(bd, _, _)| d < *bd) {
            best = Some((d, bits, ord));
        }
    };
    consider(base, 0);
    if order >= 1 {
        for row in &rows {
            let mut cand = base;
            xor(&mut cand, row);
            consider(cand, 1);
        }
    }
    if order >= 2 {
        let span = K - OSD2_SPAN..K;
        for i in span.clone() {
            for j in (i + 1)..K {
                let mut cand = base;
                xor(&mut cand, &rows[i]);
                xor(&mut cand, &rows[j]);
                consider(cand, 2);
            }
        }
    }
    let (d, bits, ord) = best?;
    (total > 0.0 && d / total <= max_distance).then_some((bits, ord))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::dsp::testkit::Rng;

    fn random_payload(rng: &mut Rng) -> u128 {
        (u128::from(rng.next_u64()) << 64 | u128::from(rng.next_u64())) & ((1u128 << 77) - 1)
    }

    fn as_bits(cw: &[u8; N]) -> Bits {
        let mut b = [0u64; 3];
        for (v, &x) in cw.iter().enumerate() {
            if x == 1 {
                set(&mut b, v);
            }
        }
        b
    }

    /// A codeword sent as +1/-1 (one as +1) through Gaussian noise of
    /// standard deviation `sigma`, received as log-likelihood ratios.
    fn through_awgn(cw: &[u8; N], sigma: f64, rng: &mut Rng) -> [f32; N] {
        let mut llr = [0f32; N];
        for (v, l) in llr.iter_mut().enumerate() {
            let x = if cw[v] == 1 { 1.0 } else { -1.0 };
            let y = x + sigma * rng.normal_pair().0;
            *l = (2.0 * y / (sigma * sigma)) as f32;
        }
        llr
    }

    /// The noise's standard deviation for an Eb/N0 in dB at this code's
    /// rate, with unit-energy symbols.
    fn sigma_for(eb_n0_db: f64) -> f64 {
        let rate = K as f64 / N as f64;
        (1.0 / (2.0 * rate * 10f64.powf(eb_n0_db / 10.0))).sqrt()
    }

    #[test]
    fn the_tables_are_the_shape_the_paper_gives() {
        let c = code();
        assert_eq!(c.generator.len(), M);
        assert!(c.generator.iter().all(|g| g >> K == 0));
        // Every column has three ones; rows have six or seven.
        assert_eq!(c.checks.iter().map(Vec::len).sum::<usize>(), 3 * N);
        assert!(c.checks.iter().all(|vs| vs.len() == 6 || vs.len() == 7));
        for cs in &c.bit_checks {
            assert!(cs[0] != cs[1] && cs[1] != cs[2] && cs[0] != cs[2]);
        }
    }

    #[test]
    fn every_codeword_the_generator_makes_passes_every_check() {
        // The unit messages span the code: if each passes, every codeword
        // does, and the generator, the parity table and the layout agree.
        for j in 0..K {
            let cw = encode(1u128 << (K - 1 - j));
            assert_eq!(failed_checks(&as_bits(&cw)), 0, "message bit {j}");
            assert_eq!(cw[j], 1);
            assert_eq!(cw[..K].iter().map(|&b| u32::from(b)).sum::<u32>(), 1);
        }
        let mut rng = Rng::new(1);
        for _ in 0..200 {
            let w = crc::with_crc(random_payload(&mut rng));
            assert_eq!(failed_checks(&as_bits(&encode(w))), 0);
        }
    }

    #[test]
    fn a_clean_codeword_needs_nothing() {
        let w = crc::with_crc(0x1234_5678_9ABC_DEF0_1234);
        let cw = encode(w);
        let llr: [f32; N] = cw.map(|b| if b == 1 { 4.0 } else { -4.0 });
        assert_eq!(
            decode(&llr, 30, 2),
            Some(Decoded {
                payload: 0x1234_5678_9ABC_DEF0_1234,
                iterations: 0,
                repaired: 0,
                osd: None,
                distance: 0.0,
            })
        );
    }

    #[test]
    fn weak_wrong_bits_are_repaired_and_counted() {
        let p = 0x0F0F_0F0F_0F0F_0F0F_0F0F;
        let cw = encode(crc::with_crc(p));
        let mut llr: [f32; N] = cw.map(|b| if b == 1 { 3.0 } else { -3.0 });
        for v in (0..N).step_by(17) {
            llr[v] = -llr[v] * 0.2;
        }
        let d = decode(&llr, 30, 0).expect("repaired");
        assert_eq!(d.payload, p);
        assert_eq!(d.repaired, (0..N).step_by(17).count() as u32);
        assert!(d.iterations >= 1);
        assert_eq!(d.osd, None);
    }

    #[test]
    fn a_codeword_with_a_wrong_crc_is_not_a_message() {
        // A real codeword of the LDPC code whose last 14 message bits are not
        // the CRC of the first 77: belief propagation converges to it at
        // once, and it must still be refused.
        let w = crc::with_crc(0xABCDEF) ^ 1;
        let cw = encode(w);
        let llr: [f32; N] = cw.map(|b| if b == 1 { 4.0 } else { -4.0 });
        assert_eq!(decode(&llr, 30, 2), None);
    }

    /// Decodes out of `trials` at an Eb/N0, and how many came out wrong.
    fn rate(eb_n0_db: f64, trials: usize, osd: u8, seed: u64) -> (usize, usize, usize) {
        let mut rng = Rng::new(seed);
        let sigma = sigma_for(eb_n0_db);
        let (mut ok, mut wrong, mut by_osd) = (0, 0, 0);
        for _ in 0..trials {
            let p = random_payload(&mut rng);
            let llr = through_awgn(&encode(crc::with_crc(p)), sigma, &mut rng);
            match decode(&llr, 30, osd) {
                Some(d) if d.payload == p => {
                    ok += 1;
                    by_osd += usize::from(d.osd.is_some());
                }
                Some(_) => wrong += 1,
                None => {}
            }
        }
        (ok, wrong, by_osd)
    }

    #[test]
    fn decoding_holds_its_measured_rates_on_a_gaussian_channel() {
        // Measured with `measure_the_decoder` below; the bar later work keeps.
        let (ok, wrong, _) = rate(3.0, 200, 0, 7);
        assert!(ok >= 196, "BP at 3 dB: {ok}/200");
        assert_eq!(wrong, 0);
        let (bp, _, _) = rate(1.5, 200, 0, 8);
        let (with_osd, wrong, by_osd) = rate(1.5, 200, 2, 8);
        assert!(
            with_osd > bp,
            "OSD adds decodes at 1.5 dB: {bp} -> {with_osd}"
        );
        assert!(by_osd > 0);
        assert_eq!(wrong, 0, "no wrong message at 1.5 dB");
    }

    #[test]
    fn noise_alone_decodes_to_nothing() {
        let mut rng = Rng::new(99);
        for _ in 0..300 {
            let mut llr = [0f32; N];
            for l in llr.iter_mut() {
                *l = (2.0 * rng.normal_pair().0) as f32;
            }
            assert_eq!(decode(&llr, 30, 2), None);
        }
    }

    /// Prints the decode table the bar below was set from. `cargo test --release measure_the_decoder -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn measure_the_decoder() {
        for db in [0.5, 1.0, 1.5, 2.0, 2.5, 3.0, 3.5] {
            let (bp, bpw, _) = rate(db, 1000, 0, 11);
            let (o1, o1w, _) = rate(db, 1000, 1, 11);
            let (o2, o2w, n2) = rate(db, 1000, 2, 11);
            println!(
                "Eb/N0 {db:>3} dB: BP+OSD0 {bp:>4} ({bpw} wrong)  OSD1 {o1:>4} ({o1w})  OSD2 {o2:>4} ({o2w}, {n2} by OSD)"
            );
        }
    }

    /// What each threshold keeps and lets through: true transmissions OSD
    /// recovers after belief propagation failed, against noise decoded to a
    /// message at all, for OSD orders 1 and 2.
    #[test]
    #[ignore]
    fn measure_the_osd_thresholds() {
        let mut rng = Rng::new(21);
        let found = |order: u8, llr: &[f32; N]| -> Option<f32> {
            let mut hard = [0u64; 3];
            for (v, &l) in llr.iter().enumerate() {
                if l > 0.0 {
                    set(&mut hard, v);
                }
            }
            let (bits, _) = osd(llr, &hard, order, f32::INFINITY)?;
            let total: f32 = llr.iter().map(|l| l.abs()).sum();
            let d: f32 = (0..N)
                .filter(|&v| get(&bits, v) != get(&hard, v))
                .map(|v| llr[v].abs())
                .sum();
            Some(d / total)
        };
        for order in [1u8, 2] {
            let mut good = Vec::new();
            let mut bad = Vec::new();
            let mut bp_failed = 0;
            for db in [0.5, 1.0, 1.5, 2.0] {
                for _ in 0..1000 {
                    let p = random_payload(&mut rng);
                    let llr = through_awgn(&encode(crc::with_crc(p)), sigma_for(db), &mut rng);
                    if belief_propagation(&llr, 30).is_some_and(|(b, _)| crc::check(head(&b))) {
                        continue;
                    }
                    bp_failed += 1;
                    let mut hard = [0u64; 3];
                    for (v, &l) in llr.iter().enumerate() {
                        if l > 0.0 {
                            set(&mut hard, v);
                        }
                    }
                    if let Some((bits, _)) = osd(&llr, &hard, order, f32::INFINITY) {
                        let d = found(order, &llr).unwrap();
                        if head(&bits) >> 14 == p {
                            good.push(d)
                        } else {
                            bad.push(d)
                        }
                    }
                }
            }
            let mut noise = Vec::new();
            for _ in 0..20000 {
                let mut llr = [0f32; N];
                for l in llr.iter_mut() {
                    *l = (2.0 * rng.normal_pair().0) as f32;
                }
                if let Some(d) = found(order, &llr) {
                    noise.push(d);
                }
            }
            println!("order {order}: {bp_failed} BP failures (0.5-2 dB)");
            for t in [0.05f32, 0.06, 0.07, 0.075, 0.08, 0.085, 0.09, 0.10, 0.11] {
                let g = good.iter().filter(|&&d| d <= t).count();
                let b = bad.iter().filter(|&&d| d <= t).count();
                let n = noise.iter().filter(|&&d| d <= t).count();
                println!("  T {t:.3}: recovered {g:>4}  wrong {b:>2}  noise {n:>3}/20000");
            }
        }
    }
}
