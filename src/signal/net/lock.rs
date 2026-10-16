// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Where a locked NET radio goes next: a step from the keyboard, or the one
//! place a view needs it to be when it is opened.
//!
//! **Three kinds of step, decided by what the view listens to.** The BLE list
//! and the census are fed by the advertising decoder, and advertising only
//! happens on channels 37, 38 and 39, so on those views a step is the next of
//! the three and nothing between them is a place worth stopping. A Wi-Fi
//! view decodes a network only on its own channel's centre, so there a step
//! is the next Wi-Fi channel, 1 to 13. Every other view reads the band in
//! blocks the width of what the radio can see at once (the classic channels
//! watched, or the span), so a step moves one block along, wrapping at the
//! band's ends rather than stopping against them.
//!
//! **Opening an advertising view while locked off the advertising channels
//! moves the radio onto the nearest of them**, once, and says so. A live
//! review found the BLE list silent on a lock inherited from the Survey,
//! which had stopped on a data channel where no advertising ever comes. A
//! tuning chosen after the view is open is left alone: a data channel is
//! where secondary advertising is sent, and moving someone off it every
//! time would be the instrument overruling its user. **A Wi-Fi view opened
//! off a channel centre moves onto the nearest one** the same way, once.
//!
//! Plain data in, plain data out, no radio: the task in `tasks::net` applies
//! the answer through `NetState::lock_at`, the same way the occupancy
//! cursor's `L` does.

use crate::signal::ble::channel::{advertising_channels_hz, centre_hz, channel_of};
use crate::state::LockTarget;

/// The views a locked radio belongs on an advertising channel for: the ones
/// the advertising decoder feeds. The Connection view among them, since a
/// CONNECT_IND heard on an advertising channel is how it finds a connection.
pub const ADVERTISING_VIEWS: &[&str] = &["net_ble", "net_census", "net_coded", "net_connection"];

/// The views the LE Coded receiver runs for, and nothing of LE 1M's or
/// classic's: LE Coded's own chain (`ble::coded_rx`), on the advertising
/// channel in view.
pub const CODED_VIEWS: &[&str] = &["net_coded"];

/// The view the band is measured for: the survey's occupancy and
/// coexistence panels are the only ones that show it, so elsewhere its cost
/// would buy nothing on screen.
pub const SURVEY_VIEW: &str = "net_survey";

/// The Wi-Fi views that keep a channel's visits, the survey and Airtime,
/// for the same reason; on the others the band scan they need would buy
/// nothing on screen.
pub const WIFI_VISIT_VIEWS: &[&str] = &["wifi_survey", "wifi_airtime"];

/// The views the classic receiver runs for: the Classic, Piconet and Bench
/// views read the same receiver, so they watch the same channels,
/// step the radio by the same block, and say the same thing in the menu.
pub const CLASSIC_VIEWS: &[&str] = &["net_bt", "net_piconet", "net_bench"];

/// The lowest and highest centre a step lands on: classic channel 0 and 78,
/// which are also BLE channels 37 and 39.
const BOTTOM_HZ: u64 = super::gate::LOWEST_CENTRE_HZ;
const TOP_HZ: u64 = 2_480_000_000;

/// How a view steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stepping {
    /// Between the three advertising channels.
    Advertising,
    /// By this many megahertz, the block the radio sees at once.
    Block(u64),
    /// Between the Wi-Fi channels' centres, 1 to 13
    /// (`band::WIFI_STEPPED`).
    WifiChannels,
}

/// How `preset`, filed under `section`, steps, with `block_mhz` the width it
/// reads the band in. Every view under a Wi-Fi section steps the Wi-Fi
/// channels, read from the section rather than a list of names, so the
/// views still to come need no entry here.
pub fn stepping(section: &str, preset: &str, block_mhz: u64) -> Stepping {
    if super::is_wifi(section) {
        Stepping::WifiChannels
    } else if ADVERTISING_VIEWS.contains(&preset) {
        Stepping::Advertising
    } else {
        Stepping::Block(block_mhz.max(1))
    }
}

/// The next place from `tuned_hz`, forwards or back, and what to call it.
pub fn step(tuned_hz: u64, how: Stepping, forward: bool) -> LockTarget {
    match how {
        Stepping::Advertising => {
            let adv = advertising_channels_hz();
            let next = match adv.iter().position(|&hz| on(tuned_hz, hz)) {
                Some(i) if forward => adv[(i + 1) % adv.len()],
                Some(i) => adv[(i + adv.len() - 1) % adv.len()],
                // Off them: the first one past the tuning, in the direction
                // asked, wrapping round.
                None if forward => adv
                    .iter()
                    .copied()
                    .find(|&hz| hz > tuned_hz)
                    .unwrap_or(adv[0]),
                None => adv
                    .iter()
                    .rev()
                    .copied()
                    .find(|&hz| hz < tuned_hz)
                    .unwrap_or(adv[adv.len() - 1]),
            };
            LockTarget {
                tune_hz: next,
                why: advertising_name(next),
            }
        }
        Stepping::WifiChannels => {
            let centres = super::band::wifi_stepped_centres();
            let next = match centres.iter().position(|&hz| on_wifi(tuned_hz, hz)) {
                Some(i) if forward => centres[(i + 1) % centres.len()],
                Some(i) => centres[(i + centres.len() - 1) % centres.len()],
                // Off them: the first one past the tuning, in the direction
                // asked, wrapping round.
                None if forward => centres
                    .iter()
                    .copied()
                    .find(|&hz| hz > tuned_hz)
                    .unwrap_or(centres[0]),
                None => centres
                    .iter()
                    .rev()
                    .copied()
                    .find(|&hz| hz < tuned_hz)
                    .unwrap_or(centres[centres.len() - 1]),
            };
            LockTarget {
                tune_hz: next,
                why: wifi_name(next),
            }
        }
        Stepping::Block(mhz) => {
            let step = mhz * 1_000_000;
            let within = if forward {
                Some(tuned_hz.saturating_add(step)).filter(|&hz| hz <= TOP_HZ)
            } else {
                tuned_hz.checked_sub(step).filter(|&hz| hz >= BOTTOM_HZ)
            };
            match within {
                Some(hz) => LockTarget {
                    tune_hz: hz,
                    why: format!("{} {mhz} MHz", if forward { "up" } else { "down" }),
                },
                // Past the end: round to the other one, said as that rather
                // than as a step it was not.
                None => LockTarget {
                    tune_hz: if forward { BOTTOM_HZ } else { TOP_HZ },
                    why: format!(
                        "round to the {} of the band",
                        if forward { "bottom" } else { "top" }
                    ),
                },
            }
        }
    }
}

/// Where opening `preset`, filed under `section`, while locked at
/// `tuned_hz` takes the radio: the nearest Wi-Fi channel centre for a Wi-Fi
/// view off all of them, the nearest advertising channel for a view the
/// advertising decoder feeds that finds itself off all three; `None`
/// everywhere else.
pub fn entering_view(section: &str, preset: &str, tuned_hz: u64) -> Option<LockTarget> {
    if super::is_wifi(section) {
        return entering_wifi(tuned_hz);
    }
    if !ADVERTISING_VIEWS.contains(&preset) {
        return None;
    }
    let adv = advertising_channels_hz();
    if adv.iter().any(|&hz| on(tuned_hz, hz)) {
        return None;
    }
    let nearest = adv
        .iter()
        .copied()
        .min_by_key(|&hz| hz.abs_diff(tuned_hz))?;
    Some(LockTarget {
        tune_hz: nearest,
        why: format!(
            "{}; this view decodes advertising, and {:.3} MHz carries none",
            advertising_name(nearest),
            tuned_hz as f64 / 1e6
        ),
    })
}

/// The nearest Wi-Fi channel centre, 1 to 13, for a tuning on none of them;
/// exactly between two, the lower (the first found).
fn entering_wifi(tuned_hz: u64) -> Option<LockTarget> {
    let centres = super::band::wifi_stepped_centres();
    if centres.iter().any(|&hz| on_wifi(tuned_hz, hz)) {
        return None;
    }
    let nearest = centres
        .iter()
        .copied()
        .min_by_key(|&hz| hz.abs_diff(tuned_hz))?;
    Some(LockTarget {
        tune_hz: nearest,
        why: format!(
            "{}; a Wi-Fi view decodes on a channel centre, and {:.3} MHz is not one",
            wifi_name(nearest),
            tuned_hz as f64 / 1e6
        ),
    })
}

/// Whether `tuned_hz` counts as on the Wi-Fi channel at `centre`: the
/// tolerance `band::wifi_channel` names a channel within.
fn on_wifi(tuned_hz: u64, centre: u64) -> bool {
    let named = super::band::wifi_channel(tuned_hz);
    named.is_some() && named == super::band::wifi_channel(centre)
}

/// `Wi-Fi channel 7`.
fn wifi_name(hz: u64) -> String {
    match super::band::wifi_channel(hz) {
        Some(ch) => format!("Wi-Fi channel {ch}"),
        None => "a Wi-Fi channel".to_string(),
    }
}

/// Whether `tuned_hz` counts as on the channel at `centre`: the tolerance
/// `channel_of` applies, so a view that decodes there is not moved.
fn on(tuned_hz: u64, centre: u64) -> bool {
    channel_of(tuned_hz).and_then(centre_hz) == Some(centre)
}

/// `BLE advertising channel 38`.
fn advertising_name(hz: u64) -> String {
    match channel_of(hz) {
        Some(ch) => format!("BLE advertising channel {ch}"),
        None => "a BLE advertising channel".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CH37: u64 = 2_402_000_000;
    const CH38: u64 = 2_426_000_000;
    const CH39: u64 = 2_480_000_000;

    /// The advertising views step round 37, 38, 39 and back, either way.
    #[test]
    fn advertising_views_step_round_the_three_channels() {
        let fwd = |hz| step(hz, Stepping::Advertising, true).tune_hz;
        let back = |hz| step(hz, Stepping::Advertising, false).tune_hz;
        assert_eq!(fwd(CH37), CH38);
        assert_eq!(fwd(CH38), CH39);
        assert_eq!(fwd(CH39), CH37);
        assert_eq!(back(CH37), CH39);
        assert_eq!(back(CH38), CH37);
        assert!(step(CH37, Stepping::Advertising, true)
            .why
            .contains("channel 38"));
    }

    /// Off the three, a step goes to the next one in the direction asked.
    /// **The Connection view is an advertising view.** It is fed by the
    /// advertising decoder (a CONNECT_IND is how a connection is found), so
    /// it steps the three advertising channels and, opened locked off them,
    /// moves to the nearest, as the other LE views do.
    #[test]
    fn the_connection_view_steps_and_opens_as_an_advertising_view() {
        assert_eq!(stepping("le", "net_connection", 8), Stepping::Advertising);
        let t = entering_view("le", "net_connection", 2_440_000_000).expect("moved");
        assert_eq!(t.tune_hz, 2_426_000_000);
        assert!(entering_view("le", "net_connection", 2_426_000_000).is_none());
    }

    #[test]
    fn off_the_advertising_channels_a_step_finds_the_next_one() {
        let fwd = |hz| step(hz, Stepping::Advertising, true).tune_hz;
        let back = |hz| step(hz, Stepping::Advertising, false).tune_hz;
        assert_eq!(fwd(2_435_500_000), CH39);
        assert_eq!(back(2_435_500_000), CH38);
        assert_eq!(back(2_410_000_000), CH37);
        assert_eq!(fwd(2_470_000_000), CH39);
    }

    /// Other views move a block at a time and wrap at the band's ends.
    #[test]
    fn block_views_step_a_block_and_wrap() {
        let b = Stepping::Block(8);
        assert_eq!(step(2_440_000_000, b, true).tune_hz, 2_448_000_000);
        assert_eq!(step(2_440_000_000, b, false).tune_hz, 2_432_000_000);
        assert_eq!(step(2_476_000_000, b, true).tune_hz, BOTTOM_HZ);
        assert_eq!(step(2_405_000_000, b, false).tune_hz, TOP_HZ);
        assert!(step(2_405_000_000, b, false)
            .why
            .contains("top of the band"));
        assert_eq!(step(2_440_000_000, b, true).why, "up 8 MHz");
        assert_eq!(stepping("classic", "net_bt", 8), b);
        assert_eq!(stepping("le", "net_ble", 8), Stepping::Advertising);
        assert_eq!(stepping("net", "net_survey", 0), Stepping::Block(1));
    }

    const WIFI: &str = "wifi_channel";
    const CH1: u64 = 2_412_000_000;
    const CH6: u64 = 2_437_000_000;
    const CH7: u64 = 2_442_000_000;
    const CH13: u64 = 2_472_000_000;

    /// Every view filed under a Wi-Fi section steps the Wi-Fi channels,
    /// whatever its preset is called; Bluetooth's views step as they did.
    #[test]
    fn wifi_views_step_the_wifi_channels() {
        assert_eq!(stepping(WIFI, "wifi_survey", 20), Stepping::WifiChannels);
        assert_eq!(
            stepping("wifi_room", "anything", 20),
            Stepping::WifiChannels
        );
        assert_eq!(stepping("le", "net_ble", 8), Stepping::Advertising);
        assert_eq!(stepping("classic", "net_bt", 8), Stepping::Block(8));
    }

    /// From a centre to the next, 1 to 13, wrapping both ways; 14 is never
    /// a stop.
    #[test]
    fn wifi_steps_go_round_one_to_thirteen() {
        let fwd = |hz| step(hz, Stepping::WifiChannels, true);
        let back = |hz| step(hz, Stepping::WifiChannels, false);
        assert_eq!(fwd(CH6).tune_hz, CH7);
        assert_eq!(fwd(CH6).why, "Wi-Fi channel 7");
        assert_eq!(back(CH7).tune_hz, CH6);
        assert_eq!(fwd(CH13).tune_hz, CH1);
        assert_eq!(back(CH1).tune_hz, CH13);
        for hz in [CH1, CH6, CH13] {
            assert_ne!(fwd(hz).tune_hz, 2_484_000_000);
        }
    }

    /// Off a centre, a step goes to the first centre past the tuning in the
    /// direction asked.
    #[test]
    fn off_a_wifi_centre_a_step_finds_the_next_one() {
        let fwd = |hz| step(hz, Stepping::WifiChannels, true).tune_hz;
        let back = |hz| step(hz, Stepping::WifiChannels, false).tune_hz;
        assert_eq!(fwd(2_440_000_000), CH7);
        assert_eq!(back(2_440_000_000), CH6);
        assert_eq!(fwd(2_480_000_000), CH1);
        assert_eq!(back(2_402_000_000), CH13);
    }

    /// Opening a Wi-Fi view off a centre moves to the nearest of 1 to 13,
    /// and says why; exactly between two, the lower. On a centre it stays.
    #[test]
    fn a_wifi_view_opened_off_centre_moves_to_the_nearest_channel() {
        let t = entering_view(WIFI, "wifi_survey", 2_440_000_000).expect("moved");
        assert_eq!(t.tune_hz, CH7);
        assert!(t.why.starts_with("Wi-Fi channel 7; "), "{}", t.why);
        assert!(t.why.contains("2440.000 MHz is not one"), "{}", t.why);
        let between = entering_view(WIFI, "wifi_survey", 2_439_500_000).unwrap();
        assert_eq!(between.tune_hz, CH6);
        assert_eq!(
            entering_view(WIFI, "wifi_survey", 2_484_000_000)
                .unwrap()
                .tune_hz,
            CH13,
            "14 is not a place a Wi-Fi view is taken to"
        );
        assert!(entering_view(WIFI, "wifi_survey", CH6).is_none());
        assert!(entering_view(WIFI, "wifi_survey", CH6 + 500_000).is_none());
    }

    /// The two rules do not cross: a Bluetooth view is not moved onto a
    /// Wi-Fi centre, nor a Wi-Fi view onto an advertising channel.
    #[test]
    fn the_wifi_and_advertising_rules_keep_to_their_own_views() {
        assert!(entering_view("classic", "net_bt", 2_440_000_000).is_none());
        let adv = entering_view("le", "net_ble", CH6).unwrap();
        assert_eq!(adv.tune_hz, CH38);
        let wifi = entering_view(WIFI, "net_ble", 2_440_000_000).unwrap();
        assert_eq!(wifi.tune_hz, CH7, "filed under Wi-Fi, a Wi-Fi view");
    }

    /// Opening an advertising view off the channels moves to the nearest
    /// one, and says why; on a channel, or on another view, it stays.
    #[test]
    fn an_advertising_view_opened_off_channel_moves_to_the_nearest() {
        let t = entering_view("le", "net_ble", 2_435_500_000).expect("moved");
        assert_eq!(t.tune_hz, CH38);
        assert!(t.why.contains("carries none"), "{}", t.why);
        assert_eq!(
            entering_view("le", "net_census", 2_470_000_000)
                .unwrap()
                .tune_hz,
            CH39
        );
        assert!(entering_view("le", "net_ble", CH38).is_none());
        assert!(entering_view("le", "net_ble", CH38 + 500_000).is_none());
        assert!(entering_view("classic", "net_bt", 2_435_500_000).is_none());
    }
}
