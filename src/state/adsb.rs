// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

use std::collections::VecDeque;
use std::time::Instant;

pub const ADSB_FRAME_HISTORY_LIMIT: usize = 128;

#[derive(Clone, Debug)]
pub struct AdsbFrameEntry {
    pub received_at: Instant,
    pub icao_address: Option<u32>,
    pub downlink_format: u8,
    pub signal_dbfs: f32,
    pub summary: String,
}

#[derive(Clone)]
pub struct AdsbState {
    pub frames: VecDeque<AdsbFrameEntry>,
    pub frames_session: u64,
    pub last_frame_at: Option<Instant>,
    pub tuned_hz: u64,
    pub sample_rate_hz: f64,
    pub rate_supported: bool,
}

impl Default for AdsbState {
    fn default() -> Self {
        Self {
            frames: VecDeque::with_capacity(ADSB_FRAME_HISTORY_LIMIT),
            frames_session: 0,
            last_frame_at: None,
            tuned_hz: 0,
            sample_rate_hz: 0.0,
            rate_supported: false,
        }
    }
}

impl AdsbState {
    pub fn push_frame(&mut self, frame: AdsbFrameEntry) {
        if self.frames.len() == ADSB_FRAME_HISTORY_LIMIT {
            self.frames.pop_front();
        }
        self.last_frame_at = Some(frame.received_at);
        self.frames_session = self.frames_session.saturating_add(1);
        self.frames.push_back(frame);
    }
}
