// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! One 15 s cycle in, its decodes out: the search, the soft bits, the code
//! and the message, then the decoded signals taken out of the cycle and the
//! whole done again for what was beneath them (the QEX paper, section 6).
//!
//! **Subtraction, the paper's way.** A decoded signal is rebuilt at the
//! cycle's rate from its tones, its measured frequency, start and drift
//! (`r`), and the channel's slowly varying complex gain is estimated as a
//! low-pass of the received samples times `r*`; that gain times `r` is then
//! subtracted. Fading and phase wander are followed, not assumed away. The
//! signal here is complex baseband, so the paper's `2 Re[g r]` is simply
//! `g r`.
//!
//! **Passes.** After a pass that decoded something, the cycle is searched
//! again; a pass that decodes nothing new ends it, and so does
//! [`MAX_PASSES`]. A message decoded twice in one cycle (two candidates on one
//! transmission, or the same message found again after subtraction) is
//! reported once.

use std::collections::HashSet;

use num_complex::Complex;

use super::demod::{self, Physics};
use super::hash::HashBook;
use super::ldpc;
use super::message::{self, Message};
use super::modulate::{self, Waveform, NOMINAL_START, SYMBOLS};
use super::search::{self, Baseband, Spectrogram, DOWN_RATE, RATE};

/// The most passes over one cycle (the paper: a third pass "will sometimes"
/// find more).
pub const MAX_PASSES: usize = 3;

/// Candidates whose fine Costas power is below this are not decoded. Noise's
/// best candidates have a fine power of 1.7 (median) to 2.7 (most); a
/// decodable signal sits well above. Measured over 108 transmissions at -19
/// to -21 dB: every floor from 0 to 2.4 decoded the same 66, the time per
/// cycle on the dev machine falling from 4.0 s to 1.6 s; 2.2 (1.85 s) leaves
/// a margin under the lowest that lost nothing.
pub const MIN_POWER: f32 = 2.2;

/// The decoder's tunable numbers, so the conformance rows can try others.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub llr_scale: f32,
    /// Candidates whose fine Costas power is below this are not decoded.
    /// Zero decodes every candidate.
    pub min_power: f32,
    pub max_passes: usize,
    /// The most candidates examined in a pass, strongest first.
    pub max_candidates: usize,
}

impl Settings {
    pub const DEFAULT: Settings = Settings {
        llr_scale: demod::LLR_SCALE,
        min_power: MIN_POWER,
        max_passes: MAX_PASSES,
        max_candidates: search::MAX_CANDIDATES,
    };
}

/// Belief propagation's iteration limit, and the OSD order tried after it.
const BP_ITERATIONS: u32 = 30;
const OSD_ORDER: u8 = 2;

/// An OSD decode must show its sync: at least this many of the 21 Costas
/// symbols strongest on their own tone, and no more than
/// [`OSD_MAX_REPAIRED`] bits repaired. OSD's distance bound alone holds on
/// white-noise soft bits (`ldpc::OSD_MAX_DISTANCE`) but not on a real
/// cycle's candidates: a strong signal read at the wrong time or tone gives
/// confident, structured, wrong soft bits, and OSD finds a codeword whose CRC
/// passes near them. Measured over 27 cycles (`measure_what_parts_true_from_
/// false`): 16 true OSD decodes had 12 to 20 Costas symbols right and 16 to
/// 35 bits repaired; 19 false ones had 3 to 9 and 35 to 48. Either test alone
/// parts them; both together leave a margin on each.
const OSD_MIN_COSTAS: u8 = 11;
const OSD_MAX_REPAIRED: u32 = 34;

/// The low-pass that estimates the channel's gain during subtraction: a
/// moving average this many seconds long.
const GAIN_SECONDS: f64 = 0.32;

/// One transmission read, and what it took.
#[derive(Clone, Debug, PartialEq)]
pub struct Decode {
    pub message: Message,
    pub payload: u128,
    /// Seconds from the nominal start, 0.5 s into the cycle.
    pub dt: f64,
    pub physics: Physics,
    /// Costas symbols on the right tone, of 21.
    pub costas: u8,
    /// Symbols per block in the soft decisions that decoded (1, 2 or 3).
    pub block: u8,
    pub iterations: u32,
    /// Received bits the codeword disagreed with, of 174.
    pub repaired: u32,
    pub osd: Option<u8>,
    /// The codeword's distance from what was received (`ldpc::Decoded`).
    pub distance: f32,
    /// The pass that found it, from 1.
    pub pass: u8,
}

/// A cycle's decodes and the count of what did not become one.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    pub decodes: Vec<Decode>,
    /// Candidates examined, over all passes.
    pub candidates: usize,
    /// Payloads whose check passed but whose fields the paper gives no
    /// meaning: counted, never shown.
    pub malformed: usize,
    pub passes: usize,
}

/// What a pass found before messages are unpacked.
struct Found {
    payload: u128,
    start: f64,
    physics: Physics,
    tones: [u8; SYMBOLS],
    costas: u8,
    block: u8,
    decoded: ldpc::Decoded,
    pass: u8,
}

/// Decode a cycle (at [`RATE`], the buffer starting at the 15 s boundary).
/// Calls heard in full are taught to `book` before any message is unpacked,
/// so a hash sent in this same cycle can already be named.
pub fn decode(samples: &[Complex<f32>], book: &mut HashBook) -> Report {
    decode_with(samples, book, &Settings::DEFAULT)
}

/// [`decode`] with other settings.
pub fn decode_with(samples: &[Complex<f32>], book: &mut HashBook, settings: &Settings) -> Report {
    let mut x = samples.to_vec();
    let mut found: Vec<Found> = Vec::new();
    let mut seen: HashSet<u128> = HashSet::new();
    let mut report = Report::default();
    for pass in 1..=settings.max_passes {
        report.passes = pass;
        let spec = Spectrogram::new(&x);
        let mut cands = search::candidates(&spec);
        cands.truncate(settings.max_candidates);
        report.candidates += cands.len();
        let bb = Baseband::new(&x);
        let mut new: Vec<Found> = Vec::new();
        for c in &cands {
            // A strong transmission raises many candidates around it, and
            // each decodes the same message: only the first is measured.
            let Some(f) = try_candidate(&bb, c, pass as u8, settings, &mut seen) else {
                continue;
            };
            new.push(f);
        }
        if new.is_empty() {
            break;
        }
        for f in &new {
            subtract(&mut x, f);
        }
        found.extend(new);
    }

    for f in &found {
        if let Ok(m) = message::unpack(f.payload, book) {
            for call in m.calls_in_full() {
                book.learn(call);
            }
        }
    }
    for f in found {
        match message::unpack(f.payload, book) {
            Ok(message) => report.decodes.push(Decode {
                message,
                payload: f.payload,
                dt: f.start - NOMINAL_START,
                physics: f.physics,
                costas: f.costas,
                block: f.block,
                iterations: f.decoded.iterations,
                repaired: f.decoded.repaired,
                osd: f.decoded.osd,
                distance: f.decoded.distance,
                pass: f.pass,
            }),
            Err(_) => report.malformed += 1,
        }
    }
    report
        .decodes
        .sort_by(|a, b| a.physics.f0.total_cmp(&b.physics.f0));
    report
}

/// Synchronise, read and decode one candidate.
fn try_candidate(
    bb: &Baseband,
    c: &search::Candidate,
    pass: u8,
    settings: &Settings,
    seen: &mut HashSet<u128>,
) -> Option<Found> {
    let synced = search::refine(bb, c);
    if synced.power < settings.min_power {
        return None;
    }
    let (y, at_zero) = bb.around(synced.f0);
    let n0 = (synced.start * DOWN_RATE).round() as i64;
    let hz = synced.f0 - at_zero;
    let z = demod::correlate(&y, n0, hz);
    let costas = demod::costas_matched(&z);
    for block in 1..=3u8 {
        let llr = demod::soft_bits(&z, usize::from(block), settings.llr_scale);
        let Some(decoded) = ldpc::decode(&llr, BP_ITERATIONS, OSD_ORDER) else {
            continue;
        };
        if decoded.osd.is_some() && (costas < OSD_MIN_COSTAS || decoded.repaired > OSD_MAX_REPAIRED)
        {
            continue;
        }
        if !seen.insert(decoded.payload) {
            return None;
        }
        let word91 = crate::signal::ft8::crc::with_crc(decoded.payload);
        let tones = demod::tones_of(word91);
        let mut physics = demod::measure(&y, n0 as f64 / DOWN_RATE, hz, &tones, &z);
        physics.f0 += at_zero;
        return Some(Found {
            payload: decoded.payload,
            start: physics.start,
            physics,
            tones,
            costas,
            block,
            decoded,
            pass,
        });
    }
    None
}

/// Take a decoded transmission out of the cycle.
fn subtract(x: &mut [Complex<f32>], f: &Found) {
    let mut r = vec![Complex::new(0.0f32, 0.0); x.len()];
    let w = Waveform {
        f0: f.physics.f0,
        start: f.start,
        amplitude: 1.0,
        drift: f.physics.drift,
        phase: 0.0,
    };
    modulate::add(&f.tones, &w, RATE, &mut r);
    // g = low-pass of x r*, a centred moving average by prefix sums.
    let n = x.len();
    let half = (GAIN_SECONDS * RATE / 2.0) as usize;
    let mut prefix = vec![Complex::new(0.0f64, 0.0); n + 1];
    let mut weight = vec![0f64; n + 1];
    for i in 0..n {
        let p = x[i] * r[i].conj();
        prefix[i + 1] = prefix[i] + Complex::new(f64::from(p.re), f64::from(p.im));
        weight[i + 1] = weight[i] + f64::from(r[i].norm_sqr());
    }
    for i in 0..n {
        if r[i].norm_sqr() == 0.0 {
            continue;
        }
        let a = i.saturating_sub(half);
        let b = (i + half + 1).min(n);
        let wsum = weight[b] - weight[a];
        if wsum <= 0.0 {
            continue;
        }
        let g = (prefix[b] - prefix[a]) / wsum;
        let gr = g * Complex::new(f64::from(r[i].re), f64::from(r[i].im));
        x[i] -= Complex::new(gr.re as f32, gr.im as f32);
    }
}
