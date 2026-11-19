// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Which format a frame is, told as the standard's own receiver tells it
//! (IEEE 802.11-2024, 27.3.24, and 19.3.9.4.3 for HT).
//!
//! **In the receiver's order.** A QBPSK first symbol after the long
//! training field is HT-greenfield, which has no L-SIG. Otherwise the
//! L-SIG is read, and only a 6 Mb/s one can be anything but non-HT: every
//! later format sets it so (10.27.4, 27.3.11.5, 36.3.12.5), and that keeps
//! QPSK and QAM data, whose two axes carry equal power, from ever being
//! tested for a rotation. Then a QBPSK second symbol is HT-mixed's HT-SIG;
//! a second symbol repeating the L-SIG is an RL-SIG, HE when the LENGTH
//! leaves 1 or 2 over 3 and EHT or a later PHY when it leaves none; a
//! QBPSK third symbol is VHT's SIG-A; anything else is non-HT. A symbol
//! past the duration the L-SIG announces is never tested: it is whatever
//! came after the frame.
//!
//! **The repeat has a law, as the gates do.** Once the L-SIG is read, the
//! 48 coded bits it was sent as are known again; read as BPSK, a repeat
//! agrees with them in sign on nearly every tone, while any other symbol's
//! coded bits agree as a fair coin would, Binomial(48, 1/2). The threshold
//! is the count a coin reaches with probability [`RL_SIG_FALSE_ALARM`].
//! Against the known bits, only the second symbol's noise can disagree:
//! comparing two received symbols, both noisy, missed one HE frame in 20 at
//! 3 dB.
//!
//! **HE and EHT are read to their network's name.** After an RL-SIG come
//! 52-tone symbols ([`super::sig::Header::wide_symbol`]), their four edge
//! tones measured from the L-SIG and the RL-SIG: HE-SIG-A or U-SIG, whose
//! colour, direction and TXOP [`super::hesig`] reads under the CRC. An
//! extended-range PPDU, HE's or a later PHY's, sends its SIG field's two
//! symbols each twice, the first repeat QBPSK, which is how it is told;
//! the repeats are added in before decoding.

use num_complex::Complex;

use super::bcc;
use super::hesig::{HeKind, HeSigA, USig};
use super::htsig::{self, HtSig, SigFault};
use super::interleave::{deinterleave, deinterleave_wide, interleave};
use super::lsig::{self, LSig, Refused};
use super::sig::{header, Header, Symbol};
use super::signal_field::{Coding, Rate};
use super::sync::Sync;

/// The chance that a symbol which is not a repeat passes for one.
pub const RL_SIG_FALSE_ALARM: f64 = 1e-6;

/// A frame's format, and what its signalling said.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// No later format's signature: a clause 17 or 18 frame.
    NonHt,
    /// QBPSK after the L-SIG, and its HT-SIG.
    HtMixed(Result<HtSig, SigFault>),
    /// QBPSK straight after the long field, and its HT-SIG.
    Greenfield(Result<HtSig, SigFault>),
    /// QBPSK on the third symbol: VHT-SIG-A, whose CRC is checked and
    /// whose fields are not read.
    Vht(Result<(), SigFault>),
    /// An RL-SIG with LENGTH leaving 1 or 2 over 3: HE, which kind the
    /// remainder and the rotations allow, and its SIG-A.
    He {
        kind: HeKind,
        sig_a: Result<HeSigA, SigFault>,
    },
    /// An RL-SIG with LENGTH a multiple of 3: a U-SIG, EHT's or a later
    /// PHY's, in an extended-range preamble or not.
    USig {
        er: bool,
        sig: Result<USig, SigFault>,
    },
}

/// What the symbols after a frame's long training field said: its L-SIG
/// (none in greenfield), and its format (none when the L-SIG was refused).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Heard {
    pub lsig: Option<Result<LSig, Refused>>,
    pub format: Option<Format>,
}

/// Which of three a frame's signalling was: an L-SIG read, a greenfield
/// HT-SIG read in place of one, or neither.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signalling {
    LSigRead,
    Greenfield,
    /// Neither an L-SIG nor a greenfield HT-SIG passed its checks.
    Unreadable,
}

impl Heard {
    /// Which of the three it was. A QBPSK first symbol whose HT-SIG fails
    /// is unreadable, not greenfield: noise tips the rotation either way,
    /// and only the CRC says a greenfield frame was there.
    pub fn signalling(&self) -> Signalling {
        match (self.lsig, self.format) {
            (Some(Ok(_)), _) => Signalling::LSigRead,
            (None, Some(Format::Greenfield(Ok(_)))) => Signalling::Greenfield,
            _ => Signalling::Unreadable,
        }
    }
}

const BPSK: Complex<f64> = Complex::new(1.0, 0.0);
const QBPSK: Complex<f64> = Complex::new(0.0, 1.0);

/// The chance that a fair coin tossed 48 times comes up the same `t` times
/// or more.
fn coin_tail(t: usize) -> f64 {
    let mut choose = 1.0f64;
    let mut tail = 0.0;
    for k in 0..=48usize {
        if k >= t {
            tail += choose;
        }
        choose = choose * (48 - k) as f64 / (k + 1) as f64;
    }
    tail / 2f64.powi(48)
}

/// The fewest agreements a repeat must have: the least a fair coin reaches
/// with probability [`RL_SIG_FALSE_ALARM`] at most.
fn repeat_threshold() -> usize {
    (0..=48)
        .find(|&t| coin_tail(t) <= RL_SIG_FALSE_ALARM)
        .unwrap_or(49)
}

/// Whether readings in subcarrier order repeat the L-SIG whose decoded
/// bits are `lsig`: its coded bits, interleaved as sent, against their
/// signs.
fn repeats(lsig: &[bool], second: &[f32]) -> bool {
    let sent = interleave(&bcc::encode(lsig), 1);
    let agree = sent
        .iter()
        .zip(second)
        .filter(|(b, r)| **b == (**r > 0.0))
        .count();
    agree >= repeat_threshold()
}

/// A two-symbol SIG field's 48 bits, each symbol read at its rotation and
/// deinterleaved, decoded unterminated so its tail is a check.
fn sig_bits(one: &Symbol, two: &Symbol, turns: [Complex<f64>; 2]) -> Vec<bool> {
    let mut r = deinterleave(&one.readings(turns[0]), 1);
    r.extend(deinterleave(&two.readings(turns[1]), 1));
    // 96 readings always decode to 48 bits.
    bcc::decode(&r, Coding::Half, false).unwrap_or_else(|| vec![true; 48])
}

/// A 52-bit SIG field from the 52-tone symbols from the third after the
/// long field: two, each deinterleaved; or in extended range four, each
/// pair's repeat (not interleaved, the first QBPSK) added reading by
/// reading to its first. Decoded unterminated so the tail is a check.
fn wide_bits(h: &Header<'_>, edges: &[Complex<f64>; 4], er: bool) -> Option<Vec<bool>> {
    let symbol = |n| h.wide_symbol(n, edges);
    let mut r = deinterleave_wide(&symbol(2)?.readings(BPSK));
    if er {
        for (x, y) in r.iter_mut().zip(symbol(3)?.readings(QBPSK)) {
            *x += y;
        }
        let mut second = deinterleave_wide(&symbol(4)?.readings(BPSK));
        for (x, y) in second.iter_mut().zip(symbol(5)?.readings(BPSK)) {
            *x += y;
        }
        r.extend(second);
    } else {
        r.extend(deinterleave_wide(&symbol(3)?.readings(BPSK)));
    }
    // 104 readings always decode to 52 bits; a field of ones fails its tail.
    Some(bcc::decode(&r, Coding::Half, false).unwrap_or_else(|| vec![true; 52]))
}

/// After an RL-SIG: HE by the LENGTH's remainder 1 or 2, with an
/// extended-range SU told from MU by a QBPSK second symbol after the
/// RL-SIG; remainder 0 a U-SIG, extended range told the same way.
fn after_repeat(h: &Header<'_>, length: u16) -> Option<Format> {
    let edges = h.edges()?;
    let er = h.wide_symbol(3, &edges)?.quadrature() > 0.0;
    Some(match length % 3 {
        0 => Format::USig {
            er,
            sig: USig::from_bits(&wide_bits(h, &edges, er)?),
        },
        1 => Format::He {
            kind: HeKind::SuOrTb,
            sig_a: HeSigA::from_bits(&wide_bits(h, &edges, false)?, HeKind::SuOrTb),
        },
        _ => {
            let kind = if er { HeKind::ErSu } else { HeKind::Mu };
            Format::He {
                kind,
                sig_a: HeSigA::from_bits(&wide_bits(h, &edges, er)?, kind),
            }
        }
    })
}

/// Read the symbols after the long field of the frame `sync` describes,
/// from `samples` starting at stream pair `first_pair`. `None` unless the
/// symbols the decision needs are held.
pub fn read(samples: &[Complex<f32>], first_pair: u64, sync: &Sync) -> Option<Heard> {
    let h = header(samples, first_pair, sync)?;
    let first = h.symbol(0)?;
    if first.quadrature() > 0.0 {
        let second = h.symbol(1)?;
        let bits = sig_bits(&first, &second, [QBPSK, QBPSK]);
        return Some(Heard {
            lsig: None,
            format: Some(Format::Greenfield(HtSig::from_bits(&bits))),
        });
    }
    let decoded = lsig::bits(&first.readings(BPSK));
    let l = LSig::from_bits(&decoded);
    let heard = |format| Heard {
        lsig: Some(l),
        format,
    };
    let Ok(said) = l else {
        return Some(heard(None));
    };
    let n_sym = said.n_sym();
    if said.rate != Rate::R6 {
        return Some(heard(Some(Format::NonHt)));
    }
    let second = h.symbol(1)?;
    let third = h.symbol(2)?;
    if n_sym >= 2 && second.quadrature() > 0.0 {
        let bits = sig_bits(&second, &third, [QBPSK, QBPSK]);
        return Some(heard(Some(Format::HtMixed(HtSig::from_bits(&bits)))));
    }
    if repeats(&decoded, &second.readings(BPSK)) {
        return Some(heard(Some(after_repeat(&h, said.length)?)));
    }
    if n_sym >= 2 && third.quadrature() > 0.0 {
        let bits = sig_bits(&second, &third, [BPSK, QBPSK]);
        return Some(heard(Some(Format::Vht(htsig::check(&bits)))));
    }
    Some(heard(Some(Format::NonHt)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::dsp::testkit::Rng;
    use crate::signal::dsp::uncertainty::Uncertain;
    use crate::signal::wifi::detect::StfDetector;
    use crate::signal::wifi::hesig::{self, Bss, HeKind, HePpdu, HeSigA, USig};
    use crate::signal::wifi::htsig;
    use crate::signal::wifi::ppdu::{
        assemble, greenfield_ltf, l_sig_at, long_training, random_symbol, short_training,
        two_sig_symbols, wide_l_sig_at, wide_sig_symbols,
    };
    use crate::signal::wifi::signal_field::Rate;
    use crate::signal::wifi::sync::{examine, Verdict};

    const LEAD: usize = 700;

    /// The kinds of preamble a 2.4 GHz channel carries, built as their
    /// clauses build them, each followed by DATA-like symbols.
    #[derive(Clone, Copy, Debug)]
    enum Kind {
        NonHt(Rate, u16),
        Mixed(u16, HtSig),
        Greenfield(HtSig),
        Vht(u16),
        /// An L-SIG and its repeat with this LENGTH, as HE and EHT send
        /// them, and a 52-bit SIG field after, in its extended-range form
        /// when the flag says so.
        Repeated(u16, [bool; 52], bool),
    }

    fn sig() -> HtSig {
        HtSig {
            mcs: 7,
            cbw40: false,
            length: 1500,
            stbc: 0,
            ldpc: false,
            short_gi: true,
            ness: 0,
        }
    }

    fn bpsk() -> Vec<Complex<f64>> {
        vec![Complex::new(1.0, 0.0), Complex::new(-1.0, 0.0)]
    }

    fn qpsk() -> Vec<Complex<f64>> {
        let r = std::f64::consts::FRAC_1_SQRT_2;
        [(1.0, 1.0), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0)]
            .map(|(a, b)| Complex::new(a * r, b * r))
            .to_vec()
    }

    /// `kind`'s preamble and symbols, eight after the long field in all.
    fn build(kind: Kind, rng: &mut Rng) -> Vec<Complex<f64>> {
        let mut ltf = long_training();
        let mut symbols: Vec<Vec<Complex<f64>>> = Vec::new();
        let mut data = bpsk();
        match kind {
            Kind::NonHt(rate, length) => {
                symbols.push(l_sig_at(rate, length, 0));
                if rate.n_bpsc() > 1 {
                    data = qpsk();
                }
            }
            Kind::Mixed(length, s) => {
                symbols.push(l_sig_at(Rate::R6, length, 0));
                symbols.extend(two_sig_symbols(&htsig::bits(&s), [true, true], 1));
            }
            Kind::Greenfield(s) => {
                ltf = greenfield_ltf();
                symbols.extend(two_sig_symbols(&htsig::bits(&s), [true, true], 0));
            }
            Kind::Vht(length) => {
                symbols.push(l_sig_at(Rate::R6, length, 0));
                // VHT-SIG-A's own fields matter not here, only its CRC.
                symbols.extend(two_sig_symbols(&htsig::bits(&sig()), [false, true], 1));
            }
            Kind::Repeated(length, bits, er) => {
                symbols.push(wide_l_sig_at(Rate::R6, length, 0));
                symbols.push(wide_l_sig_at(Rate::R6, length, 1));
                symbols.extend(wide_sig_symbols(&bits, er, 2));
            }
        }
        while symbols.len() < 8 {
            let n = symbols.len();
            symbols.push(random_symbol(rng, &data, n));
        }
        assemble(&short_training(), &ltf, &symbols)
    }

    /// `frame` at `LEAD` into noise `snr_db` below it.
    fn on_air(frame: &[Complex<f64>], snr_db: f64, rng: &mut Rng) -> Vec<Complex<f32>> {
        let power = frame[..320].iter().map(|z| z.norm_sqr()).sum::<f64>() / 320.0;
        let noise = rng.noise(LEAD + frame.len() + 600, 10f64.powf(-snr_db / 10.0));
        let mut x: Vec<Complex<f32>> = noise;
        for (o, z) in x[LEAD..].iter_mut().zip(frame) {
            let z = z / power.sqrt();
            *o += Complex::new(z.re as f32, z.im as f32);
        }
        x
    }

    /// Synchronised as the timing tests show it is, the offset's
    /// uncertainty what the synchronisation states at 10 dB.
    fn placed() -> Sync {
        Sync {
            ltf_start: (LEAD + 192) as u64,
            offset: Uncertain::from_sigma(0.0, 2_000.0),
            snr: 100.0,
        }
    }

    /// Found and synchronised through the chain as it runs, then read.
    fn received(x: &[Complex<f32>]) -> Option<Heard> {
        let peak = StfDetector::new().push(0, x).first()?.peak;
        let Some(Verdict::ThisChannel(s)) = examine(x, 0, peak) else {
            return None;
        };
        read(x, 0, &s)
    }

    fn lsig(rate: Rate, length: u16) -> Option<Result<LSig, Refused>> {
        Some(Ok(LSig { rate, length }))
    }

    /// Every format's signature, at 10 dB through the chain as it runs,
    /// told for what it is and never for another.
    #[test]
    fn every_signature_is_told_for_what_it_is() {
        let mut rng = Rng::new(90);
        let cases = [
            (
                Kind::NonHt(Rate::R6, 200),
                Heard {
                    lsig: lsig(Rate::R6, 200),
                    format: Some(Format::NonHt),
                },
            ),
            (
                Kind::NonHt(Rate::R54, 1500),
                Heard {
                    lsig: lsig(Rate::R54, 1500),
                    format: Some(Format::NonHt),
                },
            ),
            (
                Kind::Mixed(2001, sig()),
                Heard {
                    lsig: lsig(Rate::R6, 2001),
                    format: Some(Format::HtMixed(Ok(sig()))),
                },
            ),
            (
                Kind::Greenfield(sig()),
                Heard {
                    lsig: None,
                    format: Some(Format::Greenfield(Ok(sig()))),
                },
            ),
            (
                Kind::Vht(900),
                Heard {
                    lsig: lsig(Rate::R6, 900),
                    format: Some(Format::Vht(Ok(()))),
                },
            ),
        ];
        for (kind, want) in cases {
            for _ in 0..20 {
                let x = on_air(&build(kind, &mut rng), 10.0, &mut rng);
                assert_eq!(received(&x), Some(want), "{kind:?}");
            }
        }
    }

    fn bss(colour: u8, uplink: bool) -> Bss {
        Bss {
            colour,
            uplink,
            txop_us: Some(1536),
        }
    }

    /// Every HE PPDU and every U-SIG, at 10 dB through the chain as it
    /// runs: named by its remainder, its rotations and its Format bit, its
    /// colour, direction and TXOP read under the CRC; a later PHY by its
    /// version number; an extended-range preamble by its QBPSK repeat.
    #[test]
    fn every_he_and_usig_is_read() {
        let he = |ppdu, bss| HeSigA { ppdu, bss };
        let cases = [
            (1000, he(HePpdu::Su, bss(23, true)), HeKind::SuOrTb),
            (1003, he(HePpdu::Tb, bss(41, true)), HeKind::SuOrTb),
            (1001, he(HePpdu::Mu, bss(63, false)), HeKind::Mu),
            (1004, he(HePpdu::ErSu, bss(5, false)), HeKind::ErSu),
        ];
        let mut rng = Rng::new(96);
        for (length, sig, kind) in cases {
            let er = kind == HeKind::ErSu;
            let want = Some(Format::He {
                kind,
                sig_a: Ok(sig),
            });
            for _ in 0..20 {
                let k = Kind::Repeated(length, hesig::he_bits(&sig), er);
                let x = on_air(&build(k, &mut rng), 10.0, &mut rng);
                let heard = received(&x).expect("found");
                assert_eq!(heard.lsig, lsig(Rate::R6, length));
                assert_eq!(heard.format, want, "{sig:?}");
            }
        }
        for (version, er) in [(0, false), (3, false), (0, true)] {
            let sig = USig {
                version,
                bss: bss(9, true),
            };
            let want = Some(Format::USig { er, sig: Ok(sig) });
            for _ in 0..20 {
                let k = Kind::Repeated(1002, hesig::usig_bits(&sig), er);
                let x = on_air(&build(k, &mut rng), 10.0, &mut rng);
                assert_eq!(received(&x).expect("found").format, want, "{sig:?}");
            }
        }
    }

    /// A preamble whose carrier lay 14 kHz from the rest of its frame (5.8
    /// ppm, the most the recordings showed): by an extended-range SU's
    /// last SIG-A symbol the preamble's offset is 2.3 rad wrong, and the
    /// pilots, weighed against the offset's stated error, still right
    /// enough of it. At 15 dB through the chain, every one read (and at 6,
    /// 10 and 20 dB too, tried by hand; widening the pilots' prior by the
    /// wander changed nothing, so it was not done).
    #[test]
    fn a_pulled_preambles_later_symbols_are_righted() {
        let sig = HeSigA {
            ppdu: HePpdu::ErSu,
            bss: bss(5, false),
        };
        let mut rng = Rng::new(100);
        let mut read_right = 0;
        for _ in 0..20 {
            let f = build(Kind::Repeated(1004, hesig::he_bits(&sig), true), &mut rng);
            let mut x = on_air(&f, 15.0, &mut rng);
            let turn = std::f64::consts::TAU * 14e3 / 20e6;
            for (n, z) in x[LEAD..].iter_mut().enumerate() {
                *z *= Complex::from_polar(1.0, (turn * n.min(320) as f64) as f32);
            }
            let want = Some(Format::He {
                kind: HeKind::ErSu,
                sig_a: Ok(sig),
            });
            read_right += (received(&x).map(|h| h.format) == Some(want)) as usize;
        }
        assert_eq!(read_right, 20);
    }

    /// A U-SIG whose CRC was spoiled in sending is an RL-SIG with its U-SIG
    /// refused, and named nothing more.
    #[test]
    fn a_spoiled_usig_is_refused() {
        let mut rng = Rng::new(97);
        let mut bits = hesig::usig_bits(&USig {
            version: 0,
            bss: bss(9, true),
        });
        bits[9] = !bits[9];
        let x = on_air(
            &build(Kind::Repeated(1002, bits, false), &mut rng),
            25.0,
            &mut rng,
        );
        assert_eq!(
            read(&x, 0, &placed()).map(|h| h.format),
            Some(Some(Format::USig {
                er: false,
                sig: Err(SigFault::Crc)
            }))
        );
    }

    /// The edge tones are measured, not borrowed from +-26: through an
    /// echo 750 ns late at 0.8, which turns the channel a quarter turn and
    /// more between 26 and 28, each edge tone's channel against its
    /// neighbour's at +-26 is the true ratio, at 30 dB.
    #[test]
    fn the_edge_tones_are_the_channels_own() {
        let sig = HeSigA {
            ppdu: HePpdu::Su,
            bss: bss(23, true),
        };
        let echo = Complex::from_polar(0.8, 1.0);
        let truth = |k: i32| {
            Complex::new(1.0, 0.0)
                + echo * Complex::from_polar(1.0, -std::f64::consts::TAU * (15 * k) as f64 / 64.0)
        };
        let mut rng = Rng::new(99);
        let f = build(Kind::Repeated(1000, hesig::he_bits(&sig), false), &mut rng);
        let mut echoed = f.clone();
        for (n, z) in f.iter().enumerate() {
            if n + 15 < echoed.len() {
                echoed[n + 15] += z * echo;
            }
        }
        let x = on_air(&echoed, 30.0, &mut rng);
        let peak = StfDetector::new().push(0, &x)[0].peak;
        let Some(Verdict::ThisChannel(s)) = examine(&x, 0, peak) else {
            panic!("not judged this channel's");
        };
        let h = crate::signal::wifi::sig::header(&x, 0, &s).unwrap();
        let edges = h.edges().unwrap();
        let centre = |k: i32| {
            let e = crate::signal::wifi::channel::estimate(
                &x,
                0,
                s.ltf_start,
                s.offset.value(),
                &crate::signal::wifi::channel::Floor::flat(),
            )
            .unwrap();
            e.h[if k < 0 { 0 } else { 51 }]
        };
        for (i, k) in [-28i32, -27, 27, 28].into_iter().enumerate() {
            let near = k.signum() * 26;
            let got = edges[i] / centre(near);
            let want = truth(k) / truth(near);
            assert!((got - want).norm() < 0.1, "{k}: {got} against {want}");
            // And it is not the neighbour's: the channel has moved.
            assert!((want - 1.0).norm() > 0.5, "{k}: {want}");
        }
    }

    /// Through a 250 ns echo at half amplitude, which moves the channel
    /// across the band, an HE SU's SIG-A is read at 12 dB with the edge
    /// tones measured from the L-SIG.
    #[test]
    fn the_edge_tones_are_measured() {
        let sig = HeSigA {
            ppdu: HePpdu::Su,
            bss: bss(23, true),
        };
        let mut rng = Rng::new(98);
        for _ in 0..20 {
            let f = build(Kind::Repeated(1000, hesig::he_bits(&sig), false), &mut rng);
            let mut echoed = f.clone();
            for (n, z) in f.iter().enumerate() {
                if n + 5 < echoed.len() {
                    echoed[n + 5] += z * Complex::from_polar(0.5, 2.0);
                }
            }
            let x = on_air(&echoed, 12.0, &mut rng);
            assert_eq!(
                received(&x).expect("found").format,
                Some(Format::He {
                    kind: HeKind::SuOrTb,
                    sig_a: Ok(sig)
                })
            );
        }
    }

    /// An HT-SIG whose CRC was spoiled in sending is refused by it, and
    /// says nothing; the frame is still HT-mixed by its rotation.
    #[test]
    fn a_spoiled_ht_sig_is_refused_by_its_crc() {
        let mut rng = Rng::new(91);
        let mut bits = htsig::bits(&sig());
        bits[36] = !bits[36];
        let symbols = vec![
            l_sig_at(Rate::R6, 300, 0),
            two_sig_symbols(&bits, [true, true], 1)[0].clone(),
            two_sig_symbols(&bits, [true, true], 1)[1].clone(),
            random_symbol(&mut rng, &bpsk(), 3),
        ];
        let frame = assemble(&short_training(), &long_training(), &symbols);
        let x = on_air(&frame, 25.0, &mut rng);
        assert_eq!(
            read(&x, 0, &placed()).map(|h| h.format),
            Some(Some(Format::HtMixed(Err(SigFault::Crc))))
        );
    }

    /// **The RL-SIG's law**: non-HT frames at 6 Mb/s, whose second symbol
    /// is data, are never taken for HE or EHT over 3000 frames at 20 dB;
    /// and the threshold is where a fair coin's agreements reach 1e-6.
    #[test]
    fn non_ht_data_is_never_a_repeat() {
        let t = repeat_threshold();
        eprintln!("a repeat agrees on {t} of 48 tones or more");
        assert!(coin_tail(t) <= RL_SIG_FALSE_ALARM);
        assert!(coin_tail(t - 1) > RL_SIG_FALSE_ALARM);
        let mut rng = Rng::new(92);
        for i in 0..3000 {
            let length = 100 + (i % 1500) as u16;
            let x = on_air(
                &build(Kind::NonHt(Rate::R6, length), &mut rng),
                20.0,
                &mut rng,
            );
            let heard = read(&x, 0, &placed()).unwrap();
            assert_eq!(heard.format, Some(Format::NonHt), "{length}");
        }
    }

    /// Where the L-SIG can be read at all, its repeat is found: at 3 dB,
    /// through the chain as it runs, of the frames whose L-SIG was read,
    /// nearly every one is named HE.
    #[test]
    fn a_repeat_is_found_wherever_the_l_sig_is_read() {
        let mut rng = Rng::new(95);
        let (mut read_ok, mut named) = (0, 0);
        for i in 0..400 {
            let length = 1000 + 3 * (i % 300) as u16;
            let sig = HeSigA {
                ppdu: HePpdu::Su,
                bss: bss(23, true),
            };
            let k = Kind::Repeated(length, hesig::he_bits(&sig), false);
            let x = on_air(&build(k, &mut rng), 3.0, &mut rng);
            let Some(heard) = received(&x) else {
                continue;
            };
            if let Some(Ok(_)) = heard.lsig {
                read_ok += 1;
                named += matches!(heard.format, Some(Format::He { .. })) as usize;
            }
        }
        eprintln!("3 dB: {named} of {read_ok} read L-SIGs named HE");
        assert!(
            read_ok > 300 && named * 100 >= read_ok * 97,
            "{named} of {read_ok}"
        );
    }

    /// A symbol past the duration the L-SIG announces is never tested: a
    /// 6 Mb/s frame of one DATA symbol followed by a QBPSK one stays non-HT.
    #[test]
    fn past_the_announced_end_nothing_is_tested() {
        let mut rng = Rng::new(93);
        let symbols = vec![
            l_sig_at(Rate::R6, 0, 0),
            random_symbol(&mut rng, &bpsk(), 1),
            two_sig_symbols(&htsig::bits(&sig()), [true, true], 1)[1].clone(),
        ];
        let frame = assemble(&short_training(), &long_training(), &symbols);
        let x = on_air(&frame, 25.0, &mut rng);
        assert_eq!(
            read(&x, 0, &placed()).map(|h| h.format),
            Some(Some(Format::NonHt))
        );
    }

    /// A frame is counted greenfield only when its HT-SIG passed: a
    /// rotation alone, which noise can tip, is unreadable.
    #[test]
    fn greenfield_is_counted_by_its_crc() {
        let gf = |sig| Heard {
            lsig: None,
            format: Some(Format::Greenfield(sig)),
        };
        assert_eq!(gf(Ok(sig())).signalling(), Signalling::Greenfield);
        assert_eq!(gf(Err(SigFault::Crc)).signalling(), Signalling::Unreadable);
        let refused = Heard {
            lsig: Some(Err(Refused::Tail)),
            format: None,
        };
        assert_eq!(refused.signalling(), Signalling::Unreadable);
        let mixed_refused = Heard {
            lsig: lsig(Rate::R6, 300),
            format: Some(Format::HtMixed(Err(SigFault::Crc))),
        };
        assert_eq!(mixed_refused.signalling(), Signalling::LSigRead);
    }

    /// A refused L-SIG leaves the format unread.
    #[test]
    fn a_refused_l_sig_names_no_format() {
        let mut rng = Rng::new(94);
        let mut bits = crate::signal::wifi::signal_field::bits(Rate::R6, 100);
        bits[17] = !bits[17];
        let symbols = vec![
            crate::signal::wifi::ppdu::sig_symbol(
                &crate::signal::wifi::bcc::encode(&bits),
                false,
                0,
            ),
            random_symbol(&mut rng, &bpsk(), 1),
            random_symbol(&mut rng, &bpsk(), 2),
        ];
        let frame = assemble(&short_training(), &long_training(), &symbols);
        let x = on_air(&frame, 25.0, &mut rng);
        assert_eq!(
            read(&x, 0, &placed()),
            Some(Heard {
                lsig: Some(Err(Refused::Parity)),
                format: None
            })
        );
    }
    /// **What a recording's frames say, by hand**
    /// (`SDRTOP_REPLAY=path.sigmf-data`, ci8 at 20 Msps on a channel
    /// centre): every frame of this channel read offline, counted by
    /// format, by L-SIG rate and length, with the refusals by check.
    /// `cargo test --release a_recordings_formats -- --ignored
    /// --nocapture`.
    #[test]
    #[ignore]
    fn a_recordings_formats() {
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
        let mut by_format = std::collections::BTreeMap::new();
        let mut by_mcs = std::collections::BTreeMap::new();
        let centre = std::fs::read_to_string(path.replace(".sigmf-data", ".sigmf-meta"))
            .ok()
            .and_then(|m| serde_json::from_str::<serde_json::Value>(&m).ok())
            .and_then(|m| m["captures"][0]["core:frequency"].as_f64())
            .unwrap_or(2_462e6);
        let mut clocks: Vec<(usize, String, Uncertain, Uncertain, f64)> = Vec::new();
        let mut pulls_ppm: Vec<(f64, usize)> = Vec::new();
        let mut detector = StfDetector::new();
        for at in (0..pairs).step_by(131_072) {
            for d in detector.push(at as u64, &stretch(at, 131_072)) {
                let Some((from, len)) = crate::signal::wifi::sync::needs(d.peak) else {
                    continue;
                };
                let x = stretch(from as usize, len);
                if let Some(Verdict::ThisChannel(s)) = examine(&x, from, d.peak) {
                    let Some(heard) = read(&x, from, &s) else {
                        continue;
                    };
                    let name = match heard.format {
                        None => "L-SIG refused".to_string(),
                        Some(Format::HtMixed(Ok(_))) => "HT-mixed".to_string(),
                        Some(Format::Greenfield(Ok(_))) => "greenfield".to_string(),
                        Some(Format::HtMixed(Err(_)) | Format::Greenfield(Err(_))) => {
                            "HT, HT-SIG refused".to_string()
                        }
                        Some(f) => format!("{f:?}"),
                    };
                    if let Some(p) = crate::signal::wifi::clock::plan(&heard) {
                        let full = stretch(s.ltf_start as usize, p.end());
                        if let Some(c) =
                            crate::signal::wifi::clock::measure(&full, s.ltf_start, &s, &p)
                        {
                            let data = c.carrier_hz.value() / centre * 1e6;
                            if p.count >= 100 {
                                eprintln!(
                                    "  long: {} symbols, {name}, clock {:.2} +- {:.2}, carrier (preamble) {:.2}, carrier (data) {data:.2}",
                                    p.count,
                                    c.clock_ppm.value(),
                                    c.clock_ppm.sigma(),
                                    s.offset.value() / centre * 1e6,
                                );
                            }
                            let carrier =
                                Uncertain::from_sigma(data, c.carrier_hz.sigma() / centre * 1e6);
                            let c = c.clock_ppm;
                            clocks.push((p.count, name.clone(), c, carrier, 10.0 * s.snr.log10()));
                        }
                    }
                    if let Some(p) =
                        crate::signal::wifi::clock::layout(&heard).filter(|p| p.count >= 8)
                    {
                        let full = stretch(s.ltf_start as usize, p.end());
                        if let Some(r) =
                            crate::signal::wifi::clock::measure(&full, s.ltf_start, &s, &p)
                        {
                            if r.carrier_hz.sigma() / centre * 1e6 < 0.5 {
                                let pull = s.offset.value() - r.carrier_hz.value();
                                pulls_ppm.push((pull / centre * 1e6, p.count));
                            }
                        }
                    }
                    *by_format.entry(name).or_insert(0) += 1;
                    if let Some(Format::HtMixed(Ok(h)) | Format::Greenfield(Ok(h))) = heard.format {
                        *by_mcs.entry(h.mcs).or_insert(0) += 1;
                    }
                    let non_ht = heard.format == Some(Format::NonHt);
                    match heard.lsig {
                        Some(Ok(l)) if non_ht => {
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
                        _ => {}
                    }
                }
            }
        }
        let secs = pairs as f64 / 20e6;
        eprintln!("{secs:.1} s; by format {by_format:?}");
        eprintln!("HT MCS {by_mcs:?}");
        eprintln!("non-HT by L-SIG rate (Mb/s: frames) {by_rate:?}");
        eprintln!("non-HT by rate and length {by_length:?}");
        eprintln!("refused {refused:?}");
        eprintln!(
            "non-HT airtime {:.1} ms, {:.2} % of the recording",
            airtime as f64 / 1e3,
            airtime as f64 / 1e4 / secs
        );
        // How far each preamble's carrier lay from its DATA field's.
        let mut abs: Vec<f64> = pulls_ppm.iter().map(|p| p.0.abs()).collect();
        abs.sort_by(f64::total_cmp);
        if !abs.is_empty() {
            let n = pulls_ppm.len() as f64;
            let mean = pulls_ppm.iter().map(|p| p.0).sum::<f64>() / n;
            let rms = (pulls_ppm.iter().map(|p| p.0 * p.0).sum::<f64>() / n).sqrt();
            let q = |f: f64| abs[((abs.len() - 1) as f64 * f) as usize];
            eprintln!(
                "preamble - data carrier over {} frames: mean {mean:+.2} ppm, rms {rms:.2}, |.| median {:.2}, 90 % {:.2}, 99 % {:.2}, max {:.2}",
                pulls_ppm.len(), q(0.5), q(0.9), q(0.99), q(1.0)
            );
        }
        // The clocks against their carriers: by how many symbols they were
        // read over, the worst pulls, and by carrier (a transmitter's
        // likeness until the MAC header names it).
        let summary = |sel: &dyn Fn(usize) -> bool| {
            let mut c = crate::state::SdrMetrics::fixture().net.wifi.clock_check;
            let mut big = 0;
            for (n, _, clock, carrier, _) in clocks.iter().filter(|r| sel(r.0)) {
                c.add(&crate::state::WifiClock {
                    clock_ppm: *clock,
                    carrier_ppm: *carrier,
                    preamble_ppm: *carrier,
                    symbols: *n,
                    when: std::time::Instant::now(),
                });
                let pull = (clock.value() - carrier.value()) / clock.sigma().hypot(carrier.sigma());
                big += (pull.abs() > 5.0) as usize;
            }
            (c.frames, c.mean(), c.chi2_per_dof(), big)
        };
        for (label, lo, hi) in [
            ("8-29", 8, 30),
            ("30-99", 30, 100),
            ("100+", 100, usize::MAX),
        ] {
            eprintln!(
                "clocks over {label} symbols: {:?}",
                summary(&|n| n >= lo && n < hi)
            );
        }
        let mut pulls: Vec<_> = clocks
            .iter()
            .map(|(n, f, c, k, snr)| {
                (
                    (c.value() - k.value()) / c.sigma().hypot(k.sigma()),
                    *n,
                    f.clone(),
                    *c,
                    *k,
                    *snr,
                )
            })
            .collect();
        pulls.sort_by(|a, b| b.0.abs().total_cmp(&a.0.abs()));
        for p in pulls.iter().take(8) {
            eprintln!("  pull {:+.1}: {} symbols, {}, clock {:.1} +- {:.1}, carrier {:.2} +- {:.2}, {:.0} dB", p.0, p.1, p.2, p.3.value(), p.3.sigma(), p.4.value(), p.4.sigma(), p.5);
        }
        let mut by_carrier: std::collections::BTreeMap<i64, Vec<usize>> = Default::default();
        for (i, r) in clocks.iter().enumerate() {
            by_carrier
                .entry((r.3.value() / 2.0).round() as i64 * 2)
                .or_default()
                .push(i);
        }
        for (ppm, idx) in by_carrier.iter().filter(|(_, v)| v.len() >= 20) {
            let mut c = crate::state::SdrMetrics::fixture().net.wifi.clock_check;
            for &i in idx {
                let r = &clocks[i];
                c.add(&crate::state::WifiClock {
                    clock_ppm: r.2,
                    carrier_ppm: r.3,
                    preamble_ppm: r.3,
                    symbols: r.0,
                    when: std::time::Instant::now(),
                });
            }
            eprintln!(
                "  carrier near {ppm:+} ppm: {} frames, clock - carrier {:?}, chi2 {:?}",
                c.frames,
                c.mean(),
                c.chi2_per_dof()
            );
        }
    }
}
