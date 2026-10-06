// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Device-agnostic per-block sample accumulation. Every IQ backend funnels its
//! raw byte blocks through here: HackRF from its `extern "C"` callback,
//! RTL-SDR from its owned read thread, SoapySDR from its read loop.
//!
//! **In two halves, on two threads**, and the split is the point. [`arrive`]
//! runs on the driver's thread and does only what must happen the moment a
//! block arrives: stamp its place in the stream, read the settings it was
//! captured under, hand it to the recorder. [`digest`] does everything else,
//! on the intake thread (`super::intake`), so the driver is never kept waiting
//! by the work. A HackRF whose transfer thread was kept waiting lost a third of
//! its samples at 20 Msps without a word: the radio's own buffer overflowed
//! while the callback was still folding the previous block. Only the byte→sample decode
//! branches on [`SampleFormat`]; the saturation test, every accumulator, the
//! histogram, drops, jitter, and the hand-off to the FFT worker are identical.
//! The rail a sample is tested against is the *declared full scale*, which is
//! also what the histogram bins and the peak level use, so the ADC bench's three
//! readings of one sample cannot disagree.

use std::time::Instant;

use super::traits::{RxContext, SampleFormat, SampleGeometry};

/// Closest two constellation samples may be taken, in I/Q pairs.
///
/// A floor, not the stride: see [`const_stride`]. Adjacent samples of a band-
/// limited stream are correlated, and a scope wants points that are not.
const CONST_DECIMATE: usize = 1024;
/// Constellation points one block may contribute.
///
/// A budget spread over the whole block rather than a cap that stops partway.
/// It bounds the per-block vector and the drain-and-extend the lock below pays
/// for, and it sets how fast the [`crate::state::CONSTELLATION_CAP`]-deep ring
/// turns over: sixteen blocks, which is about 210 ms of a 10 Msps HackRF.
const CONST_MAX_PER_BLOCK: usize = 64;

/// Pairs between constellation samples, for a block of this many pairs.
///
/// At least [`CONST_DECIMATE`], and otherwise wide enough that the budget
/// reaches the end of the block.
///
/// **The budget used to truncate instead.** A HackRF transfer is 131072 pairs,
/// which offers 128 candidates at a fixed stride of 1024, and the 64-point
/// budget ran out halfway: every point in the cloud came from the first half of
/// every transfer, and so did the ellipse, the EVM and the MER fitted to it. A
/// burst landing in the second half was in none of them. The smaller blocks the
/// other two backends deliver never reached the budget, so nothing there
/// changes.
fn const_stride(pairs: usize) -> usize {
    CONST_DECIMATE.max(pairs.div_ceil(CONST_MAX_PER_BLOCK))
}

/// Bin a centered signed sample into the 32-bucket signed ADC histogram:
/// bin 0 = −FS rail, 16 = mid-scale, 31 = +FS rail.
///
/// Integer arithmetic against the device's own full scale, so an 8-bit radio
/// lands on exactly the `(v + 128) / 8` this used to hardcode.
#[cfg(test)]
fn signed_bin(g: &SampleGeometry, v: i64) -> usize {
    let fs = g.full_scale as i64;
    ((v + fs) / bin_width(fs)).clamp(0, 31) as usize
}

/// Counts per histogram bucket: the full −FS..+FS span divided into 32.
///
/// `.max(1)` is not defensive dressing. A driver reporting a full scale under
/// 16 counts would otherwise divide by zero inside the RX callback, which is the
/// worst place in the program to find out.
#[inline]
fn bin_width(full_scale: i64) -> i64 {
    (full_scale * 2 / 32).max(1)
}

/// Counts per bucket of the *amplitude* histogram, which spans 0..+FS rather
/// than −FS..+FS and so uses half the width.
#[inline]
fn amp_bin_width(full_scale: i64) -> u64 {
    (full_scale as u64 / 32).max(1)
}

/// Where a sample lands in the two histograms, decided once per block.
///
/// **A shift where a bucket is a power of two wide**, which it is on every
/// 8-bit and 16-bit converter, and the division otherwise. The two agree
/// exactly: for a numerator that is not negative, a shift by `k` is a division
/// by `2^k`, and a negative one (a sample past minus full scale) clamps to
/// bucket 0 either way. The divisions this replaces were the largest single
/// cost of the per-sample fold, measured: four a pair, on the thread that must
/// keep up with the radio.
#[derive(Clone, Copy, Default)]
struct Bins {
    fs: i64,
    signed_width: i64,
    signed_shift: Option<u32>,
    amp_width: u64,
    amp_shift: Option<u32>,
}

/// Everything the fold needs to know about one centered sample.
///
/// Worked out per sample on a 16-bit radio, and looked up on an 8-bit one,
/// where a byte has only 256 values: [`Bins::table`].
#[derive(Clone, Copy, Default)]
struct Sample {
    v: i64,
    rail: bool,
    signed_bucket: u8,
    /// `|v|`, and its amplitude bucket. The pair's Chebyshev amplitude is the
    /// larger of its two, and so is its bucket, because the bucketing never
    /// puts a larger amplitude in a lower bucket.
    abs: u32,
    amp_bucket: u8,
}

impl Bins {
    /// One decoded sample, with its buckets.
    #[inline(always)]
    fn sample(&self, (v, rail): (i64, bool)) -> Sample {
        let abs = v.unsigned_abs();
        Sample {
            v,
            rail,
            signed_bucket: self.signed(v) as u8,
            abs: abs as u32,
            amp_bucket: self.amp(abs) as u8,
        }
    }

    /// Every byte of an 8-bit format, decoded and bucketed once.
    fn table(&self, format: SampleFormat) -> [Sample; 256] {
        std::array::from_fn(|b| self.sample(decode(format, self.fs, b as u8)))
    }

    fn new(g: &SampleGeometry) -> Self {
        let fs = g.full_scale as i64;
        let signed_width = bin_width(fs);
        let amp_width = amp_bin_width(fs);
        let shift = |w: u64| w.is_power_of_two().then(|| w.trailing_zeros());
        Self {
            fs,
            signed_width,
            signed_shift: shift(signed_width as u64),
            amp_width,
            amp_shift: shift(amp_width),
        }
    }

    /// The signed ADC histogram's bucket for a centered sample.
    #[inline]
    fn signed(&self, v: i64) -> usize {
        let n = v + self.fs;
        let bucket = match self.signed_shift {
            Some(k) => n >> k,
            None => n / self.signed_width,
        };
        bucket.clamp(0, 31) as usize
    }

    /// The amplitude histogram's bucket for a Chebyshev amplitude.
    #[inline]
    fn amp(&self, amp: u64) -> usize {
        let bucket = match self.amp_shift {
            Some(k) => amp >> k,
            None => amp / self.amp_width,
        };
        (bucket as usize).min(31)
    }
}

/// Both halves at once, on the caller's thread: [`arrive`] then [`digest`].
///
/// What the tests drive; a backend goes through `super::intake::deliver`.
/// `dropped_pairs` is the backend's short-transfer count (HackRF computes it
/// from `buffer_length − valid_length`; RTL-SDR has no equivalent and passes
/// 0). `now` is captured by the *caller* so jitter measures the true
/// inter-callback interval, not callback-entry-plus-processing time.
#[cfg(test)]
pub fn process_block(
    buf: &[u8],
    geometry: SampleGeometry,
    dropped_pairs: u64,
    ctx: &RxContext,
    now: Instant,
) {
    let arrival = arrive(buf, geometry, dropped_pairs, ctx, now);
    digest(buf, geometry, &arrival, 0, ctx);
}

/// What was true when a block arrived, taken on the driver's thread and
/// carried with the block to wherever it is digested.
#[derive(Clone, Copy)]
pub struct Arrival {
    cal: crate::state::IqCalState,
    demod_enabled: bool,
    net_enabled: bool,
    centre_hz: u64,
    rate_hz: f64,
    /// The block's place in the stream, after the pairs the driver lost.
    pub first_pair: u64,
    pub dropped_pairs: u64,
    now: Instant,
}

/// The half that runs on the driver's thread: stamp, snapshot, record.
///
/// One short lock, one atomic add, and while a recording runs one copy.
/// Nothing here grows with the work the block will cause.
pub fn arrive(
    buf: &[u8],
    geometry: SampleGeometry,
    dropped_pairs: u64,
    ctx: &RxContext,
    now: Instant,
) -> Arrival {
    let pairs = buf.len() / geometry.bytes_per_pair();

    // Snapshot the live correction state once (cheap Copy). The accumulators below
    // stay on the RAW samples, because a correction has to be built from what the
    // front end actually did, while a corrected copy of the stream feeds the FFT,
    // the demod and the constellation so the [D] DC-block / [C] auto-cal cleanup
    // is visible. What the bench *prints* is the residual after that correction
    // and not the raw impairment: this comment used to say otherwise, and the
    // split it was describing lives in `tasks::rx::metrics::iq_metrics`.
    // Read both feed gates in the same lock as the correction state - each
    // costs an extra copy of the block, so each must be free when switched off.
    // The NET gate is the section on screen rather than a switch of its own:
    // that section is the only consumer, and a preset the user is not looking at
    // is not a reason to copy every block off the USB callback.
    // The tuning is read in the same lock: it travels with the block, so a
    // worker that processes it later still knows where it was captured.
    // The recorder's gate is its own atomic, read before the lock so an idle
    // recorder adds nothing inside it; a running one takes the gain with the
    // tuning, from the same lock, so the file's gain and frequency are the
    // settings of one moment.
    let recording = ctx.record.armed();
    let (cal, demod_enabled, net_enabled, centre_hz, rate_hz, gain) = {
        let mut m = ctx.metrics.lock().unwrap_or_else(|e| e.into_inner());
        // What the radio delivered, counted where it arrives: a block the
        // intake later has no room for still crossed the USB link.
        m.radio.bytes_since_last_poll += buf.len() as u64;
        (
            m.iq.cal,
            m.demod.enabled,
            m.ui.is_net_section(),
            m.radio.frequency,
            m.radio.config_sample_rate,
            recording.then(|| (m.radio.gains.clone(), m.radio.amp_enabled)),
        )
    };
    // Where this block sits in the stream. The pairs the driver lost come
    // first - they happened before this block - and then the block itself, so
    // the next block's position is exactly past both.
    let first_pair = ctx.stream_pairs.fetch_add(
        dropped_pairs + pairs as u64,
        std::sync::atomic::Ordering::Relaxed,
    ) + dropped_pairs;
    // The recorder takes the raw bytes, whatever the correction: a correction
    // can be applied to a recording afterwards and never taken out of one.
    if let Some((gains, boost)) = gain {
        ctx.record.offer(
            super::record_tap::BlockAt {
                first_pair,
                pairs: pairs as u64,
                driver_dropped: dropped_pairs,
                centre_hz,
                rate_hz,
                arrived_unix: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs_f64())
                    .unwrap_or(0.0),
            },
            buf,
            gains,
            boost,
        );
    }

    Arrival {
        cal,
        demod_enabled,
        net_enabled,
        centre_hz,
        rate_hz,
        first_pair,
        dropped_pairs,
        now,
    }
}

/// Every per-sample figure of one block, with no lock and no state: the part
/// of [`digest`] that runs once for every pair.
fn accumulate(buf: &[u8], geometry: SampleGeometry, cal: crate::state::IqCalState) -> Accumulators {
    let format = geometry.format;
    let fs_counts = geometry.full_scale as i64;
    let pairs = buf.len() / geometry.bytes_per_pair();
    let correcting = cal.correcting();
    let mut acc = Accumulators {
        geometry,
        bins: Bins::new(&geometry),
        full_scale: geometry.full_scale,
        const_stride: const_stride(pairs),
        correcting,
        cal,
        ..Accumulators::default()
    };
    if correcting {
        acc.out.reserve(buf.len());
    }

    // The width branch is taken **once per block**, not once per sample. Both
    // arms call the same `fold`, so there is one copy of the accumulation and
    // two ways of getting a pair out of the bytes.
    match format {
        SampleFormat::Int8 | SampleFormat::Uint8 => {
            let table = acc.bins.table(format);
            for c in buf.as_chunks::<2>().0 {
                acc.fold(&table[c[0] as usize], &table[c[1] as usize]);
            }
        }
        SampleFormat::Int16 => {
            let bins = acc.bins;
            for c in buf.as_chunks::<4>().0 {
                acc.fold(
                    &bins.sample(decode_i16(fs_counts, [c[0], c[1]])),
                    &bins.sample(decode_i16(fs_counts, [c[2], c[3]])),
                );
            }
        }
    }
    acc
}

/// The half that does the work: every accumulator, and the hand-off to the
/// FFT, demod and NET feeds.
///
/// `lost_before` is how many pairs never reached this function between the
/// previous block and this one besides the ones the driver reported: blocks
/// the intake queue had no room for. They are drops like any other, and the
/// feeds downstream are told a gap came before this block.
pub fn digest(
    buf: &[u8],
    geometry: SampleGeometry,
    arrival: &Arrival,
    lost_before: u64,
    ctx: &RxContext,
) {
    let Arrival {
        cal,
        demod_enabled,
        net_enabled,
        centre_hz,
        rate_hz,
        first_pair,
        dropped_pairs,
        now,
    } = *arrival;
    let dropped_pairs = dropped_pairs + lost_before;
    let pairs = buf.len() / geometry.bytes_per_pair();
    let correcting = cal.correcting();
    let acc = accumulate(buf, geometry, cal);

    let Accumulators {
        saturated,
        i_sum,
        q_sum,
        i_sq,
        q_sq,
        iq_cross,
        hist: local_hist,
        signed: local_signed,
        peak: local_peak,
        consts: local_const,
        out: out_buf,
        ..
    } = acc;

    let block_seq: u64;

    // Single brief lock to flush accumulated results - O(1), no loops inside.
    {
        let Ok(mut m) = ctx.metrics.lock() else {
            let taken = ctx.sample_tx.try_send(buf.to_vec()).is_ok();
            ctx.fft_feed.record(ctx.sample_tx.len(), taken);
            return;
        };

        if dropped_pairs > 0 {
            m.acc.drops += dropped_pairs;
            m.signal.total_drops_session += dropped_pairs;
        }

        m.acc.saturated += saturated;
        m.acc.i_sum += i_sum;
        m.acc.q_sum += q_sum;
        m.acc.i_sq_sum += i_sq as u64;
        m.acc.q_sq_sum += q_sq as u64;
        m.acc.iq_cross_sum += iq_cross;
        m.acc.sample_count += pairs as u64;

        for (acc, &local) in m.acc.iq_hist.iter_mut().zip(local_hist.iter()) {
            *acc += local;
        }
        for (acc, &local) in m.acc.adc_signed_hist.iter_mut().zip(local_signed.iter()) {
            *acc += local;
        }
        m.acc.peak_amp = m.acc.peak_amp.max(local_peak);

        if !local_const.is_empty() {
            let cap = crate::state::CONSTELLATION_CAP;
            let excess = m.iq.constellation.len() + local_const.len();
            if excess > cap {
                m.iq.constellation.drain(..excess - cap);
            }
            m.iq.constellation.extend(local_const.iter().copied());
        }

        if let Some(last) = m.acc.last_callback {
            let gap_us = now.duration_since(last).as_micros() as u64;
            m.acc.jitter_sum_us += gap_us;
            m.acc.jitter_sq_sum += gap_us.saturating_mul(gap_us);
            m.acc.jitter_count += 1;
            // Rolling per-callback gap ring for the lab_timing strip chart. Bounded
            // FIFO; the poll task only snapshots it, so it stays continuous across
            // the 200 ms windows the sum/variance accumulators reset on.
            if m.acc.cb_gaps_us.len() >= crate::state::CB_GAP_HISTORY_LEN {
                m.acc.cb_gaps_us.pop_front();
            }
            m.acc.cb_gaps_us.push_back(gap_us);
        }
        m.acc.last_callback = Some(now);
        // Stamped here, inside the lock that already runs, so the demod can tell a
        // contiguous run of blocks from one interrupted by a drop.
        m.demod.block_seq = m.demod.block_seq.wrapping_add(1);
        block_seq = m.demod.block_seq;
    }

    // Forward the corrected stream when a correction is active, else the raw bytes.
    let forward = if correcting { out_buf } else { buf.to_vec() };
    // The demod sees the same corrected stream as the FFT: a residual DC offset
    // would otherwise land straight on the discriminator's carrier-offset reading,
    // since a centre-tuned channel sits exactly on the DC spike.
    //
    // `dropped_pairs` travels with it. The sequence number is stamped in this
    // function, so it can only record a block lost after this point; samples the
    // driver threw away never became a block and leave the numbers consecutive.
    // Without the flag the demod read the run as unbroken straight across the
    // hole, and CTCSS reported a tone measured over the join.
    if demod_enabled {
        ctx.demod_tx
            .try_send(super::StreamBlock {
                seq: block_seq,
                gap_before: dropped_pairs > 0,
                bytes: forward.clone(),
                first_pair,
                centre_hz,
                rate_hz,
            })
            .ok();
    }
    // The third feed, and the one that counts what it could not take. A Wi-Fi
    // frame at 6 Mbps carrying 1500 bytes is 2 ms, which at 20 Msps is 40 000
    // samples and several driver blocks: one block lost in the middle destroys
    // the frame. An unbounded queue would only move that failure into memory, so
    // this drops like the other two - and, unlike the other two, says so.
    if net_enabled {
        let taken = ctx
            .net_tx
            .try_send(super::StreamBlock {
                seq: block_seq,
                gap_before: dropped_pairs > 0,
                bytes: forward.clone(),
                first_pair,
                centre_hz,
                rate_hz,
            })
            .is_ok();
        ctx.net_feed.record(ctx.net_tx.len(), taken);
    }
    // The one point where a lost block is observable. This channel is lossy by
    // design - dropping under load beats blocking the USB callback - but lossy
    // and invisible are different things: it carries no sequence number, so once
    // `try_send` has returned there is nothing downstream that can tell the block
    // ever existed. The depth is read straight after, which can only understate
    // it if the worker drained the queue in between.
    let taken = ctx.sample_tx.try_send(forward).is_ok();
    ctx.fft_feed.record(ctx.sample_tx.len(), taken);
}

/// The running totals one block folds into, and the per-pair body that fills
/// them.
///
/// Split out so the two sample widths share one accumulation instead of two
/// copies that agree only until someone edits one of them. The width branch
/// lives outside the loop, in `process_block`, which is the same discipline
/// `signal::fft::frame` uses: an answer that cannot change within a block is not
/// worth asking thousands of times.
#[derive(Default)]
struct Accumulators {
    geometry: SampleGeometry,
    /// The histograms' bucketing, precomputed once.
    bins: Bins,
    /// Pairs between constellation samples, from this block's own length.
    const_stride: usize,
    /// Pairs until the next constellation sample: a countdown rather than a
    /// remainder, because a remainder is a division every pair.
    until_const: usize,
    full_scale: f32,
    cal: crate::state::IqCalState,
    correcting: bool,

    saturated: u64,
    i_sum: i64,
    q_sum: i64,
    i_sq: i64,
    q_sq: i64,
    iq_cross: i64,
    hist: [u64; 32],
    /// Signed I/Q distribution, the ADC bell.
    signed: [u64; 32],
    /// Loudest |i|,|q| this block.
    peak: u32,
    consts: Vec<(f32, f32)>,
    /// Corrected samples re-encoded for the display path. Empty unless a
    /// correction is live.
    out: Vec<u8>,
}

impl Accumulators {
    /// Fold one decoded I/Q pair in, in the order the block holds them: the
    /// constellation countdown relies on it.
    ///
    /// `always`, because the hint alone left it a call per pair, measured:
    /// every running sum went back to memory between two samples.
    #[inline(always)]
    fn fold(&mut self, si: &Sample, sq: &Sample) {
        let (i, q) = (si.v, sq.v);
        self.i_sum += i;
        self.q_sum += q;
        self.i_sq += i * i;
        self.q_sq += q * q;
        self.iq_cross += i * q;
        self.saturated += u64::from(si.rail) + u64::from(sq.rail);
        // Chebyshev distance over 32 bins. `unsigned_abs` of the centered value
        // can reach full scale itself (the -FS extreme); the bucket is clamped
        // to the last bin (`Bins::amp`) instead of indexing [32] and panicking
        // inside the RX callback.
        self.hist[si.amp_bucket.max(sq.amp_bucket) as usize] += 1;
        // Both on the RAW samples: the physical ADC's-eye view.
        self.peak = self.peak.max(si.abs.max(sq.abs));
        self.signed[si.signed_bucket as usize] += 1;
        self.signed[sq.signed_bucket as usize] += 1;

        // Display path: corrected samples feed the FFT (re-encoded bytes) and the
        // constellation. When no correction is active these equal the raw samples.
        let (ci, cq) = if self.correcting {
            self.cal.apply(i as f32, q as f32)
        } else {
            (i as f32, q as f32)
        };
        if self.correcting {
            encode_into(&mut self.out, ci, cq, &self.geometry);
        }
        // Constellation decimation: one normalised (I, Q) pair per stride, and
        // the stride is chosen so the budget lands evenly across the whole
        // block rather than running out inside it. Frozen ([F]) → stop
        // collecting so the cloud holds its last shape.
        if self.until_const == 0 {
            if !self.cal.frozen {
                self.consts
                    .push((ci / self.full_scale, cq / self.full_scale));
            }
            self.until_const = self.const_stride;
        }
        self.until_const -= 1;
    }
}

/// Re-encode one corrected (I, Q) sample back to the wire byte format, clamping
/// to the device's own range. Used only when a correction is active.
fn encode_into(out: &mut Vec<u8>, i: f32, q: f32, g: &SampleGeometry) {
    let hi = g.full_scale - 1.0;
    let lo = -g.full_scale;
    let ci = i.round().clamp(lo, hi) as i32;
    let cq = q.round().clamp(lo, hi) as i32;
    match g.format {
        SampleFormat::Int8 => {
            out.push(ci as i8 as u8);
            out.push(cq as i8 as u8);
        }
        SampleFormat::Uint8 => {
            out.push((ci + 128) as u8);
            out.push((cq + 128) as u8);
        }
        SampleFormat::Int16 => {
            out.extend_from_slice(&(ci as i16).to_le_bytes());
            out.extend_from_slice(&(cq as i16).to_le_bytes());
        }
    }
}

/// Whether a centered sample sits on the converter's own rail.
///
/// **Against the declared full scale, not against the container.** A twelve-bit
/// converter handing over sixteen-bit words rails at 2047 counts and never comes
/// near 32767, so a test written against `i16::MAX` cannot fire on one - and
/// every SoapySDR radio that reports `CS16` with a full scale below 32768 is
/// exactly that. The saturation reading stayed at 0.00 % for the whole session
/// while the signed histogram was slammed into its top bucket and the peak read
/// 0 dBFS: three accounts of one sample, and only this one said the front end
/// was fine.
///
/// `full_scale - 1` on the positive side and `-full_scale` on the negative,
/// which is what two's complement gives at every width: 127 / -128 at eight
/// bits, 2047 / -2048 at twelve, 32767 / -32768 at sixteen.
///
/// A full scale below one count locates no rail at all, and a clip cannot be
/// asserted against a rail nobody can find. `soapy::caps::geometry_for` refuses
/// such a scale before it can reach here and both native backends report 128, so
/// this is the unreachable case declining to answer rather than calling every
/// sample in the block a clip.
#[inline]
fn on_rail(full_scale: i64, v: i64) -> bool {
    full_scale >= 1 && (v >= full_scale - 1 || v <= -full_scale)
}

/// Decode one raw byte of an 8-bit format into a centered signed value in
/// [-128, 127], and say whether it sits on a rail.
///
/// The two formats differ only here: HackRF sends `Int8`, RTL-SDR `Uint8` whose
/// analogue zero sits between code 127 and code 128. Centering by 128 rather
/// than by 127.5 is what keeps this an integer path, and every accumulator below
/// it integer with it: a half-count offset would have to be carried in floats
/// through the whole per-sample loop to buy back a shift that is the same for
/// every sample in it.
///
/// It is **not** negligible where it lands, though, whatever this comment used
/// to say. Half an LSB is 0.0039 of full scale, which is 78 % of the DC offset
/// the IQ bench warns at and a -45 dBFS floor under a spike it grades from -40:
/// no RTL-SDR could read better than that however clean its front end. Nothing
/// here changes, because the fix costs nothing where the arithmetic is already
/// floating point - see [`SampleGeometry::centre_bias`] and the one place that
/// adds it back, `tasks::rx::metrics::iq_metrics`.
///
/// `#[inline]` because this runs twice per sample in the RX callback.
#[inline]
fn decode(format: SampleFormat, full_scale: i64, b: u8) -> (i64, bool) {
    let v = match format {
        SampleFormat::Int8 => b as i8 as i64,
        SampleFormat::Uint8 => b as i64 - 128,
        // Unreachable: `process_block` sends 16-bit blocks down the other arm,
        // where a pair is four bytes and one byte on its own means nothing.
        SampleFormat::Int16 => return (0, false),
    };
    (v, on_rail(full_scale, v))
}

/// Decode one little-endian signed 16-bit component, and say whether it sits on
/// a rail.
///
/// Little endian because that is what `SOAPY_SDR_CS16` is on every platform
/// sdrtop runs on. Getting the byte order wrong here does not crash: it produces
/// a spectrum that looks plausible and is wrong, which is the hardest kind of
/// bug to notice, so the test asserts against a literal byte pair rather than
/// against another expression that could be wrong the same way.
#[inline]
fn decode_i16(full_scale: i64, bytes: [u8; 2]) -> (i64, bool) {
    let v = i16::from_le_bytes(bytes) as i64;
    (v, on_rail(full_scale, v))
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    use super::{SampleFormat, SampleGeometry};
    use crate::hardware::{RxContext, StreamBlock};
    use crate::state::SdrMetrics;

    // These exercise the decode/saturation/histogram arithmetic that
    // `process_block` performs inline; constructing a full RxContext is left to
    // the hardware-in-the-loop verification.

    /// A live `RxContext` with the demod switched on, and the two receiving ends.
    ///
    /// The module comment used to say building one of these was left to the
    /// hardware-in-the-loop verification. It is not: every field is plain data
    /// and two channels, so what `process_block` forwards can be read back here
    /// with no radio anywhere.
    /// Both receivers come back so the caller holds them: a dropped receiver
    /// disconnects its channel and every `try_send` after that silently fails,
    /// which is the same shape as the bug being tested for.
    pub(crate) fn rx_ctx() -> (
        Arc<RxContext>,
        crossbeam_channel::Receiver<Vec<u8>>,
        crossbeam_channel::Receiver<StreamBlock>,
        crossbeam_channel::Receiver<StreamBlock>,
    ) {
        rx_ctx_holding(8, 8)
    }

    /// The same, with the FFT queue's depth chosen: a shallow one can be filled
    /// in a test, which is how the lossy hand-off is exercised without needing a
    /// slow FFT worker to be behind.
    fn rx_ctx_holding(
        fft_cap: usize,
        net_cap: usize,
    ) -> (
        Arc<RxContext>,
        crossbeam_channel::Receiver<Vec<u8>>,
        crossbeam_channel::Receiver<StreamBlock>,
        crossbeam_channel::Receiver<StreamBlock>,
    ) {
        let (sample_tx, sample_rx) = crossbeam_channel::bounded(fft_cap);
        let (demod_tx, demod_rx) = crossbeam_channel::bounded(8);
        let (net_tx, net_rx) = crossbeam_channel::bounded(net_cap);
        let (power_tx, _) = crossbeam_channel::bounded(1);
        let mut m = SdrMetrics::fixture();
        m.demod.enabled = true;
        m.ui.section = crate::signal::net::SECTION.to_string();
        let ctx = RxContext {
            metrics: Arc::new(Mutex::new(m)),
            sample_tx,
            fft_feed: crate::hardware::FeedHealth::default(),
            demod_tx,
            net_tx,
            net_feed: crate::hardware::FeedHealth::default(),
            power_tx,
            geometry: eight_bit(),
            stream_pairs: std::sync::atomic::AtomicU64::new(0),
            record: Default::default(),
            intake: Default::default(),
        };
        (Arc::new(ctx), sample_rx, demod_rx, net_rx)
    }

    /// **The recorder gets the raw bytes**, even while the other feeds carry
    /// a DC-blocked stream, stamped with the block's place in the stream and
    /// the gain set when it arrived. Idle, it gets nothing.
    #[test]
    fn the_recorder_gets_the_raw_block_with_its_place_and_gain() {
        use crate::hardware::record_tap::RecordMsg;
        let (ctx, sample_rx, _demod_rx, _net_rx) = rx_ctx();
        {
            let mut m = ctx.metrics.lock().unwrap();
            m.iq.cal.dc_block_on = true;
            m.iq.cal.dc_i_raw = 10.0;
            m.radio.gains = vec![24.0, 30.0];
            m.radio.amp_enabled = true;
        }
        let block: Vec<u8> = (0..64u8).collect();
        super::process_block(&block, eight_bit(), 0, &ctx, Instant::now());
        let (tx, rx) = crossbeam_channel::unbounded();
        ctx.record.arm(tx);
        super::process_block(&block, eight_bit(), 7, &ctx, Instant::now());
        match rx.try_recv().unwrap() {
            RecordMsg::Block {
                at,
                bytes,
                gains,
                boost,
            } => {
                assert_eq!(bytes, block, "the raw bytes, not the corrected ones");
                assert_eq!(
                    (at.first_pair, at.pairs, at.driver_dropped),
                    (32 + 7, 32, 7)
                );
                assert_eq!((gains, boost), (vec![24.0, 30.0], true));
            }
            RecordMsg::Refused { .. } | RecordMsg::Radio(_) => {
                panic!("an empty queue refused a block")
            }
        }
        assert!(rx.try_recv().is_err(), "the idle block was not recorded");
        let forwarded = sample_rx.try_iter().last().unwrap();
        assert_ne!(
            forwarded, block,
            "the FFT feed still sees the corrected stream"
        );
    }

    /// The per-sample fold as it was before the bins became shifts and the
    /// constellation stride a countdown: four divisions a pair. Kept as the
    /// reference the fast one is held to, field by field.
    fn reference_accumulate(
        buf: &[u8],
        geometry: SampleGeometry,
        cal: crate::state::IqCalState,
    ) -> super::Accumulators {
        let format = geometry.format;
        let fs = geometry.full_scale as i64;
        let pairs = buf.len() / geometry.bytes_per_pair();
        let stride = super::const_stride(pairs);
        let amp_width = super::amp_bin_width(fs);
        let mut a = super::Accumulators {
            geometry,
            full_scale: geometry.full_scale,
            const_stride: stride,
            correcting: cal.correcting(),
            cal,
            ..super::Accumulators::default()
        };
        let pair_at = |k: usize| match format {
            SampleFormat::Int16 => {
                let c = &buf[k * 4..k * 4 + 4];
                (
                    super::decode_i16(fs, [c[0], c[1]]),
                    super::decode_i16(fs, [c[2], c[3]]),
                )
            }
            _ => (
                super::decode(format, fs, buf[k * 2]),
                super::decode(format, fs, buf[k * 2 + 1]),
            ),
        };
        for idx in 0..pairs {
            let ((i, i_sat), (q, q_sat)) = pair_at(idx);
            a.i_sum += i;
            a.q_sum += q;
            a.i_sq += i * i;
            a.q_sq += q * q;
            a.iq_cross += i * q;
            a.saturated += u64::from(i_sat) + u64::from(q_sat);
            let amp = i.unsigned_abs().max(q.unsigned_abs());
            a.hist[((amp / amp_width) as usize).min(31)] += 1;
            a.peak = a.peak.max(amp as u32);
            a.signed[super::signed_bin(&geometry, i)] += 1;
            a.signed[super::signed_bin(&geometry, q)] += 1;
            let (ci, cq) = if a.correcting {
                cal.apply(i as f32, q as f32)
            } else {
                (i as f32, q as f32)
            };
            if a.correcting {
                super::encode_into(&mut a.out, ci, cq, &geometry);
            }
            if !cal.frozen && idx.is_multiple_of(stride) {
                a.consts.push((ci / a.full_scale, cq / a.full_scale));
            }
        }
        a
    }

    fn assert_same(what: &str, fast: &super::Accumulators, reference: &super::Accumulators) {
        assert_eq!(
            (
                fast.saturated,
                fast.i_sum,
                fast.q_sum,
                fast.i_sq,
                fast.q_sq,
                fast.iq_cross
            ),
            (
                reference.saturated,
                reference.i_sum,
                reference.q_sum,
                reference.i_sq,
                reference.q_sq,
                reference.iq_cross
            ),
            "{what}: sums"
        );
        assert_eq!(fast.hist, reference.hist, "{what}: amplitude histogram");
        assert_eq!(fast.signed, reference.signed, "{what}: signed histogram");
        assert_eq!(fast.peak, reference.peak, "{what}: peak");
        assert_eq!(fast.consts, reference.consts, "{what}: constellation");
        assert_eq!(fast.out, reference.out, "{what}: corrected stream");
    }

    /// **Bit for bit what the divisions gave**, on every input that matters:
    /// every one of the 65 536 byte pairs in both 8-bit formats, 16-bit
    /// blocks at a power-of-two full scale and at one that is not (the
    /// division path), samples past both rails, a live correction, a frozen
    /// constellation, and block lengths that do and do not divide by the
    /// stride.
    #[test]
    fn the_fast_fold_is_the_reference_fold_bit_for_bit() {
        let every_pair: Vec<u8> = (0..=255u8)
            .flat_map(|a| (0..=255u8).flat_map(move |b| [a, b]))
            .collect();
        let mut rng = crate::signal::dsp::testkit::Rng::new(11);
        let wide: Vec<u8> = (0..40_001)
            .flat_map(|_| {
                let v = ((rng.unit() * 2.0 - 1.0) * 40_000.0) as i16;
                v.to_le_bytes()
            })
            .collect();
        let correcting = crate::state::IqCalState {
            dc_block_on: true,
            dc_i_raw: 3.5,
            dc_q_raw: -1.25,
            ..Default::default()
        };
        let frozen = crate::state::IqCalState {
            frozen: true,
            ..Default::default()
        };
        let cases = [
            ("int8", SampleFormat::Int8, 128.0, &every_pair),
            ("uint8", SampleFormat::Uint8, 128.0, &every_pair),
            ("int16 at 2048", SampleFormat::Int16, 2048.0, &wide),
            ("int16 at 1000", SampleFormat::Int16, 1000.0, &wide),
            ("int16 at 32768", SampleFormat::Int16, 32768.0, &wide),
        ];
        for (name, format, full_scale, buf) in cases {
            let g = SampleGeometry { format, full_scale };
            for cal in [crate::state::IqCalState::default(), correcting, frozen] {
                for len in [buf.len(), buf.len() / 3 * 2] {
                    let len = len - len % g.bytes_per_pair();
                    let block = &buf[..len];
                    assert_same(
                        name,
                        &super::accumulate(block, g, cal),
                        &reference_accumulate(block, g, cal),
                    );
                }
            }
        }
    }

    /// **What one HackRF block costs to process, measured.** Not a check:
    /// run it by hand, in release, on the machine the question is about
    /// (`cargo test --release what_a_block_costs -- --ignored --nocapture`).
    /// A 131 072-pair block of noise through `process_block` alone, the Lab
    /// feeds only; at 20 Msps the block lasts 6.55 ms, and the callback that
    /// cannot keep inside that loses samples.
    #[test]
    #[ignore]
    fn what_a_block_costs() {
        let (ctx, sample_rx, _d, _n) = rx_ctx();
        ctx.metrics.lock().unwrap().demod.enabled = false;
        ctx.metrics.lock().unwrap().ui.section = "lab".to_string();
        let mut rng = crate::signal::dsp::testkit::Rng::new(3);
        let block: Vec<u8> = rng
            .noise(131_072, 0.02)
            .iter()
            .flat_map(|z| [(z.re * 128.0) as i8 as u8, (z.im * 128.0) as i8 as u8])
            .collect();
        let cal = crate::state::IqCalState::default();
        let (mut old, mut new, mut whole) = (0.0, 0.0, 0.0);
        const ROUNDS: usize = 200;
        // Interleaved, so a machine that is busier for a moment is busier
        // for all three.
        for _ in 0..ROUNDS {
            let t = Instant::now();
            std::hint::black_box(reference_accumulate(&block, eight_bit(), cal));
            old += t.elapsed().as_secs_f64();
            let t = Instant::now();
            std::hint::black_box(super::accumulate(&block, eight_bit(), cal));
            new += t.elapsed().as_secs_f64();
            let t = Instant::now();
            super::process_block(&block, eight_bit(), 0, &ctx, Instant::now());
            whole += t.elapsed().as_secs_f64();
            while sample_rx.try_recv().is_ok() {}
        }
        let us = |s: f64| s / ROUNDS as f64 * 1e6;
        eprintln!(
            "per 131072-pair block: fold as it was {:.0} us, fold now {:.0} us, process_block now {:.0} us (a block lasts 6554 us at 20 Msps)",
            us(old),
            us(new),
            us(whole)
        );
    }

    /// The block the FFT feed could not take is counted.
    ///
    /// The channel is lossy by design and carries no sequence number, so this is
    /// the only point in the program where the loss is observable at all: after
    /// the `try_send` returns the block is simply gone, and nothing downstream
    /// can tell it ever existed. It used to be discarded with an `.ok()`, and the
    /// spectrum's frame rate halved under load with every panel reporting a
    /// comfortable buffer.
    #[test]
    fn a_block_the_fft_feed_cannot_take_is_counted() {
        let (ctx, _sample_rx, _demod_rx, _net_rx) = rx_ctx_holding(1, 8);
        let block = vec![0x10u8; 64];

        super::process_block(&block, eight_bit(), 0, &ctx, Instant::now());
        let (depth, dropped) = ctx.fft_feed.take();
        assert_eq!((depth, dropped), (1, 0), "the queue took it");

        // The worker has not drained it, so the next block has nowhere to go.
        super::process_block(&block, eight_bit(), 0, &ctx, Instant::now());
        let (depth, dropped) = ctx.fft_feed.take();
        assert_eq!(dropped, 1, "a block the spectrum never saw");
        assert_eq!(depth, 1, "and the queue was full when it happened");
    }

    /// A section nobody is looking at costs nothing on the hot path.
    ///
    /// The gate is the section on screen rather than a switch of its own,
    /// because the section is the feed's only consumer. Forwarding costs a full
    /// copy of every block inside the USB callback, and paying it for a preset
    /// the user has not opened is exactly the kind of cost that is invisible
    /// until it is a dropped frame.
    #[test]
    fn a_section_nobody_is_looking_at_is_never_fed() {
        let (ctx, _sample_rx, _demod_rx, net_rx) = rx_ctx();
        {
            let mut m = ctx.metrics.lock().unwrap();
            m.ui.section = "lab".to_string();
        }
        super::process_block(&[0x10u8; 64], eight_bit(), 0, &ctx, Instant::now());
        assert!(net_rx.is_empty(), "nothing forwarded, and nothing copied");
        let (depth, dropped) = ctx.net_feed.take();
        assert_eq!(
            (depth, dropped),
            (0, 0),
            "and no refusal either: a closed gate is not a loss"
        );

        // Open it, and the same block arrives with its continuity facts.
        {
            let mut m = ctx.metrics.lock().unwrap();
            m.ui.section = crate::signal::net::SECTION.to_string();
        }
        super::process_block(&[0x10u8; 64], eight_bit(), 0, &ctx, Instant::now());
        let block = net_rx.try_recv().expect("one block");
        assert!(!block.gap_before);
        assert_eq!(block.bytes.len(), 64);
    }

    /// Every block carries where it sits in the stream and the tuning it was
    /// captured at, and the position counts what the driver lost as well as
    /// what arrived - so two positions alone say whether blocks are
    /// contiguous. A new stream starts again from zero.
    #[test]
    fn blocks_carry_their_stream_position_and_capture_tuning() {
        let (ctx, _sample_rx, _demod_rx, net_rx) = rx_ctx_holding(8, 8);
        {
            let mut m = ctx.metrics.lock().unwrap();
            m.radio.frequency = 2_402_000_000;
            m.radio.config_sample_rate = 4_000_000.0;
        }
        // 32 pairs, then 32 more after the driver reports 10 lost.
        super::process_block(&[0u8; 64], eight_bit(), 0, &ctx, Instant::now());
        super::process_block(&[0u8; 64], eight_bit(), 10, &ctx, Instant::now());
        let a = net_rx.try_recv().unwrap();
        let b = net_rx.try_recv().unwrap();
        assert_eq!(a.first_pair, 0);
        assert_eq!(
            b.first_pair,
            32 + 10,
            "past the first block and the lost pairs"
        );
        assert_eq!((a.centre_hz, a.rate_hz), (2_402_000_000, 4_000_000.0));

        // Retuned between blocks: each carries the tuning it was captured at.
        ctx.metrics.lock().unwrap().radio.frequency = 2_480_000_000;
        super::process_block(&[0u8; 64], eight_bit(), 0, &ctx, Instant::now());
        let c = net_rx.try_recv().unwrap();
        assert_eq!(c.centre_hz, 2_480_000_000);
        assert_eq!(c.first_pair, 32 + 10 + 32, "and the stream carried on");

        // A restarted stream counts from zero again.
        ctx.begin_stream();
        super::process_block(&[0u8; 64], eight_bit(), 0, &ctx, Instant::now());
        assert_eq!(net_rx.try_recv().unwrap().first_pair, 0);
    }

    /// The block the NET feed could not take is counted.
    ///
    /// The same shape as the FFT feed's test above and for the same reason,
    /// with one difference that matters: this number is testimony rather than
    /// diagnostics. A Wi-Fi frame spans several driver blocks, so a refusal
    /// here is a frame nobody will ever see, and a panel that reported the
    /// frames it decoded without reporting these would be presenting a lower
    /// bound as a total.
    #[test]
    fn a_block_the_net_feed_cannot_take_is_counted() {
        let (ctx, _sample_rx, _demod_rx, _net_rx) = rx_ctx_holding(8, 1);
        let block = vec![0x10u8; 64];

        super::process_block(&block, eight_bit(), 0, &ctx, Instant::now());
        assert_eq!(ctx.net_feed.take(), (1, 0), "the queue took it");

        // Nothing has drained it, so the next block has nowhere to go.
        let (depth, dropped) = {
            super::process_block(&block, eight_bit(), 0, &ctx, Instant::now());
            ctx.net_feed.take()
        };
        assert_eq!(dropped, 1, "a block no decoder will ever see");
        assert_eq!(depth, 1, "and the queue was full when it happened");
    }

    /// The stride follows the block, so the budget reaches the end of it.
    ///
    /// The two smaller backends never spent the budget and must not move; the
    /// HackRF's transfer is the one that did.
    #[test]
    fn the_stride_spreads_the_budget_over_whatever_block_arrives() {
        assert_eq!(super::const_stride(16_384), 1_024, "a SoapySDR read");
        assert_eq!(super::const_stride(32_768), 1_024, "an RTL-SDR transfer");
        assert_eq!(super::const_stride(131_072), 2_048, "a HackRF transfer");
        assert_eq!(super::const_stride(0), 1_024, "an empty overflow block");
    }

    /// Whatever the block, the budget is spent and not overspent: every point
    /// costs the lock a copy, and the last one has to be inside the block.
    #[test]
    fn every_block_size_lands_inside_its_budget() {
        for pairs in [
            1_usize, 1_024, 16_384, 32_768, 65_536, 131_072, 262_144, 131_073,
        ] {
            let stride = super::const_stride(pairs);
            let taken = (0..pairs).filter(|i| i.is_multiple_of(stride)).count();
            assert!(
                taken <= super::CONST_MAX_PER_BLOCK,
                "{pairs} pairs took {taken} points"
            );
            assert!(stride >= super::CONST_DECIMATE, "{pairs} pairs: {stride}");
        }
    }

    /// End to end: the cloud is drawn from the whole transfer.
    ///
    /// The two halves of the block tell themselves apart by amplitude, so what
    /// lands in the ring says which part of the transfer it came from. With a
    /// fixed stride and a budget that stopped at 64, the answer was always "the
    /// first half" - and the ellipse, EVM and MER fitted to the cloud inherited
    /// that blind spot.
    #[test]
    fn the_cloud_is_drawn_from_the_whole_transfer_and_not_just_its_start() {
        let (ctx, _fft_rx, _demod_rx, _net_rx) = rx_ctx();
        const PAIRS: usize = 131_072; // one HackRF transfer
        let mut block = vec![0u8; PAIRS * 2];
        for (idx, pair) in block.as_chunks_mut::<2>().0.iter_mut().enumerate() {
            pair[0] = if idx < PAIRS / 2 { 10 } else { 100 };
        }

        super::process_block(&block, eight_bit(), 0, &ctx, Instant::now());

        let m = ctx.metrics.lock().unwrap();
        let cloud = &m.iq.constellation;
        assert_eq!(cloud.len(), 64, "the budget is spent, not overspent");
        let late = cloud.iter().filter(|(i, _)| *i > 0.5).count();
        assert_eq!(
            late, 32,
            "half the points belong to the second half of the transfer"
        );
    }

    /// The two halves of the correction split, which prose alone used to assert
    /// and two comments used to assert wrongly.
    ///
    /// A correction has to be built from what the front end actually did, so the
    /// per-sample sums are taken before it. If they ever moved onto the corrected
    /// stream the bench would measure the app's own arithmetic, report a
    /// perfectly balanced radio, and have nothing left to build the next
    /// correction from.
    #[test]
    fn the_accumulators_stay_on_the_raw_stream_while_a_correction_runs() {
        let (ctx, _fft_rx, _demod_rx, _net_rx) = rx_ctx();
        {
            let mut m = ctx.metrics.lock().unwrap();
            m.iq.cal = crate::state::IqCalState {
                dc_block_on: true,
                dc_i_raw: 10.0,
                dc_q_raw: -5.0,
                ..crate::state::IqCalState::default()
            };
        }
        // Four pairs of (i = 20, q = -30). The correction would make them
        // (10, -25), which is what the sums must *not* say.
        let block: Vec<u8> = std::iter::repeat_n([20u8, (-30i8) as u8], 4)
            .flatten()
            .collect();

        super::process_block(&block, eight_bit(), 0, &ctx, Instant::now());

        let m = ctx.metrics.lock().unwrap();
        assert_eq!(m.acc.i_sum, 80, "four raw 20s, not four corrected 10s");
        assert_eq!(m.acc.q_sum, -120, "four raw -30s, not four corrected -25s");
    }

    /// And the other half: the corrected stream is what leaves for the FFT, and
    /// the demod gets the same one rather than the raw bytes.
    ///
    /// The FFT is not a display. Every spectrum measurement the app makes is
    /// derived from it, and a residual DC offset would land straight on the
    /// demod's carrier-offset reading, since a centre-tuned channel sits exactly
    /// on the DC spike.
    #[test]
    fn the_corrected_stream_is_what_the_fft_and_the_demod_receive() {
        let (ctx, fft_rx, demod_rx, _net_rx) = rx_ctx();
        {
            let mut m = ctx.metrics.lock().unwrap();
            m.iq.cal = crate::state::IqCalState {
                dc_block_on: true,
                dc_i_raw: 10.0,
                dc_q_raw: -5.0,
                ..crate::state::IqCalState::default()
            };
        }
        let block: Vec<u8> = std::iter::repeat_n([20u8, (-30i8) as u8], 4)
            .flatten()
            .collect();
        let corrected: Vec<u8> = std::iter::repeat_n([10u8, (-25i8) as u8], 4)
            .flatten()
            .collect();

        super::process_block(&block, eight_bit(), 0, &ctx, Instant::now());

        assert_eq!(fft_rx.recv().expect("no FFT block"), corrected);
        assert_eq!(
            demod_rx.recv().expect("no demod block").bytes,
            corrected,
            "the demod must see the same stream the FFT does"
        );
    }

    /// The block a driver-side loss precedes is marked as such.
    ///
    /// This is the fact the sequence number cannot carry. It is stamped inside
    /// this function, so it counts blocks that reached it; samples the driver
    /// threw away never became a block and leave no gap in the numbers. Both
    /// backends have a way to lose them - a HackRF short transfer
    /// (`valid_length < buffer_length`) and a SoapySDR overflow - and both report
    /// it through `dropped_pairs`, which until now went only to the drop counter.
    #[test]
    fn a_driver_side_loss_marks_the_forwarded_demod_block() {
        let (ctx, _fft_rx, demod_rx, _net_rx) = rx_ctx();
        let block = vec![0u8; 8]; // four Int8 pairs, contents irrelevant

        super::process_block(&block, eight_bit(), 0, &ctx, Instant::now());
        let clean = demod_rx.recv().expect("the first block was not forwarded");
        assert!(!clean.gap_before, "nothing was lost before this one");

        super::process_block(&block, eight_bit(), 64, &ctx, Instant::now());
        let after_loss = demod_rx.recv().expect("the second block was not forwarded");
        assert!(
            after_loss.gap_before,
            "the driver reported 64 lost pairs and the demod was not told"
        );
        assert_eq!(
            after_loss.seq,
            clean.seq + 1,
            "the numbers stay consecutive across the loss, which is exactly why \
             the flag has to exist"
        );
    }

    /// A SoapySDR overflow forwards nothing but the fact of the loss.
    ///
    /// The overflow path has no samples to hand over - it calls `process_block`
    /// with an empty slice purely so the drop is counted. That block still has to
    /// break the run, or the audio either side of the overflow is spliced.
    #[test]
    fn an_empty_overflow_block_still_carries_the_gap() {
        let (ctx, _fft_rx, demod_rx, _net_rx) = rx_ctx();
        super::process_block(&[], eight_bit(), 16_384, &ctx, Instant::now());
        let b = demod_rx.recv().expect("the overflow was not forwarded");
        assert!(b.bytes.is_empty());
        assert!(b.gap_before, "an overflow is a gap by definition");
    }

    // --- Int8 (HackRF) decode -------------------------------------------------
    #[test]
    fn int8_flags_both_rails_and_nothing_between() {
        // This used to assert `0x7F == 0x7F || 0x7F == 0x80`, which is true of
        // any program. It now asks `decode` itself.
        for b in [0x7Fu8, 0x80] {
            assert!(
                super::decode(SampleFormat::Int8, 128, b).1,
                "{b:#04x} is a rail"
            );
        }
        for b in [0x00u8, 0x40, 0x7E, 0x81, 0xC0] {
            assert!(
                !super::decode(SampleFormat::Int8, 128, b).1,
                "{b:#04x} is not a rail"
            );
        }
    }

    /// An 8-bit geometry, which is what both shipped radios report.
    pub(crate) fn eight_bit() -> SampleGeometry {
        SampleGeometry {
            format: SampleFormat::Int8,
            full_scale: 128.0,
        }
    }

    #[test]
    fn signed_bin_maps_rails_and_centre() {
        let g = eight_bit();
        assert_eq!(super::signed_bin(&g, -128), 0, "−FS rail → bin 0");
        assert_eq!(super::signed_bin(&g, 0), 16, "mid-scale → centre bin");
        assert_eq!(super::signed_bin(&g, 127), 31, "+FS rail → top bin");
        // Clamps out-of-range without panicking on the array index.
        assert_eq!(super::signed_bin(&g, 200), 31);
        assert_eq!(super::signed_bin(&g, -200), 0);
    }

    /// The bin widths this file used to hardcode, now derived, must come out
    /// identical for 8 bits. If someone later "tidies" full_scale to the RTL's
    /// true 127.5 bias, this is what fails and says why.
    #[test]
    fn eight_bit_geometry_reproduces_the_old_constants() {
        assert_eq!(
            super::bin_width(128),
            8,
            "the signed histogram was (v+128)/8"
        );
        assert_eq!(super::amp_bin_width(128), 4, "the amplitude one was amp/4");
    }

    /// The same arithmetic on a wider converter still puts the rails in the end
    /// bins and mid-scale in the middle. Exercised before any device reports it,
    /// because the alternative is finding out from a stranger's screenshot.
    #[test]
    fn a_wider_converter_bins_the_same_way() {
        let g = SampleGeometry {
            format: SampleFormat::Int8,
            full_scale: 32768.0,
        };
        assert_eq!(super::signed_bin(&g, -32768), 0);
        assert_eq!(super::signed_bin(&g, 0), 16);
        assert_eq!(super::signed_bin(&g, 32767), 31);
    }

    // --- Int16 (SoapySDR CS16) decode ----------------------------------------
    /// Byte order, asserted against literal bytes rather than against another
    /// expression that could be wrong the same way. A swapped decoder produces
    /// a spectrum that looks plausible and is wrong, which is the hardest kind
    /// of mistake to spot on a screen.
    #[test]
    fn int16_is_little_endian() {
        assert_eq!(
            super::decode_i16(32768, [0x00, 0x01]).0,
            256,
            "low byte first"
        );
        assert_eq!(super::decode_i16(32768, [0x01, 0x00]).0, 1);
        assert_eq!(
            super::decode_i16(32768, [0xFF, 0xFF]).0,
            -1,
            "two's complement"
        );
        assert_eq!(super::decode_i16(32768, [0x00, 0x80]).0, -32768);
    }

    /// The rail is the converter's, not the container's.
    ///
    /// An Airspy R2 through SoapySDR reports `CS16` with a full scale of 2048:
    /// twelve bits handed over in sixteen-bit words. Its ADC rails at 2047
    /// counts and can never reach 32767, so a clip test written against
    /// `i16::MAX` cannot fire on one - and the same is true of every 12- and
    /// 14-bit radio this backend exists for. The saturation reading sat at
    /// 0.00 % for the whole session while the front end was slamming its rails.
    #[test]
    fn a_twelve_bit_converter_clips_at_its_own_rail() {
        // The converter pinned at each rail in turn.
        for v in [2047i16, -2048] {
            assert!(
                super::decode_i16(2048, v.to_le_bytes()).1,
                "{v} is the rail of a converter whose full scale is 2048"
            );
        }
        // One count inside either rail is not clipping, at this scale as at any.
        for v in [2046i16, -2047, 0] {
            assert!(
                !super::decode_i16(2048, v.to_le_bytes()).1,
                "{v} is not a rail"
            );
        }
    }

    /// The three readings of one fact must agree at every declared scale.
    ///
    /// The clip flag, the signed histogram and the peak level are three accounts
    /// of the same sample, and they were taken against two different notions of
    /// full scale: the histogram and the peak followed the geometry, the clip
    /// flag followed the container. A sample can sit in the histogram's top
    /// bucket, read 0 dBFS, and report as unclipped - which is what the ADC bench
    /// showed on every 12-bit radio.
    #[test]
    fn the_clip_flag_agrees_with_the_histogram_and_the_peak() {
        for (format, fs) in [
            (SampleFormat::Int8, 128i64),
            (SampleFormat::Int16, 2048),
            (SampleFormat::Int16, 32768),
        ] {
            let g = SampleGeometry {
                format,
                full_scale: fs as f32,
            };
            let flagged = |v: i64| match format {
                SampleFormat::Int16 => super::decode_i16(fs, (v as i16).to_le_bytes()).1,
                _ => super::decode(format, fs, v as i8 as u8).1,
            };
            // The positive rail: top histogram bucket, 0 dBFS, and clipping.
            assert_eq!(super::signed_bin(&g, fs - 1), 31, "{format:?}/{fs}");
            assert!(
                (20.0 * ((fs - 1) as f32 / fs as f32).log10()).abs() < 0.1,
                "{format:?}/{fs}: the peak reading does not call this full scale"
            );
            assert!(
                flagged(fs - 1),
                "{format:?}/{fs}: +rail must read as clipping"
            );
            // The negative rail, the same three ways.
            assert_eq!(super::signed_bin(&g, -fs), 0, "{format:?}/{fs}");
            assert!(flagged(-fs), "{format:?}/{fs}: -rail must read as clipping");
            // Mid-scale is none of those things.
            assert_eq!(super::signed_bin(&g, 0), 16, "{format:?}/{fs}");
            assert!(!flagged(0), "{format:?}/{fs}: mid-scale is not clipping");
        }
    }

    #[test]
    fn int16_flags_both_rails_and_nothing_inside_them() {
        assert!(super::decode_i16(32768, [0xFF, 0x7F]).1, "+32767 is a rail");
        assert!(super::decode_i16(32768, [0x00, 0x80]).1, "-32768 is a rail");
        // One count inside either rail is not clipping.
        assert!(!super::decode_i16(32768, [0xFE, 0x7F]).1);
        assert!(!super::decode_i16(32768, [0x01, 0x80]).1);
        assert!(!super::decode_i16(32768, [0x00, 0x00]).1);
    }

    /// A 16-bit pair is four bytes, so a block holds half as many pairs as an
    /// 8-bit block of the same length. Getting this wrong scales every
    /// throughput and drop reading by two.
    #[test]
    fn a_sixteen_bit_pair_is_four_bytes() {
        let g = SampleGeometry {
            format: SampleFormat::Int16,
            full_scale: 32768.0,
        };
        assert_eq!(g.bytes_per_pair(), 4);
        assert_eq!(
            1024 / g.bytes_per_pair(),
            256,
            "1 KiB is 256 pairs, not 512"
        );
    }

    /// The signed histogram against a 16-bit full scale puts the rails in the
    /// end bins, exactly as it does at 8.
    #[test]
    fn int16_bins_across_its_own_full_scale() {
        let g = SampleGeometry {
            format: SampleFormat::Int16,
            full_scale: 32768.0,
        };
        assert_eq!(super::signed_bin(&g, -32768), 0);
        assert_eq!(super::signed_bin(&g, 0), 16);
        assert_eq!(super::signed_bin(&g, 32767), 31);
    }

    /// Re-encoding a corrected sample round-trips through the wire format. The
    /// 16-bit path writes four bytes where the 8-bit ones write two, and a
    /// mismatch here would desynchronise the whole display stream by a byte.
    #[test]
    fn encoding_round_trips_at_both_widths() {
        let wide = SampleGeometry {
            format: SampleFormat::Int16,
            full_scale: 32768.0,
        };
        let mut out = Vec::new();
        super::encode_into(&mut out, 1234.0, -5678.0, &wide);
        assert_eq!(out.len(), 4, "one 16-bit pair is four bytes");
        assert_eq!(super::decode_i16(32768, [out[0], out[1]]).0, 1234);
        assert_eq!(super::decode_i16(32768, [out[2], out[3]]).0, -5678);

        let narrow = eight_bit();
        out.clear();
        super::encode_into(&mut out, 100.0, -100.0, &narrow);
        assert_eq!(out.len(), 2);
        assert_eq!(super::decode(SampleFormat::Int8, 128, out[0]).0, 100);
    }

    /// Clamping follows the declared full scale, so a correction that overshoots
    /// lands on the rail instead of wrapping to the opposite one.
    #[test]
    fn encoding_clamps_rather_than_wrapping() {
        let wide = SampleGeometry {
            format: SampleFormat::Int16,
            full_scale: 32768.0,
        };
        let mut out = Vec::new();
        super::encode_into(&mut out, 90_000.0, -90_000.0, &wide);
        assert_eq!(super::decode_i16(32768, [out[0], out[1]]).0, 32767);
        assert_eq!(super::decode_i16(32768, [out[2], out[3]]).0, -32768);
    }

    /// A driver reporting a tiny full scale must not divide by zero inside the
    /// RX callback, which is the one place in the program that cannot afford it.
    #[test]
    fn a_nonsense_full_scale_does_not_divide_by_zero() {
        for fs in [0i64, 1, 15] {
            assert!(super::bin_width(fs) >= 1);
            assert!(super::amp_bin_width(fs) >= 1);
        }
        let g = SampleGeometry {
            format: SampleFormat::Int8,
            full_scale: 0.0,
        };
        let _ = super::signed_bin(&g, 0);
    }

    /// ...and it must not call the whole block a clip either.
    ///
    /// The rail test is a comparison rather than a division, so it fails the
    /// other way: at a full scale of zero, `v >= -1` is true of very nearly
    /// every sample, and the ADC bench would report 100 % saturation on a
    /// perfectly healthy stream. A rail that cannot be located is declined.
    #[test]
    fn a_full_scale_that_locates_no_rail_reports_no_clipping() {
        for fs in [0i64, -1] {
            for v in [0i64, 1, -1, 127, -128, 32767] {
                assert!(
                    !super::on_rail(fs, v),
                    "full scale {fs} locates no rail, so {v} cannot be on one"
                );
            }
        }
        // One count is a degenerate but locatable scale: 0 and -1 are its rails.
        assert!(super::on_rail(1, 0));
        assert!(super::on_rail(1, -1));
    }

    #[test]
    fn int8_centered_value() {
        let v = |b| super::decode(SampleFormat::Int8, 128, b).0;
        assert_eq!(v(0x7F), 127);
        assert_eq!(v(0x80), -128);
        assert_eq!(v(0x00), 0);
    }

    // --- Uint8 (RTL-SDR) decode ----------------------------------------------
    #[test]
    fn uint8_centered_value() {
        // 0x00 → -128, 0x80 → 0, 0xFF → +127
        let v = |b| super::decode(SampleFormat::Uint8, 128, b).0;
        assert_eq!(v(0x00), -128);
        assert_eq!(v(0x80), 0);
        assert_eq!(v(0xFF), 127);
    }

    #[test]
    fn uint8_flags_the_unsigned_extremes() {
        for b in [0x00u8, 0xFF] {
            assert!(
                super::decode(SampleFormat::Uint8, 128, b).1,
                "{b:#04x} is a rail"
            );
        }
        assert!(
            !super::decode(SampleFormat::Uint8, 128, 0x80).1,
            "the DC-bias midpoint must not read as clipping"
        );
    }

    // --- Histogram binning (shared) ------------------------------------------
    #[test]
    fn histogram_extreme_does_not_overflow() {
        // Centered -128 (Uint8 0x00, or Int8 0x80) → unsigned_abs 128 → bin 31.
        let v: i64 = -128;
        let amp = v.unsigned_abs();
        assert_eq!(amp, 128);
        assert_eq!(((amp / 4) as usize).min(31), 31);
    }

    #[test]
    fn histogram_zero_amplitude_bin_zero() {
        let v: i64 = 0;
        assert_eq!(((v.unsigned_abs() / 4) as usize).min(31), 0);
    }
}
