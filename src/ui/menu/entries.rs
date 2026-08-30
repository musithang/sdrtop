//! The menu's right column: the views inside the selected section.
//!
//! Draws only. The number beside each view is its `slot`, which is the key that
//! selects it, so this file must never invent a number of its own: the menu
//! would then be teaching a key that does not work.

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use super::model::Section;

const CURSOR: &str = "\u{25B8} "; // ▸

/// How many terminal rows one view occupies: the title, and its blurb under it.
const ROWS_PER_ENTRY: usize = 2;

pub fn render(f: &mut Frame, area: Rect, section: &Section, cursor: usize, theme: &crate::Theme) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let visible = (area.height as usize / ROWS_PER_ENTRY).max(1);
    let first = scroll_offset(cursor, section.entries.len(), visible);

    let mut lines: Vec<Line> = Vec::with_capacity(visible * ROWS_PER_ENTRY);
    for (i, entry) in section.entries.iter().enumerate().skip(first).take(visible) {
        let chosen = i == cursor;
        let title_style = if chosen {
            Style::default()
                .fg(theme.value_hi)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.value)
        };
        // A view with no slot has no number key, so it shows no number. The blank
        // keeps the titles aligned with the ones that do.
        let key = match entry.slot {
            Some(slot) => format!("{slot}  "),
            None => "   ".to_string(),
        };
        lines.push(Line::from(vec![
            Span::styled(
                if chosen { CURSOR } else { "  " },
                Style::default().fg(theme.border_accent),
            ),
            Span::styled(key, Style::default().fg(theme.border_accent)),
            Span::styled(entry.title.clone(), title_style),
        ]));
        lines.push(Line::from(Span::styled(
            format!("     {}", entry.blurb.clone().unwrap_or_default()),
            Style::default().fg(theme.label),
        )));
    }

    f.render_widget(Paragraph::new(lines), area);
}

/// The first entry to draw so that `cursor` is on screen.
///
/// Pure and separate so it can be tested without a terminal: an off-by-one here
/// shows up as a cursor you cannot see, which is the kind of bug that only
/// appears on someone else's short window.
fn scroll_offset(cursor: usize, total: usize, visible: usize) -> usize {
    if total <= visible {
        return 0;
    }
    // Keep the cursor in view, and never scroll past the last full window.
    cursor
        .saturating_sub(visible - 1)
        .min(total.saturating_sub(visible))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_section_that_fits_never_scrolls() {
        assert_eq!(scroll_offset(0, 4, 4), 0);
        assert_eq!(scroll_offset(3, 4, 4), 0);
        assert_eq!(scroll_offset(0, 1, 9), 0);
    }

    #[test]
    fn the_cursor_stays_on_screen_when_it_does_not_fit() {
        // Five entries, room for two: the cursor is always inside the window.
        for cursor in 0..5 {
            let first = scroll_offset(cursor, 5, 2);
            assert!(
                (first..first + 2).contains(&cursor),
                "cursor {cursor} is off screen with offset {first}"
            );
        }
    }

    #[test]
    fn it_never_scrolls_past_the_last_window() {
        assert_eq!(scroll_offset(4, 5, 2), 3);
        assert_eq!(scroll_offset(99, 5, 2), 3);
    }
}
