// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The floating-point half of the poll: raw accumulator sums in, IQ and timing
//! measurements out.
//!
//! **This module is why the split exists.** Everything here is a pure function of
//! numbers the poll already drained, with no lock, no device and no clock in it,
//! which is exactly the property the hot path needs: `sqrt`, `log10` and `asin`
//! must not run while the mutex is held, because every frame the UI draws is
//! waiting on that same mutex.
//!
//! It is also the half that was untestable while it lived in the middle of a
//! 400-line `async move` block. The radio maths below - I/Q amplitude and phase
//! imbalance, ADC loading in dBFS, callback jitter - had no tests at all before
//! R8a.

use crate::hardware::SampleGeometry;
use crate::state::IqCalState;

/// The floor every dBFS reading is clamped to, so a silent stream reports a
/// number rather than `-inf`.
pub(super) const DBFS_FLOOR: f32 = -120.0;

/// The raw integer moments one poll window drained from the accumulators.
///
/// Sums rather than averages on purpose: they are accumulated as integers in the
/// hot path and divided exactly once, here.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Moments {
    pub i_sum: i64,
    pub q_sum: i64,
    pub i_sq_sum: u64,
    pub q_sq_sum: u64,
    pub cross_sum: i64,
    pub samples: u64,
    /// Largest single-component magnitude seen in the window, in counts.
    pub peak_amp: u32,
}

/// What one window says about the front end.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct IqMetrics {
    /// DC offset as a fraction of full scale, **after** whatever correction is
    /// active, so it agrees with the corrected constellation.
    pub dc_i: f32,
    pub dc_q: f32,
    /// Mean I/Q in raw sample units, referred to the converter's own zero: what
    /// DC-block subtracts, measured before correction because that is what a
    /// correction has to be built from.
    ///
    /// Carries [`SampleGeometry::centre_bias`], so on an unsigned radio it is
    /// half a count above the decoded mean. It has to: the re-encode in the hot
    /// path puts the corrected samples back on the same 128-centred grid, so
    /// subtracting the decoded mean would leave the stream half an LSB the other
    /// side of zero and report the result as centred.
    pub dc_i_raw: f32,
    pub dc_q_raw: f32,
    /// Candidate auto-cal Q-row coefficients, from the raw moments.
    pub iq_corr: (f32, f32),
    /// Residual amplitude imbalance after the active correction. `None` when Q
    /// carries no power to compare against.
    pub iq_imbalance_db: Option<f32>,
    /// Residual quadrature error in degrees, same caveat.
    pub phase_imbalance: Option<f32>,
    /// Loudest sample of the window, dBFS.
    pub adc_peak_dbfs: f32,
    /// Full-bandwidth RMS, dBFS.
    pub adc_rms_dbfs: f32,
}

impl IqMetrics {
    /// What an empty window reports: no impairment measured, and the dBFS floor
    /// rather than `-inf`. `iq_corr` is the identity Q-row.
    pub(super) fn idle() -> Self {
        Self {
            dc_i: 0.0,
            dc_q: 0.0,
            dc_i_raw: 0.0,
            dc_q_raw: 0.0,
            iq_corr: (0.0, 1.0),
            iq_imbalance_db: None,
            phase_imbalance: None,
            adc_peak_dbfs: DBFS_FLOOR,
            adc_rms_dbfs: DBFS_FLOOR,
        }
    }
}

/// Derive the window's IQ metrics.
///
/// `cal` is the correction that was live while the window was captured. It
/// splits the answer in two, deliberately:
///
/// - the **candidate** coefficients and the DC to subtract come from the raw
///   moments, because that is what `[C]` / `[D]` capture and apply;
/// - the **displayed** impairment is the residual left after the active
///   correction, so the number agrees with the scope beside it instead of
///   reporting a fault the app is already compensating.
///
/// `full_scale` is the device's own count for 0 dBFS, from
/// [`crate::hardware::SampleGeometry`]. It used to be a module constant of 128,
/// which was true of both radios sdrtop could open and of nothing else.
pub(super) fn iq_metrics(m: Moments, cal: IqCalState, geometry: SampleGeometry) -> IqMetrics {
    let full_scale = geometry.full_scale as f64;
    if m.samples == 0 {
        return IqMetrics::idle();
    }

    let n = m.samples as f64;
    let mean_i = m.i_sum as f64 / n;
    let mean_q = m.q_sum as f64 / n;
    let var_i = (m.i_sq_sum as f64 / n - mean_i * mean_i).max(0.0);
    let var_q = (m.q_sq_sum as f64 / n - mean_q * mean_q).max(0.0);
    let cov_iq = m.cross_sum as f64 / n - mean_i * mean_q;

    // ADC loading: peak from the loudest sample, RMS from the total I/Q power,
    // both referenced to full scale.
    let adc_peak_dbfs = if m.peak_amp > 0 {
        20.0 * (m.peak_amp as f32 / full_scale as f32).log10()
    } else {
        DBFS_FLOOR
    };
    let adc_rms_dbfs = {
        let p = (var_i + var_q) / (full_scale * full_scale);
        if p > 0.0 {
            (10.0 * p.log10()) as f32
        } else {
            DBFS_FLOOR
        }
    };

    let iq_corr = crate::signal::iq_correction_coeffs(var_i, var_q, cov_iq);

    let (ev_i, ev_q, ecov) = if cal.cal_applied {
        crate::signal::corrected_moments(var_i, var_q, cov_iq, cal.c_qi as f64, cal.c_qq as f64)
    } else {
        (var_i, var_q, cov_iq)
    };
    // The converter's zero is not always the decoder's: an unsigned format is
    // centred on a value that is not a code, and the hot path rounds it to one
    // because its accumulators are integers. Only the offset takes the
    // correction back - shifting every sample by the same amount moves the
    // centre and not the spread, so the variances and the covariance above are
    // the same numbers either way, and so is everything derived from them.
    let bias = geometry.centre_bias();
    let (offset_i, offset_q) = (mean_i + bias, mean_q + bias);
    let (emean_i, emean_q) = if cal.dc_block_on || cal.cal_applied {
        (
            offset_i - cal.dc_i_raw as f64,
            offset_q - cal.dc_q_raw as f64,
        )
    } else {
        (offset_i, offset_q)
    };

    let i_ac = ev_i.sqrt();
    let q_ac = ev_q.sqrt();
    let denom = ev_i + ev_q;
    IqMetrics {
        dc_i: (emean_i / full_scale) as f32,
        dc_q: (emean_q / full_scale) as f32,
        dc_i_raw: offset_i as f32,
        dc_q_raw: offset_q as f32,
        iq_corr,
        iq_imbalance_db: (q_ac > 0.0).then(|| (20.0 * (i_ac / q_ac).log10()) as f32),
        phase_imbalance: (denom > 0.0).then(|| {
            let sin_theta = (2.0 * ecov / denom).clamp(-1.0, 1.0);
            (sin_theta.asin() * 180.0 / std::f64::consts::PI) as f32
        }),
        adc_peak_dbfs,
        adc_rms_dbfs,
    }
}

/// Mean callback period and its standard deviation, in microseconds. `None` when
/// no callback landed in the window.
pub(super) fn callback_timing(sum_us: u64, sq_sum: u64, count: u64) -> Option<(u64, u64)> {
    if count == 0 {
        return None;
    }
    let mean = sum_us / count;
    let sq_mean = sq_sum / count;
    // Saturating: the two sums are accumulated independently, so a window that
    // straddles a reset can leave `sq_mean` below `mean²` and underflow.
    let variance = sq_mean.saturating_sub(mean.saturating_mul(mean));
    Some((mean, (variance as f64).sqrt() as u64))
}

/// What fraction of a pull backend's read loop went on work rather than waiting.
///
/// **The pull-mode answer to "is the host keeping up".** A push backend is
/// graded on whether callbacks arrive on time, because the driver paces them. A
/// pull loop sets its own rhythm, so arrival times say nothing; what says
/// everything is whether the loop ever gets to block. Below one there is
/// headroom. At one it never waits, which means it is behind and the driver's
/// buffer is filling behind it.
///
/// `None` before the first complete pass, when there is no window to divide by.
pub(super) fn occupancy(wait_us: u64, work_us: u64) -> Option<f32> {
    let total = wait_us.checked_add(work_us)?;
    if total == 0 {
        return None;
    }
    Some((work_us as f64 / total as f64) as f32)
}

/// How much history the sample-rate estimate averages over, in microseconds.
///
/// **This is a resolution requirement, not a taste.** Bytes arrive in whole
/// driver blocks, so a poll window holds a whole number of them and nothing in
/// between: measured on a HackRF through SoapyHackRF at 10 Msps, a 200 ms window
/// held anywhere from 120 to 127 blocks of 32768 bytes, and not one partial
/// block in 126 windows. One block is 8192 ppm of a 200 ms window, and the
/// observed spread of the per-window estimate was -17 000 to +40 000 ppm.
///
/// `TimingCause::Clock` fires above 500 ppm. An estimator sixteen times coarser
/// than its own threshold does not measure a clock, it measures how many blocks
/// happened to fit.
///
/// Summing consecutive windows fixes it by arithmetic rather than by smoothing:
/// the whole-block error survives only at the two ends of the baseline, so it
/// divides by the total elapsed time. One block is 8192 ppm over 200 ms, 164 ppm
/// over 10 s, and 55 ppm over 30 s. Thirty seconds leaves a comfortable margin
/// under the threshold.
const RATE_BASELINE_US: u64 = 30_000_000;

/// Below this the baseline is too short to be worth a number, and the reading is
/// `None` rather than a plausible-looking one.
///
/// Ten seconds, because that is where the block quantisation drops to 164 ppm at
/// 10 Msps, comfortably under the 500 ppm the grade fires at. A shorter floor
/// was tried at two seconds and measured on hardware: the estimate read -1379,
/// -797 and -670 ppm over its first twenty seconds, which would have traded a
/// permanent false clock fault for a temporary one.
const RATE_BASELINE_MIN_US: u64 = 10_000_000;

/// Pairs each window's bytes with the interval its blocks actually arrived in.
///
/// **This is what removes the quantisation rather than averaging it away.** A
/// window's byte count is exact: it is a whole number of driver blocks. What
/// was not exact was the time it was divided by, measured between poll instants
/// that fall wherever they fall between two block arrivals. The mismatch is up
/// to one block, and one block is 437 ppm over a 30 s baseline on a HackRF,
/// whose transfer is 262144 bytes. Measured on hardware, that made the native
/// bench swing between -159 and +499 ppm on a stream whose real offset is
/// about +130, and grade itself `GOOD` or `MARGINAL` at random.
///
/// Timing from the last block of one window to the last block of the next makes
/// the numerator and the denominator refer to the same two boundaries, and the
/// error cancels exactly. What is left is the callback's own scheduling jitter,
/// about 148 µs on a HackRF, which over 30 s is 5 ppm.
///
/// It holds an `Instant` but never reads one: the timestamp is recorded by
/// `process_block` in the hot path and handed here, so this module keeps its
/// rule of having no clock of its own.
#[derive(Debug, Default)]
pub(super) struct RateTracker {
    baseline: RateBaseline,
    prev_block_at: Option<std::time::Instant>,
}

impl RateTracker {
    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }

    /// Fold in one poll window: the arrival of its last block, and the bytes
    /// that arrived since the previous window's last block.
    ///
    /// A window with no block in it carries no timestamp and is skipped, which
    /// is also what makes the first window after a reset cost nothing: there is
    /// no earlier boundary to measure from, so there is nothing to fold.
    pub(super) fn push(&mut self, last_block_at: Option<std::time::Instant>, bytes: u64) {
        let Some(at) = last_block_at else { return };
        let Some(prev) = self.prev_block_at.replace(at) else {
            return;
        };
        self.baseline
            .push(at.duration_since(prev).as_micros() as u64, bytes);
    }

    pub(super) fn rate(&self, bytes_per_pair: usize) -> Option<u32> {
        self.baseline.rate(bytes_per_pair)
    }
}

/// Sliding long-baseline estimate of the delivered sample rate.
///
/// Task local, deliberately: it is neither drawn nor shared, so it has no
/// business being cloned into every frame with the rest of `SdrMetrics`.
///
/// No clock in here either, in keeping with the rest of this module. It is
/// handed each window's length rather than reading one.
#[derive(Debug, Default)]
pub(super) struct RateBaseline {
    /// `(window length in microseconds, bytes in that window)`, oldest first.
    windows: std::collections::VecDeque<(u64, u64)>,
    elapsed_us: u64,
    bytes: u64,
}

impl RateBaseline {
    /// Fold in one poll window, dropping whatever no longer fits the baseline.
    pub(super) fn push(&mut self, elapsed_us: u64, bytes: u64) {
        if elapsed_us == 0 {
            return;
        }
        self.windows.push_back((elapsed_us, bytes));
        self.elapsed_us += elapsed_us;
        self.bytes += bytes;
        // Keep at least one window beyond the target so the baseline never
        // shrinks below it after a trim.
        while self.windows.len() > 1 {
            let (oldest_us, oldest_bytes) = self.windows[0];
            if self.elapsed_us - oldest_us < RATE_BASELINE_US {
                break;
            }
            self.windows.pop_front();
            self.elapsed_us -= oldest_us;
            self.bytes -= oldest_bytes;
        }
    }

    /// Complex samples per second over the baseline, or `None` while it is still
    /// too short to mean anything.
    ///
    /// `bytes_per_pair` comes from the device's `SampleGeometry`. It used to be
    /// a hardcoded 2, which is right for `Int8` and `Uint8` and reports double
    /// the true rate for `Int16`.
    pub(super) fn rate(&self, bytes_per_pair: usize) -> Option<u32> {
        if self.elapsed_us < RATE_BASELINE_MIN_US || bytes_per_pair == 0 {
            return None;
        }
        let pairs = self.bytes / bytes_per_pair as u64;
        u32::try_from(pairs.checked_mul(1_000_000)? / self.elapsed_us).ok()
    }
}

/// One poll window of the radio's own drop count.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct DropWindow {
    /// Drops since the last reading.
    pub events: u32,
    /// The reading itself.
    pub reading: crate::hardware::RadioDrops,
    /// Where in the stream the window's drops can be: after the position at
    /// the last reading, and no later than this reading's position plus what
    /// was still in flight between the radio and the callback. A drop in the
    /// radio leaves no mark in the positions, so this bracket is as close as
    /// the stream can place it.
    pub from_pair: u64,
    pub to_pair: u64,
}

/// Turn a cumulative reading into a window, against the previous one.
///
/// `seen` is `(count, stream position)` at the last reading of this stream,
/// or `None`. The count restarts with each stream, and so do the positions,
/// so a count or a position that went backwards is a new stream: everything
/// it holds is new, from the start of the stream.
pub(super) fn drop_window(
    seen: &mut Option<(u32, u64)>,
    reading: crate::hardware::RadioDrops,
    position: u64,
    bytes_per_pair: u64,
) -> DropWindow {
    let (events, from_pair) = match *seen {
        Some((count, at)) if reading.events >= count && position >= at => {
            (reading.events - count, at)
        }
        _ => (reading.events, 0),
    };
    *seen = Some((reading.events, position));
    DropWindow {
        events,
        reading,
        from_pair,
        to_pair: position + reading.in_flight_bytes / bytes_per_pair.max(1),
    }
}

/// Where the constellation is zoomed to, held from poll to poll.
///
/// **Out at once, in only after a second.** A cloud that grows past the outer
/// ring would be drawn off the edge, so the scale follows it out on the next
/// poll; a cloud that shrinks is only zoomed in on once it has wanted the
/// smaller ring for [`ZOOM_IN_POLLS`] polls in a row, so an rms hovering on a
/// step boundary does not flicker the whole picture between two scales. A
/// frozen constellation keeps its scale: a held cloud re-zoomed under the
/// live rms would be a different picture of the same points.
#[derive(Debug, Default)]
pub(super) struct ConstellationScale {
    outer_db: Option<f64>,
    wants_in: u32,
}

/// Polls of 200 ms a smaller ring must be wanted before the picture zooms in.
const ZOOM_IN_POLLS: u32 = 5;

impl ConstellationScale {
    /// The outer ring after a window whose magnitude rms was `rms_dbfs`.
    pub(super) fn update(&mut self, rms_dbfs: f64, frozen: bool) -> f64 {
        let want = crate::state::IqState::outer_ring_for(rms_dbfs);
        let now = match self.outer_db {
            Some(held) if frozen => held,
            Some(held) if want < held => {
                self.wants_in += 1;
                if self.wants_in >= ZOOM_IN_POLLS {
                    self.wants_in = 0;
                    want
                } else {
                    held
                }
            }
            _ => {
                self.wants_in = 0;
                want
            }
        };
        self.outer_db = Some(now);
        now
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::SampleFormat;

    /// The scale follows a growing cloud at once and a shrinking one only
    /// after a second of wanting it; a frozen cloud keeps its scale.
    #[test]
    fn the_constellation_zooms_out_at_once_and_in_after_a_second() {
        let mut s = ConstellationScale::default();
        assert_eq!(s.update(-14.0, false), -9.0, "5 dB over, on a 3 dB step");
        assert_eq!(s.update(-1.0, false), 0.0, "out at once, never past FS");
        for _ in 0..ZOOM_IN_POLLS - 1 {
            assert_eq!(s.update(-14.0, false), 0.0, "held while it waits");
        }
        assert_eq!(s.update(-14.0, false), -9.0, "in after a second");
        // Hovering across a step boundary goes out once, then holds there.
        let hovering: Vec<f64> = [-13.9, -14.1, -13.9, -14.1, -13.9, -14.1]
            .into_iter()
            .map(|rms| s.update(rms, false))
            .collect();
        assert_eq!(hovering, vec![-6.0; 6]);
        let held = s.outer_db.unwrap();
        assert_eq!(s.update(-40.0, true), held, "frozen keeps its scale");
        assert_eq!(s.update(5.0, true), held);
    }

    /// A HackRF: signed, so its zero is a code and there is nothing to add back.
    /// Used by every test whose subject is not the centring itself, so their
    /// expectations are unchanged by the bias.
    const EIGHT_BIT: SampleGeometry = SampleGeometry {
        format: SampleFormat::Int8,
        full_scale: 128.0,
        converter_bits: None,
    };
    /// An RTL-SDR: unsigned, centred on a value that is not a code.
    const RTL: SampleGeometry = SampleGeometry {
        format: SampleFormat::Uint8,
        full_scale: 128.0,
        converter_bits: None,
    };

    /// Moments for `n` samples with the given per-component variance and no DC,
    /// as an ideal balanced front end would produce.
    fn balanced(n: u64, var: u64) -> Moments {
        Moments {
            i_sum: 0,
            q_sum: 0,
            i_sq_sum: var * n,
            q_sq_sum: var * n,
            cross_sum: 0,
            samples: n,
            peak_amp: 0,
        }
    }

    fn no_cal() -> IqCalState {
        IqCalState::default()
    }

    /// A perfectly centred RTL-SDR must read as having no DC offset.
    ///
    /// CU8's analogue zero sits between code 127 and code 128, and the hot path
    /// centres on 128 because the accumulators are integers. A stream split
    /// evenly between those two codes is as centred as an 8-bit converter can
    /// be, and it decodes to a mean of -0.5 counts. Reported as it stands that
    /// is 0.0039 of full scale on every channel, 78 % of the offset this bench
    /// warns at, and a -45 dBFS floor under a spike it grades from -40: a number
    /// describing the decoder rather than the radio, and one no RTL-SDR could
    /// ever read better than.
    #[test]
    fn a_centred_unsigned_stream_has_no_dc_offset() {
        let n = 1_000;
        let mo = Moments {
            i_sum: -(n as i64) / 2,
            q_sum: -(n as i64) / 2,
            i_sq_sum: n / 2,
            q_sq_sum: n / 2,
            cross_sum: n as i64 / 2,
            samples: n,
            peak_amp: 1,
        };
        let m = iq_metrics(mo, no_cal(), RTL);
        assert_eq!(m.dc_i, 0.0, "the decode convention is not an offset");
        assert_eq!(m.dc_q, 0.0);
        assert_eq!(m.dc_i_raw, 0.0, "and there is nothing for [D] to subtract");
        assert_eq!(m.dc_q_raw, 0.0);
    }

    /// The same window on a signed radio keeps its offset. There the zero really
    /// is a code, so half a count below it is half a count of real DC, and adding
    /// the correction everywhere would invent one radio's fix on another.
    #[test]
    fn a_signed_radio_keeps_the_offset_the_unsigned_one_does_not_have() {
        let n = 1_000;
        let mo = Moments {
            i_sum: -(n as i64) / 2,
            q_sum: -(n as i64) / 2,
            i_sq_sum: n / 2,
            q_sq_sum: n / 2,
            cross_sum: n as i64 / 2,
            samples: n,
            peak_amp: 1,
        };
        let m = iq_metrics(mo, no_cal(), EIGHT_BIT);
        assert_eq!(m.dc_i_raw, -0.5);
        assert!((m.dc_i - (-0.5 / 128.0)).abs() < 1e-9, "got {}", m.dc_i);
    }

    /// The correction shifts where zero is, and must not move the spread around
    /// it: the variance and covariance are the same numbers either way, so the
    /// imbalance readings a device is graded on cannot depend on its format.
    #[test]
    fn the_centring_moves_the_offset_and_nothing_else() {
        let mo = Moments {
            i_sum: -500,
            q_sum: -500,
            i_sq_sum: 40_000,
            q_sq_sum: 10_000,
            cross_sum: 3_000,
            samples: 1_000,
            peak_amp: 60,
        };
        let signed = iq_metrics(mo, no_cal(), EIGHT_BIT);
        let unsigned = iq_metrics(mo, no_cal(), RTL);
        assert_eq!(signed.iq_imbalance_db, unsigned.iq_imbalance_db);
        assert_eq!(signed.phase_imbalance, unsigned.phase_imbalance);
        assert_eq!(signed.adc_rms_dbfs, unsigned.adc_rms_dbfs);
        assert_eq!(signed.adc_peak_dbfs, unsigned.adc_peak_dbfs);
        assert_eq!(signed.iq_corr, unsigned.iq_corr);
        assert!(
            unsigned.dc_i_raw > signed.dc_i_raw,
            "only the offset moves: {} vs {}",
            unsigned.dc_i_raw,
            signed.dc_i_raw
        );
    }

    #[test]
    fn an_empty_window_measures_nothing_rather_than_zero() {
        // The distinction matters: 0.0 dB imbalance is a *perfect* front end, and
        // a stopped radio must not claim that.
        let m = iq_metrics(Moments::default(), no_cal(), EIGHT_BIT);
        assert_eq!(m, IqMetrics::idle());
        assert!(m.iq_imbalance_db.is_none() && m.phase_imbalance.is_none());
        assert_eq!(
            m.adc_peak_dbfs, DBFS_FLOOR,
            "a silent stream reports the floor, not -inf"
        );
        assert_eq!(
            m.iq_corr,
            (0.0, 1.0),
            "the identity Q-row leaves the samples alone"
        );
    }

    #[test]
    fn a_balanced_front_end_reports_no_impairment() {
        let m = iq_metrics(balanced(1000, 400), no_cal(), EIGHT_BIT);
        assert!(
            m.iq_imbalance_db.unwrap().abs() < 1e-4,
            "{:?}",
            m.iq_imbalance_db
        );
        assert!(
            m.phase_imbalance.unwrap().abs() < 1e-4,
            "{:?}",
            m.phase_imbalance
        );
        assert!(m.dc_i.abs() < 1e-6 && m.dc_q.abs() < 1e-6);
    }

    #[test]
    fn amplitude_imbalance_is_the_i_over_q_ratio_in_db() {
        // I carries 4x the power of Q, so twice the amplitude: +6.02 dB.
        let mut mo = balanced(1000, 100);
        mo.i_sq_sum *= 4;
        let db = iq_metrics(mo, no_cal(), EIGHT_BIT).iq_imbalance_db.unwrap();
        assert!((db - 6.0206).abs() < 0.01, "got {db}");
        // And the sign follows which arm is louder.
        let mut swapped = balanced(1000, 100);
        swapped.q_sq_sum *= 4;
        assert!(
            (iq_metrics(swapped, no_cal(), EIGHT_BIT)
                .iq_imbalance_db
                .unwrap()
                + 6.0206)
                .abs()
                < 0.01
        );
    }

    #[test]
    fn phase_imbalance_comes_out_of_the_i_q_covariance() {
        // Equal power in both arms with covariance c gives sin(theta) = 2c/(vi+vq).
        // Here c = vi/2, so sin(theta) = 0.5 and theta = 30 degrees.
        let mut mo = balanced(1000, 200);
        mo.cross_sum = 100 * 1000;
        let deg = iq_metrics(mo, no_cal(), EIGHT_BIT).phase_imbalance.unwrap();
        assert!((deg - 30.0).abs() < 0.01, "got {deg}");
    }

    #[test]
    fn a_perfectly_correlated_pair_does_not_blow_past_ninety_degrees() {
        // 2c/(vi+vq) can exceed 1 on a degenerate window, and `asin` of that is
        // NaN. The clamp is what keeps a number on screen.
        let mut mo = balanced(100, 50);
        mo.cross_sum = 10_000 * 100;
        let deg = iq_metrics(mo, no_cal(), EIGHT_BIT).phase_imbalance.unwrap();
        assert!(deg.is_finite() && (deg - 90.0).abs() < 1e-3, "got {deg}");
    }

    #[test]
    fn dc_offset_is_reported_as_a_fraction_of_full_scale() {
        let mo = Moments {
            i_sum: 64 * 1000,
            q_sum: -32 * 1000,
            samples: 1000,
            i_sq_sum: 100 * 1000,
            q_sq_sum: 100 * 1000,
            ..Moments::default()
        };
        let m = iq_metrics(mo, no_cal(), EIGHT_BIT);
        assert!(
            (m.dc_i - 0.5).abs() < 1e-6,
            "64 of 128 counts is half scale: {}",
            m.dc_i
        );
        assert!((m.dc_q + 0.25).abs() < 1e-6);
        // The raw figures stay in counts, because that is what DC-block subtracts.
        assert!((m.dc_i_raw - 64.0).abs() < 1e-4);
        assert!((m.dc_q_raw + 32.0).abs() < 1e-4);
    }

    #[test]
    fn dc_block_removes_the_offset_from_the_displayed_figure_only() {
        // With DC-block on, the panel should read the residual (zero), while the
        // raw estimate keeps tracking so the block follows slow drift.
        let mo = Moments {
            i_sum: 64 * 1000,
            q_sum: 0,
            samples: 1000,
            i_sq_sum: 100 * 1000,
            q_sq_sum: 100 * 1000,
            ..Moments::default()
        };
        let cal = IqCalState {
            dc_block_on: true,
            dc_i_raw: 64.0,
            ..IqCalState::default()
        };
        let m = iq_metrics(mo, cal, EIGHT_BIT);
        assert!(m.dc_i.abs() < 1e-6, "residual should be ~0, got {}", m.dc_i);
        assert!(
            (m.dc_i_raw - 64.0).abs() < 1e-4,
            "the raw estimate must keep tracking"
        );
    }

    #[test]
    fn adc_loading_is_referenced_to_full_scale() {
        let mut mo = balanced(1000, 0);
        mo.peak_amp = 128;
        assert!(
            iq_metrics(mo, no_cal(), EIGHT_BIT).adc_peak_dbfs.abs() < 1e-4,
            "a rail-hitting sample is 0 dBFS"
        );
        mo.peak_amp = 64;
        let half = iq_metrics(mo, no_cal(), EIGHT_BIT).adc_peak_dbfs;
        assert!(
            (half + 6.0206).abs() < 0.01,
            "half scale is -6 dBFS, got {half}"
        );
        // No sample recorded at all: the floor, never -inf.
        mo.peak_amp = 0;
        assert_eq!(
            iq_metrics(mo, no_cal(), EIGHT_BIT).adc_peak_dbfs,
            DBFS_FLOOR
        );
    }

    #[test]
    fn adc_rms_sums_the_power_of_both_arms() {
        // var_i = var_q = 128²/2 gives total power 128², i.e. 0 dBFS.
        let var = (128 * 128) / 2;
        let db = iq_metrics(balanced(1000, var), no_cal(), EIGHT_BIT).adc_rms_dbfs;
        assert!(db.abs() < 0.01, "got {db}");
        // A dead-silent stream floors rather than reporting -inf.
        assert_eq!(
            iq_metrics(balanced(1000, 0), no_cal(), EIGHT_BIT).adc_rms_dbfs,
            DBFS_FLOOR
        );
    }

    #[test]
    fn callback_timing_is_mean_and_standard_deviation() {
        // Three gaps of 1000 us: mean 1000, no spread.
        assert_eq!(callback_timing(3000, 3 * 1000 * 1000, 3), Some((1000, 0)));
        // 900 / 1100 us: mean 1000, sd 100.
        let sq = 900u64 * 900 + 1100 * 1100;
        assert_eq!(callback_timing(2000, sq, 2), Some((1000, 100)));
    }

    #[test]
    fn callback_timing_declines_an_empty_window() {
        assert_eq!(
            callback_timing(0, 0, 0),
            None,
            "no callbacks means no measurement"
        );
    }

    #[test]
    fn callback_timing_survives_sums_that_disagree() {
        // The two sums are accumulated separately, so a window straddling a reset
        // can leave the mean of squares below the square of the mean. That is a
        // subtraction away from underflowing a u64 into a vast bogus jitter.
        assert_eq!(callback_timing(1_000_000, 0, 1), Some((1_000_000, 0)));
    }
    /// The dBFS reading follows the device's own full scale. A 16-bit radio
    /// whose peak sample is at its rail is at 0 dBFS, exactly like an 8-bit one
    /// at its own. Before this was a parameter it would have read as -48 dBFS,
    /// which is a receiver that looks broken and is not.
    #[test]
    fn full_scale_is_the_devices_own_and_not_a_constant() {
        let mut mo = Moments {
            samples: 1000,
            ..Moments::default()
        };
        mo.peak_amp = 32768;
        assert!(
            iq_metrics(
                mo,
                no_cal(),
                SampleGeometry {
                    format: SampleFormat::Int16,
                    full_scale: 32768.0,
                    converter_bits: None,
                },
            )
            .adc_peak_dbfs
            .abs()
                < 1e-4,
            "a 16-bit rail is 0 dBFS on a 16-bit device"
        );
        assert!(
            iq_metrics(mo, no_cal(), EIGHT_BIT).adc_peak_dbfs > 40.0,
            "and the same counts on an 8-bit device are far above its rail"
        );
    }

    // ── The radio's own drops ───────────────────────────────────────────────

    fn drops(events: u32) -> crate::hardware::RadioDrops {
        crate::hardware::RadioDrops {
            events,
            longest_bytes: 4096,
            in_flight_bytes: 1_000,
        }
    }

    /// The first reading of a stream holds everything since it started; the
    /// next only what is new, bracketed from the last reading's position to
    /// this one's plus what was still in flight.
    #[test]
    fn a_drop_window_is_what_is_new_and_where_it_can_be() {
        let mut seen = None;
        let first = drop_window(&mut seen, drops(2), 50_000, 2);
        assert_eq!(
            (first.events, first.from_pair, first.to_pair),
            (2, 0, 50_500)
        );
        let quiet = drop_window(&mut seen, drops(2), 90_000, 2);
        assert_eq!(quiet.events, 0);
        let more = drop_window(&mut seen, drops(5), 130_000, 2);
        assert_eq!(
            (more.events, more.from_pair, more.to_pair),
            (3, 90_000, 130_500)
        );
    }

    /// A count or a position that went backwards is a new stream, and all of
    /// its count is new.
    #[test]
    fn a_new_stream_starts_its_count_again() {
        let mut seen = Some((40, 900_000));
        let w = drop_window(&mut seen, drops(1), 10_000, 2);
        assert_eq!((w.events, w.from_pair), (1, 0));
        let mut seen = Some((0, 900_000));
        let w = drop_window(&mut seen, drops(4), 10_000, 2);
        assert_eq!((w.events, w.from_pair), (4, 0), "fewer pairs: a restart");
    }

    // ── Read-loop occupancy ─────────────────────────────────────────────────

    #[test]
    fn occupancy_is_the_share_of_the_loop_spent_working() {
        assert_eq!(occupancy(900, 100), Some(0.1), "mostly waiting is healthy");
        assert_eq!(occupancy(500, 500), Some(0.5));
        assert_eq!(
            occupancy(0, 1_000),
            Some(1.0),
            "never blocking means the loop is behind"
        );
    }

    /// Before the first pass there is no window, and dividing by one that does
    /// not exist would report a confident 0 or 100 per cent.
    #[test]
    fn no_window_yet_is_not_an_answer() {
        assert_eq!(occupancy(0, 0), None);
    }

    /// Two counters read at different instants can in principle be nonsense.
    /// Saturating rather than panicking is the right response in a poll loop.
    #[test]
    fn absurd_counters_do_not_panic() {
        assert_eq!(occupancy(u64::MAX, u64::MAX), None, "the sum overflows");
        assert_eq!(occupancy(u64::MAX, 0), Some(0.0));
    }

    // ── The sample-rate baseline ────────────────────────────────────────────
    //
    // The fixture reproduces the measurement that motivated it: a driver that
    // hands over whole 32768-byte blocks, so each 200 ms window carries a whole
    // number of them and the count wobbles by a few either way.

    const BLOCK_BYTES: u64 = 32_768;
    const WINDOW_US: u64 = 200_000;
    /// 10 Msps of CS8 is 20 MB/s, which is 4 000 000 bytes per 200 ms window,
    /// i.e. 122.07 blocks. A real window therefore holds 122 or 123, and the
    /// hardware capture ranged over 120 to 127.
    const TRUE_RATE: u64 = 10_000_000;

    /// Feed `b` with windows from a device running at `rate` samples per second
    /// that delivers **whole blocks only**.
    ///
    /// The quantisation is produced rather than copied from the capture: bytes
    /// accumulate at the true rate, a window carries away as many whole blocks
    /// as have accumulated, and the remainder carries into the next one. That
    /// is the physical situation, and it is why the per-window count wobbles
    /// while the long-run total does not.
    ///
    /// Writing this out was worth it: the first version of these tests hardcoded
    /// eight block counts read off the capture, whose mean was 123.125 against a
    /// true 122.07. The fixture itself ran 8600 ppm fast and failed the test it
    /// was written to support.
    fn feed_quantised(b: &mut RateBaseline, rate: u64, bytes_per_pair: u64, windows: usize) {
        let mut pending = 0u64;
        for _ in 0..windows {
            pending += rate * bytes_per_pair * WINDOW_US / 1_000_000;
            let delivered = (pending / BLOCK_BYTES) * BLOCK_BYTES;
            pending -= delivered;
            b.push(WINDOW_US, delivered);
        }
    }

    fn ppm_off(rate: u32) -> i64 {
        (rate as i64 - TRUE_RATE as i64) * 1_000_000 / TRUE_RATE as i64
    }

    /// A single window cannot measure a clock, and the estimate is not asked to.
    #[test]
    fn a_short_baseline_is_refused_rather_than_answered_badly() {
        let mut b = RateBaseline::default();
        b.push(WINDOW_US, 127 * BLOCK_BYTES);
        assert_eq!(b.rate(2), None, "200 ms is not a baseline");
        feed_quantised(&mut b, TRUE_RATE, 2, 20); // 4 s
        assert_eq!(b.rate(2), None, "nor is 4 s, which measured -1379 ppm");
    }

    /// The whole reason the tracker exists: with the window timed between block
    /// arrivals, a whole-block byte count divides by the interval those exact
    /// blocks spanned, and the quantisation is gone rather than averaged down.
    ///
    /// Sized on the HackRF's transfer, which is the case that exposed it: 131072
    /// pairs is 262144 bytes, eight times the SoapySDR read, and one of those
    /// over a 30 s baseline is 437 ppm against a 500 ppm threshold. Measured on
    /// hardware, the native bench swung between -159 and +499 ppm on a stream
    /// whose real offset is about +130.
    #[test]
    fn timing_between_block_arrivals_removes_the_quantisation() {
        const HACKRF_BLOCK: u64 = 262_144;
        let base = std::time::Instant::now();
        let mut t = RateTracker::default();

        // A device delivering whole HackRF transfers at exactly 10 Msps. Poll
        // windows fall wherever they fall, so each carries a varying number of
        // blocks, but every block boundary lands on the exact rate.
        let block_us = HACKRF_BLOCK * 1_000_000 / (TRUE_RATE * 2); // 13107 us
        let mut blocks_sent = 0u64;
        for window in 1..=200u64 {
            // 200 ms of polling holds 15.26 blocks, so the count alternates.
            let want = window * 200_000 / block_us;
            let n = want - blocks_sent;
            blocks_sent = want;
            let at = base + std::time::Duration::from_micros(blocks_sent * block_us);
            t.push(Some(at), n * HACKRF_BLOCK);
        }
        let ppm = ppm_off(t.rate(2).expect("plenty of baseline"));
        assert!(
            ppm.abs() < 50,
            "block-boundary timing should be all but exact: {ppm} ppm"
        );
    }

    /// A window with no block in it carries no boundary, and the first window
    /// after a reset has nothing earlier to measure from. Neither may fold.
    #[test]
    fn a_window_without_a_block_boundary_is_skipped() {
        let base = std::time::Instant::now();
        let mut t = RateTracker::default();
        t.push(None, 4_000_000);
        assert_eq!(t.rate(2), None, "no boundary, nothing to measure");
        t.push(Some(base), 4_000_000);
        assert_eq!(t.rate(2), None, "the first boundary is only a start point");

        // And a reset puts it back to needing a fresh start point.
        for i in 1..=200u64 {
            t.push(
                Some(base + std::time::Duration::from_micros(i * 200_000)),
                4_000_000,
            );
        }
        assert!(t.rate(2).is_some());
        t.reset();
        assert_eq!(
            t.rate(2),
            None,
            "a reset forgets the baseline and its origin"
        );
    }

    /// The stride, at all three widths. `Int16` is the one that was wrong: the
    /// hardcoded 2 reported double the true rate for a four-byte pair.
    #[test]
    fn the_rate_divides_by_the_geometrys_stride() {
        let mut narrow = RateBaseline::default();
        let mut wide = RateBaseline::default();
        for _ in 0..30 {
            // The same sample count at each width is twice the bytes at Int16.
            narrow.push(WINDOW_US, 2_000_000);
            wide.push(WINDOW_US, 4_000_000);
        }
        assert_eq!(
            narrow.rate(2),
            wide.rate(4),
            "the same stream of pairs reads as the same rate at either width"
        );
        assert_eq!(
            wide.rate(2),
            narrow.rate(2).map(|r| r * 2),
            "and reading a four-byte pair as two bytes doubles it, which is the bug"
        );
    }

    /// The baseline slides rather than growing forever, so a rate change is not
    /// averaged against the old rate for the rest of the session.
    #[test]
    fn the_baseline_slides_and_forgets() {
        let mut b = RateBaseline::default();
        for _ in 0..400 {
            b.push(WINDOW_US, 4_000_000);
        }
        assert!(
            b.elapsed_us <= RATE_BASELINE_US + WINDOW_US,
            "80 s of windows must not accumulate into an 80 s baseline: {}",
            b.elapsed_us
        );
    }

    /// A window with no elapsed time is not a window, and must not divide.
    #[test]
    fn a_zero_length_window_is_ignored() {
        let mut b = RateBaseline::default();
        b.push(0, 4_000_000);
        assert_eq!(b.rate(2), None);
        assert_eq!(b.elapsed_us, 0);
    }
}
