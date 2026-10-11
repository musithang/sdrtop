// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The menu's left column.
//!
//! Two kinds of row, separated by a rule: the **sections**, which come from
//! [`super::model`], and the **panes**, which do not. `Keys` and `Options` are
//! panes: they group nothing and hold no layouts, so listing them among the
//! sections would say they were sections.
//!
//! A section with a parent is drawn indented under a **heading** row naming
//! it (`Bluetooth`), before the first of its group. The heading is drawn, not
//! counted: it is not a section, so the cursor never stops on it, and every
//! index below is over sections and panes alone.
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

/// Each section's colour: the theme's series colours in section order, so
/// every palette has one and the chip in the
/// list, the heading over the views and their number keys agree.
pub fn accent(section: usize, theme: &crate::Theme) -> ratatui::style::Color {
    theme.series_color(section)
}

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

    let mut lines: Vec<Line> = Vec::with_capacity(row_count(menu) + 2);
    let mut heading: Option<&str> = None;
    for (i, section) in menu.sections.iter().enumerate() {
        if section.parent.is_some() && section.parent != heading {
            lines.push(heading_row(section.parent.unwrap_or_default(), theme));
        }
        heading = section.parent;
        lines.push(row(
            &section.title,
            Some(accent(i, theme)),
            i == selected,
            section.parent.is_some(),
            theme,
        ));
    }
    // The rule is what says the rows below are a different kind of thing.
    lines.push(Line::from(Span::styled(
        "\u{2508}".repeat(inner.width as usize),
        Style::default().fg(theme.border_dim),
    )));
    for (i, (_, label)) in PANES.iter().enumerate() {
        lines.push(row(
            label,
            None,
            menu.sections.len() + i == selected,
            false,
            theme,
        ));
    }

    lines.truncate(inner.height as usize);
    f.render_widget(Paragraph::new(lines), inner);
}

/// A heading over a group of sections: no gutter mark and no chip, since it
/// is never selected and holds no layouts of its own.
fn heading_row(label: &str, theme: &crate::Theme) -> Line<'static> {
    Line::from(vec![
        crate::ui::chrome::selection_gutter(false, theme),
        Span::raw("  "),
        Span::styled(
            label.to_string(),
            Style::default()
                .fg(theme.label)
                .add_modifier(Modifier::BOLD),
        ),
    ])
}

/// A row: the selection bar every selectable list in the app uses
/// (`chrome::selection_gutter`), a section's colour chip or a pane's blank,
/// and the label; indented when it sits under a heading.
fn row(
    label: &str,
    chip: Option<ratatui::style::Color>,
    chosen: bool,
    indented: bool,
    theme: &crate::Theme,
) -> Line<'static> {
    let style = if chosen {
        Style::default()
            .fg(theme.value_hi)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.label)
    };
    Line::from(vec![
        crate::ui::chrome::selection_gutter(chosen, theme),
        Span::raw(if indented { "  " } else { "" }),
        match chip {
            Some(c) => Span::styled("\u{25cf} ", Style::default().fg(c)),
            None => Span::raw("  "),
        },
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

    /// The column is the nine sections plus the panes under the rule. The
    /// Bluetooth heading is drawn, not counted: the cursor never stops on it.
    #[test]
    fn the_column_counts_sections_and_panes() {
        assert_eq!(row_count(&menu()), 9 + PANES.len());
    }

    /// A pane selects its own row regardless of which section the cursor came
    /// from, so returning from Keys lands back where you were.
    #[test]
    fn a_pane_selects_a_row_below_the_sections() {
        let m = menu();
        assert_eq!(selected_row(&m, 2, MenuPane::Views), 2);
        assert_eq!(selected_row(&m, 2, MenuPane::Keys), 9);
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
