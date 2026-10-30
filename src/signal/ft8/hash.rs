// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The calls heard in full, so that a call sent only as a hash can be named.
//!
//! The paper (section 2): a receiver shows a hashed call as the most recently
//! decoded call that maps to the same code, in angle brackets, and as `<...>`
//! when it has none. A hash is never inverted and never guessed; the book only
//! remembers what was heard, and a collision is resolved the paper's way, the
//! latest call winning.

use std::collections::HashMap;

use super::fields::hashes;

/// How many bits of hash a field carried.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HashBits {
    H10,
    H12,
    H22,
}

#[derive(Clone, Debug, Default)]
pub struct HashBook {
    calls: HashMap<(HashBits, u32), String>,
}

impl HashBook {
    pub fn new() -> Self {
        Self::default()
    }

    /// Remember a call heard in full under all three of its hashes. Text that
    /// cannot be hashed (not of the call alphabet, too long) is ignored.
    pub fn learn(&mut self, call: &str) {
        if let Some(h) = hashes(call) {
            self.calls.insert((HashBits::H10, h.h10), call.to_string());
            self.calls.insert((HashBits::H12, h.h12), call.to_string());
            self.calls.insert((HashBits::H22, h.h22), call.to_string());
        }
    }

    /// The call last heard with this hash, if any.
    pub fn lookup(&self, bits: HashBits, value: u32) -> Option<&str> {
        self.calls.get(&(bits, value)).map(String::as_str)
    }

    pub fn clear(&mut self) {
        self.calls.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_heard_call_answers_to_each_of_its_hashes() {
        let mut book = HashBook::new();
        assert_eq!(book.lookup(HashBits::H22, 1_420_834), None);
        book.learn("PJ4/K1ABC");
        assert_eq!(book.lookup(HashBits::H10, 346), Some("PJ4/K1ABC"));
        assert_eq!(book.lookup(HashBits::H12, 1387), Some("PJ4/K1ABC"));
        assert_eq!(book.lookup(HashBits::H22, 1_420_834), Some("PJ4/K1ABC"));
        // The same number at another width is another hash.
        assert_eq!(book.lookup(HashBits::H12, 346), None);
    }

    #[test]
    fn the_latest_call_wins_a_collision_and_clear_forgets() {
        let mut book = HashBook::new();
        book.learn("K1ABC");
        let h = hashes("K1ABC").unwrap();
        // Force a collision by hand: another call stored under the same code.
        book.calls
            .insert((HashBits::H12, h.h12), "W9XYZ".to_string());
        assert_eq!(book.lookup(HashBits::H12, h.h12), Some("W9XYZ"));
        book.learn("K1ABC");
        assert_eq!(book.lookup(HashBits::H12, h.h12), Some("K1ABC"));
        book.clear();
        assert_eq!(book.lookup(HashBits::H12, h.h12), None);
    }
}
