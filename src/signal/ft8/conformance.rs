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
                }
                None => self.wrong += 1,
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
            rows.add(&sent, &x, &cycle::Settings::DEFAULT);
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

/// Noise alone, every candidate decoded: what comes out is false.
#[test]
#[ignore]
fn measure_false_decodes() {
    let mut rng = Rng::new(53);
    let (mut false_decodes, mut malformed, mut cycles) = (0, 0, 0);
    for _ in 0..40 {
        let x = cycle(&[], &mut rng);
        let r = cycle::decode(&x, &mut HashBook::new());
        false_decodes += r.decodes.len();
        malformed += r.malformed;
        cycles += 1;
        for d in &r.decodes {
            println!(
                "false: {} osd {:?} block {} costas {} repaired {}",
                d.message.text(),
                d.osd,
                d.block,
                d.costas,
                d.repaired
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
        rows.add(&sent, &x, &cycle::Settings::DEFAULT);
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

#[test]
fn noise_alone_reads_as_nothing() {
    let mut rng = Rng::new(83);
    let x = cycle(&[], &mut rng);
    let r = cycle::decode_with(&x, &mut HashBook::new(), &QUICK);
    assert!(r.decodes.is_empty(), "{:?}", r.decodes);
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
