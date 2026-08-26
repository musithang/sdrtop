//! Device-agnostic per-block sample accumulation. Both backends funnel their
//! raw USB byte blocks through [`process_block`]: HackRF from its `extern "C"`
//! callback, RTL-SDR from its owned read thread. Only the byte→sample decode and
//! the saturation test branch on [`SampleFormat`]; every accumulator, the
//! histogram, drops, jitter, and the hand-off to the FFT worker are identical.

use std::time::Instant;

use super::traits::{RxContext, SampleFormat};

/// Fold one raw byte block into the shared metrics accumulators and forward it
/// to the FFT worker.
///
/// `dropped_pairs` is the backend's short-transfer count (HackRF computes it
/// from `buffer_length − valid_length`; RTL-SDR has no equivalent and passes 0).
/// `now` is captured by the *caller* so jitter measures the true inter-callback
/// interval, not callback-entry-plus-processing time.
/// Take one constellation sample per this many I/Q pairs.
const CONST_DECIMATE: usize = 1024;
/// Hard cap on constellation points collected per block (bounds lock time).
const CONST_MAX_PER_BLOCK: usize = 64;

/// Bin a centered signed sample (`[-128, 127]`) into the 32-bucket signed ADC
/// histogram: bin 0 = −FS rail, 16 = mid-scale, 31 = +FS rail.
#[inline]
fn signed_bin(v: i64) -> usize {
    ((v + 128) / 8).clamp(0, 31) as usize
}

pub fn process_block(
    buf: &[u8],
    format: SampleFormat,
    dropped_pairs: u64,
    ctx: &RxContext,
    now: Instant,
) {
    // Per-sample math runs entirely without the mutex.
    let mut saturated: u64 = 0;
    let mut i_sum: i64 = 0;
    let mut q_sum: i64 = 0;
    let mut i_sq: i64 = 0;
    let mut q_sq: i64 = 0;
    let mut iq_cross: i64 = 0;
    let mut local_hist: [u64; 32] = [0; 32];
    let mut local_signed: [u64; 32] = [0; 32]; // signed I/Q distribution (ADC bell)
    let mut local_peak: u32 = 0; // loudest |i|,|q| this block
    let mut local_const: Vec<(f32, f32)> = Vec::new();

    // Snapshot the live correction state once (cheap Copy). The accumulators below
    // stay on the RAW samples — the diagnostics measure the true hardware
    // impairment — while a corrected copy of the stream feeds the FFT and the
    // constellation so the [D] DC-block / [C] auto-cal cleanup is visible.
    // Read the demod gate in the same lock as the correction state — the demod
    // costs an extra block copy, so it must be free when switched off.
    let (cal, demod_enabled) = {
        let m = ctx.metrics.lock().unwrap_or_else(|e| e.into_inner());
        (m.iq.cal, m.demod.enabled)
    };
    let correcting = cal.correcting();
    let mut out_buf: Vec<u8> = if correcting {
        Vec::with_capacity(buf.len())
    } else {
        Vec::new()
    };

    for (pair_idx, chunk) in buf.as_chunks::<2>().0.iter().enumerate() {
        let (i, i_sat) = decode(format, chunk[0]);
        let (q, q_sat) = decode(format, chunk[1]);
        i_sum += i;
        q_sum += q;
        i_sq += i * i;
        q_sq += q * q;
        iq_cross += i * q;
        if i_sat {
            saturated += 1;
        }
        if q_sat {
            saturated += 1;
        }
        // Chebyshev distance, 32 bins of width 4. `unsigned_abs` of the centered
        // value can reach 128 (the -128 extreme); `.min(31)` clamps that to the
        // last bin instead of indexing [32] and panicking inside the callback.
        let amp = i.unsigned_abs().max(q.unsigned_abs());
        local_hist[((amp / 4) as usize).min(31)] += 1;
        // Signed sample distribution (I and Q each) for the ADC-loading bell, plus the
        // loudest sample — both on the RAW samples, the physical ADC's-eye view.
        local_peak = local_peak.max(amp as u32);
        local_signed[signed_bin(i)] += 1;
        local_signed[signed_bin(q)] += 1;

        // Display path: corrected samples feed the FFT (re-encoded bytes) and the
        // constellation. When no correction is active these equal the raw samples.
        let (ci, cq) = if correcting {
            cal.apply(i as f32, q as f32)
        } else {
            (i as f32, q as f32)
        };
        if correcting {
            let (bi, bq) = encode_pair(ci, cq, format);
            out_buf.push(bi);
            out_buf.push(bq);
        }
        // Constellation decimation: one normalised (I, Q) pair per CONST_DECIMATE.
        // Frozen ([F]) → stop collecting so the cloud holds its last shape.
        if !cal.frozen && pair_idx % CONST_DECIMATE == 0 && local_const.len() < CONST_MAX_PER_BLOCK
        {
            local_const.push((ci / 128.0, cq / 128.0));
        }
    }

    let pairs = (buf.len() / 2) as u64;
    let block_seq: u64;

    // Single brief lock to flush accumulated results — O(1), no loops inside.
    {
        let Ok(mut m) = ctx.metrics.lock() else {
            ctx.sample_tx.try_send(buf.to_vec()).ok();
            return;
        };

        m.radio.bytes_since_last_poll += buf.len() as u64;

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
        m.acc.sample_count += pairs;

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
    if demod_enabled {
        ctx.demod_tx.try_send((block_seq, forward.clone())).ok();
    }
    ctx.sample_tx.try_send(forward).ok();
}

/// Re-encode one corrected (I, Q) sample back to the wire byte format, clamping to
/// the 8-bit range. Used only when a correction is active.
fn encode_pair(i: f32, q: f32, format: SampleFormat) -> (u8, u8) {
    let ci = i.round().clamp(-128.0, 127.0) as i32;
    let cq = q.round().clamp(-128.0, 127.0) as i32;
    match format {
        SampleFormat::Int8 => (ci as i8 as u8, cq as i8 as u8),
        SampleFormat::Uint8 => ((ci + 128) as u8, (cq + 128) as u8),
    }
}

/// Decode one raw byte into a centered signed value in [-128, 127], and say
/// whether it sits on a rail.
///
/// The two formats differ only here: HackRF sends `Int8`, RTL-SDR `Uint8` biased
/// at 127.5. Centering Uint8 by 128 rather than the true bias keeps the
/// downstream DC-offset `/128.0` normalization valid, and the half-LSB
/// difference is negligible for diagnostics.
///
/// `#[inline]` because this runs twice per sample in the RX callback.
#[inline]
fn decode(format: SampleFormat, b: u8) -> (i64, bool) {
    match format {
        SampleFormat::Int8 => (b as i8 as i64, b == 0x80 || b == 0x7F),
        SampleFormat::Uint8 => (b as i64 - 128, b == 0x00 || b == 0xFF),
    }
}

#[cfg(test)]
mod tests {
    use super::SampleFormat;

    // These exercise the decode/saturation/histogram arithmetic that
    // `process_block` performs inline; constructing a full RxContext is left to
    // the hardware-in-the-loop verification.

    // --- Int8 (HackRF) decode -------------------------------------------------
    #[test]
    fn int8_flags_both_rails_and_nothing_between() {
        // This used to assert `0x7F == 0x7F || 0x7F == 0x80`, which is true of
        // any program. It now asks `decode` itself.
        for b in [0x7Fu8, 0x80] {
            assert!(super::decode(SampleFormat::Int8, b).1, "{b:#04x} is a rail");
        }
        for b in [0x00u8, 0x40, 0x7E, 0x81, 0xC0] {
            assert!(
                !super::decode(SampleFormat::Int8, b).1,
                "{b:#04x} is not a rail"
            );
        }
    }

    #[test]
    fn signed_bin_maps_rails_and_centre() {
        assert_eq!(super::signed_bin(-128), 0, "−FS rail → bin 0");
        assert_eq!(super::signed_bin(0), 16, "mid-scale → centre bin");
        assert_eq!(super::signed_bin(127), 31, "+FS rail → top bin");
        // Clamps out-of-range without panicking on the array index.
        assert_eq!(super::signed_bin(200), 31);
        assert_eq!(super::signed_bin(-200), 0);
    }

    #[test]
    fn int8_centered_value() {
        let v = |b| super::decode(SampleFormat::Int8, b).0;
        assert_eq!(v(0x7F), 127);
        assert_eq!(v(0x80), -128);
        assert_eq!(v(0x00), 0);
    }

    // --- Uint8 (RTL-SDR) decode ----------------------------------------------
    #[test]
    fn uint8_centered_value() {
        // 0x00 → -128, 0x80 → 0, 0xFF → +127
        let v = |b| super::decode(SampleFormat::Uint8, b).0;
        assert_eq!(v(0x00), -128);
        assert_eq!(v(0x80), 0);
        assert_eq!(v(0xFF), 127);
    }

    #[test]
    fn uint8_flags_the_unsigned_extremes() {
        for b in [0x00u8, 0xFF] {
            assert!(
                super::decode(SampleFormat::Uint8, b).1,
                "{b:#04x} is a rail"
            );
        }
        assert!(
            !super::decode(SampleFormat::Uint8, 0x80).1,
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
