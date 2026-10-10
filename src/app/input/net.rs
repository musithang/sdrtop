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

/// The piconet roster: the arrows move the cursor through the piconets in the
/// order the panel draws them (`NetState::bt_roster`); the cursor holds a
/// LAP, so a re-sort keeps it on the same piconet. `s` orders by the next
/// column and `r` reverses, as on the Census, each back to the top.
pub(super) fn net_bt_piconets(key: KeyEvent, ctx: &mut InputCtx<'_>) -> KeyAction {
    let mut m = metrics(ctx.state);
    let order: Vec<u32> = m.net.bt_roster().iter().map(|p| p.lap).collect();
    match key.code {
        KeyCode::Up => m.net.bt_view.move_by(&order, -1),
        KeyCode::Down => m.net.bt_view.move_by(&order, 1),
        KeyCode::Char('s') => {
            m.net.bt_sort.cycle();
            m.net.bt_view.reset_view();
        }
        KeyCode::Char('r') => {
            m.net.bt_sort.reverse();
            m.net.bt_view.reset_view();
        }
        // The selected piconet, packet by packet: the Piconet view opens on
        // it (`global::presets::try_set_preset` carries the selection).
        KeyCode::Enter => {
            if m.net.bt_view.cursor(&order).is_none() {
                m.push_log("Piconets: select a piconet first (↑↓)");
                return KeyAction::Continue;
            }
            drop(m);
            return global::presets::try_set_preset(ctx.engine, ctx.state, "net_piconet");
        }
        _ => {
            drop(m);
            return global::handle(key, ctx);
        }
    }
    KeyAction::Continue
}

/// The Piconet view's packet list. `↓` scrolls into the past and holds the
/// list at its newest packet first, so the rows do not slide under the
/// reader as packets arrive; `↑` scrolls back up; `h` holds the list or lets
/// it run, as on the BLE list; `End` is live again, at the top. `l` swaps
/// every packet for the piconet's LMP log and back, from live at the top;
/// the log is a mode, like the hold, so `End`, `h` and a step to another
/// piconet keep it. `← →` step to the previous or next piconet in the
/// roster's order, the view starting afresh on each; with none heard they
/// do nothing, rather than stepping the radio from under a focused list.
pub(super) fn net_bt_packets(key: KeyEvent, ctx: &mut InputCtx<'_>) -> KeyAction {
    let mut m = metrics(ctx.state);
    let order: Vec<u32> = m.net.bt_roster().iter().map(|p| p.lap).collect();
    let packets: Option<&std::collections::VecDeque<_>> = m
        .net
        .bt_view
        .cursor(&order)
        .and_then(|i| m.net.bt_piconets.iter().find(|p| p.lap == order[i]))
        .map(|p| crate::ui::panels::net::bt::bt_packets::shown(p, &m.net.packets_view));
    let newest = packets.and_then(|k| k.front()).map(|k| (k.stream, k.at_us));
    // Rows below the held packet (or the newest), to scroll no further than.
    let below = packets.map_or(0, |k| {
        let from = m.net.packets_view.held.map_or(Some(0), |(stream, at)| {
            k.iter().position(|p| p.stream == stream && p.at_us == at)
        });
        from.map_or(0, |i| k.len().saturating_sub(i + 1))
    });
    let view = &mut m.net.packets_view;
    match key.code {
        KeyCode::Up => view.first_visible = view.first_visible.saturating_sub(1),
        KeyCode::Down => {
            if view.held.is_none() {
                view.held = newest;
            }
            if view.held.is_some() {
                view.first_visible = (view.first_visible + 1).min(below);
            }
        }
        KeyCode::Char('h') => {
            *view = match view.held {
                Some(_) => crate::state::PacketsView {
                    lmp_only: view.lmp_only,
                    ..Default::default()
                },
                None => crate::state::PacketsView {
                    held: newest,
                    ..*view
                },
            };
        }
        KeyCode::End => {
            *view = crate::state::PacketsView {
                lmp_only: view.lmp_only,
                ..Default::default()
            }
        }
        KeyCode::Char('l') => {
            *view = crate::state::PacketsView {
                lmp_only: !view.lmp_only,
                ..Default::default()
            }
        }
        KeyCode::Left | KeyCode::Right => step_piconet(&mut m, &order, key.code == KeyCode::Right),
        _ => {
            drop(m);
            return global::handle(key, ctx);
        }
    }
    KeyAction::Continue
}

/// The Connection view: `↑↓` scroll through the events kept, as far as the
/// oldest one's last row; `End` goes back to the newest; `← →` step to the
/// previous or next connection followed.
pub(super) fn net_ble_connection(key: KeyEvent, ctx: &mut InputCtx<'_>) -> KeyAction {
    let mut m = metrics(ctx.state);
    let rows = crate::ui::panels::net::ble::ble_connection::selected(&m).map_or(0, |f| {
        crate::ui::panels::net::ble::ble_connection::row_count(&f.connection)
    });
    let order: Vec<u32> = m
        .net
        .ble_connections
        .iter()
        .map(|f| f.connection.access_address())
        .collect();
    let view = &mut m.net.connection_view;
    match key.code {
        KeyCode::Up => view.first_visible = view.first_visible.saturating_sub(1),
        KeyCode::Down => view.first_visible = (view.first_visible + 1).min(rows.saturating_sub(1)),
        KeyCode::End => view.first_visible = 0,
        // The previous or next connection in the order they were heard,
        // newest first, each from its newest event; no wrapping.
        KeyCode::Left | KeyCode::Right if !order.is_empty() => {
            let at = view
                .selected
                .and_then(|aa| order.iter().position(|&o| o == aa))
                .unwrap_or(0);
            let to = if key.code == KeyCode::Right {
                (at + 1).min(order.len() - 1)
            } else {
                at.saturating_sub(1)
            };
            *view = crate::state::ConnectionView {
                selected: Some(order[to]),
                first_visible: 0,
            };
        }
        _ => {
            drop(m);
            return global::handle(key, ctx);
        }
    }
    KeyAction::Continue
}

/// `← →` in the Piconet view, the list's or the bench's: the previous or
/// next piconet in the roster's order, the list starting afresh on it. With
/// none heard, nothing, rather than a cleared selection.
fn step_piconet(m: &mut SdrMetrics, order: &[u32], forward: bool) {
    if !order.is_empty() {
        m.net.bt_view.move_by(order, if forward { 1 } else { -1 });
        m.net.packets_view = crate::state::PacketsView {
            lmp_only: m.net.packets_view.lmp_only,
            ..Default::default()
        };
    }
}

/// The Piconet view's bench: `← →` step through the piconets as the list
/// does, so either panel focused moves the view.
pub(super) fn net_bt_bench(key: KeyEvent, ctx: &mut InputCtx<'_>) -> KeyAction {
    let mut m = metrics(ctx.state);
    let order: Vec<u32> = m.net.bt_roster().iter().map(|p| p.lap).collect();
    match key.code {
        KeyCode::Left | KeyCode::Right => step_piconet(&mut m, &order, key.code == KeyCode::Right),
        _ => {
            drop(m);
            return global::handle(key, ctx);
        }
    }
    KeyAction::Continue
}

/// The classic hop scatter: `↑↓` move the piconet selection the roster
/// shares, in the roster's order; `+`/`-` zoom the window; `←`/`→` move it
/// back and forward in time, no further back than the oldest hit kept;
/// `End` returns to now.
pub(super) fn net_bt_hops(key: KeyEvent, ctx: &mut InputCtx<'_>) -> KeyAction {
    let mut m = metrics(ctx.state);
    let order: Vec<u32> = m.net.bt_roster().iter().map(|p| p.lap).collect();
    let oldest_ms = m
        .net
        .bt_hops
        .back()
        .map_or(0, |h| h.seen.elapsed().as_millis() as u64);
    match key.code {
        KeyCode::Up => m.net.bt_view.move_by(&order, -1),
        KeyCode::Down => m.net.bt_view.move_by(&order, 1),
        KeyCode::Char('+') | KeyCode::Char('=') => m.net.hop_view.zoom_in(),
        KeyCode::Char('-') => m.net.hop_view.zoom_out(),
        KeyCode::Left => m.net.hop_view.back(oldest_ms),
        KeyCode::Right => m.net.hop_view.forward(),
        KeyCode::End => m.net.hop_view.back_ms = 0,
        _ => {
            drop(m);
            return global::handle(key, ctx);
        }
    }
    KeyAction::Continue
}

/// The BLE packet list: the arrows move the cursor through the packets in the
/// order the panel draws them (`NetState::ble_shown`), newest first; `Enter`
/// opens a CONNECT_IND's connection on LE 3, and on any other packet narrows
/// the list to its address and back; `t` narrows it to one kind of PDU in
/// turn and back to all; `h` holds the list and lets it run again; `p` switches the PHY (it shadows the next
/// layout only while the list is focused, as the census shadows `s` and
/// `r`). Anything else goes on to the global keys.
pub(super) fn net_ble_packets(key: KeyEvent, ctx: &mut InputCtx<'_>) -> KeyAction {
    let mut m = metrics(ctx.state);
    let order: Vec<u64> = m.net.ble_shown().iter().map(|p| p.seq).collect();
    match key.code {
        KeyCode::Up => m.net.ble_view.selection.move_by(&order, -1),
        KeyCode::Down => m.net.ble_view.selection.move_by(&order, 1),
        // On a CONNECT_IND, its connection, on LE 3; on any other packet,
        // only its advertiser, or all again. A CONNECT_IND carries no
        // advertiser address first, so it never had a filter to lose.
        KeyCode::Enter => match selected_connection(&m) {
            Some(aa) => {
                let followed = m
                    .net
                    .ble_connections
                    .iter()
                    .any(|f| f.connection.access_address() == aa);
                if !followed {
                    m.push_log(
                        "BLE list: that connection is not followed (its CONNECT_IND failed its CRC, or it was let go)",
                    );
                    return KeyAction::Continue;
                }
                m.net.connection_view = crate::state::ConnectionView {
                    selected: Some(aa),
                    first_visible: 0,
                };
                drop(m);
                return global::presets::try_set_preset(ctx.engine, ctx.state, "net_connection");
            }
            None => filter_to_selected(&mut m),
        },
        KeyCode::Char('t') => {
            m.net.ble_view.kind = crate::state::PduKind::step(m.net.ble_view.kind);
        }
        // `h` is the spectrum's hold everywhere else; no NET layout shows a
        // spectrum, and holding a list is the same idea.
        KeyCode::Char('h') => {
            let held = match m.net.ble_view.held.take() {
                Some(_) => None,
                None => Some((m.net.ble_packets.clone(), m.net.ble_heard)),
            };
            m.net.ble_view.held = held;
        }
        _ => {
            drop(m);
            return global::handle(key, ctx);
        }
    }
    KeyAction::Continue
}

/// The LE Coded list: the arrows move the cursor through the packets in the
/// order the panel draws them (`NetState::coded_shown`), newest first; `h`
/// holds the list and lets it run again, as LE 2's does. Anything else goes on
/// to the global keys.
pub(super) fn net_coded_packets(key: KeyEvent, ctx: &mut InputCtx<'_>) -> KeyAction {
    let mut m = metrics(ctx.state);
    let order: Vec<u64> = m.net.coded_shown().iter().map(|p| p.seq).collect();
    match key.code {
        KeyCode::Up => m.net.coded_view.selection.move_by(&order, -1),
        KeyCode::Down => m.net.coded_view.selection.move_by(&order, 1),
        KeyCode::Char('h') => {
            let held = match m.net.coded_view.held.take() {
                Some(_) => None,
                None => Some((m.net.coded_packets.clone(), m.net.coded_heard)),
            };
            m.net.coded_view.held = held;
        }
        _ => {
            drop(m);
            return global::handle(key, ctx);
        }
    }
    KeyAction::Continue
}

/// The access address of the connection the selected packet set up, when it
/// is a CONNECT_IND.
fn selected_connection(m: &SdrMetrics) -> Option<u32> {
    let selected = m.net.ble_view.selection.selected?;
    m.net
        .ble_shown()
        .into_iter()
        .find(|p| p.seq == selected)
        .filter(|p| p.pdu_type == crate::signal::ble::pdu::PduType::ConnectInd)
        .and_then(|p| crate::signal::ble::connect::decode_octets(&p.payload))
        .map(|c| c.access_address)
}

/// `Enter` on the packet list: show only the selected packet's advertiser, or,
/// when already filtered, everything again. A packet that carries no
/// advertiser address (a SCAN_REQ) has nothing to filter by, and the log
/// says so; a CONNECT_IND opens its connection instead (`net_ble_packets`).
fn filter_to_selected(m: &mut SdrMetrics) {
    if m.net.ble_view.filter.take().is_some() {
        return;
    }
    let selected = m.net.ble_view.selection.selected;
    let address = m
        .net
        .ble_shown()
        .into_iter()
        .find(|p| Some(p.seq) == selected)
        .map(|p| p.adv_addr);
    match address {
        None => m.push_log("BLE list: select a packet first"),
        Some(None) => m.push_log("BLE list: that packet carries no advertiser address"),
        Some(Some(a)) => {
            m.net.ble_view.filter = Some(a);
            m.net.ble_view.selection.reset_view();
        }
    }
}

/// A device selected in the census arrives in the BLE packet list with the
/// list already narrowed to it: the same filter
/// `Enter` sets there, from the census's own selection. Called on a switch
/// from a layout showing the census to one showing the list, so leaving the
/// census is what carries the choice; a filter cleared in the list stays
/// cleared until the census is visited again.
///
/// Nothing selected carries nothing, and leaves whatever filter the list
/// had. The log says it happened, the address shown as the section shows
/// addresses, because a list that arrives narrowed without a word reads as
/// a quiet room.
pub(super) fn carry_census_selection(m: &mut SdrMetrics) {
    // The selection while the census is still focused; the choice it left
    // behind once focus ended and cleared the selection. Spent either way, so an old choice
    // does not come back on a later visit that chose nothing.
    let Some(address) = m
        .net
        .census
        .selection
        .selected
        .or(m.net.census.chosen.take())
    else {
        return;
    };
    m.net.census.chosen = None;
    let random = m
        .net
        .census
        .devices
        .iter()
        .find(|d| d.address == address)
        .is_some_and(|d| d.random);
    m.net.ble_view.filter = Some(address);
    m.net.ble_view.selection.reset_view();
    let shown = m.net.show_address(address, random, None);
    m.push_log(format!(
        "BLE list: only {shown}, as selected in the census (Enter shows all)"
    ));
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
    // `T` on the device the reference rests on lets it go: a wrong choice
    // used to stay applied for up to fifteen minutes, with nothing but the
    // expiry to end it. Every tag reads [RELATIVE] again on the next frame.
    let is_reference = device.is_some_and(|d| {
        m.radio
            .reference
            .as_ref()
            .is_some_and(|r| r.trusted == Some(d.address))
    });
    if is_reference {
        let name = device
            .map(|d| d.address_text(&m.net, None))
            .unwrap_or_default();
        m.radio.reference = None;
        m.push_log(format!(
            "Reference: {name} no longer trusted; offsets are relative again"
        ));
        return;
    }
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
        assert_eq!(r.trusted, Some([0xa4, 0x83, 0xe7, 0x1c, 0x09, 1]));
        drop(m);

        // `T` again on the same device lets it go, at once and logged, with
        // no entry opened.
        trust_selected(&mut metrics(&state));
        {
            let m = metrics(&state);
            assert!(m.radio.reference.is_none());
            assert!(m.ui.input_mode == InputMode::Normal);
            assert!(m
                .ui
                .log
                .iter()
                .any(|l| l.text.contains("no longer trusted")));
        }

        // On another device, with a reference in place, it asks to replace it
        // rather than clearing it.
        trust_selected(&mut metrics(&state));
        type_(KeyCode::Char('2'));
        type_(KeyCode::Enter);
        {
            let mut m = metrics(&state);
            m.net.census.devices[1].crystal_offset_ppm = Some(Uncertain::from_sigma(4.0, 0.3));
            m.net.census.selection.selected = Some([0xa4, 0x83, 0xe7, 0x1c, 0x09, 2]);
        }
        trust_selected(&mut metrics(&state));
        let m = metrics(&state);
        assert!(
            m.radio.reference.is_some(),
            "kept until a new figure is typed"
        );
        assert!(matches!(
            m.ui.input_mode,
            InputMode::ReferenceAccuracyInput { .. }
        ));
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

    /// **`Enter` narrows to the selected packet's address and widens again;
    /// `h` holds the list and lets it run**, both through the handler.
    #[test]
    fn enter_filters_and_h_holds_the_packet_list() {
        let mut m = SdrMetrics::fixture().streaming();
        for seq in 1..=3u64 {
            m.net.ble_packets.push_front(crate::state::BlePacket {
                seq,
                adv_addr: Some([seq as u8; 6]),
                ..sample_packet()
            });
        }
        m.net.ble_heard = 3;
        m.net.ble_view.selection.selected = Some(2);
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

        press(KeyCode::Enter);
        assert_eq!(metrics(&state).net.ble_view.filter, Some([2; 6]));
        assert_eq!(metrics(&state).net.ble_shown().len(), 1);
        press(KeyCode::Enter);
        assert_eq!(metrics(&state).net.ble_view.filter, None);

        press(KeyCode::Char('h'));
        assert!(metrics(&state).net.ble_view.held.is_some());
        {
            let mut m = metrics(&state);
            m.net.ble_heard = 4;
            m.net.ble_packets.push_front(crate::state::BlePacket {
                phy: crate::signal::ble::Phy::OneM,
                seq: 4,
                ..sample_packet()
            });
            assert_eq!(m.net.ble_shown().len(), 3, "the held copy");
            assert_eq!(m.net.ble_behind(), 1);
        }
        press(KeyCode::Char('h'));
        assert!(metrics(&state).net.ble_view.held.is_none());
        assert_eq!(metrics(&state).net.ble_shown().len(), 4, "live again");
    }

    /// **`t` steps the list through the kinds of PDU and back to all**, and
    /// the kind and the address narrow it together.
    #[test]
    fn t_steps_the_packet_list_through_the_kinds() {
        use crate::signal::ble::pdu::PduType;
        use crate::state::PduKind;
        let mut m = SdrMetrics::fixture().streaming();
        let types = [
            PduType::AdvInd,
            PduType::ScanReq,
            PduType::ScanRsp,
            PduType::ConnectInd,
            PduType::AdvNonconnInd,
            PduType::AdvDirectInd,
            PduType::AdvScanInd,
            PduType::Other(0x7),
        ];
        for (i, pdu_type) in types.into_iter().enumerate() {
            m.net.ble_packets.push_front(crate::state::BlePacket {
                seq: i as u64 + 1,
                pdu_type,
                adv_addr: Some([i as u8 + 1; 6]),
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
        let shown = |s: &Arc<Mutex<SdrMetrics>>| -> Vec<u64> {
            metrics(s).net.ble_shown().iter().map(|p| p.seq).collect()
        };
        let kind = |s: &Arc<Mutex<SdrMetrics>>| metrics(s).net.ble_view.kind;

        assert_eq!(shown(&state).len(), 8);
        press(KeyCode::Char('t'));
        assert_eq!(kind(&state), Some(PduKind::Connect));
        assert_eq!(shown(&state), vec![4]);
        press(KeyCode::Char('t'));
        assert_eq!(kind(&state), Some(PduKind::Scan));
        assert_eq!(shown(&state), vec![3, 2]);
        press(KeyCode::Char('t'));
        assert_eq!(kind(&state), Some(PduKind::Advertising));
        assert_eq!(shown(&state), vec![7, 6, 5, 1]);

        // With an address as well: both have to hold.
        metrics(&state).net.ble_view.filter = Some([1; 6]);
        assert_eq!(shown(&state), vec![1]);
        metrics(&state).net.ble_view.filter = None;

        press(KeyCode::Char('t'));
        assert_eq!(kind(&state), None);
        assert_eq!(shown(&state).len(), 8);
    }

    /// A packet with no advertiser address has nothing to filter by.
    #[test]
    fn filtering_on_a_packet_without_an_address_says_why() {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.ble_packets.push_front(crate::state::BlePacket {
            phy: crate::signal::ble::Phy::OneM,
            seq: 1,
            adv_addr: None,
            ..sample_packet()
        });
        m.net.ble_view.selection.selected = Some(1);
        filter_to_selected(&mut m);
        assert_eq!(m.net.ble_view.filter, None);
        assert!(m
            .ui
            .log
            .iter()
            .any(|l| l.text.contains("no advertiser address")));
    }

    /// A CONNECT_IND's payload: InitA, AdvA, then LLData with `aa`.
    fn connect_payload(aa: u32) -> Vec<u8> {
        let mut p = vec![
            0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0xa1, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6,
        ];
        p.extend(aa.to_le_bytes());
        p.extend([
            0x7c, 0x5b, 0x3a, 1, 0, 0, 6, 0, 0, 0, 100, 0, 0xff, 0xff, 0xff, 0xff, 0x1f, 7,
        ]);
        p
    }

    /// A state with two connections followed, newest first, and the
    /// CONNECT_IND of `aa` selected in the BLE list.
    fn with_connections(state: &Arc<Mutex<SdrMetrics>>, selected_aa: u32) {
        let mut m = metrics(state);
        let now = Instant::now();
        for aa in [0x1111_1111u32, 0x2222_2222] {
            let c = crate::signal::ble::connect::decode_octets(&connect_payload(aa)).unwrap();
            m.net
                .follow(&c, (Some(false), false, false), 0.0, 20e6, now);
        }
        m.net.ble_packets.push_front(crate::state::BlePacket {
            seq: 1,
            pdu_type: crate::signal::ble::pdu::PduType::ConnectInd,
            length: 34,
            adv_addr: None,
            payload: connect_payload(selected_aa),
            ..sample_packet()
        });
        m.net.ble_heard = 1;
        m.net.ble_view.selection.selected = Some(1);
    }

    /// `Enter` on a followed CONNECT_IND opens LE 3 on its connection.
    #[test]
    fn enter_on_a_connect_ind_opens_its_connection() {
        let (mut engine, keys, state) = focused_on("net_ble", "net_ble_packets");
        with_connections(&state, 0x1111_1111);
        key(&mut engine, &keys, &state, KeyCode::Enter);
        assert_eq!(engine.active_preset(), "net_connection");
        let m = metrics(&state);
        assert_eq!(m.net.connection_view.selected, Some(0x1111_1111));
        assert_eq!(m.net.ble_view.filter, None, "not a filter");
    }

    /// A CONNECT_IND whose connection is not followed (its CRC failed, or
    /// it was let go) says so and stays.
    #[test]
    fn enter_on_an_unfollowed_connect_ind_says_so() {
        let (mut engine, keys, state) = focused_on("net_ble", "net_ble_packets");
        with_connections(&state, 0x3333_3333);
        key(&mut engine, &keys, &state, KeyCode::Enter);
        assert_eq!(engine.active_preset(), "net_ble");
        assert!(metrics(&state)
            .ui
            .log
            .iter()
            .any(|l| l.text.contains("not followed")));
    }

    /// `← →` on LE 3 step through the connections followed, newest first,
    /// each from its newest event.
    #[test]
    fn the_arrows_step_between_connections() {
        let (mut engine, keys, state) = focused_on("net_connection", "net_ble_connection");
        with_connections(&state, 0x1111_1111);
        metrics(&state).net.connection_view.first_visible = 3;
        key(&mut engine, &keys, &state, KeyCode::Right);
        let view = metrics(&state).net.connection_view;
        assert_eq!(
            view.selected,
            Some(0x1111_1111),
            "from the newest to the next"
        );
        assert_eq!(view.first_visible, 0);
        key(&mut engine, &keys, &state, KeyCode::Right);
        assert_eq!(
            metrics(&state).net.connection_view.selected,
            Some(0x1111_1111),
            "the last stays"
        );
        key(&mut engine, &keys, &state, KeyCode::Left);
        assert_eq!(
            metrics(&state).net.connection_view.selected,
            Some(0x2222_2222)
        );
    }

    fn sample_packet() -> crate::state::BlePacket {
        crate::state::BlePacket {
            phy: crate::signal::ble::Phy::OneM,
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
            coded: None,
            ext: None,
        }
    }

    /// **The scatter's keys** (6.2): `+`/`-` step the zoom and stop at the
    /// ends, `←` moves back a quarter window but no further than the oldest
    /// hit kept, `End` returns to now, and `↓` selects the piconet the
    /// roster shows first.
    #[test]
    fn the_hop_scatter_zooms_scrubs_and_selects() {
        let state = Arc::new(Mutex::new(SdrMetrics::fixture()));
        {
            let mut m = metrics(&state);
            let seen = Instant::now() - Duration::from_secs(12);
            crate::signal::bt::piconet::observe(&mut m.net.bt_piconets, 0x5a3c71, 10, seen);
            m.net.bt_hops.push_back(crate::state::BtHop {
                channel: 10,
                lap: 0x5a3c71,
                seen,
                at_us: 0.0,
                stream: 0,
                header: None,
            });
        }
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
            net_bt_hops(KeyEvent::new(code, KeyModifiers::NONE), &mut ctx);
        };
        let view = |s: &Arc<Mutex<SdrMetrics>>| metrics(s).net.hop_view;

        press(KeyCode::Char('+'));
        assert_eq!(view(&state).span_ms(), 10_000);
        for _ in 0..9 {
            press(KeyCode::Char('+'));
        }
        assert_eq!(view(&state).span_ms(), 500, "stops at the shortest");
        for _ in 0..3 {
            press(KeyCode::Char('-'));
        }
        assert_eq!(view(&state).span_ms(), 5_000);

        for _ in 0..20 {
            press(KeyCode::Left);
        }
        let back = view(&state).back_ms;
        assert!(
            (11_900..=12_500).contains(&back),
            "held at the oldest hit: {back}"
        );
        press(KeyCode::End);
        assert_eq!(view(&state).back_ms, 0);

        press(KeyCode::Down);
        assert_eq!(metrics(&state).net.bt_view.selected, Some(0x5a3c71));
    }

    /// A deck built as the app builds it, on `preset`, with `panel` focused
    /// the way its letter focuses it.
    fn focused_on(
        preset: &str,
        panel: &str,
    ) -> (LayoutEngine, crate::app::FocusKeys, Arc<Mutex<SdrMetrics>>) {
        let (mut engine, keys) = crate::app::App::build_ui(preset, &HashMap::new(), None, true);
        engine.focus(panel);
        let state = Arc::new(Mutex::new(SdrMetrics::fixture().streaming()));
        metrics(&state).ui.focused_panel = Some(panel.to_string());
        (engine, keys, state)
    }

    fn key(
        engine: &mut LayoutEngine,
        keys: &crate::app::FocusKeys,
        state: &Arc<Mutex<SdrMetrics>>,
        code: KeyCode,
    ) {
        let mut show_footer = true;
        crate::app::input::handle_key(
            KeyEvent::new(code, KeyModifiers::NONE),
            state,
            None,
            engine,
            &mut show_footer,
            keys,
        );
    }

    /// The Connection view scrolls through the rows its events make, no
    /// further than the last, and `End` goes back to the newest.
    #[test]
    fn the_connection_view_scrolls_its_events() {
        let (mut engine, keys, state) = focused_on("net_connection", "net_ble_connection");
        {
            let mut m = metrics(&state);
            let c = crate::signal::ble::connect::ConnectIndData {
                init_a: [0; 6],
                adv_a: [0; 6],
                access_address: 0x5065_4b6a,
                crc_init: 0x3a_5b7c,
                win_size: 1,
                win_offset: 0,
                interval: 6,
                latency: 0,
                timeout: 100,
                channel_map: (1u64 << 37) - 1,
                hop_increment: 7,
                sca: 0,
            };
            let now = std::time::Instant::now();
            m.net
                .follow(&c, (Some(false), false, false), 0.0, 20e6, now);
            // Missed, so each is a row of its own (a run out of view is one).
            for _ in 0..3 {
                m.net.ble_connections[0]
                    .connection
                    .account(crate::signal::ble::follow::Listened::Yes, Vec::new());
            }
        }
        for _ in 0..5 {
            key(&mut engine, &keys, &state, KeyCode::Down);
        }
        assert_eq!(
            metrics(&state).net.connection_view.first_visible,
            2,
            "three rows"
        );
        key(&mut engine, &keys, &state, KeyCode::Up);
        assert_eq!(metrics(&state).net.connection_view.first_visible, 1);
        key(&mut engine, &keys, &state, KeyCode::End);
        assert_eq!(metrics(&state).net.connection_view.first_visible, 0);
    }

    /// **Leaving focus leaves no trace**:
    /// every NET position goes on `Esc`, every mode chosen on purpose stays.
    #[test]
    fn leaving_focus_clears_every_position_and_keeps_every_mode() {
        // A layout, the panel focused on it, what puts a position (and a
        // mode) there, and whether leaving cleared the one and kept the other.
        type Case = (
            &'static str,
            &'static str,
            fn(&mut SdrMetrics),
            fn(&SdrMetrics) -> bool,
        );
        let cases: [Case; 7] = [
            (
                "net_survey",
                "net_occupancy",
                |m| m.net.band_cursor.selected = Some(37),
                |m| m.net.band_cursor.selected.is_none(),
            ),
            (
                "net_survey",
                "net_coexist",
                |m| m.net.band_scrub = Some(12),
                |m| m.net.band_scrub.is_none(),
            ),
            (
                "net_census",
                "net_census",
                |m| {
                    m.net.census.selection.selected = Some([1; 6]);
                    m.net.census.sort = 3;
                },
                // The sort is a mode and stays.
                |m| m.net.census.selection.selected.is_none() && m.net.census.sort == 3,
            ),
            (
                "net_ble",
                "net_ble_packets",
                |m| {
                    m.net.ble_view.selection.selected = Some(7);
                    m.net.ble_view.filter = Some([2; 6]);
                },
                // The filter is a mode and stays.
                |m| {
                    m.net.ble_view.selection.selected.is_none()
                        && m.net.ble_view.filter == Some([2; 6])
                },
            ),
            (
                "net_bt",
                "net_bt_hops",
                |m| {
                    m.net.bt_view.selected = Some(0x5a3c71);
                    m.net.hop_view.back_ms = 8_000;
                    m.net.hop_view.zoom = 2;
                },
                // Where the window ends goes; the zoom stays.
                |m| {
                    m.net.bt_view.selected.is_none()
                        && m.net.hop_view.back_ms == 0
                        && m.net.hop_view.zoom == 2
                },
            ),
            (
                "net_bt",
                "net_bt_piconets",
                |m| m.net.bt_view.selected = Some(0x5a3c71),
                |m| m.net.bt_view.selected.is_none(),
            ),
            (
                "net_piconet",
                "net_bt_packets",
                |m| {
                    m.net.bt_view.selected = Some(0x5a3c71);
                    m.net.packets_view.first_visible = 12;
                    m.net.packets_view.held = Some((1, 5_000.0));
                },
                // The scroll goes; the piconet is what the view is of, and
                // the hold is a mode, as the BLE list's is.
                |m| {
                    m.net.packets_view.first_visible == 0
                        && m.net.packets_view.held == Some((1, 5_000.0))
                        && m.net.bt_view.selected == Some(0x5a3c71)
                },
            ),
        ];
        for (preset, panel, set, cleared) in cases {
            let (mut engine, keys, state) = focused_on(preset, panel);
            set(&mut metrics(&state));
            key(&mut engine, &keys, &state, KeyCode::Esc);
            assert!(
                engine.focused_panel_name().is_none(),
                "{panel} still focused"
            );
            assert!(cleared(&metrics(&state)), "{panel} kept a position");
        }
    }

    /// Classic 2 with two piconets heard, the first with `n` packets kept, one
    /// a slot apart on stream 1, and the packet list focused.
    fn piconet_view(n: u32) -> (LayoutEngine, crate::app::FocusKeys, Arc<Mutex<SdrMetrics>>) {
        use crate::signal::bt::piconet::{observe, observe_packet, BtPacket, PayloadVerdict};
        let (engine, keys, state) = focused_on("net_piconet", "net_bt_packets");
        {
            let mut m = metrics(&state);
            m.net.bt_channels_watched = (60..80).collect();
            observe(&mut m.net.bt_piconets, 0xc3_d318, 73, Instant::now());
            observe(
                &mut m.net.bt_piconets,
                0xfe_17f1,
                71,
                Instant::now() - Duration::from_secs(5),
            );
            for k in 0..n {
                observe_packet(
                    &mut m.net.bt_piconets,
                    0xc3_d318,
                    BtPacket {
                        seen: Instant::now(),
                        at_us: k as f64 * 625.0,
                        stream: 1,
                        channel: 73,
                        header: None,
                        direction: None,
                        deviation: Default::default(),
                        carrier: Default::default(),
                        f0_ppm: None,
                        payload: PayloadVerdict::NoPayload,
                        content: None,
                    },
                );
            }
            m.net.bt_view.selected = Some(0xc3_d318);
        }
        (engine, keys, state)
    }

    /// The packet list's keys: `↓` holds the list where it is and scrolls
    /// into the past, `↑` back up, `h` holds or lets it run, `End` returns
    /// to live at the top.
    #[test]
    fn the_packet_list_scrolls_holds_and_returns_to_live() {
        let (mut engine, keys, state) = piconet_view(30);
        let view = |s: &Arc<Mutex<SdrMetrics>>| metrics(s).net.packets_view;

        key(&mut engine, &keys, &state, KeyCode::Up);
        assert_eq!(view(&state).first_visible, 0, "nothing above the top");
        assert_eq!(view(&state).held, None, "and still live");

        key(&mut engine, &keys, &state, KeyCode::Down);
        key(&mut engine, &keys, &state, KeyCode::Down);
        assert_eq!(view(&state).first_visible, 2);
        assert_eq!(
            view(&state).held,
            Some((1, 29.0 * 625.0)),
            "held at the newest, so the rows do not slide under the reader"
        );
        key(&mut engine, &keys, &state, KeyCode::Up);
        assert_eq!(view(&state).first_visible, 1);
        for _ in 0..40 {
            key(&mut engine, &keys, &state, KeyCode::Down);
        }
        assert_eq!(view(&state).first_visible, 29, "no further than the oldest");

        key(&mut engine, &keys, &state, KeyCode::End);
        assert_eq!(view(&state), Default::default(), "live, at the top");

        key(&mut engine, &keys, &state, KeyCode::Char('h'));
        assert_eq!(view(&state).held, Some((1, 29.0 * 625.0)));
        key(&mut engine, &keys, &state, KeyCode::Char('h'));
        assert_eq!(view(&state), Default::default());
    }

    /// `l` switches to the piconet's LMP log and back, each time from live
    /// at the top; `End` goes back to live without leaving the log; `↓` in
    /// the log holds it at its newest message; `← →` keep the log on.
    #[test]
    fn l_switches_to_the_lmp_log_and_keeps_it() {
        let (mut engine, keys, state) = piconet_view(30);
        {
            let mut m = metrics(&state);
            let p = &mut m.net.bt_piconets[0];
            assert_eq!(p.lap, 0xc3_d318);
            let older: Vec<_> = p.packets.iter().skip(10).take(5).cloned().collect();
            p.lmp.extend(older);
        }
        let view = |s: &Arc<Mutex<SdrMetrics>>| metrics(s).net.packets_view;
        key(&mut engine, &keys, &state, KeyCode::Down);
        key(&mut engine, &keys, &state, KeyCode::Char('l'));
        assert_eq!(
            view(&state),
            crate::state::PacketsView {
                lmp_only: true,
                ..Default::default()
            },
            "the log, from live at the top"
        );

        key(&mut engine, &keys, &state, KeyCode::Down);
        key(&mut engine, &keys, &state, KeyCode::Down);
        let newest_lmp = 19.0 * 625.0;
        assert_eq!(
            view(&state).held,
            Some((1, newest_lmp)),
            "held at the log's newest"
        );
        for _ in 0..10 {
            key(&mut engine, &keys, &state, KeyCode::Down);
        }
        assert_eq!(
            view(&state).first_visible,
            4,
            "no further than the log's oldest"
        );

        key(&mut engine, &keys, &state, KeyCode::End);
        assert_eq!(
            view(&state),
            crate::state::PacketsView {
                lmp_only: true,
                ..Default::default()
            },
            "live, still the log"
        );

        key(&mut engine, &keys, &state, KeyCode::Right);
        assert!(view(&state).lmp_only, "the next piconet's log");
        key(&mut engine, &keys, &state, KeyCode::Left);
        key(&mut engine, &keys, &state, KeyCode::Char('l'));
        assert_eq!(view(&state), Default::default(), "every packet again");
    }

    /// `← →` step through the piconets in the roster's order, the view
    /// starting afresh on each; with no piconet heard they do nothing, and
    /// never step the radio from under the list.
    #[test]
    fn the_arrows_step_through_the_piconets() {
        let (mut engine, keys, state) = piconet_view(30);
        metrics(&state).net.packets_view.first_visible = 4;
        key(&mut engine, &keys, &state, KeyCode::Right);
        assert_eq!(metrics(&state).net.bt_view.selected, Some(0xfe_17f1));
        assert_eq!(metrics(&state).net.packets_view, Default::default());
        key(&mut engine, &keys, &state, KeyCode::Right);
        assert_eq!(
            metrics(&state).net.bt_view.selected,
            Some(0xfe_17f1),
            "stops at the end"
        );
        key(&mut engine, &keys, &state, KeyCode::Left);
        assert_eq!(metrics(&state).net.bt_view.selected, Some(0xc3_d318));

        let (mut engine, keys, state) = focused_on("net_piconet", "net_bt_packets");
        {
            let mut m = metrics(&state);
            m.net.mode = crate::state::NetMode::Lock;
            m.net.bt_view.selected = Some(0x12_3456);
        }
        key(&mut engine, &keys, &state, KeyCode::Right);
        let m = metrics(&state);
        assert_eq!(m.net.bt_view.selected, Some(0x12_3456), "left as it was");
        assert!(m.net.lock_at.is_none(), "the radio did not move");
    }

    /// Unfocused, `← →` step a locked radio in Classic 2 exactly as in Classic 1:
    /// the same classic receiver, the same block.
    #[test]
    fn the_piconet_view_steps_the_radio_as_the_classic_view_does() {
        let step = |preset: &str| {
            let (mut engine, keys) = crate::app::App::build_ui(preset, &HashMap::new(), None, true);
            let state = Arc::new(Mutex::new(SdrMetrics::fixture().streaming()));
            {
                let mut m = metrics(&state);
                m.ui.active_preset = preset.to_string();
                m.ui.section = crate::signal::net::SECTION.to_string();
                m.net.mode = crate::state::NetMode::Lock;
                m.radio.frequency = 2_440_000_000;
                m.radio.config_sample_rate = 20e6;
                m.radio.bb_filter_hz = 0;
                m.net.bt_capacity = 4;
            }
            key(&mut engine, &keys, &state, KeyCode::Right);
            let m = metrics(&state);
            m.net.lock_at.as_ref().map(|t| t.tune_hz)
        };
        assert_eq!(step("net_bt"), Some(2_444_000_000));
        assert_eq!(step("net_piconet"), step("net_bt"));
    }

    /// `Enter` on the Classic view's roster opens the Piconet view on the
    /// piconet selected there, the selection carried as it is.
    #[test]
    fn enter_opens_the_piconet_view() {
        let (mut engine, keys, state) = focused_on("net_bt", "net_bt_piconets");
        crate::signal::bt::piconet::observe(
            &mut metrics(&state).net.bt_piconets,
            0xc3_d318,
            73,
            Instant::now(),
        );
        key(&mut engine, &keys, &state, KeyCode::Down);
        assert_eq!(metrics(&state).net.bt_view.selected, Some(0xc3_d318));
        key(&mut engine, &keys, &state, KeyCode::Enter);
        assert_eq!(engine.active_preset(), "net_piconet");
        assert_eq!(metrics(&state).net.bt_view.selected, Some(0xc3_d318));

        // Nothing selected: nothing to open, and the log says so.
        let (mut engine, keys, state) = focused_on("net_bt", "net_bt_piconets");
        key(&mut engine, &keys, &state, KeyCode::Enter);
        assert_eq!(engine.active_preset(), "net_bt");
        assert!(metrics(&state)
            .ui
            .log
            .iter()
            .any(|l| l.text.contains("select a piconet")));
    }

    /// `s` walks the roster's columns and `r` reverses, as on the Census;
    /// the arrows then step through the rows as they are drawn, and the
    /// selected piconet stays selected.
    #[test]
    fn s_and_r_order_the_roster_and_the_arrows_follow() {
        let (mut engine, keys, state) = focused_on("net_bt", "net_bt_piconets");
        let now = Instant::now();
        for (lap, ago) in [(0x30_0000, 1), (0x10_0000, 9), (0x20_0000, 5)] {
            crate::signal::bt::piconet::observe(
                &mut metrics(&state).net.bt_piconets,
                lap,
                73,
                now - std::time::Duration::from_secs(ago),
            );
        }
        // By LAP: the first row is the lowest.
        key(&mut engine, &keys, &state, KeyCode::Down);
        assert_eq!(metrics(&state).net.bt_view.selected, Some(0x10_0000));
        metrics(&state).net.bt_view.first_visible = 2;

        key(&mut engine, &keys, &state, KeyCode::Char('s'));
        assert_eq!(metrics(&state).net.bt_sort.key(), "KIND");
        assert_eq!(
            metrics(&state).net.bt_view.first_visible,
            0,
            "back to the top"
        );
        assert_eq!(metrics(&state).net.bt_view.selected, Some(0x10_0000));

        key(&mut engine, &keys, &state, KeyCode::Char('s'));
        assert_eq!(metrics(&state).net.bt_sort.key(), "LAST");
        // Youngest first: 0x30, 0x20, 0x10; the cursor on 0x10, the last.
        key(&mut engine, &keys, &state, KeyCode::Up);
        assert_eq!(metrics(&state).net.bt_view.selected, Some(0x20_0000));

        key(&mut engine, &keys, &state, KeyCode::Char('r'));
        assert!(metrics(&state).net.bt_sort.descending);
        // Oldest first now: 0x10, 0x20, 0x30.
        key(&mut engine, &keys, &state, KeyCode::Down);
        assert_eq!(metrics(&state).net.bt_view.selected, Some(0x30_0000));
    }

    /// The Classic view's selection carries into the Piconet view however
    /// the view is changed, as the census's carries into the BLE list: the
    /// Piconet view is the selected piconet's, and arriving on nothing
    /// because a focus ended on the way would be arriving at the wrong view.
    #[test]
    fn the_classic_selection_carries_into_the_piconet_view() {
        let (mut engine, _keys, state) = focused_on("net_bt", "net_bt_piconets");
        metrics(&state).net.bt_view.selected = Some(0xc3_d318);
        super::global::presets::try_set_preset(&mut engine, &state, "net_piconet");
        assert_eq!(engine.active_preset(), "net_piconet");
        assert_eq!(metrics(&state).net.bt_view.selected, Some(0xc3_d318));

        // And into the Bench, from either.
        let (mut engine, _keys, state) = focused_on("net_bt", "net_bt_piconets");
        metrics(&state).net.bt_view.selected = Some(0xc3_d318);
        super::global::presets::try_set_preset(&mut engine, &state, "net_bench");
        assert_eq!(engine.active_preset(), "net_bench");
        assert_eq!(metrics(&state).net.bt_view.selected, Some(0xc3_d318));
    }

    /// **The one selection that outlives its focus**: a census device chosen,
    /// `Esc`, then the BLE layout, still arrives filtered to it; the choice is
    /// spent, so a later visit that chose nothing carries nothing.
    #[test]
    fn a_census_choice_still_carries_after_leaving_focus() {
        let device = [0xa4, 0x83, 0xe7, 0x1c, 0x09, 0xbe];
        let (mut engine, keys, state) = focused_on("net_census", "net_census");
        metrics(&state).net.census.selection.selected = Some(device);
        key(&mut engine, &keys, &state, KeyCode::Esc);
        assert_eq!(metrics(&state).net.census.selection.selected, None);
        super::global::presets::try_set_preset(&mut engine, &state, "net_ble");
        assert_eq!(metrics(&state).net.ble_view.filter, Some(device));

        metrics(&state).net.ble_view.filter = None;
        super::global::presets::try_set_preset(&mut engine, &state, "net_census");
        super::global::presets::try_set_preset(&mut engine, &state, "net_ble");
        assert_eq!(metrics(&state).net.ble_view.filter, None, "spent");
    }

    /// A layout switch that takes the focused panel off screen ends its
    /// focus the same way `Esc` does; the carry reads the selection first.
    #[test]
    fn switching_layout_ends_a_focus_it_hides() {
        let device = [1, 2, 3, 4, 5, 6];
        let (mut engine, _, state) = focused_on("net_census", "net_census");
        metrics(&state).net.census.selection.selected = Some(device);
        super::global::presets::try_set_preset(&mut engine, &state, "net_ble");
        assert!(engine.focused_panel_name().is_none());
        assert_eq!(metrics(&state).ui.focused_panel, None);
        assert_eq!(metrics(&state).net.ble_view.filter, Some(device));
        assert_eq!(metrics(&state).net.census.selection.selected, None);
    }

    /// **Every focusable panel is accounted for**: its name is either an arm
    /// of `reset_positions` or on the `NO_POSITION` list, read from the
    /// source the way the registry tests read the dispatch. A new panel with
    /// a cursor cannot be added without saying what leaving it resets.
    #[test]
    fn every_focusable_panel_says_what_leaving_it_resets() {
        let dispatch = include_str!("mod.rs");
        let view = include_str!("global/view.rs");
        let resets = &view[view.find("fn reset_positions").unwrap()..];
        // Reset whatever is focused, not by arm.
        let always = ["spectrum", "waterfall"];
        let focusable: Vec<&str> = dispatch
            .lines()
            .filter_map(|l| l.trim().strip_prefix("Some(\""))
            .filter_map(|l| l.split('"').next())
            .collect();
        assert!(focusable.len() >= 15, "{focusable:?}");
        for name in focusable {
            let armed = resets.contains(&format!("\"{name}\""));
            let listed = super::global::view::NO_POSITION.contains(&name);
            assert!(
                armed || listed || always.contains(&name),
                "{name}: neither reset on leaving focus nor listed as having no position"
            );
            assert!(
                !(armed && listed),
                "{name} is both reset and listed as positionless"
            );
        }
    }

    /// **A census choice travels with the user** (5.9): leaving the census
    /// for the BLE layout narrows the packet list to the selected device;
    /// arriving from anywhere else, or with nothing selected, leaves the
    /// list as it was. Through the one function every layout switch takes.
    #[test]
    fn a_device_selected_in_the_census_arrives_in_the_list_filtered_to_it() {
        let (mut engine, _) = crate::app::App::build_ui("net_census", &HashMap::new(), None, true);
        let device = [0xa4, 0x83, 0xe7, 0x1c, 0x09, 0xbe];
        let state = Arc::new(Mutex::new(SdrMetrics::fixture()));
        metrics(&state).net.census.selection.selected = Some(device);
        let switch = |engine: &mut LayoutEngine, to: &str| {
            super::global::presets::try_set_preset(engine, &state, to);
        };

        switch(&mut engine, "net_ble");
        assert_eq!(metrics(&state).net.ble_view.filter, Some(device));
        assert!(metrics(&state)
            .ui
            .log
            .iter()
            .any(|l| l.text.contains("as selected in the census")));

        // Cleared in the list, it stays cleared on a switch that does not
        // come from the census.
        metrics(&state).net.ble_view.filter = None;
        switch(&mut engine, "net_survey");
        switch(&mut engine, "net_ble");
        assert_eq!(metrics(&state).net.ble_view.filter, None);

        // Nothing selected carries nothing, and leaves the list's own filter.
        let other = [1, 2, 3, 4, 5, 6];
        metrics(&state).net.ble_view.filter = Some(other);
        metrics(&state).net.census.selection.selected = None;
        switch(&mut engine, "net_census");
        switch(&mut engine, "net_ble");
        assert_eq!(metrics(&state).net.ble_view.filter, Some(other));
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
