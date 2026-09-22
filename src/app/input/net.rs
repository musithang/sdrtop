// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The NET section's panel handlers.

use crossterm::event::{KeyCode, KeyEvent};

use super::{global, metrics, InputCtx, KeyAction};
use crate::state::{InputMode, SdrMetrics};

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

/// The occupancy profile: a cursor across the band, one megahertz cell at a
/// time, `B` to put it on the busiest cell (the headline it replaces), and `L`
/// to lock the receiver where the cursor stands.
///
/// **`L` only asks.** It switches NET to lock and leaves the target
/// (`survey::lock_target`: off the cell's centre, or a BLE advertising
/// channel's exact centre) in `NetState::lock_at`; the survey task, the one
/// place NET retunes from, applies it and logs where the radio went and why.
/// Refused, with the reason, in observer mode or with no cursor set.
///
/// Moves through every cell, observed or not, because a cell nobody looked at
/// is a place on the band too, and its readout says so.
pub(super) fn net_occupancy(key: KeyEvent, ctx: &mut InputCtx<'_>) -> KeyAction {
    let cells: Vec<usize> = (0..crate::signal::net::occupancy::CELLS).collect();
    let mut m = metrics(ctx.state);
    match key.code {
        KeyCode::Left => m.net.band_cursor.move_by(&cells, -1),
        KeyCode::Right => m.net.band_cursor.move_by(&cells, 1),
        KeyCode::Char('l') => {
            let Some(cell) = m.net.band_cursor.selected else {
                m.push_log(
                    "Lock: put the cursor on a cell first (\u{2190}\u{2192} or B)".to_string(),
                );
                return KeyAction::Continue;
            };
            if ctx.device.is_none() {
                m.push_log("Lock: no radio to retune in observer mode".to_string());
                return KeyAction::Continue;
            }
            let target = crate::signal::net::survey::lock_target(cell);
            m.push_log(format!(
                "NET locking for {} MHz",
                crate::signal::net::occupancy::cell_centre_hz(cell) / 1_000_000
            ));
            if m.net.mode != crate::state::NetMode::Lock {
                m.net.mode = crate::state::NetMode::Lock;
                // The same restart the `m` key makes: survey and lock are
                // different regimes for how often a megahertz is watched.
                m.net.band.restart_watch();
            }
            m.net.lock_at = Some(target);
        }
        KeyCode::Char('b') => {
            let busiest = m
                .net
                .band
                .cells
                .iter()
                .enumerate()
                .filter(|(_, c)| c.observed() && c.duty > 0.0)
                .max_by(|a, b| a.1.duty.total_cmp(&b.1.duty))
                .map(|(i, _)| i);
            match busiest {
                Some(cell) => m.net.band_cursor.selected = Some(cell),
                None => m.push_log("Occupancy: nothing above the floor yet".to_string()),
            }
        }
        _ => {
            drop(m);
            return global::handle(key, ctx);
        }
    }
    KeyAction::Continue
}

/// The coexistence history's time cursor, shared with the profile above it:
/// `↓` goes back a moment, `↑` forward, and forward past the newest, or `N`,
/// is now again.
///
/// **It remembers the moment, not the row.** The cursor holds the column's
/// identity (`BandOccupancy::columns_taken`), so as new columns arrive every
/// half second it stays on the moment it was put on and moves down the canvas
/// with it, until the moment scrolls out of the history and the profile goes
/// back to now.
pub(super) fn net_coexist(key: KeyEvent, ctx: &mut InputCtx<'_>) -> KeyAction {
    let mut m = metrics(ctx.state);
    let band = &m.net.band;
    let back = m.net.band_scrub.and_then(|id| band.back_of(id));
    let next = match key.code {
        KeyCode::Down => Some(back.map_or(0, |b| b + 1)),
        KeyCode::Up => back.and_then(|b| b.checked_sub(1)),
        KeyCode::Char('n') => None,
        _ => {
            drop(m);
            return global::handle(key, ctx);
        }
    };
    // Past the oldest column there is nothing to go back to: stay on it.
    let oldest = band.history.len().checked_sub(1);
    let next = next.map(|b| oldest.map_or(b, |o| b.min(o)));
    m.net.band_scrub = next.and_then(|b| m.net.band.id_back(b));
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
        // Trust the selected transmitter as the frequency reference: ask how
        // far, through the same text entry frequency uses. `t` is the timing
        // diagnostics panel's focus letter, which no NET preset holds.
        KeyCode::Char('t') => trust_selected(&mut m),
        _ => {
            drop(m);
            return global::handle(key, ctx);
        }
    }
    KeyAction::Continue
}

/// The BLE packet list: the arrows move the cursor through the packets in the
/// order the panel draws them, newest first; anything else goes on to the
/// global keys.
pub(super) fn net_ble_packets(key: KeyEvent, ctx: &mut InputCtx<'_>) -> KeyAction {
    let mut m = metrics(ctx.state);
    let order: Vec<u64> = m.net.ble_packets.iter().map(|p| p.seq).collect();
    match key.code {
        KeyCode::Up => m.net.ble_view.selection.move_by(&order, -1),
        KeyCode::Down => m.net.ble_view.selection.move_by(&order, 1),
        _ => {
            drop(m);
            return global::handle(key, ctx);
        }
    }
    KeyAction::Continue
}

/// Start the reference-accuracy entry for the selected census device, or say
/// why not: nothing selected, or no offset measured to reference against.
fn trust_selected(m: &mut SdrMetrics) {
    let device = m
        .net
        .census
        .selection
        .selected
        .and_then(|a| m.net.census.devices.iter().find(|d| d.address == a));
    match device {
        None => m.push_log("Reference: select a device in the census first"),
        Some(d) if d.crystal_offset_ppm.is_none() => {
            let name = d.address_text(&m.net, None);
            m.push_log(format!(
                "Reference: no offset measured for {name} yet, nothing to reference against"
            ));
        }
        Some(d) => {
            let address = d.address;
            m.ui.input_buf.clear();
            m.ui.input_mode = InputMode::ReferenceAccuracyInput { address };
        }
    }
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
            packets,
            best_snr_db: Some(10.0),
            last_seen: now,
            ..Device::heard(
                [0xa4, 0x83, 0xe7, 0x1c, 0x09, tail],
                false,
                now - Duration::from_secs(60),
            )
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
        m.net.census.sort = crate::signal::net::census::column("PKTS");
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

    /// **`T` on a selected device, a figure, Enter: the reference is set**,
    /// through the same dispatch every key takes (`handle_key`), with the
    /// provenance naming whose word it is. Our oscillator 10 ppm fast and the
    /// trusted device dead on: the air shows it at -10, so ours reads +10.
    #[test]
    fn t_then_a_figure_makes_the_selected_device_the_reference() {
        use crate::signal::dsp::uncertainty::Uncertain;
        let mut m = SdrMetrics::fixture().streaming();
        let trusted = Device {
            crystal_offset_ppm: Some(Uncertain::from_sigma(-10.0, 0.3)),
            ..device(1, 50)
        };
        m.net.census.devices = vec![trusted, device(2, 5)];
        m.net.census.selection.selected = Some([0xa4, 0x83, 0xe7, 0x1c, 0x09, 1]);
        let state = Arc::new(Mutex::new(m));
        let mut engine = LayoutEngine::new(
            crate::config::LayoutConfig::default_config(),
            PanelRegistry::new(),
        );
        let mut show_footer = true;
        let focus_keys = HashMap::new();
        let key = |c| KeyEvent::new(c, KeyModifiers::NONE);

        {
            let mut ctx = InputCtx {
                state: &state,
                device: None,
                engine: &mut engine,
                show_footer: &mut show_footer,
                focus_keys: &focus_keys,
            };
            net_census(key(KeyCode::Char('t')), &mut ctx);
        }
        assert!(matches!(
            metrics(&state).ui.input_mode,
            InputMode::ReferenceAccuracyInput { .. }
        ));

        let mut type_ = |code| {
            super::super::handle_key(
                key(code),
                &state,
                None,
                &mut engine,
                &mut show_footer,
                &focus_keys,
            );
        };
        // Zero is refused and the entry stays open to be corrected.
        type_(KeyCode::Char('0'));
        type_(KeyCode::Enter);
        assert!(metrics(&state).radio.reference.is_none());
        assert!(matches!(
            metrics(&state).ui.input_mode,
            InputMode::ReferenceAccuracyInput { .. }
        ));
        type_(KeyCode::Backspace);
        type_(KeyCode::Char('2'));
        type_(KeyCode::Enter);

        let m = metrics(&state);
        assert!(m.ui.input_mode == InputMode::Normal);
        let r = m.radio.reference.as_ref().expect("a reference");
        assert!((r.ppm - 10.0).abs() < 1e-12, "{}", r.ppm);
        assert_eq!(r.provenance, crate::state::Provenance::Referenced);
        assert_eq!(r.source, "a4:83:e7:1c:09:01 (user-stated ±2 ppm)");
    }

    /// A device with no offset cannot be a reference: `T` says so in the log
    /// and opens no entry.
    #[test]
    fn t_on_a_device_without_an_offset_says_why_and_asks_nothing() {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.census.devices = vec![device(1, 50)];
        m.net.census.selection.selected = Some([0xa4, 0x83, 0xe7, 0x1c, 0x09, 1]);
        trust_selected(&mut m);
        assert!(m.ui.input_mode == InputMode::Normal);
        let said = |m: &SdrMetrics, text: &str| m.ui.log.iter().any(|l| l.text.contains(text));
        assert!(said(&m, "nothing to reference against"));

        m.net.census.selection.selected = None;
        trust_selected(&mut m);
        assert!(said(&m, "select a device"));
    }

    /// **The arrows walk the packets in the order the list draws them**,
    /// newest first: the first press lands on the newest.
    #[test]
    fn the_arrows_select_packets_newest_first() {
        let mut m = SdrMetrics::fixture().streaming();
        for seq in 1..=3u64 {
            m.net.ble_packets.push_front(crate::state::BlePacket {
                seq,
                ..sample_packet()
            });
        }
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
            net_ble_packets(KeyEvent::new(code, KeyModifiers::NONE), &mut ctx);
        };
        let selected = |s: &Arc<Mutex<SdrMetrics>>| metrics(s).net.ble_view.selection.selected;
        press(KeyCode::Down);
        assert_eq!(selected(&state), Some(3));
        press(KeyCode::Down);
        assert_eq!(selected(&state), Some(2));
        press(KeyCode::Up);
        assert_eq!(selected(&state), Some(3));
    }

    fn sample_packet() -> crate::state::BlePacket {
        crate::state::BlePacket {
            seq: 0,
            channel: 37,
            pdu_type: crate::signal::ble::pdu::PduType::AdvInd,
            ch_sel: false,
            tx_add_random: false,
            rx_add_random: false,
            length: 6,
            adv_addr: Some([1, 2, 3, 4, 5, 6]),
            payload: Vec::new(),
            crc_ok: true,
            snr_db: None,
            freq_offset_hz: None,
            modulation: None,
            drift: None,
            seen: Instant::now(),
        }
    }

    /// **One letter, two panels, never on one screen**: on the BLE layout `v`
    /// focuses the packet list, on the Lab timing bench the same `v` still
    /// focuses timing vitals. Built the way the app builds, keys and all.
    #[test]
    fn a_shared_focus_letter_focuses_the_panel_on_screen() {
        let (mut engine, focus_keys) =
            crate::app::App::build_ui("net_ble", &HashMap::new(), None, true);
        let state = Arc::new(Mutex::new(SdrMetrics::fixture()));
        let mut show_footer = true;
        let mut focus = |engine: &mut LayoutEngine, preset: &str| {
            engine.set_preset(preset);
            let mut ctx = InputCtx {
                state: &state,
                device: None,
                engine,
                show_footer: &mut show_footer,
                focus_keys: &focus_keys,
            };
            super::global::handle(
                KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE),
                &mut ctx,
            );
            metrics(&state).ui.focused_panel.clone()
        };
        assert_eq!(
            focus(&mut engine, "net_ble").as_deref(),
            Some("net_ble_packets")
        );
        assert_eq!(
            focus(&mut engine, "lab_timing").as_deref(),
            Some("timing_vitals")
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

    /// The occupancy cursor: the arrows walk the band a megahertz at a time
    /// and stop at its ends, and `B` lands on the busiest observed cell.
    #[test]
    fn the_occupancy_cursor_walks_the_band_and_finds_the_busiest_cell() {
        let mut m = SdrMetrics::fixture().streaming();
        let mut cells =
            vec![crate::state::CellReading::default(); crate::signal::net::occupancy::CELLS];
        for (i, duty) in [(10usize, 0.2), (40, 0.9), (60, 0.5)] {
            cells[i].windows = 100;
            cells[i].duty = duty;
        }
        m.net.band.cells = cells;
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
            net_occupancy(KeyEvent::new(code, KeyModifiers::NONE), &mut ctx);
        };
        let at = |s: &Arc<Mutex<SdrMetrics>>| metrics(s).net.band_cursor.selected;

        press(KeyCode::Right);
        assert_eq!(at(&state), Some(0), "the first press starts at 2400 MHz");
        press(KeyCode::Left);
        assert_eq!(at(&state), Some(0), "and stops at the band's edge");
        press(KeyCode::Char('b'));
        assert_eq!(at(&state), Some(40), "the busiest observed cell");
        press(KeyCode::Right);
        assert_eq!(at(&state), Some(41));
    }

    /// The time cursor: down goes back, up comes forward and past the newest
    /// is now again, `N` is now, and the cursor stays on its moment when a new
    /// column arrives.
    #[test]
    fn the_time_cursor_walks_the_history_and_keeps_its_moment() {
        let mut m = SdrMetrics::fixture().streaming();
        for v in [0.1f32, 0.2, 0.3] {
            m.net.band.history.push_back(vec![v; 4]);
            m.net.band.columns_taken += 1;
        }
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
            net_coexist(KeyEvent::new(code, KeyModifiers::NONE), &mut ctx);
        };
        let back = |s: &Arc<Mutex<SdrMetrics>>| {
            let m = metrics(s);
            m.net.band_scrub.and_then(|id| m.net.band.back_of(id))
        };

        press(KeyCode::Down);
        assert_eq!(back(&state), Some(0), "the first step lands on the newest");
        press(KeyCode::Down);
        press(KeyCode::Down);
        press(KeyCode::Down);
        assert_eq!(back(&state), Some(2), "and stops at the oldest");
        press(KeyCode::Up);
        assert_eq!(back(&state), Some(1));

        // A new column arrives: the cursor is one step further back, on the
        // same moment.
        {
            let mut m = metrics(&state);
            m.net.band.history.push_back(vec![0.4; 4]);
            m.net.band.columns_taken += 1;
        }
        assert_eq!(back(&state), Some(2));

        press(KeyCode::Char('n'));
        assert_eq!(metrics(&state).net.band_scrub, None);
        press(KeyCode::Down);
        press(KeyCode::Up);
        assert_eq!(
            metrics(&state).net.band_scrub,
            None,
            "forward past the newest is now"
        );
    }

    /// `L` locks where the cursor stands, by asking: the mode goes to lock
    /// and the target waits in `lock_at` for the survey task. With no cursor,
    /// or no radio, it says why and asks nothing.
    #[test]
    fn l_asks_for_a_lock_where_the_cursor_stands() {
        let state = Arc::new(Mutex::new(SdrMetrics::fixture().streaming()));
        let concrete = Arc::new(Tuner {
            caps: crate::hardware::native::hackrf::caps(),
            calls: Mutex::new(Vec::new()),
        });
        let tuner: Arc<dyn crate::hardware::SdrDevice> = concrete.clone();
        let mut engine = LayoutEngine::new(
            crate::config::LayoutConfig::default_config(),
            PanelRegistry::new(),
        );
        let mut show_footer = true;
        let focus_keys = HashMap::new();
        let mut press = |device: Option<&Arc<dyn crate::hardware::SdrDevice>>| {
            let mut ctx = InputCtx {
                state: &state,
                device,
                engine: &mut engine,
                show_footer: &mut show_footer,
                focus_keys: &focus_keys,
            };
            net_occupancy(
                KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE),
                &mut ctx,
            );
        };

        press(Some(&tuner));
        assert!(
            log(&state).contains("put the cursor on a cell first"),
            "{}",
            log(&state)
        );
        assert!(metrics(&state).net.lock_at.is_none());

        metrics(&state).net.band_cursor.selected = Some(41);
        press(None);
        assert!(log(&state).contains("observer mode"), "{}", log(&state));
        assert!(metrics(&state).net.lock_at.is_none());

        press(Some(&tuner));
        let m = metrics(&state);
        assert_eq!(m.net.mode, crate::state::NetMode::Lock);
        assert_eq!(
            m.net.lock_at,
            Some(crate::signal::net::survey::lock_target(41))
        );
        assert!(
            concrete.calls.lock().unwrap().is_empty(),
            "the key asks; the survey task tunes"
        );
    }
}
