// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The NET section's panel handlers.

use crossterm::event::{KeyCode, KeyEvent};

use super::{global, metrics, InputCtx, KeyAction};

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
}
