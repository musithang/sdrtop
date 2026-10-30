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
use super::ldpc;
use super::modulate::{self, Waveform, NOMINAL_START, SYMBOLS};
use super::search::{self, Baseband, Spectrogram, RATE};
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
    let mut out: Vec<Sent> = Vec::new();
    while out.len() < n {
        let f0 = 200.0 + rng.unit() * 2700.0;
        if out.iter().any(|s| (s.f0 - f0).abs() < 60.0) {
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
