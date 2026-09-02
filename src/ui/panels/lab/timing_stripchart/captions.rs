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

/// What the plot is, written three times at three lengths.
const DESC: [&str; 3] = [
    "each point is one RX callback \u{00b7} deviation of its interval from the expected period \u{00b7} bars past the band are late",
    "per-callback interval deviation from the expected period",
    "callback interval deviation",
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
pub(super) fn description(iw: usize) -> &'static str {
    // Every caption row starts with one space, so that column is not available.
    let room = iw.saturating_sub(LEAD_W);
    DESC.iter()
        .find(|d| d.chars().count() <= room)
        .copied()
        .unwrap_or(DESC[DESC.len() - 1])
}

/// The colour key, and what the guide line marks.
pub(super) fn key_row(budget_us: u64, theme: &crate::Theme) -> Line<'static> {
    Line::from(vec![
        Span::raw(" "),
        Span::styled("\u{25AC} in budget", Style::default().fg(theme.value)),
        Span::raw("   "),
        Span::styled(
            "\u{25AC} over budget",
            Style::default().fg(theme.status_warn),
        ),
        Span::raw("   "),
        Span::styled(
            format!("\u{2504} \u{00b1}{budget_us} \u{00b5}s deadline"),
            Style::default().fg(theme.border_dim),
        ),
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
    stale: bool,
    theme: &crate::Theme,
) -> Line<'static> {
    let lbl = Style::default().fg(theme.label);
    // With no measured period there is no time axis to name, only a sample axis.
    if stale || t.cb_period_us == 0 {
        return Line::from(vec![
            Span::raw(" "),
            Span::styled("one point per RX callback", lbl),
        ]);
    }
    Line::from(vec![
        Span::raw(" "),
        Span::styled(
            format!(
                "\u{2190} {:.1} s window \u{00b7} one point per RX callback \u{2192}",
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
            let d = description(iw);
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
        assert!(description(30).len() < description(60).len());
        assert!(description(60).len() < description(120).len());
        for d in DESC {
            let need = d.chars().count() + LEAD_W;
            assert_eq!(description(need), d, "{d:?} should be chosen at {need}");
        }
        // Every form but the last gives way one column below its own length.
        // The shortest is the floor and is returned however narrow the panel is.
        for d in &DESC[..DESC.len() - 1] {
            let need = d.chars().count() + LEAD_W;
            assert_ne!(
                description(need - 1),
                *d,
                "{d:?} was chosen one column too narrow"
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
