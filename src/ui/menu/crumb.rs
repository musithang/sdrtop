// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The line over the cascade: the whole path, `LAB › Wi-Fi › 2.4 GHz ›
//! Channel › Frames`, each name in its row's colour, with a count at the
//! right.
//!
//! It is what lets columns fold away on a narrow screen: whatever the cascade
//! leaves out, this line still says.

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use super::ink;
use super::tree::{Kind, Node};

/// `root` is the tile's name and colour; `trail` the rows the path passes
/// through ([`super::tree::Tree::trail`]); `hint` the count at the right.
pub fn render(
    f: &mut Frame,
    area: Rect,
    root: (&str, Color),
    trail: &[&Node],
    hint: &str,
    theme: &crate::Theme,
) {
    f.render_widget(
        Paragraph::new(line(area.width as usize, root, trail, hint, theme)),
        area,
    );
}

fn line<'a>(
    width: usize,
    root: (&str, Color),
    trail: &[&Node],
    hint: &str,
    theme: &crate::Theme,
) -> Line<'a> {
    let rule = Style::default().fg(theme.border_dim);
    let mut spans = vec![
        Span::styled("── ", rule),
        Span::styled(
            root.0.to_uppercase(),
            Style::default().fg(root.1).add_modifier(Modifier::BOLD),
        ),
    ];
    for node in trail {
        spans.push(Span::styled(" › ", rule));
        let mut style = Style::default().fg(if node.later || node.refused.is_some() {
            theme.stale
        } else {
            ink::of(node.tone, theme)
        });
        if matches!(node.kind, Kind::View(_)) {
            style = style.fg(theme.value).add_modifier(Modifier::BOLD);
        }
        spans.push(Span::styled(node.title.clone(), style));
    }
    let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    let tail = if hint.is_empty() {
        String::new()
    } else {
        format!(" {hint} ")
    };
    let fill = width.saturating_sub(used + tail.chars().count() + 2);
    spans.push(Span::styled(format!(" {}", "─".repeat(fill)), rule));
    spans.push(Span::styled(tail, Style::default().fg(theme.label)));
    spans.push(Span::styled("─", rule));
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LayoutConfig;
    use crate::ui::menu::{model, tree};

    fn text(width: usize, names: &[&str]) -> String {
        let theme = crate::Theme::sdr();
        let built = tree::build(&model::build(&LayoutConfig::default_config().presets), &[]);
        let mut rows = built.root(tree::Tile::Lab);
        let mut path = tree::Path::default();
        for name in names {
            let i = rows
                .iter()
                .position(|n| n.is_stop() && n.title == *name)
                .unwrap();
            path.rows.push(i);
            rows = rows[i].children();
        }
        let trail = built.trail(tree::Tile::Lab, &path);
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width as u16, 1)).unwrap();
        terminal
            .draw(|f| {
                render(
                    f,
                    f.size(),
                    ("Lab", theme.value_hi),
                    &trail,
                    "27 views",
                    &theme,
                )
            })
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..width as u16)
            .map(|x| buf.get(x, 0).symbol().to_string())
            .collect()
    }

    /// The whole path, and the count at the right edge of the width given.
    #[test]
    fn the_crumb_says_the_whole_path() {
        let line = text(120, &["Wi-Fi", "2.4 GHz", "Channel", "Frames"]);
        assert!(
            line.starts_with("── LAB › Wi-Fi › 2.4 GHz › Channel › Frames ─"),
            "{line}"
        );
        assert!(line.ends_with(" 27 views ─"), "{line}");
        assert_eq!(line.chars().count(), 120);
    }

    /// With nothing chosen yet, the tile alone.
    #[test]
    fn an_empty_path_names_the_tile() {
        let line = text(60, &[]);
        assert!(line.starts_with("── LAB ─"), "{line}");
    }
}
