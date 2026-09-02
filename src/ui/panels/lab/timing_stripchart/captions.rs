// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The text around the plot: what it shows, what the colours mean, and how much
//! time is on screen.

use ratatui::{
    style::Style,
    text::{Line, Span},
};

use crate::state::TimingState;

use super::chart::shown_samples;

/// The leading space every caption row is indented by.
const LEAD_W: usize = 1;

/// What the plot is, written three times at three lengths, once per delivery
/// model.
///
/// A pull backend has no callbacks and no expected period to deviate from: each
/// point is one `readStream` return, and the spread is our own loop draining a
/// buffer. Saying otherwise is the same invented fact the verdict used to print.
const DESC_PUSH: [&str; 3] = [
    "each point is one RX callback \u{00b7} deviation of its interval from the expected period \u{00b7} bars past the band are late",
    "per-callback interval deviation from the expected period",
    "callback interval deviation",
];
const DESC_PULL: [&str; 3] = [
    "each point is one read \u{00b7} how far its gap sat from the mean \u{00b7} a pull loop drains, then waits",
    "read gap spread \u{00b7} our own rhythm, not the link's",
    "read gap spread",
];

/// The longest form that actually fits, counting the leading space.
///
/// **Chosen by measuring, not by a hand-picked width.** The thresholds used to be
/// literals - 78 and 40 - while the sentences they select need 113 and 57
/// columns, so every tier was picked at widths it could not fit: a 48-column
/// panel read `…from the expe`, an 80-column one `…from the expected p`. Deriving
/// the threshold from each string means editing a sentence moves its own
/// threshold with it.
///
/// The shortest form is the floor. It needs 28 columns and the panel's
/// `min_size` is 48, so it is only clipped in a layout the panel already
/// declines.
pub(super) fn description(iw: usize, delivery: crate::hardware::DeliveryModel) -> &'static str {
    let table = match delivery {
        crate::hardware::DeliveryModel::Push => &DESC_PUSH,
        crate::hardware::DeliveryModel::Pull => &DESC_PULL,
    };
    // Every caption row starts with one space, so that column is not available.
    let room = iw.saturating_sub(LEAD_W);
    table
        .iter()
        .find(|d| d.chars().count() <= room)
        .copied()
        .unwrap_or(table[table.len() - 1])
}

/// The colour key, and what the guide line marks.
///
/// A pull backend has no deadline to be in or out of, so the legend names the
/// band rather than a verdict on it.
pub(super) fn key_row(
    budget_us: u64,
    delivery: crate::hardware::DeliveryModel,
    theme: &crate::Theme,
) -> Line<'static> {
    let (inside, outside, guide) = match delivery {
        crate::hardware::DeliveryModel::Push => (
            "\u{25AC} in budget",
            "\u{25AC} over budget",
            format!("\u{2504} \u{00b1}{budget_us} \u{00b5}s deadline"),
        ),
        crate::hardware::DeliveryModel::Pull => (
            "\u{25AC} near the mean",
            "\u{25AC} a long read gap",
            format!("\u{2504} \u{00b1}{budget_us} \u{00b5}s, for scale only"),
        ),
    };
    Line::from(vec![
        Span::raw(" "),
        Span::styled(inside, Style::default().fg(theme.value)),
        Span::raw("   "),
        Span::styled(outside, Style::default().fg(theme.status_warn)),
        Span::raw("   "),
        Span::styled(guide, Style::default().fg(theme.border_dim)),
    ])
}

/// How long a stretch of stream the plot covers.
///
/// Derived from what the chart actually plots rather than from the buffer's
/// length: a wide panel shows more history than a narrow one, and the caption has
/// to say which.
pub(super) fn window_secs(chart_w: u16, t: &TimingState) -> f64 {
    let shown = shown_samples(chart_w, t.cb_deviations_us.len());
    (shown as u64 * t.cb_period_us) as f64 / 1e6
}

pub(super) fn window_row(
    chart_w: u16,
    t: &TimingState,
    delivery: crate::hardware::DeliveryModel,
    stale: bool,
    theme: &crate::Theme,
) -> Line<'static> {
    let lbl = Style::default().fg(theme.label);
    let unit = match delivery {
        crate::hardware::DeliveryModel::Push => "one point per RX callback",
        crate::hardware::DeliveryModel::Pull => "one point per read",
    };
    // With no measured period there is no time axis to name, only a sample axis.
    if stale || t.cb_period_us == 0 {
        return Line::from(vec![Span::raw(" "), Span::styled(unit, lbl)]);
    }
    Line::from(vec![
        Span::raw(" "),
        Span::styled(
            format!(
                "\u{2190} {:.1} s window \u{00b7} {unit} \u{2192}",
                window_secs(chart_w, t)
            ),
            lbl,
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each tier must actually fit the width it is chosen for, or the caption is
    /// clipped by the very check that picked it.
    #[test]
    fn every_description_fits_the_width_that_selects_it() {
        for iw in [28usize, 40, 57, 60, 113, 200] {
            let d = description(iw, crate::hardware::DeliveryModel::Push);
            assert!(
                d.chars().count() <= iw - LEAD_W,
                "iw={iw}: {:?} is {} chars",
                d,
                d.chars().count()
            );
        }
    }

    /// Wider panels say more, never less, and the change happens exactly where
    /// the longer sentence starts to fit.
    #[test]
    fn the_description_grows_with_the_panel() {
        let push = crate::hardware::DeliveryModel::Push;
        assert!(description(30, push).len() < description(60, push).len());
        assert!(description(60, push).len() < description(120, push).len());
        for d in DESC_PUSH {
            let need = d.chars().count() + LEAD_W;
            assert_eq!(
                description(need, push),
                d,
                "{d:?} should be chosen at {need}"
            );
        }
        // Every form but the last gives way one column below its own length.
        // The shortest is the floor and is returned however narrow the panel is.
        for d in &DESC_PUSH[..DESC_PUSH.len() - 1] {
            let need = d.chars().count() + LEAD_W;
            assert_ne!(
                description(need - 1, push),
                *d,
                "{d:?} was chosen one column too narrow"
            );
        }
        // The pull table has to obey the same width contract, or a SoapySDR
        // panel would clip a sentence the push one fits.
        let pull = crate::hardware::DeliveryModel::Pull;
        for d in DESC_PULL {
            let need = d.chars().count() + LEAD_W;
            assert_eq!(
                description(need, pull),
                d,
                "{d:?} should be chosen at {need}"
            );
        }
    }

    /// The window is what is plotted, so a wider chart covers more time.
    #[test]
    fn a_wider_chart_covers_more_time() {
        let mut t = TimingState {
            cb_period_us: 4_096,
            ..Default::default()
        };
        t.cb_deviations_us = vec![0; 1_000];
        let narrow = window_secs(40, &t);
        let wide = window_secs(120, &t);
        assert!(wide > narrow, "narrow {narrow} wide {wide}");
        // And never claims more history than the snapshot holds.
        t.cb_deviations_us.truncate(4);
        assert!((window_secs(200, &t) - 4.0 * 4_096.0 / 1e6).abs() < 1e-9);
    }
}
