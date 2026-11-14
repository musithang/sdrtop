// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The FT8 receiver held to known transmissions, the way the NET and POCSAG
//! receivers are held to theirs: cycles built from the closed-form waveform at
//! stated signal-to-noise ratios, and rows comparing what the receiver found
//! with what was sent.
//!
//! **SNR is FT8's own: signal power over noise power in 2500 Hz** (the QEX
//! paper, section 6). The noise here is complex and white across the whole
//! 12 kHz, so its power in 2500 Hz is 2500/12000 of the total; the ratio of
//! the two powers is the same whether the signal is written as the paper's
//! real audio or as this complex baseband.
//!
//! Test-only. The rows printed by the ignored `measure_` tests are the numbers
//! written into the plan; the plain tests hold the bars.

use std::f64::consts::TAU;

use num_complex::Complex;

use super::crc;
use super::cycle;
use super::fields::G15;
use super::hash::HashBook;
use super::ldpc;
use super::message::{self, Call, Message};
use super::modulate::{self, Waveform, NOMINAL_START, SYMBOLS};
use super::search::{self, Baseband, Spectrogram, DOWN_RATE, RATE};
use crate::signal::dsp::testkit::Rng;

/// One 15 s cycle at [`RATE`].
pub const CYCLE_SAMPLES: usize = 15 * RATE as usize;

/// A transmission put into a test cycle.
#[derive(Clone, Copy, Debug)]
pub struct Sent {
    pub payload: u128,
    pub f0: f64,
    /// Seconds from the nominal start (positive: late).
    pub dt: f64,
    pub snr_db: f64,
    pub drift: f64,
}

impl Sent {
    pub fn start(&self) -> f64 {
        NOMINAL_START + self.dt
    }

    pub fn tones(&self) -> [u8; SYMBOLS] {
        modulate::channel_symbols(&ldpc::encode(crc::with_crc(self.payload)))
    }
}

/// A cycle of unit-power white noise with the transmissions added at their
/// SNRs.
pub fn cycle(sent: &[Sent], rng: &mut Rng) -> Vec<Complex<f32>> {
    let mut x = rng.noise(CYCLE_SAMPLES, 1.0);
    let noise_in_2500 = 2500.0 / RATE;
    for s in sent {
        let amplitude = (noise_in_2500 * 10f64.powf(s.snr_db / 10.0)).sqrt();
        let w = Waveform {
            f0: s.f0,
            start: s.start(),
            amplitude,
            drift: s.drift,
            phase: rng.unit() * std::f64::consts::TAU,
        };
        modulate::add(&s.tones(), &w, RATE, &mut x);
    }
    x
}

/// Paths a fading channel's gain is the sum of: enough that the gain's
/// amplitude is close to Rayleigh.
const FADING_PATHS: usize = 32;

/// [`cycle`] with each transmission through its own fading path: its gain
/// a sum of [`FADING_PATHS`] equal paths whose Doppler shifts are Gaussian
/// with a two-sided spread (two standard deviations) of `spread_hz`, the
/// shape of the Watterson model an HF channel simulator uses. The gain's
/// power is one on average but not in any one cycle, so beside the cycle
/// comes each transmission's SNR as it arrived: its power over the 12.64 s.
pub fn faded(sent: &[Sent], spread_hz: f64, rng: &mut Rng) -> (Vec<Complex<f32>>, Vec<f64>) {
    let mut x = rng.noise(CYCLE_SAMPLES, 1.0);
    let noise_in_2500 = 2500.0 / RATE;
    let mut arrived = Vec::new();
    for s in sent {
        let amplitude = (noise_in_2500 * 10f64.powf(s.snr_db / 10.0)).sqrt();
        let mut one = vec![Complex::new(0.0f32, 0.0); CYCLE_SAMPLES];
        let w = Waveform {
            f0: s.f0,
            start: s.start(),
            amplitude,
            drift: s.drift,
            phase: 0.0,
        };
        modulate::add(&s.tones(), &w, RATE, &mut one);
        // Each path a phasor turned by its Doppler shift every sample.
        let (mut now, step): (Vec<Complex<f64>>, Vec<Complex<f64>>) = (0..FADING_PATHS)
            .map(|_| {
                let f = rng.normal_pair().0 * spread_hz / 2.0;
                (
                    Complex::from_polar(1.0, rng.unit() * TAU),
                    Complex::from_polar(1.0, TAU * f / RATE),
                )
            })
            .unzip();
        let norm = (FADING_PATHS as f64).sqrt();
        let first = (s.start() * RATE).ceil().max(0.0) as usize;
        let last = (first + (SYMBOLS as f64 * modulate::SYMBOL_SECONDS * RATE) as usize)
            .min(CYCLE_SAMPLES);
        let mut power = 0.0;
        for (i, (v, o)) in x.iter_mut().zip(&one).enumerate() {
            let g = now.iter().sum::<Complex<f64>>() / norm;
            for (p, d) in now.iter_mut().zip(&step) {
                *p *= d;
            }
            if (first..last).contains(&i) {
                power += g.norm_sqr();
            }
            if o.re != 0.0 || o.im != 0.0 {
                let g = g * Complex::new(f64::from(o.re), f64::from(o.im));
                *v += Complex::new(g.re as f32, g.im as f32);
            }
        }
        power /= (last - first).max(1) as f64;
        arrived.push(s.snr_db + 10.0 * power.log10());
    }
    (x, arrived)
}

/// `n` transmissions at one SNR, at distinct frequencies at least 60 Hz
/// apart, with DT from -0.3 to +1.0 s.
pub fn spread(n: usize, snr_db: f64, rng: &mut Rng) -> Vec<Sent> {
    spread_apart(n, snr_db, 60.0, rng)
}

/// [`spread`] with the frequencies at least `gap` Hz apart. Drawn at random
/// one by one, so a gap the band cannot fit `n` times would never finish:
/// refused instead.
pub fn spread_apart(n: usize, snr_db: f64, gap: f64, rng: &mut Rng) -> Vec<Sent> {
    assert!(
        n as f64 * gap <= 2700.0 * 0.6,
        "{n} transmissions {gap} Hz apart do not fit 2700 Hz drawn at random"
    );
    let mut out: Vec<Sent> = Vec::new();
    while out.len() < n {
        let f0 = 200.0 + rng.unit() * 2700.0;
        if out.iter().any(|s| (s.f0 - f0).abs() < gap) {
            continue;
        }
        let payload =
            (u128::from(rng.next_u64()) << 64 | u128::from(rng.next_u64())) & ((1u128 << 77) - 1);
        out.push(Sent {
            payload,
            f0,
            dt: -0.3 + rng.unit() * 1.3,
            snr_db,
            drift: 0.0,
        });
    }
    out
}

/// Of `sent`, those the search found (a candidate within a bin and a frame),
/// with the fine estimate's errors in start (s) and tone 0 (Hz).
pub fn search_rows(x: &[Complex<f32>], sent: &[Sent]) -> (usize, Vec<(f64, f64)>) {
    let spec = Spectrogram::new(x);
    let cands = search::candidates(&spec);
    let bb = Baseband::new(x);
    let mut errors = Vec::new();
    for s in sent {
        let Some(c) = cands
            .iter()
            .find(|c| (c.f0 - s.f0).abs() <= 3.2 && (c.start - s.start()).abs() <= 0.1)
        else {
            continue;
        };
        let f = search::refine(&bb, c);
        errors.push((f.start - s.start(), f.f0 - s.f0));
    }
    (cands.len(), errors)
}

fn rms(v: impl Iterator<Item = f64> + Clone) -> f64 {
    let n = v.clone().count().max(1) as f64;
    (v.map(|x| x * x).sum::<f64>() / n).sqrt()
}

#[test]
fn a_clear_signal_is_found_where_it_was_sent() {
    let mut rng = Rng::new(3);
    let sent = spread(6, -12.0, &mut rng);
    let x = cycle(&sent, &mut rng);
    let (_, errors) = search_rows(&x, &sent);
    assert_eq!(errors.len(), sent.len());
    for (dt, df) in &errors {
        assert!(dt.abs() < 0.010, "start off by {dt} s");
        assert!(df.abs() < 0.3, "tone 0 off by {df} Hz");
    }
}

#[test]
fn a_weak_signal_is_still_a_candidate() {
    let mut rng = Rng::new(4);
    let sent = spread(6, -20.0, &mut rng);
    let x = cycle(&sent, &mut rng);
    let (_, errors) = search_rows(&x, &sent);
    assert!(errors.len() >= 5, "found {} of 6 at -20 dB", errors.len());
}

#[test]
fn drift_and_a_late_start_do_not_lose_the_signal() {
    let mut rng = Rng::new(5);
    let mut sent = spread(3, -14.0, &mut rng);
    sent[0].drift = 2.0;
    sent[1].dt = 2.0;
    sent[2].dt = -1.5;
    let x = cycle(&sent, &mut rng);
    let (_, errors) = search_rows(&x, &sent);
    assert_eq!(errors.len(), 3);
    for (dt, df) in &errors {
        assert!(dt.abs() < 0.02 && df.abs() < 0.5, "{dt} s, {df} Hz");
    }
}

/// The search's rows: found of sent, RMS start and frequency error, and how
/// many candidates noise alone raises.
/// `cargo test --release ft8::conformance::measure_the_search -- --ignored --nocapture`
#[test]
#[ignore]
fn measure_the_search() {
    let mut rng = Rng::new(17);
    println!("SNR    found      rms dt     rms df    candidates/cycle");
    for snr in [-8.0, -12.0, -16.0, -18.0, -20.0, -21.0, -22.0, -23.0, -24.0] {
        let (mut found, mut total, mut cands) = (0, 0, 0);
        let mut errs = Vec::new();
        for _ in 0..8 {
            let sent = spread(10, snr, &mut rng);
            let x = cycle(&sent, &mut rng);
            let (n, e) = search_rows(&x, &sent);
            found += e.len();
            total += sent.len();
            cands += n;
            errs.extend(e);
        }
        println!(
            "{snr:>5} {found:>3}/{total:<3}  {:>7.1} ms  {:>6.3} Hz  {:>5}",
            rms(errs.iter().map(|e| e.0)) * 1000.0,
            rms(errs.iter().map(|e| e.1)),
            cands / 8
        );
    }
    let mut n = 0;
    for _ in 0..8 {
        let x = cycle(&[], &mut rng);
        n += search::candidates(&Spectrogram::new(&x)).len();
    }
    println!("noise only: {} candidates/cycle", n / 8);
}

/// Sync scores: the true candidates' at each SNR against the best that noise
/// alone raises.
#[test]
#[ignore]
fn measure_the_sync_floor() {
    let mut rng = Rng::new(23);
    for snr in [-20.0, -22.0, -23.0, -24.0] {
        let mut scores = Vec::new();
        for _ in 0..6 {
            let sent = spread(10, snr, &mut rng);
            let x = cycle(&sent, &mut rng);
            let cands = search::candidates(&Spectrogram::new(&x));
            for s in &sent {
                if let Some(c) = cands
                    .iter()
                    .find(|c| (c.f0 - s.f0).abs() <= 3.2 && (c.start - s.start()).abs() <= 0.1)
                {
                    scores.push(c.sync);
                }
            }
        }
        scores.sort_by(f32::total_cmp);
        println!(
            "{snr} dB: min {:.2} p10 {:.2} p50 {:.2}",
            scores[0],
            scores[scores.len() / 10],
            scores[scores.len() / 2]
        );
    }
    let mut noise = Vec::new();
    for _ in 0..6 {
        let x = cycle(&[], &mut rng);
        noise.extend(
            search::candidates(&Spectrogram::new(&x))
                .iter()
                .map(|c| c.sync),
        );
    }
    noise.sort_by(f32::total_cmp);
    let q = |f: f64| noise[((noise.len() - 1) as f64 * f) as usize];
    println!(
        "noise: n {} p50 {:.2} p90 {:.2} p99 {:.2} max {:.2}",
        noise.len(),
        q(0.5),
        q(0.9),
        q(0.99),
        noise[noise.len() - 1]
    );
}

/// The fine Costas power, the same way: does it part the weak signals from
/// noise better than the coarse score?
#[test]
#[ignore]
fn measure_the_fine_power_floor() {
    let mut rng = Rng::new(29);
    for snr in [-20.0, -22.0, -23.0, -24.0] {
        let mut p = Vec::new();
        for _ in 0..4 {
            let sent = spread(10, snr, &mut rng);
            let x = cycle(&sent, &mut rng);
            let cands = search::candidates(&Spectrogram::new(&x));
            let bb = Baseband::new(&x);
            for s in &sent {
                if let Some(c) = cands
                    .iter()
                    .find(|c| (c.f0 - s.f0).abs() <= 3.2 && (c.start - s.start()).abs() <= 0.1)
                {
                    p.push(search::refine(&bb, c).power);
                }
            }
        }
        p.sort_by(f32::total_cmp);
        println!(
            "{snr} dB: min {:.2} p10 {:.2} p50 {:.2}",
            p[0],
            p[p.len() / 10],
            p[p.len() / 2]
        );
    }
    let mut noise = Vec::new();
    for _ in 0..4 {
        let x = cycle(&[], &mut rng);
        let bb = Baseband::new(&x);
        noise.extend(
            search::candidates(&Spectrogram::new(&x))
                .iter()
                .map(|c| search::refine(&bb, c).power),
        );
    }
    noise.sort_by(f32::total_cmp);
    let q = |f: f64| noise[((noise.len() - 1) as f64 * f) as usize];
    println!(
        "noise: n {} p50 {:.2} p90 {:.2} p99 {:.2} max {:.2}",
        noise.len(),
        q(0.5),
        q(0.9),
        q(0.99),
        noise[noise.len() - 1]
    );
}

#[test]
#[ignore]
fn time_the_search_stages() {
    use std::time::Instant;
    let mut rng = Rng::new(31);
    let sent = spread(10, -15.0, &mut rng);
    let x = cycle(&sent, &mut rng);
    let t = Instant::now();
    let spec = Spectrogram::new(&x);
    println!("spectrogram {:?}", t.elapsed());
    let t = Instant::now();
    let cands = search::candidates(&spec);
    println!("candidates {:?} ({})", t.elapsed(), cands.len());
    let t = Instant::now();
    let bb = Baseband::new(&x);
    println!("baseband {:?}", t.elapsed());
    let t = Instant::now();
    let (_y, _) = bb.around(1000.0);
    println!("one slice {:?}", t.elapsed());
    let t = Instant::now();
    for c in cands.iter().take(10) {
        let _ = search::refine(&bb, c);
    }
    println!("ten refines {:?}", t.elapsed());
}

/// What a cycle's decodes say about what was sent.
#[derive(Default, Debug)]
pub struct DecodeRows {
    pub sent: usize,
    pub right: usize,
    pub wrong: usize,
    pub duplicates: usize,
    /// Estimate minus truth, for the right ones: SNR dB, DT s, f0 Hz, and
    /// the f0 error over its stated sigma.
    pub snr_err: Vec<f64>,
    pub dt_err: Vec<f64>,
    pub f0_err: Vec<f64>,
    pub f0_z: Vec<f64>,
    pub drift_err: Vec<f64>,
    pub seconds: Vec<f64>,
}

impl DecodeRows {
    pub fn add(&mut self, sent: &[Sent], x: &[Complex<f32>], settings: &cycle::Settings) {
        let mut book = HashBook::new();
        let t = std::time::Instant::now();
        let report = cycle::decode_with(x, &mut book, settings);
        self.seconds.push(t.elapsed().as_secs_f64());
        self.sent += sent.len();
        let mut hit = std::collections::HashSet::new();
        for d in &report.decodes {
            match sent.iter().position(|s| s.payload == d.payload) {
                Some(i) => {
                    if !hit.insert(i) {
                        self.duplicates += 1;
                        continue;
                    }
                    let s = &sent[i];
                    self.right += 1;
                    self.snr_err.push(d.physics.snr_db - s.snr_db);
                    self.dt_err.push(d.dt - s.dt);
                    self.f0_err.push(d.physics.f0 - s.f0);
                    self.f0_z.push((d.physics.f0 - s.f0) / d.physics.f0_sigma);
                    self.drift_err.push(d.physics.drift - s.drift);
                    // `FT8_SHOW_OFF=<Hz>`: every decode whose f0 is off by
                    // more, and how it was decoded.
                    let off: f64 =
                        std::env::var("FT8_SHOW_OFF").map_or(f64::INFINITY, |v| v.parse().unwrap());
                    if (d.physics.f0 - s.f0).abs() > off {
                        println!(
                            "off: f0 {:+.3} Hz dt {:+.1} ms snr {:.1} (sent {:.1}) costas {} block {} osd {:?} repaired {} iterations {} pass {} sigma {:.3}",
                            d.physics.f0 - s.f0,
                            (d.dt - s.dt) * 1e3,
                            d.physics.snr_db,
                            s.snr_db,
                            d.costas,
                            d.block,
                            d.osd,
                            d.repaired,
                            d.iterations,
                            d.pass,
                            d.physics.f0_sigma
                        );
                    }
                }
                None => {
                    self.wrong += 1;
                    let near = sent
                        .iter()
                        .min_by(|a, b| {
                            (a.f0 - d.physics.f0)
                                .abs()
                                .total_cmp(&(b.f0 - d.physics.f0).abs())
                        })
                        .map_or(f64::NAN, |s| s.f0 - d.physics.f0);
                    println!(
                        "wrong: {} f0 {:.1} (nearest sent {:+.1} Hz) snr {:.1} costas {} osd {:?} repaired {} distance {:.4} contradiction {:.4} pass {}",
                        d.message.text(),
                        d.physics.f0,
                        near,
                        d.physics.snr_db,
                        d.costas,
                        d.osd,
                        d.repaired,
                        d.distance,
                        d.trace.contradiction(),
                        d.pass
                    );
                }
            }
        }
    }
}

fn mean(v: &[f64]) -> f64 {
    v.iter().sum::<f64>() / v.len().max(1) as f64
}

fn rms_of(v: &[f64]) -> f64 {
    rms(v.iter().copied())
}

/// A cycle of `n` messages that unpack (standard CQs with a locator), at one
/// SNR, spread as [`spread`].
pub fn readable(n: usize, snr_db: f64, rng: &mut Rng) -> Vec<Sent> {
    readable_apart(n, snr_db, 60.0, rng)
}

/// [`readable`] with the frequencies at least `gap` Hz apart.
pub fn readable_apart(n: usize, snr_db: f64, gap: f64, rng: &mut Rng) -> Vec<Sent> {
    let mut out = spread_apart(n, snr_db, gap, rng);
    for (k, s) in out.iter_mut().enumerate() {
        let call = format!(
            "K{}A{}{}",
            k % 10,
            (b'A' + (rng.next_u64() % 26) as u8) as char,
            (b'A' + (k / 10) as u8) as char
        );
        let grid = format!(
            "{}{}{}{}",
            (b'A' + (rng.next_u64() % 18) as u8) as char,
            (b'A' + (rng.next_u64() % 18) as u8) as char,
            rng.next_u64() % 10,
            rng.next_u64() % 10
        );
        s.payload = message::pack::pack(&Message::Standard {
            to: Call::Cq(None),
            from: Call::Std { call, suffix: None },
            ack: false,
            word: G15::Grid(grid),
        });
    }
    out
}

#[test]
fn a_clean_cycle_decodes_every_message_once_with_its_physics() {
    let mut rng = Rng::new(41);
    let sent = readable(5, -12.0, &mut rng);
    let x = cycle(&sent, &mut rng);
    let mut rows = DecodeRows::default();
    // Five strong signals are among the strongest candidates; the cap only
    // keeps the unoptimised test build quick.
    let settings = cycle::Settings {
        max_candidates: 40,
        ..cycle::Settings::DEFAULT
    };
    rows.add(&sent, &x, &settings);
    assert_eq!((rows.right, rows.wrong, rows.duplicates), (5, 0, 0));
    for e in &rows.snr_err {
        assert!(e.abs() < 1.0, "SNR off by {e} dB");
    }
    for e in &rows.dt_err {
        assert!(e.abs() < 0.008, "DT off by {e} s");
    }
    for e in &rows.f0_err {
        assert!(e.abs() < 0.02, "tone 0 off by {e} Hz");
    }
    for z in &rows.f0_z {
        assert!(z.abs() < 3.0, "tone 0 off by {z} of its stated sigma");
    }
}

/// Every candidate lands in exactly one place, the stages only narrow, and
/// what the funnel calls new decodes is what the cycle reported.
#[test]
fn the_funnel_accounts_for_every_candidate() {
    let mut rng = Rng::new(43);
    let sent = readable(8, -12.0, &mut rng);
    let x = cycle(&sent, &mut rng);
    let settings = cycle::Settings {
        max_candidates: 60,
        ..cycle::Settings::DEFAULT
    };
    let report = cycle::decode_with(&x, &mut HashBook::new(), &settings);
    assert!(report.decodes.len() >= 7, "{} of 8", report.decodes.len());
    assert_eq!(report.funnel.len(), report.passes);
    let mut new = 0;
    for f in &report.funnel {
        assert!(f.candidates >= f.synced && f.synced >= f.coded, "{f:?}");
        assert_eq!(
            f.coded,
            f.by_bp + f.by_osd + f.duplicates + f.refused_zero + f.refused_osd,
            "{f:?}"
        );
        new += f.by_bp + f.by_osd;
    }
    assert_eq!(new, report.decodes.len() + report.malformed);
    assert_eq!(
        report.funnel.iter().map(|f| f.candidates).sum::<usize>(),
        report.candidates
    );
}

/// The floor under a band with signals in it, by the quantile it is read
/// at: 0 to 40 transmissions at -5 to +20 dB on unit noise, whose true floor
/// is 1 / RATE per hertz. Rows: quantile, then the error in dB at each
/// crowding, mean of 6 cycles; then how many of the 6 `floor::read` gives,
/// and its worst error among them.
#[test]
#[ignore]
fn measure_the_floor_under_signals() {
    use super::floor;
    let want = 10.0 * (1.0 / RATE).log10();
    let quantiles = [0.5, 0.3, 0.2, 0.1];
    let crowds = [0usize, 4, 12, 24, 40];
    let mut err = vec![vec![0.0f64; crowds.len()]; quantiles.len()];
    let mut given = vec![(0usize, 0.0f64); crowds.len()];
    for (ci, &n) in crowds.iter().enumerate() {
        for seed in 0..6 {
            let mut rng = Rng::new(900 + seed);
            let mut sent = spread_apart(n, 0.0, 40.0, &mut rng);
            for s in sent.iter_mut() {
                s.snr_db = -5.0 + rng.unit() * 25.0;
            }
            let x = cycle(&sent, &mut rng);
            for (qi, &q) in quantiles.iter().enumerate() {
                err[qi][ci] += (floor::read_at(&x, q).unwrap() - want) / 6.0;
            }
            if let Some(f) = floor::read(&x) {
                let e = f64::from(f) - want;
                given[ci].0 += 1;
                if e.abs() > given[ci].1.abs() {
                    given[ci].1 = e;
                }
            }
        }
    }
    println!("q      none    4      12     24     40");
    for (qi, q) in quantiles.iter().enumerate() {
        let row: Vec<String> = err[qi].iter().map(|e| format!("{e:+.2}")).collect();
        println!("{q:<5}  {}", row.join("  "));
    }
    let row: Vec<String> = given
        .iter()
        .map(|(n, e)| format!("{n}/6 {e:+.2}"))
        .collect();
    println!("read   {}", row.join("  "));
}

/// The floor reads the noise the radio delivered, on its word's own scale,
/// through the channel at the HF+'s rates: white noise of a known power over
/// the whole capture is that power over the rate, per hertz.
#[test]
fn the_floor_reads_the_noise_the_radio_delivered() {
    for rate in [192_000.0, 768_000.0, 912_000.0] {
        let mut rng = Rng::new(47);
        let power = 1e-6;
        let x = rng.noise((rate * 3.0) as usize, power);
        let mut ch = super::channel::Channel::new(rate).expect("a route");
        let mut out = Vec::new();
        for block in x.chunks(16_384) {
            ch.push(block, &mut out);
        }
        // Past the filters' start and the DC mean's first estimate.
        let settled = &out[RATE as usize / 2..];
        let floor = super::floor::read(settled).expect("a floor");
        let want = 10.0 * (power / rate).log10();
        assert!(
            (f64::from(floor) - want).abs() < 0.5,
            "{rate}: read {floor} dB/Hz, sent {want}"
        );
    }
}

/// On a fading path the carrier's phase wanders through the transmission;
/// the SNR must still be the power that arrived, not what a coherent sum of
/// the 79 symbols keeps of it.
#[test]
fn snr_holds_on_a_fading_path() {
    let mut rng = Rng::new(83);
    let settings = cycle::Settings {
        max_candidates: 40,
        ..cycle::Settings::DEFAULT
    };
    let sent = readable(6, -12.0, &mut rng);
    let (x, arrived) = faded(&sent, 2.0, &mut rng);
    let report = cycle::decode_with(&x, &mut HashBook::new(), &settings);
    let err: Vec<f64> = report
        .decodes
        .iter()
        .filter_map(|d| {
            let i = sent.iter().position(|s| s.payload == d.payload)?;
            Some(d.physics.snr_db - arrived[i])
        })
        .collect();
    assert!(err.len() >= 5, "only {} of 6 decoded", err.len());
    for d in &report.decodes {
        assert!(
            d.physics.coherence < 0.7,
            "a path fading at 2 Hz read as steady: coherence {}",
            d.physics.coherence
        );
    }
    for e in &err {
        assert!(e.abs() < 1.5, "SNR off by {e} dB on a fading path");
    }
    assert!(mean(&err).abs() < 0.7, "SNR biased by {} dB", mean(&err));
}

/// A strong signal fading at 2 Hz is taken out closely enough that a weak
/// one 20 Hz above it still decodes: the subtraction follows the gain at the
/// rate the path changes it.
#[test]
fn a_weak_signal_comes_out_from_beneath_a_fading_one() {
    let mut rng = Rng::new(101);
    let settings = cycle::Settings {
        max_candidates: 60,
        ..cycle::Settings::DEFAULT
    };
    let weak = readable_apart(4, -14.0, 200.0, &mut rng);
    let mut strong = readable(4, -2.0, &mut rng);
    for (s, w) in strong.iter_mut().zip(&weak) {
        s.f0 = w.f0 - 20.0;
        s.dt = w.dt + 0.2;
    }
    let both: Vec<Sent> = weak.iter().chain(&strong).copied().collect();
    let (x, _) = faded(&both, 2.0, &mut rng);
    let report = cycle::decode_with(&x, &mut HashBook::new(), &settings);
    let read = |sent: &[Sent]| {
        sent.iter()
            .filter(|s| report.decodes.iter().any(|d| d.payload == s.payload))
            .count()
    };
    assert_eq!(read(&strong), 4);
    assert!(
        read(&weak) >= 3,
        "{} of 4 weak ones from beneath",
        read(&weak)
    );
}

/// SNR error against the SNR that arrived, by Doppler spread.
#[test]
#[ignore]
fn measure_the_snr_on_fading_paths() {
    let mut rng = Rng::new(89);
    for snr in [-14.0, -18.0, -20.0] {
        for spread in [0.0, 0.5, 1.0, 2.0] {
            let (mut err, mut coh, mut sent_n) = (Vec::new(), Vec::new(), 0);
            for _ in 0..4 {
                let sent = readable(12, snr, &mut rng);
                sent_n += sent.len();
                let (x, arrived) = faded(&sent, spread, &mut rng);
                let report = cycle::decode(&x, &mut HashBook::new());
                for d in &report.decodes {
                    if let Some(i) = sent.iter().position(|s| s.payload == d.payload) {
                        err.push(d.physics.snr_db - arrived[i]);
                        coh.push(d.physics.coherence);
                    }
                }
            }
            println!(
                "{snr:>5} dB, spread {spread:.1} Hz: {:>2}/{sent_n} decoded, SNR bias {:+.2} rms {:.2} dB",
                err.len(),
                mean(&err),
                rms_of(&err)
            );
            coh.sort_by(f64::total_cmp);
            if !coh.is_empty() {
                println!(
                    "    coherence min {:.2} median {:.2} max {:.2}",
                    coh[0],
                    coh[coh.len() / 2],
                    coh[coh.len() - 1]
                );
            }
        }
    }
}

/// A weak signal 20 Hz above a strong one, both fading: read alone, and from
/// beneath the strong one after its subtraction.
#[test]
#[ignore]
fn measure_beneath_a_fading_signal() {
    let mut rng = Rng::new(97);
    for spread in [0.0, 1.0, 2.0] {
        let (mut alone, mut beneath, mut strong_read, mut n) = (0, 0, 0, 0);
        for _ in 0..8 {
            let weak = readable_apart(6, -16.0, 150.0, &mut rng);
            let mut strong = readable(6, -4.0, &mut rng);
            for (s, w) in strong.iter_mut().zip(&weak) {
                s.f0 = w.f0 - 20.0;
                s.dt = w.dt + 0.2;
            }
            n += weak.len();
            let count = |sent: &[Sent], decodes: &[cycle::Decode]| {
                sent.iter()
                    .filter(|s| decodes.iter().any(|d| d.payload == s.payload))
                    .count()
            };
            let (x, _) = faded(&weak, spread, &mut rng);
            alone += count(&weak, &cycle::decode(&x, &mut HashBook::new()).decodes);
            let both: Vec<Sent> = weak.iter().chain(&strong).copied().collect();
            let (x, _) = faded(&both, spread, &mut rng);
            let r = cycle::decode(&x, &mut HashBook::new());
            beneath += count(&weak, &r.decodes);
            strong_read += count(&strong, &r.decodes);
        }
        println!(
            "spread {spread:.1} Hz: weak alone {alone}/{n}, beneath a strong one {beneath}/{n} (strong {strong_read}/{n})"
        );
    }
}

/// Decode rows by SNR; the 50 % point is compared with the paper's Table 5.
#[test]
#[ignore]
fn measure_the_decoder() {
    let mut rng = Rng::new(43);
    println!(
        "SNR   decoded  wrong dup  SNR bias/rms     DT rms    f0 rms    f0/sigma rms  s/cycle"
    );
    for snr in [-14.0, -17.0, -19.0, -20.0, -21.0, -22.0, -23.0] {
        let mut rows = DecodeRows::default();
        for _ in 0..6 {
            let sent = readable(12, snr, &mut rng);
            let x = cycle(&sent, &mut rng);
            rows.add(&sent, &x, &settings_from_env());
        }
        println!(
            "{snr:>5} {:>3}/{:<3} {:>4} {:>3}  {:>+5.2} / {:>4.2} dB  {:>5.1} ms  {:>6.3} Hz  {:>6.2}        {:>4.2}",
            rows.right, rows.sent, rows.wrong, rows.duplicates,
            mean(&rows.snr_err), rms_of(&rows.snr_err),
            rms_of(&rows.dt_err) * 1000.0, rms_of(&rows.f0_err), rms_of(&rows.f0_z),
            mean(&rows.seconds),
        );
    }
}

/// The soft-decision scale, swept at the threshold.
#[test]
#[ignore]
fn measure_the_llr_scale() {
    for scale in [1.5f32, 2.0, 2.8, 3.5, 4.5] {
        let mut rng = Rng::new(47);
        let mut rows = DecodeRows::default();
        let settings = cycle::Settings {
            llr_scale: scale,
            ..cycle::Settings::DEFAULT
        };
        for snr in [-20.0, -21.0, -22.0] {
            for _ in 0..3 {
                let sent = readable(12, snr, &mut rng);
                let x = cycle(&sent, &mut rng);
                rows.add(&sent, &x, &settings);
            }
        }
        println!(
            "scale {scale}: {}/{} wrong {}",
            rows.right, rows.sent, rows.wrong
        );
    }
}

/// The decoder's settings with any of these from the environment, to try
/// other numbers on the rows and on a recording: `FT8_CANDS` (the most
/// candidates a pass reads), `FT8_PASSES`, `FT8_MIN_POWER`, `FT8_GATE` (the
/// Costas count a candidate needs to be tried again), `FT8_RESYNC=0`,
/// `FT8_OSD_BP` (the posteriors OSD is also run on), `FT8_GUARD=0` (every
/// codeword reported, believed or not), and the OSD guard's numbers
/// (`cycle::OsdGuard`): `FT8_OSD_COSTAS`, `FT8_OSD_COSTAS_SLOPE`,
/// `FT8_OSD_REPAIRED`, `FT8_OSD_SNR`, `FT8_CONTRA`, `FT8_CONTRA_SLOPE`,
/// `FT8_CONTRA_0` (a negative one puts order 0 on the others' lines),
/// `FT8_COSTAS_0`, and
/// `FT8_SHADOW_DB` (how much stronger a transmission of its own pass an
/// OSD codeword waits for; `inf`: none), and `FT8_UNREAD` (the Costas count
/// from which a candidate is read again with a stretch unread; 22: never).
pub(crate) fn settings_from_env() -> cycle::Settings {
    fn var<T: std::str::FromStr>(name: &str, default: T) -> T {
        std::env::var(name).map_or(default, |v| {
            v.parse()
                .unwrap_or_else(|_| panic!("{name}: not a number: {v}"))
        })
    }
    let d = cycle::Settings::DEFAULT;
    cycle::Settings {
        max_candidates: var("FT8_CANDS", d.max_candidates),
        max_passes: var("FT8_PASSES", d.max_passes),
        min_power: var("FT8_MIN_POWER", d.min_power),
        resync_min_costas: var("FT8_GATE", d.resync_min_costas),
        osd_from_bp: var("FT8_OSD_BP", d.osd_from_bp),
        resync: std::env::var("FT8_RESYNC").map_or(d.resync, |v| v != "0"),
        guard: std::env::var("FT8_GUARD").map_or(d.guard, |v| v != "0"),
        shadow_db: var("FT8_SHADOW_DB", d.shadow_db),
        unread_min_costas: var("FT8_UNREAD", d.unread_min_costas),
        osd_guard: cycle::OsdGuard {
            min_costas: var("FT8_OSD_COSTAS", d.osd_guard.min_costas),
            costas_per_db: var("FT8_OSD_COSTAS_SLOPE", d.osd_guard.costas_per_db),
            max_repaired: var("FT8_OSD_REPAIRED", d.osd_guard.max_repaired),
            min_snr_db: var("FT8_OSD_SNR", d.osd_guard.min_snr_db),
            max_contradiction: var("FT8_CONTRA", d.osd_guard.max_contradiction),
            contradiction_per_db: var("FT8_CONTRA_SLOPE", d.osd_guard.contradiction_per_db),
            order_0_contradiction: var("FT8_CONTRA_0", d.osd_guard.order_0_contradiction),
            order_0_costas: var("FT8_COSTAS_0", d.osd_guard.order_0_costas),
        },
        ..d
    }
}

/// Noise alone, every candidate decoded: what comes out is false. 40
/// cycles, or `FT8_NOISE_CYCLES`; the settings as [`settings_from_env`].
#[test]
#[ignore]
fn measure_false_decodes() {
    let mut rng = Rng::new(53);
    let (mut false_decodes, mut malformed, mut cycles) = (0, 0, 0);
    let settings = settings_from_env();
    let n: usize = std::env::var("FT8_NOISE_CYCLES").map_or(40, |v| v.parse().unwrap());
    for _ in 0..n {
        let x = cycle(&[], &mut rng);
        let r = cycle::decode_with(&x, &mut HashBook::new(), &settings);
        false_decodes += r.decodes.len();
        malformed += r.malformed;
        cycles += 1;
        for d in &r.decodes {
            println!(
                "false: {} osd {:?} block {} costas {} repaired {} distance {:.4} snr {:.2} coherence {:.3} drift {:.3} f0_sigma {:.4} pass {} contradiction {:.4}",
                d.message.text(),
                d.osd,
                d.block,
                d.costas,
                d.repaired,
                d.distance,
                d.physics.snr_db,
                d.physics.coherence,
                d.physics.drift,
                d.physics.f0_sigma,
                d.pass,
                d.trace.contradiction()
            );
        }
    }
    println!("{cycles} noise cycles: {false_decodes} false decodes, {malformed} malformed");
}

/// A crowded band: 40 transmissions from -6 to -20 dB, five pairs sharing a
/// frequency within a tone, the rest anywhere.
#[test]
#[ignore]
fn measure_a_crowded_band() {
    let mut rng = Rng::new(59);
    let mut rows = DecodeRows::default();
    for _ in 0..4 {
        let mut sent = readable_apart(40, -10.0, 0.0, &mut rng);
        for s in sent.iter_mut() {
            s.snr_db = -6.0 - rng.unit() * 14.0;
            s.f0 = 200.0 + rng.unit() * 2700.0;
        }
        for k in 0..5 {
            sent[2 * k + 1].f0 = sent[2 * k].f0 + rng.unit() * 6.0;
            sent[2 * k + 1].snr_db = sent[2 * k].snr_db - 6.0;
        }
        let x = cycle(&sent, &mut rng);
        rows.add(&sent, &x, &settings_from_env());
    }
    println!(
        "crowded: {}/{} decoded, {} wrong, {} duplicates, {:.2} s/cycle",
        rows.right,
        rows.sent,
        rows.wrong,
        rows.duplicates,
        mean(&rows.seconds)
    );
}

/// What tells a true OSD decode from a false one: for every decode OSD made,
/// right or wrong, its Costas count, bits repaired, distance and block.
#[test]
#[ignore]
fn measure_what_parts_true_from_false() {
    let mut rng = Rng::new(61);
    let mut true_ones: Vec<(u8, u32, f32, u8)> = Vec::new();
    let mut false_ones: Vec<(u8, u32, f32, u8, f64)> = Vec::new();
    let mut take = |sent: &[Sent], x: &[Complex<f32>]| {
        let r = cycle::decode(x, &mut HashBook::new());
        for d in &r.decodes {
            let truth = sent.iter().any(|s| s.payload == d.payload);
            let row = (d.costas, d.repaired, d.distance, d.block);
            if truth {
                if d.osd.is_some() {
                    true_ones.push(row);
                }
            } else {
                false_ones.push((row.0, row.1, row.2, row.3, d.dt));
            }
        }
    };
    for snr in [-20.0, -21.0, -22.0, -23.0, -24.0] {
        for _ in 0..3 {
            let sent = readable(12, snr, &mut rng);
            let x = cycle(&sent, &mut rng);
            take(&sent, &x);
        }
    }
    for _ in 0..6 {
        let sent = readable(10, -8.0, &mut rng);
        let x = cycle(&sent, &mut rng);
        take(&sent, &x);
    }
    for _ in 0..6 {
        let x = cycle(&[], &mut rng);
        take(&[], &x);
    }
    true_ones.sort_by(|a, b| a.2.total_cmp(&b.2));
    println!("true OSD decodes (costas, repaired, distance, block):");
    for t in &true_ones {
        println!("  {t:?}");
    }
    println!("false decodes (costas, repaired, distance, block, dt):");
    for f in &false_ones {
        println!("  {f:?}");
    }
}

/// Where the SNR estimate's error comes from: the amplitude or the noise.
#[test]
#[ignore]
fn measure_the_snr_parts() {
    let mut rng = Rng::new(67);
    let sigma_y2 = DOWN_RATE / RATE; // unit noise at RATE, as the slice sees it
    for snr in [-10.0, -14.0, -18.0] {
        let (mut amp, mut noise) = (Vec::new(), Vec::new());
        for _ in 0..3 {
            let sent = readable(12, snr, &mut rng);
            let x = cycle(&sent, &mut rng);
            let r = cycle::decode(&x, &mut HashBook::new());
            for d in &r.decodes {
                let true_amp = (2500.0 / RATE * 10f64.powf(snr / 10.0)).sqrt();
                amp.push(20.0 * (d.physics.amplitude / true_amp).log10());
                let a2 = d.physics.amplitude * d.physics.amplitude;
                let est_sigma2 = a2 / (2500.0 / DOWN_RATE) / 10f64.powf(d.physics.snr_db / 10.0);
                noise.push(10.0 * (est_sigma2 / sigma_y2).log10());
            }
        }
        println!(
            "{snr} dB: amplitude {:+.2} dB (rms {:.2}), noise {:+.2} dB (rms {:.2}), n {}",
            mean(&amp),
            rms_of(&amp),
            mean(&noise),
            rms_of(&noise),
            amp.len()
        );
    }
}

/// Decodes and time with the fine-power gate at several floors.
#[test]
#[ignore]
fn measure_the_power_gate() {
    for min_power in [0.0f32, 1.8, 2.0, 2.2, 2.4] {
        let mut rng = Rng::new(71);
        let mut rows = DecodeRows::default();
        let settings = cycle::Settings {
            min_power,
            ..cycle::Settings::DEFAULT
        };
        for snr in [-19.0, -20.0, -21.0] {
            for _ in 0..3 {
                let sent = readable(12, snr, &mut rng);
                let x = cycle(&sent, &mut rng);
                rows.add(&sent, &x, &settings);
            }
        }
        println!(
            "gate {min_power}: {}/{} wrong {} {:.2} s/cycle",
            rows.right,
            rows.sent,
            rows.wrong,
            mean(&rows.seconds)
        );
    }
}

/// Unoptimised test builds are slow; these settings keep the plain tests
/// quick by examining only the strongest candidates.
const QUICK: cycle::Settings = cycle::Settings {
    max_candidates: 40,
    ..cycle::Settings::DEFAULT
};

/// Reading a pass's candidates side by side changes nothing: a crowded
/// cycle on one thread and on four gives the same report, decode for
/// decode, funnel for funnel. Which of two candidates of one transmission
/// is kept is decided in the search's order, as one after another would.
#[test]
fn the_decodes_do_not_depend_on_the_threads() {
    let mut rng = Rng::new(61);
    let mut sent = readable_apart(16, -10.0, 0.0, &mut rng);
    for s in sent.iter_mut() {
        s.snr_db = -4.0 - rng.unit() * 14.0;
    }
    // The same message twice, far apart: two candidates that decode one
    // payload, the second a duplicate only by the order they are taken in.
    // And a weak one within a tone of another.
    sent[5].payload = sent[4].payload;
    sent[4].snr_db = -6.0;
    sent[5].snr_db = -6.0;
    sent[1].f0 = sent[0].f0 + 2.0;
    let x = cycle(&sent, &mut rng);
    let at = |threads| {
        let settings = cycle::Settings { threads, ..QUICK };
        cycle::decode_with(&x, &mut HashBook::new(), &settings)
    };
    let one = at(1);
    assert!(one.decodes.len() >= 8, "{}", one.decodes.len());
    assert!(
        one.funnel.iter().any(|f| f.duplicates > 0),
        "{:?}",
        one.funnel
    );
    assert_eq!(at(4), one);
}

#[test]
fn a_weak_signal_under_a_strong_one_comes_out_after_subtraction() {
    let mut rng = Rng::new(73);
    let mut sent = readable(2, -8.0, &mut rng);
    sent[1].f0 = sent[0].f0 + 3.0;
    sent[1].dt = sent[0].dt + 0.3;
    sent[1].snr_db = -16.0;
    let x = cycle(&sent, &mut rng);
    let r = cycle::decode_with(&x, &mut HashBook::new(), &QUICK);
    let pass_of = |p: u128| r.decodes.iter().find(|d| d.payload == p).map(|d| d.pass);
    assert_eq!(pass_of(sent[0].payload), Some(1));
    assert_eq!(pass_of(sent[1].payload), Some(2), "{:?}", r.decodes);
    assert_eq!(r.decodes.len(), 2);
}

/// A cycle with one transmission whose symbols `from..to` (of the 79) are
/// another message's: `other`'s there, `sent`'s everywhere else.
#[cfg(test)]
fn spliced(sent: &Sent, other: u128, from: usize, to: usize, rng: &mut Rng) -> Vec<Complex<f32>> {
    let mut tones = sent.tones();
    let others = modulate::channel_symbols(&ldpc::encode(crc::with_crc(other)));
    tones[from..to].copy_from_slice(&others[from..to]);
    let mut x = rng.noise(CYCLE_SAMPLES, 1.0);
    let w = Waveform {
        f0: sent.f0,
        start: sent.start(),
        amplitude: (2500.0 / RATE * 10f64.powf(sent.snr_db / 10.0)).sqrt(),
        drift: 0.0,
        phase: 0.7,
    };
    modulate::add(&tones, &w, RATE, &mut x);
    x
}

/// **A message changed after its start.** An operator answering a caller a
/// second or two into a CQ sends the CQ's first symbols and the answer's
/// rest: off the air, a -9 dB transmission with every Costas symbol right
/// whose first ten data symbols were the CQ of the cycle before, 10 dB over
/// its own tones. Plain and wrong, they keep the code from the message;
/// taken as unknown, it reads. The same for a transmission whose end another
/// one stood over.
#[test]
fn a_transmission_spliced_from_two_messages_is_read_with_the_splice_unread() {
    use super::demod::Unread;
    let call = |c: &str| Call::Std {
        call: c.into(),
        suffix: None,
    };
    let message = |to: Call| {
        message::pack::pack(&Message::Standard {
            to,
            from: call("HA5ZZT"),
            ack: false,
            word: G15::Grid("JN97".into()),
        })
    };
    let (answer, cq) = (message(call("K1ABC")), message(Call::Cq(None)));
    let sent = Sent {
        payload: answer,
        f0: 1200.0,
        dt: 0.2,
        snr_db: -9.0,
        drift: 0.0,
    };
    let never = cycle::Settings {
        unread_min_costas: 22,
        ..QUICK
    };
    // The CQ's first ten data symbols (7 to 16 of the 79), then the CQ's
    // last twelve (60 to 71), which are its own parity.
    for (from, to, unread) in [(7, 17, Unread::Start), (60, 72, Unread::End)] {
        let x = spliced(&sent, cq, from, to, &mut Rng::new(97));
        let r = cycle::decode_with(&x, &mut HashBook::new(), &QUICK);
        assert_eq!(r.decodes.len(), 1, "{unread:?}: {:?}", r.decodes);
        assert_eq!(r.decodes[0].message.text(), "K1ABC HA5ZZT JN97");
        assert_eq!(r.decodes[0].unread, Some(unread));
        assert_eq!(r.decodes[0].costas, 21);
        let r = cycle::decode_with(&x, &mut HashBook::new(), &never);
        assert!(r.decodes.is_empty(), "{unread:?}: {:?}", r.decodes);
    }
    // A whole transmission is read as it always was.
    let x = cycle(&[sent], &mut Rng::new(97));
    let r = cycle::decode_with(&x, &mut HashBook::new(), &QUICK);
    assert_eq!(r.decodes.len(), 1);
    assert_eq!(r.decodes[0].unread, None);
}

#[test]
fn drift_is_measured() {
    let mut rng = Rng::new(79);
    let mut sent = readable(1, -12.0, &mut rng);
    sent[0].drift = 2.0;
    let x = cycle(&sent, &mut rng);
    let r = cycle::decode_with(&x, &mut HashBook::new(), &QUICK);
    assert_eq!(r.decodes.len(), 1);
    let d = &r.decodes[0];
    assert!((d.physics.drift - 2.0).abs() <= 0.25, "{}", d.physics.drift);
    assert!(
        (d.physics.f0 - sent[0].f0).abs() < 0.05,
        "{}",
        d.physics.f0 - sent[0].f0
    );
}

/// A decode read at a place a dozen milliseconds and most of a hertz off
/// (OSD can decode there) is still measured where the signal is. Fitted
/// coherently at that place alone, f0 came out 0.8 Hz off with a spread of
/// 0.01 Hz.
#[test]
fn a_decode_read_off_its_place_is_measured_where_it_is() {
    use super::demod;
    use super::search::{Baseband, DOWN_RATE};
    let mut rng = Rng::new(97);
    let sent = readable(1, -8.0, &mut rng);
    let x = cycle(&sent, &mut rng);
    let bb = Baseband::new(&x);
    let s = &sent[0];
    for (dt, df) in [(0.0, 0.0), (0.013, -0.8), (-0.013, 0.8), (0.02, 0.5)] {
        let (y, at_zero) = bb.around(s.f0 + df);
        let start = s.start() + dt;
        let hz = s.f0 + df - at_zero;
        let z = demod::correlate(&y, (start * DOWN_RATE).round() as i64, hz);
        let m = demod::measured_from(&y, start, hz, &s.tones(), &z).physics;
        let f0 = m.f0 + at_zero;
        assert!(
            (f0 - s.f0).abs() < 0.05,
            "read {dt} s, {df} Hz off: f0 {:+.3} Hz",
            f0 - s.f0
        );
        assert!(
            (m.start - s.start()).abs() < 0.003,
            "read {dt} s, {df} Hz off: start {:+.1} ms",
            (m.start - s.start()) * 1e3
        );
    }
}

#[test]
fn noise_alone_reads_as_nothing() {
    let mut rng = Rng::new(83);
    let x = cycle(&[], &mut rng);
    let r = cycle::decode_with(&x, &mut HashBook::new(), &QUICK);
    assert!(r.decodes.is_empty(), "{:?}", r.decodes);
}

/// **A steady carrier is not a message.** On 20 m by day the same few tones
/// (646, 525, 2400 Hz) "decoded" cycle after cycle as an empty message: a
/// carrier puts every data symbol on tone 0, tone 0 is the bits 000, and the
/// all-zero codeword passes both the code and the check (the CRC of zero is
/// zero). It reads as thirteen blanks of free text, which nobody sends.
#[test]
fn a_steady_carrier_decodes_to_nothing() {
    let mut rng = Rng::new(89);
    let mut x = cycle(&[], &mut rng);
    for hz in [646.0, 1525.0, 2400.0] {
        for (k, v) in x.iter_mut().enumerate() {
            let ph = std::f64::consts::TAU * hz * k as f64 / 12_000.0;
            *v += Complex::new(ph.cos() as f32, ph.sin() as f32) * 0.05;
        }
    }
    let r = cycle::decode_with(&x, &mut HashBook::new(), &cycle::Settings::DEFAULT);
    assert!(
        r.decodes.is_empty(),
        "{:?}",
        r.decodes
            .iter()
            .map(|d| d.message.text())
            .collect::<Vec<_>>()
    );
}

#[test]
fn a_hash_sent_in_the_same_cycle_as_its_call_is_named() {
    use super::hash::HashBits;
    let mut rng = Rng::new(89);
    let mut sent = readable(2, -10.0, &mut rng);
    sent[0].payload = message::pack::pack(&Message::NonStandard {
        to: Call::Cq(None),
        from: Call::NonStd("PJ4/K1ABC".into()),
        word: G15::Blank,
    });
    let h22 = super::fields::hashes("PJ4/K1ABC").unwrap().h22;
    sent[1].payload = message::pack::pack(&Message::Standard {
        to: Call::Std {
            call: "W9XYZ".into(),
            suffix: None,
        },
        from: Call::Hashed {
            bits: HashBits::H22,
            value: h22,
            call: None,
        },
        ack: false,
        word: G15::Report(-11),
    });
    let x = cycle(&sent, &mut rng);
    let r = cycle::decode_with(&x, &mut HashBook::new(), &QUICK);
    let texts: Vec<String> = r.decodes.iter().map(|d| d.message.text()).collect();
    assert!(
        texts.contains(&"W9XYZ <PJ4/K1ABC> -11".to_string()),
        "{texts:?}"
    );
}

/// One crowded cycle, timed pass by pass.
#[test]
#[ignore]
fn time_a_crowded_cycle() {
    use std::time::Instant;
    let mut rng = Rng::new(59);
    let mut sent = readable_apart(40, -10.0, 0.0, &mut rng);
    for s in sent.iter_mut() {
        s.snr_db = -6.0 - rng.unit() * 14.0;
        s.f0 = 200.0 + rng.unit() * 2700.0;
    }
    let x = cycle(&sent, &mut rng);
    for passes in 1..=3 {
        let t = Instant::now();
        let settings = cycle::Settings {
            max_passes: passes,
            ..cycle::Settings::DEFAULT
        };
        let r = cycle::decode_with(&x, &mut HashBook::new(), &settings);
        println!(
            "{passes} passes: {:?}, {} decodes, {} candidates",
            t.elapsed(),
            r.decodes.len(),
            r.candidates
        );
    }
}

#[test]
fn the_band_shows_energy_where_a_signal_sits() {
    let mut rng = Rng::new(97);
    let mut sent = readable(1, -5.0, &mut rng);
    sent[0].f0 = 1500.0;
    let x = cycle(&sent, &mut rng);
    let band = Spectrogram::new(&x).band(25.0, 3000.0);
    assert_eq!(band.len(), 120);
    // The signal's 50 Hz from 1500 Hz stands out; far from it, the floor.
    assert!(band[61] > 3.0, "{}", band[61]);
    assert!(band[20].abs() < 1.0, "{}", band[20]);
}

/// Every decode of many crowded cycles, true or not, beside what the guard
/// and the contradiction read on it: the population a guard is set from.
/// Cycles of 12 transmissions, fading by 0.5, 1 or 2 Hz at -18 to -21 dB
/// and steady at -20 to -22 dB (or strong, `FT8_GUARD_STRONG`),
/// `FT8_GUARD_CYCLES` of each kind (100); the
/// settings as [`settings_from_env`]. One `ROW` line per decode:
/// `kind spread snr_set truth osd costas repaired distance snr contradiction
/// against_noise mismatched`.
#[test]
#[ignore]
fn measure_the_guards_population() {
    let n: usize = std::env::var("FT8_GUARD_CYCLES").map_or(100, |v| v.parse().unwrap());
    let seed: u64 = std::env::var("FT8_SEED").map_or(71, |v| v.parse().unwrap());
    let settings = settings_from_env();
    // `FT8_GUARD_STRONG=1`: strong and crowded instead, 20 a cycle from -10
    // to -17 dB, four pairs within a tone of each other.
    let strong = std::env::var("FT8_GUARD_STRONG").is_ok_and(|v| v != "0");
    let mut rng = Rng::new(seed);
    let (mut right, mut wrong) = (0, 0);
    for i in 0..2 * n {
        let fading = i < n;
        let spread = [0.5, 1.0, 2.0][i % 3];
        let snr = if strong {
            -10.0 - (i % 8) as f64
        } else if fading {
            -18.0 - (i % 4) as f64
        } else {
            -20.0 - (i % 3) as f64
        };
        let mut sent = readable(if strong { 20 } else { 12 }, snr, &mut rng);
        if strong {
            for s in sent.iter_mut() {
                s.snr_db = -10.0 - rng.unit() * 7.0;
                s.f0 = 200.0 + rng.unit() * 2700.0;
            }
            for k in 0..4 {
                sent[2 * k + 1].f0 = sent[2 * k].f0 + rng.unit() * 6.25;
            }
        }
        let x = if fading {
            faded(&sent, spread, &mut rng).0
        } else {
            cycle(&sent, &mut rng)
        };
        let r = cycle::decode_with(&x, &mut HashBook::new(), &settings);
        for d in &r.decodes {
            let truth = sent.iter().any(|s| s.payload == d.payload);
            if truth {
                right += 1;
            } else {
                wrong += 1;
            }
            let t = &d.trace;
            let (mut against, mut mismatched) = (0f32, 0);
            for s in modulate::data_positions() {
                let row = &t.energies[s][1..=8];
                let sent_e = row[usize::from(t.tones[s])];
                let best = row.iter().copied().fold(f32::MIN, f32::max);
                if best > sent_e {
                    against += (best - sent_e) / t.noise;
                    mismatched += 1;
                }
            }
            println!(
                "ROW {} {} {} {} {} {} {} {:.4} {:.2} {:.4} {:.2} {}",
                if fading { "fade" } else { "awgn" },
                if fading { spread } else { 0.0 },
                snr,
                u8::from(truth),
                d.osd.map_or(-1, i32::from),
                d.costas,
                d.repaired,
                d.distance,
                d.physics.snr_db,
                t.contradiction(),
                against,
                mismatched
            );
        }
    }
    println!("{right} right, {wrong} wrong");
}
