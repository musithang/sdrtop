// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The NET feature's own layer: what is true of the 2.4 GHz band and of the
//! radio pointed at it, above any one protocol.
//!
//! Dependencies run one way. `dsp` knows no protocol; the protocol arcs know
//! only `dsp`; this module sits above them and aggregates. It is also where the
//! questions live that no single protocol can answer, and [`gate`] is the first
//! of them: **can this radio do any of this at all?**

/// The menu section id this feature's presets are filed under.
///
/// Named here rather than in the menu because three unrelated places need to
/// agree on it: the section table, the startup path that drops the section when
/// the gate refuses, and the header that renders differently inside it. A string
/// literal in each would be three chances to disagree.
pub const SECTION: &str = "net";

pub mod band;
pub mod census;
#[cfg(test)]
pub mod conformance;
pub mod gate;
pub mod lock;
pub mod occupancy;
pub mod scan;
pub mod survey;
pub mod vendor;
pub mod worker;
