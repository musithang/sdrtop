//! The menu's left column.
//!
//! Two kinds of row, separated by a rule: the **sections**, which come from
//! [`super::model`], and the **panes**, which do not. `Keys` and `Options` are
//! panes: they group nothing and hold no layouts, so listing them among the
//! sections would say they were sections.
//!
//! Draws only. Which row is selected is decided by the caller.

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::state::MenuPane;

use super::model::Menu;

/// The marker in front of the selected row. A glyph rather than a background
/// highlight, so the selection reads the same on a 16 colour terminal.
const CURSOR: &str = "\u{25B8} "; // ▸

/// The rows that are not sections, in the order they appear under the rule.
pub const PANES: &[(MenuPane, &str)] = &[(MenuPane::Keys, "Keys"), (MenuPane::Options, "Options")];

/// Total rows in the column: every section, then every pane.
pub fn row_count(menu: &Menu) -> usize {
    menu.sections.len() + PANES.len()
}

/// Which row is selected, as an index into that combined list.
pub fn selected_row(menu: &Menu, section: usize, pane: MenuPane) -> usize {
    match PANES.iter().position(|(p, _)| *p == pane) {
        Some(i) => menu.sections.len() + i,
        None => section,
    }
}

/// What a row index means. `Err(pane)` for the rows under the rule.
pub fn row_target(menu: &Menu, row: usize) -> Result<usize, MenuPane> {
    if row < menu.sections.len() {
        Ok(row)
    } else {
        Err(PANES[(row - menu.sections.len()).min(PANES.len() - 1)].0)
    }
}

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

    let mut lines: Vec<Line> = Vec::with_capacity(row_count(menu) + 1);
    for (i, section) in menu.sections.iter().enumerate() {
        lines.push(row(&section.title, i == selected, theme));
    }
    // The rule is what says the rows below are a different kind of thing.
    lines.push(Line::from(Span::styled(
        "\u{2508}".repeat(inner.width as usize),
        Style::default().fg(theme.border_dim),
    )));
    for (i, (_, label)) in PANES.iter().enumerate() {
        lines.push(row(label, menu.sections.len() + i == selected, theme));
    }

    lines.truncate(inner.height as usize);
    f.render_widget(Paragraph::new(lines), inner);
}

fn row(label: &str, chosen: bool, theme: &crate::Theme) -> Line<'static> {
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
        Span::styled(label.to_string(), style),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LayoutConfig;

    fn menu() -> Menu {
        super::super::model::build(&LayoutConfig::default_config().presets)
    }

    /// The column is the four sections plus the panes under the rule.
    #[test]
    fn the_column_counts_sections_and_panes() {
        assert_eq!(row_count(&menu()), 4 + PANES.len());
    }

    /// A pane selects its own row regardless of which section the cursor came
    /// from, so returning from Keys lands back where you were.
    #[test]
    fn a_pane_selects_a_row_below_the_sections() {
        let m = menu();
        assert_eq!(selected_row(&m, 2, MenuPane::Views), 2);
        assert_eq!(selected_row(&m, 2, MenuPane::Keys), 4);
    }

    /// Every row resolves to exactly one thing, and the two directions agree.
    #[test]
    fn every_row_round_trips() {
        let m = menu();
        for row in 0..row_count(&m) {
            match row_target(&m, row) {
                Ok(section) => assert_eq!(selected_row(&m, section, MenuPane::Views), row),
                Err(pane) => assert_eq!(selected_row(&m, 0, pane), row),
            }
        }
    }
}
