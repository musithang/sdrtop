// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The NET feature's own layer: what is true of the 2.4 GHz band and of the
//! radio pointed at it, above any one protocol.
//!
//! Dependencies run one way. `dsp` knows no protocol; the protocol arcs know
//! only `dsp`; this module sits above them and aggregates. It is also where the
//! questions live that no single protocol can answer, and [`gate`] is the first
//! of them: **can this radio do any of this at all?**

/// The menu sections this feature's presets are filed under: the band's own,
/// then each Bluetooth's, LE Coded's apart from LE's because it has a receive
/// chain of its own. One radio requirement admits or refuses all four.
///
/// Named here rather than in the menu because several unrelated places need to
/// agree on them: the section table, the startup path that drops the sections
/// when the gate refuses, the header that renders differently inside them, and
/// the footer keys that only mean something there. A string literal in each
/// would be that many chances to disagree.
pub const SECTIONS: [&str; 4] = [SECTION, "le", "classic", "coded"];

/// The band's own section (Capability, Survey), and the one a test sets when
/// it only needs "a NET view".
pub const SECTION: &str = "net";

/// Whether a menu section is one of this feature's.
pub fn is_net(section: &str) -> bool {
    SECTIONS.contains(&section)
}

pub mod band;
pub mod census;
#[cfg(test)]
pub mod conformance;
pub mod gate;
pub mod listen;
pub mod lock;
pub mod measure;
pub mod occupancy;
pub mod scan;
pub mod survey;
pub mod vendor;
pub mod worker;
