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
    /// One value per stage, in the order `caps.gain.stages()` lists them.
    ///
    /// **The single source of truth for gain.** It used to be two `u32` fields
    /// named after a HackRF, which is the shape the other two radios were forced
    /// into: an RTL-SDR's `vga` meant nothing and a SoapySDR device's `lna` was a
    /// whole-chain figure. Position, not name, decides what a value is now.
    ///
    /// `f64` because a stage can have a fractional step or a negative minimum;
    /// see [`crate::hardware::StageSpec`].
    pub gains: Vec<f64>,
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

impl RadioState {
    /// The front stage's value, rounded, for the many readouts that still speak
    /// in whole dB.
    ///
    /// **A view, not a second copy.** The vector is the truth; this is the
    /// projection the existing panels were written against. A device whose
    /// stages have fractional steps is displayed to the nearest dB by these two
    /// until the panels learn otherwise, which is a display limit rather than a
    /// storage one.
    pub fn primary_gain(&self) -> u32 {
        Self::whole(self.gains.first().copied())
    }

    /// The second stage, or zero on a device that has only one.
    pub fn secondary_gain(&self) -> u32 {
        Self::whole(self.gains.get(1).copied())
    }

    /// Everything the chain is contributing, added up.
    ///
    /// What a single-knob device's readout means. On an RTL-SDR there is one
    /// stage so this equals the primary; on a SoapySDR device the knob sets a
    /// total that sdrtop then distributes, and this is the figure that was
    /// actually achieved.
    pub fn total_gain(&self) -> f64 {
        self.gains.iter().copied().filter(|v| v.is_finite()).sum()
    }

    /// One stage by position, exact.
    ///
    /// The exact-value pair to [`Self::set_stage_gain`], read from G8 where the
    /// knob starts distributing across stages. The two rounding views above are
    /// what the panels use until then.
    #[allow(dead_code)] // read from G8
    pub fn stage_gain(&self, index: usize) -> f64 {
        self.gains.get(index).copied().unwrap_or(0.0)
    }

    /// Set one stage by position, growing the vector if the caller is ahead of
    /// it. Nothing here snaps: that is [`crate::hardware::StageSpec::snap`]'s
    /// job and it needs the shape, which lives in `caps`.
    pub fn set_stage_gain(&mut self, index: usize, db: f64) {
        if self.gains.len() <= index {
            self.gains.resize(index + 1, 0.0);
        }
        self.gains[index] = db;
    }

    pub fn set_primary_gain(&mut self, db: u32) {
        self.set_stage_gain(0, db as f64);
    }

    pub fn set_secondary_gain(&mut self, db: u32) {
        self.set_stage_gain(1, db as f64);
    }

    fn whole(v: Option<f64>) -> u32 {
        v.filter(|x| x.is_finite())
            .map(|x| x.max(0.0).round() as u32)
            .unwrap_or(0)
    }
}
