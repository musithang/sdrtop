// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The action chips, and how long ago the auto-cal was taken.
//!
//! Separate from [`super::verdict`] because it reports state rather than judging
//! it - the chips light from `IqCalState` directly, and say nothing about whether
//! the readings are good. A line under them used to spell the chips out again
//! in words ("DC-block OFF · auto-cal idle"); what the chips cannot show is the
//! calibration's age, and that is all that is left of it.

use ratatui::{
    style::Style,
    text::{Line, Span},
};

use crate::state::IqCalState;

use super::rows::Rows;

/// Inner width at which the full chip labels fit. Below it they fall back to
/// single letters, or the freeze chip is clipped off the right edge.
const FULL_LABEL_MIN_W: usize = 37;

pub(super) fn lines(cal: &IqCalState, rows: &Rows) -> Vec<Line<'static>> {
    let (d, c, f) = if rows.iw >= FULL_LABEL_MIN_W {
        ("D DC-block", "C auto-cal", "F freeze")
    } else {
        ("D", "C", "F")
    };
    let chips = vec![
        Span::raw(" "),
        rows.chip(d, cal.dc_block_on),
        Span::raw(" "),
        rows.chip(c, cal.cal_applied),
        Span::raw(" "),
        rows.chip(f, cal.frozen),
    ];
    let age = Span::styled(age(cal, now_secs()), Style::default().fg(rows.dim()));
    let used: usize = chips.iter().map(|s| s.content.chars().count()).sum();
    // Beside the chips where it fits, under them where it does not.
    if used + 2 + age.content.chars().count() <= rows.iw {
        let mut line = chips;
        line.push(Span::raw("  "));
        line.push(age);
        vec![Line::from(line)]
    } else {
        vec![Line::from(chips), Line::from(vec![Span::raw(" "), age])]
    }
}

/// The calibration's state in words the chips cannot say. `now` is passed in so
/// the age can be tested without waiting or mocking a clock.
fn age(cal: &IqCalState, now: u64) -> String {
    if cal.cal_pending && !cal.cal_applied {
        return "capturing\u{2026}".to_string();
    }
    match cal.last_cal_at {
        None => "no cal yet".to_string(),
        Some(t) => {
            let ago = now.max(t).saturating_sub(t);
            if ago < 60 {
                format!("cal {ago}s ago")
            } else {
                format!("cal {}m ago", ago / 60)
            }
        }
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_age_says_never_capturing_or_how_long_ago() {
        assert_eq!(age(&IqCalState::default(), 100), "no cal yet");
        let pending = IqCalState {
            cal_pending: true,
            ..Default::default()
        };
        assert!(age(&pending, 100).contains("capturing"));
        let cal = |t| IqCalState {
            last_cal_at: Some(t),
            cal_applied: true,
            ..Default::default()
        };
        assert_eq!(age(&cal(100), 130), "cal 30s ago");
        assert_eq!(age(&cal(100), 160), "cal 1m ago");
    }

    /// A clock that steps backwards (NTP, suspend) must not produce a huge age
    /// from a subtraction that wrapped.
    #[test]
    fn a_backwards_clock_reads_zero_rather_than_forever() {
        let cal = IqCalState {
            last_cal_at: Some(500),
            ..Default::default()
        };
        assert_eq!(age(&cal, 100), "cal 0s ago");
    }
}
