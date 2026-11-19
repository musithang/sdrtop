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

/// Wi-Fi's menu sections, one per question its views answer: what the
/// channel is doing, what the room's radio path is like, who is on the
/// network, and how good one transmitter is. Filed apart from the Bluetooth
/// ones, so the two benches never share a menu group, and admitted by the
/// same band gate plus Wi-Fi's own rate (`gate::wifi_verdict`).
///
/// Prefixed rather than bare (`channel`), so a user preset filed under a
/// word of its own does not land in Wi-Fi's tree by accident.
pub const WIFI_SECTIONS: [&str; 4] = [
    "wifi_channel",
    "wifi_room",
    "wifi_network",
    "wifi_transmitter",
];

/// Every section of this feature, Bluetooth's then Wi-Fi's: for the places
/// that need one static list of them, the footer's key rows above all. Held
/// to the two lists it repeats by a test.
pub const ALL_SECTIONS: [&str; 8] = [
    SECTION,
    "le",
    "classic",
    "coded",
    "wifi_channel",
    "wifi_room",
    "wifi_network",
    "wifi_transmitter",
];

/// Whether a menu section is one of Wi-Fi's.
pub fn is_wifi(section: &str) -> bool {
    WIFI_SECTIONS.contains(&section)
}

/// Whether a menu section is one of this feature's, Bluetooth's or Wi-Fi's.
pub fn is_net(section: &str) -> bool {
    SECTIONS.contains(&section) || is_wifi(section)
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
pub mod wifi;
pub mod worker;

#[cfg(test)]
mod tests {
    use super::*;

    /// The Wi-Fi groups are NET sections, so everything gated on one (the
    /// header, the footer's keys, the startup filter) holds under Wi-Fi; and
    /// they are Wi-Fi's, never Bluetooth's.
    #[test]
    fn is_net_covers_the_wifi_sections() {
        for s in WIFI_SECTIONS {
            assert!(is_net(s), "{s}");
            assert!(is_wifi(s), "{s}");
        }
        for s in SECTIONS {
            assert!(is_net(s), "{s}");
            assert!(!is_wifi(s), "{s}");
        }
        assert!(!is_net("lab"));
        assert!(!is_wifi("wifi"));
    }

    /// The combined list is the two lists, Bluetooth's first, and nothing
    /// else: a section added to either cannot be missed by it.
    #[test]
    fn all_sections_is_bluetooth_then_wifi() {
        let both: Vec<&str> = SECTIONS.iter().chain(&WIFI_SECTIONS).copied().collect();
        assert_eq!(ALL_SECTIONS.to_vec(), both);
    }
}
