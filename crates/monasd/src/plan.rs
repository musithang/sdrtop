// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! What a headless run will be, decided before it starts.
//!
//! [`plan`] decides, on plain data, what a headless run will be (where it
//! listens, whether that needs a token, what it computes) and refuses with the
//! reason anything it cannot do, before a radio is touched.

use std::net::SocketAddr;

use crate::config::MonasdSettings;
use crate::engine::command::{Command, PAGER_ADDRESS_MAX};
use crate::engine::mode::{Mode, MODES};
use crate::state::AddressDisplay;

/// The port the API listens on when none is named.
pub const DEFAULT_PORT: u16 = 7373;

/// What a headless run will be.
#[derive(Clone, Debug, PartialEq)]
pub struct HeadlessPlan {
    pub listen: SocketAddr,
    pub token: Option<String>,
    pub mode: Mode,
    /// Whether the spectrum is computed. Off on the 2.4 GHz views, where it
    /// would cost a core at the rates they run at and nothing there reads it.
    pub spectrum: bool,
    /// The display switches to set before anything is served: how addresses
    /// are shown, and the pager's mask and own address.
    pub display: Vec<Command>,
}

/// Decide a headless run from the flags (`listen`, `mode`), which win, and
/// the `[monasd]` block. The mode is one of [`MODES`], by name. Refused, with
/// the reason, when it could not run as asked.
pub fn plan(
    listen: Option<&str>,
    mode: Option<&str>,
    settings: &MonasdSettings,
) -> Result<HeadlessPlan, String> {
    let listen_text = listen
        .map(str::to_string)
        .or_else(|| settings.listen.clone())
        .unwrap_or_else(|| format!("127.0.0.1:{DEFAULT_PORT}"));
    let listen: SocketAddr = listen_text
        .parse()
        .map_err(|_| format!("{listen_text} is not an address to listen on (host:port)"))?;
    let token = settings.token.clone().filter(|t| !t.is_empty());
    if !listen.ip().is_loopback() && token.is_none() {
        return Err(format!(
            "listening on {listen} lets anyone who reaches it retune the radio; \
             put a token in the config's [monasd] block (token = \"some long random string\"), \
             or listen on 127.0.0.1"
        ));
    }

    let name = mode
        .map(str::to_string)
        .or_else(|| settings.mode.clone())
        .unwrap_or_else(|| "spectrum".to_string());
    let Some(mode) = Mode::named(&name) else {
        let names: Vec<&str> = MODES.iter().map(|(n, _)| *n).collect();
        return Err(format!(
            "there is no mode called {name}; the modes are {}",
            names.join(", ")
        ));
    };
    let mut display = Vec::new();
    if let Some(show) = &settings.addresses {
        if AddressDisplay::named(show).is_none() {
            let names: Vec<&str> = AddressDisplay::ALL.iter().map(|m| m.label()).collect();
            return Err(format!(
                "addresses = \"{show}\" in [monasd] is no address mode; the modes are {}",
                names.join(", ")
            ));
        }
        display.push(Command::Addresses { show: show.clone() });
    }
    if let Some(ric) = settings.pager_mine.filter(|&r| r > PAGER_ADDRESS_MAX) {
        return Err(format!(
            "pager_mine = {ric} in [monasd] is not a pager address: an address is 21 bits, \
             at most {PAGER_ADDRESS_MAX}"
        ));
    }
    if settings.pager_masked.is_some() || settings.pager_mine.is_some() {
        display.push(Command::Pager {
            masked: settings.pager_masked.unwrap_or(false),
            mine: settings.pager_mine,
        });
    }
    Ok(HeadlessPlan {
        listen,
        token,
        spectrum: !mode.feeds.net,
        mode,
        display,
    })
}

/// Which of the radios found to run on: the one, when there is one. With no
/// one at a terminal to choose, several are refused, by name.
pub fn pick(labels: &[String]) -> Result<usize, String> {
    match labels {
        [_] => Ok(0),
        _ => Err(format!(
            "{} radios found ({}); say which with --device",
            labels.len(),
            labels.join(", ")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MonasdSettings;

    #[test]
    fn listen_defaults_to_loopback() {
        let plan = plan(None, None, &MonasdSettings::default()).unwrap();
        assert!(plan.listen.ip().is_loopback(), "{}", plan.listen);
        assert_eq!(plan.listen.port(), DEFAULT_PORT);
        assert_eq!(plan.token, None);
    }

    /// Off loopback the radio can be retuned by anyone who reaches the port,
    /// so a token is required, and the refusal says how to give one.
    #[test]
    fn a_non_loopback_address_without_a_token_is_refused_with_how_to_make_one() {
        let why = plan(Some("0.0.0.0:7373"), None, &MonasdSettings::default()).unwrap_err();
        assert!(why.contains("[monasd]") && why.contains("token"), "{why}");

        let with_token = MonasdSettings {
            token: Some("a-long-random-string".into()),
            ..MonasdSettings::default()
        };
        let plan = plan(Some("0.0.0.0:7373"), None, &with_token).unwrap();
        assert!(!plan.listen.ip().is_loopback());
    }

    /// The display switches in the file are set before anything is served;
    /// one that names no mode, or no address, stops the start with why.
    #[test]
    fn the_display_switches_are_planned_or_refused_at_the_start() {
        let file = MonasdSettings {
            addresses: Some("masked".into()),
            pager_masked: Some(true),
            ..MonasdSettings::default()
        };
        let plan = plan(None, None, &file).unwrap();
        assert_eq!(
            plan.display,
            vec![
                Command::Addresses {
                    show: "masked".into()
                },
                Command::Pager {
                    masked: true,
                    mine: None
                },
            ]
        );
        assert!(super::plan(None, None, &MonasdSettings::default())
            .unwrap()
            .display
            .is_empty());
        let wrong = MonasdSettings {
            addresses: Some("secret".into()),
            ..MonasdSettings::default()
        };
        let why = super::plan(None, None, &wrong).unwrap_err();
        assert!(why.contains("full, oui, masked"), "{why}");
        let wide = MonasdSettings {
            pager_mine: Some(PAGER_ADDRESS_MAX + 1),
            ..MonasdSettings::default()
        };
        assert!(super::plan(None, None, &wide)
            .unwrap_err()
            .contains("21 bits"));
    }

    #[test]
    fn an_address_that_is_not_one_is_refused() {
        let why = plan(Some("here"), None, &MonasdSettings::default()).unwrap_err();
        assert!(why.contains("here"), "{why}");
    }

    /// A mode nobody knows computes nothing, so it is refused, with the ones
    /// that exist.
    #[test]
    fn an_unknown_mode_is_refused_with_the_list_of_modes() {
        let why = plan(None, Some("net_surveyy"), &MonasdSettings::default()).unwrap_err();
        assert!(why.contains("net_surveyy"), "{why}");
        assert!(
            why.contains("net_survey,") || why.contains("net_survey "),
            "{why}"
        );
    }

    /// A known mode is the one the terminal asks for showing the view of
    /// that name; the flag wins over the file.
    #[test]
    fn a_mode_is_what_the_terminal_asks_for_by_that_name() {
        let file = MonasdSettings {
            mode: Some("radio_pocsag".into()),
            ..MonasdSettings::default()
        };
        let plan = plan(None, Some("net_survey"), &file).unwrap();
        assert_eq!(
            plan.mode,
            crate::engine::mode::Mode::for_view(monas::signal::net::SECTION, "net_survey")
        );
    }

    /// No mode named: the plain spectrum, the one thing every radio does.
    #[test]
    fn no_mode_named_is_the_spectrum() {
        let plan = plan(None, None, &MonasdSettings::default()).unwrap();
        assert_eq!(plan.mode.view, "spectrum");
        assert!(plan.spectrum);
    }

    /// With no one at a terminal to ask, one radio is the one; several are
    /// refused with their names, so `--device` can say which.
    #[test]
    fn several_radios_are_refused_with_their_names() {
        assert_eq!(pick(&["HackRF One".to_string()]), Ok(0));
        let why = pick(&["HackRF One".to_string(), "RTL-SDR".to_string()]).unwrap_err();
        assert!(
            why.contains("HackRF One") && why.contains("RTL-SDR"),
            "{why}"
        );
        assert!(why.contains("--device"), "{why}");
    }
}
