// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! What a headless run will be, decided before it starts.
//!
//! [`plan`] decides, on plain data, what a headless run will be (where it
//! listens, whether that needs a token, what it computes) and refuses with the
//! reason anything it cannot do, before a radio is touched.

use std::net::SocketAddr;

use super::mode::Mode;
use crate::config::{HeadlessSettings, LayoutConfig};

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
}

/// Decide a headless run from the flags (`listen`, `view`), which win, and
/// the `[headless]` block. Refused, with the reason, when it could not run as
/// asked.
pub fn plan(
    listen: Option<&str>,
    view: Option<&str>,
    settings: &HeadlessSettings,
    layouts: &LayoutConfig,
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
             put a token in the config's [headless] block (token = \"some long random string\"), \
             or listen on 127.0.0.1"
        ));
    }

    let view = view
        .map(str::to_string)
        .or_else(|| settings.mode.clone())
        .unwrap_or_else(|| "spectrum".to_string());
    let Some(preset) = layouts.presets.get(&view) else {
        let mut views: Vec<&str> = layouts.presets.keys().map(String::as_str).collect();
        views.sort_unstable();
        return Err(format!(
            "there is no view called {view}; the views are {}",
            views.join(", ")
        ));
    };
    let mode = Mode::for_view(preset.section.as_deref().unwrap_or_default(), &view);
    Ok(HeadlessPlan {
        listen,
        token,
        spectrum: !mode.feeds.net,
        mode,
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
    use crate::config::{HeadlessSettings, LayoutConfig};

    fn layouts() -> LayoutConfig {
        LayoutConfig::default_config()
    }

    #[test]
    fn listen_defaults_to_loopback() {
        let plan = plan(None, None, &HeadlessSettings::default(), &layouts()).unwrap();
        assert!(plan.listen.ip().is_loopback(), "{}", plan.listen);
        assert_eq!(plan.listen.port(), DEFAULT_PORT);
        assert_eq!(plan.token, None);
    }

    /// Off loopback the radio can be retuned by anyone who reaches the port,
    /// so a token is required, and the refusal says how to give one.
    #[test]
    fn a_non_loopback_address_without_a_token_is_refused_with_how_to_make_one() {
        let why = plan(
            Some("0.0.0.0:7373"),
            None,
            &HeadlessSettings::default(),
            &layouts(),
        )
        .unwrap_err();
        assert!(why.contains("[headless]") && why.contains("token"), "{why}");

        let with_token = HeadlessSettings {
            token: Some("a-long-random-string".into()),
            ..HeadlessSettings::default()
        };
        let plan = plan(Some("0.0.0.0:7373"), None, &with_token, &layouts()).unwrap();
        assert!(!plan.listen.ip().is_loopback());
    }

    #[test]
    fn an_address_that_is_not_one_is_refused() {
        let why = plan(Some("here"), None, &HeadlessSettings::default(), &layouts()).unwrap_err();
        assert!(why.contains("here"), "{why}");
    }

    /// A view nobody knows computes nothing, so it is refused, with the ones
    /// that exist.
    #[test]
    fn an_unknown_mode_is_refused_with_the_list_of_views() {
        let why = plan(
            None,
            Some("net_surveyy"),
            &HeadlessSettings::default(),
            &layouts(),
        )
        .unwrap_err();
        assert!(why.contains("net_surveyy"), "{why}");
        assert!(
            why.contains("net_survey,") || why.contains("net_survey "),
            "{why}"
        );
    }

    /// A known view gets the mode the terminal would ask for showing it; the
    /// flag wins over the file.
    #[test]
    fn a_view_asks_for_the_mode_the_terminal_would() {
        let file = HeadlessSettings {
            mode: Some("radio_pocsag".into()),
            ..HeadlessSettings::default()
        };
        let plan = plan(None, Some("net_survey"), &file, &layouts()).unwrap();
        assert_eq!(
            plan.mode,
            crate::engine::mode::Mode::for_view(crate::signal::net::SECTION, "net_survey")
        );
    }

    /// No view named: the plain spectrum, the one thing every radio does.
    #[test]
    fn no_view_named_is_the_spectrum() {
        let plan = plan(None, None, &HeadlessSettings::default(), &layouts()).unwrap();
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
