// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

/// Ring-buffer capacity for the IQ constellation (number of normalised sample pairs).
/// Oldest pairs are discarded when this limit is reached.
pub const CONSTELLATION_CAP: usize = 1024;

/// Lab IQ correction state - the live DSP behind the `[D]` DC-block / `[C]`
/// auto-cal chips and `[F]` freeze. Coefficients are applied in the RX hot path
/// ([`digest`](crate::hardware::process::digest)) to the samples
/// that feed the FFT, the demod and the constellation, so the spectrum/scope DC
/// spike and the cloud actually clean up.
///
/// **The accumulators stay on the raw stream; the readings do not.** Those are
/// two different things and this comment used to run them together. The
/// per-sample sums are taken before any correction, because a correction has to
/// be built from what the front end actually did. What the IQ bench then prints
/// is the *residual* after the active correction, so a lit `[C]` beside a
/// still-bad number means the correction is not keeping up, rather than the
/// panel reporting a fault the app is already cancelling. The split is made in
/// `tasks::rx::metrics::iq_metrics`, whose header is the one account of it.
#[derive(Clone, Copy)]
pub struct IqCalState {
    /// `[D]` - subtract the live DC estimate from the stream.
    pub dc_block_on: bool,
    /// `[C]` - an I/Q amplitude+phase correction matrix has been captured & applied.
    pub cal_applied: bool,
    /// `[C]` was just pressed; the next metrics cycle captures the coefficients.
    pub cal_pending: bool,
    /// Unix seconds of the last successful auto-cal (for "last cal Xm ago").
    pub last_cal_at: Option<u64>,
    /// `[F]` - pause constellation accumulation (the cloud freezes in place).
    pub frozen: bool,
    /// DC to subtract, in raw sample units (mean I/Q); tracks live while correcting.
    pub dc_i_raw: f32,
    pub dc_q_raw: f32,
    /// Q-correction matrix row: `q_out = c_qi·i' + c_qq·q'` (identity = `0.0, 1.0`).
    pub c_qi: f32,
    pub c_qq: f32,
}

impl Default for IqCalState {
    fn default() -> Self {
        Self {
            dc_block_on: false,
            cal_applied: false,
            cal_pending: false,
            last_cal_at: None,
            frozen: false,
            dc_i_raw: 0.0,
            dc_q_raw: 0.0,
            c_qi: 0.0,
            c_qq: 1.0,
        }
    }
}

impl IqCalState {
    /// Apply the active correction to one raw sample: remove DC when blocking or
    /// calibrating, then the Q-row matrix when calibrated.
    ///
    /// **Not a display path**, whatever this used to say. The corrected samples
    /// are re-encoded and sent to the FFT worker, and every spectrum measurement
    /// the app makes is derived from there: the noise floor, channel power,
    /// occupied bandwidth, ACPR. The demod gets the same stream. Only the
    /// accumulators in `process_block` are left on the raw one.
    pub fn apply(&self, i: f32, q: f32) -> (f32, f32) {
        let (mut ip, mut qp) = (i, q);
        if self.dc_block_on || self.cal_applied {
            ip -= self.dc_i_raw;
            qp -= self.dc_q_raw;
        }
        if self.cal_applied {
            (ip, self.c_qi * ip + self.c_qq * qp)
        } else {
            (ip, qp)
        }
    }

    /// Whether any correction currently modifies the samples.
    pub fn correcting(&self) -> bool {
        self.dc_block_on || self.cal_applied
    }
}

#[cfg(test)]
mod cal_tests {
    use super::*;

    #[test]
    fn default_is_identity() {
        let c = IqCalState::default();
        assert!(!c.correcting());
        assert_eq!(c.apply(12.0, -7.0), (12.0, -7.0));
    }

    #[test]
    fn dc_block_subtracts_dc_only() {
        let c = IqCalState {
            dc_block_on: true,
            dc_i_raw: 5.0,
            dc_q_raw: -3.0,
            ..IqCalState::default()
        };
        assert!(c.correcting());
        assert_eq!(c.apply(10.0, 0.0), (5.0, 3.0)); // i-5, q-(-3)
    }

    #[test]
    fn cal_applied_runs_q_matrix_after_dc() {
        let c = IqCalState {
            cal_applied: true,
            c_qi: -0.5,
            c_qq: 2.0,
            ..IqCalState::default()
        };
        // I passes through; Q_out = c_qi·I + c_qq·Q (DC is zero here).
        assert_eq!(c.apply(4.0, 1.0), (4.0, -0.5 * 4.0 + 2.0 * 1.0));
    }
}

#[derive(Clone)]
pub struct IqState {
    pub iq_imbalance_db: f32,
    pub dc_offset_i: f32,
    pub dc_offset_q: f32,
    pub cb_period_us: u64,
    pub cb_jitter_us: u64,
    pub jitter_history: std::collections::VecDeque<u64>,
    pub iq_amplitude_hist: [u64; 32],
    /// Signed ADC sample histogram (I and Q binned together) for the Lab RF
    /// ADC-loading bell: the device's own -FS..+FS span cut into 32, so bin 16 is
    /// mid-scale and 0/31 are the rails. `(v + 128) / 8` on an 8-bit radio, which
    /// is what this used to say as though it were the rule; the bin width comes
    /// from the declared full scale, so a 12-bit converter's bell fills the same
    /// 32 buckets instead of huddling in the middle four.
    /// Snapshotted from the accumulator each ~200 ms window, like `iq_amplitude_hist`.
    pub adc_signed_hist: [u64; 32],
    /// How full the FFT feed's queue got, as a percentage of its depth.
    ///
    /// **The deepest it reached during the window, not its depth at poll time.**
    /// The queue holds four blocks and the FFT worker drains it continuously, so
    /// a reading taken once every 200 ms is a point sample of something that is
    /// almost always empty: it read a comfortable 0 % straight through backlogs
    /// it never happened to land in. The high-water mark is kept in the hot path
    /// by [`crate::hardware::FeedHealth`] instead.
    pub buf_fill_pct: f32,
    pub buf_fill_history: std::collections::VecDeque<u64>,
    /// Blocks the FFT feed had to refuse in the last poll window, and since the
    /// session started.
    ///
    /// Not folded into `signal.drops_per_sec`, which counts samples the *radio*
    /// lost. This is a block sdrtop threw away because its own FFT worker was
    /// behind: a different fact, with a different cause and a different fix, and
    /// one witness each.
    pub fft_drops: u64,
    pub fft_drops_session: u64,
    pub phase_imbalance_deg: f32,
    /// Live I/Q correction state (`[D]` DC-block / `[C]` auto-cal / `[F]` freeze).
    pub cal: IqCalState,
    /// IRR (image-rejection ratio, dB) trend history for the Lab IQ diagnostics
    /// sparkline. Sampled at the same ~500 ms cadence and [`super::SNR_HISTORY_LEN`] depth
    /// as the command-rail SIGNAL traces so a full panel-width sweep ≈ 60 s.
    pub irr_history: std::collections::VecDeque<f32>,
    /// Decimated I/Q sample ring buffer for the 2-D constellation display.
    /// Values are normalised to [-1, 1] by the device's own full scale, which is
    /// 128 on both shipped radios and was written here as a literal back when
    /// those were the only two. Written in the RX hot-path at a
    /// 1 : `CONST_DECIMATE` (`hardware::process`) decimation; oldest pairs are evicted once the
    /// buffer reaches [`CONSTELLATION_CAP`].
    pub constellation: std::collections::VecDeque<(f32, f32)>,
}
