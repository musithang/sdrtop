// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

/// Ring-buffer capacity for the IQ constellation (number of normalised sample pairs).
/// Oldest pairs are discarded when this limit is reached.
pub const CONSTELLATION_CAP: usize = 1024;

/// Lab IQ correction state - the live DSP behind the `[D]` DC-block / `[C]`
/// auto-cal chips and `[F]` freeze. Coefficients are applied in the RX hot path
/// ([`process_block`](crate::hardware::process::process_block)) to the samples that feed
/// the FFT and the constellation, so the spectrum/scope DC spike and the cloud
/// actually clean up. The *metrics* stay measured on the raw stream, so the
/// diagnostics keep reporting the true hardware impairment being compensated.
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
    /// Apply the active correction to one raw sample (display path only): remove DC
    /// when blocking or calibrating, then the Q-row matrix when calibrated.
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
    /// ADC-loading bell: bin `((v + 128) / 8)`, bin 16 = mid-scale, 0/31 = the rails.
    /// Snapshotted from the accumulator each ~200 ms window, like `iq_amplitude_hist`.
    pub adc_signed_hist: [u64; 32],
    pub buf_fill_pct: f32,
    pub buf_fill_history: std::collections::VecDeque<u64>,
    pub phase_imbalance_deg: f32,
    /// Live I/Q correction state ([D] DC-block / [C] auto-cal / [F] freeze).
    pub cal: IqCalState,
    /// IRR (image-rejection ratio, dB) trend history for the Lab IQ diagnostics
    /// sparkline. Sampled at the same ~500 ms cadence and [`SNR_HISTORY_LEN`] depth
    /// as the command-rail SIGNAL traces so a full panel-width sweep ≈ 60 s.
    pub irr_history: std::collections::VecDeque<f32>,
    /// Decimated I/Q sample ring buffer for the 2-D constellation display.
    /// Values are normalised to [-1, 1] (divided by 128). Written in the RX
    /// hot-path at a 1 : [`CONST_DECIMATE`] decimation; oldest pairs are
    /// evicted once the buffer reaches [`CONSTELLATION_CAP`].
    pub constellation: std::collections::VecDeque<(f32, f32)>,
}
