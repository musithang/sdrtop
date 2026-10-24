// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The pager worker: one thread, one block at a time, the shape of
//! `signal::NetWorker` and for its reasons. It owns what is carried between
//! blocks (the channel's filters, the decoder's clocks and the page being
//! read), so everything under it stays a pure function tested with no radio.
//!
//! **Nothing is carried across a break.** A block whose position does not
//! continue the last one's (a refused block, a driver's drop, a new stream)
//! or that was captured at another tuning ends every transmission in
//! progress: the open page is published as cut, the filters and clocks start
//! again. Joining samples milliseconds apart would read codewords that were
//! never sent.
//!
//! **The channel is the tuned frequency.** The radio's DC offset sits there,
//! and the channel takes it off (`channel::Channel`), so nothing has to
//! retune the radio for this view and the frequency on screen is the one
//! being read.
//!
//! **What it costs is measured, and shown** (`state::PagerFeed::load`):
//! the work takes a share of real time that grows with the sample rate, and
//! nothing lowers the rate behind the user's back.
//!
//! One lock block a block, and every float outside it.

use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crossbeam_channel::Receiver;
use num_complex::Complex;

use super::channel::Channel;
use super::demod::{Decoder, Received};
use crate::hardware::{SampleGeometry, StreamBlock};
use crate::signal::stream::plan_block;
use crate::state::SdrMetrics;

/// How much one block moves the load reading: about a second's worth of
/// blocks to settle, so the figure does not flicker with each one.
const LOAD_ALPHA: f64 = 0.05;

pub struct PagerWorker {
    rx: Receiver<StreamBlock>,
    state: Arc<Mutex<SdrMetrics>>,
    geometry: SampleGeometry,
}

impl PagerWorker {
    pub fn new(
        rx: Receiver<StreamBlock>,
        state: Arc<Mutex<SdrMetrics>>,
        geometry: SampleGeometry,
    ) -> Self {
        Self {
            rx,
            state,
            geometry,
        }
    }

    /// Until the feed's sender is gone.
    pub fn run(self) {
        let mut session = Session::new(self.geometry);
        while let Ok(block) = self.rx.recv() {
            session.block(&block, &self.state);
        }
    }
}

/// What is carried from one block to the next.
struct Session {
    geometry: SampleGeometry,
    last_seq: u64,
    drop_ref: Option<u64>,
    next_pair: Option<u64>,
    tuning: Option<(u64, f64)>,
    receiver: Option<(Channel, Decoder)>,
    iq: Vec<Complex<f32>>,
    ch: Vec<Complex<f32>>,
    load: Option<f64>,
}

impl Session {
    fn new(geometry: SampleGeometry) -> Self {
        Self {
            geometry,
            last_seq: 0,
            drop_ref: None,
            next_pair: None,
            tuning: None,
            receiver: None,
            iq: Vec::new(),
            ch: Vec::new(),
            load: None,
        }
    }

    fn block(&mut self, b: &StreamBlock, state: &Mutex<SdrMetrics>) {
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
        let previous = self.tuning.map_or(b.centre_hz, |t| t.0);
        self.tuning = Some(tuning);

        // Everything in progress ends at a break or a retune, published as
        // cut, at the tuning it was received at, not this block's; a retune
        // also needs a channel built for the new rate.
        let mut cut: Vec<Received> = Vec::new();
        if !continuous || retuned {
            if let Some((channel, decoder)) = &mut self.receiver {
                cut.extend(decoder.reset());
                channel.reset();
            }
        }
        let mut heard: Vec<Received> = Vec::new();
        if retuned || self.receiver.is_none() {
            let channel = Channel::new(b.rate_hz, 0.0);
            let decoder = Decoder::new(channel.rate());
            self.receiver = Some((channel, decoder));
        }

        let t0 = Instant::now();
        let (locked, channel_rate) = match &mut self.receiver {
            Some((channel, decoder)) => {
                channel.push(&self.iq, &mut self.ch);
                heard.extend(decoder.push(&self.ch));
                (decoder.locked(), Some(channel.rate()))
            }
            None => (None, None),
        };
        let spent = t0.elapsed().as_secs_f64();
        if b.rate_hz > 0.0 && pairs > 0 {
            let share = spent / (pairs as f64 / b.rate_hz);
            self.load = Some(match self.load {
                Some(l) => l + LOAD_ALPHA * (share - l),
                None => share,
            });
        }
        let unix_s = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0.0, |d| d.as_secs_f64());
        let now = Instant::now();

        let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
        let p = &mut m.pager;
        p.feed.blocks_in = p.feed.blocks_in.saturating_add(1);
        p.feed.gaps = p.feed.gaps.saturating_add(u64::from(broke));
        p.feed.blocks_lost = p.feed.blocks_lost.saturating_add(plan.dropped);
        p.feed.last_block = Some(now);
        if broke || plan.dropped > 0 {
            p.feed.last_loss = Some(now);
        }
        p.feed.load = self.load;
        p.locked = locked;
        p.channel_rate = channel_rate;
        for r in cut {
            p.push(r.page, r.link, previous, now, unix_s);
        }
        for r in heard {
            p.push(r.page, r.link, b.centre_hz, now, unix_s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::SampleFormat;
    use crate::signal::pocsag::demod::tests::{air_bits, alpha, fsk, Air};
    use crate::signal::pocsag::page::tests::transmit;
    use crate::signal::pocsag::page::{End, Function};

    const RATE: f64 = 2.4e6;

    fn geometry() -> SampleGeometry {
        SampleGeometry {
            format: SampleFormat::Int8,
            full_scale: 128.0,
            converter_bits: None,
        }
    }

    /// The radio's own bytes: 8-bit I and Q, with the DC offset a tuner
    /// leaves at the centre, where this channel sits.
    fn bytes(iq: &[Complex<f32>]) -> Vec<u8> {
        iq.iter()
            .flat_map(|z| {
                let q = |v: f32| ((v + 0.08) * 127.0).round().clamp(-127.0, 127.0) as i8 as u8;
                [q(z.re), q(z.im)]
            })
            .collect()
    }

    fn blocks(iq: &[Complex<f32>], pairs: usize, centre_hz: u64) -> Vec<StreamBlock> {
        iq.chunks(pairs)
            .scan(0u64, |at, c| {
                let b = StreamBlock {
                    seq: *at / pairs as u64,
                    gap_before: false,
                    bytes: bytes(c),
                    first_pair: *at,
                    centre_hz,
                    rate_hz: RATE,
                };
                *at += c.len() as u64;
                Some(b)
            })
            .collect()
    }

    fn air(text: &str) -> Vec<Complex<f32>> {
        let batches = transmit(&[(200_001, Function::D, alpha(text))]);
        fsk(
            &air_bits(&batches, 576),
            Air::at(1200),
            RATE,
            20_000,
            20_000,
        )
    }

    fn run(blocks: Vec<StreamBlock>) -> SdrMetrics {
        let state = Arc::new(Mutex::new(SdrMetrics::fixture()));
        let (tx, rx) = crossbeam_channel::unbounded();
        for b in blocks {
            tx.send(b).unwrap();
        }
        drop(tx);
        PagerWorker::new(rx, Arc::clone(&state), geometry()).run();
        let m = state.lock().unwrap().clone();
        m
    }

    /// From the radio's 8-bit bytes, a page on the tuned centre with the
    /// tuner's DC sitting on it, into the inbox, numbered, with its tuning,
    /// and the feed counted.
    #[test]
    fn a_page_on_the_centre_reaches_the_inbox() {
        let m = run(blocks(
            &air("ON THE CENTRE, THROUGH THE DC"),
            65_536,
            439_987_500,
        ));
        assert_eq!(m.pager.inbox.len(), 1, "{:?}", m.pager.inbox);
        let e = &m.pager.inbox[0];
        assert_eq!(e.page.text(), "ON THE CENTRE, THROUGH THE DC");
        assert_eq!((e.n, e.centre_hz), (1, 439_987_500));
        assert_eq!(m.pager.feed.gaps, 0);
        assert!(m.pager.feed.blocks_in > 0);
        assert!(m.pager.feed.load.is_some());
        assert!(m.pager.channel_rate.is_some());
    }

    /// A block missing in the middle of a page cuts it there: published as
    /// cut, the gap counted and dated, nothing read across the hole.
    #[test]
    fn a_lost_block_cuts_the_page() {
        let mut bs = blocks(
            &air("A PAGE LONG ENOUGH THAT A BLOCK GOES MISSING IN THE MIDDLE OF IT, TRULY"),
            32_768,
            439_987_500,
        );
        let gone = bs.len() / 2;
        bs.remove(gone);
        let m = run(bs);
        assert_eq!(m.pager.feed.gaps, 1);
        assert!(m.pager.feed.blocks_lost >= 1);
        assert!(m.pager.feed.last_loss.is_some());
        assert!(
            m.pager.inbox.iter().any(|e| e.page.end == End::Cut),
            "{:?}",
            m.pager.inbox
        );
    }

    /// A retune mid-page cuts it too, and the next tuning is read on its own.
    #[test]
    fn a_retune_cuts_the_page() {
        let iq = air("TUNED AWAY HALF WAY THROUGH THE PAGE, SO IT IS CUT HERE");
        let mut bs = blocks(&iq, 32_768, 439_987_500);
        let half = bs.len() / 2;
        for b in &mut bs[half..] {
            b.centre_hz = 145_600_000;
        }
        let m = run(bs);
        assert!(m.pager.inbox.iter().all(|e| e.centre_hz == 439_987_500));
        assert!(m.pager.inbox.iter().any(|e| e.page.end == End::Cut));
    }
}
