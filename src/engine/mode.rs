// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! What the engine is computing right now, and therefore what it feeds.
//!
//! The feeds off the radio's thread each cost a copy of every block, so a
//! decoder nobody is reading must cost nothing. That used to be decided by
//! reading the section and the layout on screen straight out of the shared
//! state, which is right while there is a screen and meaningless without one.
//! The mode is the same decision held by the engine: the terminal sets it from
//! the layout it shows, and a headless instance sets it from what it was
//! started to do.
//!
//! **One rule, two readers.** [`Mode::for_view`] answers with the same
//! primitives the screen uses to describe itself (`signal::net::is_net`,
//! `is_wifi`, the view lists in `signal::net::lock`, the pager and FT8
//! section names), and a test holds it to the screen's own answers on every
//! built-in layout.

use serde::Serialize;

use crate::signal::net::lock::{
    ADVERTISING_VIEWS, CLASSIC_VIEWS, CODED_VIEWS, SURVEY_VIEW, WIFI_VISIT_VIEWS,
};

/// Which sample feeds run besides the spectrum's and the demodulator's, each
/// a full copy of every block while it is on.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize)]
pub struct Feeds {
    /// The 2.4 GHz receivers, Bluetooth and Wi-Fi.
    pub net: bool,
    /// The pager decoder.
    pub pager: bool,
    /// The FT8 decoder.
    pub ft8: bool,
}

/// What the 2.4 GHz receivers are asked to do, once their feed is on.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize)]
pub struct NetView {
    /// The classic Bluetooth receivers.
    pub classic: bool,
    /// The band's occupancy, measured dwell by dwell.
    pub survey: bool,
    /// The LE Coded receiver.
    pub coded: bool,
    /// Wi-Fi rather than Bluetooth.
    pub wifi: bool,
    /// Wi-Fi's channel visits, which need the band scan.
    pub wifi_visits: bool,
    /// The three LE advertising channels.
    pub advertising: bool,
}

/// What the engine is computing. See the module comment.
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub struct Mode {
    /// The section that asked for it, by the name the layouts use.
    pub section: String,
    /// The view that asked for it, by its layout name: what a recording and
    /// an export say they were made from.
    pub view: String,
    pub feeds: Feeds,
    pub net: NetView,
    /// The FT8 lab rather than the FT8 world view.
    pub ft8_lab: bool,
}

impl Mode {
    /// The mode a view in a section asks for.
    pub fn for_view(section: &str, view: &str) -> Mode {
        let net = crate::signal::net::is_net(section);
        Mode {
            section: section.to_string(),
            view: view.to_string(),
            feeds: Feeds {
                net,
                pager: section == crate::signal::pocsag::SECTION,
                ft8: crate::signal::ft8::SECTIONS.contains(&section),
            },
            net: NetView {
                classic: CLASSIC_VIEWS.contains(&view),
                survey: view == SURVEY_VIEW,
                coded: CODED_VIEWS.contains(&view),
                wifi: crate::signal::net::is_wifi(section),
                wifi_visits: WIFI_VISIT_VIEWS.contains(&view),
                advertising: ADVERTISING_VIEWS.contains(&view),
            },
            ft8_lab: section == crate::signal::ft8::LAB_SECTION,
        }
    }

    /// Nothing asked for: no feed beyond what the spectrum and the
    /// demodulator ask for on their own.
    pub fn idle() -> Mode {
        Mode::for_view("", "")
    }
    /// The mode called `name` in [`MODES`], or `None` for a name Monas does
    /// not know.
    pub fn named(name: &str) -> Option<Mode> {
        MODES
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(n, section)| Mode::for_view(section, n))
    }
}

/// What Monas can compute, by the name a client asks for it with, and the
/// section that name belongs to: the plain spectrum, and every view that asks
/// for a feed of its own. Named as the terminal's views are, so a script and
/// the person at the terminal mean the same thing by `net_survey`. A view
/// that asks for nothing beyond the spectrum (the Lab benches, the micro
/// views) needs no name of its own here: `spectrum` computes what it shows.
pub const MODES: &[(&str, &str)] = &[
    ("spectrum", ""),
    ("net", crate::signal::net::SECTION),
    ("net_survey", crate::signal::net::SECTION),
    ("net_ble", "le"),
    ("net_census", "le"),
    ("net_connection", "le"),
    ("net_coded", "coded"),
    ("net_bt", "classic"),
    ("net_piconet", "classic"),
    ("net_bench", "classic"),
    ("wifi_survey", "wifi_channel"),
    ("wifi_frames", "wifi_channel"),
    ("wifi_airtime", "wifi_channel"),
    ("radio_pocsag", crate::signal::pocsag::SECTION),
    ("radio_ft8", crate::signal::ft8::SECTION),
    ("lab_ft8_signal", crate::signal::ft8::LAB_SECTION),
    ("lab_ft8_band", crate::signal::ft8::LAB_SECTION),
    ("lab_ft8_receiver", crate::signal::ft8::LAB_SECTION),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Every named mode is the mode its view asks for in its own section.
    #[test]
    fn a_named_mode_is_its_view_in_its_section() {
        for (name, section) in MODES {
            assert_eq!(
                Mode::named(name),
                Some(Mode::for_view(section, name)),
                "{name}"
            );
        }
        assert_eq!(Mode::named("not_a_view"), None);
    }

    /// The plain spectrum asks for no feed of its own; every other named
    /// mode asks for something, or it would not need a name.
    #[test]
    fn only_the_spectrum_is_named_without_a_feed() {
        for (name, _) in MODES {
            let mode = Mode::named(name).unwrap();
            let idle =
                mode.feeds == Feeds::default() && mode.net == NetView::default() && !mode.ft8_lab;
            assert_eq!(idle, *name == "spectrum", "{name}");
        }
    }

    /// Nothing asked for, nothing fed.
    #[test]
    fn idle_feeds_nothing() {
        let idle = Mode::idle();
        assert_eq!(idle.feeds, Feeds::default());
        assert_eq!(idle.net, NetView::default());
        assert!(!idle.ft8_lab);
    }
}
