// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Where a locked NET radio goes next: a step from the keyboard, or the one
//! place a view needs it to be when it is opened.
//!
//! **Two kinds of step, decided by what the view listens to.** The BLE list
//! and the census are fed by the advertising decoder, and advertising only
//! happens on channels 37, 38 and 39, so on those views a step is the next of
//! the three and nothing between them is a place worth stopping. Every other
//! view reads the band in blocks the width of what the radio can see at once
//! (the classic channels watched, or the span), so a step moves one block
//! along, wrapping at the band's ends rather than stopping against them.
//!
//! **Opening an advertising view while locked off the advertising channels
//! moves the radio onto the nearest of them**, once, and says so. A live
//! review found the BLE list silent on a lock inherited from the Survey,
//! which had stopped on a data channel where no advertising ever comes. A
//! tuning chosen after the view is open is left alone: a data channel is
//! where secondary advertising is sent, and moving someone off it every
//! time would be the instrument overruling its user.
//!
//! Plain data in, plain data out, no radio: the task in `tasks::net` applies
//! the answer through `NetState::lock_at`, the same way the occupancy
//! cursor's `L` does.

use crate::signal::ble::channel::{advertising_channels_hz, centre_hz, channel_of};
use crate::state::LockTarget;

/// The views a locked radio belongs on an advertising channel for: the ones
/// the advertising decoder feeds.
pub const ADVERTISING_VIEWS: &[&str] = &["net_ble", "net_census"];

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
}

/// How `preset` steps, with `block_mhz` the width it reads the band in.
pub fn stepping(preset: &str, block_mhz: u64) -> Stepping {
    if ADVERTISING_VIEWS.contains(&preset) {
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

/// Where opening `preset` while locked at `tuned_hz` takes the radio: the
/// nearest advertising channel, for a view the advertising decoder feeds
/// that finds itself off all three; `None` everywhere else.
pub fn entering_view(preset: &str, tuned_hz: u64) -> Option<LockTarget> {
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
        assert_eq!(stepping("net_bt", 8), b);
        assert_eq!(stepping("net_ble", 8), Stepping::Advertising);
        assert_eq!(stepping("net_survey", 0), Stepping::Block(1));
    }

    /// Opening an advertising view off the channels moves to the nearest
    /// one, and says why; on a channel, or on another view, it stays.
    #[test]
    fn an_advertising_view_opened_off_channel_moves_to_the_nearest() {
        let t = entering_view("net_ble", 2_435_500_000).expect("moved");
        assert_eq!(t.tune_hz, CH38);
        assert!(t.why.contains("carries none"), "{}", t.why);
        assert_eq!(
            entering_view("net_census", 2_470_000_000).unwrap().tune_hz,
            CH39
        );
        assert!(entering_view("net_ble", CH38).is_none());
        assert!(entering_view("net_ble", CH38 + 500_000).is_none());
        assert!(entering_view("net_bt", 2_435_500_000).is_none());
    }
}
