// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! A cursor that remembers an item, not a row.
//!
//! Every NET list that can be read closely - the census, the BLE packet feed,
//! the classic piconet roster, the Survey instrument's cells - needs the same
//! cursor, and the census had already found the one rule that matters: **rows
//! move**. A re-sort reorders them, a live population reorders them anyway as
//! counts change, and a new arrival at the front shifts everything below it. A
//! cursor stored as an index slides onto whichever item lands in its row,
//! which reads as the user's own mistake. So the cursor is the item's key, and
//! its row is looked up in whatever ordering is on screen at the moment.
//!
//! **No selection is a state, not a default of row zero.** Before the first
//! key press nothing is selected, and a panel must not draw a highlight on a
//! row nobody chose. The first move enters the list from the direction it was
//! going: down lands on the first row, up on the last.
//!
//! The key type is the panel's own identity for an item: an address for the
//! census, a LAP for the piconet roster, a cell index for the Survey band.

/// Where the cursor is, as the item it is on, and where the view starts.
#[derive(Clone, Debug)]
pub struct Selection<K> {
    /// The item the cursor is on. `None` until something has been selected.
    pub selected: Option<K>,
    /// Where the viewport starts, so a long list does not jump under the
    /// cursor. Reset whenever the ordering changes underneath it.
    pub first_visible: usize,
}

impl<K> Default for Selection<K> {
    fn default() -> Self {
        Self {
            selected: None,
            first_visible: 0,
        }
    }
}

impl<K: Copy + PartialEq> Selection<K> {
    /// Move `delta` rows through `ordered`, and remember the item rather than
    /// the position. Stops at the ends rather than wrapping; an empty list
    /// leaves nothing selected.
    pub fn move_by(&mut self, ordered: &[K], delta: isize) {
        if ordered.is_empty() {
            self.selected = None;
            return;
        }
        let at = self
            .cursor(ordered)
            .map(|i| i as isize)
            .unwrap_or(if delta < 0 {
                ordered.len() as isize
            } else {
                -1
            });
        let next = (at + delta).clamp(0, ordered.len() as isize - 1) as usize;
        self.selected = Some(ordered[next]);
    }

    /// The row the cursor is on in this ordering. `None` when nothing is
    /// selected, or when the item it was on has gone - which is an answer,
    /// not an error: that item is no longer in the list.
    pub fn cursor(&self, ordered: &[K]) -> Option<usize> {
        let k = self.selected?;
        ordered.iter().position(|x| *x == k)
    }

    /// The ordering changed underneath the cursor: start the view at the top
    /// of the new one. The selected item is kept; it is still the same item.
    pub fn reset_view(&mut self) {
        self.first_visible = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The one that matters: the cursor stays on the item, not the row.**
    #[test]
    fn the_cursor_follows_the_item_through_a_reordering() {
        let by_count = [3u8, 1, 2];
        let by_name = [1u8, 2, 3];
        let mut s = Selection::default();

        s.move_by(&by_count, 1);
        assert_eq!(s.selected, Some(3), "the first row of this ordering");
        assert_eq!(s.cursor(&by_count), Some(0));

        // Re-sorted: the cursor is on the same item, further down.
        assert_eq!(s.cursor(&by_name), Some(2));

        // An item that has gone leaves the cursor nowhere rather than on
        // whatever took its place.
        assert_eq!(s.cursor(&[1, 2]), None);
    }

    /// A new arrival at the front shifts every row down by one; the cursor
    /// must not slide onto the neighbour that now occupies its old row.
    #[test]
    fn a_new_arrival_at_the_front_does_not_move_the_cursor() {
        let before = [10u8, 20, 30];
        let mut s = Selection::default();
        s.move_by(&before, 1);
        s.move_by(&before, 1);
        assert_eq!(s.selected, Some(20));
        let after = [5u8, 10, 20, 30];
        assert_eq!(s.cursor(&after), Some(2), "still on 20, one row lower");
        s.move_by(&after, 1);
        assert_eq!(s.selected, Some(30), "and moving continues from there");
    }

    #[test]
    fn the_cursor_stops_at_the_ends_rather_than_wrapping() {
        let rows = [1u8, 2, 3];
        let mut s = Selection::default();
        s.move_by(&rows, 1);
        assert_eq!(s.selected, Some(1));
        for _ in 0..5 {
            s.move_by(&rows, 1);
        }
        assert_eq!(s.selected, Some(3), "the bottom, and it stays there");
        for _ in 0..5 {
            s.move_by(&rows, -1);
        }
        assert_eq!(s.selected, Some(1), "and the top");
    }

    /// Nothing is selected until a key is pressed, and the first move enters
    /// the list from the direction it was going.
    #[test]
    fn the_first_move_enters_from_the_direction_it_was_going() {
        let rows = [1u8, 2, 3];
        let s: Selection<u8> = Selection::default();
        assert_eq!(s.cursor(&rows), None, "no row is selected by default");

        let mut down = Selection::default();
        down.move_by(&rows, 1);
        assert_eq!(down.selected, Some(1));

        let mut up = Selection::default();
        up.move_by(&rows, -1);
        assert_eq!(up.selected, Some(3));
    }

    /// When the selected item has gone, the next move re-enters the list
    /// rather than doing nothing.
    #[test]
    fn a_gone_item_is_left_by_re_entering_the_list() {
        let mut s = Selection::default();
        s.move_by(&[7u8, 8], 1);
        assert_eq!(s.selected, Some(7));
        s.move_by(&[8u8, 9], 1);
        assert_eq!(s.selected, Some(8), "down from nowhere is the first row");
    }

    #[test]
    fn an_empty_list_has_nothing_to_be_on() {
        let mut s = Selection::default();
        s.move_by(&[1u8], 1);
        s.move_by(&[] as &[u8], 1);
        assert_eq!(s.selected, None);
    }

    #[test]
    fn resetting_the_view_keeps_the_selection() {
        let mut s = Selection::default();
        s.move_by(&[1u8, 2], 1);
        s.first_visible = 7;
        s.reset_view();
        assert_eq!(s.first_visible, 0);
        assert_eq!(s.selected, Some(1));
    }
}
