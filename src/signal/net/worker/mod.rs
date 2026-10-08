// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The 2.4 GHz worker: one thread, one block at a time.
//!
//! Same shape as [`crate::signal::DemodWorker`], for the same reason: a thread
//! that owns the state carried between blocks, so that everything below it can
//! be a pure function of its arguments and be tested with no radio anywhere.
//!
//! **One thread owns it; the decoders of one block run side by side.** The
//! BLE receiver and each classic channel's receiver own everything they
//! touch and read the same block, so [`classic::push_all`] runs them on scoped
//! threads for that block and hands their answers back in the order one
//! thread would have produced them: the same results, in less time on a
//! machine with more than one core.
//!
//! **It counts what arrived and measures what was in the band.** What the
//! receiver missed is a first-class displayed number rather than an
//! inference, and a feed whose losses are only visible once
//! there is something to lose is a feed nobody will trust when the losses
//! matter - so the counting was built and shown before any measurement sat on
//! top of it. The band measurement is [`super::scan`] over
//! [`super::occupancy`].
//!
//! **The decoders run here**, rather than each in its own task, because they
//! need the same per-block bytes the occupancy scan already has: a second
//! worker reading the same channel would need its own copy of the geometry
//! and the retune detection this one carries.
//!
//! - **BLE**, on the advertising channel in view: its packets, the census,
//!   the connections a CONNECT_IND sets up (followed event by event), and
//!   the auxiliary packets an `ADV_EXT_IND`'s AuxPtr promises.
//! - **LE Coded**, on its own view, in LE 1M's place, with its AuxPtrs
//!   followed the same way.
//! - **Classic Bluetooth**: one `signal::bt::receive::Receiver` per channel
//!   `signal::bt::channel::channels_in_span` and the configured
//!   [`SAFE_BT_CHANNELS`]-guarded cap together let it watch, closest to the
//!   tuned centre first, rebuilt whenever the tuning or the wanted channel
//!   list changes. One `signal::bt::header::PiconetClock` per LAP narrows
//!   each piconet's UAP as far as a header alone can (two candidates, not
//!   one: `PiconetClock`'s doc has why), and `signal::bt::payload::
//!   break_uap_tie` breaks the tie from a captured payload.
//!   [`classic::Classic`] remembers a LAP resolved this way for the rest of
//!   the session: a piconet's real UAP does not change, so a later header whose
//!   payload cannot be read (a POLL or an FHS, say) must not undo an answer
//!   already earned.
//!
//! **Split by what each part carries from one block to the next.** Each owns
//! its state and the lock blocks that publish it, and keeps every float and
//! every receiver's work outside them; [`NetWorker::run`] only sequences
//! them, in the order the block needs:
//!
//! - [`feed`]: where the stream stands (a gap, a pause, a new stream, a
//!   retune), the samples held for the measurement path, and the decode load.
//! - [`band`]: the survey's band measurement, a dwell at a time.
//! - [`le`]: the BLE and LE Coded receivers, the AuxPtr promises their
//!   advertisements made ([`promises`]) and the connections they followed
//!   ([`follow`]); [`ble`] is what a decoded LE packet becomes in the state.
//! - [`classic`]: the classic fleet and what each piconet has taught it.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use crossbeam_channel::Receiver as SampleReceiver;

use crate::hardware::{SampleGeometry, StreamBlock};
use crate::state::SdrMetrics;

// The views the classic receiver runs for (the Classic and the Piconet
// view): a plain string comparison, because that is what the menu is keyed
// by, and the registry's structural tests hold the strings and the preset
// files' names to agreeing. A view left out would show an empty list forever.
use super::lock::CLASSIC_VIEWS;

mod band;
mod ble;
mod classic;
mod feed;
mod follow;
mod le;
mod promises;

use band::Band;
use classic::{push_all, Classic};
use feed::{decoded_block, Feed};
use le::Le;

/// Above this many simultaneous classic BT channels, `NetWorker::new` logs a
/// warning naming the cost rather than staying quiet about it -
/// `signal::bt::receive`'s own doc has the measured tap counts this is
/// guarding against. Matches `config::default_bt_channels`, so a default
/// config never warns; only a config that deliberately asks for more does.
pub const SAFE_BT_CHANNELS: usize = 8;

/// Where one block sits in the stream and what it was captured at: what
/// every receiver is built for and every window is cut against.
#[derive(Clone, Copy, Debug)]
struct Tuning {
    first_pair: u64,
    centre_hz: f64,
    rate_hz: f64,
    /// The usable span: the baseband filter's where the radio has one.
    span_hz: f64,
}

/// What the user is looking at, read from the state once a block: which
/// receivers the block is for.
#[derive(Clone, Copy, Debug)]
struct View {
    /// The NET section is open: nothing decodes while it is not.
    open: bool,
    /// A classic view (`lock::CLASSIC_VIEWS`).
    classic: bool,
    /// The survey, the one view that measures the band.
    survey: bool,
    /// An LE Coded view (`lock::CODED_VIEWS`).
    coded: bool,
    /// The LE PHY the user chose (`NetState::ble_phy`).
    phy: crate::signal::ble::Phy,
    /// LOCK rather than SURVEY.
    locked: bool,
    /// A connection is being followed.
    following: bool,
}

impl View {
    /// The view, and the usable span of the tuning at `rate_hz`, read inside
    /// the caller's lock.
    fn read(m: &SdrMetrics, rate_hz: f64) -> (View, f64) {
        // The usable span is the baseband filter's where the radio has one,
        // because the bins the front end rolled off carry no measurement and
        // averaging them in would drag every cell at the edges of the view
        // down towards a floor that is not the band's. Where there is no
        // filter, the rate is all we know.
        let span = if m.radio.bb_filter_hz > 0 {
            m.radio.bb_filter_hz as f64
        } else {
            rate_hz
        };
        let view = View {
            open: m.ui.is_net_section(),
            classic: CLASSIC_VIEWS.contains(&m.ui.active_preset.as_str()),
            survey: m.ui.active_preset == super::lock::SURVEY_VIEW,
            coded: super::lock::CODED_VIEWS.contains(&m.ui.active_preset.as_str()),
            phy: m.net.ble_phy,
            locked: m.net.mode == crate::state::NetMode::Lock,
            following: m
                .net
                .ble_connections
                .iter()
                .any(|f| *f.connection.state() == crate::signal::ble::follow::State::Following),
        };
        (view, span.min(rate_hz))
    }
}

pub struct NetWorker {
    pub sample_rx: SampleReceiver<StreamBlock>,
    pub state: Arc<Mutex<SdrMetrics>>,
    pub geometry: SampleGeometry,
    /// How many classic BT channels to give a live receiver at once - see
    /// [`SAFE_BT_CHANNELS`] and `signal::bt::receive`'s own doc for why this
    /// is capped rather than left to follow the full view.
    pub bt_channels: usize,
    /// How often one piconet's slot grid may be refitted
    /// (`signal::bt::slots`): the rate search is real work, and a jitter
    /// figure does not need refreshing faster. A test sets it to zero.
    pub slot_fit_every: std::time::Duration,
    /// How many classic channels the survey starts with, before its first
    /// load reading: none, because room for a receiver is measured, not
    /// assumed. A test sets it, since the reading follows the clock.
    pub survey_bt_start: usize,
}

impl NetWorker {
    pub fn new(
        sample_rx: SampleReceiver<StreamBlock>,
        state: Arc<Mutex<SdrMetrics>>,
        geometry: SampleGeometry,
        bt_channels: usize,
    ) -> Self {
        if bt_channels > SAFE_BT_CHANNELS {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            m.push_log(format!(
                "NET: [net].bt_channels = {bt_channels} asks for real capacity - \
                 classic Bluetooth's own 1 MHz channel spacing makes each watched \
                 channel expensive (see signal::bt::receive's own doc); the \
                 default of {SAFE_BT_CHANNELS} is the reasoned-safe figure"
            ));
        }
        Self {
            slot_fit_every: std::time::Duration::from_secs(1),
            survey_bt_start: 0,
            sample_rx,
            state,
            geometry,
            bt_channels,
        }
    }

    pub fn run(self) {
        let mut feed = Feed::new(self.geometry);
        let mut band = Band::default();
        let mut le = Le::default();
        let mut classic = Classic::new(self.survey_bt_start);

        while let Ok(block) = self.sample_rx.recv() {
            // The clock is read outside the lock, because the lock block below
            // does integer work only and a float or a syscall inside one is a
            // dropped frame on the UI thread.
            let now = Instant::now();
            let arrival = feed.arrive(&block);
            let StreamBlock {
                bytes,
                first_pair,
                centre_hz,
                rate_hz,
                ..
            } = block;
            if arrival.new_stream {
                classic.new_stream();
            }
            classic.rate(rate_hz);
            if !arrival.continuous {
                le.interrupted(&self.state);
                classic.fleet.clear();
            }
            if arrival.retuned {
                le.retuned(&self.state);
            }

            let (view, span_hz) = {
                let mut m = self.state.lock().unwrap_or_else(|e| e.into_inner());
                arrival.count(&mut m.net.health, now);
                View::read(&m, rate_hz)
            };
            // The tuning and rate these samples were captured at, from the
            // block rather than the state: see `StreamBlock::centre_hz`.
            let tuning = Tuning {
                first_pair,
                centre_hz: centre_hz as f64,
                rate_hz,
                span_hz,
            };
            let feeds = le.choose(tuning, view, &self.state);

            // Decoded once, by the first receiver that needs it, before any
            // runs, and shared by them all.
            let classic_here = view.open && (view.classic || view.survey);
            let follow_here = view.open && view.following;
            let survey_here = view.open && view.survey;
            let mut iq: Option<Vec<num_complex::Complex<f32>>> = None;
            if feeds.any() || classic_here || follow_here || survey_here {
                decoded_block(&mut iq, &bytes, self.geometry);
            }
            if let Some(block) = iq.as_mut() {
                feed.take_off_dc(block, first_pair, rate_hz);
            }
            if survey_here {
                band.measure(iq.as_deref(), tuning, now, &self.state);
            } else {
                band.unobserved(view.open, now, &self.state);
            }
            let block: &[num_complex::Complex<f32>] = iq.as_deref().unwrap_or(&[]);
            // What is held of the stream, this block included: what every
            // window and measurement below is cut from, and where it ends
            // once this block is decoded.
            let window = feed.window(iq.as_deref().map(|v| (first_pair, v)));
            let held_end = iq.as_deref().map(|v| (first_pair + v.len() as u64) as f64);
            let mut ble_packets: Option<Vec<crate::signal::ble::pdu::Packet>> = None;

            // The classic receiver, one per channel the current tuning and
            // the cap together let it watch: `self.bt_channels` on the
            // Classic view, and on the survey only as many as its measured
            // load leaves room for (`SURVEY_LOAD_HIGH`), so the coexistence
            // history can mark classic hits without starving the band
            // measurement the survey is for. Classic BT has no fixed channel
            // set to gate on, so the preset's name is the gate.
            if classic_here {
                classic.watch(tuning, view.classic, self.bt_channels, &self.state);
                let (ble_out, answers) =
                    push_all(le.beside(feeds), &mut classic.fleet, block, first_pair);
                ble_packets = ble_out;
                classic.read(
                    answers,
                    &window,
                    tuning,
                    now,
                    self.slot_fit_every,
                    &self.state,
                );
            } else {
                classic.stand_down(&self.state);
            }

            // The BLE receiver alone, when no classic fleet ran beside it,
            // then what it found, as it always was.
            let ble_packets = ble_packets.or_else(|| le.push_ble(feeds, block, first_pair));
            le.ble_packets(feeds, ble_packets, &window, tuning, now, &self.state);
            if let Some(end) = held_end.filter(|_| follow_here) {
                le.follow_events(&window, end, tuning, &self.state);
            }
            if let Some(this) = iq.as_deref() {
                le.coded_packets(feeds, this, &window, tuning, now, &self.state);
            }
            if let Some(end) = held_end {
                le.keep_promises(&window, end, tuning, now, &self.state);
            }

            let longer = view.following || view.coded || le.waiting();
            feed.hold(first_pair, iq, view.open, longer, rate_hz);

            // Closing the section stops `process_block` forwarding, but blocks
            // already in the channel still arrive - and the run they belong to
            // is over whether or not they are the last of it.
            if !view.open {
                feed.close();
                band.close();
                le.close(&self.state);
                classic.close();
                let mut m = self.state.lock().unwrap_or_else(|e| e.into_inner());
                Classic::unpublish(&mut m);
                m.net.ble_channel = None;
                // Nothing is being decoded, so there is no load to report -
                // and a figure from before the section closed must not be
                // shown on reopening as if it were current.
                m.net.health.decode_load = None;
            } else if let Some(reading) = feed.cost(now.elapsed(), arrival.pairs, rate_hz) {
                // The clock read and the division both happened above, outside
                // the lock; only the finished figure goes in.
                let mut m = self.state.lock().unwrap_or_else(|e| e.into_inner());
                m.net.health.decode_load = Some(reading);
                // The survey's classic channels follow the load it measured:
                // one fewer over the high mark, one more under the low one.
                if view.survey {
                    classic.loaded(reading, self.bt_channels);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::SampleFormat;
    use crate::state::BlePacket;

    /// The survey starts with no classic channel, and says it is the load:
    /// its first reading has not come yet, and a receiver it has not measured
    /// room for is not run on the band measurement's time.
    #[test]
    fn the_survey_runs_no_classic_channel_until_its_load_leaves_room() {
        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = crate::signal::net::SECTION.to_string();
        m.ui.active_preset = super::super::lock::SURVEY_VIEW.to_string();
        m.radio.frequency = 2_441_000_000;
        m.radio.config_sample_rate = 8e6;
        m.radio.bb_filter_hz = 0;
        let state = Arc::new(Mutex::new(m));
        let (tx, rx) = crossbeam_channel::unbounded();
        tx.send(stamped(&state, 1, false, vec![0u8; 2 * 4096]))
            .unwrap();
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), eight_bit(), SAFE_BT_CHANNELS).run();
        let m = state.lock().unwrap();
        assert!(m.net.bt_channels_watched.is_empty());
        assert!(m.net.bt_load_limited);
        assert_eq!(
            m.net.bt_refused.as_deref(),
            Some("not running: the survey's load leaves no room")
        );
    }

    /// A block as `hardware::process::process_block` would stamp it: its
    /// position is the `seq`-th block of this size, so a jump in `seq` is a
    /// jump in the stream, and its tuning is whatever the test put in the
    /// state - read now, at capture, the way the real stamp is.
    fn stamped(
        state: &Arc<Mutex<SdrMetrics>>,
        seq: u64,
        gap_before: bool,
        bytes: Vec<u8>,
    ) -> StreamBlock {
        let m = state.lock().unwrap();
        let pairs = bytes.len() as u64 / 2;
        StreamBlock {
            seq,
            gap_before,
            first_pair: seq.saturating_sub(1) * pairs,
            centre_hz: m.radio.frequency,
            rate_hz: m.radio.config_sample_rate,
            bytes,
        }
    }

    /// **What the receive chain costs, measured.** Not a check: run it by
    /// hand, in release, on the machine the question is about
    /// (`cargo test --release measure_the_receive_chain -- --ignored
    /// --nocapture`), and it prints each view's wall time over the stream
    /// time it covered, the figure the header calls the decode load. Fed
    /// noise, so it measures the chain's floor, not a busy room's.
    #[test]
    #[ignore]
    fn measure_the_receive_chain() {
        use crate::signal::dsp::testkit::Rng;
        const SECONDS: f64 = 4.0;
        const BLOCK_PAIRS: usize = 131_072;
        let cases: [(&str, f64, u64); 7] = [
            ("net_survey", 8e6, 2_426_500_000),
            ("net_ble", 8e6, 2_426_000_000),
            ("net_ble", 20e6, 2_426_000_000),
            ("net_bt", 4e6, 2_440_000_000),
            ("net_bt", 8e6, 2_440_000_000),
            ("net_bt", 20e6, 2_440_000_000),
            ("net_survey", 20e6, 2_426_500_000),
        ];
        // Noise at a level an ADC sees in a quiet band: well clear of clipping.
        let block: Vec<u8> = Rng::new(7)
            .noise(BLOCK_PAIRS, 0.02)
            .iter()
            .flat_map(|z| {
                let q = |v: f32| (v * 128.0).clamp(-127.0, 127.0) as i8 as u8;
                [q(z.re), q(z.im)]
            })
            .collect();
        // `SDRTOP_MEASURE=net_bt@8` runs that one case alone, for a profiler.
        let only = std::env::var("SDRTOP_MEASURE").ok();
        for (preset, rate, tuned) in cases {
            let name = format!("{preset}@{}", rate / 1e6);
            if only.as_ref().is_some_and(|o| *o != name) {
                continue;
            }
            let mut m = SdrMetrics::fixture().streaming();
            m.ui.section = crate::signal::net::SECTION.to_string();
            m.ui.active_preset = preset.to_string();
            m.radio.frequency = tuned;
            m.radio.config_sample_rate = rate;
            m.radio.bb_filter_hz = 0;
            let state = Arc::new(Mutex::new(m));
            let blocks = (SECONDS * rate / BLOCK_PAIRS as f64).ceil() as u64;
            let (tx, rx) = crossbeam_channel::unbounded();
            for seq in 1..=blocks {
                tx.send(stamped(&state, seq, false, block.clone())).unwrap();
            }
            drop(tx);
            let start = std::time::Instant::now();
            NetWorker::new(rx, Arc::clone(&state), eight_bit(), SAFE_BT_CHANNELS).run();
            let wall = start.elapsed().as_secs_f64();
            let stream = blocks as f64 * BLOCK_PAIRS as f64 / rate;
            let m = state.lock().unwrap();
            eprintln!(
                "{preset:<11} {:>5.1} Msps  BLE ch {:?}  BT {} ch  {:.2}x real time",
                rate / 1e6,
                m.net.ble_channel,
                m.net.bt_channels_watched.len(),
                wall / stream
            );
        }
    }

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
            tx.send(stamped(&state, seq, gap_before, vec![0u8; pairs * 2]))
                .unwrap();
        }
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), eight_bit(), SAFE_BT_CHANNELS).run();
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
        assert!(
            h.last_loss.is_none(),
            "nothing was lost, so there is no when"
        );
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
        assert!(h.last_loss.is_none(), "and no loss is dated either");
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
        assert!(h.last_loss.is_some(), "a loss says when it happened");
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
        // The band is measured on the survey, the view that shows it.
        m.ui.active_preset = super::super::lock::SURVEY_VIEW.to_string();
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
            tx.send(stamped(&state, seq, false, bytes)).unwrap();
        }
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), eight_bit(), SAFE_BT_CHANNELS).run();

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
        tx.send(stamped(&state, 1, false, vec![0x05u8; 128 * 400 * 2]))
            .unwrap();
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), eight_bit(), SAFE_BT_CHANNELS).run();

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

    /// Through the actual worker rather than
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
        use crate::signal::ble::Phy;
        use crate::signal::dsp::testkit::{at_snr, Rng};
        use num_complex::Complex;

        const SPS: usize = 4;
        const SAMPLE_RATE: f64 = 4_000_000.0;
        const CHANNEL: u8 = 37; // 2402 MHz

        let addr = [0x11u8, 0x22, 0x33, 0x44, 0x55, 0x66];
        let mut bits = preamble_bits(ADVERTISING_ACCESS_ADDRESS, Phy::OneM);
        bits.extend_from_slice(&access_address_bits(ADVERTISING_ACCESS_ADDRESS));
        bits.extend_from_slice(&encode(
            CHANNEL,
            0x00,
            &crate::signal::ble::pdu::air_octets(addr),
        ));
        let mut rng = Rng::new(1);
        bits.extend((0..16).map(|_| rng.next_u64() & 1 == 1));
        // Heard out of silence, as every packet is: its SNR is read against
        // the noise just before it.
        let mut clean = vec![Complex::new(0.0f32, 0.0); 200 * SPS];
        clean.extend(modulate(&bits, SPS, 250_000.0, SAMPLE_RATE, 0.5));
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
        tx.send(stamped(&state, 1, false, bytes)).unwrap();
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), geometry, SAFE_BT_CHANNELS).run();

        let m = state.lock().unwrap();
        assert!(m.net.ble_refused.is_none(), "{:?}", m.net.ble_refused);
        assert_eq!(
            m.net.ble_channel,
            Some(CHANNEL),
            "the receiver says where it is running"
        );
        assert_eq!(m.net.ble_packets.len(), 1, "{:?}", m.net.ble_packets);
        let p = &m.net.ble_packets[0];
        assert_eq!(p.channel, CHANNEL);
        assert_eq!(p.adv_addr, Some(addr));
        assert!(p.crc_ok);
        // Numbered on arrival, so `masked` can show it.
        assert_eq!(m.net.address_book.get(addr), Some(1));

        // What was decoded reaches the state whole: the payload the AD
        // structures will be read from, the address in air order inside it.
        assert_eq!(
            p.payload,
            crate::signal::ble::pdu::air_octets(addr).to_vec(),
            "{p:?}"
        );
        assert!(!p.ch_sel && !p.rx_add_random);
        // The section's frame error curve took it, in its SNR bin.
        assert_eq!(m.net.fer.total(), 1, "{:?}", m.net.fer);
        // A confirmed device reaches the shared census too, keyed by the same
        // address `net_ble_packets` shows.
        assert_eq!(m.net.census.devices.len(), 1, "{:?}", m.net.census.devices);
        assert_eq!(m.net.census.devices[0].address, addr);
        assert_eq!(m.net.census.devices[0].packets, 1);
    }

    /// A synthetic ADV_IND on LE 2M, as raw 8 Msps bytes on data channel
    /// `ch`: the advertising access address and the channel's own
    /// whitening, as a secondary advertising channel carries them.
    fn ble_2m_bytes(ch: u8) -> (Vec<u8>, [u8; 6]) {
        use crate::signal::ble::detect::{
            access_address_bits, preamble_bits, ADVERTISING_ACCESS_ADDRESS,
        };
        use crate::signal::ble::gfsk::modulate;
        use crate::signal::ble::pdu::encode;
        use crate::signal::ble::Phy;
        use crate::signal::dsp::testkit::{at_snr, Rng};
        use num_complex::Complex;

        let addr = [0x11u8, 0x22, 0x33, 0x44, 0x55, 0x66];
        let mut bits: Vec<bool> = (0..16).map(|i| i % 3 == 0).collect();
        bits.extend(preamble_bits(ADVERTISING_ACCESS_ADDRESS, Phy::TwoM));
        bits.extend_from_slice(&access_address_bits(ADVERTISING_ACCESS_ADDRESS));
        bits.extend_from_slice(&encode(
            ch,
            0x00,
            &crate::signal::ble::pdu::air_octets(addr),
        ));
        let mut rng = Rng::new(1);
        bits.extend((0..32).map(|_| rng.next_u64() & 1 == 1));
        let clean = modulate(&bits, 4, Phy::TwoM.deviation_hz(), 8_000_000.0, 0.5);
        let noisy = at_snr(&clean, 20.0, &mut Rng::new(2));
        let geometry = eight_bit();
        let bytes = noisy
            .iter()
            .flat_map(|s: &Complex<f32>| {
                let re = (s.re * geometry.full_scale).clamp(-127.0, 127.0) as i8;
                let im = (s.im * geometry.full_scale).clamp(-127.0, 127.0) as i8;
                [re as u8, im as u8]
            })
            .collect();
        (bytes, addr)
    }

    /// A connection set up and run on the air, as a 20 Msps stream tuned to
    /// 2426 MHz: a CONNECT_IND on advertising channel 38, then `events`
    /// events on the channels CSA #1 gives, each in view one an empty PDU
    /// from the Central at its anchor and one from the Peripheral T_IFS
    /// after. Returns the stream's eight-bit bytes, block by block, and the
    /// access address.
    fn a_connection_on_the_air(events: u16) -> (Vec<Vec<u8>>, u32) {
        a_connection_with_chsel(events, false, None)
    }

    /// [`a_connection_on_the_air`] with the CONNECT_IND's ChSel bit, and,
    /// when given, an ADV_DIRECT_IND from its AdvA with that ChSel 150 us
    /// before it, the PDU it answers.
    fn a_connection_with_chsel(
        events: u16,
        connect_ch_sel: bool,
        advertised: Option<bool>,
    ) -> (Vec<Vec<u8>>, u32) {
        use crate::signal::ble::connect::Csa1;
        use crate::signal::ble::data::{encode as encode_data, DataPdu};
        use crate::signal::ble::detect::{
            access_address_bits, preamble_bits, ADVERTISING_ACCESS_ADDRESS,
        };
        use crate::signal::ble::gfsk::modulate;
        use crate::signal::ble::pdu::encode;
        use crate::signal::ble::Phy;
        use crate::signal::dsp::testkit::Rng;
        use num_complex::Complex;

        const RATE: f64 = 20e6;
        const SPS: usize = 20;
        const TUNED: f64 = 2_426e6;
        const BLOCK: usize = 131_072;
        const AA: u32 = 0x5065_4b6a;
        const CRC: u32 = 0x3a_5b7c;
        let us = |x: f64| x * 1e-6 * RATE;
        let total = (us(2_000.0 + 1_250.0 + events as f64 * 7_500.0) as usize / BLOCK + 2) * BLOCK;
        let mut iq = vec![Complex::new(0.0f32, 0.0); total];
        let mut rng = Rng::new(17);
        // A burst whose preamble starts at `at`, on `ch`.
        let mut put = |at: usize, ch: u8, aa: u32, pdu_bits: Vec<bool>| {
            let mut bits: Vec<bool> = (0..16).map(|_| rng.next_u64() & 1 == 1).collect();
            bits.extend(preamble_bits(aa, Phy::OneM));
            bits.extend_from_slice(&access_address_bits(aa));
            bits.extend(pdu_bits);
            let wave = modulate(&bits, SPS, Phy::OneM.deviation_hz(), RATE, 0.5);
            let offset = crate::signal::ble::channel::centre_hz(ch).unwrap() as f64 - TUNED;
            let from = at - 16 * SPS;
            for (k, s) in wave.iter().enumerate() {
                let n = from + k;
                let ph = std::f64::consts::TAU * offset * n as f64 / RATE;
                iq[n] += s * Complex::new(ph.cos() as f32, ph.sin() as f32) * 0.6;
            }
        };
        // CONNECT_IND: InitA, AdvA, then LLData: AA, CRCInit, WinSize 1,
        // WinOffset 0, Interval 6 (7.5 ms), Latency 0, Timeout 100, every
        // channel, Hop 7, SCA 0.
        let mut payload = vec![
            0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0xa1, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6,
        ];
        payload.extend(AA.to_le_bytes());
        payload.extend(&CRC.to_le_bytes()[..3]);
        payload.extend([1, 0, 0, 6, 0, 0, 0, 100, 0, 0xff, 0xff, 0xff, 0xff, 0x1f, 7]);
        let connect_at = us(2_000.0) as usize;
        if let Some(adv_ch_sel) = advertised {
            // ADV_DIRECT_IND: AdvA then TargetA (the initiator), air order.
            let direct: Vec<u8> = payload[6..12]
                .iter()
                .chain(&payload[..6])
                .copied()
                .collect();
            let header = 0x01 | (adv_ch_sel as u8) << 5;
            // 8 + 32 + 16 + 12 * 8 + 24 bits, then T_IFS.
            let before = (8 + 32 + 16 + 12 * 8 + 24) * SPS + us(150.0) as usize;
            put(
                connect_at - before,
                38,
                ADVERTISING_ACCESS_ADDRESS,
                encode(38, header, &direct),
            );
        }
        let header = 0x05 | (connect_ch_sel as u8) << 5;
        put(
            connect_at,
            38,
            ADVERTISING_ACCESS_ADDRESS,
            encode(38, header, &payload),
        );
        // Preamble 8, address 32, header 16, 34 octets, CRC 24, 20 pairs a bit.
        let connect_end = connect_at + (8 + 32 + 16 + 34 * 8 + 24) * SPS;
        let empty = |sn: bool| DataPdu {
            llid: 1,
            nesn: !sn,
            sn,
            md: false,
            cte_info: None,
            payload: Vec::new(),
            crc_ok: true,
        };
        let mut csa = Csa1::new(7, (1u64 << 37) - 1).unwrap();
        for k in 0..events {
            let ch = csa.next();
            if !(7..=14).contains(&ch) {
                continue;
            }
            let anchor = connect_end + us(1_250.0 + k as f64 * 7_500.0) as usize;
            put(anchor, ch, AA, encode_data(&empty(false), CRC, ch));
            let central_end = anchor + (8 + 32 + 16 + 24) * SPS;
            put(
                central_end + us(150.0) as usize,
                ch,
                AA,
                encode_data(&empty(true), CRC, ch),
            );
        }
        let blocks = iq
            .chunks(BLOCK)
            .map(|c| {
                c.iter()
                    .flat_map(|s| {
                        let re = (s.re * 128.0).clamp(-127.0, 127.0) as i8;
                        let im = (s.im * 128.0).clamp(-127.0, 127.0) as i8;
                        [re as u8, im as u8]
                    })
                    .collect()
            })
            .collect();
        (blocks, AA)
    }

    /// Runs the worker on the LE Advertising view over `blocks`, leaving out
    /// the ones `skip` names (a lost block), and returns the state.
    fn follow_over(blocks: &[Vec<u8>], skip: &[usize]) -> SdrMetrics {
        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = "le".to_string();
        m.ui.active_preset = "net_ble".to_string();
        m.radio.frequency = 2_426_000_000;
        m.radio.config_sample_rate = 20e6;
        m.radio.bb_filter_hz = 0;
        let state = Arc::new(Mutex::new(m));
        let (tx, rx) = crossbeam_channel::unbounded();
        for (i, bytes) in blocks.iter().enumerate() {
            if !skip.contains(&i) {
                tx.send(stamped(&state, i as u64 + 1, false, bytes.clone()))
                    .unwrap();
            }
        }
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), eight_bit(), SAFE_BT_CHANNELS).run();
        let m = state.lock().unwrap().clone();
        m
    }

    /// **What following a connection costs, measured**, as
    /// `measure_the_receive_chain` measures the views: run by hand, in
    /// release, on the machine the question is about (`cargo test --release
    /// measure_following_a_connection -- --ignored --nocapture`). One
    /// connection at the shortest interval the Core allows (7.5 ms), a
    /// second of it, against `net_ble@20` in that probe for the same view
    /// with nothing to follow.
    #[test]
    #[ignore]
    fn measure_following_a_connection() {
        let (blocks, _) = a_connection_on_the_air(133);
        let start = std::time::Instant::now();
        let m = follow_over(&blocks, &[]);
        let wall = start.elapsed().as_secs_f64();
        let stream = blocks.iter().map(|b| b.len() / 2).sum::<usize>() as f64 / 20e6;
        let c = &m.net.ble_connections[0].connection;
        let followed = c
            .events()
            .iter()
            .filter(|e| e.account == crate::signal::ble::follow::Account::Followed)
            .count();
        eprintln!(
            "following at 20 Msps: {} events, {followed} followed, {:.2}x real time",
            c.events().len(),
            wall / stream
        );
    }

    /// A recording at `path` (ci8 at 20 Msps tuned to 2426 MHz) through the
    /// worker on `section`'s `preset`, locked, every block taken: the feed
    /// waits for the worker, so nothing is lost to load. The state it left,
    /// and how many blocks went in.
    fn replay(path: &str, section: &str, preset: &str) -> (SdrMetrics, u64) {
        use std::io::Read;
        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = section.to_string();
        m.ui.active_preset = preset.to_string();
        m.net.mode = crate::state::NetMode::Lock;
        // Tuned and sampled as the recording says beside it, where it says.
        let meta = std::fs::read_to_string(path.replace(".sigmf-data", ".sigmf-meta"))
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
        let capture = meta
            .as_ref()
            .and_then(|m| m["captures"][0]["core:frequency"].as_u64());
        let rate = meta
            .as_ref()
            .and_then(|m| m["global"]["core:sample_rate"].as_f64());
        m.radio.frequency = capture.unwrap_or(2_426_000_000);
        m.radio.config_sample_rate = rate.unwrap_or(20e6);
        m.radio.bb_filter_hz = 0;
        let state = Arc::new(Mutex::new(m));
        let (tx, rx) = crossbeam_channel::bounded(4);
        let st = Arc::clone(&state);
        let worker = std::thread::spawn(move || {
            NetWorker::new(rx, st, eight_bit(), SAFE_BT_CHANNELS).run();
        });
        let mut f = std::fs::File::open(path).unwrap();
        let mut seq = 1u64;
        loop {
            let mut buf = vec![0u8; 131_072 * 2];
            let mut got = 0;
            while got < buf.len() {
                let n = f.read(&mut buf[got..]).unwrap();
                if n == 0 {
                    break;
                }
                got += n;
            }
            if got < buf.len() {
                break;
            }
            tx.send(stamped(&state, seq, false, buf)).unwrap();
            seq += 1;
        }
        drop(tx);
        worker.join().unwrap();
        let m = state.lock().unwrap().clone();
        (m, seq - 1)
    }

    /// Counts the LE 1M `ADV_EXT_IND`s on channel 38 in a recording
    /// (`SDRTOP_REPLAY=path.sigmf-data`, ci8 at 20 Msps tuned to 2426 MHz)
    /// and where their AuxPtrs point: how much following them would have to
    /// listen to. By hand, in release.
    #[test]
    #[ignore]
    fn count_1m_extended_advertising() {
        use crate::signal::ble::aux_ptr::AuxPhy;
        use std::io::Read;
        let Ok(path) = std::env::var("SDRTOP_REPLAY") else {
            return;
        };
        let mut rx = crate::signal::ble::receive::Receiver::new(
            20e6,
            38,
            crate::signal::ble::Phy::OneM,
            2_426e6,
        )
        .unwrap();
        let mut f = std::fs::File::open(&path).unwrap();
        let (mut at, mut all, mut ext, mut ok) = (0u64, 0u64, 0u64, 0u64);
        let mut by_phy = std::collections::BTreeMap::new();
        let (mut in_view, mut out_of_view, mut none) = (0u64, 0u64, 0u64);
        let mut senders = std::collections::BTreeSet::new();
        let mut buf = vec![0u8; 131_072 * 2];
        let mut iq = Vec::new();
        let started = std::time::Instant::now();
        while f.read_exact(&mut buf).is_ok() {
            crate::signal::demod::decode(&buf, eight_bit(), usize::MAX, &mut iq);
            for p in rx.push_iq_at(&iq, at) {
                all += 1;
                if p.pdu_type != crate::signal::ble::pdu::PduType::Other(0x07) {
                    continue;
                }
                ext += 1;
                if !p.crc_ok {
                    continue;
                }
                ok += 1;
                let Ok(h) = crate::signal::ble::ext::parse(&p.payload) else {
                    continue;
                };
                if let Some(a) = h.adi {
                    senders.insert((a.sid, a.did));
                }
                match h.aux_ptr.filter(|a| !a.promises_nothing()) {
                    None => none += 1,
                    Some(a) => {
                        let phy = match a.phy {
                            Some(AuxPhy::OneM) => "1M",
                            Some(AuxPhy::TwoM) => "2M",
                            Some(AuxPhy::Coded) => "Coded",
                            None => "reserved",
                        };
                        *by_phy.entry(phy).or_insert(0u64) += 1;
                        if (7..=14).contains(&a.channel) {
                            in_view += 1;
                        } else {
                            out_of_view += 1;
                        }
                    }
                }
            }
            at += 131_072;
        }
        let secs = at as f64 / 20e6;
        eprintln!(
            "{secs:.1} s · {all} packets on 38 · {ext} ADV_EXT_IND ({ok} CRC good, {:.2}/s) · \
             AuxPtr by PHY {by_phy:?} · to 7..=14 {in_view}, elsewhere {out_of_view}, none {none} · \
             {} sets · {:.1} s to read",
            ok as f64 / secs,
            senders.len(),
            started.elapsed().as_secs_f64()
        );
    }

    /// Replays a recording (`SDRTOP_REPLAY=path.sigmf-data`, ci8 at 20 Msps
    /// tuned to 2426 MHz) of a phone advertising on LE Coded through the
    /// worker on the LE Coded view, locked on channel 38, and prints what it
    /// read. By hand, in release: the phone's `ADV_EXT_IND`s are decoded,
    /// at least one AuxPtr is followed to its `AUX_ADV_IND`, the phone's
    /// name is read from it, and every trigger ends once.
    #[test]
    #[ignore]
    fn replay_a_coded_recording() {
        let Ok(path) = std::env::var("SDRTOP_REPLAY") else {
            return;
        };
        let (m, blocks) = replay(&path, "coded", "net_coded");
        let f = m.net.health.coded;
        let a = m.net.health.aux;
        let kept: Vec<_> = m.net.coded_packets.iter().collect();
        let role = |p: &&BlePacket| p.ext.as_ref().map(|e| e.role.label());
        let primaries = kept
            .iter()
            .filter(|p| role(p) == Some("ADV_EXT_IND"))
            .count();
        let auxes: Vec<_> = kept
            .iter()
            .filter(|p| role(p) == Some("AUX_ADV_IND"))
            .collect();
        let names: std::collections::BTreeSet<String> = auxes
            .iter()
            .filter_map(|p| {
                let e = p.ext.as_ref()?;
                let structures = crate::signal::ble::ad::parse(&e.header.adv_data);
                crate::signal::ble::ad::name(&structures).map(|(n, _)| n.to_string())
            })
            .collect();
        let snr = |ps: &[&&BlePacket]| {
            let mut v: Vec<f64> = ps.iter().filter_map(|p| p.snr_db).collect();
            v.sort_by(f64::total_cmp);
            let (lo, hi) = (v.first().copied(), v.last().copied());
            let median = (!v.is_empty()).then(|| v[v.len() / 2]);
            format!(
                "{} to {} dB, median {}, {} of {} read",
                lo.map_or("-".into(), |x| format!("{x:.1}")),
                hi.map_or("-".into(), |x| format!("{x:.1}")),
                median.map_or("-".into(), |x| format!("{x:.1}")),
                v.len(),
                ps.len()
            )
        };
        let prim: Vec<_> = kept
            .iter()
            .filter(|p| role(p) == Some("ADV_EXT_IND"))
            .collect();
        let repairs: Vec<u32> = kept
            .iter()
            .filter_map(|p| p.coded.as_ref().map(|c| c.fec_repairs))
            .collect();
        eprintln!(
            "blocks {blocks} · heard {} · kept {} ({primaries} ADV_EXT_IND, {} AUX_ADV_IND)",
            m.net.coded_heard,
            kept.len(),
            auxes.len()
        );
        eprintln!(
            "funnel: {} triggers · {} CRC ok · {} CRC failed · {} gave up",
            f.triggered, f.decoded, f.crc_failed, f.gave_up
        );
        eprintln!(
            "aux: {} heard · {} missed · {} not in view · {} feed lost · {} none promised · {} refused",
            a.heard, a.missed, a.not_in_view, a.feed_lost, a.none_promised, a.refused
        );
        eprintln!(
            "SNR primary {} · aux {} · FEC repairs max {} mean {:.2}",
            snr(&prim),
            snr(&auxes),
            repairs.iter().max().unwrap_or(&0),
            repairs.iter().sum::<u32>() as f64 / repairs.len().max(1) as f64
        );
        for p in auxes.iter().take(3) {
            eprintln!(
                "aux ch {} · {} · SNR {:?} · CFO {:?}",
                p.channel,
                p.phy.label(),
                p.snr_db,
                p.freq_offset_hz
            );
        }
        eprintln!("names {names:?}");
        assert!(primaries > 0, "no ADV_EXT_IND");
        assert!(a.heard > 0 && !auxes.is_empty(), "no AuxPtr followed");
        assert!(!names.is_empty(), "no name read");
        assert_eq!(f.triggered, f.decoded + f.crc_failed + f.gave_up, "{f:?}");
    }

    /// Replays a recording (`SDRTOP_REPLAY=path.sigmf-data`, ci8 at 20 Msps
    /// tuned to 2426 MHz) through the worker on LE 2, locked, and prints the
    /// connections followed. By hand, in release.
    #[test]
    #[ignore]
    fn replay_a_recording() {
        use std::io::Read;
        let Ok(path) = std::env::var("SDRTOP_REPLAY") else {
            return;
        };
        let (m, blocks) = replay(&path, "le", "net_ble");
        let connects = m
            .net
            .ble_packets
            .iter()
            .filter(|p| p.pdu_type == crate::signal::ble::pdu::PduType::ConnectInd)
            .count();
        eprintln!(
            "blocks {blocks} · packets kept {} · CONNECT_IND kept {connects}",
            m.net.ble_packets.len()
        );
        eprintln!("funnel: {:?}", m.net.health.ble);
        // A second pass, with no follower: every packet with the link's
        // access address on the eight data channels in view, and the
        // CONNECT_IND on 38, from `SDRTOP_REPLAY_FROM` pairs on.
        if let Some(f) = m.net.ble_connections.first() {
            use crate::signal::ble::receive::{Link, Receiver};
            let (aa, crc) = (f.connection.access_address(), f.connection.crc_init());
            let from: u64 = std::env::var("SDRTOP_REPLAY_FROM")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            let mut adv = Receiver::new(20e6, 38, crate::signal::ble::Phy::OneM, 2_426e6).unwrap();
            let mut links: Vec<(u8, Receiver)> = (7..=14u8)
                .map(|ch| {
                    (
                        ch,
                        Receiver::for_link(
                            20e6,
                            ch,
                            crate::signal::ble::Phy::OneM,
                            2_426e6,
                            Link::Data {
                                access_address: aa,
                                crc_init: crc,
                            },
                        )
                        .unwrap(),
                    )
                })
                .collect();
            let mut f = std::fs::File::open(&path).unwrap();
            use std::io::Seek;
            f.seek(std::io::SeekFrom::Start(from * 2)).unwrap();
            let mut at = from;
            let mut heard = Vec::new();
            loop {
                let mut buf = vec![0u8; 131_072 * 2];
                let mut got = 0;
                while got < buf.len() {
                    let n = f.read(&mut buf[got..]).unwrap();
                    if n == 0 {
                        break;
                    }
                    got += n;
                }
                if got < buf.len() {
                    break;
                }
                let mut iq = Vec::new();
                crate::signal::demod::decode(&buf, eight_bit(), usize::MAX, &mut iq);
                for p in adv.push_iq_at(&iq, at) {
                    if p.pdu_type == crate::signal::ble::pdu::PduType::ConnectInd {
                        let end = p.pdu_pair.map(|c| {
                            c + (crate::signal::ble::pdu::used_bits(p.length) as f64 - 0.5) * 20.0
                        });
                        eprintln!(
                            "CONNECT_IND crc {} pdu_pair {:?} end {:?}",
                            p.crc_ok, p.pdu_pair, end
                        );
                    }
                }
                for (ch, rx) in links.iter_mut() {
                    rx.push_iq_at(&iq, at);
                    for (d, timing) in rx.take_data() {
                        heard.push((timing.start_pair, *ch, d.llid, d.payload.len(), d.crc_ok));
                    }
                }
                at += 131_072;
                if at > from + 200_000_000 {
                    break;
                }
            }
            heard.sort_by(|a, b| a.0.total_cmp(&b.0));
            eprintln!("heard with AA {aa:08x}: {}", heard.len());
            for h in heard.iter().take(60) {
                eprintln!(
                    "  start {:.1} ch{} llid {} len {} crc {}",
                    h.0, h.1, h.2, h.3, h.4
                );
            }
        }
        for f in &m.net.ble_connections {
            let c = &f.connection;
            let p = c.params();
            eprintln!(
                "AA {:08x} CSA#{} interval {} win {}+{} sca {} state {:?}",
                c.access_address(),
                c.algorithm(),
                p.interval,
                p.win_offset,
                p.win_size,
                p.sca,
                c.state()
            );
            for e in c.events().iter().rev() {
                let ctl: Vec<String> = e
                    .pdus
                    .iter()
                    .filter_map(|h| {
                        h.control
                            .as_ref()
                            .map(|k| format!("{:?} {}", k.name, k.words))
                    })
                    .collect();
                if !ctl.is_empty() || (125..=150).contains(&e.counter) {
                    eprintln!(
                        "  {:>4} ch{:>2} {:?} {:?} {:?}",
                        e.counter,
                        e.channel,
                        e.account,
                        e.pdus
                            .iter()
                            .map(|h| (h.sender, h.pdu.llid, h.pdu.payload.len(), h.pdu.crc_ok))
                            .collect::<Vec<_>>(),
                        ctl
                    );
                }
            }
            eprintln!(
                "encrypted from {:?} · clock {:?} · t_ifs {:?}",
                c.encrypted_from(),
                c.clock_ppm(),
                c.t_ifs()
            );
        }
    }

    /// **The band is measured where it is shown.** On the survey a dwell of
    /// noise fills the band; on the Advertising view the same samples leave
    /// it unmeasured, and the receiver it was costing time runs alone.
    #[test]
    fn the_band_is_measured_on_the_survey_only() {
        use crate::signal::dsp::testkit::Rng;
        let block: Vec<u8> = Rng::new(7)
            .noise(131_072, 0.02)
            .iter()
            .flat_map(|z| {
                let q = |v: f32| (v * 128.0).clamp(-127.0, 127.0) as i8 as u8;
                [q(z.re), q(z.im)]
            })
            .collect();
        let run = |section: &str, preset: &str| {
            let mut m = SdrMetrics::fixture().streaming();
            m.ui.section = section.to_string();
            m.ui.active_preset = preset.to_string();
            m.radio.frequency = 2_426_000_000;
            m.radio.config_sample_rate = 20e6;
            m.radio.bb_filter_hz = 0;
            let state = Arc::new(Mutex::new(m));
            let (tx, rx) = crossbeam_channel::unbounded();
            // A dwell is 50 ms of windows, a block 6.5 ms: a dozen is enough.
            for seq in 1..=12 {
                tx.send(stamped(&state, seq, false, block.clone())).unwrap();
            }
            drop(tx);
            NetWorker::new(rx, Arc::clone(&state), eight_bit(), SAFE_BT_CHANNELS).run();
            let m = state.lock().unwrap().clone();
            m
        };
        let survey = run("net", super::super::lock::SURVEY_VIEW);
        assert!(
            !survey.net.band.cells.is_empty(),
            "the survey measures the band"
        );
        let ble = run("le", "net_ble");
        assert!(ble.net.band.cells.is_empty(), "LE 2 does not");
        assert_eq!(ble.net.ble_channel, Some(38), "and its receiver still runs");
    }

    /// An LE Coded S=8 `ADV_EXT_IND` on channel 38 at 20 Msps, the radio at
    /// 2426 MHz, starting at pair `start` of `blocks` blocks of 131 072 pairs,
    /// as the radio's eight-bit bytes.
    fn coded_on_the_air(start: usize, blocks: usize) -> Vec<Vec<u8>> {
        use crate::signal::ble::coded::{self, Coding};
        use crate::signal::ble::detect::ADVERTISING_ACCESS_ADDRESS;
        let symbols = coded::transmit(
            ADVERTISING_ACCESS_ADDRESS,
            Coding::S8,
            38,
            0x07,
            &[6, 0b0001_1000, 0x23, 0x31, 0x09, 0x64, 0x40],
        );
        let wave = crate::signal::ble::gfsk::modulate(&symbols, 20, 250e3, 20e6, 0.5);
        let len = 131_072;
        let mut iq = vec![num_complex::Complex::new(0.0f32, 0.0); len * blocks];
        let mut rng = crate::signal::dsp::testkit::Rng::new(5);
        for (z, n) in iq.iter_mut().zip(rng.noise(len * blocks, 0.0005)) {
            *z = n;
        }
        for (k, w) in wave.iter().enumerate() {
            iq[start + k] += w * 0.5;
        }
        let q = |v: f32| (v * 128.0).clamp(-127.0, 127.0) as i8 as u8;
        iq.chunks(len)
            .map(|c| c.iter().flat_map(|z| [q(z.re), q(z.im)]).collect())
            .collect()
    }

    fn run_view(section: &str, preset: &str, blocks: &[(u64, Vec<u8>)]) -> SdrMetrics {
        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = section.to_string();
        m.ui.active_preset = preset.to_string();
        m.radio.frequency = 2_426_000_000;
        m.radio.config_sample_rate = 20e6;
        m.radio.bb_filter_hz = 0;
        let state = Arc::new(Mutex::new(m));
        let (tx, rx) = crossbeam_channel::unbounded();
        for (seq, bytes) in blocks {
            tx.send(stamped(&state, *seq, false, bytes.clone()))
                .unwrap();
        }
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), eight_bit(), SAFE_BT_CHANNELS).run();
        let m = state.lock().unwrap().clone();
        m
    }

    /// Coded S=8 packets on the air at 20 Msps, the radio at 2426 MHz:
    /// `(channel, header octet 0, payload, first pair)` each, in `blocks`
    /// blocks of 131 072 pairs, as the radio's eight-bit bytes.
    fn coded_scene(packets: &[(u8, u8, Vec<u8>, usize)], blocks: usize) -> Vec<Vec<u8>> {
        coded_scene_at(packets, blocks, 0.5, num_complex::Complex::new(0.0, 0.0))
    }

    /// [`coded_scene`] with the packets sent at `amplitude` and the radio's
    /// DC offset `dc` added to every sample.
    fn coded_scene_at(
        packets: &[(u8, u8, Vec<u8>, usize)],
        blocks: usize,
        amplitude: f32,
        dc: num_complex::Complex<f32>,
    ) -> Vec<Vec<u8>> {
        use crate::signal::ble::coded::{self, Coding};
        use crate::signal::ble::detect::ADVERTISING_ACCESS_ADDRESS;
        let len = 131_072;
        let mut rng = crate::signal::dsp::testkit::Rng::new(6);
        let mut iq = rng.noise(len * blocks, 0.0005);
        iq.iter_mut().for_each(|z| *z += dc);
        for (ch, byte0, payload, start) in packets {
            let symbols =
                coded::transmit(ADVERTISING_ACCESS_ADDRESS, Coding::S8, *ch, *byte0, payload);
            let wave = crate::signal::ble::gfsk::modulate(&symbols, 20, 250e3, 20e6, 0.5);
            let shift = crate::signal::ble::channel::centre_hz(*ch).unwrap() as f64 - 2.426e9;
            let step = std::f64::consts::TAU * shift / 20e6;
            for (k, w) in wave.iter().enumerate() {
                let rot =
                    num_complex::Complex::from_polar(amplitude, (step * (start + k) as f64) as f32);
                iq[start + k] += w * rot;
            }
        }
        let q = |v: f32| (v * 128.0).clamp(-127.0, 127.0) as i8 as u8;
        iq.chunks(len)
            .map(|c| c.iter().flat_map(|z| [q(z.re), q(z.im)]).collect())
            .collect()
    }

    /// An ADV_EXT_IND's payload: ADI 0x3123 (SID 3, DID 0x123) and an
    /// AuxPtr to `channel`, 100 units of 30 us on, LE Coded, CA 1.
    fn adv_ext_ind(channel: u8) -> Vec<u8> {
        let v: u32 = channel as u32 | 1 << 6 | 100 << 8 | 0b010 << 21;
        vec![
            6,
            0b0001_1000,
            0x23,
            0x31,
            v as u8,
            (v >> 8) as u8,
            (v >> 16) as u8,
        ]
    }

    /// Its AUX_ADV_IND's: AdvA 66:55:44:33:22:11, the same ADI, the name.
    fn aux_adv_ind() -> Vec<u8> {
        let mut payload = vec![
            9,
            0b0000_1001,
            0x11,
            0x22,
            0x33,
            0x44,
            0x55,
            0x66,
            0x23,
            0x31,
        ];
        payload.extend([6, 0x09, b'P', b'i', b'x', b'e', b'l']);
        payload
    }

    fn scene_view(packets: &[(u8, u8, Vec<u8>, usize)]) -> SdrMetrics {
        let blocks: Vec<(u64, Vec<u8>)> = coded_scene(packets, 4)
            .into_iter()
            .enumerate()
            .map(|(i, b)| (i as u64 + 1, b))
            .collect();
        run_view("coded", "net_coded", &blocks)
    }

    /// **An LE Coded advertisement is followed to its auxiliary packet.**
    /// The ADV_EXT_IND on 38 points 3000 us on to data channel 9, in view;
    /// the AUX_ADV_IND there is heard, bound to it, and names the advertiser.
    #[test]
    fn a_coded_advertisement_is_followed_to_its_aux() {
        use crate::signal::ble::aux_ptr::AuxOutcome;
        let m = scene_view(&[
            (38, 0x07, adv_ext_ind(9), 20_000),
            (9, 0x47, aux_adv_ind(), 20_000 + 60_000),
        ]);
        assert_eq!(m.net.coded_packets.len(), 2, "{:?}", m.net.coded_packets);
        let aux = &m.net.coded_packets[0];
        let superior = &m.net.coded_packets[1];
        let aux_ext = aux.ext.as_ref().expect("read as extended");
        assert_eq!(
            aux_ext.role,
            crate::state::ExtRole::AuxAdv {
                superior_seq: Some(superior.seq)
            }
        );
        assert_eq!(aux.channel, 9);
        assert_eq!(aux.adv_addr, Some([0x66, 0x55, 0x44, 0x33, 0x22, 0x11]));
        match superior.ext.as_ref().expect("read as extended").aux {
            AuxOutcome::Heard { seq, after_us } => {
                assert_eq!(seq, aux.seq);
                assert!((after_us - 3000.0).abs() < 31.0, "{after_us}");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(m.net.health.aux.heard, 1);
    }

    /// An AuxPtr to a channel the radio does not see is said to be one.
    #[test]
    fn an_aux_out_of_view_is_said_to_be() {
        use crate::signal::ble::aux_ptr::AuxOutcome;
        let m = scene_view(&[(38, 0x07, adv_ext_ind(30), 20_000)]);
        assert_eq!(m.net.coded_packets.len(), 1);
        let ext = m.net.coded_packets[0].ext.as_ref().unwrap();
        assert_eq!(ext.aux, AuxOutcome::NotInView);
        assert_eq!(m.net.health.aux.not_in_view, 1);
    }

    /// An AuxPtr to LE 2M at 20 Msps, where no LE 2M receiver can be
    /// built: not followed, and why, rather than "its samples were not held".
    #[test]
    fn an_aux_on_a_phy_this_rate_cannot_receive_is_refused() {
        use crate::signal::ble::aux_ptr::AuxOutcome;
        let mut payload = adv_ext_ind(9);
        // The AuxPtr's PHY bits, 21 to 23 of its three octets: LE 2M.
        payload[6] = (payload[6] & 0b0001_1111) | 0b001 << 5;
        let m = scene_view(&[(38, 0x07, payload, 20_000)]);
        let ext = m.net.coded_packets[0].ext.as_ref().unwrap();
        assert!(matches!(ext.aux, AuxOutcome::Refused(_)), "{:?}", ext.aux);
        assert_eq!(m.net.health.aux.feed_lost, 0);
        assert_eq!(m.net.health.aux.refused, 1);
    }

    /// In view, listened to, and nothing there: missed.
    #[test]
    fn an_aux_not_sent_is_missed() {
        use crate::signal::ble::aux_ptr::AuxOutcome;
        let m = scene_view(&[(38, 0x07, adv_ext_ind(9), 20_000)]);
        let ext = m.net.coded_packets[0].ext.as_ref().unwrap();
        assert_eq!(ext.aux, AuxOutcome::Missed);
        assert_eq!(m.net.health.aux.missed, 1);
    }

    /// LE 1M advertising packets on the air at 20 Msps, the radio at 2426
    /// MHz: `(channel, header octet 0, payload, first pair)` each, in
    /// `blocks` blocks, as the radio's eight-bit bytes.
    fn le_scene(packets: &[(u8, u8, Vec<u8>, usize)], blocks: usize) -> Vec<Vec<u8>> {
        le_scene_at(packets, blocks, 0.5, num_complex::Complex::new(0.0, 0.0))
    }

    /// [`le_scene`] with the packets sent at `amplitude` and the radio's DC
    /// offset `dc` added to every sample.
    fn le_scene_at(
        packets: &[(u8, u8, Vec<u8>, usize)],
        blocks: usize,
        amplitude: f32,
        dc: num_complex::Complex<f32>,
    ) -> Vec<Vec<u8>> {
        use crate::signal::ble::detect::{
            access_address_bits, preamble_bits, ADVERTISING_ACCESS_ADDRESS,
        };
        let len = 131_072;
        let mut iq = crate::signal::dsp::testkit::Rng::new(8).noise(len * blocks, 0.0005);
        iq.iter_mut().for_each(|z| *z += dc);
        for (ch, byte0, payload, start) in packets {
            let mut bits = preamble_bits(ADVERTISING_ACCESS_ADDRESS, crate::signal::ble::Phy::OneM);
            bits.extend_from_slice(&access_address_bits(ADVERTISING_ACCESS_ADDRESS));
            bits.extend(crate::signal::ble::pdu::encode(*ch, *byte0, payload));
            let wave = crate::signal::ble::gfsk::modulate(&bits, 20, 250e3, 20e6, 0.5);
            let shift = crate::signal::ble::channel::centre_hz(*ch).unwrap() as f64 - 2.426e9;
            let step = std::f64::consts::TAU * shift / 20e6;
            for (k, w) in wave.iter().enumerate() {
                let rot =
                    num_complex::Complex::from_polar(amplitude, (step * (start + k) as f64) as f32);
                iq[start + k] += w * rot;
            }
        }
        let q = |v: f32| (v * 128.0).clamp(-127.0, 127.0) as i8 as u8;
        iq.chunks(len)
            .map(|c| c.iter().flat_map(|z| [q(z.re), q(z.im)]).collect())
            .collect()
    }

    /// The LE Advertising view over `le_scene`'s packets, four blocks.
    fn le_view(packets: &[(u8, u8, Vec<u8>, usize)]) -> SdrMetrics {
        let blocks: Vec<(u64, Vec<u8>)> = le_scene(packets, 4)
            .into_iter()
            .enumerate()
            .map(|(i, b)| (i as u64 + 1, b))
            .collect();
        run_view("le", "net_ble", &blocks)
    }

    /// How faint the packets of the two tests below are: about 12 dB over
    /// the noise in 1 MHz, a packet across a few rooms.
    const FAINT: f32 = 0.02;

    /// The radio's DC offset in those tests: about 14 dB over the faint
    /// packet, as a HackRF's was on the air, under a packet at the tuned
    /// centre.
    const RADIO_DC: (f32, f32) = (0.08, -0.05);

    /// `blocks` numbered from 1 as the worker's feed numbers them.
    fn numbered(blocks: Vec<Vec<u8>>) -> Vec<(u64, Vec<u8>)> {
        blocks
            .into_iter()
            .enumerate()
            .map(|(i, b)| (i as u64 + 1, b))
            .collect()
    }

    /// **The radio's DC is taken off before any receiver sees the block.**
    /// A packet at the tuned centre, as every advertising packet is in
    /// LOCK, shares baseband 0 Hz with the radio's own DC offset. Added to
    /// the samples, not to the frequency, a DC stronger than the packet
    /// keeps the phasor from turning about the origin, and the frequency
    /// the receivers read falls apart: on the air an LE Coded phone two
    /// rooms away was heard in 0 of 77 advertisements until it was taken
    /// off. The same faint packet with no DC is the control.
    #[test]
    fn a_faint_coded_packet_at_the_centre_is_heard_through_the_radios_dc() {
        for dc in [
            num_complex::Complex::new(0.0, 0.0),
            num_complex::Complex::new(RADIO_DC.0, RADIO_DC.1),
        ] {
            let blocks = coded_scene_at(&[(38, 0x07, adv_ext_ind(9), 20_000)], 4, FAINT, dc);
            let m = run_view("coded", "net_coded", &numbered(blocks));
            let heard = m.net.coded_packets.iter().filter(|p| p.crc_ok).count();
            assert_eq!(heard, 1, "DC {dc}: {:?}", m.net.health.coded);
        }
    }

    /// How faint the LE 1M test's packet is: LE 1M has no coding gain, so it
    /// is sent three times as strong as [`FAINT`], clear of where it stops
    /// being heard, and the radio's DC is still stronger.
    const LE_FAINT: f32 = 0.06;

    /// LE 1M the same: on the air, half again as many packets passed their
    /// CRC once the DC was taken off.
    #[test]
    fn a_faint_le_1m_packet_at_the_centre_is_heard_through_the_radios_dc() {
        let payload: Vec<u8> = (0..20).collect();
        for dc in [
            num_complex::Complex::new(0.0, 0.0),
            num_complex::Complex::new(RADIO_DC.0, RADIO_DC.1),
        ] {
            let blocks = le_scene_at(&[(38, 0x02, payload.clone(), 20_000)], 4, LE_FAINT, dc);
            let m = run_view("le", "net_ble", &numbered(blocks));
            let heard = m.net.ble_packets.iter().filter(|p| p.crc_ok).count();
            assert_eq!(heard, 1, "DC {dc}: {:?}", m.net.health.ble);
        }
    }

    /// `adv_ext_ind(channel)` with its AuxPtr's PHY set to `phy` (Table
    /// 2.14's three bits: 0 LE 1M, 1 LE 2M, 2 LE Coded).
    fn adv_ext_ind_on(channel: u8, phy: u8) -> Vec<u8> {
        let mut payload = adv_ext_ind(channel);
        payload[6] = (payload[6] & 0b0001_1111) | phy << 5;
        payload
    }

    /// **An LE 1M advertisement is followed to its auxiliary packet**, as an
    /// LE Coded one is: the `ADV_EXT_IND` on 38 points 3000 us on to data
    /// channel 9 on LE 1M; the `AUX_ADV_IND` there is heard, joins the LE
    /// list bound to it, and names the advertiser.
    #[test]
    fn a_1m_adv_ext_ind_is_followed_to_its_aux() {
        use crate::signal::ble::aux_ptr::AuxOutcome;
        let m = le_view(&[
            (38, 0x07, adv_ext_ind_on(9, 0), 20_000),
            (9, 0x47, aux_adv_ind(), 20_000 + 60_000),
        ]);
        let list: Vec<_> = m.net.ble_packets.iter().collect();
        assert_eq!(list.len(), 2, "{list:?}");
        let (aux, superior) = (list[0], list[1]);
        assert_eq!(superior.channel, 38);
        let aux_ext = aux.ext.as_ref().expect("read as extended");
        assert_eq!(
            aux_ext.role,
            crate::state::ExtRole::AuxAdv {
                superior_seq: Some(superior.seq)
            }
        );
        assert_eq!(aux.channel, 9);
        assert_eq!(aux.phy, crate::signal::ble::Phy::OneM);
        assert_eq!(aux.adv_addr, Some([0x66, 0x55, 0x44, 0x33, 0x22, 0x11]));
        let sup_ext = superior.ext.as_ref().expect("read as extended");
        assert!(
            matches!(
                sup_ext.aux,
                AuxOutcome::Heard { seq, after_us } if seq == aux.seq && (after_us - 3000.0).abs() < 5.0
            ),
            "{:?}",
            sup_ext.aux
        );
        assert_eq!(m.net.health.aux.heard, 1);
        // Nothing on the LE Coded list: one advertisement, one list.
        assert!(m.net.coded_packets.is_empty());
    }

    /// An LE 1M capture the worker cuts short at a break still ends in the
    /// funnel, as LE Coded's does: every trigger decoded, failed or given up.
    #[test]
    fn a_ble_capture_cut_by_a_break_still_ends_in_the_funnel() {
        let payload: Vec<u8> = (0..31u8).collect();
        // The packet (about 6 600 pairs) straddles the first block's end.
        let blocks = le_scene(&[(38, 0x02, payload, 131_072 - 2_000)], 3);
        let sent = vec![(1, blocks[0].clone()), (3, blocks[2].clone())];
        let m = run_view("le", "net_ble", &sent);
        let f = m.net.health.ble;
        assert!(f.gave_up >= 1, "{f:?}");
        assert_eq!(f.triggered, f.decoded + f.crc_failed + f.gave_up, "{f:?}");
    }

    /// **An extended advertiser is counted in the census.** Its address and
    /// name are in the auxiliary packet, not the `ADV_EXT_IND`, so the
    /// followed aux is what counts it: by its AdvA, with what its AdvData
    /// says, and with no arrival, since an aux's timing is its primary's
    /// offset, not an advertising interval.
    #[test]
    fn an_extended_advertiser_is_counted_in_the_census() {
        let m = le_view(&[
            (38, 0x07, adv_ext_ind_on(9, 0), 20_000),
            (9, 0x47, aux_adv_ind(), 20_000 + 60_000),
        ]);
        let addr = [0x66, 0x55, 0x44, 0x33, 0x22, 0x11];
        let devices = &m.net.census.devices;
        assert_eq!(devices.len(), 1, "{devices:?}");
        let d = &devices[0];
        assert_eq!(d.address, addr);
        assert_eq!(d.ble_pdu_codes().collect::<Vec<_>>(), [0x07]);
        let said = m.net.advertised.get(&addr).expect("its AdvData read");
        assert_eq!(said.name.as_ref().map(|n| n.0.as_str()), Some("Pixel"));
    }

    /// An LE 1M advertisement whose AuxPtr is out of view says so, on the
    /// LE list, and an LE 2M one at 20 Msps is not followed, and why.
    #[test]
    fn a_1m_adv_ext_ind_says_what_became_of_its_aux() {
        use crate::signal::ble::aux_ptr::AuxOutcome;
        let m = le_view(&[(38, 0x07, adv_ext_ind_on(30, 0), 20_000)]);
        let ext = m.net.ble_packets[0].ext.as_ref().expect("read as extended");
        assert_eq!(ext.aux, AuxOutcome::NotInView);
        let m = le_view(&[(38, 0x07, adv_ext_ind_on(9, 1), 20_000)]);
        let ext = m.net.ble_packets[0].ext.as_ref().expect("read as extended");
        assert!(matches!(ext.aux, AuxOutcome::Refused(_)), "{:?}", ext.aux);
    }

    /// **Samples from another tuning are not listened to.** A retune keeps
    /// the stream's positions running, so a promise whose window comes after
    /// it would be cut from samples mixed for the old centre. The ADV_EXT_IND
    /// points 6 ms on (20 units of 300 us), past its block; the next block
    /// arrives retuned by 2 MHz, and the promise ends not followed, and why.
    #[test]
    fn a_promise_across_a_retune_is_not_followed() {
        use crate::signal::ble::aux_ptr::AuxOutcome;
        let v: u32 = 9 | 1 << 6 | 1 << 7 | 20 << 8;
        let payload = vec![
            6,
            0b0001_1000,
            0x23,
            0x31,
            v as u8,
            (v >> 8) as u8,
            (v >> 16) as u8,
        ];
        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = "le".to_string();
        m.ui.active_preset = "net_ble".to_string();
        m.radio.frequency = 2_426_000_000;
        m.radio.config_sample_rate = 20e6;
        m.radio.bb_filter_hz = 0;
        let state = Arc::new(Mutex::new(m));
        let (tx, rx) = crossbeam_channel::unbounded();
        for (i, bytes) in le_scene(&[(38, 0x07, payload, 20_000)], 3)
            .into_iter()
            .enumerate()
        {
            let mut block = stamped(&state, i as u64 + 1, false, bytes);
            if i > 0 {
                block.centre_hz = 2_428_000_000;
            }
            tx.send(block).unwrap();
        }
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), eight_bit(), SAFE_BT_CHANNELS).run();
        let m = state.lock().unwrap();
        let ext = m.net.ble_packets[0].ext.as_ref().expect("read as extended");
        assert_eq!(
            ext.aux,
            AuxOutcome::Refused("the radio retuned before its window")
        );
        assert_eq!(m.net.health.aux.refused, 1);
    }

    /// **The LE Coded view runs LE Coded's receiver, and only it.** A Coded
    /// advertisement on channel 38 is one row in the Coded list, its scheme,
    /// repairs and measurement with it; the LE 1M list stays empty and its
    /// receiver gets no channel. On LE 2 the same samples give no Coded row.
    #[test]
    fn the_coded_view_runs_the_coded_receiver_alone() {
        let blocks: Vec<(u64, Vec<u8>)> = coded_on_the_air(20_000, 2)
            .into_iter()
            .enumerate()
            .map(|(i, b)| (i as u64 + 1, b))
            .collect();
        let m = run_view("coded", "net_coded", &blocks);
        assert_eq!(m.net.coded_channel, Some(38));
        assert_eq!(m.net.coded_packets.len(), 1, "{:?}", m.net.coded_refused);
        let p = &m.net.coded_packets[0];
        assert_eq!(
            p.phy,
            crate::signal::ble::Phy::Coded(crate::signal::ble::coded::Coding::S8)
        );
        assert!(p.crc_ok);
        let facts = p.coded.as_ref().expect("an LE Coded packet's facts");
        assert!(facts.reading.and_then(|r| r.snr_db).is_some(), "measured");
        assert!(m.net.ble_packets.is_empty());
        assert_eq!(m.net.ble_channel, None);
        assert_eq!(m.net.health.coded.decoded, 1);

        let le2 = run_view("le", "net_ble", &blocks);
        assert!(le2.net.coded_packets.is_empty());
    }

    /// **A gap inside a Coded packet is not stitched.** The block that held
    /// its middle never arrives; what is left on either side is no packet.
    #[test]
    fn a_gap_inside_a_coded_capture_is_not_stitched() {
        let blocks = coded_on_the_air(120_000, 3);
        let sent = vec![(1, blocks[0].clone()), (3, blocks[2].clone())];
        let m = run_view("coded", "net_coded", &sent);
        assert!(m.net.coded_packets.is_empty(), "{:?}", m.net.coded_packets);
    }

    /// A capture the worker cuts short at a break still ends in the funnel:
    /// every trigger is decoded, failed or given up, and the counts add up.
    #[test]
    fn a_capture_cut_by_a_break_still_ends_in_the_funnel() {
        let blocks = coded_on_the_air(120_000, 3);
        let sent = vec![(1, blocks[0].clone()), (3, blocks[2].clone())];
        let m = run_view("coded", "net_coded", &sent);
        let f = m.net.health.coded;
        assert!(f.gave_up >= 1, "{f:?}");
        assert_eq!(f.triggered, f.decoded + f.crc_failed + f.gave_up, "{f:?}");
    }

    /// **A connection heard set up is followed.** Its CONNECT_IND on
    /// channel 38 starts it; every event on a channel the 20 MHz window
    /// holds (data channels 7 to 14) is followed, its Central placed at the
    /// anchor and its Peripheral T_IFS after; the rest are out of view.
    #[test]
    fn a_connection_heard_set_up_is_followed() {
        use crate::signal::ble::follow::{Account, Sender};
        let (blocks, aa) = a_connection_on_the_air(24);
        let m = follow_over(&blocks, &[]);
        assert_eq!(m.net.ble_connections.len(), 1);
        let c = &m.net.ble_connections[0].connection;
        assert_eq!(c.access_address(), aa);
        let events = c.events();
        assert!(events.len() >= 20, "{} accounted", events.len());
        for e in events {
            let in_view = (7..=14).contains(&e.channel);
            assert_eq!(e.account == Account::NotInView, !in_view, "{e:?}");
            if in_view {
                assert_eq!(e.account, Account::Followed, "{e:?}");
                let senders: Vec<_> = e.pdus.iter().map(|p| p.sender).collect();
                assert_eq!(
                    senders,
                    [Some(Sender::Central), Some(Sender::Peripheral)],
                    "{e:?}"
                );
                let t_ifs = e.pdus[1].t_ifs_us.unwrap();
                assert!((t_ifs - 150.0).abs() < 1.0, "{t_ifs}");
            }
        }
        assert!(
            events
                .iter()
                .filter(|e| e.account == Account::Followed)
                .count()
                >= 5
        );
    }

    /// **The advertising PDU's ChSel counts too.** A CONNECT_IND with ChSel
    /// set answering an ADV_DIRECT_IND without it is a CSA #1 connection,
    /// and is followed as one; with the advertising PDU never heard, which
    /// algorithm is unknown and the connection says so.
    #[test]
    fn the_answered_advertisings_chsel_picks_the_algorithm() {
        use crate::signal::ble::follow::{Account, State};
        let (blocks, _) = a_connection_with_chsel(24, true, Some(false));
        let m = follow_over(&blocks, &[]);
        let c = &m.net.ble_connections[0].connection;
        assert_eq!(c.algorithm(), 1);
        assert!(c.events().iter().any(|e| e.account == Account::Followed));

        let (blocks, _) = a_connection_with_chsel(24, true, None);
        let m = follow_over(&blocks, &[]);
        let c = &m.net.ble_connections[0].connection;
        assert!(
            matches!(c.state(), State::NotFollowed { .. }),
            "{:?}",
            c.state()
        );
    }

    /// A block lost inside an event's window: that event is not listened
    /// to, and says so; the ones after are followed again.
    #[test]
    fn an_event_in_a_lost_block_is_feed_lost() {
        use crate::signal::ble::follow::Account;
        let (blocks, _) = a_connection_on_the_air(24);
        // Event 6, on channel 12, has its anchor in the eighth block.
        let m = follow_over(&blocks, &[7]);
        let c = &m.net.ble_connections[0].connection;
        let six = c.events().iter().find(|e| e.counter == 6).unwrap();
        assert_eq!(six.account, Account::FeedLost, "{six:?}");
        let later = c.events().iter().find(|e| e.counter == 11).unwrap();
        assert_eq!(later.account, Account::Followed, "{later:?}");
    }

    /// Surveying, a position whose centre is not a BLE channel still decodes
    /// the advertising channel it holds: tuned to 2403.5 MHz, the first
    /// position of an 8 Msps pass, a channel-37 packet 1.5 MHz below the
    /// centre is heard as channel 37. The tuning alone gave data channel 0
    /// there, and 37 was never decoded in a survey.
    #[test]
    fn a_survey_position_decodes_the_advertising_channel_it_holds() {
        use crate::signal::ble::detect::{
            access_address_bits, preamble_bits, ADVERTISING_ACCESS_ADDRESS,
        };
        use crate::signal::ble::gfsk::modulate;
        use crate::signal::ble::pdu::encode;
        use crate::signal::ble::Phy;
        use crate::signal::dsp::nco::Nco;
        use crate::signal::dsp::testkit::{at_snr, Rng};
        use num_complex::Complex;

        const RATE: f64 = 8_000_000.0;
        let tuned = 2_403_500_000u64;
        let addr = [0x21u8, 0x32, 0x43, 0x54, 0x65, 0x76];
        let mut bits: Vec<bool> = (0..32).map(|i| i % 3 == 0).collect();
        bits.extend(preamble_bits(ADVERTISING_ACCESS_ADDRESS, Phy::OneM));
        bits.extend_from_slice(&access_address_bits(ADVERTISING_ACCESS_ADDRESS));
        bits.extend_from_slice(&encode(
            37,
            0x00,
            &crate::signal::ble::pdu::air_octets(addr),
        ));
        let mut rng = Rng::new(3);
        bits.extend((0..64).map(|_| rng.next_u64() & 1 == 1));
        let mut iq = modulate(&bits, 8, Phy::OneM.deviation_hz(), RATE, 0.5);
        let offset = crate::signal::ble::channel::centre_hz(37).unwrap() as f64 - tuned as f64;
        Nco::new(offset, RATE).mix(&mut iq);
        let noisy = at_snr(&iq, 25.0, &mut Rng::new(4));
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
        m.ui.active_preset = "net_survey".to_string();
        m.radio.frequency = tuned;
        m.radio.config_sample_rate = RATE;
        m.radio.bb_filter_hz = 0;
        let state = Arc::new(Mutex::new(m));
        let (tx, rx) = crossbeam_channel::unbounded();
        tx.send(stamped(&state, 1, false, bytes)).unwrap();
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), geometry, SAFE_BT_CHANNELS).run();

        let m = state.lock().unwrap();
        assert_eq!(m.net.ble_channel, Some(37));
        assert_eq!(m.net.ble_packets.len(), 1, "{:?}", m.net.health.ble);
        let p = &m.net.ble_packets[0];
        assert_eq!(p.channel, 37);
        assert!(p.crc_ok);
        assert_eq!(p.adv_addr, Some(addr));
    }

    /// **LE 2M, run through the worker**: on a data channel a 2M packet is
    /// decoded, stamped as LE 2M, and measured like any other; on an
    /// advertising channel the decoder does not run at all and says why.
    #[test]
    fn le_2m_is_decoded_off_the_advertising_channels_and_refused_on_them() {
        let data_ch = 10u8;
        let (bytes, addr) = ble_2m_bytes(data_ch);
        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = crate::signal::net::SECTION.to_string();
        m.radio.frequency = crate::signal::ble::channel::centre_hz(data_ch).unwrap();
        m.radio.config_sample_rate = 8_000_000.0;
        m.radio.bb_filter_hz = 0;
        m.net.ble_phy = crate::signal::ble::Phy::TwoM;
        let state = Arc::new(Mutex::new(m));
        let (tx, rx) = crossbeam_channel::unbounded();
        tx.send(stamped(&state, 1, false, bytes)).unwrap();
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), eight_bit(), SAFE_BT_CHANNELS).run();
        {
            let m = state.lock().unwrap();
            assert!(m.net.ble_refused.is_none(), "{:?}", m.net.ble_refused);
            assert_eq!(m.net.ble_packets.len(), 1, "{:?}", m.net.health.ble);
            let p = &m.net.ble_packets[0];
            assert_eq!(p.phy, crate::signal::ble::Phy::TwoM);
            assert_eq!(p.adv_addr, Some(addr));
            assert!(p.crc_ok);
            // Measured on LE 2M's own clock.
            assert!(p.drift.is_some(), "{p:?}");
        }

        let (bytes, _) = ble_2m_bytes(37);
        {
            let mut m = state.lock().unwrap();
            m.radio.frequency = crate::signal::ble::channel::centre_hz(37).unwrap();
            m.net.ble_packets.clear();
        }
        let (tx, rx) = crossbeam_channel::unbounded();
        tx.send(stamped(&state, 1, false, bytes)).unwrap();
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), eight_bit(), SAFE_BT_CHANNELS).run();
        let m = state.lock().unwrap();
        let why = m.net.ble_refused.clone().unwrap_or_default();
        assert!(
            why.contains("LE 2M is not used on the primary advertising channels"),
            "{why}"
        );
        assert!(m.net.ble_packets.is_empty());
        assert_eq!(m.net.ble_channel, None);
    }

    /// The same synthetic ADV_IND the test above sends, as raw 4 Msps
    /// channel-37 bytes, and the state that goes with it.
    fn ble_packet_bytes() -> (Vec<u8>, [u8; 6], Arc<Mutex<SdrMetrics>>) {
        use crate::signal::ble::detect::{
            access_address_bits, preamble_bits, ADVERTISING_ACCESS_ADDRESS,
        };
        use crate::signal::ble::gfsk::modulate;
        use crate::signal::ble::pdu::encode;
        use crate::signal::ble::Phy;
        use crate::signal::dsp::testkit::{at_snr, Rng};
        use num_complex::Complex;

        let addr = [0x11u8, 0x22, 0x33, 0x44, 0x55, 0x66];
        let mut bits = preamble_bits(ADVERTISING_ACCESS_ADDRESS, Phy::OneM);
        bits.extend_from_slice(&access_address_bits(ADVERTISING_ACCESS_ADDRESS));
        bits.extend_from_slice(&encode(
            37,
            0x00,
            &crate::signal::ble::pdu::air_octets(addr),
        ));
        let mut rng = Rng::new(1);
        bits.extend((0..16).map(|_| rng.next_u64() & 1 == 1));
        let clean = modulate(&bits, 4, 250_000.0, 4_000_000.0, 0.5);
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
        m.radio.config_sample_rate = 4_000_000.0;
        m.radio.bb_filter_hz = 0;
        (bytes, addr, Arc::new(Mutex::new(m)))
    }

    /// Run the worker over the packet cut in two, the second half placed at
    /// `second_at` in the stream, and count the packets that came out.
    fn split_packet(second_at_offset: u64) -> usize {
        let (bytes, _, state) = ble_packet_bytes();
        // Cut inside the header: the preamble and access address are in the
        // first half, the rest of the packet in the second.
        let cut = (8 + 32 + 8) * 4 * 2;
        let first_pairs = cut as u64 / 2;
        let (tx, rx) = crossbeam_channel::unbounded();
        for (seq, first_pair, part) in [
            (1, 0, bytes[..cut].to_vec()),
            (2, first_pairs + second_at_offset, bytes[cut..].to_vec()),
        ] {
            tx.send(StreamBlock {
                seq,
                gap_before: false,
                bytes: part,
                first_pair,
                centre_hz: 2_402_000_000,
                rate_hz: 4_000_000.0,
            })
            .unwrap();
        }
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), eight_bit(), SAFE_BT_CHANNELS).run();
        let n = state.lock().unwrap().net.ble_packets.len();
        n
    }

    /// Control: a packet cut across two *contiguous* blocks is still one
    /// packet - the receiver carries its state across a block boundary, as it
    /// must, since packets straddle boundaries all the time.
    #[test]
    fn a_packet_across_two_contiguous_blocks_is_still_decoded() {
        assert_eq!(split_packet(0), 1);
    }

    /// **The regression.** The same bytes, but the second block sits a
    /// thousand pairs further on in the stream - a refused block in between.
    /// The worker used to carry the capture straight across, and here that
    /// even yields a clean CRC, because this test's second half happens to be
    /// the true continuation; on a real radio it never is, and the splice was
    /// a packet made of two moments. A capture must never span a break.
    #[test]
    fn a_capture_never_spans_a_break_in_the_stream() {
        assert_eq!(split_packet(1_000), 0);
    }

    /// A block is decoded at the tuning it was captured at, not at wherever
    /// the radio has moved to by the time the worker reaches it.
    #[test]
    fn a_block_is_decoded_at_the_tuning_it_was_captured_at() {
        let (bytes, addr, state) = ble_packet_bytes();
        let block = stamped(&state, 1, false, bytes);
        // The survey moves on before the worker gets to the block.
        state.lock().unwrap().radio.frequency = 2_480_000_000;
        let (tx, rx) = crossbeam_channel::unbounded();
        tx.send(block).unwrap();
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), eight_bit(), SAFE_BT_CHANNELS).run();
        let m = state.lock().unwrap();
        assert_eq!(m.net.ble_packets.len(), 1, "{:?}", m.net.ble_packets);
        assert_eq!(m.net.ble_packets[0].channel, 37, "the capture's channel");
        assert_eq!(m.net.ble_packets[0].adv_addr, Some(addr));
        assert!(m.net.ble_packets[0].crc_ok);
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
        tx.send(stamped(&state, 1, false, vec![0u8; 256])).unwrap();
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), eight_bit(), SAFE_BT_CHANNELS).run();
        let m = state.lock().unwrap();
        assert!(m.net.ble_refused.is_some());
        assert_eq!(m.net.ble_channel, None, "no receiver, no channel");
        assert!(m.net.ble_packets.is_empty());
    }

    /// Closing the section drops the receiver and the load figure with it,
    /// so neither is shown on reopening as if it were still true.
    #[test]
    fn a_closed_section_leaves_no_receiver_or_load_behind() {
        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = "lab".to_string();
        m.radio.frequency = 2_402_000_000;
        m.radio.config_sample_rate = 4_000_000.0;
        m.net.ble_channel = Some(37);
        m.net.health.decode_load = Some(0.4);
        let state = Arc::new(Mutex::new(m));
        let (tx, rx) = crossbeam_channel::unbounded();
        tx.send(stamped(&state, 1, false, vec![0u8; 256])).unwrap();
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), eight_bit(), SAFE_BT_CHANNELS).run();
        let m = state.lock().unwrap();
        assert_eq!(m.net.ble_channel, None);
        assert_eq!(m.net.health.decode_load, None);
    }

    /// `section` open on `preset`, tuned to an advertising channel, with an
    /// LE Coded account left over from before: what one block makes of it.
    fn coded_account_after(section: &str, preset: &str) -> SdrMetrics {
        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = section.to_string();
        m.ui.active_preset = preset.to_string();
        m.radio.frequency = 2_426_000_000;
        m.radio.config_sample_rate = 4_000_000.0;
        m.radio.bb_filter_hz = 0;
        m.net.coded_channel = Some(38);
        m.net.coded_refused = Some("an earlier visit's reason".to_string());
        let state = Arc::new(Mutex::new(m));
        let (tx, rx) = crossbeam_channel::unbounded();
        tx.send(stamped(&state, 1, false, vec![0u8; 256])).unwrap();
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), eight_bit(), SAFE_BT_CHANNELS).run();
        let m = state.lock().unwrap().clone();
        m
    }

    /// **Off the LE Coded view there is no LE Coded receiver, and the state
    /// says so.** Its channel is what the header's band field and the menu's
    /// live line read as "running"; left behind, the BLE view would show a
    /// CODED channel nothing is decoding. A refusal from an earlier visit goes
    /// too, as the classic account's does.
    #[test]
    fn leaving_the_coded_view_takes_its_channel_and_refusal_with_it() {
        let m = coded_account_after(crate::signal::net::SECTION, "net_ble");
        assert_eq!(m.net.coded_channel, None);
        assert_eq!(m.net.coded_refused, None);
    }

    /// The same once the section closes: nothing decodes, so nothing is
    /// shown as decoding on reopening.
    #[test]
    fn a_closed_section_leaves_no_coded_channel_behind() {
        let m = coded_account_after("lab", "net_coded");
        assert_eq!(m.net.coded_channel, None);
        assert_eq!(m.net.coded_refused, None);
    }

    /// **On the LE Coded view, LE 1M is off for that reason, and says so.**
    /// Tuned to 2426 MHz, an advertising channel, the BLE decoder is not
    /// running because LE Coded runs in its place, not because the tuning is
    /// off one: an export's BLE file repeats this reason word for word, and
    /// "not tuned to an advertising channel" there would be a false one.
    #[test]
    fn on_the_coded_view_the_ble_decoder_says_why_it_is_off() {
        let m = coded_account_after(crate::signal::net::SECTION, "net_coded");
        assert_eq!(m.net.ble_channel, None);
        assert_eq!(
            m.net.ble_refused.as_deref(),
            Some("the LE Coded view runs LE Coded in LE 1M's place")
        );
    }

    /// End to end through the worker: enough stream to cover a load window
    /// publishes a load. `feed`'s unit tests hold the arithmetic; this holds
    /// the wiring, which they cannot see.
    #[test]
    fn the_worker_publishes_a_load_once_a_window_of_stream_has_passed() {
        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = crate::signal::net::SECTION.to_string();
        m.radio.frequency = 2_437_000_000;
        m.radio.config_sample_rate = 1_000_000.0;
        let state = Arc::new(Mutex::new(m));
        let (tx, rx) = crossbeam_channel::unbounded();
        // Eight 100 000-pair blocks at 1 Msps: 0.8 s of stream.
        for seq in 1..=8 {
            tx.send(stamped(&state, seq, false, vec![0u8; 200_000]))
                .unwrap();
        }
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), eight_bit(), SAFE_BT_CHANNELS).run();
        let load = state.lock().unwrap().net.health.decode_load;
        assert!(load.is_some_and(|l| l > 0.0), "{load:?}");
    }

    /// A view too narrow for `signal::bt::receive::front_end`'s own working
    /// rate is refused, with a reason - the same "refused, not silent"
    /// discipline `ble_refused` already follows.
    #[test]
    fn a_view_too_narrow_for_the_working_rate_is_refused() {
        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = crate::signal::net::SECTION.to_string();
        m.ui.active_preset = "net_bt".to_string();
        m.radio.frequency = 2_437_000_000;
        m.radio.config_sample_rate = 2_000_000.0; // below the 4 Msps working rate
        let state = Arc::new(Mutex::new(m));
        let (tx, rx) = crossbeam_channel::unbounded();
        tx.send(stamped(&state, 1, false, vec![0u8; 256])).unwrap();
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), eight_bit(), SAFE_BT_CHANNELS).run();
        let m = state.lock().unwrap();
        assert!(m.net.bt_refused.is_some(), "{:?}", m.net.bt_refused);
        assert!(m.net.bt_hops.is_empty());
        assert!(m.net.bt_channels_watched.is_empty());
    }

    /// A view wide enough to hold real classic BT channels clears the
    /// refusal and says which channels it is actually watching - capped at
    /// [`SAFE_BT_CHANNELS`] here, not the full count a 20 MHz view could
    /// otherwise see, because the default config asks for no more.
    #[test]
    fn a_wide_enough_view_clears_the_refusal_and_names_the_watched_channels() {
        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = crate::signal::net::SECTION.to_string();
        m.ui.active_preset = "net_bt".to_string();
        m.radio.frequency = 2_441_000_000;
        m.radio.config_sample_rate = 20_000_000.0;
        m.radio.bb_filter_hz = 0;
        let state = Arc::new(Mutex::new(m));
        let (tx, rx) = crossbeam_channel::unbounded();
        tx.send(stamped(&state, 1, false, vec![0u8; 256])).unwrap();
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), eight_bit(), SAFE_BT_CHANNELS).run();
        let m = state.lock().unwrap();
        assert!(m.net.bt_refused.is_none(), "{:?}", m.net.bt_refused);
        assert_eq!(m.net.bt_channels_watched.len(), SAFE_BT_CHANNELS);
    }

    /// Through the actual worker rather than
    /// `signal::bt::receive::Receiver` directly: a synthetic classic BT
    /// access code sitting on one channel of a wideband capture reaches
    /// `net.bt_hops`, tagged with the channel it was found on.
    #[test]
    fn a_synthetic_classic_bt_access_code_reaches_bt_hops() {
        use crate::signal::ble::gfsk::modulate;
        use crate::signal::bt::access_code::access_code_bits;
        use crate::signal::dsp::testkit::{at_snr, Rng};
        use num_complex::Complex;

        const RAW_RATE: f64 = 20_000_000.0;
        // The channel IS the tuned centre - zero offset, so it lands inside
        // the watched cap regardless of how the nearest-first sort breaks
        // ties, the same worked-example shape
        // `signal::bt::receive::the_channel_and_the_tuning_can_both_vary`
        // already exercises directly.
        let ch = 45u8;
        let channel_hz = crate::signal::bt::channel::centre_hz(ch).unwrap();
        let lap = 0x0044_5566;

        // Forty settling symbols either side - see
        // `signal::bt::receive`'s own test-module doc (`SETTLE_SYMBOLS`)
        // for why a bare few bits of margin let the modulator's own pulse
        // shaping and this receiver's own channel-select filter corrupt the
        // access code under test rather than merely surrounding it.
        let mut bits: Vec<bool> = (0..40).map(|i| i % 2 == 0).collect();
        bits.extend(access_code_bits(lap));
        bits.extend((0..40).map(|i| i % 2 == 1));
        let sps = (RAW_RATE / 1_000_000.0) as usize;
        let clean = modulate(&bits, sps, 160_000.0, RAW_RATE, 0.5);
        let placed = at_snr(&clean, 40.0, &mut Rng::new(7));

        let geometry = eight_bit();
        let bytes: Vec<u8> = placed
            .iter()
            .flat_map(|s: &Complex<f32>| {
                let re = (s.re * geometry.full_scale).clamp(-127.0, 127.0) as i8;
                let im = (s.im * geometry.full_scale).clamp(-127.0, 127.0) as i8;
                [re as u8, im as u8]
            })
            .collect();

        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = crate::signal::net::SECTION.to_string();
        m.ui.active_preset = "net_bt".to_string();
        m.radio.frequency = channel_hz;
        m.radio.config_sample_rate = RAW_RATE;
        m.radio.bb_filter_hz = 0;
        let state = Arc::new(Mutex::new(m));

        let (tx, rx) = crossbeam_channel::unbounded();
        tx.send(stamped(&state, 1, false, bytes)).unwrap();
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), geometry, SAFE_BT_CHANNELS).run();

        let m = state.lock().unwrap();
        assert!(m.net.bt_refused.is_none(), "{:?}", m.net.bt_refused);
        assert_eq!(m.net.bt_hops.len(), 1, "{:?}", m.net.bt_hops);
        let hop = &m.net.bt_hops[0];
        assert_eq!(hop.channel, ch);
        assert_eq!(hop.lap, lap);
    }

    /// The same access code on the survey, once its load has left room for
    /// one channel: the nearest classic channel runs, its hit reaches
    /// `net.bt_hops` for the coexistence history, and the account says the
    /// load holds it to that one.
    #[test]
    fn the_survey_marks_classic_hits_once_its_load_leaves_room() {
        use crate::signal::ble::gfsk::modulate;
        use crate::signal::bt::access_code::access_code_bits;
        use crate::signal::dsp::testkit::{at_snr, Rng};
        use num_complex::Complex;

        const RAW_RATE: f64 = 20_000_000.0;
        let ch = 45u8;
        let channel_hz = crate::signal::bt::channel::centre_hz(ch).unwrap();
        let lap = 0x0044_5566;
        let mut bits: Vec<bool> = (0..40).map(|i| i % 2 == 0).collect();
        bits.extend(access_code_bits(lap));
        bits.extend((0..40).map(|i| i % 2 == 1));
        let sps = (RAW_RATE / 1_000_000.0) as usize;
        let clean = modulate(&bits, sps, 160_000.0, RAW_RATE, 0.5);
        let placed = at_snr(&clean, 40.0, &mut Rng::new(7));
        let geometry = eight_bit();
        let bytes: Vec<u8> = placed
            .iter()
            .flat_map(|s: &Complex<f32>| {
                let re = (s.re * geometry.full_scale).clamp(-127.0, 127.0) as i8;
                let im = (s.im * geometry.full_scale).clamp(-127.0, 127.0) as i8;
                [re as u8, im as u8]
            })
            .collect();

        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = crate::signal::net::SECTION.to_string();
        m.ui.active_preset = super::super::lock::SURVEY_VIEW.to_string();
        m.radio.frequency = channel_hz;
        m.radio.config_sample_rate = RAW_RATE;
        m.radio.bb_filter_hz = 0;
        let state = Arc::new(Mutex::new(m));
        let (tx, rx) = crossbeam_channel::unbounded();
        tx.send(stamped(&state, 1, false, bytes)).unwrap();
        drop(tx);
        let mut worker = NetWorker::new(rx, Arc::clone(&state), geometry, SAFE_BT_CHANNELS);
        worker.survey_bt_start = 1;
        worker.run();

        let m = state.lock().unwrap();
        assert!(m.net.bt_refused.is_none(), "{:?}", m.net.bt_refused);
        assert_eq!(m.net.bt_channels_watched, vec![ch]);
        assert!(m.net.bt_load_limited);
        assert_eq!(m.net.bt_hops.len(), 1, "{:?}", m.net.bt_hops);
        assert_eq!(m.net.bt_hops[0].lap, lap);
    }

    /// An access code with nothing after it on channel 45 of a 20 Msps
    /// capture tuned to it: the device bytes and the tuning.
    fn access_code_only(lap: u32) -> (Vec<u8>, u64) {
        use crate::signal::ble::gfsk::modulate;
        use crate::signal::bt::access_code::access_code_bits;
        use crate::signal::dsp::testkit::{at_snr, Rng};
        use num_complex::Complex;
        const RAW_RATE: f64 = 20_000_000.0;
        let channel_hz = crate::signal::bt::channel::centre_hz(45).unwrap();
        let mut bits: Vec<bool> = (0..40).map(|i| i % 2 == 0).collect();
        bits.extend(access_code_bits(lap));
        bits.extend((0..40).map(|i| i % 2 == 1));
        let clean = modulate(&bits, 20, 160_000.0, RAW_RATE, 0.5);
        let placed = at_snr(&clean, 40.0, &mut Rng::new(7));
        let geometry = eight_bit();
        let bytes = placed
            .iter()
            .flat_map(|s: &Complex<f32>| {
                let re = (s.re * geometry.full_scale).clamp(-127.0, 127.0) as i8;
                let im = (s.im * geometry.full_scale).clamp(-127.0, 127.0) as i8;
                [re as u8, im as u8]
            })
            .collect();
        (bytes, channel_hz)
    }

    /// Run one capture through the worker on `preset`, tuned to `tuned`.
    fn run_classic(preset: &str, tuned: u64, bytes: Vec<u8>) -> Arc<Mutex<SdrMetrics>> {
        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = crate::signal::net::SECTION.to_string();
        m.ui.active_preset = preset.to_string();
        m.radio.frequency = tuned;
        m.radio.config_sample_rate = 20_000_000.0;
        m.radio.bb_filter_hz = 0;
        let state = Arc::new(Mutex::new(m));
        let (tx, rx) = crossbeam_channel::unbounded();
        tx.send(stamped(&state, 1, false, bytes)).unwrap();
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), eight_bit(), SAFE_BT_CHANNELS).run();
        state
    }

    /// The Piconet view listens as the Classic view does: without it in the
    /// gate, the view would be empty forever.
    #[test]
    fn the_piconet_view_runs_the_classic_receiver() {
        let (bytes, tuned) = access_code_only(0x0044_5566);
        let state = run_classic("net_piconet", tuned, bytes);
        let m = state.lock().unwrap();
        assert!(m.net.bt_refused.is_none(), "{:?}", m.net.bt_refused);
        assert_eq!(m.net.bt_channels_watched.len(), SAFE_BT_CHANNELS);
        assert_eq!(m.net.bt_hops.len(), 1);
    }

    /// The Bench view listens too: it reads the same receiver's packets.
    #[test]
    fn the_bench_view_runs_the_classic_receiver() {
        let (bytes, tuned) = access_code_only(0x0044_5566);
        let state = run_classic("net_bench", tuned, bytes);
        let m = state.lock().unwrap();
        assert!(m.net.bt_refused.is_none(), "{:?}", m.net.bt_refused);
        assert_eq!(m.net.bt_hops.len(), 1);
    }

    /// A header read at one clock is a packet with its direction, which is
    /// that clock's slot parity, and its payload's verdict: this DH1's CRC
    /// checks out.
    #[test]
    fn a_header_read_at_one_clock_is_a_packet_with_its_direction() {
        use crate::signal::bt::piconet::{Direction, HeaderRead, PayloadVerdict};
        let lap = 0x0033_2211u32;
        let (bytes, tuned) = dh1_capture(lap, 0x7b);
        let state = run_classic("net_bt", tuned, bytes);
        let m = state.lock().unwrap();
        let p = m.net.bt_piconets.iter().find(|p| p.lap == lap).unwrap();
        assert_eq!(p.packets.len(), 1, "{:?}", p.packets);
        let packet = &p.packets[0];
        let Some(HeaderRead::Decoded(h)) = packet.header else {
            panic!("{:?}", packet.header);
        };
        assert_eq!(packet.direction, Some(Direction::of_clk6(h.clk6)));
        assert_eq!(packet.payload, PayloadVerdict::Crc(true));
        assert!(packet.deviation.settled.n > 0, "its own readings kept");
        // Its own f0, with the spread the pooled sums cannot give back, and
        // the same figure the sums hold.
        assert_eq!(packet.carrier.f0_ppm.n, 1);
        let f0 = packet.f0_ppm.expect("its own f0");
        assert!(
            (f0.value() - packet.carrier.f0_ppm.sum).abs() < 1e-3,
            "{f0:?}"
        );
        assert!(f0.sigma() > 0.0 && f0.sigma().is_finite(), "{f0:?}");
        let side = p.headers.sides.clone().of(packet.direction).packets;
        assert_eq!(side, 1, "counted on its side, not under unknown");
        assert_eq!(p.headers.sides.unknown.packets, 0);
    }

    /// An access code with no header after it is an ID row: no header, no
    /// payload, no direction.
    #[test]
    fn a_hit_with_no_header_is_an_id_row() {
        use crate::signal::bt::piconet::PayloadVerdict;
        let lap = 0x0044_5566;
        let (bytes, tuned) = access_code_only(lap);
        let state = run_classic("net_bt", tuned, bytes);
        let m = state.lock().unwrap();
        let p = m.net.bt_piconets.iter().find(|p| p.lap == lap).unwrap();
        assert_eq!(p.packets.len(), 1);
        let packet = &p.packets[0];
        assert_eq!(packet.header, None);
        assert_eq!(packet.payload, PayloadVerdict::NoPayload);
        assert_eq!(packet.direction, None);
        assert_eq!(p.headers.sides.unknown.packets, 1);
    }

    /// A classic packet on channel 45 of a 20 Msps capture tuned to it:
    /// `lap`'s access code, a real DH1 header whose HEC is `true_uap`'s, and
    /// a real DH1 payload with its own CRC-16. Returns the device bytes and
    /// the tuning.
    fn dh1_capture(lap: u32, true_uap: u8) -> (Vec<u8>, u64) {
        use crate::signal::ble::gfsk::modulate;
        use crate::signal::bt::access_code::access_code_bits;
        use crate::signal::bt::header;
        use crate::signal::bt::payload;
        use crate::signal::dsp::testkit::{at_snr, Rng};
        use num_complex::Complex;

        const RAW_RATE: f64 = 20_000_000.0;
        let ch = 45u8;
        let channel_hz = crate::signal::bt::channel::centre_hz(ch).unwrap();
        let clk6 = 22u8;

        let lt_addr = 0b010u8;
        let flags = 0b110u8;
        let type_bits = 0b0100u8; // DH1
        let data10 = (lt_addr as u16) | ((type_bits as u16) << 3) | ((flags as u16) << 7);
        let hec = (0u16..256)
            .map(|h| h as u8)
            .find(|&h| header::uap_from_hec(data10, h) == true_uap)
            .expect("uap_from_hec is a bijection in hec for a fixed data10");
        let mut header_host = [false; header::HEADER_BITS];
        for (i, slot) in header_host[0..10].iter_mut().enumerate() {
            *slot = (data10 >> i) & 1 != 0;
        }
        for (i, slot) in header_host[10..18].iter_mut().enumerate() {
            *slot = (hec >> i) & 1 != 0;
        }
        let whitened_header = header::unwhiten_header(&header_host, clk6);

        let body = [0x44u8, 0x55, 0x66];
        let mut payload_host = vec![false, true, false]; // LLID, FLOW
        for i in 0..5 {
            payload_host.push((body.len() as u8 >> i) & 1 != 0); // LENGTH
        }
        for &byte in &body {
            for i in 0..8 {
                payload_host.push((byte >> i) & 1 != 0);
            }
        }
        let crc = payload::crcgen(&payload_host, true_uap);
        for i in 0..16 {
            payload_host.push((crc >> i) & 1 != 0);
        }
        let payload_whitened = header::unwhiten_at(&payload_host, clk6, header::HEADER_BITS);
        // A generous, arbitrary window past the real payload's own end -
        // this worker's own receiver always captures a fixed maximum
        // regardless of the real payload's shorter length
        // (`signal::bt::receive::PAYLOAD_CAPTURE_BITS`'s own doc).
        const PAYLOAD_CAPTURE_BITS: usize = 343 * 8;

        let mut bits: Vec<bool> = (0..40).map(|i| i % 2 == 0).collect();
        bits.extend(access_code_bits(lap));
        bits.extend([true, false, true, false]); // 4-bit trailer, content unread
        for &b in &whitened_header {
            bits.extend([b, b, b]); // FEC(1/3): each bit sent three times
        }
        bits.extend(payload_whitened.iter().copied());
        bits.extend((0..(PAYLOAD_CAPTURE_BITS - payload_whitened.len())).map(|i| i % 2 == 1));
        bits.extend((0..40).map(|i| i % 2 == 0));

        let sps = (RAW_RATE / 1_000_000.0) as usize;
        let clean = modulate(&bits, sps, 160_000.0, RAW_RATE, 0.5);
        let placed = at_snr(&clean, 40.0, &mut Rng::new(11));

        let geometry = eight_bit();
        let bytes: Vec<u8> = placed
            .iter()
            .flat_map(|s: &Complex<f32>| {
                let re = (s.re * geometry.full_scale).clamp(-127.0, 127.0) as i8;
                let im = (s.im * geometry.full_scale).clamp(-127.0, 127.0) as i8;
                [re as u8, im as u8]
            })
            .collect();

        (bytes, channel_hz)
    }

    /// Through the actual worker: a synthetic
    /// classic BT packet carrying a real DH1 header and a real DH1
    /// payload (its own genuine CRC-16) resolves `net.bt_uap` to exactly
    /// one confirmed UAP - not the two-candidate floor a header alone
    /// reaches - the same day `signal::bt::payload::break_uap_tie` proved
    /// it could, wired here instead of left standing untested.
    #[test]
    fn a_real_dh1_payload_resolves_bt_uap_to_one_confirmed_value() {
        const RAW_RATE: f64 = 20_000_000.0;
        let lap = 0x0033_2211u32;
        let true_uap = 0x7bu8;
        use crate::signal::bt::header;
        let (bytes, channel_hz) = dh1_capture(lap, true_uap);
        let geometry = eight_bit();
        // What `dh1_capture` puts in the header.
        let lt_addr = 0b010u8;

        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = crate::signal::net::SECTION.to_string();
        m.ui.active_preset = "net_bt".to_string();
        m.radio.frequency = channel_hz;
        m.radio.config_sample_rate = RAW_RATE;
        m.radio.bb_filter_hz = 0;
        let state = Arc::new(Mutex::new(m));

        let (tx, rx) = crossbeam_channel::unbounded();
        tx.send(stamped(&state, 1, false, bytes)).unwrap();
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), geometry, SAFE_BT_CHANNELS).run();

        let m = state.lock().unwrap();
        let narrowed = m
            .net
            .bt_uap
            .get(&lap)
            .expect("this LAP's header should have arrived");
        assert_eq!(narrowed, &vec![true_uap], "{narrowed:?}");

        // Resolved by this very payload, the same header is then read under the
        // UAP: a DH1 from LT_ADDR 2, counted on the roster.
        let p = m
            .net
            .bt_piconets
            .iter()
            .find(|p| p.lap == lap)
            .expect("the access code made a row");
        let h = &p.headers;
        assert_eq!((h.captured, h.decoded, h.undecoded), (1, 1, 0), "{h:?}");
        assert_eq!(h.types[header::PacketType::Dh1.code() as usize], 1);
        assert_eq!(h.lt_addrs, 1 << lt_addr);

        // The header's own symbols give the modulation index the
        // modulator was set to: 160 kHz at 1 Msym/s is h = 0.32. Read
        // against the slicer's fast tracker it came out 0.297 (149 kHz); from
        // the header's own settled centre, 0.322 (160.9 +/- 0.8 kHz).
        let df1 = h
            .deviation
            .settled
            .mean()
            .expect("settled runs in a header");
        let index = df1.value() * 2.0 / 1e6;
        assert!((index - 0.32).abs() < 0.01, "h = {index} from {df1:?}");

        // The header joins its own hit, so the export can say what the hit
        // carried.
        let hop = m
            .net
            .bt_hops
            .iter()
            .find(|h| h.lap == lap)
            .expect("the hit");
        assert!(
            matches!(
                hop.header,
                Some(crate::signal::bt::piconet::HeaderRead::Decoded(h))
                    if h.packet_type == header::PacketType::Dh1
            ),
            "{hop:?}"
        );
    }

    /// An inquiry code is counted and placed, and nothing more: the same
    /// capture that resolves an ordinary LAP's UAP and reads its header
    /// leaves the GIAC with no UAP, no header read and no slot fit, since
    /// every searching device sends it and none of those would be one
    /// device's.
    #[test]
    fn an_inquiry_code_is_counted_but_never_narrowed_or_fitted() {
        const RAW_RATE: f64 = 20_000_000.0;
        let giac = 0x9E_8B33;
        let (bytes, channel_hz) = dh1_capture(giac, 0x7b);
        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = crate::signal::net::SECTION.to_string();
        m.ui.active_preset = "net_bt".to_string();
        m.radio.frequency = channel_hz;
        m.radio.config_sample_rate = RAW_RATE;
        m.radio.bb_filter_hz = 0;
        let state = Arc::new(Mutex::new(m));
        let (tx, rx) = crossbeam_channel::unbounded();
        tx.send(stamped(&state, 1, false, bytes)).unwrap();
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), eight_bit(), SAFE_BT_CHANNELS).run();

        let m = state.lock().unwrap();
        let p = m
            .net
            .bt_piconets
            .iter()
            .find(|p| p.lap == giac)
            .expect("the hit is still counted");
        assert_eq!(p.hits, 1);
        assert!(!m.net.bt_uap.contains_key(&giac), "{:?}", m.net.bt_uap);
        assert_eq!(p.headers.captured, 0);
        assert!(p.slots.is_none());
        // Its pace is still read: one hit, no spacing yet.
        assert_eq!(p.pace, crate::signal::bt::slots::Pace::default());
        assert!(m.net.bt_hops.iter().all(|h| h.header.is_none()));
    }

    /// **Slot timing end to end** (6.5): twelve access codes of one LAP,
    /// each on a slot of a piconet whose slots run 30 ppm long on our clock,
    /// across equal blocks of one continuous stream. The worker dates them
    /// on the stream's clock and fits a grid whose leftover is well inside
    /// the specification's 1 µs.
    #[test]
    fn a_piconets_access_codes_give_its_slot_grid() {
        use crate::signal::ble::gfsk::modulate;
        use crate::signal::bt::access_code::access_code_bits;
        use crate::signal::dsp::testkit::Rng;
        use num_complex::Complex;

        const RAW_RATE: f64 = 4_000_000.0;
        const BLOCK: usize = 10_000; // 2.5 ms: one access code a block at most
        let ch = 45u8;
        let channel_hz = crate::signal::bt::channel::centre_hz(ch).unwrap();
        let lap = 0x0012_3456u32;
        let slot_samples = 625e-6 * (1.0 + 30e-6) * RAW_RATE;
        let slots = [0u32, 5, 11, 16, 22, 28, 33, 40, 46, 51, 57, 63];

        let mut bits: Vec<bool> = (0..40).map(|i| i % 2 == 0).collect();
        bits.extend(access_code_bits(lap));
        bits.extend((0..40).map(|i| i % 2 == 1));
        let burst = modulate(&bits, 4, 160_000.0, RAW_RATE, 0.5);
        let total = ((*slots.last().unwrap() as f64 + 4.0) * slot_samples) as usize;
        let total = total.div_ceil(BLOCK) * BLOCK;
        let mut rng = Rng::new(9);
        let mut iq: Vec<Complex<f32>> = rng.noise(total, 1e-4);
        for &k in &slots {
            let at = (5_000.0 + k as f64 * slot_samples).round() as usize;
            for (i, s) in burst.iter().enumerate() {
                iq[at + i] += s;
            }
        }
        let geometry = eight_bit();
        let bytes: Vec<u8> = iq
            .iter()
            .flat_map(|s| {
                let re = (s.re * geometry.full_scale).clamp(-127.0, 127.0) as i8;
                let im = (s.im * geometry.full_scale).clamp(-127.0, 127.0) as i8;
                [re as u8, im as u8]
            })
            .collect();

        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = crate::signal::net::SECTION.to_string();
        m.ui.active_preset = "net_bt".to_string();
        m.radio.frequency = channel_hz;
        m.radio.config_sample_rate = RAW_RATE;
        m.radio.bb_filter_hz = 0;
        let state = Arc::new(Mutex::new(m));
        let (tx, rx) = crossbeam_channel::unbounded();
        for (i, chunk) in bytes.chunks(BLOCK * 2).enumerate() {
            tx.send(stamped(&state, i as u64 + 1, false, chunk.to_vec()))
                .unwrap();
        }
        drop(tx);
        let mut worker = NetWorker::new(rx, Arc::clone(&state), geometry, SAFE_BT_CHANNELS);
        worker.slot_fit_every = std::time::Duration::ZERO;
        worker.run();

        let m = state.lock().unwrap();
        let p = m
            .net
            .bt_piconets
            .iter()
            .find(|p| p.lap == lap)
            .expect("a row");
        assert_eq!(p.hits, slots.len() as u64);
        let fit = p.slots.as_ref().expect("fitted").as_ref().expect("a grid");
        assert_eq!(fit.hits, slots.len());
        // Measured when written: 27.1 ppm (40 ms is short to resolve a
        // rate), rms 0.15 +/- 0.03 us, max 0.21 us - our own quarter-symbol
        // dating and the rounding of each burst to a sample, nothing more.
        assert!((fit.rate_ppm - 30.0).abs() < 5.0, "{fit:?}");
        assert!(fit.rms_us.value() < 0.3, "{fit:?}");
        assert!(fit.max_us < 0.5, "{fit:?}");

        // Every one of those hits exports its residual from that grid.
        let rows = crate::export::bt::rows(&m);
        let col = crate::export::bt::HEADER
            .split(',')
            .position(|h| h == "slot_residual_us")
            .unwrap();
        assert_eq!(rows.len(), slots.len());
        for row in &rows {
            let r: f64 = row.split(',').nth(col).unwrap().parse().expect(row);
            assert!(r.abs() < 0.5, "{row}");
        }
    }

    /// Any other preset's blocks leave `bt_refused` unset, so a stale
    /// refusal from a previous `net_bt` visit does not linger onto a screen
    /// that never claimed to be that preset.
    #[test]
    fn a_different_preset_leaves_bt_refused_unset() {
        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = crate::signal::net::SECTION.to_string();
        m.ui.active_preset = "net_ble".to_string();
        m.radio.frequency = 2_402_000_000;
        m.radio.config_sample_rate = 4_000_000.0;
        let state = Arc::new(Mutex::new(m));
        let (tx, rx) = crossbeam_channel::unbounded();
        tx.send(stamped(&state, 1, false, vec![0u8; 256])).unwrap();
        drop(tx);
        NetWorker::new(rx, Arc::clone(&state), eight_bit(), SAFE_BT_CHANNELS).run();
        let m = state.lock().unwrap();
        assert!(m.net.bt_refused.is_none(), "{:?}", m.net.bt_refused);
    }
}
