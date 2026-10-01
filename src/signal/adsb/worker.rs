// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

use std::sync::{Arc, Mutex};
use std::time::Instant;

use crossbeam_channel::Receiver;

use crate::hardware::{SampleGeometry, StreamBlock};
use crate::signal::adsb::AdsbReceiver;
use crate::state::{AdsbFrameEntry, SdrMetrics};

pub struct AdsbWorker {
    pub sample_rx: Receiver<StreamBlock>,
    pub state: Arc<Mutex<SdrMetrics>>,
    pub geometry: SampleGeometry,
}

impl AdsbWorker {
    pub fn new(
        sample_rx: Receiver<StreamBlock>,
        state: Arc<Mutex<SdrMetrics>>,
        geometry: SampleGeometry,
    ) -> Self {
        Self {
            sample_rx,
            state,
            geometry,
        }
    }

    pub fn run(self) {
        let mut receiver = AdsbReceiver::new();
        let mut last_seq: Option<u64> = None;
        let mut next_pair: Option<u64> = None;
        let mut last_centre: Option<u64> = None;
        let mut last_rate = 0.0;
        let mut iq = Vec::new();
        let pair_bytes = self.geometry.bytes_per_pair() as u64;

        while let Ok(StreamBlock {
            seq,
            gap_before,
            bytes,
            first_pair,
            centre_hz,
            rate_hz,
        }) = self.sample_rx.recv()
        {
            let retuned = last_centre.is_some_and(|centre| centre != centre_hz)
                || (last_rate > 0.0 && (last_rate - rate_hz).abs() > 1.0);
            let gap = gap_before
                || last_seq.is_some_and(|previous| seq != previous.wrapping_add(1))
                || next_pair.is_some_and(|expected| expected != first_pair)
                || retuned;
            last_seq = Some(seq);
            next_pair = Some(first_pair + bytes.len() as u64 / pair_bytes.max(1));
            last_centre = Some(centre_hz);
            last_rate = rate_hz;

            if gap {
                receiver.reset();
            }

            let active = {
                let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
                let active = state.ui.is_adsb_section() && state.radio.hw_streaming;
                if active {
                    state.adsb.rate_supported = crate::signal::adsb::supports_sample_rate(rate_hz);
                    state.adsb.tuned_hz = centre_hz;
                    state.adsb.sample_rate_hz = rate_hz;
                }
                active
            };
            if !active {
                receiver.reset();
                continue;
            }

            crate::signal::demod::decode(&bytes, self.geometry, usize::MAX, &mut iq);
            let Some(frames) = receiver.push_iq(&iq, rate_hz) else {
                continue;
            };
            if frames.is_empty() {
                continue;
            }

            let now = Instant::now();
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            for frame in frames {
                let summary = frame
                    .summary
                    .lines()
                    .find(|line| !line.trim().is_empty())
                    .unwrap_or("Mode S frame")
                    .trim()
                    .to_string();
                let signal_dbfs = if frame.signal_level > 0.0 {
                    (10.0 * frame.signal_level.log10()).max(-120.0) as f32
                } else {
                    -120.0
                };
                state.adsb.push_frame(AdsbFrameEntry {
                    received_at: now,
                    icao_address: frame.icao_address,
                    downlink_format: frame.downlink_format,
                    signal_dbfs,
                    summary,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_worker_type_is_constructible_with_the_sample_geometry() {
        let (_tx, rx) = crossbeam_channel::bounded(1);
        let state = Arc::new(Mutex::new(SdrMetrics::fixture()));
        let worker = AdsbWorker::new(rx, state, SampleGeometry::default());
        assert_eq!(worker.geometry.bytes_per_pair(), 2);
    }
}
