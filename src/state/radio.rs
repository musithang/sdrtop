// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

use std::collections::VecDeque;
use std::time::Instant;

#[derive(Clone)]
pub struct RadioState {
    pub frequency: u64,
    pub config_sample_rate: f64,
    pub actual_sample_rate: u32,
    pub bb_filter_hz: u32,
    pub lna_gain: u32,
    pub vga_gain: u32,
    pub amp_enabled: bool,
    pub rx_enabled: bool,
    pub hw_streaming: bool,
    /// When the current RX session started - `Some` while streaming, `None` when
    /// stopped. Drives the micro_health session timer.
    pub rx_start_time: Option<Instant>,
    pub bytes_since_last_poll: u64,
    pub last_poll_time: Instant,
    pub current_throughput_bps: u64,
    pub throughput_history: VecDeque<u64>,
    pub sample_rate_history: VecDeque<u64>,
}
