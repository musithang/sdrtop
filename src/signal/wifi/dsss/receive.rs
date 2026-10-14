// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Finding a DSSS frame's SYNC, and reading its SFD and header.
//!
//! **The matched filter at every sample.** One symbol's 20 samples against
//! the Barker chips as our capture hears them ([`super::chips`]), every
//! sample, so each of a symbol's 20 timings is tried. The template has no
//! mean, so whatever DC the radio leaves cannot pass for chips (in the
//! window's energy it only makes the gate harder to pass). For noise, the
//! window's power along the template over its power is Beta(1, 19),
//! whatever the template; for a symbol of the chips it is near one.
//!
//! **A SYNC is a run, with a law.** One symbol passing is a coin with odds
//! [`per_symbol`]; a SYNC is [`RUN`] symbols in a row passing at one
//! timing, which disjoint windows of noise do with probability
//! [`SYNC_FALSE_ALARM`]. A frame passes at several timings near its best,
//! and at 1 or 2 Mb/s keeps the chips through its data, so a run reaching
//! [`RUN`] where a timing within four samples already holds one is the same
//! frame; otherwise it is a new one. A
//! run at any other timing is another frame. (Waiting instead for a quiet
//! stretch where nothing passed never ends in noise, where a window passes
//! one time in four: it swallowed three frames in four on a recording.)
//!
//! **Then the frame is read** from the run's first symbol on: the timing
//! refined to the sample, the carrier from the SYNC's differential
//! products squared (which DBPSK's half turns cannot cancel), the bits
//! descrambled until an SFD, long or short, and then 48 header bits, DBPSK
//! or DQPSK as the SFD said.

use num_complex::Complex;

use super::chips::{template, SYMBOL};
use super::header::{Descrambler, Fault, Header, Preamble, LONG_SFD, SHORT_SFD};
use crate::signal::dsp::uncertainty::Uncertain;

/// The chance that noise passes for a SYNC at one timing and one start.
pub const SYNC_FALSE_ALARM: f64 = 1e-9;

/// Symbols in a row a SYNC must show at one timing.
pub const RUN: usize = 16;

/// The chance one symbol of noise passes the gate: the run's false alarm,
/// shared out over its symbols.
pub fn per_symbol() -> f64 {
    SYNC_FALSE_ALARM.powf(1.0 / RUN as f64)
}

/// The gate on one symbol's normalised matched-filter power: where
/// Beta(1, 19)'s tail, (1 - t)^19, is [`per_symbol`].
pub fn threshold() -> f64 {
    1.0 - per_symbol().powf(1.0 / (SYMBOL as f64 - 1.0))
}

/// The furthest symbol a read needs after the run's first: a long SYNC
/// found at its very first symbol, its SFD, and the header at 1 Mb/s; and
/// eight more, since a symbol of noise passes the gate about one time in
/// four and a run can begin a few symbols before its frame (more than six
/// one time in 2500).
const SYMBOLS_READ: usize = 128 + 16 + 48 + 8;

/// The timing is refined this far either side of the run's.
const REFINE: usize = 3;

/// The timings either side of an event's that, holding a run of their own
/// already, make it the same frame: a frame passes at several timings
/// near its best, which reach a run one after another.
const NEAR: usize = 4;

/// A SYNC found: the stream pair its run's first symbol starts on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Found {
    pub first: u64,
}

/// The matched filter run along the stream, and the runs at each of a
/// symbol's 20 timings.
pub struct SyncDetector {
    /// The last 19 samples of the block before, for windows across blocks.
    tail: Vec<Complex<f32>>,
    /// Working rows, kept between blocks: a block's worth of memory
    /// allocated afresh each time costs the kernel more than the filter.
    buf: Vec<Complex<f32>>,
    out: Vec<Complex<f32>>,
    power: Vec<f32>,
    energy: Vec<f32>,
    pass: Vec<bool>,
    correlator: Correlator,
    runs: [u32; SYMBOL],
    gate: f64,
}

impl Default for SyncDetector {
    fn default() -> Self {
        Self {
            tail: Vec::new(),
            buf: Vec::new(),
            out: Vec::new(),
            power: Vec::new(),
            energy: Vec::new(),
            pass: Vec::new(),
            correlator: Correlator::new(),
            runs: [0; SYMBOL],
            gate: threshold(),
        }
    }
}

impl SyncDetector {
    pub fn new() -> Self {
        Self::default()
    }

    /// Look through a block starting at stream pair `first_pair`, the
    /// stream unbroken since the block before.
    pub fn push(&mut self, first_pair: u64, block: &[Complex<f32>]) -> Vec<Found> {
        let t = template();
        let et: f32 = t.iter().map(|v| v * v).sum();
        let tail = self.tail.len();
        let buf_first = first_pair - tail as u64;
        let len = tail + block.len();
        let mut found = Vec::new();
        if len >= SYMBOL {
            let m = len + 1 - SYMBOL;
            let buf = &mut self.buf;
            buf.clear();
            buf.extend_from_slice(&self.tail);
            buf.extend_from_slice(block);
            // Each window's energy, its 20 powers summed directly, eight
            // windows at a time in registers: no running sum to drift.
            let power = &mut self.power;
            power.clear();
            power.extend(buf.iter().map(|z| z.norm_sqr()));
            self.correlator.correlate(buf, &mut self.out);
            // Whether each window passes the gate: |c|^2 against gate x
            // energy x the template's energy, without a division.
            let gate = self.gate as f32 * et;
            let energy = &mut self.energy;
            energy.clear();
            energy.resize(m, 0.0);
            const LANES: usize = 8;
            let whole = m / LANES * LANES;
            for n in (0..whole).step_by(LANES) {
                let mut e = [0.0f32; LANES];
                for i in 0..SYMBOL {
                    let p = &power[n + i..n + i + LANES];
                    for j in 0..LANES {
                        e[j] += p[j];
                    }
                }
                energy[n..n + LANES].copy_from_slice(&e);
            }
            for (n, e) in energy.iter_mut().enumerate().skip(whole) {
                *e = power[n..n + SYMBOL].iter().sum();
            }
            let pass = &mut self.pass;
            pass.clear();
            pass.extend(
                self.out
                    .iter()
                    .zip(energy.iter())
                    .map(|(c, e)| c.norm_sqr() > gate * e),
            );
            // The runs at each timing: window n continues the run of window
            // n - 20, so the 20 timings are 20 lanes with no dependence
            // between them. A run reaching RUN is an event.
            let mut lanes: [u32; SYMBOL] = std::array::from_fn(|j| {
                self.runs[((buf_first + j as u64) % SYMBOL as u64) as usize]
            });
            let carried = lanes;
            let mut events = Vec::new();
            for (chunk, passes) in pass.chunks(SYMBOL).enumerate() {
                for (j, &p) in passes.iter().enumerate() {
                    lanes[j] = (lanes[j] + 1) * p as u32;
                    if lanes[j] == RUN as u32 {
                        events.push(chunk * SYMBOL + j);
                    }
                }
            }
            for (j, lane) in lanes.iter().enumerate() {
                self.runs[((buf_first + j as u64) % SYMBOL as u64) as usize] = *lane;
            }
            // Each event is a new frame unless a timing near it already
            // holds a run as long: then a frame already found (or about to
            // be, at that timing) is passing there too. After a gap, even a
            // SIFS between a frame and its acknowledgement, every run has
            // broken, so the next frame's first event stands.
            let run_at = |n: isize| -> u32 {
                // The run of window n's timing, as of window n.
                let mut k = 0u32;
                let mut at = n;
                while k < RUN as u32 {
                    if at < 0 {
                        return k + carried[(at.rem_euclid(SYMBOL as isize)) as usize];
                    }
                    if !pass[at as usize] {
                        return k;
                    }
                    k += 1;
                    at -= SYMBOL as isize;
                }
                k
            };
            for &e in &events {
                let e = e as isize;
                let near = (1..=NEAR as isize).any(|d| {
                    run_at(e - d) >= RUN as u32 || run_at(e + d - SYMBOL as isize) >= RUN as u32
                });
                if !near {
                    found.push(Found {
                        first: (buf_first as i64 + e as i64 - (SYMBOL * (RUN - 1)) as i64) as u64,
                    });
                }
            }
            let keep = &block[block.len().saturating_sub(SYMBOL - 1)..];
            if keep.len() < SYMBOL - 1 {
                let from = self.tail.len() + keep.len() - (SYMBOL - 1);
                self.tail.drain(..from);
            } else {
                self.tail.clear();
            }
            self.tail.extend_from_slice(keep);
        } else {
            self.tail.extend_from_slice(block);
        }
        found
    }
}

/// The matched filter by fast transforms (overlap-save): every window's
/// sum against the template, the same sums as one window at a time, at a
/// fraction of the multiplies, with the transform library's own vector
/// code where the processor has it.
struct Correlator {
    forward: std::sync::Arc<dyn rustfft::Fft<f32>>,
    inverse: std::sync::Arc<dyn rustfft::Fft<f32>>,
    /// The template's transform, conjugated, over the transform's length.
    shape: Vec<Complex<f32>>,
    seg: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
}

/// The transform's length: 493 windows from each, the fastest of 512 to
/// 4096 on the slow machine this is judged on.
const SPAN: usize = 512;

impl Correlator {
    fn new() -> Self {
        let mut planner = rustfft::FftPlanner::new();
        let forward = planner.plan_fft_forward(SPAN);
        let inverse = planner.plan_fft_inverse(SPAN);
        let mut shape = vec![Complex::new(0.0f32, 0.0); SPAN];
        for (s, v) in shape.iter_mut().zip(template()) {
            *s = Complex::new(*v, 0.0);
        }
        forward.process(&mut shape);
        for s in &mut shape {
            *s = s.conj() / SPAN as f32;
        }
        let scratch = vec![
            Complex::new(0.0, 0.0);
            forward
                .get_inplace_scratch_len()
                .max(inverse.get_inplace_scratch_len())
        ];
        Self {
            forward,
            inverse,
            shape,
            seg: vec![Complex::new(0.0, 0.0); SPAN],
            scratch,
        }
    }

    /// `out[n]` the sum over the template of `x[n..n + 20]`, for every
    /// window `x` holds whole.
    fn correlate(&mut self, x: &[Complex<f32>], out: &mut Vec<Complex<f32>>) {
        out.clear();
        if x.len() < SYMBOL {
            return;
        }
        let m = x.len() + 1 - SYMBOL;
        let step = SPAN + 1 - SYMBOL;
        let mut from = 0;
        while from < m {
            let held = (x.len() - from).min(SPAN);
            self.seg[..held].copy_from_slice(&x[from..from + held]);
            self.seg[held..].fill(Complex::new(0.0, 0.0));
            self.forward
                .process_with_scratch(&mut self.seg, &mut self.scratch);
            for (z, h) in self.seg.iter_mut().zip(&self.shape) {
                *z *= h;
            }
            self.inverse
                .process_with_scratch(&mut self.seg, &mut self.scratch);
            let take = step.min(m - from);
            out.extend_from_slice(&self.seg[..take]);
            from += step;
        }
    }
}

/// A DSSS frame read as far as its header.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Read {
    pub preamble: Preamble,
    pub header: Result<Header, Fault>,
    /// The stream pair the frame's first SYNC symbol started on: with its
    /// airtime, where it ends.
    pub start: u64,
    /// The carrier against our tuning, in hertz.
    pub offset_hz: Uncertain,
}

/// What a SYNC turned out to be.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Verdict {
    Frame(Read),
    /// A SYNC with no SFD after it where one should be.
    NoSfd,
}

/// The stretch of the stream a found SYNC is read from: the refinement
/// either side, and every symbol a long frame's header could need.
pub fn needs(found: &Found) -> Option<(u64, usize)> {
    let from = found.first.checked_sub(REFINE as u64)?;
    Some((from, 2 * REFINE + SYMBOL * (SYMBOLS_READ + 1)))
}

/// The matched filter's output for the symbol whose window starts at `at`
/// in `samples`, and its normalised power.
fn despread(samples: &[Complex<f32>], at: usize) -> (Complex<f64>, f64) {
    let t = template();
    let w = &samples[at..at + SYMBOL];
    let c: Complex<f64> = w
        .iter()
        .zip(t)
        .map(|(z, v)| Complex::new(z.re as f64, z.im as f64) * *v as f64)
        .sum();
    let e: f64 = w.iter().map(|z| z.norm_sqr() as f64).sum();
    let et: f64 = t.iter().map(|v| (*v as f64).powi(2)).sum();
    (
        c,
        if e > 0.0 {
            c.norm_sqr() / (e * et)
        } else {
            0.0
        },
    )
}

/// Read the frame whose SYNC was `found`, from `samples` starting at
/// `first_pair`. `None` unless its stretch is held.
pub fn examine(samples: &[Complex<f32>], first_pair: u64, found: &Found) -> Option<Verdict> {
    let (from, len) = needs(found)?;
    let held = from.checked_sub(first_pair)? as usize;
    if samples.len() < held + len {
        return None;
    }
    // The timing: the shift of the run's within REFINE whose symbols carry
    // the most.
    let run_at = held + REFINE;
    let at = (run_at - REFINE..=run_at + REFINE)
        .max_by(|&a, &b| {
            let score = |s: usize| {
                (0..RUN)
                    .map(|k| despread(samples, s + SYMBOL * k).1)
                    .sum::<f64>()
            };
            score(a).total_cmp(&score(b))
        })
        .unwrap_or(run_at);
    let (symbols, rho): (Vec<Complex<f64>>, Vec<f64>) = (0..=SYMBOLS_READ)
        .map(|k| despread(samples, at + SYMBOL * k))
        .unzip();
    let d: Vec<Complex<f64>> = symbols.windows(2).map(|w| w[1] * w[0].conj()).collect();
    // The products between two symbols that both passed the gate: the
    // run may have begun on a symbol or two of noise.
    let gate = threshold();
    let chips = |range: std::ops::Range<usize>| -> Vec<Complex<f64>> {
        range
            .filter(|&i| rho[i] > gate && rho[i + 1] > gate)
            .map(|i| d[i])
            .collect()
    };

    // The carrier from the run's products, squared: a half turn's sign
    // gone, twice the phase a symbol turns left.
    let carrier = |products: &[Complex<f64>]| -> Option<Uncertain> {
        if products.len() < 2 {
            return None;
        }
        let sum: Complex<f64> = products.iter().map(|p| p * p).sum();
        let turn = sum.arg() / 2.0;
        let spread: Vec<f64> = products
            .iter()
            .map(|p| (p * p * Complex::from_polar(1.0, -2.0 * turn)).arg() / 2.0)
            .collect();
        let n = spread.len() as f64;
        let var = spread.iter().map(|a| a * a).sum::<f64>() / (n - 1.0).max(1.0) / n;
        let per_us = 1.0 / (std::f64::consts::TAU * 1e-6);
        Some(Uncertain::from_variance(
            turn * per_us,
            var * per_us * per_us,
        ))
    };
    let Some(first) = carrier(&chips(0..RUN + 2)) else {
        return Some(Verdict::NoSfd);
    };
    let turn = Complex::from_polar(1.0, -std::f64::consts::TAU * first.value() * 1e-6);

    // DBPSK bits, descrambled, until an SFD.
    let mut descrambler = Descrambler::default();
    let mut last = [false; 16];
    let mut sfd = None;
    for (k, p) in d.iter().enumerate() {
        let bit = descrambler.bit((p * turn).re < 0.0);
        last.rotate_left(1);
        last[15] = bit;
        if k >= 7 + 16 {
            if last == LONG_SFD {
                sfd = Some((k, Preamble::Long));
                break;
            }
            if last == SHORT_SFD {
                sfd = Some((k, Preamble::Short));
                break;
            }
        }
        if k + 1 + 24 >= d.len() {
            break;
        }
    }
    // An SFD too late for its header to be held is no SFD: bit errors took
    // the real one and the pattern turned up by chance further on.
    let header_symbols = match sfd {
        Some((_, Preamble::Long)) => 48,
        _ => 24,
    };
    let Some((k, preamble)) = sfd.filter(|&(k, _)| k + 1 + header_symbols <= d.len()) else {
        return Some(Verdict::NoSfd);
    };
    // The carrier again over the whole SYNC seen, before the SFD.
    let Some(offset_hz) = carrier(&chips(0..k.saturating_sub(16).max(RUN))) else {
        return Some(Verdict::NoSfd);
    };
    let turn = Complex::from_polar(1.0, -std::f64::consts::TAU * offset_hz.value() * 1e-6);

    // d[k] is the change into symbol k + 1: the SFD's last bit. The frame
    // began SYNC and SFD symbols before the one after it.
    let sync = match preamble {
        Preamble::Long => 128,
        Preamble::Short => 56,
    };
    let start_symbol = (k + 1 + 1) as i64 - (sync + 16) as i64;
    let start = (first_pair as i64 + at as i64 + SYMBOL as i64 * start_symbol) as u64;

    let mut bits = Vec::with_capacity(48);
    match preamble {
        Preamble::Long => {
            for p in &d[k + 1..k + 1 + 48] {
                bits.push(descrambler.bit((p * turn).re < 0.0));
            }
        }
        Preamble::Short => {
            // DQPSK dibits, d0 first (Table 15-8): 00 0, 01 a quarter turn
            // counterclockwise, 11 a half, 10 a quarter clockwise.
            for p in &d[k + 1..k + 1 + 24] {
                let z = p * turn;
                let (d0, d1) = if z.re.abs() >= z.im.abs() {
                    if z.re >= 0.0 {
                        (false, false)
                    } else {
                        (true, true)
                    }
                } else if z.im >= 0.0 {
                    (false, true)
                } else {
                    (true, false)
                };
                bits.push(descrambler.bit(d0));
                bits.push(descrambler.bit(d1));
            }
        }
    }
    Some(Verdict::Frame(Read {
        preamble,
        header: Header::from_bits(&bits),
        start,
        offset_hz,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::dsp::testkit::Rng;
    use crate::signal::wifi::dsss::header::Rate;
    use crate::signal::wifi::dsss::transmit::frame;

    const LEAD: usize = 700;

    /// `frame` `LEAD` samples into noise `snr_db` below it a sample, offset
    /// by `offset_hz`.
    fn on_air(f: &[Complex<f64>], snr_db: f64, offset_hz: f64, rng: &mut Rng) -> Vec<Complex<f32>> {
        let power = f.iter().map(|z| z.norm_sqr()).sum::<f64>() / f.len() as f64;
        // Enough after it for the longest read a SYNC can ask for.
        let mut x = rng.noise(LEAD + f.len() + 4_400, 10f64.powf(-snr_db / 10.0));
        let turn = std::f64::consts::TAU * offset_hz / 20e6;
        for (n, (o, z)) in x[LEAD..].iter_mut().zip(f).enumerate() {
            let z = z / power.sqrt() * Complex::from_polar(1.0, turn * n as f64);
            *o += Complex::new(z.re as f32, z.im as f32);
        }
        x
    }

    /// Found in blocks of 4096, then read.
    fn received(x: &[Complex<f32>]) -> Option<Verdict> {
        let mut d = SyncDetector::new();
        let mut found = Vec::new();
        for (i, b) in x.chunks(4096).enumerate() {
            found.extend(d.push((i * 4096) as u64, b));
        }
        let f = found.first()?;
        examine(x, 0, f)
    }

    fn header(rate: Rate, locked: bool, length_us: u16) -> Header {
        Header {
            rate,
            locked,
            extension: false,
            length_us,
        }
    }

    /// Long and short frames at every rate, their locked-clocks bit either
    /// way, found and read at 15 and 5 dB a sample (25 and 15 after the
    /// Barker code's gain) and 60 kHz off; their start placed to the
    /// sample, and their offset read.
    #[test]
    fn frames_are_found_and_their_headers_read() {
        let mut rng = Rng::new(130);
        for snr in [15.0, 5.0] {
            for preamble in [Preamble::Long, Preamble::Short] {
                for (i, rate) in [Rate::M1, Rate::M2, Rate::M5_5, Rate::M11]
                    .into_iter()
                    .enumerate()
                {
                    if preamble == Preamble::Short && rate == Rate::M1 {
                        continue;
                    }
                    let h = header(rate, i % 2 == 0, 300 + 17 * i as u16);
                    let f = frame(preamble, &h, 40, 4, &mut rng);
                    let x = on_air(&f, snr, 60e3, &mut rng);
                    match received(&x) {
                        Some(Verdict::Frame(r)) => {
                            assert_eq!(r.preamble, preamble, "{snr} {rate:?}");
                            assert_eq!(r.header, Ok(h), "{snr} {preamble:?} {rate:?}");
                            assert_eq!(r.start, (LEAD + 4 * 20) as u64, "{snr} {preamble:?}");
                            assert!(
                                (r.offset_hz.value() - 60e3).abs()
                                    < 4.0 * r.offset_hz.sigma() + 200.0,
                                "{:?}",
                                r.offset_hz
                            );
                        }
                        other => panic!("{snr} dB {preamble:?} {rate:?}: {other:?}"),
                    }
                }
            }
        }
    }

    /// At 0 dB a sample (10 after the gain) most headers still read: 45 of
    /// 50 at least.
    #[test]
    fn at_zero_db_most_headers_read() {
        let mut rng = Rng::new(131);
        let h = header(Rate::M11, true, 1234);
        let mut good = 0;
        for _ in 0..50 {
            let f = frame(Preamble::Long, &h, 20, 4, &mut rng);
            if let Some(Verdict::Frame(r)) = received(&on_air(&f, 0.0, 20e3, &mut rng)) {
                good += (r.header == Ok(h)) as usize;
            }
        }
        eprintln!("0 dB: {good} of 50 headers read");
        assert!(good >= 45, "{good}");
    }

    /// **The gate's law**: on noise, one symbol's normalised matched-filter
    /// power passes the threshold as often as Beta(1, 19) says, and a run
    /// of `RUN` at that rate is `SYNC_FALSE_ALARM`.
    #[test]
    fn noise_passes_the_gate_as_its_law_says() {
        let p = per_symbol();
        assert!((p.powi(RUN as i32) - SYNC_FALSE_ALARM).abs() < 1e-12);
        let mut rng = Rng::new(132);
        let x = rng.noise(200_000, 1.0);
        let t = super::super::chips::template();
        let et: f64 = t.iter().map(|v| (*v as f64).powi(2)).sum();
        let gate = threshold();
        let mut pass = 0;
        let trials = x.len() / 20 - 1;
        for k in 0..trials {
            let w = &x[20 * k..20 * k + 20];
            let c: Complex<f64> = w
                .iter()
                .zip(t)
                .map(|(z, v)| Complex::new(z.re as f64, z.im as f64) * *v as f64)
                .sum();
            let e: f64 = w.iter().map(|z| z.norm_sqr() as f64).sum();
            pass += (c.norm_sqr() / (e * et) > gate) as usize;
        }
        let rate = pass as f64 / trials as f64;
        let se = (p * (1.0 - p) / trials as f64).sqrt();
        assert!((rate - p).abs() < 4.0 * se, "{rate} against {p}");
        // And no SYNC found in it.
        let mut d = SyncDetector::new();
        let found: Vec<Found> = x
            .chunks(4096)
            .enumerate()
            .flat_map(|(i, b)| d.push((i * 4096) as u64, b))
            .collect();
        assert!(found.is_empty(), "{found:?}");
    }

    /// Frames one after another, as the air has them: an 802.11b frame,
    /// noise, an OFDM frame, 10 us of quiet, another 802.11b frame at
    /// another timing, then one at the first one's: each found, once.
    #[test]
    fn frames_in_a_row_are_each_found_once() {
        use crate::signal::wifi::ppdu::ppdu;
        use crate::signal::wifi::signal_field::Rate as OfdmRate;
        let mut rng = Rng::new(138);
        let h = header(Rate::M2, true, 400);
        let unit = |f: Vec<Complex<f64>>| {
            let p = f.iter().map(|z| z.norm_sqr()).sum::<f64>() / f.len() as f64;
            f.into_iter().map(move |z| z / p.sqrt()).collect::<Vec<_>>()
        };
        let mut stream: Vec<Complex<f64>> = vec![Complex::new(0.0, 0.0); 700];
        stream.extend(unit(frame(Preamble::Long, &h, 30, 0, &mut rng)));
        stream.extend(vec![Complex::new(0.0, 0.0); 3_000]);
        let psdu: Vec<u8> = (0..200).collect();
        stream.extend(unit(ppdu(&psdu, OfdmRate::R24, 0x5d)));
        stream.extend(vec![Complex::new(0.0, 0.0); 200 + 7]);
        stream.extend(unit(frame(Preamble::Short, &h, 30, 0, &mut rng)));
        stream.extend(vec![Complex::new(0.0, 0.0); 200 - 7]);
        stream.extend(unit(frame(Preamble::Long, &h, 30, 0, &mut rng)));
        stream.extend(vec![Complex::new(0.0, 0.0); 5_000]);
        let noise = rng.noise(stream.len(), 0.1);
        let x: Vec<Complex<f32>> = stream
            .iter()
            .zip(noise)
            .map(|(z, n)| Complex::new(z.re as f32, z.im as f32) + n)
            .collect();
        let mut d = SyncDetector::new();
        let found: Vec<Found> = x
            .chunks(4096)
            .enumerate()
            .flat_map(|(i, b)| d.push((i * 4096) as u64, b))
            .collect();
        assert_eq!(found.len(), 3, "{found:?}");
        let read: Vec<Preamble> = found
            .iter()
            .map(|f| match examine(&x, 0, f) {
                Some(Verdict::Frame(r)) => {
                    assert_eq!(r.header, Ok(h));
                    r.preamble
                }
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(read, [Preamble::Long, Preamble::Short, Preamble::Long]);
    }

    /// An acknowledgement a SIFS after its frame, at exactly the frame's
    /// timing: found as a frame of its own, at 20 and at 8 dB.
    #[test]
    fn an_acknowledgement_at_the_same_timing_is_its_own_frame() {
        let mut rng = Rng::new(141);
        for snr in [20.0, 8.0] {
            for _ in 0..10 {
                let data = header(Rate::M1, false, 800);
                let ack = header(Rate::M1, false, 112);
                let mut f = frame(Preamble::Long, &data, 100, 4, &mut rng);
                // 10 us of silence, then the acknowledgement, on the same
                // 20-sample grid.
                f.extend(vec![Complex::new(0.0, 0.0); 200 - 8 * 20]);
                f.extend(frame(Preamble::Long, &ack, 112, 4, &mut rng));
                let x = on_air(&f, snr, 10e3, &mut rng);
                let mut d = SyncDetector::new();
                let found: Vec<Found> = x
                    .chunks(4096)
                    .enumerate()
                    .flat_map(|(i, b)| d.push((i * 4096) as u64, b))
                    .collect();
                let headers: Vec<_> = found
                    .iter()
                    .filter_map(|f| match examine(&x, 0, f) {
                        Some(Verdict::Frame(r)) => r.header.ok(),
                        _ => None,
                    })
                    .collect();
                assert_eq!(headers, [data, ack], "{snr} dB: {found:?}");
            }
        }
    }

    /// A long frame at 1 Mb/s, 1.5 ms of data after its header, is one
    /// frame: followed to its end, never found again inside it.
    #[test]
    fn a_long_frame_is_found_once() {
        let mut rng = Rng::new(139);
        for snr in [20.0, 8.0] {
            let h = header(Rate::M1, true, 1500);
            let x = on_air(
                &frame(Preamble::Long, &h, 1500, 4, &mut rng),
                snr,
                30e3,
                &mut rng,
            );
            let mut d = SyncDetector::new();
            let found: Vec<Found> = x
                .chunks(4096)
                .enumerate()
                .flat_map(|(i, b)| d.push((i * 4096) as u64, b))
                .collect();
            assert_eq!(found.len(), 1, "{snr} dB: {found:?}");
        }
    }

    /// DC the radio leaves, ten times the noise's power, is no SYNC, and a
    /// frame under it is read all the same.
    #[test]
    fn dc_is_no_sync() {
        let mut rng = Rng::new(137);
        let dc = Complex::new(2.2f32, -2.2);
        let x: Vec<Complex<f32>> = rng.noise(100_000, 1.0).iter().map(|z| z + dc).collect();
        let mut d = SyncDetector::new();
        let found: Vec<Found> = x
            .chunks(4096)
            .enumerate()
            .flat_map(|(i, b)| d.push((i * 4096) as u64, b))
            .collect();
        assert!(found.is_empty(), "{found:?}");
        let h = header(Rate::M1, true, 2400);
        let f = on_air(
            &frame(Preamble::Long, &h, 40, 4, &mut rng),
            10.0,
            0.0,
            &mut rng,
        );
        let with_dc: Vec<Complex<f32>> = f.iter().map(|z| z + dc * 0.3).collect();
        match received(&with_dc) {
            Some(Verdict::Frame(r)) => assert_eq!(r.header, Ok(h)),
            other => panic!("{other:?}"),
        }
    }

    /// OFDM frames, whatever their rate, are never a SYNC.
    #[test]
    fn ofdm_is_never_a_sync() {
        use crate::signal::wifi::ppdu::ppdu;
        use crate::signal::wifi::signal_field::Rate as OfdmRate;
        let mut rng = Rng::new(133);
        let psdu: Vec<u8> = (0..300).map(|i| (i * 13) as u8).collect();
        for rate in [OfdmRate::R6, OfdmRate::R24, OfdmRate::R54] {
            for _ in 0..3 {
                let x = on_air(&ppdu(&psdu, rate, 0x5d), 20.0, 0.0, &mut rng);
                let mut d = SyncDetector::new();
                let found: Vec<Found> = x
                    .chunks(4096)
                    .enumerate()
                    .flat_map(|(i, b)| d.push((i * 4096) as u64, b))
                    .collect();
                assert!(found.is_empty(), "{rate:?}: {found:?}");
            }
        }
    }

    /// The transforms' sums are the windows' own, to f32's precision,
    /// over blocks of any length.
    #[test]
    fn the_fast_correlation_is_the_direct_one() {
        let mut rng = Rng::new(136);
        let t = super::super::chips::template();
        let mut c = Correlator::new();
        for len in [19, 20, 21, 1024, 1025, 3000, 4096] {
            let x = rng.noise(len, 1.0);
            let mut out = Vec::new();
            c.correlate(&x, &mut out);
            assert_eq!(out.len(), (len + 1).saturating_sub(SYMBOL), "{len}");
            for (n, o) in out.iter().enumerate() {
                let direct: Complex<f32> =
                    x[n..n + SYMBOL].iter().zip(t).map(|(z, v)| z * *v).sum();
                assert!((o - direct).norm() < 1e-4, "{len} {n}: {o} {direct}");
            }
        }
    }

    /// What needs says is what examine reads.
    #[test]
    fn needs_is_what_examine_reads() {
        let mut rng = Rng::new(134);
        let h = header(Rate::M2, false, 500);
        let x = on_air(
            &frame(Preamble::Long, &h, 40, 4, &mut rng),
            15.0,
            0.0,
            &mut rng,
        );
        let mut d = SyncDetector::new();
        let f = d.push(0, &x)[0];
        let (from, len) = needs(&f).unwrap();
        let cut = &x[from as usize..from as usize + len];
        assert!(matches!(examine(cut, from, &f), Some(Verdict::Frame(_))));
        assert!(examine(&cut[..len - 1], from, &f).is_none());
    }

    /// **What the detector costs, measured.** Not a check: by hand, in
    /// release (`cargo test --release what_the_dsss_detector_costs --
    /// --ignored --nocapture`), a second of noise in blocks of 131072.
    #[test]
    #[ignore]
    fn what_the_dsss_detector_costs() {
        let mut rng = Rng::new(135);
        let block = rng.noise(131_072, 1.0);
        let mut d = SyncDetector::new();
        let blocks = 20_000_000 / 131_072 + 1;
        let start = std::time::Instant::now();
        for i in 0..blocks {
            std::hint::black_box(d.push((i * 131_072) as u64, std::hint::black_box(&block)));
        }
        let secs = start.elapsed().as_secs_f64();
        eprintln!(
            "{:.0} ms for a second of samples: {:.0} % of real time",
            secs * 1e3,
            secs * 100.0 * 20e6 / (blocks * 131_072) as f64
        );
    }

    /// **What a recording's 802.11b frames say, by hand**
    /// (`SDRTOP_REPLAY=path.sigmf-data`, ci8 at 20 Msps on a channel
    /// centre): every SYNC found and read, counted by preamble, rate,
    /// length and locked clocks, with the refusals. `cargo test --release
    /// a_recordings_802_11b -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn a_recordings_802_11b() {
        let Ok(path) = std::env::var("SDRTOP_REPLAY") else {
            return;
        };
        let bytes = std::fs::read(&path).unwrap();
        let pairs = bytes.len() / 2;
        let pair = |p: &[u8; 2]| Complex::new(p[0] as i8 as f32 / 128.0, p[1] as i8 as f32 / 128.0);
        // The radio's DC off, as the worker's feed takes it, summed in f64.
        let dc = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| {
                let z = pair(p);
                Complex::new(z.re as f64, z.im as f64)
            })
            .sum::<Complex<f64>>()
            / pairs as f64;
        let dc = Complex::new(dc.re as f32, dc.im as f32);
        let stretch = |from: usize, len: usize| -> Vec<Complex<f32>> {
            bytes[2 * from..2 * (from + len).min(pairs)]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|p| pair(p) - dc)
                .collect()
        };
        let mut d = SyncDetector::new();
        let mut kinds = std::collections::BTreeMap::new();
        let (mut found, mut no_sfd, mut refused) = (0, 0, std::collections::BTreeMap::new());
        let mut offsets = Vec::new();
        let (mut last_end, mut inside_read) = (0u64, 0);
        for at in (0..pairs).step_by(131_072) {
            for f in d.push(at as u64, &stretch(at, 131_072)) {
                found += 1;
                let Some((from, len)) = needs(&f) else {
                    continue;
                };
                match examine(&stretch(from as usize, len), from, &f) {
                    Some(Verdict::Frame(r)) => match r.header {
                        Ok(h) => {
                            last_end = r.start + 20 * h.airtime_us(r.preamble) as u64;
                            let bucket = match h.octets() {
                                0..=63 => "0-63",
                                64..=255 => "64-255",
                                256..=511 => "256-511",
                                _ => "512+",
                            };
                            *kinds
                                .entry((
                                    format!("{:?}", r.preamble),
                                    h.rate.mbps(),
                                    bucket,
                                    h.locked,
                                ))
                                .or_insert(0) += 1;
                            offsets.push(r.offset_hz.value());
                        }
                        Err(e) => *refused.entry(e.label()).or_insert(0) += 1,
                    },
                    Some(Verdict::NoSfd) => {
                        no_sfd += 1;
                        if f.first < last_end {
                            inside_read += 1;
                        }
                    }
                    None => {}
                }
            }
        }
        let secs = pairs as f64 / 20e6;
        eprintln!("{secs:.1} s: {found} SYNCs, {no_sfd} with no SFD ({inside_read} inside a frame read before), refused {refused:?}");
        for (k, n) in &kinds {
            eprintln!("  {k:?}: {n} ({:.1}/s)", *n as f64 / secs);
        }
        if !offsets.is_empty() {
            offsets.sort_by(f64::total_cmp);
            eprintln!(
                "  offsets: median {:.1} kHz, from {:.1} to {:.1}",
                offsets[offsets.len() / 2] / 1e3,
                offsets[0] / 1e3,
                offsets[offsets.len() - 1] / 1e3
            );
        }
    }
}
