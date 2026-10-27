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
    /// The squares and the cross product are 128-bit because a poll window
    /// of a 32-bit format can hold more than 64 bits of them: a full-scale
    /// square is 2^46 counts on a 16-bit converter carried with eight
    /// fraction bits, 200 ms at 912 kS/s is 2^17.5 pairs, and that is already
    /// 2^63.5 before a library's correction carries a sample past its word or
    /// a poll comes late.
    pub i_sq_sum: u128,
    pub q_sq_sum: u128,
    pub sample_count: u64,
    pub jitter_sum_us: u64,
    pub jitter_sq_sum: u64,
    pub jitter_count: u64,
    pub iq_cross_sum: i128,
    pub last_callback: Option<Instant>,
    /// Rolling window of recent inter-callback gaps (µs), newest at the back.
    /// Unlike the sum/variance accumulators above, this is NOT zeroed each poll -
    /// it is a continuous ring the poll task only snapshots, feeding the
    /// `lab_timing` strip chart and the deadline / late-callback math. Deviation
    /// from the expected period is derived downstream, not stored here.
    pub cb_gaps_us: VecDeque<u64>,
    pub iq_hist: [u64; 32],
    /// Every I and Q sample by its 6 dB band below full scale, one band a
    /// bit of the converter, for the ADC Loading bench: band 0 is the top
    /// bit, the converter's last band holds everything under its LSB.
    /// `hardware::process` decides the band.
    pub adc_band_hist: [u64; crate::hardware::ADC_BANDS],
    /// Loudest sample magnitude this window (max |i|,|q|), for the ADC peak level.
    pub peak_amp: u32,
}
