// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! How long a backend's tuning call takes: from just before
//! [`SdrDevice::set_frequency`] is called to the moment it returns, timed over
//! several real retunes.
//!
//! **That, and only that, is what is measured, and the name says so.** B19 first
//! built this as "retune latency": set the frequency, then wait for the next
//! block to arrive, and call the wait the time to a block "reflecting the new
//! frequency". Nothing checked that it did. The next block is as likely to
//! carry samples captured before the retune and still queued, and the wait was
//! dominated by where the stream happened to be in its current block: a HackRF
//! block is 131,072 samples, 6.55 ms at 20 Msps and 32.8 ms at 4 Msps. The
//! figure would have been mostly block phase, labelled as the radio's retune
//! time, right where it decides whether a connection can be followed. Replaced
//! 2026-09-19 before anything had shown it (POLICY rule 5: the label names what
//! was measured).
//!
//! **What the call time does and does not tell you.** It is the host-to-radio
//! part: the driver, the USB control transfer, whatever the firmware does before
//! it answers. It does not include the synthesiser settling after the answer, if
//! the firmware does not wait for lock, and it says nothing about samples
//! already in flight. So it bounds from one side only: a call that alone takes
//! longer than a 7.5 ms BLE connection interval rules following out; a call
//! that is shorter is necessary, not sufficient. When the radio actually changes
//! frequency in the sample stream is a different measurement, and a research
//! step of its own (`dev_docs/net-ux-polish-plan.md`, after the plan).
//!
//! No streaming is needed and none is assumed: the call is timed whether or not
//! blocks are flowing.

use std::time::Instant;

use crate::hardware::SdrDevice;
use crate::signal::dsp::uncertainty::{mean_with_uncertainty, Uncertain};

/// The shortest interval a BLE connection may hop at: Core 5.4 Vol 6 Part B,
/// the connection state (4.5): "The connInterval shall be a multiple of 1.25 ms
/// in the range 7.5 ms to 4.0 s." A follower has to be on the next channel
/// within it, so a tuning call longer than this rules following out.
pub const MIN_CONNECTION_INTERVAL_MS: f64 = 7.5;

/// Where a measurement retunes to, in order: across the 2.4 GHz band and back,
/// so the synthesiser moves by 78 MHz, 54 MHz and smaller steps rather than
/// between two neighbours. Ten calls, the BLE advertising channels among them.
pub const BAND_HOPS_HZ: [u64; 10] = [
    2_402_000_000,
    2_480_000_000,
    2_426_000_000,
    2_440_000_000,
    2_402_000_000,
    2_480_000_000,
    2_412_000_000,
    2_462_000_000,
    2_426_000_000,
    2_480_000_000,
];

/// The tuning call's duration, over several calls.
#[derive(Debug, Clone, PartialEq)]
pub struct CallMeasurement {
    /// Milliseconds, mean and its standard uncertainty, over the calls that
    /// succeeded. Unresolved (infinite variance) with fewer than two, the
    /// shape every reading in the app takes when it has nothing to stand on.
    pub call_ms: Uncertain,
    /// The slowest successful call, because a follower has to survive the
    /// worst one, not the average. `None` when no call succeeded.
    pub worst_ms: Option<f64>,
    pub attempts: usize,
    /// Calls the backend refused, with the first refusal's own words.
    pub failed: usize,
    pub first_error: Option<String>,
}

/// Time `set_frequency` once per entry of `frequencies`, in order.
///
/// `tuned` is called after each successful call, **outside the timed span**,
/// with the frequency the radio is now on, so a caller can keep its own record
/// of the tuning in step (the RX pipeline stamps every block with it) without
/// that bookkeeping landing in the figure.
pub fn measure_calls(
    device: &dyn SdrDevice,
    frequencies: &[u64],
    mut tuned: impl FnMut(u64),
) -> CallMeasurement {
    let mut calls_ms = Vec::with_capacity(frequencies.len());
    let mut failed = 0;
    let mut first_error = None;
    for &hz in frequencies {
        let start = Instant::now();
        let result = device.set_frequency(hz);
        let elapsed = start.elapsed();
        match result {
            Ok(()) => {
                calls_ms.push(elapsed.as_secs_f32() * 1000.0);
                tuned(hz);
            }
            Err(e) => {
                failed += 1;
                first_error.get_or_insert_with(|| e.to_string());
            }
        }
    }
    CallMeasurement {
        call_ms: mean_with_uncertainty(&calls_ms),
        worst_ms: calls_ms
            .iter()
            .copied()
            .fold(None, |w: Option<f32>, x| Some(w.map_or(x, |w| w.max(x))))
            .map(f64::from),
        attempts: frequencies.len(),
        failed,
        first_error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::{DeviceCapabilities, DeviceInfo, RxContext};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    /// A device whose tuning call takes a known time, and refuses the
    /// frequencies it is told to.
    struct FakeDevice {
        caps: DeviceCapabilities,
        delay: Duration,
        refuse: Vec<u64>,
    }

    impl SdrDevice for FakeDevice {
        fn capabilities(&self) -> &DeviceCapabilities {
            &self.caps
        }
        fn info(&self) -> DeviceInfo {
            DeviceInfo::default()
        }
        fn start_rx(&self, _ctx: Arc<RxContext>) -> anyhow::Result<()> {
            Ok(())
        }
        fn stop_rx(&self) -> anyhow::Result<()> {
            Ok(())
        }
        fn is_streaming(&self) -> bool {
            false
        }
        fn set_frequency(&self, hz: u64) -> anyhow::Result<()> {
            if self.refuse.contains(&hz) {
                anyhow::bail!("injected refusal at {hz}");
            }
            std::thread::sleep(self.delay);
            Ok(())
        }
        fn set_sample_rate(&self, hz: f64) -> anyhow::Result<crate::hardware::RateSet> {
            Ok(crate::hardware::RateSet::new(hz, Some(hz), 0))
        }
        fn set_lna_gain(&self, _db: u32) -> anyhow::Result<()> {
            Ok(())
        }
    }

    fn device(delay_ms: u64, refuse: &[u64]) -> FakeDevice {
        FakeDevice {
            caps: crate::hardware::native::hackrf::caps(),
            delay: Duration::from_millis(delay_ms),
            refuse: refuse.to_vec(),
        }
    }

    /// The figure is the call's own duration: close to the fake's known delay,
    /// never below it, and the worst call is at least the mean.
    #[test]
    fn the_call_time_is_the_calls_own_duration() {
        let got = measure_calls(&device(5, &[]), &BAND_HOPS_HZ, |_| {});
        assert_eq!(got.attempts, 10);
        assert_eq!(got.failed, 0);
        let ms = got.call_ms.value();
        assert!((5.0..30.0).contains(&ms), "{ms}");
        assert!(got.call_ms.sigma().is_finite());
        assert!(got.worst_ms.unwrap() >= ms);
    }

    /// The caller's bookkeeping runs once per successful call, with the
    /// frequency just set, and its cost is not in the figure: a slow callback
    /// on a fast device still reads fast.
    #[test]
    fn the_callback_follows_the_tuning_and_stays_out_of_the_figure() {
        let seen = Mutex::new(Vec::new());
        let got = measure_calls(&device(0, &[2_480_000_000]), &BAND_HOPS_HZ, |hz| {
            seen.lock().unwrap().push(hz);
            std::thread::sleep(Duration::from_millis(20));
        });
        let seen = seen.into_inner().unwrap();
        assert_eq!(seen.len(), 7, "three calls to 2480 MHz were refused");
        assert!(!seen.contains(&2_480_000_000));
        assert!(got.call_ms.value() < 10.0, "{:?}", got.call_ms);
        assert_eq!(got.failed, 3);
        assert!(got.first_error.unwrap().contains("2480000000"));
    }

    /// Every call refused: nothing to average, and it says so rather than
    /// reporting a zero.
    #[test]
    fn a_backend_that_refuses_every_call_has_no_figure() {
        let got = measure_calls(&device(0, &BAND_HOPS_HZ), &BAND_HOPS_HZ, |_| {});
        assert_eq!(got.failed, 10);
        assert_eq!(got.worst_ms, None);
        assert!(!got.call_ms.sigma().is_finite(), "{:?}", got.call_ms);
    }
}
