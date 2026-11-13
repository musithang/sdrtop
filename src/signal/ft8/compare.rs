// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Our decoder beside another one, on the same samples. Test-only: nothing
//! here ships, and no other decoder is linked or called; this writes files
//! for one to read and reads files one wrote.
//!
//! Two directions:
//!
//! - `cycles_from_a_recording`: a SigMF recording (`Ctrl+R`) through our own
//!   channel, cut on the UTC quarter minutes, each cycle written as a 12 kHz
//!   16-bit WAV named `YYMMDD_HHMMSS.wav` (the name another decoder takes
//!   the cycle's time from), and our decodes of the very same samples in
//!   `ours.txt`.
//! - `decode_wavs`: every `*.wav` in a directory (a simulator's, say) read as
//!   upper sideband and decoded by ours, into `ours.txt`.
//!
//! The WAV is the upper sideband alone: the channel's complex samples with
//! the negative frequencies removed. Taking the real part of the complex
//! samples instead would fold the other sideband's noise onto the signals and
//! hand the other decoder a band 3 dB noisier than ours.
//!
//! `ours.txt` has one line per decode, in the other decoder's own column
//! order: `HHMMSS SNR DT FREQ ~ MESSAGE`.

use std::path::{Path, PathBuf};

use num_complex::Complex;
use rustfft::FftPlanner;

use super::channel::{Channel, RATE_HZ};
use super::cycle::{self, Settings};
use super::hash::HashBook;

const CYCLE: usize = 15 * RATE_HZ as usize;

fn rfc3339_to_unix(s: &str) -> f64 {
    // `2026-10-30T20:35:40.954746Z`
    let (date, time) = s.trim_end_matches('Z').split_once('T').unwrap();
    let mut d = date.split('-').map(|v| v.parse::<i64>().unwrap());
    let (y, m, day) = (d.next().unwrap(), d.next().unwrap(), d.next().unwrap());
    let mut t = time.split(':');
    let (hh, mm): (f64, f64) = (
        t.next().unwrap().parse().unwrap(),
        t.next().unwrap().parse().unwrap(),
    );
    let ss: f64 = t.next().unwrap().parse().unwrap();
    // Days from civil (H. Hinnant).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    days as f64 * 86_400.0 + hh * 3600.0 + mm * 60.0 + ss
}

/// A recording through FT8's channel: the UTC of the channel's first
/// sample, and the channel's samples. Read block by block, so a recording
/// of hours at the radio's rate never has to fit in memory (two hours of the
/// HF+ at 192 kS/s are 11.5 GB of pairs; at 12 kS/s they are 0.7 GB).
fn channel_of_recording(meta: &Path) -> (f64, Vec<Complex<f32>>) {
    channel_of_recording_from(meta, 0)
}

/// The same from capture `first` on: the channel started where the stream
/// resumed after a break, as the live worker starts it again at every break,
/// and everything before left out.
fn channel_of_recording_from(meta: &Path, first: usize) -> (f64, Vec<Complex<f32>>) {
    use std::io::{Read, Seek};
    let m: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(meta).unwrap()).unwrap();
    let g = &m["global"];
    let rate = g["core:sample_rate"].as_f64().unwrap();
    let full = g["sdrtop:full_scale"].as_f64().unwrap_or(1.0) as f32;
    let kind = g["core:datatype"].as_str().unwrap().to_string();
    let width = match kind.as_str() {
        "ci32_le" | "cf32_le" => 8,
        "ci16_le" => 4,
        "ci8" => 2,
        other => panic!("datatype {other} not read here"),
    };
    let pair = |c: &[u8]| -> Complex<f32> {
        match kind.as_str() {
            "ci32_le" => Complex::new(
                i32::from_le_bytes(c[..4].try_into().unwrap()) as f32 / full,
                i32::from_le_bytes(c[4..8].try_into().unwrap()) as f32 / full,
            ),
            "cf32_le" => Complex::new(
                f32::from_le_bytes(c[..4].try_into().unwrap()),
                f32::from_le_bytes(c[4..8].try_into().unwrap()),
            ),
            "ci16_le" => Complex::new(
                f32::from(i16::from_le_bytes([c[0], c[1]])) / full,
                f32::from(i16::from_le_bytes([c[2], c[3]])) / full,
            ),
            _ => Complex::new(f32::from(c[0] as i8) / full, f32::from(c[1] as i8) / full),
        }
    };
    // The samples the driver lost are known to the sample: a capture's
    // global index runs ahead of its place in the file by exactly them. They
    // are filled with zeros, so the recording is one timeline and every
    // cycle in it can be cut; both decoders are handed the same gaps.
    let caps = &m["captures"].as_array().unwrap()[first..];
    let at = |c: &serde_json::Value, k: &str| c[k].as_u64().unwrap() as usize;
    let mut ch = Channel::new(rate).expect("a route to 12 000 S/s");
    let mut x = Vec::new();
    let mut file =
        std::io::BufReader::new(std::fs::File::open(meta.with_extension("sigmf-data")).unwrap());
    let mut buf = vec![0u8; width * 65_536];
    // The channel is handed 16 384 pairs at a time from the start, zero
    // fills included, whatever the file reads return: its DC estimate is
    // seeded from the first block, so the blocks are kept as they always
    // were and the channel's output is what it always was.
    const BLOCK: usize = 16_384;
    let mut staged: Vec<Complex<f32>> = Vec::with_capacity(2 * BLOCK + 65_536);
    let mut feed = |pairs: &mut dyn Iterator<Item = Complex<f32>>, x: &mut Vec<Complex<f32>>| {
        staged.extend(pairs);
        let whole = staged.len() / BLOCK * BLOCK;
        for block in staged[..whole].chunks(BLOCK) {
            ch.push(block, x);
        }
        staged.drain(..whole);
    };
    let g0 = at(&caps[0], "core:global_index");
    let mut place = g0;
    let mut lost = 0usize;
    file.seek(std::io::SeekFrom::Start(
        (at(&caps[0], "core:sample_start") * width) as u64,
    ))
    .unwrap();
    for (k, c) in caps.iter().enumerate() {
        let index = at(c, "core:global_index");
        if place < index {
            let n = index - place;
            feed(&mut std::iter::repeat_n(Complex::new(0.0, 0.0), n), &mut x);
            place += n;
            lost += n;
        }
        let (from, to) = (
            at(c, "core:sample_start"),
            caps.get(k + 1)
                .map_or(usize::MAX, |n| at(n, "core:sample_start")),
        );
        let mut left = to.saturating_sub(from);
        while left > 0 {
            let want = left.min(65_536) * width;
            let got = file.read(&mut buf[..want]).unwrap();
            if got == 0 {
                break;
            }
            feed(
                &mut buf[..got - got % width].chunks(width).map(pair),
                &mut x,
            );
            place += got / width;
            left -= got / width;
        }
    }
    if !staged.is_empty() {
        ch.push(&staged, &mut x);
    }
    // Arrivals are late, never early: the earliest any capture allows.
    let t0 = caps
        .iter()
        .map(|c| {
            rfc3339_to_unix(c["core:datetime"].as_str().unwrap())
                - (at(c, "core:global_index") - g0) as f64 / rate
        })
        .fold(f64::INFINITY, f64::min);
    println!(
        "{:.1} s at {rate} S/s, {lost} pairs lost and filled ({:.3} %)",
        (place - g0) as f64 / rate,
        lost as f64 * 100.0 / (place - g0).max(1) as f64
    );
    // Channel sample k was taken at t0 + k / 12000 - the channel's delay.
    (t0 - ch.delay(), x)
}

/// The upper sideband of a cycle, as real audio with peaks near full scale.
fn usb_audio(x: &[Complex<f32>]) -> Vec<i16> {
    let n = x.len();
    let mut planner = FftPlanner::<f32>::new();
    let mut buf = x.to_vec();
    planner.plan_fft_forward(n).process(&mut buf);
    // Keep 0 Hz up to (not including) Nyquist; the rest is the other side.
    for b in buf.iter_mut().skip(n / 2) {
        *b = Complex::new(0.0, 0.0);
    }
    planner.plan_fft_inverse(n).process(&mut buf);
    let real: Vec<f32> = buf.iter().map(|c| c.re).collect();
    let peak = real.iter().fold(0f32, |a, v| a.max(v.abs())).max(1e-12);
    real.iter()
        .map(|v| (v / peak * 20_000.0).round() as i16)
        .collect()
}

/// Real audio back to the complex samples the decoder takes: the analytic
/// signal, the negative frequencies removed.
fn analytic(audio: &[f32]) -> Vec<Complex<f32>> {
    let n = audio.len();
    let mut planner = FftPlanner::<f32>::new();
    let mut buf: Vec<Complex<f32>> = audio.iter().map(|&v| Complex::new(v, 0.0)).collect();
    planner.plan_fft_forward(n).process(&mut buf);
    let scale = 2.0 / n as f32;
    for (k, b) in buf.iter_mut().enumerate() {
        *b = if k == 0 || k >= n / 2 {
            Complex::new(0.0, 0.0)
        } else {
            *b * scale
        };
    }
    planner.plan_fft_inverse(n).process(&mut buf);
    buf
}

fn write_wav(path: &Path, audio: &[i16]) {
    let mut out = Vec::with_capacity(44 + audio.len() * 2);
    let data_len = (audio.len() * 2) as u32;
    out.extend(b"RIFF");
    out.extend((36 + data_len).to_le_bytes());
    out.extend(b"WAVEfmt ");
    out.extend(16u32.to_le_bytes());
    out.extend(1u16.to_le_bytes()); // PCM
    out.extend(1u16.to_le_bytes()); // mono
    out.extend((RATE_HZ as u32).to_le_bytes());
    out.extend((RATE_HZ as u32 * 2).to_le_bytes());
    out.extend(2u16.to_le_bytes());
    out.extend(16u16.to_le_bytes());
    out.extend(b"data");
    out.extend(data_len.to_le_bytes());
    for s in audio {
        out.extend(s.to_le_bytes());
    }
    std::fs::write(path, out).unwrap();
}

/// The 16-bit mono samples of a WAV, wherever its data chunk is.
fn read_wav(path: &Path) -> Vec<f32> {
    let b = std::fs::read(path).unwrap();
    let mut at = 12;
    while at + 8 <= b.len() {
        let id = &b[at..at + 4];
        let len = u32::from_le_bytes(b[at + 4..at + 8].try_into().unwrap()) as usize;
        if id == b"data" {
            return b[at + 8..(at + 8 + len).min(b.len())]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| f32::from(i16::from_le_bytes(*c)) / 32768.0)
                .collect();
        }
        at += 8 + len + (len & 1);
    }
    panic!("no data chunk in {}", path.display());
}

/// `YYMMDD` of a Unix time, UTC (H. Hinnant's civil from days).
fn yymmdd(unix: f64) -> String {
    let z = (unix / 86_400.0).floor() as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{:02}{m:02}{d:02}", y % 100)
}

fn hhmmss(unix: f64) -> String {
    let s = unix.rem_euclid(86_400.0) as u64;
    format!("{:02}{:02}{:02}", s / 3600, (s / 60) % 60, s % 60)
}

/// The decoder's settings, any of them from the environment
/// ([`super::conformance::settings_from_env`]).
fn settings() -> Settings {
    super::conformance::settings_from_env()
}

/// How much what was received speaks against the tones a decode was sent
/// as, over the 58 data symbols: the energy on a symbol's strongest tone
/// beyond the decoded one's, summed where they differ, over the signal's
/// energy on the decoded tones; the largest one such excess over a cell's
/// noise; how many of the stronger half of the symbols differ; and how many
/// differ where the strongest tone stands 8 dB over the noise.
fn contradiction(t: &super::trace::Trace) -> (f32, f32, usize, usize) {
    let data: Vec<usize> = super::modulate::data_positions().collect();
    let row = |s: usize| -> [f32; 8] { std::array::from_fn(|k| t.energies[s][k + 1]) };
    let (mut against, mut signal, mut worst) = (0f32, 0f32, 0f32);
    let mut by_strength: Vec<(f32, bool)> = Vec::new();
    let mut loud = 0;
    for &s in &data {
        let r = row(s);
        let sent = r[usize::from(t.tones[s])];
        let best = r.iter().copied().fold(f32::MIN, f32::max);
        signal += (sent - t.noise).max(0.0);
        let differs = best > sent;
        if differs {
            against += best - sent;
            worst = worst.max((best - sent) / t.noise);
            if best > 6.3 * t.noise {
                loud += 1;
            }
        }
        by_strength.push((best, differs));
    }
    by_strength.sort_by(|a, b| b.0.total_cmp(&a.0));
    let strong = by_strength[..by_strength.len() / 2]
        .iter()
        .filter(|(_, d)| *d)
        .count();
    (against / signal.max(f32::MIN_POSITIVE), worst, strong, loud)
}

fn decode_lines(label: &str, x: &[Complex<f32>], book: &mut HashBook) -> Vec<String> {
    let t = std::time::Instant::now();
    let report = cycle::decode_with(x, book, &settings());
    eprintln!(
        "{label}: {:.2} s, {} candidates",
        t.elapsed().as_secs_f64(),
        report.candidates
    );
    for d in report
        .decodes
        .iter()
        .filter(|d| d.message.text().trim().is_empty())
    {
        eprintln!(
            "EMPTY {label} f0 {:.1} payload {:#x} costas {} block {} iter {} repaired {} osd {:?} pass {} message {:?}",
            d.physics.f0, d.payload, d.costas, d.block, d.iterations, d.repaired, d.osd, d.pass, d.message
        );
    }
    // `FT8_FEATURES=1`: what each decode measured and took, one line each
    // on stderr, to tell false decodes from true ones by.
    if std::env::var("FT8_FEATURES").is_ok() {
        for d in &report.decodes {
            let c = contradiction(&d.trace);
            eprintln!(
                "FEAT {label} {:.2} {:.1} {} {} {} {} {} {:.4} {} {:.3} {:.3} {:.4} {:.4} {:.2} {} {} {}",
                d.physics.snr_db,
                d.physics.f0,
                d.costas,
                d.block,
                d.iterations,
                d.repaired,
                d.osd.map_or(-1, i32::from),
                d.distance,
                d.pass,
                d.physics.coherence,
                d.physics.drift,
                d.physics.f0_sigma,
                c.0,
                c.1,
                c.2,
                c.3,
                d.message.text()
            );
        }
    }
    report
        .decodes
        .iter()
        .map(|d| {
            format!(
                "{label} {:>3.0} {:>4.1} {:>4.0} ~  {}",
                d.physics.snr_db,
                d.dt,
                d.physics.f0,
                d.message.text()
            )
        })
        .collect()
}

fn out_dir() -> PathBuf {
    let dir = PathBuf::from(std::env::var("FT8_OUT").expect("FT8_OUT: where to write"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// `FT8_SIGMF=<recording>.sigmf-meta FT8_OUT=<dir> cargo test --release
/// cycles_from_a_recording -- --ignored --nocapture`
#[test]
#[ignore]
fn cycles_from_a_recording() {
    let Ok(meta) = std::env::var("FT8_SIGMF") else {
        return;
    };
    let (first, x) = channel_of_recording(Path::new(&meta));
    let dir = out_dir();
    let mut ours = Vec::new();
    let mut book = HashBook::new();
    if let Ok(call) = std::env::var("FT8_CALL") {
        book.learn(&call);
    }
    let mut start = (first / 15.0).ceil() * 15.0;
    loop {
        let i = ((start - first) * RATE_HZ as f64).round() as usize;
        if i + CYCLE > x.len() {
            break;
        }
        let cyc = &x[i..i + CYCLE];
        let name = format!("{}_{}.wav", yymmdd(start), hhmmss(start));
        write_wav(&dir.join(&name), &usb_audio(cyc));
        let lines = decode_lines(&hhmmss(start), cyc, &mut book);
        println!("{name}: {} decodes", lines.len());
        ours.extend(lines);
        start += 15.0;
    }
    std::fs::write(dir.join("ours.txt"), ours.join("\n") + "\n").unwrap();
}

/// One cycle of a recording cut the way the live worker may have cut it:
/// the channel started at capture `FT8_CAPTURE` (where the stream resumed
/// after a break, as the worker starts it again), the cycle `FT8_CYCLE`
/// (`hhmmss`) cut at each shift of its start in `FT8_SHIFTS` (`from:to:step`,
/// milliseconds), since the live clock places the boundary by its own fit.
/// Every decode is printed (with `FT8_FEATURES=1`, what it measured).
///
/// `FT8_SIGMF=<rec>.sigmf-meta FT8_CAPTURE=13 FT8_CYCLE=125400
/// FT8_SHIFTS=-100:100:10 cargo test --release a_cycle_as_the_live_worker_cut_it
/// -- --ignored --nocapture`
#[test]
#[ignore]
fn a_cycle_as_the_live_worker_cut_it() {
    let Ok(meta) = std::env::var("FT8_SIGMF") else {
        return;
    };
    let var = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("{k}"));
    let capture: usize = var("FT8_CAPTURE").parse().unwrap();
    let cycle = var("FT8_CYCLE");
    let shifts: Vec<i64> = var("FT8_SHIFTS")
        .split(':')
        .map(|v| v.parse().unwrap())
        .collect();
    let (first, x) = channel_of_recording_from(Path::new(&meta), capture);
    // The cycle's start: the first quarter minute after the channel's start
    // whose time of day is `hhmmss`.
    let of_day = |h: &str| {
        let n: f64 = h.parse().unwrap();
        (n / 10_000.0).floor() * 3600.0 + (n / 100.0 % 100.0).floor() * 60.0 + n % 100.0
    };
    let day = (first / 86_400.0).floor() * 86_400.0;
    let start = day + of_day(&cycle);
    let mut book = HashBook::new();
    let mut ms = shifts[0];
    while ms <= shifts[1] {
        let at = start + ms as f64 / 1000.0;
        let i = ((at - first) * RATE_HZ as f64).round() as usize;
        assert!(i + CYCLE <= x.len(), "the cycle runs past the recording");
        for line in decode_lines(&format!("{ms:+05}"), &x[i..i + CYCLE], &mut book) {
            println!("{line}");
        }
        ms += shifts[2];
    }
}

/// `FT8_WAVS=<dir> FT8_OUT=<dir> cargo test --release decode_wavs --
/// --ignored --nocapture`
#[test]
#[ignore]
fn decode_wavs() {
    let Ok(wavs) = std::env::var("FT8_WAVS") else {
        return;
    };
    let mut files: Vec<PathBuf> = std::fs::read_dir(&wavs)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "wav"))
        .collect();
    files.sort();
    let mut ours = Vec::new();
    let mut book = HashBook::new();
    for f in files {
        let mut audio = read_wav(&f);
        audio.resize(CYCLE, 0.0);
        let stem = f.file_stem().unwrap().to_string_lossy().to_string();
        let label = stem.rsplit('_').next().unwrap_or("000000").to_string();
        ours.extend(decode_lines(&label, &analytic(&audio), &mut book));
    }
    std::fs::write(out_dir().join("ours.txt"), ours.join("\n") + "\n").unwrap();
}

#[test]
fn the_wav_and_the_time_helpers_read_back() {
    assert_eq!(rfc3339_to_unix("1970-01-01T00:00:00Z"), 0.0);
    assert_eq!(yymmdd(1_793_366_100.0), "261030");
    assert_eq!(hhmmss(1_793_366_100.0), "131500");
    assert!((rfc3339_to_unix("2026-10-30T13:15:00.5Z") - 1_793_366_100.5).abs() < 1e-6);
    let dir = std::env::temp_dir().join(format!("sdrtop-ft8-compare-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("t.wav");
    write_wav(&p, &[0, 16384, -16384, 32767]);
    let back = read_wav(&p);
    assert_eq!(back.len(), 4);
    assert!((back[1] - 0.5).abs() < 1e-6);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A tone in the upper sideband survives the trip to audio and back: same
/// frequency, and nothing left at the mirror image.
#[test]
fn upper_sideband_audio_round_trips() {
    let n = 12_000;
    let f = 1500.0;
    let x: Vec<Complex<f32>> = (0..n)
        .map(|k| {
            let ph = std::f32::consts::TAU * f * k as f32 / RATE_HZ as f32;
            Complex::new(ph.cos(), ph.sin())
        })
        .collect();
    let audio: Vec<f32> = usb_audio(&x)
        .iter()
        .map(|&v| f32::from(v) / 32768.0)
        .collect();
    let back = analytic(&audio);
    let mut buf = back.clone();
    FftPlanner::<f32>::new()
        .plan_fft_forward(n)
        .process(&mut buf);
    let (peak, _) = buf
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.norm().total_cmp(&b.1.norm()))
        .unwrap();
    assert_eq!(peak, 1500);
    assert!(buf[n - 1500].norm() < buf[1500].norm() * 1e-3);
}

/// Where the decodes another decoder made and ours did not are lost. For
/// each line of `FT8_MISSES` (`hhmmss f0 dt text`, jt9's columns, from the
/// WAVs in `FT8_WAVS`): pass by pass, on what our own decodes of the
/// earlier passes left, the nearest candidate the search put there (its
/// rank of how many, its Costas score) and what reading it came to; then a
/// candidate placed by hand at jt9's own place, read the same way.
///
/// `FT8_WAVS=<dir> FT8_MISSES=<file> cargo test --release
/// why_the_other_decodes_are_missed -- --ignored --nocapture`
#[test]
#[ignore]
fn why_the_other_decodes_are_missed() {
    use super::fields::G15;
    use super::message::{pack::pack, Call, Message, Suffix};
    use super::search::{self, Baseband, Candidate, Spectrogram, DOWN_RATE};
    use super::{crc, demod, ldpc};
    // The payload of a plain two-call message, from its text: `None` for
    // anything else (a hashed call, free text).
    let truth = |text: &[&str]| -> Option<u128> {
        let call = |w: &str| -> Option<Call> {
            if w.contains('<') {
                return None;
            }
            if w == "CQ" {
                return Some(Call::Cq(None));
            }
            let (c, suffix) = match w.split_once('/') {
                Some((c, "P")) => (c, Some(Suffix::P)),
                Some((c, "R")) => (c, Some(Suffix::R)),
                Some(_) => return None,
                None => (w, None),
            };
            Some(Call::Std {
                call: c.to_string(),
                suffix,
            })
        };
        let [to, from, last] = text else {
            return None;
        };
        let (ack, word) = match *last {
            "RR73" => (false, G15::Rr73),
            "RRR" => (false, G15::Rrr),
            "73" => (false, G15::R73),
            w if w.starts_with("R+") || w.starts_with("R-") => {
                (true, G15::Report(w[1..].parse().ok()?))
            }
            w if w.starts_with('+') || w.starts_with('-') => (false, G15::Report(w.parse().ok()?)),
            w if w.len() == 4 => (false, G15::Grid(w.to_string())),
            _ => return None,
        };
        Some(pack(&Message::Standard {
            to: call(to)?,
            from: call(from)?,
            ack,
            word,
        }))
    };
    // Read at a place: wrong hard decisions against the truth per block, and
    // whether the code alone (no guard) comes back with the truth.
    let against = |bb: &Baseband, start: f64, f0: f64, payload: u128| -> String {
        let word = ldpc::encode(crc::with_crc(payload));
        let (y, at_zero) = bb.around(f0);
        let z = demod::correlate(&y, (start * DOWN_RATE).round() as i64, f0 - at_zero);
        let mut out = format!("costas {:2}", demod::costas_matched(&z));
        for b in 1..=3 {
            let llr = demod::soft_bits(&z, b, demod::LLR_SCALE);
            let wrong = llr
                .iter()
                .zip(word.iter())
                .filter(|(l, &w)| (**l > 0.0) != (w == 1))
                .count();
            let d = ldpc::decode(&llr, 30, 2);
            let got = match d {
                Some(d) if d.payload == payload => {
                    if d.osd.is_some() {
                        "OSD"
                    } else {
                        "BP"
                    }
                }
                Some(_) => "other",
                None => "-",
            };
            out += &format!("  b{b} {wrong:3} wrong {got}");
        }
        out
    };
    let (Ok(wavs), Ok(misses)) = (std::env::var("FT8_WAVS"), std::env::var("FT8_MISSES")) else {
        return;
    };
    let settings = settings();
    let say = |e: &cycle::Explained| -> String {
        match e.believed {
            Some((p, costas, osd, rep)) => {
                let text = super::message::unpack(p, &HashBook::new())
                    .map_or("malformed".to_string(), |m| m.text());
                format!("DECODES {text:?} costas {costas} osd {osd:?} repaired {rep}")
            }
            None if !e.synced => "below the power floor".to_string(),
            None if e.refused_zero => "refused: all-zero codeword".to_string(),
            None if e.refused_osd => "refused: OSD not believed".to_string(),
            None if e.coded => "coded, not believed".to_string(),
            None => "no codeword".to_string(),
        }
    };
    for line in std::fs::read_to_string(misses).unwrap().lines() {
        let mut it = line.split_whitespace();
        let (Some(t), Some(f0), Some(dt)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        let text: Vec<&str> = it.collect();
        let f0: f64 = f0.parse().unwrap();
        let start = dt.parse::<f64>().unwrap() + super::modulate::NOMINAL_START;
        let path = std::fs::read_dir(&wavs)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .find(|p| p.to_string_lossy().ends_with(&format!("{t}.wav")))
            .unwrap_or_else(|| panic!("no WAV for {t}"));
        let mut a = read_wav(&path);
        a.resize(CYCLE, 0.0);
        let mut x = analytic(&a);
        let report = cycle::decode_with(&x, &mut HashBook::new(), &settings);
        println!("{t} {f0} Hz dt {dt} {}", text.join(" "));
        for pass in 1..=settings.max_passes as u8 {
            if pass > 1 {
                for d in report.decodes.iter().filter(|d| d.pass == pass - 1) {
                    cycle::take_out_decode(&mut x, d);
                }
            }
            let spec = Spectrogram::new(&x);
            let all = search::candidates_up_to(&spec, usize::MAX);
            let bb = Baseband::new(&x);
            let near = all
                .iter()
                .enumerate()
                .filter(|(_, c)| (c.f0 - f0).abs() < 5.0 && (c.start - start).abs() < 0.2)
                .min_by(|a, b| {
                    let d =
                        |c: &Candidate| (c.f0 - f0).abs() / 6.25 + (c.start - start).abs() / 0.16;
                    d(a.1).total_cmp(&d(b.1))
                });
            let truth_at = |start: f64, f0: f64| -> String {
                truth(&text).map_or(String::new(), |p| {
                    format!("\n          truth there: {}", against(&bb, start, f0, p))
                })
            };
            let refined = near.map(|(_, c)| {
                let s = search::refine(&bb, c);
                format!(
                    "\n          refined {:+.2} Hz {:+.1} ms power {:.2}{}",
                    s.f0 - f0,
                    (s.start - start) * 1e3,
                    s.power,
                    truth_at(s.start, s.f0)
                )
            });
            let found = match near {
                Some((rank, c)) => format!(
                    "cand #{rank} of {} ({:+.1} Hz {:+.0} ms sync {:.2}){}: {}",
                    all.len(),
                    c.f0 - f0,
                    (c.start - start) * 1e3,
                    c.sync,
                    if rank >= settings.max_candidates {
                        " PAST THE CAP"
                    } else {
                        ""
                    },
                    say(&cycle::explain(&bb, c, &settings))
                ),
                None => format!("no candidate near (of {})", all.len()),
            };
            let placed = Candidate {
                start,
                f0,
                sync: 0.0,
            };
            println!(
                "  pass {pass}: {found}{}\n          placed by hand: {}{}",
                refined.unwrap_or_default(),
                say(&cycle::explain(&bb, &placed, &settings)),
                truth_at(start, f0)
            );
        }
    }
}

/// Where a decode is lost, cycle by cycle, on simulated cycles of one known
/// transmission: whether the search put a candidate near it (and its rank),
/// and, read at the place the search refined it to, how many of the 174
/// hard decisions were wrong and whether the code decoded.
///
/// `FT8_WAVS=<dir> FT8_F0=1200 FT8_START=0.8 cargo test --release
/// where_decodes_are_lost -- --ignored --nocapture`
#[test]
#[ignore]
fn where_decodes_are_lost() {
    use super::demod;
    use super::ldpc;
    use super::search::{self, Baseband, Spectrogram, DOWN_RATE};
    let Ok(wavs) = std::env::var("FT8_WAVS") else {
        return;
    };
    let f0: f64 = std::env::var("FT8_F0").map_or(1200.0, |v| v.parse().unwrap());
    let start: f64 = std::env::var("FT8_START").map_or(0.8, |v| v.parse().unwrap());
    let mut files: Vec<PathBuf> = std::fs::read_dir(&wavs)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "wav"))
        .collect();
    files.sort();
    let cycles: Vec<Vec<Complex<f32>>> = files
        .iter()
        .map(|f| {
            let mut a = read_wav(f);
            a.resize(CYCLE, 0.0);
            analytic(&a)
        })
        .collect();
    // The true codeword: from whichever cycle decodes.
    let mut book = HashBook::new();
    let truth = cycles
        .iter()
        .find_map(|x| {
            cycle::decode_with(x, &mut book, &Settings::DEFAULT)
                .decodes
                .first()
                .map(|d| d.payload)
        })
        .expect("no cycle decodes at all");
    let word = ldpc::encode(super::crc::with_crc(truth));
    println!("file  full  cand(rank,sync)  synced(dt ms, df Hz, power)  wrong(b1,b2,b3)  at-truth(b1,b2,b3)");
    for (f, x) in files.iter().zip(&cycles) {
        let full = !cycle::decode_with(x, &mut HashBook::new(), &Settings::DEFAULT)
            .decodes
            .is_empty();
        let spec = Spectrogram::new(x);
        let cands = search::candidates(&spec);
        let near = cands
            .iter()
            .enumerate()
            .find(|(_, c)| (c.f0 - f0).abs() < 4.0 && (c.start - start).abs() < 0.12);
        let bb = Baseband::new(x);
        let read = |s: f64, hz0: f64| -> ([usize; 3], [bool; 3]) {
            let (y, at_zero) = bb.around(hz0);
            let n0 = (s * DOWN_RATE).round() as i64;
            let z = demod::correlate(&y, n0, hz0 - at_zero);
            let mut wrong = [0; 3];
            let mut ok = [false; 3];
            for b in 1..=3 {
                let llr = demod::soft_bits(&z, b, demod::LLR_SCALE);
                wrong[b - 1] = llr
                    .iter()
                    .zip(word.iter())
                    .filter(|(l, &w)| (**l > 0.0) != (w == 1))
                    .count();
                ok[b - 1] = ldpc::decode(&llr, 30, 2).is_some_and(|d| d.payload == truth);
            }
            (wrong, ok)
        };
        let synced = near.map(|(_, c)| search::refine(&bb, c));
        let (w, okc) = synced.map_or(([0; 3], [false; 3]), |s| read(s.start, s.f0));
        let (wt, okt) = read(start, f0);
        // The second attempt the decoder makes: every symbol's place.
        let all = synced.map(|s| {
            let (y, at_zero) = bb.around(s.f0);
            let (st, hz) = demod::resync(&y, s.start * DOWN_RATE, s.f0 - at_zero);
            (st, hz + at_zero)
        });
        let (wa, oka) = all.map_or(([0; 3], [false; 3]), |b| read(b.0, b.1));
        // Data-aided: the tones the one-symbol decisions chose, fitted
        // coherently over the whole transmission.
        let dd = all.map(|(st, f)| {
            let (y, at_zero) = bb.around(f);
            let hz = f - at_zero;
            let n0 = (st * DOWN_RATE).round() as i64;
            let z = demod::correlate(&y, n0, hz);
            let tones: [u8; super::modulate::SYMBOLS] = std::array::from_fn(|k| {
                super::modulate::COSTAS_AT
                    .iter()
                    .find(|&&a| k >= a && k < a + 7)
                    .map(|&a| super::modulate::COSTAS[k - a])
                    .unwrap_or_else(|| {
                        (0..8u8)
                            .max_by(|&a, &b| {
                                z[k][usize::from(a)]
                                    .norm_sqr()
                                    .total_cmp(&z[k][usize::from(b)].norm_sqr())
                            })
                            .unwrap()
                    })
            });
            let ph = demod::measure(&y, n0 as f64 / DOWN_RATE, hz, &tones, &z);
            (ph.start, ph.f0 + at_zero)
        });
        let (wd, okd) = dd.map_or(([0; 3], [false; 3]), |b| read(b.0, b.1));
        println!(
            "   dd: {} {:?}{}",
            dd.map_or("-".to_string(), |b| format!(
                "{:+.1} {:+.2}",
                (b.0 - start) * 1e3,
                b.1 - f0
            )),
            wd,
            okd.map(|b| if b { 'D' } else { '.' })
                .iter()
                .collect::<String>()
        );
        let flag = |o: [bool; 3]| {
            o.map(|b| if b { 'D' } else { '.' })
                .iter()
                .collect::<String>()
        };
        println!(
            "{}  {}  {}  {}  {:?}{}  {:?}{}  all: {} {:?}{}",
            f.file_name().unwrap().to_string_lossy(),
            if full { "DEC " } else { "miss" },
            near.map_or("none".to_string(), |(r, c)| format!("#{r} {:.2}", c.sync)),
            synced.map_or("-".to_string(), |s| format!(
                "{:+.1} {:+.2} {:.2}",
                (s.start - start) * 1e3,
                s.f0 - f0,
                s.power
            )),
            w,
            flag(okc),
            wt,
            flag(okt),
            all.map_or("-".to_string(), |b| format!(
                "{:+.1} {:+.2}",
                (b.0 - start) * 1e3,
                b.1 - f0
            )),
            wa,
            flag(oka),
        );
    }
}
