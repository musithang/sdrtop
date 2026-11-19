// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The census table's own view state, and the one condition an empty
//! census is read by.

use super::NetState;

/// How the census table is being read: what orders it, and where the cursor is.
///
/// The cursor is an address, not a row number - see [`crate::state::Selection`],
/// which the census was the first to need and which every other NET list now
/// shares.
#[derive(Clone, Debug, Default)]
pub struct CensusState {
    /// The population, unordered.
    ///
    /// **Stored as found, ordered when drawn.** Keeping it sorted would mean
    /// re-sorting on every packet for a question only the panel asks, and would
    /// put the display's choice of column into the place the measurements live.
    pub devices: Vec<crate::signal::net::census::Device>,
    /// Index into `signal::net::census::SORT_KEYS`.
    pub sort: usize,
    pub descending: bool,
    /// The device the cursor is on, and where the view starts.
    pub selection: crate::state::Selection<[u8; 6]>,
    /// The device selected when the census last lost focus, kept for one
    /// thing only: a layout switch into the BLE list carries it as the
    /// list's filter (`input::net::carry_census_selection`). Leaving focus
    /// clears the selection itself, as it clears every cursor, and the carry
    /// reads this instead; it is spent by the carry that uses it.
    pub chosen: Option<[u8; 6]>,
}

impl CensusState {
    /// The next column along, wrapping.
    ///
    /// One key, cycling, because the sort key is *shown* rather than
    /// remembered - and a control the panel advertises in
    /// one direction is one the user can use without being told twice.
    pub fn cycle_sort(&mut self) {
        let keys = crate::signal::net::census::SORT_KEYS.len().max(1);
        self.sort = (self.sort + 1) % keys;
        // Back to the top of the new column rather than wherever the last one
        // left the view: the rows underneath are different rows now.
        self.selection.reset_view();
    }

    pub fn reverse(&mut self) {
        self.descending = !self.descending;
        self.selection.reset_view();
    }

    /// The population in the order the table shows it.
    ///
    /// **The one ordering both the panel and its keys read.** The cursor moves
    /// through rows as drawn; if the key handler ordered the census on its own
    /// terms, or not at all, the cursor would step through a list nobody can
    /// see. It once did exactly that: the handler moved through an empty list,
    /// so the arrows never selected anything.
    pub fn ordered(
        &self,
        now: std::time::Instant,
        radio: &crate::state::RadioState,
    ) -> Vec<crate::signal::net::census::Device> {
        let mut devices = self.devices.clone();
        crate::signal::net::census::order(&mut devices, self.sort, self.descending, now, radio);
        devices
    }

    /// The addresses of [`Self::ordered`], which is what the cursor moves
    /// through.
    pub fn ordered_addresses(
        &self,
        now: std::time::Instant,
        radio: &crate::state::RadioState,
    ) -> Vec<[u8; 6]> {
        self.ordered(now, radio).iter().map(|d| d.address).collect()
    }
}

impl NetState {
    /// Whether anything is reading addresses into the census: a BLE decoder
    /// with a channel, or one that has fired this session.
    ///
    /// **The one condition an empty census is read by**, so the panel's empty
    /// state, the decode-health panel's dashes and the export's note cannot
    /// disagree about whether an empty table means a quiet room or nobody
    /// counting.
    pub fn counting_addresses(&self) -> bool {
        self.ble_channel.is_some() || self.health.ble.triggered > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A new column or a reversed one puts different rows under the view, so
    /// the view goes back to the top - but the device the cursor is on is
    /// still that device, and stays selected.
    #[test]
    fn a_resort_resets_the_view_and_keeps_the_device() {
        let device = [0xa4, 0x83, 0xe7, 0x1c, 0x09, 0x01];
        let mut c = CensusState::default();
        c.selection.move_by(&[device], 1);
        c.selection.first_visible = 5;
        c.cycle_sort();
        assert_eq!(c.selection.first_visible, 0);
        assert_eq!(c.selection.selected, Some(device));
        c.selection.first_visible = 5;
        c.reverse();
        assert_eq!(c.selection.first_visible, 0);
        assert_eq!(c.selection.selected, Some(device));
    }

    #[test]
    fn cycling_the_sort_key_walks_the_columns_and_comes_back() {
        let keys = crate::signal::net::census::SORT_KEYS.len();
        let mut c = CensusState::default();
        assert_eq!(c.sort, 0);
        for expected in 1..keys {
            c.cycle_sort();
            assert_eq!(c.sort, expected);
        }
        c.cycle_sort();
        assert_eq!(c.sort, 0, "and round again");

        // Reversing is a separate act from choosing the column.
        assert!(!c.descending);
        c.reverse();
        assert!(c.descending);
        assert_eq!(c.sort, 0, "reversing does not move the column");
    }
}
