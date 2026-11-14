// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The FT8 worker: the radio's blocks to 15 s cycles on the UTC quarter
//! minute, each handed to a decoder of its own.
//!
//! Two threads, because the work comes in two shapes. The channel runs on
//! every block, a little at a time, and must never fall behind the stream;
//! the decode is nothing for fifteen seconds and then a few seconds of work at
//! once. So this thread keeps the channel and the time and assembles cycles,
//! and a second one decodes them. When a cycle is ready while the decoder is
//! still on the one before, the new one is not queued behind it (that only
//! moves the lag): it is published as skipped, and said.
//!
//! **Nothing is carried across a break.** A block that does not continue the
//! stream (a refused block, a driver's drop, a new stream) or that was taken
//! at another tuning ends the cycle being assembled: it is published as cut,
//! not decoded and not called empty, and the channel and the clock start
//! again.
//!
//! **Time.** Each channel sample's UTC comes from the stream clock
//! ([`super::clock`]): the stream position of the block it came from, the
//! clock's fit of position to UTC, and the channel's own delay.
//!
//! One lock block a block, and every float outside it.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crossbeam_channel::{Receiver, Sender, TrySendError};
use num_complex::Complex;

use super::channel::{Channel, RATE_HZ};
use super::clock::{Lateness, StreamClock};
use super::cycle::{self, Settings};
use super::hash::HashBook;
use crate::hardware::{SampleGeometry, TimedBlock};
use crate::signal::stream::plan_block;
use crate::state::ft8::Setting;
use crate::state::{CycleEntry, CycleOutcome, SdrMetrics};

/// How much one block moves the load reading.
const LOAD_ALPHA: f64 = 0.05;

/// Samples in a cycle at FT8's rate.
const CYCLE_SAMPLES: usize = 15 * RATE_HZ as usize;

/// Samples kept before the next cycle's start: half a second, far more
/// than the stream clock's estimate moves when it settles (milliseconds).
const KEEP_BEFORE: usize = RATE_HZ as usize / 2;

/// Blocks the feed holds for the worker.
///
/// Deep enough to ride out a stall of the whole machine, in blocks because
/// that is what a queue counts: the HF+ hands 4096 pairs at a time, 5 ms at
/// 768 kS/s, and at the four blocks the other feeds keep, 21 ms of slack was
/// all there was. Recording to an encrypted disk stalled the worker for
/// about 30 ms a few times a minute (the writeback's bursts), and each
/// stall refused a few blocks and cut a cycle: 455 in 22 minutes off the
/// air, none in 15 minutes without the recording. Sixty-four blocks are a
/// third of a second there. Nothing here needs a block soon: a cycle's time
/// comes from when its blocks arrived, stamped before the queue, and a
/// block the queue does refuse is counted as before.
pub const FEED_BLOCKS: usize = 64;

/// The clock verdict looks at this many of the latest decoded cycles.
const VERDICT_CYCLES: usize = 4;

/// One cycle to decode.
struct Job {
    start_unix: f64,
    dial_hz: u64,
    samples: Vec<Complex<f32>>,
    setting: Option<Setting>,
    setting_changed: bool,
    held: bool,
    lateness: Option<Lateness>,
}

pub struct Ft8Worker {
    rx: Receiver<TimedBlock>,
    state: Arc<Mutex<SdrMetrics>>,
    geometry: SampleGeometry,
    /// The decoder's settings: [`Settings::DEFAULT`] on the air.
    pub settings: Settings,
}

impl Ft8Worker {
    pub fn new(
        rx: Receiver<TimedBlock>,
        state: Arc<Mutex<SdrMetrics>>,
        geometry: SampleGeometry,
    ) -> Self {
        Self {
            rx,
            state,
            geometry,
            settings: Settings::DEFAULT,
        }
    }

    /// Until the feed's sender is gone; the decoder finishes what it has.
    pub fn run(self) {
        let (job_tx, job_rx) = crossbeam_channel::bounded::<Job>(1);
        let decoder_state = Arc::clone(&self.state);
        let settings = self.settings;
        let decoder = std::thread::Builder::new()
            .name("ft8-decoder".into())
            .spawn(move || decode_loop(job_rx, decoder_state, settings))
            .ok();
        let mut session = Session::new(self.geometry, job_tx);
        while let Ok(block) = self.rx.recv() {
            session.block(&block, &self.state);
        }
        drop(session);
        if let Some(d) = decoder {
            let _ = d.join();
        }
    }
}

/// The decoder's thread: one cycle at a time, the hash book carried.
fn decode_loop(rx: Receiver<Job>, state: Arc<Mutex<SdrMetrics>>, settings: Settings) {
    let mut book = HashBook::new();
    let mut recent: VecDeque<Vec<f64>> = VecDeque::new();
    while let Ok(job) = rx.recv() {
        // Your own call answers to its hashes too (the paper: a hash is
        // shown as "mycall" when it maps to it), so a call to you sent as
        // `<HA5ZZT>` is still yours. Read each cycle: it can change.
        let my_call = {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            m.ft8.decoding = true;
            m.home.callsign.clone()
        };
        if let Some(call) = &my_call {
            book.learn(call);
        }
        let t = Instant::now();
        let report = cycle::decode_with(&job.samples, &mut book, &settings);
        let seconds = t.elapsed().as_secs_f64();
        recent.push_back(report.decodes.iter().map(|d| d.dt).collect());
        while recent.len() > VERDICT_CYCLES {
            recent.pop_front();
        }
        let dts: Vec<f64> = recent.iter().flatten().copied().collect();
        let clock = crate::state::ft8_verdict(&dts);
        let entry = CycleEntry {
            start_unix: job.start_unix,
            dial_hz: job.dial_hz,
            outcome: CycleOutcome::Decoded,
            decodes: report.decodes.len(),
            candidates: report.candidates,
            malformed: report.malformed,
            seconds,
            band: report.band,
            clock: None,
            funnel: report.funnel,
            // A level is comparable with another cycle's only at one setting
            // and below the radio's hold.
            floor_db_hz: report
                .floor_db_hz
                .filter(|_| !job.held && !job.setting_changed),
            setting: job.setting,
            setting_changed: job.setting_changed,
            held: job.held,
            lateness: job.lateness,
        };
        let now = Instant::now();
        let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
        m.ft8.decoding = false;
        m.ft8
            .push_cycle(entry, report.decodes, clock, now, my_call.as_deref());
    }
}

/// A cycle that was not decoded, for the record.
fn undecoded(start_unix: f64, dial_hz: u64, outcome: CycleOutcome) -> CycleEntry {
    CycleEntry {
        start_unix,
        dial_hz,
        outcome,
        decodes: 0,
        candidates: 0,
        malformed: 0,
        seconds: 0.0,
        band: Vec::new(),
        clock: None,
        funnel: Vec::new(),
        floor_db_hz: None,
        setting: None,
        setting_changed: false,
        held: false,
        lateness: None,
    }
}

/// What is carried from one block to the next.
struct Session {
    geometry: SampleGeometry,
    jobs: Sender<Job>,
    last_seq: u64,
    drop_ref: Option<u64>,
    next_pair: Option<u64>,
    tuning: Option<(u64, f64)>,
    channel: Option<Channel>,
    clock: StreamClock,
    /// The stream position the channel's first sample came from.
    pair0: Option<u64>,
    /// Channel samples kept, and the index (since `pair0`) of the first.
    buf: Vec<Complex<f32>>,
    buf_base: usize,
    /// The cycle being assembled: its start, Unix seconds.
    next_cycle: Option<f64>,
    iq: Vec<Complex<f32>>,
    load: Option<f64>,
    /// Per block: the channel samples it became (from, to, counted as
    /// `buf_base` is) and its sample peak, the larger of |I| and |Q| over
    /// full scale, as the ADC bench measures it.
    peaks: VecDeque<(usize, usize, f32)>,
    /// Where (as `buf_base` counts) the radio's setting was seen to change.
    changes: VecDeque<usize>,
    /// The setting as last read.
    setting: Option<Setting>,
    /// The radio's stated ceiling where it is tuned, dBFS, when it has one.
    ceiling_dbfs: Option<f64>,
}

impl Session {
    fn new(geometry: SampleGeometry, jobs: Sender<Job>) -> Self {
        Self {
            geometry,
            jobs,
            last_seq: 0,
            drop_ref: None,
            next_pair: None,
            tuning: None,
            channel: None,
            clock: StreamClock::default(),
            pair0: None,
            buf: Vec::new(),
            buf_base: 0,
            next_cycle: None,
            iq: Vec::new(),
            load: None,
            peaks: VecDeque::new(),
            changes: VecDeque::new(),
            setting: None,
            ceiling_dbfs: None,
        }
    }

    /// UTC of channel sample `k` (counted from `pair0`).
    fn unix_of_sample(&self, k: f64) -> Option<f64> {
        let ch = self.channel.as_ref()?;
        let p0 = self.pair0? as f64;
        Some(self.clock.unix_of(p0)? + k / RATE_HZ as f64 - ch.delay())
    }

    /// Start again: a break, a retune, a new rate. The cycle in assembly, if
    /// the stream had reached into it, is cut.
    fn restart(&mut self, rate_hz: f64) -> Option<f64> {
        let cut = self.next_cycle.filter(|&start| {
            let have_end = self.unix_of_sample((self.buf_base + self.buf.len()) as f64);
            have_end.is_some_and(|end| end > start)
        });
        self.channel = Channel::new(rate_hz);
        self.clock.reset(rate_hz);
        self.pair0 = None;
        self.buf.clear();
        self.buf_base = 0;
        self.next_cycle = None;
        self.peaks.clear();
        self.changes.clear();
        // The first reading after a restart is not a change.
        self.setting = None;
        cut
    }

    fn block(&mut self, tb: &TimedBlock, state: &Mutex<SdrMetrics>) {
        let b = &tb.block;
        let started = self.drop_ref.is_some();
        let plan = plan_block(b.seq, b.gap_before, self.last_seq, self.drop_ref);
        self.last_seq = b.seq;
        self.drop_ref = Some(b.seq);
        let broke = started && !plan.contiguous;

        crate::signal::demod::decode(&b.bytes, self.geometry, usize::MAX, &mut self.iq);
        let pairs = self.iq.len() as u64;
        let continuous = self.next_pair == Some(b.first_pair);
        self.next_pair = Some(b.first_pair + pairs);
        let tuning = (b.centre_hz, b.rate_hz);
        let retuned = self.tuning.is_some_and(|t| t != tuning);
        let dial_before = self.tuning.map_or(b.centre_hz, |t| t.0);
        self.tuning = Some(tuning);

        let mut cut = None;
        if !continuous || retuned || self.channel.is_none() {
            cut = self.restart(b.rate_hz);
        }

        let t0 = Instant::now();
        let mut ready: Vec<Job> = Vec::new();
        let mut lost: Vec<f64> = Vec::new();
        if let Some(ch) = &mut self.channel {
            self.clock.push(b.first_pair, pairs, tb.arrived_unix);
            if self.pair0.is_none() {
                self.pair0 = Some(b.first_pair);
            }
            let from = self.buf_base + self.buf.len();
            ch.push(&self.iq, &mut self.buf);
            let peak = self
                .iq
                .iter()
                .map(|z| z.re.abs().max(z.im.abs()))
                .fold(0f32, f32::max);
            self.peaks
                .push_back((from, self.buf_base + self.buf.len(), peak));
            (ready, lost) = self.take_cycles(b.centre_hz);
        }
        let spent = t0.elapsed().as_secs_f64();
        let heard_until = self.unix_of_sample((self.buf_base + self.buf.len()) as f64);
        if b.rate_hz > 0.0 && pairs > 0 {
            let share = spent / (pairs as f64 / b.rate_hz);
            self.load = Some(match self.load {
                Some(l) => l + LOAD_ALPHA * (share - l),
                None => share,
            });
        }

        let mut skipped: Vec<CycleEntry> = Vec::new();
        for job in ready {
            let (start, dial) = (job.start_unix, job.dial_hz);
            match self.jobs.try_send(job) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                    skipped.push(undecoded(start, dial, CycleOutcome::Skipped));
                }
            }
        }

        let now = Instant::now();
        let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
        let f = &mut m.ft8;
        f.feed.blocks_in = f.feed.blocks_in.saturating_add(1);
        f.feed.gaps = f.feed.gaps.saturating_add(u64::from(broke));
        f.feed.blocks_lost = f.feed.blocks_lost.saturating_add(plan.dropped);
        f.feed.last_block = Some(now);
        if broke || plan.dropped > 0 {
            f.feed.last_loss = Some(now);
        }
        f.feed.load = self.load;
        f.unreachable_rate = self.channel.is_none().then_some(b.rate_hz);
        self.ceiling_dbfs = m.caps.gain.ceiling().map(|c| c.dbfs_at(b.centre_hz));
        let still = self
            .setting
            .as_ref()
            .is_some_and(|s| s.matches(&m.radio, b.centre_hz, b.rate_hz));
        if !still {
            if self.setting.is_some() {
                self.changes.push_back(self.buf_base + self.buf.len());
            }
            self.setting = Some(Setting::of(&m.radio, b.centre_hz, b.rate_hz));
        }
        let f = &mut m.ft8;
        f.hearing = self.next_cycle;
        f.heard_until = heard_until;
        if let Some(start) = cut {
            f.push_cycle(
                undecoded(start, dial_before, CycleOutcome::Cut),
                Vec::new(),
                None,
                now,
                None,
            );
        }
        for start in lost {
            f.push_cycle(
                undecoded(start, b.centre_hz, CycleOutcome::Cut),
                Vec::new(),
                None,
                now,
                None,
            );
        }
        for s in skipped {
            f.push_cycle(s, Vec::new(), None, now, None);
        }
    }

    /// Every cycle the buffer now holds whole, the starts of any it can no
    /// longer assemble (to be published as cut), and the buffer trimmed to
    /// [`KEEP_BEFORE`] ahead of the next cycle's start.
    fn take_cycles(&mut self, dial_hz: u64) -> (Vec<Job>, Vec<f64>) {
        let mut out = Vec::new();
        let mut lost = Vec::new();
        let Some(t0) = self.unix_of_sample(0.0) else {
            return (out, lost);
        };
        let rate = RATE_HZ as f64;
        if self.next_cycle.is_none() {
            let first = self.unix_of_sample(self.buf_base as f64).unwrap_or(t0);
            self.next_cycle = Some((first / 15.0).ceil() * 15.0);
        }
        while let Some(start) = self.next_cycle {
            let i = ((start - t0) * rate).round();
            if i < self.buf_base as f64 {
                // The clock moved the boundary further back than the reserve
                // kept: this cycle cannot be assembled whole. Said, never
                // skipped in silence.
                lost.push(start);
                self.next_cycle = Some(start + 15.0);
                continue;
            }
            let i = i as usize;
            let end = i + CYCLE_SAMPLES;
            if end > self.buf_base + self.buf.len() {
                // Not whole yet: keep from a little before its start, so the
                // clock's estimate settling by some milliseconds does not
                // move the boundary out of what is kept.
                let keep_from = i.saturating_sub(KEEP_BEFORE).max(self.buf_base);
                let drop = (keep_from - self.buf_base).min(self.buf.len());
                self.buf.drain(..drop);
                self.buf_base += drop;
                let base = self.buf_base;
                while self.peaks.front().is_some_and(|&(_, b, _)| b <= base) {
                    self.peaks.pop_front();
                }
                while self.changes.front().is_some_and(|&c| c < base) {
                    self.changes.pop_front();
                }
                break;
            }
            let from = i - self.buf_base;
            let held = self.ceiling_dbfs.is_some_and(|ceiling| {
                self.peaks
                    .iter()
                    .filter(|&&(a, b, _)| a < end && b > i)
                    .any(|&(_, _, p)| p > 0.0 && 20.0 * f64::from(p).log10() >= ceiling)
            });
            out.push(Job {
                start_unix: start,
                dial_hz,
                samples: self.buf[from..from + CYCLE_SAMPLES].to_vec(),
                setting: self.setting.clone(),
                setting_changed: self.changes.iter().any(|&c| c > i && c <= end),
                held,
                lateness: self.clock.lateness(),
            });
            self.next_cycle = Some(start + 15.0);
        }
        (out, lost)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::{SampleFormat, StreamBlock};
    use crate::signal::dsp::testkit::Rng;
    use crate::signal::ft8::fields::G15;
    use crate::signal::ft8::message::{pack::pack, Call, Message};
    use crate::signal::ft8::modulate::{self, Waveform};
    use crate::signal::ft8::{crc, ldpc};

    const RADIO: f64 = 192_000.0;
    const BLOCK: usize = 19_200;
    /// A quarter minute in the future of any test machine, on an even cycle.
    const EPOCH: f64 = 1_800_000_000.0;

    fn geometry() -> SampleGeometry {
        SampleGeometry {
            format: SampleFormat::Int16,
            full_scale: 32768.0,
            converter_bits: None,
        }
    }

    /// The radio's stream: `seconds` of noise starting at `start_unix`, with
    /// an FT8 transmission of `message` at `f0` whose first symbol starts at
    /// `tx_unix`.
    fn stream(
        start_unix: f64,
        seconds: f64,
        f0: f64,
        tx_unix: f64,
        message: &Message,
    ) -> Vec<Complex<f32>> {
        let n = (seconds * RADIO) as usize;
        let mut rng = Rng::new(5);
        let mut x = rng.noise(n, 0.01);
        let tones = modulate::channel_symbols(&ldpc::encode(crc::with_crc(pack(message))));
        let w = Waveform {
            f0,
            start: tx_unix - start_unix,
            amplitude: 0.05,
            drift: 0.0,
            phase: 0.4,
        };
        modulate::add(&tones, &w, RADIO, &mut x);
        x
    }

    fn bytes(iq: &[Complex<f32>]) -> Vec<u8> {
        iq.iter()
            .flat_map(|z| {
                let q =
                    |v: f32| ((v * 32767.0).round().clamp(-32767.0, 32767.0) as i16).to_le_bytes();
                let (a, b) = (q(z.re), q(z.im));
                [a[0], a[1], b[0], b[1]]
            })
            .collect()
    }

    /// Blocks as the intake would deliver them, each arriving `late` seconds
    /// after its last sample was taken. `skip` leaves a hole of one block
    /// after that many.
    fn deliver(
        x: &[Complex<f32>],
        start_unix: f64,
        late: f64,
        skip: Option<usize>,
    ) -> Vec<TimedBlock> {
        // A block the intake refused still took its sequence number: the
        // gap in the numbers is how the worker learns it.
        let mut out = Vec::new();
        for (k, chunk) in x.chunks(BLOCK).enumerate() {
            if Some(k) == skip {
                continue;
            }
            let seq = k as u64;
            let first_pair = (k * BLOCK) as u64;
            let end = (first_pair as usize + chunk.len()) as f64 / RADIO;
            out.push(TimedBlock {
                block: StreamBlock {
                    seq,
                    gap_before: false,
                    bytes: bytes(chunk),
                    first_pair,
                    centre_hz: 14_074_000,
                    rate_hz: RADIO,
                },
                arrived_unix: start_unix + end + late,
            });
        }
        out
    }

    fn run(blocks: Vec<TimedBlock>) -> crate::state::Ft8State {
        run_with(SdrMetrics::fixture(), blocks)
    }

    fn run_with(m: SdrMetrics, blocks: Vec<TimedBlock>) -> crate::state::Ft8State {
        let state = Arc::new(Mutex::new(m));
        let (tx, rx) = crossbeam_channel::bounded(blocks.len());
        for b in blocks {
            tx.send(b).unwrap();
        }
        drop(tx);
        let mut w = Ft8Worker::new(rx, Arc::clone(&state), geometry());
        w.settings = Settings {
            max_candidates: 40,
            ..Settings::DEFAULT
        };
        w.run();
        let m = state.lock().unwrap();
        m.ft8.clone()
    }

    /// A recording handed to the worker at the radio's own pace through a
    /// bounded queue, as the intake hands it live: how many blocks the queue
    /// refused and what became of the cycles. The decoder's threads and the
    /// channel compete for the machine here as they do in the app.
    ///
    /// `FT8_STALL_MS` holds the worker up for that long every five seconds,
    /// as a busy machine does (the state's lock held, which the worker
    /// takes once a block).
    ///
    /// `FT8_SIGMF=<rec>.sigmf-meta FT8_QUEUE=4 FT8_SECONDS=120 FT8_BLOCK=4096
    /// FT8_STALL_MS=35 cargo test --release the_feed_in_real_time --
    /// --ignored --nocapture`
    #[test]
    #[ignore]
    fn the_feed_in_real_time() {
        use std::io::Read;
        let Ok(meta) = std::env::var("FT8_SIGMF") else {
            return;
        };
        let var = |k: &str, d: usize| std::env::var(k).map_or(d, |v| v.parse().unwrap());
        let (depth, seconds, pairs) = (
            var("FT8_QUEUE", FEED_BLOCKS),
            var("FT8_SECONDS", 120),
            var("FT8_BLOCK", 4096),
        );
        let m: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&meta).unwrap()).unwrap();
        let g = &m["global"];
        assert_eq!(g["core:datatype"], "ci32_le", "an HF+ recording");
        let rate = g["core:sample_rate"].as_f64().unwrap();
        let geometry = SampleGeometry {
            format: SampleFormat::Int32,
            full_scale: g["sdrtop:full_scale"].as_f64().unwrap() as f32,
            converter_bits: None,
        };
        let mut file = std::fs::File::open(meta.replace(".sigmf-meta", ".sigmf-data")).unwrap();
        let state = Arc::new(Mutex::new(SdrMetrics::fixture()));
        let (tx, rx) = crossbeam_channel::bounded(depth);
        let worker = Ft8Worker::new(rx, Arc::clone(&state), geometry);
        let run = std::thread::spawn(move || worker.run());
        let stall = var("FT8_STALL_MS", 0) as u64;
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let staller = (stall > 0).then(|| {
            let (state, done) = (Arc::clone(&state), Arc::clone(&done));
            std::thread::spawn(move || {
                while !done.load(std::sync::atomic::Ordering::Relaxed) {
                    std::thread::sleep(std::time::Duration::from_secs(5));
                    let held = state.lock().unwrap();
                    std::thread::sleep(std::time::Duration::from_millis(stall));
                    drop(held);
                }
            })
        });
        // `FT8_UI=1`: a thread that takes the state as the screen does, a
        // whole clone under the lock every 33 ms, and times the lock held.
        let ui = std::env::var("FT8_UI").is_ok().then(|| {
            let (state, done) = (Arc::clone(&state), Arc::clone(&done));
            std::thread::spawn(move || {
                let (mut worst, mut total, mut n) = (0f64, 0f64, 0u32);
                while !done.load(std::sync::atomic::Ordering::Relaxed) {
                    let t = Instant::now();
                    let copy = state.lock().unwrap().clone();
                    let held = t.elapsed().as_secs_f64();
                    drop(copy);
                    worst = worst.max(held);
                    total += held;
                    n += 1;
                    std::thread::sleep(std::time::Duration::from_millis(33));
                }
                (worst, total / f64::from(n.max(1)))
            })
        });
        // The stream starts as far into a cycle as the recording did, so its
        // transmissions sit in their cycles.
        let into_cycle = m["captures"][0]["core:datetime"]
            .as_str()
            .and_then(|t| {
                t.trim_end_matches('Z')
                    .rsplit(':')
                    .next()?
                    .parse::<f64>()
                    .ok()
            })
            .unwrap()
            % 15.0;
        let began = Instant::now();
        let (mut refused, mut episodes, mut last_refused) = (0u64, 0u64, false);
        let mut buf = vec![0u8; pairs * 8];
        for k in 0..(seconds as f64 * rate / pairs as f64) as u64 {
            if file.read_exact(&mut buf).is_err() {
                break;
            }
            let due = (k + 1) as f64 * pairs as f64 / rate;
            if let Some(wait) = std::time::Duration::from_secs_f64(due).checked_sub(began.elapsed())
            {
                std::thread::sleep(wait);
            }
            let taken = tx
                .try_send(TimedBlock {
                    block: StreamBlock {
                        seq: k,
                        gap_before: false,
                        bytes: buf.clone(),
                        first_pair: k * pairs as u64,
                        centre_hz: 14_074_000,
                        rate_hz: rate,
                    },
                    arrived_unix: EPOCH + into_cycle + began.elapsed().as_secs_f64(),
                })
                .is_ok();
            refused += u64::from(!taken);
            episodes += u64::from(!taken && !last_refused);
            last_refused = !taken;
        }
        drop(tx);
        run.join().unwrap();
        done.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(s) = staller {
            s.join().unwrap();
        }
        if let Some((worst, mean)) = ui.map(|u| u.join().unwrap()) {
            println!(
                "the screen's clone: worst {:.1} ms, mean {:.2} ms",
                worst * 1e3,
                mean * 1e3
            );
        }
        let f = state.lock().unwrap().ft8.clone();
        let count = |o: CycleOutcome| f.cycles.iter().filter(|c| c.outcome == o).count();
        println!(
            "queue {depth}, {seconds} s, {pairs} pairs a block: refused {refused} in {episodes} episodes; cycles decoded {} cut {} skipped {}; {} decodes in {:.1} s; load {:.2}",
            count(CycleOutcome::Decoded),
            count(CycleOutcome::Cut),
            count(CycleOutcome::Skipped),
            f.cycles.iter().map(|c| c.decodes).sum::<usize>(),
            f.cycles.iter().map(|c| c.seconds).sum::<f64>(),
            f.feed.load.unwrap_or(0.0),
        );
    }

    fn cq() -> Message {
        Message::Standard {
            to: Call::Cq(None),
            from: Call::Std {
                call: "HA5ZZT".into(),
                suffix: None,
            },
            ack: false,
            word: G15::Grid("JN97".into()),
        }
    }

    #[test]
    fn a_cycle_on_the_quarter_minute_is_decoded_with_its_dt() {
        // The stream starts 7 s into a cycle; the next one begins at EPOCH +
        // 15 and the transmission starts 0.5 + 0.3 s into it. Blocks arrive
        // 20 ms after their last sample: that delay is the receive path's,
        // and DT reads it.
        let start = EPOCH + 7.0;
        let x = stream(start, 24.0, 1200.0, EPOCH + 15.0 + 0.8, &cq());
        let f = run(deliver(&x, start, 0.020, None));
        let decoded: Vec<_> = f
            .cycles
            .iter()
            .filter(|c| c.outcome == CycleOutcome::Decoded)
            .collect();
        assert_eq!(decoded.len(), 1, "{:?}", f.cycles);
        assert_eq!(decoded[0].start_unix, EPOCH + 15.0);
        assert_eq!(f.decodes.len(), 1);
        let d = &f.decodes[0].decode;
        assert_eq!(d.message.text(), "CQ HA5ZZT JN97");
        assert!((d.dt - 0.320).abs() < 0.010, "DT {}", d.dt);
        assert!((d.physics.f0 - 1200.0).abs() < 0.05, "{}", d.physics.f0);
        assert_eq!(f.stations["HA5ZZT"].locator.as_deref(), Some("JN97"));
        assert_eq!(f.feed.gaps, 0);
    }

    #[test]
    fn a_hole_in_the_stream_cuts_the_cycle_it_falls_in() {
        let start = EPOCH + 7.0;
        let x = stream(start, 24.0, 1200.0, EPOCH + 15.0 + 0.8, &cq());
        // A block missing 12 s into the stream: 4 s into the cycle.
        let f = run(deliver(&x, start, 0.020, Some(120)));
        assert!(f.decodes.is_empty());
        let cut: Vec<_> = f
            .cycles
            .iter()
            .filter(|c| c.outcome == CycleOutcome::Cut)
            .collect();
        assert_eq!(cut.len(), 1, "{:?}", f.cycles);
        assert_eq!(cut[0].start_unix, EPOCH + 15.0);
        assert_eq!(f.feed.gaps, 1);
    }

    #[test]
    fn a_clock_off_by_seconds_shows_in_dt() {
        // The computer's clock 1.5 s slow: every block is stamped 1.5 s
        // early, the cycle boundary falls 1.5 s late in the stream, and the
        // transmission reads as starting 1.5 s early (its first 0.7 s fall
        // before the cycle as this receiver cuts it; the rest decodes).
        let start = EPOCH + 7.0;
        let x = stream(start, 26.0, 1200.0, EPOCH + 15.0 + 0.8, &cq());
        let f = run(deliver(&x, start, 0.020 - 1.5, None));
        let d = &f.decodes.front().expect("decoded").decode;
        assert!((d.dt - (0.320 - 1.5)).abs() < 0.010, "DT {}", d.dt);
    }

    #[test]
    fn a_call_to_you_sent_as_a_hash_is_still_yours() {
        use crate::signal::ft8::fields::hashes;
        use crate::signal::ft8::hash::HashBits;
        let start = EPOCH + 7.0;
        let to_me = Message::Standard {
            to: Call::Hashed {
                bits: HashBits::H22,
                value: hashes("HA5ZZT").unwrap().h22,
                call: None,
            },
            from: Call::Std {
                call: "JA1VQD".into(),
                suffix: None,
            },
            ack: false,
            word: G15::Report(-21),
        };
        let x = stream(start, 24.0, 1200.0, EPOCH + 15.0 + 0.8, &to_me);
        let state = Arc::new(Mutex::new(SdrMetrics::fixture()));
        state.lock().unwrap().home.callsign = Some("HA5ZZT".into());
        let blocks = deliver(&x, start, 0.020, None);
        let (tx, rx) = crossbeam_channel::bounded(blocks.len());
        for b in blocks {
            tx.send(b).unwrap();
        }
        drop(tx);
        let mut w = Ft8Worker::new(rx, Arc::clone(&state), geometry());
        w.settings = Settings {
            max_candidates: 40,
            ..Settings::DEFAULT
        };
        w.run();
        let m = state.lock().unwrap();
        let d = m.ft8.decodes.front().expect("decoded");
        assert_eq!(d.decode.message.text(), "<HA5ZZT> JA1VQD -21");
        assert!(d.for_you);
        assert!(m.ft8.stations["JA1VQD"].called_you);
    }

    /// The stream clock's estimate moves when a fast delivery leaves its
    /// window: here blocks arrive 10 ms late for the first 20 s and 40 ms
    /// late after, so 50 s in the estimate jumps 30 ms. Every cycle must
    /// still be there, decoded or said to be cut; none may vanish (found
    /// live: 20:59:30 went missing between 20:59:15 and 20:59:45).
    #[test]
    fn no_cycle_vanishes_when_the_clock_settles() {
        let start = EPOCH + 1.0;
        let x = stream(start, 76.0, 1200.0, EPOCH + 15.0 + 0.8, &cq());
        let mut blocks = deliver(&x, start, 0.010, None);
        for b in blocks.iter_mut() {
            if b.block.first_pair as f64 / RADIO > 20.0 {
                b.arrived_unix += 0.030;
            }
        }
        let f = run(blocks);
        let mut starts: Vec<f64> = f.cycles.iter().map(|c| c.start_unix - EPOCH).collect();
        starts.sort_by(f64::total_cmp);
        assert_eq!(starts, vec![15.0, 30.0, 45.0, 60.0], "{:?}", f.cycles);
        // None cut: the reserve absorbed the clock's step. (A cycle may be
        // skipped here only because the test delivers faster than real time
        // and the decoder is still busy, which is said as skipped.)
        assert!(
            f.cycles.iter().all(|c| c.outcome != CycleOutcome::Cut),
            "{:?}",
            f.cycles
        );
    }

    /// [`stream`] 20 dB down: its noise peaks near -28 dBFS over a cycle,
    /// under the HF+'s hold, where the full-level stream's peaks would
    /// reach it (about -8 dBFS) and hold every cycle, as the radio would.
    fn quiet(start_unix: f64, seconds: f64) -> Vec<Complex<f32>> {
        stream(start_unix, seconds, 1200.0, EPOCH + 15.0 + 0.8, &cq())
            .into_iter()
            .map(|z| z * 0.1)
            .collect()
    }

    fn the_decoded(f: &crate::state::Ft8State) -> &CycleEntry {
        let decoded: Vec<_> = f
            .cycles
            .iter()
            .filter(|c| c.outcome == CycleOutcome::Decoded)
            .collect();
        assert_eq!(decoded.len(), 1, "{:?}", f.cycles);
        decoded[0]
    }

    /// One sample at -6 dBFS, 4 s into the cycle, on a radio whose hold
    /// starts at -13.5: the cycle is held and its level refused, its decode
    /// kept.
    #[test]
    fn a_peak_at_the_hold_marks_the_cycle_held() {
        let start = EPOCH + 7.0;
        let mut x = quiet(start, 24.0);
        x[((EPOCH + 19.0 - start) * RADIO) as usize] = Complex::new(0.5, 0.0);
        let f = run_with(
            SdrMetrics::fixture().airspyhf(),
            deliver(&x, start, 0.020, None),
        );
        let c = the_decoded(&f);
        assert!(c.held);
        assert_eq!(c.floor_db_hz, None);
        assert_eq!(f.decodes.len(), 1);
    }

    /// The same sample on a radio that states no ceiling: nothing is held.
    #[test]
    fn a_radio_without_a_ceiling_is_never_held() {
        let start = EPOCH + 7.0;
        let mut x = stream(start, 24.0, 1200.0, EPOCH + 15.0 + 0.8, &cq());
        x[((EPOCH + 19.0 - start) * RADIO) as usize] = Complex::new(0.5, 0.0);
        let f = run(deliver(&x, start, 0.020, None));
        let c = the_decoded(&f);
        assert!(!c.held);
        assert!(c.floor_db_hz.is_some());
    }

    /// A quiet cycle keeps its floor, read on the word's scale: the stream's
    /// noise is 1e-4 of full scale squared over 192 kS/s.
    #[test]
    fn a_quiet_cycle_keeps_its_floor_and_its_setting() {
        let start = EPOCH + 7.0;
        let x = quiet(start, 24.0);
        let f = run_with(
            SdrMetrics::fixture().airspyhf(),
            deliver(&x, start, 0.020, None),
        );
        let c = the_decoded(&f);
        assert!(!c.held);
        assert!(!c.setting_changed);
        let want = 10.0 * (1e-4f64 / RADIO).log10();
        let floor = f64::from(c.floor_db_hz.expect("a floor"));
        assert!((floor - want).abs() < 0.5, "read {floor}, sent {want}");
        let setting = c.setting.as_ref().expect("a setting");
        assert_eq!((setting.dial_hz, setting.rate_hz), (14_074_000, RADIO));
        assert_eq!(setting.gains, vec![0.0, 0.0]);
        assert!(c.lateness.is_some());
        assert!(!c.funnel.is_empty());
    }

    /// The attenuator steps 4 s into a cycle: that cycle says its setting
    /// changed, the next one does not and carries the new setting.
    #[test]
    fn a_gain_change_inside_a_cycle_is_stamped_on_it() {
        let start = EPOCH + 7.0;
        let x = quiet(start, 40.0);
        let state = Mutex::new(SdrMetrics::fixture().airspyhf());
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut session = Session::new(geometry(), tx);
        for b in deliver(&x, start, 0.020, None) {
            if start + b.block.first_pair as f64 / RADIO >= EPOCH + 19.0 {
                state.lock().unwrap().radio.gains[0] = -6.0;
            }
            session.block(&b, &state);
        }
        drop(session);
        let jobs: Vec<Job> = rx.try_iter().collect();
        let at = |t: f64| {
            jobs.iter()
                .find(|j| j.start_unix == EPOCH + t)
                .unwrap_or_else(|| panic!("no cycle at +{t}"))
        };
        assert!(at(15.0).setting_changed);
        assert!(!at(30.0).setting_changed);
        assert_eq!(at(30.0).setting.as_ref().unwrap().gains[0], -6.0);
    }
}
