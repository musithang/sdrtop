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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LayoutConfig;
    use crate::signal::net::lock::{
        ADVERTISING_VIEWS, CLASSIC_VIEWS, CODED_VIEWS, SURVEY_VIEW, WIFI_VISIT_VIEWS,
    };
    use crate::state::UiState;

    /// **Moved, not rewritten.** Every built-in layout, with the section it
    /// declares, asks the engine for exactly what the old gates read off the
    /// screen asked for. If a gate and its mode ever disagree, a view stops
    /// being fed or starts costing a core, and nothing on screen says which.
    #[test]
    fn for_view_agrees_with_the_old_gates_on_every_built_in_preset() {
        let layouts = LayoutConfig::default_config();
        assert!(
            layouts.presets.len() > 20,
            "the built-in layouts were not found"
        );
        for (preset, cfg) in &layouts.presets {
            let section = cfg.section.clone().unwrap_or_default();
            let ui = UiState {
                section: section.clone(),
                active_preset: preset.clone(),
                ..UiState::default()
            };
            let mode = Mode::for_view(&section, preset);
            let at = format!("{preset} in {section}");
            assert_eq!(mode.section, section, "{at}");
            assert_eq!(mode.view, *preset, "{at}");
            assert_eq!(mode.feeds.net, ui.is_net_section(), "{at}: net feed");
            assert_eq!(mode.feeds.pager, ui.is_pager_section(), "{at}: pager feed");
            assert_eq!(mode.feeds.ft8, ui.is_ft8_section(), "{at}: ft8 feed");
            assert_eq!(mode.ft8_lab, ui.is_ft8_lab(), "{at}: ft8 lab");
            let p = preset.as_str();
            assert_eq!(
                mode.net.classic,
                CLASSIC_VIEWS.contains(&p),
                "{at}: classic"
            );
            assert_eq!(mode.net.survey, p == SURVEY_VIEW, "{at}: survey");
            assert_eq!(mode.net.coded, CODED_VIEWS.contains(&p), "{at}: coded");
            assert_eq!(
                mode.net.wifi,
                crate::signal::net::is_wifi(&section),
                "{at}: wifi"
            );
            assert_eq!(
                mode.net.wifi_visits,
                WIFI_VISIT_VIEWS.contains(&p),
                "{at}: wifi visits"
            );
            assert_eq!(
                mode.net.advertising,
                ADVERTISING_VIEWS.contains(&p),
                "{at}: advertising"
            );
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
