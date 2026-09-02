// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The menu's right column: settings.
//!
//! **Empty on purpose.** Nothing sdrtop can be told is told here yet, and the
//! pane says so rather than showing an enabled-looking list of things that do
//! not work. It exists now because the alternative was to cut this seam later,
//! at the same time as writing the first setting, which is how a one row change
//! turns into a refactor of the pane, the enum, the column and the dispatch all
//! at once.
//!
//! Two lines and a TODO is the whole screen. Anyone who opens it already knows
//! what an empty Options pane means, so explaining it at length would be talking
//! past the reader.
//!
//! Draws only, like [`super::entries`] and [`super::keys`]. When the first real
//! row lands it goes in here and nowhere else.

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::ui::chrome;

/// The heading. Says what the pane is for, in the future tense, because that is
/// the only true tense for it right now.
const HEADING: &str = "Settings will live here.";

/// The joke, and also the truth. Written without the `// ` so it can wrap like
/// any other prose: a comment that runs off the edge of a narrow pane is still a
/// comment the reader only sees half of.
const TODO: &[&str] = &[
    "TODO: settings go here",
    "left blank on purpose, not by accident",
];

/// The pane as lines, for an inner width of `iw`.
///
/// Separate from drawing for the same reason `keys::lines` is: the wrapping is
/// the only thing here that can be wrong, and it can be checked without a
/// terminal.
fn lines(iw: usize, theme: &crate::Theme) -> Vec<Line<'static>> {
    let mut out = vec![Line::from("")];
    for row in chrome::wrap(HEADING, iw.saturating_sub(4), 3) {
        out.push(Line::from(Span::styled(
            format!("  {row}"),
            Style::default()
                .fg(theme.value_hi)
                .add_modifier(Modifier::BOLD),
        )));
    }
    out.push(Line::from(""));
    for comment in TODO {
        // Every wrapped row keeps the `// `, which is how a wrapped comment
        // looks in the source it is pretending to be.
        for row in chrome::wrap(comment, iw.saturating_sub(5), 3) {
            out.push(Line::from(Span::styled(
                format!("  // {row}"),
                Style::default().fg(theme.border_dim),
            )));
        }
    }
    out
}

pub fn render(f: &mut Frame, area: Rect, theme: &crate::Theme) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let all = lines(area.width as usize, theme);
    let shown: Vec<Line> = all.into_iter().take(area.height as usize).collect();
    f.render_widget(Paragraph::new(shown), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pane's whole job right now is to name itself and admit it is empty,
    /// so both halves are worth pinning.
    #[test]
    fn the_empty_state_names_itself_and_admits_it() {
        let text: String = lines(60, &crate::Theme::sdr())
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Settings will live here"), "{text}");
        assert!(text.contains("TODO"), "{text}");
    }

    /// The copy wraps, so it has to keep fitting the narrow single column form
    /// as well as a wide one. A row wider than the pane is a row the reader only
    /// sees half of.
    #[test]
    fn every_row_fits_the_pane() {
        for iw in [28, 40, 44, 60, 100] {
            for line in lines(iw, &crate::Theme::sdr()) {
                assert!(
                    line.width() <= iw,
                    "a {}-wide row does not fit {iw} columns: {line:?}",
                    line.width()
                );
            }
        }
    }

    /// House style.
    #[test]
    fn the_copy_uses_no_em_dashes() {
        assert!(!HEADING.contains('\u{2014}'));
        assert!(TODO.iter().all(|t| !t.contains('\u{2014}')));
    }
}
