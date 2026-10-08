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

/// How long the estimate of the radio's DC offset averages over, s: the
/// offset moves with gain and temperature over seconds, so half of one
/// follows it, while a packet's own mean, a few milliseconds of it, is
/// diluted away.
const DC_TIME_S: f64 = 0.5;

/// The radio's DC offset, tracked from the blocks' means and taken off them.
///
/// **On the samples, not on the frequency.** A receiver of an FM signal reads
/// its frequency, and an offset of the carrier is a constant there, which
/// each receiver already takes out. The radio's DC is not that: it is added
/// to the samples, at baseband 0 Hz, exactly where a packet at the tuned
/// centre sits, as every advertising packet does in LOCK. Stronger than the
/// packet, it keeps the phasor from turning about the origin and the
/// frequency read from it falls apart, which no correction after the
/// discriminator can undo. On the air, an LE Coded phone two rooms away was
/// heard in none of 77 advertisements until it was taken off, and half
/// again as many LE 1M packets passed their CRC.
#[derive(Default)]
struct Dc {
    estimate: Option<Complex<f64>>,
    /// Where the stream stood at the end of the last block measured.
    end: u64,
}

/// How long a stretch is judged noise or not, s: shorter than any gap
/// between packets, long enough that its statistics are statistics.
const QUIET_PART_S: f64 = 50e-6;

/// The rate the stretches are judged at, Hz: every sample up to it, every
/// few above. Raw noise is white, so the samples kept are as independent as
/// all of them, at a fifth of the work at 20 Msps.
const QUIET_JUDGED_HZ: f64 = 4e6;

/// The least `E|x|^4 / (E|x|^2)^2` about a stretch's own mean that counts it
/// as noise alone. Complex Gaussian noise gives 2 (its power is
/// exponential); a constant envelope, any GFSK packet, gives 1; noise and a
/// packet as strong as it, 1.75; a packet three times as strong, 1.44. Noise
/// judged on 200 samples falls under 1.5 about once in twenty, which only
/// costs a stretch.
const NOISE_KURTOSIS: f64 = 1.5;

/// How much more than the quietest noise-like stretch's spread a stretch may
/// have and still count: noise keeps its stretches within about 30 % of each
/// other, a packet 3 dB over the noise doubles one.
const QUIET_SPREAD: f64 = 2.0;

/// How much stream judged noise a block must hold before it says anything
/// about the DC, s: 40 stretches, a mean good to well under the noise, and
/// more than a short block that is one packet from end to end can offer.
const DC_EVIDENCE_S: f64 = 2e-3;

impl Dc {
    fn take_off(&mut self, block: &mut [Complex<f32>], first_pair: u64, rate_hz: f64) {
        if block.is_empty() || rate_hz.is_nan() || rate_hz <= 0.0 {
            return;
        }
        // Unmeasured for longer than it averages over, the old estimate says
        // nothing about now.
        let away = first_pair.saturating_sub(self.end) as f64 / rate_hz;
        if away > DC_TIME_S {
            self.estimate = None;
        }
        self.end = first_pair + block.len() as u64;
        if let Some(mean) = quiet_mean(block, rate_hz) {
            self.estimate = Some(match self.estimate {
                Some(old) => {
                    let weight = (block.len() as f64 / rate_hz / DC_TIME_S).min(1.0);
                    old + (mean - old) * weight
                }
                None => mean,
            });
        }
        // No estimate yet, and this block offered none: nothing is taken off.
        let Some(estimate) = self.estimate else {
            return;
        };
        let dc = Complex::new(estimate.re as f32, estimate.im as f32);
        block.iter_mut().for_each(|z| *z -= dc);
    }
}

/// The mean of `block` where nothing was on the air: over its stretches that
/// look like noise alone ([`NOISE_KURTOSIS`]) and are as quiet as the
/// quietest of those ([`QUIET_SPREAD`]), or `None` when they add up to less
/// than [`DC_EVIDENCE_S`].
///
/// The radio's DC is in every stretch alike; a packet is in some, and its own
/// mean, small but not zero (and large for a regular pattern at the centre,
/// whose carrier it is), would be taken off itself if those were counted: on
/// a clean packet at the tuned centre that spread the deviation the suites
/// read tenfold. Not a fixed share of the stretches either: in a busy stretch
/// of stream packets fill more than half of them, and in a short one all.
///
/// **Judged by the shape of the spread and by its size, each covering the
/// other's blind spot.** About its own mean, a stretch of noise has the
/// statistics of noise whatever the DC and whatever the gain; a packet's
/// does not, however small its spread, and by size alone a regular pattern
/// at the centre passed for silence. But a stretch half noise and half a
/// strong packet has the statistics of noise too (the shape reads `1 / f` for
/// a packet in a share `f` of it), and its size gives it away. Size is taken
/// about each stretch's own mean, not as power: power counts the DC, and the
/// stretches whose noise happened to point away from it would read
/// quietest.
fn quiet_mean(block: &[Complex<f32>], rate_hz: f64) -> Option<Complex<f64>> {
    let part = ((QUIET_PART_S * rate_hz) as usize).max(1);
    let stride = ((rate_hz / QUIET_JUDGED_HZ).round() as usize).max(1);
    // Each noise-like stretch's spread and mean.
    let noisy: Vec<(f64, Complex<f64>)> = block
        .chunks_exact(part)
        .filter_map(|c| {
            let kept = || {
                c.iter()
                    .step_by(stride)
                    .map(|z| Complex::new(z.re as f64, z.im as f64))
            };
            let n = kept().count() as f64;
            let mean = kept().sum::<Complex<f64>>() / n;
            let (m2, m4) = kept().fold((0.0f64, 0.0f64), |(a, b), z| {
                let p = (z - mean).norm_sqr();
                (a + p, b + p * p)
            });
            let (m2, m4) = (m2 / n, m4 / n);
            (m2 > 0.0 && m4 / (m2 * m2) >= NOISE_KURTOSIS).then_some((m2, mean))
        })
        .collect();
    let floor = noisy.iter().map(|s| s.0).fold(f64::INFINITY, f64::min);
    let quiet: Vec<Complex<f64>> = noisy
        .iter()
        .filter(|s| s.0 <= QUIET_SPREAD * floor)
        .map(|s| s.1)
        .collect();
    ((quiet.len() * part) as f64 / rate_hz >= DC_EVIDENCE_S)
        .then(|| quiet.iter().sum::<Complex<f64>>() / quiet.len() as f64)
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
    /// The radio's DC offset, as far as the blocks have shown it.
    dc: Dc,
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
            dc: Dc::default(),
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
        // Another tuning, or a stream started again (perhaps at another
        // gain), is another DC.
        if retuned || new_stream {
            self.dc = Dc::default();
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

    /// The radio's DC offset taken off this block's samples, before any
    /// receiver or the measurement path sees them ([`Dc`] has why).
    pub(super) fn take_off_dc(
        &mut self,
        block: &mut [Complex<f32>],
        first_pair: u64,
        rate_hz: f64,
    ) {
        self.dc.take_off(block, first_pair, rate_hz);
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
        self.dc = Dc::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A block of noise at `level` with the radio's DC `dc` added, as
    /// samples.
    fn noisy(len: usize, dc: Complex<f32>, seed: u64) -> Vec<Complex<f32>> {
        let mut rng = crate::signal::dsp::testkit::Rng::new(seed);
        rng.noise(len, 0.001).into_iter().map(|z| z + dc).collect()
    }

    fn mean(block: &[Complex<f32>]) -> Complex<f32> {
        block.iter().sum::<Complex<f32>>() / block.len() as f32
    }

    /// The radio's DC comes off the block it is in, from the first block on.
    #[test]
    fn the_radios_dc_comes_off_a_block() {
        let mut feed = Feed::new(SampleGeometry {
            format: crate::hardware::SampleFormat::Int8,
            full_scale: 128.0,
        });
        let dc = Complex::new(0.08, -0.05);
        let mut block = noisy(100_000, dc, 1);
        feed.take_off_dc(&mut block, 0, 20e6);
        assert!(mean(&block).norm() < 0.001, "{}", mean(&block));
    }

    /// **A block with no quiet stretch says nothing about the DC, and is
    /// left as it came.** A short block that is one packet from end to end,
    /// at the tuned centre, would otherwise offer the packet's own mean (its
    /// carrier, for a regular preamble) as the radio's, and lose it.
    #[test]
    fn a_block_with_no_quiet_stretch_is_left_alone() {
        let mut feed = Feed::new(SampleGeometry {
            format: crate::hardware::SampleFormat::Int8,
            full_scale: 128.0,
        });
        // An alternating pattern at the centre: phase swinging about a
        // fixed point, so a large mean that is signal, not DC.
        let bits: Vec<bool> = (0..200).map(|i| i % 2 == 0).collect();
        let wave = crate::signal::ble::gfsk::modulate(&bits, 20, 160_000.0, 20e6, 0.5);
        let mut block = wave.clone();
        feed.take_off_dc(&mut block, 0, 20e6);
        assert_eq!(block, wave);
    }

    /// A step in the DC (a gain change, say) is followed: three and a half
    /// of its time constants on, under 5 % of the step is left (3 % would
    /// be exact).
    #[test]
    fn a_step_in_the_dc_is_followed_within_its_time() {
        let mut feed = Feed::new(SampleGeometry {
            format: crate::hardware::SampleFormat::Int8,
            full_scale: 128.0,
        });
        let (rate, len) = (20e6, 131_072usize);
        let (before, after) = (Complex::new(0.08, -0.05), Complex::new(-0.02, 0.06));
        let mut at = 0u64;
        for k in 0..10 {
            let mut block = noisy(len, before, k);
            feed.take_off_dc(&mut block, at, rate);
            at += len as u64;
        }
        let blocks = (3.5 * DC_TIME_S * rate / len as f64).ceil() as u64;
        let mut last = Complex::new(0.0, 0.0);
        for k in 0..blocks {
            let mut block = noisy(len, after, 100 + k);
            feed.take_off_dc(&mut block, at, rate);
            last = mean(&block);
            at += len as u64;
        }
        let step = (after - before).norm();
        assert!(last.norm() < 0.05 * step, "{} left of {step}", last.norm());
    }

    /// A retune is a new DC: the estimate starts again from the first block
    /// at the new tuning rather than following there from the old one.
    #[test]
    fn a_retune_starts_the_dc_estimate_again() {
        let mut feed = Feed::new(SampleGeometry {
            format: crate::hardware::SampleFormat::Int8,
            full_scale: 128.0,
        });
        let len = 65_536usize;
        let block = |seq: u64, centre_hz: u64| StreamBlock {
            seq,
            gap_before: false,
            bytes: vec![0; len * 2],
            first_pair: (seq - 1) * len as u64,
            centre_hz,
            rate_hz: 20e6,
        };
        for seq in 1..=4u64 {
            feed.arrive(&block(seq, 2_426_000_000));
            let mut iq = noisy(len, Complex::new(0.08, -0.05), seq);
            feed.take_off_dc(&mut iq, (seq - 1) * len as u64, 20e6);
        }
        let arrival = feed.arrive(&block(5, 2_480_000_000));
        assert!(arrival.retuned);
        let mut iq = noisy(len, Complex::new(-0.03, 0.07), 5);
        feed.take_off_dc(&mut iq, 4 * len as u64, 20e6);
        assert!(mean(&iq).norm() < 0.002, "{}", mean(&iq));
    }

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
