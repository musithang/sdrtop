// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! What the stream brought: whether a block follows the last one, the
//! samples held for the measurement path, and what handling it cost.

use std::collections::VecDeque;
use std::time::Instant;

use num_complex::Complex;

use crate::hardware::{SampleGeometry, StreamBlock};
use crate::signal::net::measure::Recent;
use crate::signal::stream::plan_block;
use crate::state::NetDecodeHealth;

use super::follow::FOLLOW_HELD_S;

/// `bytes` as samples, decoded into `slot` the first time a receiver asks
/// and handed out as they are after that.
pub(super) fn decoded_block<'a>(
    slot: &'a mut Option<Vec<num_complex::Complex<f32>>>,
    bytes: &[u8],
    geometry: SampleGeometry,
) -> &'a [num_complex::Complex<f32>] {
    slot.get_or_insert_with(|| {
        let mut out = Vec::new();
        crate::signal::demod::decode(bytes, geometry, usize::MAX, &mut out);
        out
    })
}

/// The blocks the measurement path may cut a burst from: those held from
/// before, and this one.
fn held<'a>(
    recent: &'a std::collections::VecDeque<(u64, Vec<num_complex::Complex<f32>>)>,
    current: Option<(u64, &'a [num_complex::Complex<f32>])>,
) -> crate::signal::net::measure::Recent<'a> {
    crate::signal::net::measure::Recent::new(
        recent
            .iter()
            .map(|(p, v)| (*p, v.as_slice()))
            .chain(current),
    )
}

/// Where the run of blocks stands, by sequence number.
///
/// Three fields, and two of them exist only so that a gap can be told from a
/// pause. See [`Run::suspend`].
#[derive(Default)]
struct Run {
    last_seq: u64,
    /// The sequence of the previous block *that reached us*, or `None` when the
    /// run has not started.
    drop_ref: Option<u64>,
    /// Unbroken blocks since the last gap.
    blocks: u64,
}

impl Run {
    /// The section was closed, so the feed stopped forwarding.
    ///
    /// The device goes on counting every callback, so the next block to arrive
    /// will be thousands of sequence numbers away without a single one having
    /// been lost. Clearing `drop_ref` is what stops that jump being reported as
    /// the worst loss event of the session. The run length goes with it: there
    /// is no run any more.
    fn suspend(&mut self) {
        self.drop_ref = None;
        self.blocks = 0;
    }
}

/// How long one decode-load reading averages over: half a second of stream,
/// or half a second of work, whichever comes first.
///
/// Half a second: at the block sizes the native radios deliver that is dozens
/// of blocks, enough that one slow block (a page fault, a scheduler hiccup)
/// does not read as a worker in trouble, and short enough that the figure
/// follows the user opening a heavier preset within a glance.
///
/// **Either clock closes the window, and the second one is not optional.** A
/// window counted in stream time alone takes longer to fill the slower the
/// worker is, because a worker that cannot keep up only ever sees the blocks
/// the bounded feed had room for. The first live run showed exactly that: a
/// worker in trouble whose load never appeared at all, twenty seconds in. So
/// the figure that matters most arrived last, or not at all. Closing the
/// window on work time too means an overloaded worker reports within half a
/// second, at whatever it really is.
const LOAD_WINDOW_S: f64 = 0.5;

/// The decode-load accumulator: wall time spent against stream time covered.
///
/// Pure, with the clock read by the caller, so the arithmetic is testable
/// without a radio or a real stopwatch.
#[derive(Default)]
struct Load {
    busy: std::time::Duration,
    stream_s: f64,
}

impl Load {
    /// Add one block: `spent` handling it, `pairs` I/Q pairs of it at
    /// `rate_hz`. Returns a reading once a whole window has been covered, and
    /// starts the next one.
    fn add(&mut self, spent: std::time::Duration, pairs: u64, rate_hz: f64) -> Option<f64> {
        // A rate that is not a positive number covers no stream time, and
        // dividing by it would invent a load.
        if rate_hz.is_nan() || rate_hz <= 0.0 {
            return None;
        }
        self.busy += spent;
        self.stream_s += pairs as f64 / rate_hz;
        if self.stream_s < LOAD_WINDOW_S && self.busy.as_secs_f64() < LOAD_WINDOW_S {
            return None;
        }
        let load = self.busy.as_secs_f64() / self.stream_s;
        *self = Load::default();
        Some(load)
    }
}

/// What the worker carries of the stream from one block to the next: where
/// it stands, what is held of it, and what handling it costs.
pub(super) struct Feed {
    run: Run,
    pair_bytes: u64,
    /// Where the next block must start for the stream to be unbroken. `None`
    /// until a block has been seen, and again after the section closes.
    next_pair: Option<u64>,
    /// The last few decoded blocks, with their stream positions: what the
    /// measurement path cuts a burst's raw samples from ([`Recent`]), moved
    /// here as each block finishes rather than copied, and dropped at any
    /// break.
    recent: VecDeque<(u64, Vec<Complex<f32>>)>,
    /// The tuning and rate of the samples held in `recent`: a retune keeps
    /// the stream's positions running, so only this says the held samples
    /// are another tuning's.
    held_tuning: Option<(f64, f64)>,
    load: Load,
}

/// What one block's arrival says about the stream.
pub(super) struct Arrival {
    /// The I/Q pairs it holds.
    pub(super) pairs: u64,
    /// The run of blocks broke before it.
    broke: bool,
    /// Blocks the channel lost before it.
    dropped: u64,
    /// Unbroken blocks since the last gap, this one included.
    run_blocks: u64,
    /// Its first sample follows the last block's last one.
    pub(super) continuous: bool,
    /// Its position is behind the expected one: a new stream (RX was
    /// restarted, `RxContext::begin_stream`), whose clock starts again.
    pub(super) new_stream: bool,
    /// It was captured at another tuning or rate than the samples held.
    pub(super) retuned: bool,
}

impl Arrival {
    /// Counted into the feed's health, inside the caller's lock: integer
    /// work and a clock read before it, nothing else.
    pub(super) fn count(&self, h: &mut NetDecodeHealth, now: Instant) {
        h.blocks_in = h.blocks_in.saturating_add(1);
        h.pairs_in = h.pairs_in.saturating_add(self.pairs);
        h.gaps = h.gaps.saturating_add(u64::from(self.broke));
        h.blocks_lost = h.blocks_lost.saturating_add(self.dropped);
        h.run_blocks = self.run_blocks;
        h.last_block = Some(now);
        if self.broke || self.dropped > 0 {
            h.last_loss = Some(now);
        }
    }
}

impl Feed {
    pub(super) fn new(geometry: SampleGeometry) -> Self {
        Self {
            run: Run::default(),
            pair_bytes: geometry.bytes_per_pair() as u64,
            next_pair: None,
            recent: VecDeque::new(),
            held_tuning: None,
            load: Load::default(),
        }
    }

    /// One block arrived: where it stands in the run and the stream, and
    /// whether the samples held can still be joined to it. They are dropped
    /// where they cannot.
    pub(super) fn arrive(&mut self, b: &StreamBlock) -> Arrival {
        let run = &mut self.run;
        let started = run.drop_ref.is_some();
        let plan = plan_block(b.seq, b.gap_before, run.last_seq, run.drop_ref);
        run.last_seq = b.seq;
        run.drop_ref = Some(b.seq);

        let pairs = b.bytes.len() as u64 / self.pair_bytes.max(1);
        // **A run has to have started before it can be interrupted.**
        // `plan_block` guards its `dropped` count with `drop_ref` and does
        // not guard `contiguous` with anything, because for the demod the
        // difference is invisible: a first block declared discontiguous just
        // resets session state that is already empty. Here the same flag is
        // about to become a number on a panel, and the section is normally
        // opened on a radio that has been streaming for a minute - so the
        // first block through carries a sequence number thousands past
        // whatever this worker last saw, and would report an interruption
        // that never happened, once per visit.
        let broke = started && !plan.contiguous;
        run.blocks = if broke || !started { 1 } else { run.blocks + 1 };

        // **No receiver is ever carried across a break in the samples.** A
        // decimator's filter state, a capture half-filled with the start of
        // a packet, a classic receiver's symbol count - all of them assume
        // the next sample follows the last one. Across a refused block, a
        // driver drop or a restarted stream it does not, and carrying on
        // joins two moments milliseconds apart into one signal that never
        // existed. The position the block carries says whether it follows;
        // when it does not, the receivers start again from this block.
        let continuous = self.next_pair == Some(b.first_pair);
        let new_stream = self.next_pair.is_some_and(|n| b.first_pair < n);
        if !continuous {
            self.recent.clear();
        }
        self.next_pair = Some(b.first_pair + pairs);
        // Samples held from another tuning are not this one's: a window or
        // a measurement cut from them would be mixed to the wrong channel.
        let tuning = (b.centre_hz as f64, b.rate_hz);
        let retuned = self.held_tuning.is_some_and(|t| t != tuning);
        if retuned {
            self.recent.clear();
        }
        self.held_tuning = Some(tuning);
        Arrival {
            pairs,
            broke,
            dropped: plan.dropped,
            run_blocks: run.blocks,
            continuous,
            new_stream,
            retuned,
        }
    }

    /// The blocks the measurement path may cut a burst from: those held from
    /// before, and `current`, this one, where it was decoded.
    pub(super) fn window<'a>(&'a self, current: Option<(u64, &'a [Complex<f32>])>) -> Recent<'a> {
        held(&self.recent, current)
    }

    /// This block, held for the measurement path, as much as
    /// `measure::HELD_S` asks and no more, or as [`FOLLOW_HELD_S`] where
    /// something needs `longer` (a followed connection's event, an LE Coded
    /// packet at S=8, which lasts up to 17 ms and is measured whole once it
    /// ends, or a promise's window). A block nothing decoded, or one that
    /// arrived with the section closed, leaves a hole, so what was held
    /// before it can no longer be joined to what comes after.
    pub(super) fn hold(
        &mut self,
        first_pair: u64,
        iq: Option<Vec<Complex<f32>>>,
        open: bool,
        longer: bool,
        rate_hz: f64,
    ) {
        match iq {
            Some(block) if open => {
                self.recent.push_back((first_pair, block));
                let held_s = if longer {
                    FOLLOW_HELD_S.max(crate::signal::net::measure::HELD_S)
                } else {
                    crate::signal::net::measure::HELD_S
                };
                let keep = (held_s * rate_hz) as usize;
                while self.recent.len() > 1
                    && self
                        .recent
                        .iter()
                        .skip(1)
                        .map(|(_, b)| b.len())
                        .sum::<usize>()
                        >= keep
                {
                    self.recent.pop_front();
                }
            }
            _ => self.recent.clear(),
        }
    }

    /// The decode load, once a window of it is covered: `spent` handling a
    /// block of `pairs` at `rate_hz`.
    pub(super) fn cost(
        &mut self,
        spent: std::time::Duration,
        pairs: u64,
        rate_hz: f64,
    ) -> Option<f64> {
        self.load.add(spent, pairs, rate_hz)
    }

    /// The section closed: the run is suspended rather than broken, the load
    /// starts again, and the next block starts a stream of its own.
    pub(super) fn close(&mut self) {
        self.run.suspend();
        self.load = Load::default();
        self.next_pair = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wall time over stream time, reported once a window is covered.
    #[test]
    fn the_load_is_wall_time_over_the_stream_time_it_covered() {
        use std::time::Duration;
        let mut load = Load::default();
        // 1 MHz, 100 000 pairs a block: 0.1 s of stream each, 25 ms to handle.
        for _ in 0..4 {
            assert_eq!(
                load.add(Duration::from_millis(25), 100_000, 1e6),
                None,
                "under half a second of stream, no reading yet"
            );
        }
        let reading = load
            .add(Duration::from_millis(25), 100_000, 1e6)
            .expect("five blocks cover the window");
        assert!((reading - 0.25).abs() < 1e-9, "{reading}");
        // And the next window starts from nothing.
        assert_eq!(load.add(Duration::from_millis(25), 100_000, 1e6), None);
    }

    /// Above one means the worker cannot keep up with the stream, and the
    /// figure must be free to say so rather than be clamped.
    #[test]
    fn a_load_above_one_is_reported_not_clamped() {
        use std::time::Duration;
        let mut load = Load::default();
        let reading = load
            .add(Duration::from_millis(900), 600_000, 1e6)
            .expect("0.6 s of stream covers the window");
        assert!((reading - 1.5).abs() < 1e-9, "{reading}");
    }

    /// A worker too slow to cover a window of stream still reports within
    /// half a second of work, at the figure it really is - the case a
    /// stream-time window alone would hide longest.
    #[test]
    fn an_overloaded_worker_reports_on_work_time_alone() {
        use std::time::Duration;
        let mut load = Load::default();
        // 10 000 pairs at 1 Msps is 10 ms of stream; each takes 200 ms.
        assert_eq!(load.add(Duration::from_millis(200), 10_000, 1e6), None);
        assert_eq!(load.add(Duration::from_millis(200), 10_000, 1e6), None);
        let reading = load
            .add(Duration::from_millis(200), 10_000, 1e6)
            .expect("0.6 s of work closes the window");
        assert!((reading - 20.0).abs() < 1e-9, "{reading}");
    }

    /// A rate that is not a positive number covers no stream time: no
    /// reading, rather than a division that invents one.
    #[test]
    fn no_rate_means_no_load() {
        use std::time::Duration;
        let mut load = Load::default();
        for rate in [0.0, -1.0, f64::NAN] {
            assert_eq!(load.add(Duration::from_secs(1), 1_000_000, rate), None);
        }
    }
}
