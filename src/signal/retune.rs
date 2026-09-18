// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! B19's own retune-latency probe: how long a backend actually takes, from
//! [`crate::hardware::SdrDevice::set_frequency`] returning to a real block
//! reflecting the new frequency - design section 1.3's own "the backend's
//! retune latency [as] a measured quantity rather than an assumption,
//! which is itself a worthwhile thing for this app to know about its own
//! radios." Connection following (B20) is the reason this matters at all:
//! the shortest legal connection interval is 7.5 ms, so a backend whose
//! own retune takes longer than that cannot follow one, and nothing before
//! this module could say which backends that is true of.
//!
//! **What "arrived" means here, and why it needs no new hot-path work
//! beyond one atomic counter.** `RxContext::blocks_seen`'s own doc explains
//! the mechanism: incremented once, unconditionally, by
//! `hardware::process::process_block` - the same per-block granularity
//! every backend's own callback or read thread already runs at, not the
//! 200 ms RX poll, which is far too coarse to resolve a latency this
//! arc's own design section cares about down to single-digit
//! milliseconds. A probe on a thread that is not that callback or read
//! thread only has to watch this one counter for it to move.
//!
//! **Measurement only - nothing here changes how retuning behaves.** B19
//! was explicitly scoped this way: `state::sweep::SWEEP_SETTLING_MS`, the
//! one place in this codebase that already assumes a settling time (a
//! fixed 25 ms, guessed rather than measured) is untouched. Feeding a real
//! measurement back into that assumption is real, physically-tested
//! timing code (`CLAUDE.md`'s own caution about the two native gain
//! paths applies to the same spirit of "obliges a hardware rehearsal
//! before release") and is deliberately a separate, later decision, not
//! bundled into landing the measurement itself.
//!
//! **Primitive only - nothing calls this yet.** No keybinding, no panel
//! row, no automatic background measurement. The same honest scope every
//! large piece of the Bluetooth arc has landed with first (B14's own
//! `access_code`, B16's own `header`, B18's own `coded`) applies here too,
//! for a foundation-level feature rather than a protocol one.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::hardware::SdrDevice;
use crate::signal::dsp::uncertainty::{mean_with_uncertainty, Uncertain};

/// Why one attempt at [`measure_one`] did not produce a duration.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum RetuneError {
    /// `set_frequency` itself refused - the backend's own error message.
    SetFrequency(String),
    /// `set_frequency` succeeded, but [`RxContext::blocks_seen`] never
    /// moved within the given timeout - either this backend is far slower
    /// than expected, or nothing is actually streaming to move it at all
    /// (checked at the call site, not assumed here: this function has no
    /// way to tell "slow" from "not running").
    TimedOut,
}

/// One retune, timed: from just before [`SdrDevice::set_frequency`] is
/// called to the moment `blocks_seen` is next observed to have moved.
///
/// **Polls rather than blocks on a channel, deliberately.** The whole
/// point of `blocks_seen` (see this module's own doc) is that a probe
/// needs no channel, no lock, and no cooperation from the backend's own
/// callback or read thread at all - just a plain atomic load, cheap
/// enough to poll on a short, fixed interval without meaningfully
/// distorting the very latency being measured.
#[allow(dead_code)]
pub fn measure_one(
    device: &dyn SdrDevice,
    blocks_seen: &AtomicU64,
    target_hz: u64,
    timeout: Duration,
    poll_interval: Duration,
) -> Result<Duration, RetuneError> {
    let before = blocks_seen.load(Ordering::Relaxed);
    let start = Instant::now();
    device
        .set_frequency(target_hz)
        .map_err(|e| RetuneError::SetFrequency(e.to_string()))?;
    loop {
        if blocks_seen.load(Ordering::Relaxed) != before {
            return Ok(start.elapsed());
        }
        if start.elapsed() > timeout {
            return Err(RetuneError::TimedOut);
        }
        std::thread::sleep(poll_interval);
    }
}

/// How long [`measure_one`] is allowed to wait for one retune before
/// giving up - generous relative to the 7.5 ms connection interval design
/// section 1.3 cares about, since a backend far slower than that is
/// exactly the honest answer this probe exists to report, not a case to
/// refuse.
#[allow(dead_code)]
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(2);
/// How often [`measure_one`] checks `blocks_seen` while waiting - fine
/// enough not to itself be the dominant source of measured latency at the
/// scale this arc cares about, coarse enough not to spend the probing
/// thread's own time doing nothing else.
#[allow(dead_code)]
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_micros(200);

/// A backend's own retune latency, measured across several real retunes
/// rather than trusted from one - the same reason every other timing
/// figure in this app (B7's CFO, B9's drift) is an [`Uncertain`], not a
/// single point reading.
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub struct RetuneMeasurement {
    /// Milliseconds, from a real sample of successful retunes only - see
    /// [`Self::timed_out`]/[`Self::failed`] for the ones that were not.
    /// [`mean_with_uncertainty`]'s own "fewer than two readings" case
    /// (infinite variance) is exactly what a caller sees if every attempt
    /// failed or timed out, the same honest "unresolved" shape every
    /// other reading in this app uses rather than a fabricated number.
    pub latency_ms: Uncertain,
    pub attempts: usize,
    pub timed_out: usize,
    pub failed: usize,
}

/// Measure retune latency across `frequencies`, one retune per entry, in
/// the order given - a caller building the list decides the hop pattern
/// (alternating between two nearby frequencies is the closest analogue to
/// B20's own connection-following need; a caller measuring something else
/// about a backend might want a different one).
#[allow(dead_code)]
pub fn measure(
    device: &dyn SdrDevice,
    blocks_seen: &AtomicU64,
    frequencies: &[u64],
    timeout: Duration,
) -> RetuneMeasurement {
    let mut durations_ms = Vec::with_capacity(frequencies.len());
    let mut timed_out = 0usize;
    let mut failed = 0usize;
    for &hz in frequencies {
        match measure_one(device, blocks_seen, hz, timeout, DEFAULT_POLL_INTERVAL) {
            Ok(d) => durations_ms.push(d.as_secs_f32() * 1000.0),
            Err(RetuneError::TimedOut) => timed_out += 1,
            Err(RetuneError::SetFrequency(_)) => failed += 1,
        }
    }
    RetuneMeasurement {
        latency_ms: mean_with_uncertainty(&durations_ms),
        attempts: frequencies.len(),
        timed_out,
        failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::{DeviceCapabilities, DeviceInfo, RxContext};
    use std::sync::Arc;

    /// A device whose own `set_frequency` never fails and whose "retune
    /// latency" is exactly the fixed delay it was built with - spawning a
    /// thread that sleeps that long before moving the shared counter,
    /// standing in for the real gap between issuing a retune and a real
    /// backend's own next block reflecting it.
    struct FakeDevice {
        caps: DeviceCapabilities,
        blocks_seen: Arc<AtomicU64>,
        delay: Duration,
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
            true
        }
        fn set_frequency(&self, _hz: u64) -> anyhow::Result<()> {
            let blocks_seen = Arc::clone(&self.blocks_seen);
            let delay = self.delay;
            std::thread::spawn(move || {
                std::thread::sleep(delay);
                blocks_seen.fetch_add(1, Ordering::Relaxed);
            });
            Ok(())
        }
        fn set_sample_rate(&self, hz: f64) -> anyhow::Result<crate::hardware::RateSet> {
            Ok(crate::hardware::RateSet::new(hz, Some(hz), 0))
        }
        fn set_lna_gain(&self, _db: u32) -> anyhow::Result<()> {
            Ok(())
        }
    }

    /// A device that always refuses to retune - `measure_one`'s own
    /// `SetFrequency` error path, not the timeout one.
    struct RefusingDevice {
        caps: DeviceCapabilities,
    }

    impl SdrDevice for RefusingDevice {
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
        fn set_frequency(&self, _hz: u64) -> anyhow::Result<()> {
            anyhow::bail!("injected refusal")
        }
        fn set_sample_rate(&self, hz: f64) -> anyhow::Result<crate::hardware::RateSet> {
            Ok(crate::hardware::RateSet::new(hz, Some(hz), 0))
        }
        fn set_lna_gain(&self, _db: u32) -> anyhow::Result<()> {
            Ok(())
        }
    }

    /// [`measure_one`]'s own exit condition: the reported duration is
    /// close to the fake device's own real, known delay - not exact
    /// (scheduling jitter is real), but nowhere near zero or the timeout.
    #[test]
    fn measure_one_reports_close_to_the_real_injected_delay() {
        let blocks_seen = Arc::new(AtomicU64::new(0));
        let device = FakeDevice {
            caps: crate::hardware::native::hackrf::caps(),
            blocks_seen: Arc::clone(&blocks_seen),
            delay: Duration::from_millis(20),
        };
        let got = measure_one(
            &device,
            &blocks_seen,
            2_400_000_000,
            Duration::from_secs(1),
            Duration::from_micros(200),
        )
        .expect("should succeed");
        assert!(
            got >= Duration::from_millis(20) && got < Duration::from_millis(100),
            "got {got:?}"
        );
    }

    /// A device whose own `set_frequency` refuses reports that refusal,
    /// not a timeout - the two `RetuneError` variants mean different
    /// things and this checks the right one fires.
    #[test]
    fn a_refused_retune_is_reported_as_such_not_a_timeout() {
        let blocks_seen = Arc::new(AtomicU64::new(0));
        let device = RefusingDevice {
            caps: crate::hardware::native::hackrf::caps(),
        };
        let err = measure_one(
            &device,
            &blocks_seen,
            2_400_000_000,
            Duration::from_millis(50),
            Duration::from_micros(200),
        )
        .unwrap_err();
        assert!(matches!(err, RetuneError::SetFrequency(_)), "{err:?}");
    }

    /// A device that never moves the counter at all times out rather than
    /// hanging forever - `measure_one`'s own refusal to wait past `timeout`.
    #[test]
    fn a_counter_that_never_moves_times_out() {
        let blocks_seen = Arc::new(AtomicU64::new(0));
        let device = FakeDevice {
            caps: crate::hardware::native::hackrf::caps(),
            blocks_seen: Arc::clone(&blocks_seen),
            delay: Duration::from_secs(60), // effectively never, within this test
        };
        let err = measure_one(
            &device,
            &blocks_seen,
            2_400_000_000,
            Duration::from_millis(30),
            Duration::from_micros(200),
        )
        .unwrap_err();
        assert_eq!(err, RetuneError::TimedOut);
    }

    /// [`measure`]'s own exit condition: several real retunes fold into
    /// one resolved [`Uncertain`] reading, close to the fake device's own
    /// known delay, with every attempt counted as a success.
    #[test]
    fn measure_folds_several_retunes_into_one_resolved_reading() {
        let blocks_seen = Arc::new(AtomicU64::new(0));
        let device = FakeDevice {
            caps: crate::hardware::native::hackrf::caps(),
            blocks_seen: Arc::clone(&blocks_seen),
            delay: Duration::from_millis(10),
        };
        let frequencies = [
            2_400_000_000u64,
            2_410_000_000,
            2_400_000_000,
            2_410_000_000,
        ];
        let result = measure(&device, &blocks_seen, &frequencies, Duration::from_secs(1));
        assert_eq!(result.attempts, 4);
        assert_eq!(result.timed_out, 0);
        assert_eq!(result.failed, 0);
        assert!(
            result.latency_ms.is_resolved(1.0),
            "{:?}",
            result.latency_ms
        );
        assert!(
            result.latency_ms.value() >= 10.0 && result.latency_ms.value() < 50.0,
            "{:?}",
            result.latency_ms
        );
    }

    /// Every attempt failing is reported honestly - an unresolved
    /// reading, the same shape [`mean_with_uncertainty`] already gives an
    /// empty sample, not a fabricated number.
    #[test]
    fn measure_reports_unresolved_when_every_attempt_fails() {
        let blocks_seen = Arc::new(AtomicU64::new(0));
        let device = RefusingDevice {
            caps: crate::hardware::native::hackrf::caps(),
        };
        let frequencies = [2_400_000_000u64, 2_410_000_000];
        let result = measure(
            &device,
            &blocks_seen,
            &frequencies,
            Duration::from_millis(50),
        );
        assert_eq!(result.attempts, 2);
        assert_eq!(result.failed, 2);
        assert_eq!(result.timed_out, 0);
        assert!(
            !result.latency_ms.is_resolved(1.0),
            "{:?}",
            result.latency_ms
        );
    }
}
