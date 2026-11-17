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

use crate::hardware::{self, SampleGeometry, SdrDevice};
use crate::signal::{DemodWorker, FftWorker, NetWorker, PowerWorker};
use crate::state::SdrMetrics;
use crate::tasks;

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
