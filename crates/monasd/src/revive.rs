// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! A radio that goes away comes back: unplugged and plugged in again, its
//! power cut and restored, its driver giving up.
//!
//! When the stream stops by itself, the run is let go, and the same radio
//! (the same kind, the same serial) is looked for again, first quickly, then
//! every half minute. Found, it is opened, told what it was set to by the
//! commands a client would send, given a new run, and streams again. **The
//! time it was gone is one event with its length and its cause**, and a line
//! in the log, so an hour without the radio is never read as a quiet hour.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::engine::command::Command;
use crate::engine::events::Event;
use crate::engine::{start, Engine};
use crate::hardware::{RxContext, SdrDevice};
use crate::state::SdrMetrics;

/// How long to wait before attempt `n` (from 0) to open the radio again:
/// 1, 2, 4, 8 and 16 seconds, then every 30. Quick for a cable that was
/// only knocked, patient for a radio that is gone for the night.
pub fn backoff(attempt: u32) -> Duration {
    Duration::from_secs(match attempt {
        0..=4 => 1 << attempt,
        _ => 30,
    })
}

/// `d` as the log says it: `45 s`, `3 min 12 s`, `2 h 5 min`.
pub fn span(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..=59 => format!("{s} s"),
        60..=3599 => format!("{} min {} s", s / 60, s % 60),
        _ => format!("{} h {} min", s / 3600, (s / 60) % 60),
    }
}

fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// What the watcher keeps of a run, to let it go and start another.
pub struct Run {
    pub running: start::Running,
    pub rx_ctx: Arc<RxContext>,
    pub bt_channels: usize,
}

/// How to open the same radio again: the kind and serial it had, looked for
/// and opened, or why not.
pub type Reopen = dyn Fn() -> Result<Arc<dyn SdrDevice>, String> + Send + Sync;

/// Watch for the stream stopping by itself, and bring the radio back each
/// time, until `stop` turns true. `pace` is the wait before each attempt
/// ([`backoff`], or something quicker in a test).
pub async fn watch(
    engine: Engine,
    state: Arc<Mutex<SdrMetrics>>,
    recordings: Arc<crate::record::Recordings>,
    mut run: Run,
    reopen: Arc<Reopen>,
    pace: fn(u32) -> Duration,
    mut stop: tokio::sync::watch::Receiver<bool>,
) {
    let say = |text: String| {
        state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_log(text)
    };
    loop {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(1)) => {}
            _ = stop.changed() => return,
        }
        let ended = state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .radio
            .stream_ended
            .clone();
        let Some(reason) = ended else {
            continue;
        };
        let from_unix_ms = unix_ms();
        let from = Instant::now();
        say(format!(
            "monasd: the radio stopped ({reason}); opening it again"
        ));

        // Let the old run go: its stream, its context, its threads.
        let device = engine.device();
        let _ = tokio::task::spawn_blocking(move || device.stop_rx()).await;
        let Run {
            running,
            rx_ctx,
            bt_channels,
        } = run;
        drop(rx_ctx);
        if let Ok(Err(left)) =
            tokio::task::spawn_blocking(move || running.stop(Duration::from_secs(5))).await
        {
            say(format!(
                "monasd: the old run's threads did not stop: {}",
                left.join(", ")
            ));
        }
        let was = engine.snapshot();

        // Look for it until it is there, or until asked to stop.
        let mut attempt = 0;
        let device = loop {
            tokio::select! {
                _ = tokio::time::sleep(pace(attempt)) => {}
                _ = stop.changed() => return,
            }
            let open = Arc::clone(&reopen);
            match tokio::task::spawn_blocking(move || open()).await {
                Ok(Ok(device)) => break device,
                Ok(Err(why)) => {
                    // Every attempt the first minute, then each one is a
                    // half-minute apart anyway.
                    say(format!("monasd: the radio is not back yet: {why}"));
                }
                Err(e) => say(format!("monasd: looking for the radio failed: {e}")),
            }
            attempt += 1;
        };

        engine.replace_device(Arc::clone(&device));
        let restoring = engine.clone();
        let refused = tokio::task::spawn_blocking(move || restoring.restore(&was))
            .await
            .unwrap_or_default();
        for r in refused {
            say(format!("monasd: on the radio opened again, {r}"));
        }
        let geometry = device.capabilities().sample_geometry;
        let (rx_ctx, feeds) = start::feeds(&state, geometry);
        recordings.set_tap(Arc::clone(&rx_ctx.record));
        let running = start::run(&state, &device, &rx_ctx, feeds, bt_channels);
        run = Run {
            running,
            rx_ctx,
            bt_channels,
        };
        // Cleared here, not left to the first poll of the new stream: the
        // next look must not read the old end as a new one.
        state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .radio
            .stream_ended = None;
        engine.apply(Command::Stream { on: true });

        let gone = from.elapsed();
        let to_unix_ms = unix_ms();
        {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            m.events.publish(Event::Outage {
                from_unix_ms,
                to_unix_ms,
                reason: reason.clone(),
            });
            m.push_log(format!(
                "monasd: the radio is back after {}, gone because {reason}",
                span(gone)
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::testing::Bench;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn the_waits_are_quick_then_patient() {
        let waits: Vec<u64> = (0..8).map(|n| backoff(n).as_secs()).collect();
        assert_eq!(waits, [1, 2, 4, 8, 16, 30, 30, 30]);
    }

    #[test]
    fn a_span_reads_as_the_log_says_it() {
        assert_eq!(span(Duration::from_secs(45)), "45 s");
        assert_eq!(span(Duration::from_secs(192)), "3 min 12 s");
        assert_eq!(span(Duration::from_secs(7_500)), "2 h 5 min");
    }

    fn quick(_: u32) -> Duration {
        Duration::from_millis(10)
    }

    /// **The radio goes, is looked for, comes back, and the time it was gone
    /// is one event.** Two looks fail, the third finds it; the new radio is
    /// the engine's, told where the old one was tuned, streaming again, and
    /// the outage is published with its cause.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_radio_that_goes_comes_back_and_its_absence_is_said() {
        let state = Arc::new(Mutex::new(SdrMetrics::fixture()));
        let old: Arc<dyn SdrDevice> = Arc::new(Bench::new());
        let engine = Engine::new(Arc::clone(&state), Arc::clone(&old));
        engine.apply(Command::Tune { hz: 145_500_000 });
        let events = engine.subscribe(64);
        let dir = std::env::temp_dir().join(format!("monasd-revive-{}", std::process::id()));
        let geometry = old.capabilities().sample_geometry;
        let (rx_ctx, feeds) = start::feeds(&state, geometry);
        let recordings = Arc::new(crate::record::Recordings::new(
            Arc::clone(&state),
            Arc::clone(&rx_ctx.record),
            dir,
            crate::config::RecordSettings::default(),
            "monasd test".into(),
        ));
        let running = start::run(&state, &old, &rx_ctx, feeds, 0);

        let looks = Arc::new(AtomicU32::new(0));
        let fresh = Arc::new(Bench::new());
        let reopen: Arc<Reopen> = {
            let looks = Arc::clone(&looks);
            let fresh = Arc::clone(&fresh);
            Arc::new(move || {
                if looks.fetch_add(1, Ordering::SeqCst) < 2 {
                    Err("no radio of that serial".to_string())
                } else {
                    Ok(Arc::clone(&fresh) as Arc<dyn SdrDevice>)
                }
            })
        };
        let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
        let watcher = tokio::spawn(watch(
            engine.clone(),
            Arc::clone(&state),
            recordings,
            Run {
                running,
                rx_ctx,
                bt_channels: 0,
            },
            reopen,
            quick,
            stop_rx,
        ));

        state.lock().unwrap().radio.stream_ended = Some("the cable was pulled".into());
        let outage = tokio::task::spawn_blocking(move || loop {
            match events.next_blocking() {
                Some(d) => {
                    if let Event::Outage { .. } = d.event {
                        break d.event;
                    }
                }
                None => panic!("the events ended"),
            }
        })
        .await
        .unwrap();
        let Event::Outage {
            from_unix_ms,
            to_unix_ms,
            reason,
        } = outage
        else {
            unreachable!()
        };
        assert_eq!(reason, "the cable was pulled");
        assert!(from_unix_ms <= to_unix_ms);
        assert_eq!(looks.load(Ordering::SeqCst), 3);
        assert_eq!(fresh.tuned.lock().unwrap().last(), Some(&145_500_000));
        {
            let m = state.lock().unwrap();
            assert!(m.radio.stream_ended.is_none());
            assert!(m.radio.rx_enabled, "streaming asked for again");
            assert!(m.log.iter().any(|l| l.text.contains("is not back yet")));
            assert!(m
                .log
                .iter()
                .any(|l| l.text.contains("the radio is back after")));
        }
        stop_tx.send(true).unwrap();
        watcher.await.unwrap();
    }
}
