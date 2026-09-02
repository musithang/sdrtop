// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `DEPTH` and `CARRIER` - the AM pair. Two sections rather than one because
//! they answer different questions, but one module because they read the same
//! measurement and appear together or not at all.

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::state::AmMeasure;
use crate::ui::chrome;
use crate::ui::widgets::micro_common::bar_spans;

use super::fmt::val;
use super::stack::Stack;

/// `DEPTH`: modulation index, with the positive and negative peaks split out
/// under it.
pub(super) fn depth_lines(
    stack: &mut Stack<'static>,
    am: Option<AmMeasure>,
    iw: usize,
    theme: &crate::Theme,
) {
    stack.heading(chrome::section("DEPTH", "100% max", iw, theme));
    match am {
        Some(a) => {
            let ratio = a.depth_pct / 100.0;
            let color = depth_color(a.negative_pct, theme);
            stack.push(Line::from(vec![
                chrome::field("Depth", 8, theme),
                Span::styled(
                    format!("{:.0}%", a.depth_pct),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
            ]));
            let bar_w = iw.saturating_sub(9).min(14);
            if bar_w >= 4 {
                let mut row = vec![Span::raw(" ")];
                row.extend(bar_spans(ratio.clamp(0.0, 1.0) as f64, bar_w, color, theme));
                stack.ornament(Line::from(row));
            }
            // Split out because they fail differently: a negative peak
            // reaching 100 % pinches the carrier off and splatters.
            stack.detail(Line::from(vec![
                chrome::field("Pos", 8, theme),
                Span::styled(format!("{:.0}%", a.positive_pct), val(theme)),
            ]));
            stack.detail(Line::from(vec![
                chrome::field("Neg", 8, theme),
                Span::styled(
                    format!("{:.0}%", a.negative_pct),
                    Style::default().fg(depth_color(a.negative_pct, theme)),
                ),
            ]));
        }
        None => stack.gap(),
    }
}

/// `CARRIER`: the unmodulated level the depth above is a percentage of.
pub(super) fn carrier_lines(
    stack: &mut Stack<'static>,
    am: Option<AmMeasure>,
    iw: usize,
    theme: &crate::Theme,
) {
    stack.heading(chrome::section("CARRIER", "", iw, theme));
    match am {
        Some(a) => stack.push(Line::from(vec![
            chrome::field("Level", 8, theme),
            Span::styled(format!("{:.1} dBFS", a.carrier_dbfs), val(theme)),
        ])),
        None => stack.gap(),
    }
}

/// Colour for an AM depth reading, graded on the **negative** peak.
///
/// Positive over-modulation merely runs hot; a negative peak reaching 100 %
/// pinches the carrier off entirely, which clips the envelope and splatters into
/// the adjacent channel. That is the failure worth colouring for.
fn depth_color(negative_pct: f32, theme: &crate::Theme) -> ratatui::style::Color {
    if negative_pct >= 100.0 {
        theme.status_crit
    } else if negative_pct >= 90.0 {
        theme.status_warn
    } else {
        theme.status_ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn depth_color_grades_on_the_negative_peak() {
        let t = crate::Theme::sdr();
        assert_eq!(depth_color(60.0, &t), t.status_ok);
        assert_eq!(depth_color(95.0, &t), t.status_warn);
        // 100 % negative pinches the carrier off - clipping and splatter.
        assert_eq!(depth_color(100.0, &t), t.status_crit);
        assert_eq!(depth_color(130.0, &t), t.status_crit);
    }
}
