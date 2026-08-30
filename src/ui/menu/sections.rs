//! The menu's left column: the list of sections.
//!
//! Draws only. Which section is selected is decided by the caller, and which
//! sections exist is decided by [`super::model`]; this file turns the two into
//! lines and knows nothing else.

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use super::model::Menu;

/// The marker in front of the selected section. A glyph rather than a background
/// highlight, so the selection reads the same on a 16 colour terminal.
const CURSOR: &str = "\u{25B8} "; // ▸

pub fn render(f: &mut Frame, area: Rect, menu: &Menu, selected: usize, theme: &crate::Theme) {
    // A rule between the columns rather than a box: the menu already sits inside
    // one frame, and a second one would read as two panels.
    let block = Block::default()
        .borders(Borders::RIGHT)
        .border_style(Style::default().fg(theme.border_dim));
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let lines: Vec<Line> = menu
        .sections
        .iter()
        .enumerate()
        .take(inner.height as usize)
        .map(|(i, section)| {
            let chosen = i == selected;
            let style = if chosen {
                Style::default()
                    .fg(theme.value_hi)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.label)
            };
            Line::from(vec![
                Span::styled(
                    if chosen { CURSOR } else { "  " },
                    Style::default().fg(theme.border_accent),
                ),
                Span::styled(section.title.clone(), style),
            ])
        })
        .collect();

    f.render_widget(Paragraph::new(lines), inner);
}
