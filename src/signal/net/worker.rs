// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The 2.4 GHz worker: one thread, one block at a time.
//!
//! Same shape as [`crate::signal::DemodWorker`], for the same reason: a thread
//! that owns the state carried between blocks, so that everything below it can
//! be a pure function of its arguments and be tested with no radio anywhere.
//!
//! **It counts what arrived and measures what was in the band.** Design
//! section 13.2 makes what the receiver missed a first-class displayed number
//! rather than an inference, and a feed whose losses are only visible once
//! there is something to lose is a feed nobody will trust when the losses
//! matter - so the counting was built and shown before any measurement sat on
//! top of it. The band measurement is [`super::scan`] over
//! [`super::occupancy`].
//!
//! **B6 added the first decoder: BLE, on whichever of the three advertising
//! frequencies the radio is tuned to.** It runs here rather than in its own
//! task because it needs the same per-block bytes the occupancy scan already
//! has - a second worker reading the same channel would need its own copy of
//! the geometry and the retune-detection logic this one already carries. Wi-Fi
//! and classic Bluetooth arrive the same way when their own arcs reach this
//! point.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use crossbeam_channel::Receiver as SampleReceiver;

use crate::hardware::{SampleGeometry, StreamBlock};
use crate::signal::ble::receive::Receiver as BleReceiver;
use crate::signal::stream::plan_block;
use crate::state::{BlePacket, SdrMetrics};

use super::scan::Scan;

/// How much *observation* a dwell is, before it is published and started again.
///
/// **Not a wall-clock interval, which is what this was first written as.** The
/// feed is lossy and the section can be closed and reopened, so wall time and
/// time spent looking at the band are different quantities, and a duty cycle is
/// a fraction of the second one. Counting windows means a dwell interrupted by
/// dropped blocks is a shorter dwell rather than a diluted one - and it means
/// the measurement can be tested without a clock, which is how the end-to-end
/// test below exists at all.
///
/// Fifty milliseconds is about eight thousand windows. The duty cycle that
/// supports is good to a quarter of a percent, which is finer than the whole
/// percent it is shown to and not by much - see
/// [`super::occupancy::DUTY_RESOLUTION`], where the two were made to agree. It
/// publishes at most twenty times a second against a screen that redraws thirty.
const DWELL_S: f64 = 0.05;

pub struct NetWorker {
    pub sample_rx: SampleReceiver<StreamBlock>,
    pub state: Arc<Mutex<SdrMetrics>>,
    pub geometry: SampleGeometry,
}

/// What the worker carries from one block to the next.
///
/// Three fields, and two of them exist only so that a gap can be told from a
/// pause. See [`Run::suspend`].
#[derive(Default)]
struct Run {
    last_seq: u64,
    /// The sequence of the previous block *that reached us*, or `None` when the
    /// run has not started.
    drop_ref: Option<u64>,
    /// Unbroken blocks since the last gap.
    blocks: u64,
}

impl Run {
    /// The section was closed, so the feed stopped forwarding.
    ///
    /// The device goes on counting every callback, so the next block to arrive
    /// will be thousands of sequence numbers away without a single one having
    /// been lost. Clearing `drop_ref` is what stops that jump being reported as
    /// the worst loss event of the session. The run length goes with it: there
    /// is no run any more.
    fn suspend(&mut self) {
        self.drop_ref = None;
        self.blocks = 0;
    }
}

impl NetWorker {
    pub fn new(
        sample_rx: SampleReceiver<StreamBlock>,
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
        let mut run = Run::default();
        let pair_bytes = self.geometry.bytes_per_pair() as u64;
        let mut scan: Option<Scan> = None;
        let mut ble: Option<BleReceiver> = None;

        while let Ok(StreamBlock {
            seq,
            gap_before,
            bytes,
        }) = self.sample_rx.recv()
        {
            let started = run.drop_ref.is_some();
            let plan = plan_block(seq, gap_before, run.last_seq, run.drop_ref);
            // The clock is read outside the lock, because the lock block below
            // does integer work only and a float or a syscall inside one is a
            // dropped frame on the UI thread.
            let now = Instant::now();
            run.last_seq = seq;
            run.drop_ref = Some(seq);

            let pairs = bytes.len() as u64 / pair_bytes.max(1);
            // **A run has to have started before it can be interrupted.**
            // `plan_block` guards its `dropped` count with `drop_ref` and does
            // not guard `contiguous` with anything, because for the demod the
            // difference is invisible: a first block declared discontiguous just
            // resets session state that is already empty. Here the same flag is
            // about to become a number on a panel, and the section is normally
            // opened on a radio that has been streaming for a minute - so the
            // first block through carries a sequence number thousands past
            // whatever this worker last saw, and would report an interruption
            // that never happened, once per visit.
            let broke = started && !plan.contiguous;
            run.blocks = if broke || !started { 1 } else { run.blocks + 1 };

            let (still_open, centre_hz, rate_hz, span_hz) = {
                let mut m = self.state.lock().unwrap_or_else(|e| e.into_inner());
                let h = &mut m.net.health;
                h.blocks_in = h.blocks_in.saturating_add(1);
                h.pairs_in = h.pairs_in.saturating_add(pairs);
                h.gaps = h.gaps.saturating_add(u64::from(broke));
                h.blocks_lost = h.blocks_lost.saturating_add(plan.dropped);
                h.run_blocks = run.blocks;
                h.last_block = Some(now);
                // The usable span is the baseband filter's where the radio has
                // one, because the bins the front end rolled off carry no
                // measurement and averaging them in would drag every cell at the
                // edges of the view down towards a floor that is not the band's.
                // Where there is no filter, the rate is all we know.
                let span = if m.radio.bb_filter_hz > 0 {
                    m.radio.bb_filter_hz as f64
                } else {
                    m.radio.config_sample_rate
                };
                (
                    m.ui.is_net_section(),
                    m.radio.frequency as f64,
                    m.radio.config_sample_rate,
                    span.min(m.radio.config_sample_rate),
                )
            };

            // Retuning invalidates every cell mapping, so the scan is rebuilt
            // and whatever it had accumulated goes with it: half a dwell at one
            // frequency and half at another is a measurement of neither.
            if !scan
                .as_ref()
                .is_some_and(|s| s.matches(centre_hz, rate_hz, span_hz))
            {
                scan = Some(Scan::new(centre_hz, rate_hz, span_hz));
            }
            if let Some(scan) = scan.as_mut() {
                scan.push(&bytes, self.geometry);
                if scan.observed_s() >= DWELL_S {
                    let band = scan.take();
                    let mut m = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    m.net.band.absorb(band, now);
                }
            }

            // BLE decode: only possible on one of the three fixed advertising
            // frequencies, and only at a sample rate `receive::front_end` can
            // reach the working rate from. Neither condition is `net_survey`'s
            // to share, so this keeps its own refusal rather than reusing
            // `survey_refused`.
            let channel = crate::signal::ble::channel::channel_of(centre_hz as u64);
            match channel {
                Some(ch) if still_open => {
                    if !ble.as_ref().is_some_and(|r| r.matches(ch, rate_hz)) {
                        ble = match BleReceiver::new(rate_hz, ch) {
                            Ok(r) => {
                                let mut m = self.state.lock().unwrap_or_else(|e| e.into_inner());
                                m.net.ble_refused = None;
                                Some(r)
                            }
                            Err(reason) => {
                                let mut m = self.state.lock().unwrap_or_else(|e| e.into_inner());
                                m.net.ble_refused = Some(reason);
                                None
                            }
                        };
                    }
                    if let Some(rx) = ble.as_mut() {
                        let packets = rx.push(&bytes, self.geometry);
                        if !packets.is_empty() {
                            let mut m = self.state.lock().unwrap_or_else(|e| e.into_inner());
                            for p in packets {
                                // TEMP DEBUG
                                let hex: String = p
                                    .debug_pdu_bytes
                                    .iter()
                                    .chain(p.debug_crc_bytes.iter())
                                    .map(|b| format!("{b:02x}"))
                                    .collect::<Vec<_>>()
                                    .join(" ");
                                m.push_log(format!(
                                    "DEBUG coh={:.3} phase={:.2} pdu+crc: {hex}",
                                    p.debug_coherence, p.debug_phase
                                ));
                                m.net.ble_packets.push_front(BlePacket {
                                    channel: ch,
                                    pdu_type: p.pdu_type,
                                    tx_add_random: p.tx_add_random,
                                    length: p.length,
                                    adv_addr: p.adv_addr,
                                    crc_ok: p.crc_ok,
                                    seen: now,
                                });
                            }
                            m.net.ble_packets.truncate(crate::state::BLE_PACKET_LIMIT);
                        }
                    }
                }
                Some(_) => {
                    // Section closed; nothing decodes while it is.
                    ble = None;
                }
                None => {
                    ble = None;
                    let mut m = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    m.net.ble_refused = Some(
                        "not tuned to an advertising channel (2402, 2426 or 2480 MHz)".to_string(),
                    );
                }
            }

            // Closing the section stops `process_block` forwarding, but blocks
            // already in the channel still arrive - and the run they belong to
            // is over whether or not they are the last of it.
            if !still_open {
                run.suspend();
                // The band measurement stops with it. Nothing has been observed
                // since the section closed, and a panel reopened an hour later
                // showing the last dwell as if it were current is exactly what
                // rule 4 exists to prevent; the chrome's staleness marks it, and
                // dropping the scan means the next dwell starts clean.
                scan = None;
                ble = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::SampleFormat;

    fn eight_bit() -> SampleGeometry {
        SampleGeometry {
            format: SampleFormat::Int8,
            full_scale: 128.0,
        }
    }

    /// Run the worker over a scripted feed and read back what it recorded.
    ///
    /// The channel is closed before the worker starts, so `run` drains it and
    /// returns rather than blocking: the loop under test is the same one either
    /// way, and a test that has to join a live thread is a test that can hang
    /// CI.
    fn feed(blocks: &[(u64, bool, usize)], open: bool) -> crate::state::NetDecodeHealth {
        let mut m = SdrMetrics::fixture();
        m.ui.section = if open {
            crate::signal::net::SECTION.to_string()
        } else {
            String::new()
        };
        let state = Arc::new(Mutex::new(m));
        let (tx, rx) = crossbeam_channel::unbounded();
        for &(seq, gap_before, pairs) in blocks {
            tx.send(StreamBlock {
                seq,
                gap_before,
                bytes: vec![0u8; pairs * 2],
            })
            .unwrap();
        }
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), eight_bit()).run();
        let m = state.lock().unwrap();
        m.net.health.clone()
    }

    #[test]
    fn an_unbroken_feed_reports_no_gaps_and_one_growing_run() {
        let h = feed(&[(1, false, 64), (2, false, 64), (3, false, 64)], true);
        assert_eq!(h.blocks_in, 3);
        assert_eq!(h.pairs_in, 192, "eight-bit pairs are two bytes each");
        assert_eq!((h.gaps, h.blocks_lost), (0, 0));
        assert_eq!(h.run_blocks, 3);
        assert!(h.last_block.is_some());
    }

    /// Opening the section on a radio that is already streaming is the normal
    /// case, and it must not read as a loss.
    ///
    /// The device stamps every callback whether or not this feed is being
    /// forwarded to, so the first block through carries a sequence number
    /// thousands past whatever the worker last saw. Counting that as an
    /// interruption put a phantom gap on the panel once per visit, and a panel
    /// whose job is to say what the receiver missed is the last place that can
    /// afford one.
    #[test]
    fn opening_the_section_mid_stream_is_not_an_interruption() {
        let h = feed(&[(9_412, false, 64), (9_413, false, 64)], true);
        assert_eq!(h.gaps, 0, "nothing was interrupted; nothing had started");
        assert_eq!(h.blocks_lost, 0);
        assert_eq!(
            h.run_blocks, 2,
            "and the run is the two blocks that arrived"
        );
    }

    #[test]
    fn a_gap_breaks_the_run_and_starts_a_new_one() {
        // Three good blocks, then the driver says samples went missing.
        let h = feed(
            &[
                (1, false, 64),
                (2, false, 64),
                (3, false, 64),
                (4, true, 64),
            ],
            true,
        );
        assert_eq!(h.gaps, 1);
        assert_eq!(h.blocks_lost, 1, "the floor: samples went, count unknown");
        assert_eq!(h.run_blocks, 1, "and the new run is one block long");
    }

    #[test]
    fn blocks_lost_by_the_channel_are_counted_too() {
        // 1, then 5: three blocks the bounded feed refused, plus a driver gap.
        let h = feed(&[(1, false, 64), (5, true, 64)], true);
        assert_eq!(h.blocks_lost, 4);
        assert_eq!(h.gaps, 1);
    }

    /// End to end, with no radio: bytes in, a duty cycle out, on the cell the
    /// transmitter was actually on.
    ///
    /// The synthetic transmitter is a carrier three megahertz above the tuning,
    /// keyed on for a quarter of the time. Everything between those bytes and
    /// the number on the panel runs here: the transform, the bin-to-cell
    /// mapping, the noise floor and its correction, and the threshold.
    #[test]
    fn a_transmitter_in_the_band_lands_on_its_own_cell() {
        use crate::signal::dsp::testkit::Rng;
        use std::f64::consts::TAU;

        const RATE: f64 = 20_000_000.0;
        const CENTRE: u64 = 2_437_000_000;
        const OFFSET: f64 = 3_000_000.0;
        const DUTY: f64 = 0.25;
        // Eight microseconds at 20 Msps is 160 samples, so the transform is 128
        // and one block holds a whole number of windows.
        const PAIRS: usize = 128 * 400;

        let mut rng = Rng::new(0xB0_1234);
        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = crate::signal::net::SECTION.to_string();
        m.radio.frequency = CENTRE;
        m.radio.config_sample_rate = RATE;
        m.radio.bb_filter_hz = 18_000_000;
        let state = Arc::new(Mutex::new(m));

        let (tx, rx) = crossbeam_channel::unbounded();
        let mut phase = 0.0f64;
        // Enough blocks to complete one dwell, so the real publish path runs.
        for seq in 1..=20u64 {
            let mut bytes = Vec::with_capacity(PAIRS * 2);
            for i in 0..PAIRS {
                // Keyed in whole windows, so a window is either on or off and
                // the duty cycle the measurement recovers is the one keyed.
                let window = i / 128;
                let on = (window % 100) < (100.0 * DUTY) as usize;
                phase += TAU * OFFSET / RATE;
                let (mut re, mut im) = (0.0f64, 0.0f64);
                if on {
                    re = 40.0 * phase.cos();
                    im = 40.0 * phase.sin();
                }
                let (a, b) = rng.normal_pair();
                bytes.push((re + a).clamp(-127.0, 127.0) as i8 as u8);
                bytes.push((im + b).clamp(-127.0, 127.0) as i8 as u8);
            }
            tx.send(StreamBlock {
                seq,
                gap_before: false,
                bytes,
            })
            .unwrap();
        }
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), eight_bit()).run();

        let m = state.lock().unwrap();
        let band = &m.net.band;
        assert!(band.trusted, "tail {} spread {}", band.tail, band.spread);
        assert_eq!(band.cells.len(), crate::signal::net::occupancy::CELLS);

        // The carrier is at 2440 MHz, which is cell 40.
        let cell = crate::signal::net::occupancy::cell_of(2_440_000_000.0).unwrap();
        assert_eq!(cell, 40);
        let hit = &band.cells[cell];
        assert!(hit.observed());
        assert!(
            (hit.duty - DUTY).abs() < 0.02,
            "cell {cell} read {} for a keyed {DUTY}",
            hit.duty
        );

        // A cell nobody transmitted in reads empty, which is a measurement.
        let quiet = &band.cells[30];
        assert!(quiet.observed());
        assert_eq!(quiet.duty, 0.0);
        assert!(quiet.mean_dbfs < hit.mean_dbfs - 10.0);

        // A cell outside the eighteen megahertz in view was not looked at, and
        // that is a different answer from an empty one.
        assert!(!band.cells[0].observed());
        assert!(!band.cells[60].observed());
    }

    /// A dwell is published when it is a dwell, and not before.
    ///
    /// A fraction measured over four hundred windows can only take values a
    /// quarter of a percent apart, and carries a standard error four times
    /// coarser than that. Publishing whatever has arrived so far would put a
    /// number on screen finer than the observation behind it, which is the same
    /// mistake `Uncertain::decimals` exists to prevent one layer up.
    #[test]
    fn a_partial_dwell_is_not_published() {
        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = crate::signal::net::SECTION.to_string();
        m.radio.frequency = 2_437_000_000;
        m.radio.config_sample_rate = 20_000_000.0;
        m.radio.bb_filter_hz = 18_000_000;
        let state = Arc::new(Mutex::new(m));

        let (tx, rx) = crossbeam_channel::unbounded();
        // One block: four hundred windows of six and a half microseconds, which
        // is under three milliseconds of the fifty a dwell is.
        tx.send(StreamBlock {
            seq: 1,
            gap_before: false,
            bytes: vec![0x05u8; 128 * 400 * 2],
        })
        .unwrap();
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), eight_bit()).run();

        let m = state.lock().unwrap();
        assert_eq!(m.net.health.blocks_in, 1, "the block arrived");
        assert!(
            m.net.band.cells.is_empty(),
            "and nothing was published from it"
        );
    }

    #[test]
    fn closing_the_section_ends_the_run_rather_than_losing_it() {
        // The section is closed, so every block in the channel is one already in
        // flight when it closed. The jump between them must not be reported as a
        // loss, because the device counts callbacks whether or not we take them.
        let h = feed(&[(1, false, 64), (9_000, false, 64)], false);
        assert_eq!(h.blocks_in, 2, "blocks in flight are still counted");
        assert_eq!(
            (h.gaps, h.blocks_lost),
            (0, 0),
            "and the pause is not a loss"
        );
    }

    /// B6's exit condition, run through the actual worker rather than
    /// `Receiver` directly: tuned to an advertising channel at a rate the
    /// decoder can reach, a synthetic packet in the block stream ends up in
    /// `net.ble_packets`, CRC-checked.
    #[test]
    fn a_synthetic_advertising_packet_reaches_ble_packets() {
        use crate::signal::ble::detect::{
            access_address_bits, preamble_bits, ADVERTISING_ACCESS_ADDRESS,
        };
        use crate::signal::ble::gfsk::modulate;
        use crate::signal::ble::pdu::encode;
        use crate::signal::dsp::testkit::{at_snr, Rng};
        use num_complex::Complex;

        const SPS: usize = 4;
        const SAMPLE_RATE: f64 = 4_000_000.0;
        const CHANNEL: u8 = 37; // 2402 MHz

        let addr = [0x11u8, 0x22, 0x33, 0x44, 0x55, 0x66];
        let mut bits = preamble_bits(ADVERTISING_ACCESS_ADDRESS).to_vec();
        bits.extend_from_slice(&access_address_bits(ADVERTISING_ACCESS_ADDRESS));
        bits.extend_from_slice(&encode(CHANNEL, 0x00, &addr));
        let mut rng = Rng::new(1);
        bits.extend((0..16).map(|_| rng.next_u64() & 1 == 1));
        let clean = modulate(&bits, SPS, 250_000.0, SAMPLE_RATE, 0.5);
        let noisy = at_snr(&clean, 20.0, &mut Rng::new(2));

        let geometry = eight_bit();
        let bytes: Vec<u8> = noisy
            .iter()
            .flat_map(|s: &Complex<f32>| {
                let re = (s.re * geometry.full_scale).clamp(-127.0, 127.0) as i8;
                let im = (s.im * geometry.full_scale).clamp(-127.0, 127.0) as i8;
                [re as u8, im as u8]
            })
            .collect();

        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = crate::signal::net::SECTION.to_string();
        m.radio.frequency = 2_402_000_000;
        m.radio.config_sample_rate = SAMPLE_RATE;
        m.radio.bb_filter_hz = 0;
        let state = Arc::new(Mutex::new(m));

        let (tx, rx) = crossbeam_channel::unbounded();
        tx.send(StreamBlock {
            seq: 1,
            gap_before: false,
            bytes,
        })
        .unwrap();
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), geometry).run();

        let m = state.lock().unwrap();
        assert!(m.net.ble_refused.is_none(), "{:?}", m.net.ble_refused);
        assert_eq!(m.net.ble_packets.len(), 1, "{:?}", m.net.ble_packets);
        let p = &m.net.ble_packets[0];
        assert_eq!(p.channel, CHANNEL);
        assert_eq!(p.adv_addr, Some(addr));
        assert!(p.crc_ok);
    }

    /// Tuned off any advertising frequency, the worker says so rather than
    /// silently decoding nothing.
    #[test]
    fn an_off_channel_tuning_is_refused_with_a_reason() {
        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = crate::signal::net::SECTION.to_string();
        m.radio.frequency = 2_437_000_000; // Wi-Fi channel 6, not a BLE one
        m.radio.config_sample_rate = 4_000_000.0;
        let state = Arc::new(Mutex::new(m));
        let (tx, rx) = crossbeam_channel::unbounded();
        tx.send(StreamBlock {
            seq: 1,
            gap_before: false,
            bytes: vec![0u8; 256],
        })
        .unwrap();
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), eight_bit()).run();
        let m = state.lock().unwrap();
        assert!(m.net.ble_refused.is_some());
        assert!(m.net.ble_packets.is_empty());
    }
}
