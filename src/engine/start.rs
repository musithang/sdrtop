// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Starting what measures: the sample feeds off the radio's thread, the
//! workers at their other ends, and the tasks that poll, survey and sweep.
//!
//! Split in two around the moment a front end takes the engine: [`feeds`]
//! makes the queues and the context the radio's thread delivers into, which
//! a front end needs to hold (the recorder taps it), and [`run`] starts the
//! threads that read them. Nothing here knows whether anything is watching.

use std::sync::{Arc, Mutex};

use super::boot::{initial_metrics, resolve_tuning, Boot};
use crate::config::AppConfig;
use crate::hardware::{self, SampleGeometry, SdrDevice};
use crate::signal::{DemodWorker, FftWorker, NetWorker, PowerWorker};
use crate::state::SdrMetrics;
use crate::tasks;

/// Program an open radio from the config (rate, frequency, every gain stage,
/// the boost) and build the state a session starts from, with the log saying
/// who the radio is and anything the config asked for and did not get.
///
/// `observable` is whether observer mode could watch this radio were another
/// process to hold it.
pub fn open(
    cfg: &AppConfig,
    device: &Arc<dyn SdrDevice>,
    observable: bool,
) -> anyhow::Result<SdrMetrics> {
    let info = device.info();
    let caps = Arc::new(device.capabilities().clone());

    let mut tuning = resolve_tuning(&cfg.radio, &caps);
    // Taken out before `tuning` is moved into `Boot`, rather than cloning
    // the whole thing to keep them.
    let mut boot_notes = std::mem::take(&mut tuning.notes);
    let asked_rate = tuning.sample_rate;
    let sr_result = match device.set_sample_rate(asked_rate) {
        // The device is the authority on both figures: the baseband width it
        // actually selected, and the rate it actually landed on. The computed
        // width and the requested rate stand in only when the call failed.
        Ok(set) => {
            tuning.sample_rate = set.rate_hz;
            tuning.bb_filter_hz = set.bb_filter_hz;
            // A driver that rounded the configured rate onto its own grid
            // would otherwise change it silently, and the config is rewritten
            // on quit, so the user's own figure would disappear without ever
            // having been contradicted.
            if set.rate_hz != asked_rate {
                boot_notes.push(format!(
                    "Sample rate {:.6} MHz is not on this radio's grid; running at {:.6} MHz",
                    asked_rate / 1e6,
                    set.rate_hz / 1e6
                ));
            }
            Ok(())
        }
        Err(e) => Err(e),
    };
    // `amp_enabled` is the front-end-boost state for both device families:
    // HackRF's RF amp (set_amp_enable) and RTL-SDR's tuner AGC (set_tuner_agc).
    // Calling both applies the right one per device (the other is a no-op) so
    // the programmed state matches what the UI shows.
    // One call per stage the device actually has, in its own order. The two
    // named setters are what the default `set_stage_gain` maps onto, so both
    // native radios are programmed exactly as they always were.
    let stages = caps.gain.stages();
    // The radio says where it tuned, which is not always where it was asked
    // (a receiver on a tuning grid), and the state starts from its word.
    let freq_result = device.set_frequency(tuning.frequency_hz).map(|tuned| {
        tuning.frequency_hz = tuned;
    });
    let mut startup_results = vec![freq_result, sr_result];
    let gains: Vec<f64> = stages
        .iter()
        .enumerate()
        .map(|(index, spec)| tuning.gains.get(index).copied().unwrap_or(spec.min_db))
        .collect();
    startup_results.push(device.set_stage_gains(&gains));
    let boost = cfg.radio.boost_for(&info.board_name);
    startup_results.push(device.set_amp_enable(boost));
    startup_results.push(device.set_tuner_agc(boost));

    let device_options = device.options();
    hardware::debug_assert_device_options(&device_options);
    let mut initial = initial_metrics(
        cfg,
        Boot::normal(cfg, Arc::clone(&caps), tuning, &info, observable),
    )?;
    initial.device_options = Arc::new(device_options);
    {
        let m = &mut initial;
        // Read the identity back out of the state rather than off `info`
        // again: `Boot::normal` already applied the "unknown" / `0xFE`
        // fall-backs, and the log must say the same thing the header does.
        let connected = format!(
            "Connected: {} | Serial: {}",
            m.system.board_name, m.system.serial
        );
        let board = format!(
            "Board: {} | USB API: {:#06x}",
            hardware::native::hackrf::board_rev_name(m.system.board_rev),
            m.system.usb_api_version
        );
        m.push_log(connected);
        // Firmware is a HackRF concept; RTL-SDR (no on-device FW) skips it.
        if let Some(fw) = &info.fw_version {
            m.push_log(format!("Firmware: {}", fw));
        }
        // RTL-SDR reports a tuner instead of a board revision / USB-API version.
        if let Some(tuner) = &info.tuner_name {
            m.push_log(format!("Tuner: {}", tuner));
        } else if caps.acquisition == hardware::AcquisitionKind::IqSamples {
            m.push_log(board);
        }
        // Anything the backend declined while opening. Both native paths
        // have nothing to say; a SoapySDR device names any gain element
        // whose range the driver described unusably.
        for note in device.open_notes() {
            m.push_log(note.clone());
        }
        // Anything the configured tuning asked for and did not get: a gain
        // stage name this radio does not have, an entry that is not
        // `NAME=value` at all, or a sample rate the driver rounded. The gain
        // ones are computed by `resolve_tuning`, which is pure, and surfaced
        // here.
        for note in &boot_notes {
            m.push_log(note.clone());
        }
        let names = [
            "frequency",
            "sample rate",
            "LNA gain",
            "VGA gain",
            "amp",
            "tuner AGC",
        ];
        for (result, name) in startup_results.iter().zip(names.iter()) {
            if let Err(e) = result {
                m.push_log(format!("Startup: failed to set {}: {}", name, e));
            }
        }
    }
    Ok(initial)
}

/// The far ends of the sample feeds, for [`run`] to hand to the workers.
pub struct Feeds {
    sample_rx: crossbeam_channel::Receiver<Vec<u8>>,
    demod_rx: crossbeam_channel::Receiver<hardware::StreamBlock>,
    net_rx: crossbeam_channel::Receiver<hardware::StreamBlock>,
    pager_rx: crossbeam_channel::Receiver<hardware::StreamBlock>,
    ft8_rx: crossbeam_channel::Receiver<hardware::TimedBlock>,
    power_rx: crossbeam_channel::Receiver<hardware::PowerTrace>,
}

/// The queues, each with the depth its consumer needs, and the context the
/// radio's thread delivers into.
pub fn feeds(
    state: &Arc<Mutex<SdrMetrics>>,
    geometry: SampleGeometry,
) -> (Arc<hardware::RxContext>, Feeds) {
    let (sample_tx, sample_rx) = crossbeam_channel::bounded::<Vec<u8>>(4);
    // The demod queue is deliberately shallow: it duty-cycles to one update
    // per 250 ms, so anything deeper would only hold blocks it will discard.
    let (demod_tx, demod_rx) = crossbeam_channel::bounded::<crate::hardware::StreamBlock>(2);
    // The NET queue is deeper than the demod's and for the opposite reason.
    // The demod duty-cycles to four updates a second, so anything deeper
    // than two would only hold blocks it will discard. This worker is meant
    // to see every block, and four is the same depth the FFT feed uses: a
    // burst that spans several driver blocks survives a hiccup on the UI
    // thread, and anything deeper starts hiding the losses rather than
    // absorbing them.
    let (net_tx, net_rx) = crossbeam_channel::bounded::<crate::hardware::StreamBlock>(4);
    // The pager queue is the NET queue's depth for the NET queue's
    // reason: every block is wanted, and a deeper queue would hide losses.
    let (pager_tx, pager_rx) = crossbeam_channel::bounded::<crate::hardware::StreamBlock>(4);
    // The FT8 queue is deep, for its own reason: nothing there needs a
    // block soon, and a stall of the machine must not cost a cycle.
    let (ft8_tx, ft8_rx) = crossbeam_channel::bounded::<crate::hardware::TimedBlock>(
        crate::signal::ft8::worker::FEED_BLOCKS,
    );
    let (power_tx, power_rx) = crossbeam_channel::bounded::<hardware::PowerTrace>(4);
    let rx_ctx = Arc::new(hardware::RxContext {
        metrics: Arc::clone(state),
        sample_tx,
        fft_feed: hardware::FeedHealth::default(),
        demod_tx,
        net_tx,
        net_feed: hardware::FeedHealth::default(),
        pager_tx,
        pager_feed: hardware::FeedHealth::default(),
        ft8_tx,
        ft8_feed: hardware::FeedHealth::default(),
        power_tx,
        geometry,
        stream_pairs: std::sync::atomic::AtomicU64::new(0),
        streams_begun: std::sync::atomic::AtomicU64::new(0),
        record: Default::default(),
        intake: Default::default(),
    });
    (
        rx_ctx,
        Feeds {
            sample_rx,
            demod_rx,
            net_rx,
            pager_rx,
            ft8_rx,
            power_rx,
        },
    )
}

/// Start the workers and the tasks on an open radio.
pub fn run(
    state: &Arc<Mutex<SdrMetrics>>,
    device: &Arc<dyn SdrDevice>,
    rx_ctx: &Arc<hardware::RxContext>,
    feeds: Feeds,
    bt_channels: usize,
) {
    let geometry = rx_ctx.geometry;
    match device.capabilities().acquisition {
        hardware::AcquisitionKind::IqSamples => {
            // Before any stream can start: from the first block, the
            // driver's thread only stamps and hands over.
            hardware::intake::spawn(rx_ctx);
            let fft_state = Arc::clone(state);
            spawn_worker("fft-worker", move || {
                FftWorker::new(feeds.sample_rx, fft_state, geometry).run()
            });

            let demod_state = Arc::clone(state);
            spawn_worker("demod-worker", move || {
                DemodWorker::new(feeds.demod_rx, demod_state, geometry).run()
            });

            // Spawned whether or not the gate admitted the band: while the mode
            // asks for no NET view nothing is forwarded, so the thread costs one
            // blocked `recv`. Deciding here would mean two places that know what admits the
            // feature, and the one that already knows is `net::gate`.
            let net_state = Arc::clone(state);
            spawn_worker("net-worker", move || {
                NetWorker::new(feeds.net_rx, net_state, geometry, bt_channels).run()
            });

            // Spawned always, as the NET worker is: while the mode asks for no
            // pager view nothing is forwarded, and the thread waits on `recv`.
            let pager_state = Arc::clone(state);
            spawn_worker("pager-worker", move || {
                crate::signal::pocsag::worker::PagerWorker::new(
                    feeds.pager_rx,
                    pager_state,
                    geometry,
                )
                .run()
            });

            // Spawned always, as the pager's: while the mode asks for no FT8
            // view nothing is forwarded, and the thread waits on `recv`.
            let ft8_state = Arc::clone(state);
            spawn_worker("ft8-worker", move || {
                crate::signal::ft8::worker::Ft8Worker::new(feeds.ft8_rx, ft8_state, geometry).run()
            });

            tasks::spawn_rx_task(Arc::clone(state), Arc::clone(device), Arc::clone(rx_ctx));
            tasks::spawn_net_survey_task(Arc::clone(state), Arc::clone(device));
        }
        hardware::AcquisitionKind::PowerTrace => {
            let power_state = Arc::clone(state);
            spawn_worker("power-worker", move || {
                PowerWorker::new(feeds.power_rx, power_state).run()
            });
            tasks::spawn_power_rx_task(Arc::clone(state), Arc::clone(device), Arc::clone(rx_ctx));
        }
    }

    tasks::spawn_sweep_task(Arc::clone(state), Arc::clone(device));
    tasks::spawn_sys_resource_task(Arc::clone(state));
}

/// Start what watches a radio another process holds: the USB scan, and the
/// machine's own load. Nothing is fed, because nothing streams.
pub fn observe(state: &Arc<Mutex<SdrMetrics>>, profile: hardware::discovery::ObserverProfile) {
    tasks::spawn_observer_task(Arc::clone(state), profile);
    tasks::spawn_sys_resource_task(Arc::clone(state));
}

/// Spawns one of the long-lived DSP workers under a name, so `top -H` and btop
/// can say which of them is spending the CPU. Linux keeps the first 15 bytes.
/// An unnamed thread shows only the process name, and then the NET, FFT and
/// demod workers cannot be told apart from each other or from the driver's own
/// USB thread. Failing to spawn is as fatal here as in `std::thread::spawn`.
fn spawn_worker(name: &str, run: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .name(name.to_string())
        .spawn(run)
        .unwrap_or_else(|e| panic!("cannot start the {name} thread: {e}"));
}
