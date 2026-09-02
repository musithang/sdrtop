// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

use std::collections::VecDeque;
use std::time::Instant;

/// Raw accumulators written by the hardware RX callback and consumed by the
/// polling task. Never visible to the UI layer.
#[derive(Clone, Default)]
pub(crate) struct Accumulators {
    pub drops: u64,
    pub saturated: u64,
    pub i_sum: i64,
    pub q_sum: i64,
    pub i_sq_sum: u64,
    pub q_sq_sum: u64,
    pub sample_count: u64,
    pub jitter_sum_us: u64,
    pub jitter_sq_sum: u64,
    pub jitter_count: u64,
    pub iq_cross_sum: i64,
    pub last_callback: Option<Instant>,
    /// Rolling window of recent inter-callback gaps (µs), newest at the back.
    /// Unlike the sum/variance accumulators above, this is NOT zeroed each poll -
    /// it is a continuous ring the poll task only snapshots, feeding the
    /// `lab_timing` strip chart and the deadline / late-callback math. Deviation
    /// from the expected period is derived downstream, not stored here.
    pub cb_gaps_us: VecDeque<u64>,
    pub iq_hist: [u64; 32],
    /// Signed per-sample histogram (I and Q each binned) for the ADC-loading bell:
    /// bin `((v + 128) / 8)`, so bin 16 is mid-scale, 0/31 the rails.
    pub adc_signed_hist: [u64; 32],
    /// Loudest sample magnitude this window (max |i|,|q|), for the ADC peak level.
    pub peak_amp: u32,
}
