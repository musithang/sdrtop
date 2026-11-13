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
//! subtracted. Fading and phase wander are followed, not assumed away, and
//! how fast is read from the signal itself: a decode whose power one steady
//! carrier does not explain gets a shorter low-pass. The signal here is
//! complex baseband, so the paper's `2 Re[g r]` is simply `g r`.
//!
//! **Passes.** After a pass that decoded something, the cycle is searched
//! again; a pass that decodes nothing new ends it, and so does
//! [`MAX_PASSES`]. A message decoded twice in one cycle (two candidates on one
//! transmission, or the same message found again after subtraction) is
//! reported once.
//!
//! **A pass's candidates are read side by side.** No candidate's reading
//! looks at another's, so they run on up to [`MAX_THREADS`] threads; only
//! which of two candidates of one message is kept depends on order, and that
//! is decided afterwards in the search's order. The decodes are the same on
//! one thread or four (`the_decodes_do_not_depend_on_the_threads`); a
//! crowded cycle of forty took 6.5 s on one thread of the dev machine (two
//! cores, four threads) and 3.3 s on four.

use std::collections::HashSet;
use std::sync::Arc;

use num_complex::Complex;

use super::demod::{self, Physics};
use super::hash::HashBook;
use super::ldpc;
use super::message::{self, Message};
use super::modulate::{self, Waveform, NOMINAL_START, SYMBOLS};
use super::search::{self, Baseband, Spectrogram, DOWN_RATE, RATE};
use super::trace::Trace;

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

/// The fewest Costas symbols on their tone, where a candidate was first
/// read, for it to be tried again at a finer place. Noise matches about 3 of
/// 21 by chance. Swept on simulated cycles of one transmission at -20 to -22
/// dB (20 each): every gate from 0 to 13 decoded the same (18, 11 and 1),
/// the time per cycle on the dev machine falling from 3.0 s to 1.4 s; 9
/// keeps three times noise's count, and the first place is the loose one,
/// so a signal shows fewer there than where it decodes.
pub const RESYNC_MIN_COSTAS: u8 = 9;

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
    /// A candidate that does not decode where the Costas arrays put it is
    /// tried again where every symbol does (`demod::resync`).
    pub resync: bool,
    /// ...but only a candidate with at least this many of its 21 Costas
    /// symbols on their tone where it was first read ([`RESYNC_MIN_COSTAS`]).
    pub resync_min_costas: u8,
    /// The most threads reading one pass's candidates ([`MAX_THREADS`]);
    /// the machine may give fewer. The decodes do not depend on it.
    pub threads: usize,
    /// OSD also on belief propagation's first this many posteriors
    /// ([`OSD_FROM_BP`], [`ldpc::decode_deep`]).
    pub osd_from_bp: usize,
    /// Whether a codeword must pass the guards to be reported: never the
    /// all-zero one, and an OSD one only as [`Settings::osd_guard`] believes
    /// it. Off only to measure what the guards keep out.
    pub guard: bool,
    pub osd_guard: OsdGuard,
}

impl Settings {
    pub const DEFAULT: Settings = Settings {
        llr_scale: demod::LLR_SCALE,
        min_power: MIN_POWER,
        max_passes: MAX_PASSES,
        max_candidates: search::MAX_CANDIDATES,
        resync: true,
        resync_min_costas: RESYNC_MIN_COSTAS,
        threads: MAX_THREADS,
        osd_from_bp: OSD_FROM_BP,
        guard: true,
        osd_guard: OsdGuard::DEFAULT,
    };
}

/// Belief propagation's iteration limit, and the OSD order tried after it.
const BP_ITERATIONS: u32 = 30;
const OSD_ORDER: u8 = 2;

/// How many of belief propagation's posteriors OSD is also run on
/// ([`ldpc::decode_deep`]). Measured: on a Gaussian channel at 1 dB Eb/N0,
/// 493 of 1000 decoded with the received ratios alone, 730 with two
/// posteriors, none wrong; at the OSD distance limit, noise decoded 1 time in
/// 20 000 either way. On simulated cycles at -20 to -22 dB, 117 of 216
/// decoded against 132, none wrong; 400 cycles of noise, no false decode.
/// On a 20 m recording, 241 messages against 248, each of the 7 new ones
/// part of a QSO the band shows. Three and four posteriors decoded no more
/// off the air.
pub const OSD_FROM_BP: usize = 2;

/// When a codeword ordered statistics found is believed.
///
/// OSD's distance bound (`ldpc::OSD_MAX_DISTANCE`) holds on white noise, but
/// a real cycle hands it worse: a real signal, read faded, half drowned, or
/// a tone or a few milliseconds off, gives confident soft bits that are
/// partly wrong, and among the codewords near them one in a few thousand
/// passes the CRC without being what was sent. Every false decode seen on
/// simulated signals was that: at the transmission's own frequency and
/// time, not out of noise. So the codeword is held against what was
/// received, once its tones are known:
///
/// - **What the symbols say against it** ([`Trace::contradiction`]) at most
///   [`Self::max_contradiction`], plus [`Self::contradiction_per_db`] for
///   each dB the decode measured below -19 dB, where the noise in the
///   symbols a true decode differs on is a larger share of the signal.
/// - **Its sync**: [`Self::min_costas`] of the 21 Costas symbols on their
///   tone, and [`Self::costas_per_db`] more for each dB above -20 dB. A
///   strong signal read two tones off contradicts itself very little (its
///   strongest tones moved with it) but cannot show its Costas arrays.
/// - **Bits repaired** at most [`Self::max_repaired`], and **SNR** at least
///   [`Self::min_snr_db`]: below it a true OSD decode was 2 in 15 000.
///
/// Measured with every guard off (`measure_the_guards_population`), then
/// replayed: three populations of 9345, 9345 and 7490 decodes (weak and
/// spaced, fading and steady; and strong and crowded, -10 to -17 dB with
/// pairs within a tone), 9152 + 6538 true, every one of the 1145 false an
/// OSD codeword. The guard this one replaced (11 Costas, 34 repaired)
/// believed 8483 + 6442 true and 16 false; this one 8916 + 6401 true and 2
/// false, both on strong crowded signals and both believed by the old one
/// too. On 1750 simulated WAVs of one transmission each (the other
/// decoder's simulator, -17 to -23 dB, steady and fading), 811 true and 2
/// false became 836 and none, in the same time; 400 cycles of noise, none
/// either way; the crowded band, 149 of 160 and none wrong either way.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OsdGuard {
    pub min_costas: u8,
    pub costas_per_db: f64,
    pub max_repaired: u32,
    pub min_snr_db: f64,
    pub max_contradiction: f32,
    pub contradiction_per_db: f32,
}

impl OsdGuard {
    pub const DEFAULT: OsdGuard = OsdGuard {
        min_costas: 8,
        costas_per_db: 0.5,
        max_repaired: 35,
        min_snr_db: -23.5,
        max_contradiction: 0.18,
        contradiction_per_db: 0.03,
    };

    /// What can be told before the codeword is measured: its sync at the
    /// weakest it may be, and the bits repaired. A codeword refused here is
    /// never measured, which is most of them.
    pub fn may_be(&self, costas: u8, repaired: u32) -> bool {
        costas >= self.min_costas && repaired <= self.max_repaired
    }

    /// Whether the codeword is believed, measured at `snr_db` and
    /// contradicted by `contradiction`.
    pub fn believes(&self, costas: u8, repaired: u32, snr_db: f64, contradiction: f32) -> bool {
        let sync = f64::from(self.min_costas) + self.costas_per_db * (snr_db + 20.0).max(0.0);
        let against =
            self.max_contradiction + self.contradiction_per_db * (-19.0 - snr_db as f32).max(0.0);
        self.may_be(costas, repaired)
            && snr_db >= self.min_snr_db
            && f64::from(costas) >= sync
            && contradiction <= against
    }
}

/// The low-pass that estimates the channel's gain during subtraction: a
/// moving average this many seconds long, on a steady path.
const GAIN_SECONDS: f64 = 0.32;

/// The same on a fading path ([`Physics::fading`]), one symbol: short enough to follow a path
/// fading at 2 Hz. With [`GAIN_SECONDS`] there, a weak signal 20 Hz from a
/// strong one was read beneath it 4 times in 48 against 47 alone, and with
/// this 35; at 1 Hz, 39 and 43 of 46. On a steady path it costs instead: the
/// shorter average is noisier and takes some of its neighbours with it (a
/// crowded band, 141 of 160 against 149).
const FADING_GAIN_SECONDS: f64 = 0.16;

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
    /// What it looked like on the way in.
    pub trace: Arc<Trace>,
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
    /// Power across the band before anything was subtracted, dB over the
    /// cycle's median, in [`BAND_BIN_HZ`] bins from 0 to 3000 Hz.
    pub band: Vec<f32>,
    /// What became of the candidates, pass by pass.
    pub funnel: Vec<Funnel>,
    /// The noise under the band before anything was subtracted, dB per Hz
    /// on the samples' scale ([`super::floor::read`]); `None` on a band too
    /// full to see it.
    pub floor_db_hz: Option<f32>,
}

/// What became of one pass's candidates. Each candidate is counted once, at
/// the furthest it got: `candidates >= synced >= coded`, and every coded
/// candidate is exactly one of the last five.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Funnel {
    /// Candidates the search offered and the pass examined.
    pub candidates: usize,
    /// Of them, strong enough at their fine place to be read.
    pub synced: usize,
    /// Of them, read to a codeword whose check passed.
    pub coded: usize,
    /// Of those, new decodes belief propagation found.
    pub by_bp: usize,
    /// New decodes ordered statistics finished.
    pub by_osd: usize,
    /// A message already decoded this cycle, found again.
    pub duplicates: usize,
    /// The all-zero codeword, never believed.
    pub refused_zero: usize,
    /// An OSD codeword the guard did not believe ([`OsdGuard`]).
    pub refused_osd: usize,
}

/// How far one candidate got, as [`try_candidate`] goes.
#[derive(Default)]
struct Tally {
    synced: bool,
    coded: bool,
    zero: bool,
    refused_osd: bool,
    duplicate: bool,
}

/// The width of a [`Report::band`] bin.
pub const BAND_BIN_HZ: f64 = 25.0;

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
    trace: Arc<Trace>,
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
        if pass == 1 {
            report.band = spec.band(BAND_BIN_HZ, 3000.0);
            report.floor_db_hz = super::floor::read(&x);
        }
        let cands = search::candidates_up_to(&spec, settings.max_candidates);
        report.candidates += cands.len();
        let bb = Baseband::new(&x);
        let mut new: Vec<Found> = Vec::new();
        let mut funnel = Funnel {
            candidates: cands.len(),
            ..Funnel::default()
        };
        // Every candidate read on its own, side by side; then, in the
        // search's order, each first believed payload checked against what
        // was already decoded. A strong transmission raises many candidates
        // around it, and each decodes the same message: only the first, in
        // that order, is kept, exactly as one candidate after another would.
        let read = read_all(&bb, &cands, pass as u8, settings, &seen);
        for (mut tally, hit) in read {
            let found = match hit {
                Some(Hit::Found(f)) if seen.insert(f.payload) => Some(*f),
                Some(_) => {
                    tally.duplicate = true;
                    None
                }
                None => None,
            };
            funnel.synced += usize::from(tally.synced);
            funnel.coded += usize::from(tally.coded);
            match &found {
                Some(f) if f.decoded.osd.is_some() => funnel.by_osd += 1,
                Some(_) => funnel.by_bp += 1,
                None if tally.duplicate => funnel.duplicates += 1,
                None if tally.zero => funnel.refused_zero += 1,
                None if tally.refused_osd => funnel.refused_osd += 1,
                None => {}
            }
            if let Some(f) = found {
                new.push(f);
            }
        }
        report.funnel.push(funnel);
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
                trace: f.trace,
            }),
            Err(_) => report.malformed += 1,
        }
    }
    report
        .decodes
        .sort_by(|a, b| a.physics.f0.total_cmp(&b.physics.f0));
    report
}

/// Threads that read one pass's candidates: the machine's own, at most
/// this many. A decode is a few seconds of work every fifteen, and the radio's
/// own threads run beside it.
const MAX_THREADS: usize = 4;

/// What reading a candidate came to: its first believed codeword, measured,
/// or only its payload where an earlier pass already decoded it (nothing
/// left to measure: it is a duplicate whatever the order).
enum Hit {
    Found(Box<Found>),
    Before,
}

/// One candidate's reading: how far it got, and what it came to.
type Reading = (Tally, Option<Hit>);

/// Every candidate read, in the search's order, each with its tally. Read
/// side by side on [`Settings::threads`] threads at most; the result does
/// not depend on how many, since no candidate's reading looks at another's.
fn read_all(
    bb: &Baseband,
    cands: &[search::Candidate],
    pass: u8,
    settings: &Settings,
    before: &HashSet<u128>,
) -> Vec<Reading> {
    let one = |c: &search::Candidate| {
        let mut tally = Tally::default();
        let hit = try_candidate(bb, c, pass, settings, before, &mut tally);
        (tally, hit)
    };
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(settings.threads)
        .min(cands.len());
    if threads <= 1 {
        return cands.iter().map(one).collect();
    }
    // Each thread takes the next candidate not yet taken, so a slow one
    // (resync, data-aided, OSD) does not hold the others back; the results
    // go back in the search's order.
    let next = std::sync::atomic::AtomicUsize::new(0);
    let parts: Vec<Vec<(usize, Reading)>> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut mine = Vec::new();
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(c) = cands.get(i) else {
                            break;
                        };
                        mine.push((i, one(c)));
                    }
                    mine
                })
            })
            .collect();
        workers
            .into_iter()
            .map(|w| w.join().expect("a candidate's reading does not panic"))
            .collect()
    });
    let mut all: Vec<(usize, Reading)> = parts.into_iter().flatten().collect();
    all.sort_by_key(|(i, _)| *i);
    all.into_iter().map(|(_, r)| r).collect()
}

/// Synchronise, read and decode one candidate, up to its first believed
/// codeword. Whether that codeword is new this cycle is the caller's
/// question, asked in the search's order; `before` only spares measuring one
/// an earlier pass already decoded.
fn try_candidate(
    bb: &Baseband,
    c: &search::Candidate,
    pass: u8,
    settings: &Settings,
    before: &HashSet<u128>,
    tally: &mut Tally,
) -> Option<Hit> {
    let synced = search::refine(bb, c);
    if synced.power < settings.min_power {
        return None;
    }
    tally.synced = true;
    let (y, at_zero) = bb.around(synced.f0);
    let first = (synced.start * DOWN_RATE, synced.f0 - at_zero);
    // The Costas arrays' place first; failing that, the place every symbol
    // agrees on (`demod::resync`); failing that, the place the tones chosen
    // there fit best (`demod::data_aided`). Most of a weak signal's decodes
    // were lost to a place a few milliseconds and a fraction of a hertz off.
    let attempts = if settings.resync { 3 } else { 1 };
    let mut at = first;
    for attempt in 0..attempts {
        if attempt == 1 {
            let z = demod::correlate(&y, first.0.round() as i64, first.1);
            if demod::costas_matched(&z) < settings.resync_min_costas {
                return None;
            }
            let (start, hz) = demod::resync(&y, first.0, first.1);
            at = (start * DOWN_RATE, hz);
        }
        let (n0, hz) = at;
        let n0 = n0.round() as i64;
        let mut z = demod::correlate(&y, n0, hz);
        if attempt == 2 {
            let (start, hz2) = demod::data_aided(&y, n0, hz, &z);
            at = (start * DOWN_RATE, hz2);
            z = demod::correlate(&y, at.0.round() as i64, hz2);
        }
        let (n0, hz) = (at.0.round() as i64, at.1);
        let costas = demod::costas_matched(&z);
        for block in 1..=3u8 {
            let llr = demod::soft_bits(&z, usize::from(block), settings.llr_scale);
            let Some(decoded) =
                ldpc::decode_deep(&llr, BP_ITERATIONS, OSD_ORDER, settings.osd_from_bp)
            else {
                continue;
            };
            tally.coded = true;
            let guarded_osd = settings.guard && decoded.osd.is_some();
            if settings.guard && decoded.payload == 0 {
                tally.zero = true;
                continue;
            }
            if guarded_osd && !settings.osd_guard.may_be(costas, decoded.repaired) {
                tally.refused_osd = true;
                continue;
            }
            if before.contains(&decoded.payload) {
                return Some(Hit::Before);
            }
            let word91 = crate::signal::ft8::crc::with_crc(decoded.payload);
            let tones = demod::tones_of(word91);
            // Read at the Costas arrays' place, the codeword may have come
            // from a rough one (OSD decodes there too): measured there and
            // where every symbol agrees, the stronger fit kept. The later
            // attempts' places are that already, or finer.
            let measured = if attempt == 0 {
                demod::measured_from(&y, n0 as f64 / DOWN_RATE, hz, &tones, &z)
            } else {
                demod::measured(&y, n0 as f64 / DOWN_RATE, hz, &tones, &z)
            };
            let codeword = ldpc::encode(word91);
            let trace = Trace::build(&y, &measured, &tones, &llr, &codeword, at_zero);
            if guarded_osd
                && !settings.osd_guard.believes(
                    costas,
                    decoded.repaired,
                    measured.physics.snr_db,
                    trace.contradiction(),
                )
            {
                tally.refused_osd = true;
                continue;
            }
            let mut physics = measured.physics;
            physics.f0 += at_zero;
            return Some(Hit::Found(Box::new(Found {
                payload: decoded.payload,
                start: physics.start,
                physics,
                tones,
                costas,
                block,
                decoded,
                pass,
                trace: Arc::new(trace),
            })));
        }
    }
    None
}

// Never the all-zero codeword, in `try_candidate`. Its check is zero too, so
// it passes both, and anything that puts every data symbol on tone 0
// produces it: off the air on 20 m, a steady carrier under a neighbour's
// sidelobe "decoded" as an empty message cycle after cycle (2 or 3 Costas
// symbols right, not one bit repaired). It is thirteen blanks of free text,
// which nobody sends.

#[cfg(test)]
#[test]
fn the_osd_guard_holds_each_of_its_lines() {
    let g = OsdGuard::DEFAULT;
    // Weak: the sync floor, the repairs, the SNR floor.
    assert!(g.believes(8, 35, -21.0, 0.1));
    assert!(!g.believes(7, 35, -21.0, 0.1));
    assert!(!g.believes(8, 36, -21.0, 0.1));
    assert!(!g.believes(21, 20, -23.6, 0.1));
    // The contradiction allowed grows below -19 dB: 0.18, 0.24 at -21.
    assert!(g.believes(15, 20, -19.0, 0.18));
    assert!(!g.believes(15, 20, -19.0, 0.19));
    assert!(g.believes(15, 20, -21.0, 0.235));
    assert!(!g.believes(15, 20, -21.0, 0.245));
    // The sync asked for grows above -20 dB: 8, 10.5 at -15. The crowded
    // band's false decode: -14.8 dB, Costas 8, contradiction 0.08.
    assert!(!g.believes(8, 35, -14.8, 0.08));
    assert!(g.believes(11, 35, -15.0, 0.08));
    assert!(!g.believes(10, 35, -15.0, 0.08));
    // Before measuring, only the floor and the repairs.
    assert!(g.may_be(8, 35));
    assert!(!g.may_be(7, 35));
    assert!(!g.may_be(8, 36));
}

/// What one candidate came to, read as a pass reads it: whether it passed
/// the power floor, whether the code gave a codeword, whether a guard
/// refused one, and the codeword believed (payload, Costas symbols on their
/// tone, OSD order, bits repaired). For finding out where a decode is lost.
#[cfg(test)]
pub(super) struct Explained {
    pub synced: bool,
    pub coded: bool,
    pub refused_zero: bool,
    pub refused_osd: bool,
    pub believed: Option<(u128, u8, Option<u8>, u32)>,
}

#[cfg(test)]
pub(super) fn explain(bb: &Baseband, c: &search::Candidate, settings: &Settings) -> Explained {
    let mut tally = Tally::default();
    let hit = try_candidate(bb, c, 1, settings, &HashSet::new(), &mut tally);
    Explained {
        synced: tally.synced,
        coded: tally.coded,
        refused_zero: tally.zero,
        refused_osd: tally.refused_osd,
        believed: match hit {
            Some(Hit::Found(f)) => Some((f.payload, f.costas, f.decoded.osd, f.decoded.repaired)),
            _ => None,
        },
    }
}

/// Take a reported decode out of `x`, as its pass did.
#[cfg(test)]
pub(super) fn take_out_decode(x: &mut [Complex<f32>], d: &Decode) {
    let w = Waveform {
        f0: d.physics.f0,
        start: d.physics.start,
        amplitude: 1.0,
        drift: d.physics.drift,
        phase: 0.0,
    };
    let seconds = if d.physics.fading() {
        FADING_GAIN_SECONDS
    } else {
        GAIN_SECONDS
    };
    let tones = demod::tones_of(crate::signal::ft8::crc::with_crc(d.payload));
    take_out(x, &tones, &w, seconds);
}

/// Take a decoded transmission out of the cycle.
fn subtract(x: &mut [Complex<f32>], f: &Found) {
    let w = Waveform {
        f0: f.physics.f0,
        start: f.start,
        amplitude: 1.0,
        drift: f.physics.drift,
        phase: 0.0,
    };
    let seconds = if f.physics.fading() {
        FADING_GAIN_SECONDS
    } else {
        GAIN_SECONDS
    };
    take_out(x, &f.tones, &w, seconds);
}

/// Take the transmission `tones` sent as `w` (unit amplitude) out of `x`, its
/// gain and phase followed by a centred average over `seconds`. Only the
/// transmission's own samples are touched or summed: outside them the
/// reference is exactly zero, every product with it a zero that leaves a
/// running sum as it was, so sums over the transmission alone are the sums
/// over the whole cycle.
fn take_out(x: &mut [Complex<f32>], tones: &[u8; SYMBOLS], w: &Waveform, seconds: f64) {
    let Some((first, last)) = modulate::span(w, RATE, x.len()) else {
        return;
    };
    let len = last + 1 - first;
    let mut r = vec![Complex::new(0.0f32, 0.0); len];
    modulate::add_at(tones, w, RATE, &mut r, first, x.len());
    let x = &mut x[first..=last];
    // g = low-pass of x r*, a centred moving average by prefix sums.
    let half = (seconds * RATE / 2.0) as usize;
    let mut prefix = vec![Complex::new(0.0f64, 0.0); len + 1];
    let mut weight = vec![0f64; len + 1];
    for i in 0..len {
        let p = x[i] * r[i].conj();
        prefix[i + 1] = prefix[i] + Complex::new(f64::from(p.re), f64::from(p.im));
        weight[i + 1] = weight[i] + f64::from(r[i].norm_sqr());
    }
    for i in 0..len {
        if r[i].norm_sqr() == 0.0 {
            continue;
        }
        let a = i.saturating_sub(half);
        let b = (i + half + 1).min(len);
        let wsum = weight[b] - weight[a];
        if wsum <= 0.0 {
            continue;
        }
        let g = (prefix[b] - prefix[a]) / wsum;
        let gr = g * Complex::new(f64::from(r[i].re), f64::from(r[i].im));
        x[i] -= Complex::new(gr.re as f32, gr.im as f32);
    }
}

/// The subtraction as first written: the reference and both running sums
/// over the whole cycle. [`take_out`] must agree with it bit for bit.
#[cfg(test)]
fn take_out_reference(x: &mut [Complex<f32>], tones: &[u8; SYMBOLS], w: &Waveform, seconds: f64) {
    let mut r = vec![Complex::new(0.0f32, 0.0); x.len()];
    modulate::add(tones, w, RATE, &mut r);
    let n = x.len();
    let half = (seconds * RATE / 2.0) as usize;
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

/// Over the transmission alone or over the whole cycle, the subtraction
/// leaves the same samples, bit for bit: transmissions early, late, cut by
/// either end of the buffer, drifting, below and above the band's centre,
/// in noise and with a second signal under them, with both averaging times.
#[cfg(test)]
#[test]
fn the_subtraction_over_its_span_is_the_whole_cycles_bit_for_bit() {
    use crate::signal::dsp::testkit::Rng;
    let mut rng = Rng::new(11);
    let n = (15.0 * RATE) as usize;
    let mut tried = 0;
    for (k, &start) in [0.5, 2.36, -1.2, 3.1, 0.0, 14.0, 16.0].iter().enumerate() {
        for &seconds in &[GAIN_SECONDS, FADING_GAIN_SECONDS] {
            let tones: [u8; SYMBOLS] = std::array::from_fn(|_| (rng.next_u64() % 8) as u8);
            let w = Waveform {
                f0: -900.0 + 330.0 * k as f64,
                start,
                amplitude: 1.0,
                drift: 0.4 * k as f64 - 1.0,
                phase: 0.0,
            };
            let mut x: Vec<Complex<f32>> = (0..n)
                .map(|_| {
                    let (a, b) = rng.normal_pair();
                    Complex::new(a as f32, b as f32)
                })
                .collect();
            let sent = Waveform {
                amplitude: 3.0,
                phase: 1.1,
                f0: w.f0 + 0.3,
                ..w
            };
            modulate::add(&tones, &sent, RATE, &mut x);
            let under = Waveform {
                f0: w.f0 + 20.0,
                start: 0.7,
                amplitude: 2.0,
                drift: 0.0,
                phase: 0.0,
            };
            modulate::add(&[3; SYMBOLS], &under, RATE, &mut x);
            let mut want = x.clone();
            take_out_reference(&mut want, &tones, &w, seconds);
            let before = x.clone();
            take_out(&mut x, &tones, &w, seconds);
            let same = x
                .iter()
                .zip(&want)
                .all(|(a, b)| a.re.to_bits() == b.re.to_bits() && a.im.to_bits() == b.im.to_bits());
            assert!(same, "start {start}, {seconds} s");
            tried += usize::from(x != before);
        }
    }
    // All but the transmission past the buffer's end took something out.
    assert_eq!(tried, 12);
}
