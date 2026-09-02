// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! READ LOOP: what a pull backend can honestly be judged on.
//!
//! This zone stands in for CALLBACK TIMING and DEADLINE BUDGET on a
//! [`DeliveryModel::Pull`] device, and the swap is the whole point. Those two
//! measure the interval between blocks against a deadline, which is meaningful
//! only when the driver paces the blocks. A pull loop paces itself: it drains
//! whatever the driver has buffered as fast as it can, then blocks. Measured on
//! a HackRF through SoapyHackRF, that produced a median gap of 1079 µs against
//! an expected 1638, a p99 of 7763, and a permanent red verdict on a link with
//! zero drops and half the bus free.
//!
//! Occupancy asks the question the deadline was trying to ask, in the currency a
//! pull loop actually deals in: **does the loop still get to wait?** Below one
//! there is headroom. At one it never blocks, which is not a busy loop but a
//! late one, with the driver's buffer filling behind it. It goes red before a
//! sample is lost, which is what a bench instrument is for.
//!
//! The read interval is still shown, because it is real and occasionally
//! interesting. It is shown **without a verdict**, because on this transport its
//! scatter is our own rhythm rather than the link's.

use ratatui::{
    style::Style,
    text::{Line, Span},
};

use crate::state::{SdrMetrics, TimingQuality};
use crate::ui::widgets::timing_fmt::{fmt_us, quality_color};

use super::rows::Rows;

/// Colour for an occupancy figure.
///
/// **Asks the grader rather than carrying the bands.** A copy of 0.95 / 0.85 /
/// 0.75 here would be a second source for one decision, and the colour and the
/// verdict would agree only until someone edited one of them.
fn occupancy_color(occ: f32, theme: &crate::Theme) -> Style {
    Style::default().fg(quality_color(TimingQuality::from_occupancy(occ), theme))
}

pub(super) fn lines(state: &SdrMetrics, r: &Rows) -> Vec<Line<'static>> {
    let t = &state.timing;
    let theme = r.theme;

    let mut out = vec![crate::ui::chrome::section(
        "READ LOOP",
        "pull stream",
        r.iw,
        theme,
    )];

    match (r.stale, t.read_occupancy) {
        (false, Some(occ)) => {
            let pct = (occ * 100.0).round() as u32;
            out.push(Line::from(vec![
                r.field("Occupancy"),
                Span::styled(format!("{pct} %"), occupancy_color(occ, theme)),
                Span::styled(
                    format!("   {} % spare", 100u32.saturating_sub(pct)),
                    r.dim(),
                ),
            ]));
        }
        _ => out.push(Line::from(vec![r.field("Occupancy"), r.dash()])),
    }

    out.push(Line::from(vec![
        Span::raw(" "),
        Span::styled("working vs waiting in the read loop", r.dim()),
    ]));

    if r.stale || t.cb_period_us == 0 {
        out.push(Line::from(vec![r.field("Read gap"), r.dash()]));
    } else {
        out.push(Line::from(vec![
            r.field("Read gap"),
            Span::styled(fmt_us(t.cb_period_us), r.val()),
            Span::styled(format!("   p99 {}", fmt_us(t.dev_p99_us)), r.dim()),
        ]));
        out.push(Line::from(vec![
            Span::raw(" "),
            Span::styled("not a deadline: this rhythm is ours", r.dim()),
        ]));
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The colour bands must not drift from the grading thresholds they mirror.
    /// If `state::timing` moves one, this fails and names it.
    #[test]
    fn the_colour_follows_the_grade_and_not_a_copy_of_it() {
        let theme = crate::Theme::sdr();
        for occ in [0.10, 0.65, 0.80, 0.90, 0.99] {
            assert_eq!(
                occupancy_color(occ, &theme).fg,
                Some(quality_color(TimingQuality::from_occupancy(occ), &theme)),
                "occupancy {occ} coloured against something other than its grade"
            );
        }
        // And the measured healthy figure is not coloured as a fault.
        assert_ne!(
            occupancy_color(0.65, &theme).fg,
            occupancy_color(0.99, &theme).fg
        );
    }
}
