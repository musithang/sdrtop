// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The NET section's panel handlers.

use crossterm::event::{KeyCode, KeyEvent};

use super::{global, metrics, InputCtx, KeyAction};

/// The Capability panel's one action: `K` times the radio's tuning call across
/// the band (`signal::retune::measure_calls`).
///
/// **Refused rather than fought over.** It retunes the radio ten times, so it
/// will not start while the survey is hopping the same radio (lock first,
/// `m`), with no radio to retune (observer mode), or while a run is already
/// going. The run is on its own thread: ten USB control transfers are not the
/// input thread's to wait for, and no lock is held across a device call (the
/// `tasks/rx` discipline). Every retune updates `radio.frequency` too, outside
/// the timed span, because the RX pipeline stamps each block with it and the
/// NET worker would otherwise decode 2480 MHz as whatever the header said.
/// The radio goes back to where it was at the end, and the log says so.
pub(super) fn net_capability(key: KeyEvent, ctx: &mut InputCtx<'_>) -> KeyAction {
    if key.code != KeyCode::Char('k') {
        return global::handle(key, ctx);
    }
    let home = {
        let mut m = metrics(ctx.state);
        if matches!(m.net.retune, Some(crate::state::RetuneRun::Measuring)) {
            return KeyAction::Continue;
        }
        if m.net.mode == crate::state::NetMode::Survey && m.radio.hw_streaming {
            m.push_log(
                "Retune timing: the survey is hopping this radio, lock first ([M])".to_string(),
            );
            return KeyAction::Continue;
        }
        m.radio.frequency
    };
    let Some(device) = ctx.device.cloned() else {
        metrics(ctx.state)
            .push_log("Retune timing: no radio to retune in observer mode".to_string());
        return KeyAction::Continue;
    };
    {
        let mut m = metrics(ctx.state);
        m.net.retune = Some(crate::state::RetuneRun::Measuring);
        m.push_log("Retune timing: ten calls across the band".to_string());
    }
    let state = std::sync::Arc::clone(ctx.state);
    std::thread::spawn(move || {
        let result = crate::signal::retune::measure_calls(
            device.as_ref(),
            &crate::signal::retune::BAND_HOPS_HZ,
            |hz| metrics(&state).radio.frequency = hz,
        );
        let back = device.set_frequency(home);
        let mut m = metrics(&state);
        if back.is_ok() {
            m.radio.frequency = home;
        }
        let reading =
            crate::ui::widgets::reading::Reading::new(result.call_ms, "ms", f64::INFINITY);
        let refused = result
            .first_error
            .as_ref()
            .map(|e| format!(", refused: {e}"))
            .unwrap_or_default();
        let home_note = match back {
            Ok(()) => format!("; back on {:.3} MHz", home as f64 / 1e6),
            Err(e) => format!("; could not go back to {:.3} MHz: {e}", home as f64 / 1e6),
        };
        m.push_log(format!(
            "Retune timing: tuning call {} over {} of {}{refused}{home_note}",
            reading.text(),
            result.attempts - result.failed,
            result.attempts,
        ));
        m.net.retune = Some(crate::state::RetuneRun::Done(
            result,
            std::time::Instant::now(),
        ));
    });
    KeyAction::Continue
}

/// The census table: move the cursor, change what orders it.
///
/// **The cursor moves through the ordering, not through the census**, which is
/// why this asks the panel's own ordering for the addresses rather than the
/// order they happen to be stored in. `CensusState` then remembers the device
/// rather than the row, so a re-sort under the cursor leaves it where it was.
pub(super) fn net_census(key: KeyEvent, ctx: &mut InputCtx<'_>) -> KeyAction {
    let mut m = metrics(ctx.state);
    // The same ordering the panel draws, from the same function, so the cursor
    // steps through the rows on screen and not through a list of its own.
    let ordered = m
        .net
        .census
        .ordered_addresses(std::time::Instant::now(), &m.radio);
    match key.code {
        KeyCode::Up => m.net.census.selection.move_by(&ordered, -1),
        KeyCode::Down => m.net.census.selection.move_by(&ordered, 1),
        KeyCode::Char('s') => m.net.census.cycle_sort(),
        KeyCode::Char('r') => m.net.census.reverse(),
        _ => {
            drop(m);
            return global::handle(key, ctx);
        }
    }
    KeyAction::Continue
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::*;
    use crate::signal::net::census::Device;
    use crate::state::SdrMetrics;
    use crate::ui::{LayoutEngine, PanelRegistry};

    fn device(tail: u8, packets: u64) -> Device {
        let now = Instant::now();
        Device {
            address: [0xa4, 0x83, 0xe7, 0x1c, 0x09, tail],
            random: false,
            packets,
            best_snr_db: 10.0,
            first_seen: now - Duration::from_secs(60),
            last_seen: now,
            crystal_offset_ppm: None,
        }
    }

    /// **The regression: the arrows move through the rows the panel draws.**
    ///
    /// The handler used to move the cursor through an empty list, so on a
    /// populated census the arrows never selected anything. Ordered by packet
    /// count, descending, the rows are 3, 1, 2; two presses down must land on
    /// the second row as drawn, device 1 - not on the second device as stored.
    #[test]
    fn the_arrows_select_the_rows_as_the_panel_orders_them() {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.census.devices = vec![device(2, 5), device(1, 50), device(3, 500)];
        m.net.census.sort = 2; // PKTS
        m.net.census.descending = true;
        let state = Arc::new(Mutex::new(m));
        let mut engine = LayoutEngine::new(
            crate::config::LayoutConfig::default_config(),
            PanelRegistry::new(),
        );
        let mut show_footer = true;
        let focus_keys = HashMap::new();
        let mut ctx = InputCtx {
            state: &state,
            device: None,
            engine: &mut engine,
            show_footer: &mut show_footer,
            focus_keys: &focus_keys,
        };
        let mut press = |code| {
            net_census(KeyEvent::new(code, KeyModifiers::NONE), &mut ctx);
        };

        press(KeyCode::Down);
        assert_eq!(
            metrics(&state).net.census.selection.selected.map(|a| a[5]),
            Some(3),
            "the first press lands on the first row drawn"
        );
        press(KeyCode::Down);
        assert_eq!(
            metrics(&state).net.census.selection.selected.map(|a| a[5]),
            Some(1)
        );
        press(KeyCode::Up);
        assert_eq!(
            metrics(&state).net.census.selection.selected.map(|a| a[5]),
            Some(3)
        );
    }

    /// A radio whose tuning call takes 1 ms and remembers where it was sent.
    struct Tuner {
        caps: crate::hardware::DeviceCapabilities,
        calls: Mutex<Vec<u64>>,
    }

    impl crate::hardware::SdrDevice for Tuner {
        fn capabilities(&self) -> &crate::hardware::DeviceCapabilities {
            &self.caps
        }
        fn info(&self) -> crate::hardware::DeviceInfo {
            crate::hardware::DeviceInfo::default()
        }
        fn start_rx(&self, _: Arc<crate::hardware::RxContext>) -> anyhow::Result<()> {
            Ok(())
        }
        fn stop_rx(&self) -> anyhow::Result<()> {
            Ok(())
        }
        fn is_streaming(&self) -> bool {
            true
        }
        fn set_frequency(&self, hz: u64) -> anyhow::Result<()> {
            std::thread::sleep(Duration::from_millis(1));
            self.calls.lock().unwrap().push(hz);
            Ok(())
        }
        fn set_sample_rate(&self, hz: f64) -> anyhow::Result<crate::hardware::RateSet> {
            Ok(crate::hardware::RateSet::new(hz, Some(hz), 0))
        }
        fn set_lna_gain(&self, _db: u32) -> anyhow::Result<()> {
            Ok(())
        }
    }

    fn press_k(
        state: &Arc<Mutex<SdrMetrics>>,
        device: Option<&Arc<dyn crate::hardware::SdrDevice>>,
    ) {
        let mut engine = LayoutEngine::new(
            crate::config::LayoutConfig::default_config(),
            PanelRegistry::new(),
        );
        let mut show_footer = true;
        let focus_keys = HashMap::new();
        let mut ctx = InputCtx {
            state,
            device,
            engine: &mut engine,
            show_footer: &mut show_footer,
            focus_keys: &focus_keys,
        };
        net_capability(
            KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
            &mut ctx,
        );
    }

    fn log(state: &Arc<Mutex<SdrMetrics>>) -> String {
        metrics(state)
            .ui
            .log
            .iter()
            .map(|e| e.text.clone())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// **The whole run.** Ten calls across the band, the tuning record kept
    /// in step, the radio sent home at the end, and the result in the state
    /// for the panel.
    #[test]
    fn k_times_ten_calls_and_puts_the_radio_back() {
        let mut m = SdrMetrics::fixture();
        m.radio.frequency = 2_426_000_000;
        let state = Arc::new(Mutex::new(m));
        let tuner = Arc::new(Tuner {
            caps: crate::hardware::native::hackrf::caps(),
            calls: Mutex::new(Vec::new()),
        });
        let device: Arc<dyn crate::hardware::SdrDevice> = tuner.clone();
        press_k(&state, Some(&device));

        let deadline = Instant::now() + Duration::from_secs(5);
        while !matches!(
            metrics(&state).net.retune,
            Some(crate::state::RetuneRun::Done(..))
        ) {
            assert!(Instant::now() < deadline, "the run never finished");
            std::thread::sleep(Duration::from_millis(5));
        }
        let calls = tuner.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 11, "ten timed calls and one home: {calls:?}");
        assert_eq!(&calls[..10], &crate::signal::retune::BAND_HOPS_HZ);
        assert_eq!(calls[10], 2_426_000_000);
        let m = metrics(&state);
        assert_eq!(m.radio.frequency, 2_426_000_000, "back where it was");
        let Some(crate::state::RetuneRun::Done(result, _)) = &m.net.retune else {
            unreachable!()
        };
        assert_eq!((result.attempts, result.failed), (10, 0));
        assert!(result.call_ms.value() >= 1.0, "{:?}", result.call_ms);
        drop(m);
        assert!(
            log(&state).contains("back on 2426.000 MHz"),
            "{}",
            log(&state)
        );
    }

    /// Refused, with the reason in the log, while the survey is hopping the
    /// same radio, and with no radio at all.
    #[test]
    fn k_refuses_to_fight_the_survey_and_needs_a_radio() {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.mode = crate::state::NetMode::Survey;
        let state = Arc::new(Mutex::new(m));
        press_k(&state, None);
        assert!(log(&state).contains("lock first"), "{}", log(&state));
        assert!(metrics(&state).net.retune.is_none());

        metrics(&state).net.mode = crate::state::NetMode::Lock;
        press_k(&state, None);
        assert!(log(&state).contains("observer mode"), "{}", log(&state));
        assert!(metrics(&state).net.retune.is_none());
    }
}
