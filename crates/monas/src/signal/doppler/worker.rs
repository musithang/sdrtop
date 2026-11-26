// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The doppler lab's worker: blocks off its feed, one tracker per carrier,
//! rows and their differentials into the state.
//!
//! **A window never spans two tunings.** A retune or a rate change rebuilds
//! every tracker, and a window they held part of is thrown away and counted,
//! because a spectrum of samples from two centres describes neither.
//!
//! **Window numbers never go back.** A tracker numbers its windows by stream
//! position, which starts again from zero when the radio is opened again and
//! counts in larger hops at a higher rate. Each rebuilt tracker is therefore
//! lifted, on its first row, above the highest number the state has given
//! out, so a client asking for rows newer than its last still gets them.
//! Carriers of one resolution share their numbers, and are lifted alike.
//! Everything is computed outside the lock; the results go in under one.

use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crossbeam_channel::Receiver;
use rustfft::num_complex::Complex;

use super::{differential, CarrierSetting, Row, Tracker};
use crate::hardware::{SampleGeometry, StreamBlock};
use crate::state::doppler::DiffRow;
use crate::state::SdrMetrics;

pub struct DopplerWorker {
    rx: Receiver<StreamBlock>,
    state: Arc<Mutex<SdrMetrics>>,
    geometry: SampleGeometry,
}

impl DopplerWorker {
    pub fn new(
        rx: Receiver<StreamBlock>,
        state: Arc<Mutex<SdrMetrics>>,
        geometry: SampleGeometry,
    ) -> Self {
        Self {
            rx,
            state,
            geometry,
        }
    }

    pub fn run(self) {
        let mut session = Session::new(self.geometry);
        while let Ok(block) = self.rx.recv() {
            session.block(&block, &self.state);
        }
    }
}

struct Session {
    geometry: SampleGeometry,
    iq: Vec<Complex<f32>>,
    carriers: Vec<CarrierSetting>,
    tuning: Option<(u64, f64)>,
    trackers: Vec<Result<Tracker, String>>,
    /// The highest window number the state had given out when the trackers
    /// were built, and what each tracker's numbers are lifted by, fixed on
    /// its first row.
    floor: Option<u64>,
    lifts: Vec<Option<u64>>,
}

impl Session {
    fn new(geometry: SampleGeometry) -> Self {
        Self {
            geometry,
            iq: Vec::new(),
            carriers: Vec::new(),
            tuning: None,
            trackers: Vec::new(),
            floor: None,
            lifts: Vec::new(),
        }
    }

    fn block(&mut self, b: &StreamBlock, state: &Mutex<SdrMetrics>) {
        let (carriers, last_seq) = {
            let m = state.lock().unwrap_or_else(|e| e.into_inner());
            (m.doppler.carriers.clone(), m.doppler.last_seq)
        };
        crate::signal::demod::decode(&b.bytes, self.geometry, usize::MAX, &mut self.iq);

        // New carriers, or the same carriers at a new tuning: rebuild, and
        // count each window a retune cut short.
        let tuning = (b.centre_hz, b.rate_hz);
        let mut discarded = vec![false; carriers.len()];
        if carriers != self.carriers || self.tuning != Some(tuning) {
            if carriers == self.carriers {
                for (d, t) in discarded.iter_mut().zip(&self.trackers) {
                    *d = t.as_ref().is_ok_and(|t| t.holds_partial_window());
                }
            }
            self.trackers = carriers
                .iter()
                .map(|c| Tracker::new(*c, b.rate_hz, b.centre_hz))
                .collect();
            self.carriers = carriers.clone();
            self.tuning = Some(tuning);
            self.floor = last_seq;
            self.lifts = vec![None; self.trackers.len()];
        }

        let floor = self.floor;
        let rows: Vec<Vec<Row>> = self
            .trackers
            .iter_mut()
            .zip(self.lifts.iter_mut())
            .map(|(t, lift)| match t {
                Ok(t) => {
                    let mut rows = t.push(&self.iq, b.first_pair, b.gap_before);
                    if let Some(first) = rows.first() {
                        let by = *lift.get_or_insert_with(|| {
                            floor.map_or(0, |f| (f + 1).saturating_sub(first.seq))
                        });
                        for r in rows.iter_mut() {
                            r.seq += by;
                        }
                    }
                    rows
                }
                Err(_) => Vec::new(),
            })
            .collect();
        let mut diffs = Vec::new();
        if let (Some(first), Some(f1)) = (rows.first(), carriers.first()) {
            for (i, other) in rows.iter().enumerate().skip(1) {
                for r1 in first {
                    if let Some(ri) = other.iter().find(|r| r.seq == r1.seq) {
                        diffs.push((
                            r1.seq,
                            carriers[i].hz,
                            differential(f1.hz, &r1.line, carriers[i].hz, &ri.line),
                        ));
                    }
                }
            }
        }

        let now = Instant::now();
        let unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
        let d = &mut m.doppler;
        d.feed.blocks_in += 1;
        d.feed.last_block = Some(now);
        if b.gap_before {
            d.feed.gaps += 1;
            d.feed.last_loss = Some(now);
        }
        // A command may have chosen other carriers while this block was
        // measured: its rows belong to the old ones, and are dropped.
        if d.carriers != carriers {
            return;
        }
        let differenced = d.differential_refused.is_none();
        for (i, (track, rows)) in d.tracks.iter_mut().zip(rows).enumerate() {
            track.refused = self.trackers[i].as_ref().err().cloned();
            if discarded[i] {
                track.discarded += 1;
            }
            for row in rows {
                d.last_seq = Some(d.last_seq.map_or(row.seq, |s| s.max(row.seq)));
                track.push(row, now, unix_ms);
            }
        }
        if differenced {
            for (seq, fi_hz, line) in diffs {
                d.push_differential(DiffRow {
                    seq,
                    fi_hz,
                    line,
                    at: now,
                    at_unix_ms: unix_ms,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::StreamBlock;
    use crate::signal::doppler::testing::{add, carrier, int32_bytes, int32_geometry, noise};
    use crate::signal::doppler::CarrierSetting;
    use crate::state::SdrMetrics;
    use std::sync::{Arc, Mutex};

    const RATE: f64 = 96_000.0;
    const CENTRE: u64 = 10_000_000;

    fn setting(hz: u64) -> CarrierSetting {
        CarrierSetting {
            hz,
            span_hz: 25.0,
            resolution_hz: 0.1,
        }
    }

    fn state(carriers: Vec<CarrierSetting>) -> Arc<Mutex<SdrMetrics>> {
        let mut m = SdrMetrics::fixture().streaming();
        m.mode = crate::engine::mode::Mode::named("lab_doppler").unwrap();
        m.doppler.set_carriers(carriers);
        Arc::new(Mutex::new(m))
    }

    /// One stretch of stream: `seconds` from pair `start` at `centre`, two
    /// tones (at 20 kHz and 24 kHz above `CENTRE`, each 0.2 Hz up) in noise.
    struct Stretch {
        start: u64,
        seconds: f64,
        centre: u64,
        gap_before: bool,
    }

    /// Feed the stretches through a running worker and hand back its state.
    fn feed(carriers: Vec<CarrierSetting>, stretches: &[Stretch]) -> SdrMetrics {
        let st = state(carriers);
        feed_into(&st, stretches);
        let m = st.lock().unwrap().clone();
        m
    }

    /// The same, into a state that may already hold rows, through a new
    /// worker, as after the radio is opened again.
    fn feed_into(st: &Arc<Mutex<SdrMetrics>>, stretches: &[Stretch]) {
        let (tx, rx) = crossbeam_channel::bounded(4);
        let worker = DopplerWorker::new(rx, Arc::clone(st), int32_geometry());
        let running = std::thread::spawn(move || worker.run());
        let (mut p1, mut p2, mut seed, mut seq) = (0.0, 1.0, 21u64, 0u64);
        for s in stretches {
            let total = (s.seconds * RATE) as u64;
            let mut at = 0u64;
            let mut first = true;
            while at < total {
                let n = 16_384usize.min((total - at) as usize);
                let pair = s.start + at;
                // The tones sit at fixed radio frequencies: their baseband
                // offsets follow the tuning.
                let shift = CENTRE as f64 - s.centre as f64;
                let a = carrier(RATE, &|_| 20_000.2 + shift, 0.002, pair, n, &mut p1);
                let b = carrier(RATE, &|_| 24_000.2 + shift, 0.002, pair, n, &mut p2);
                let iq = add(add(a, &b), &noise(n, 0.05, &mut seed));
                seq += 1;
                tx.send(StreamBlock {
                    seq,
                    gap_before: s.gap_before && std::mem::take(&mut first),
                    bytes: int32_bytes(&iq, 8_388_608.0),
                    first_pair: pair,
                    centre_hz: s.centre,
                    rate_hz: RATE,
                })
                .unwrap();
                at += n as u64;
            }
        }
        drop(tx);
        running.join().unwrap();
    }

    fn stretch(start_s: f64, seconds: f64, centre: u64, gap_before: bool) -> Stretch {
        Stretch {
            start: (start_s * RATE) as u64,
            seconds,
            centre,
            gap_before,
        }
    }

    #[test]
    fn a_synthetic_stream_becomes_rows_in_the_state() {
        let m = feed(
            vec![setting(CENTRE + 20_000)],
            &[stretch(0.0, 25.0, CENTRE, false)],
        );
        let rows = &m.doppler.tracks[0].rows;
        assert!(rows.len() >= 3, "{}", rows.len());
        let l = rows.back().unwrap().row.line.as_ref().unwrap();
        assert!((l.hz - 0.2).abs() < 0.02, "{}", l.hz);
        assert!(m.doppler.feed.blocks_in > 0);
    }

    /// **A retune inside a window discards it**: no row is made of samples
    /// from two tunings, and the discard is counted.
    #[test]
    fn a_retune_inside_a_window_discards_it() {
        let switch_s = 7.0;
        let m = feed(
            vec![setting(CENTRE + 20_000)],
            &[
                stretch(0.0, switch_s, CENTRE, false),
                stretch(switch_s, 25.0, CENTRE + 1_000, false),
            ],
        );
        let t = &m.doppler.tracks[0];
        assert!(t.discarded >= 1);
        for r in &t.rows {
            let begins = r.row.end_pair as f64 / RATE - r.row.window_s;
            assert!(
                begins >= switch_s - 0.05,
                "row {} spans the retune",
                r.row.seq
            );
        }
        assert!(!t.rows.is_empty());
    }

    #[test]
    fn two_carriers_share_seq_and_make_a_differential() {
        let m = feed(
            vec![setting(CENTRE + 20_000), setting(CENTRE + 24_000)],
            &[stretch(0.0, 25.0, CENTRE, false)],
        );
        let seqs = |i: usize| {
            m.doppler.tracks[i]
                .rows
                .iter()
                .map(|r| r.row.seq)
                .collect::<Vec<_>>()
        };
        assert_eq!(seqs(0), seqs(1));
        let d = m.doppler.differential.back().unwrap();
        assert_eq!(d.fi_hz, CENTRE + 24_000);
        let l = d.line.as_ref().unwrap();
        // Both tones 0.2 Hz up: the second against the first, scaled.
        let k = (CENTRE + 24_000) as f64 / (CENTRE + 20_000) as f64;
        assert!(
            (l.hz - (0.2 - k * 0.2)).abs() < 3.0 * l.sigma_hz + 0.005,
            "{}",
            l.hz
        );
    }

    /// **The window numbers never go back**: the radio opened again counts
    /// its stream from zero, and a client asking for rows newer than the last
    /// it had must still get the new ones.
    #[test]
    fn window_numbers_keep_rising_when_the_stream_starts_over() {
        let st = state(vec![setting(CENTRE + 20_000), setting(CENTRE + 24_000)]);
        feed_into(&st, &[stretch(0.0, 60.0, CENTRE, false)]);
        let before = st.lock().unwrap().doppler.tracks[0]
            .rows
            .back()
            .unwrap()
            .row
            .seq;
        feed_into(&st, &[stretch(0.0, 25.0, CENTRE, true)]);
        let m = st.lock().unwrap().clone();
        for t in &m.doppler.tracks {
            let seqs: Vec<u64> = t.rows.iter().map(|r| r.row.seq).collect();
            assert!(seqs.windows(2).all(|w| w[1] > w[0]), "{seqs:?}");
            assert!(*seqs.last().unwrap() > before + 2, "{seqs:?}");
        }
        let d: Vec<u64> = m.doppler.differential.iter().map(|d| d.seq).collect();
        assert!(d.windows(2).all(|w| w[1] > w[0]), "{d:?}");
        assert_eq!(
            d.last(),
            m.doppler.tracks[1].rows.back().map(|r| &r.row.seq)
        );
    }

    /// **Switched away and back**: no row spans the pause, and the first
    /// window after it starts clean.
    #[test]
    fn switching_away_and_back_starts_a_clean_window() {
        let m = feed(
            vec![setting(CENTRE + 20_000)],
            &[
                stretch(0.0, 12.0, CENTRE, false),
                stretch(72.0, 25.0, CENTRE, true),
            ],
        );
        let rows = &m.doppler.tracks[0].rows;
        for r in rows {
            let (begins, ends) = (
                r.row.end_pair as f64 / RATE - r.row.window_s,
                r.row.end_pair as f64 / RATE,
            );
            assert!(
                ends <= 12.01 || begins >= 71.99,
                "row {} spans the pause",
                r.row.seq
            );
        }
        // The first window after it is wholly after it, so nothing inside it
        // was lost: it is clean, and starts no earlier than the stream did.
        let after = rows
            .iter()
            .find(|r| r.row.end_pair as f64 / RATE > 72.0)
            .unwrap();
        assert!(after.row.end_pair as f64 / RATE - after.row.window_s >= 71.99);
        assert!(!after.row.feed_loss);
    }
}
