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
use super::clock::StreamClock;
use super::cycle::{self, Settings};
use super::hash::HashBook;
use crate::hardware::{SampleGeometry, TimedBlock};
use crate::signal::stream::plan_block;
use crate::state::{CycleEntry, CycleOutcome, SdrMetrics};

/// How much one block moves the load reading.
const LOAD_ALPHA: f64 = 0.05;

/// Samples in a cycle at FT8's rate.
const CYCLE_SAMPLES: usize = 15 * RATE_HZ as usize;

/// The clock verdict looks at this many of the latest decoded cycles.
const VERDICT_CYCLES: usize = 4;

/// One cycle to decode.
struct Job {
    start_unix: f64,
    dial_hz: u64,
    samples: Vec<Complex<f32>>,
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
        if let Some(ch) = &mut self.channel {
            self.clock.push(b.first_pair, pairs, tb.arrived_unix);
            if self.pair0.is_none() {
                self.pair0 = Some(b.first_pair);
            }
            ch.push(&self.iq, &mut self.buf);
            ready = self.take_cycles(b.centre_hz);
        }
        let spent = t0.elapsed().as_secs_f64();
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
        f.hearing = self.next_cycle;
        if let Some(start) = cut {
            f.push_cycle(
                undecoded(start, dial_before, CycleOutcome::Cut),
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

    /// Every cycle the buffer now holds whole, and the buffer trimmed to the
    /// next one's start.
    fn take_cycles(&mut self, dial_hz: u64) -> Vec<Job> {
        let mut out = Vec::new();
        let Some(t0) = self.unix_of_sample(0.0) else {
            return out;
        };
        let rate = RATE_HZ as f64;
        if self.next_cycle.is_none() {
            let first = self.unix_of_sample(self.buf_base as f64).unwrap_or(t0);
            self.next_cycle = Some((first / 15.0).ceil() * 15.0);
        }
        while let Some(start) = self.next_cycle {
            let i = ((start - t0) * rate).round();
            if i < self.buf_base as f64 {
                // The clock moved the boundary behind what is kept: this
                // cycle is incomplete at its start, so go to the next.
                self.next_cycle = Some(start + 15.0);
                continue;
            }
            let i = i as usize;
            let end = i + CYCLE_SAMPLES;
            if end > self.buf_base + self.buf.len() {
                // Not whole yet: keep from its start.
                let drop = (i - self.buf_base).min(self.buf.len());
                self.buf.drain(..drop);
                self.buf_base += drop;
                break;
            }
            let from = i - self.buf_base;
            out.push(Job {
                start_unix: start,
                dial_hz,
                samples: self.buf[from..from + CYCLE_SAMPLES].to_vec(),
            });
            self.next_cycle = Some(start + 15.0);
        }
        out
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
        let state = Arc::new(Mutex::new(SdrMetrics::fixture()));
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
}
