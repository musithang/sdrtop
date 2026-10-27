// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The columns under the tiles: each the rows of the one selected to its
//! left, and the card to the right of the last.
//!
//! **The path is always visible.** The selected row of every column keeps its
//! gutter mark, so the way down is drawn, not remembered; the column the
//! cursor is in adds the bright title. When the cursor rests on a row that
//! holds more, the column it would open is drawn beside it, unmarked, as a
//! preview. On a screen too narrow for every column the leftmost fold away
//! first: the crumb above still says the whole path.
//!
//! Draws only, and returns the rect it left for the card.

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use super::ink;
use super::tree::{Kind, Node, Path, Tile, Tree};

/// A column's width, rule included.
const COLUMN: u16 = 25;

/// The narrowest card worth drawing beside the columns.
const CARD_MIN: u16 = 40;

/// How a column is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// Left of the cursor: its selected row is part of the path.
    Path,
    /// The cursor's own.
    Cursor,
    /// What the cursor's row would open: nothing selected.
    Preview,
}

/// Draw the columns down `path` in `tile`'s tree into `area`. `cursor` is
/// false while the focus is on the tiles: the path is drawn, the cursor not.
/// Returns where the card goes.
pub fn render(
    f: &mut Frame,
    area: Rect,
    tree: &Tree,
    tile: Tile,
    path: &Path,
    cursor: bool,
    theme: &crate::Theme,
) -> Rect {
    let mut columns: Vec<(&[Node], Option<usize>, State)> = tree
        .columns(tile, path)
        .into_iter()
        .zip(path.rows.iter())
        .map(|(rows, &i)| (rows, Some(i), State::Path))
        .collect();
    if let Some(last) = columns.last_mut() {
        last.2 = if cursor { State::Cursor } else { State::Path };
    }
    if let Some(node) = tree.node(tile, path) {
        if !node.later && node.children().iter().any(Node::is_stop) {
            columns.push((node.children(), None, State::Preview));
        }
    }
    if columns.is_empty() {
        return area;
    }

    // As many columns as leave the card its room, the rightmost kept.
    let room = area.width.saturating_sub(CARD_MIN) / COLUMN;
    let shown = (room as usize).clamp(1, columns.len());
    let first = columns.len() - shown;
    let width = if room == 0 {
        (area.width / 2).min(COLUMN)
    } else {
        COLUMN
    };

    let mut x = area.x;
    for (depth, (rows, selected, state)) in columns.iter().enumerate().skip(first) {
        let rect = Rect::new(x, area.y, width.saturating_sub(1), area.height);
        let title = column_title(tile, depth, rows);
        column(f, rect, title, rows, *selected, *state, theme);
        let rule = Style::default().fg(theme.stale);
        for y in area.y..area.y + area.height {
            f.buffer_mut().set_string(rect.x + rect.width, y, "┊", rule);
        }
        x += width;
    }
    Rect::new(
        x + 1,
        area.y,
        area.right().saturating_sub(x + 1),
        area.height,
    )
}

/// What a column is a list of, in the word a reader would use.
fn column_title(tile: Tile, depth: usize, rows: &[Node]) -> &'static str {
    let stops = || rows.iter().filter(|n| n.is_stop());
    if stops().all(|n| matches!(n.kind, Kind::View(_))) {
        "VIEW"
    } else if stops().any(|n| n.band.is_some()) && depth > 0 {
        "BAND"
    } else if depth == 0 {
        match tile {
            Tile::Lab => "AREA",
            Tile::Radio => "WHO IS TALKING",
        }
    } else {
        "GROUP"
    }
}

fn column(
    f: &mut Frame,
    area: Rect,
    title: &str,
    rows: &[Node],
    selected: Option<usize>,
    state: State,
    theme: &crate::Theme,
) {
    let w = area.width as usize;
    let mut lines: Vec<Line> = Vec::new();
    let mut at = 0;
    for (i, node) in rows.iter().enumerate() {
        if !node.is_stop() {
            if i > 0 {
                lines.push(Line::raw(""));
            }
            lines.push(Line::from(Span::styled(
                format!("  {}", node.title),
                Style::default().fg(theme.border_dim),
            )));
            continue;
        }
        if Some(i) == selected {
            at = lines.len();
        }
        lines.push(row(node, w, Some(i) == selected, state, theme));
    }

    // Keep the selected row in view: the list scrolls under the heading.
    let body = area.height.saturating_sub(2) as usize;
    let offset = (at + 1).saturating_sub(body);
    let head = Line::from(Span::styled(
        format!("  {title}"),
        Style::default()
            .fg(theme.border_dim)
            .add_modifier(Modifier::BOLD),
    ));
    let mut out = vec![head, Line::raw("")];
    out.extend(lines.into_iter().skip(offset));
    f.render_widget(Paragraph::new(out), area);
}

fn row<'a>(node: &Node, w: usize, on: bool, state: State, theme: &crate::Theme) -> Line<'a> {
    let dead = node.later || node.refused.is_some();
    let tone = if dead {
        theme.stale
    } else {
        ink::of(node.tone, theme)
    };
    let cursor = on && state == State::Cursor;
    let gutter = if on && state != State::Preview {
        Span::styled("▌", Style::default().fg(tone))
    } else {
        Span::raw(" ")
    };
    let lead = match &node.kind {
        Kind::View(e) => Span::styled(
            e.slot.map_or("·".to_string(), |s| s.to_string()),
            Style::default().fg(if dead {
                theme.stale
            } else {
                theme.border_accent
            }),
        ),
        _ => Span::styled(if dead { "○" } else { "●" }, Style::default().fg(tone)),
    };
    let mut title = Style::default().fg(match (dead, cursor, on) {
        (true, _, _) => theme.stale,
        (false, true, _) => theme.value_hi,
        (false, false, true) if state == State::Path => theme.border_focused,
        _ => theme.value,
    });
    if on && state != State::Preview {
        title = title.add_modifier(Modifier::BOLD);
    }
    let branch = matches!(node.kind, Kind::Branch { .. });

    // A refused group says so; the views inside it are grey, and saying
    // it again on each would only repeat the group.
    let hint = match (&node.refused, branch) {
        (Some(_), true) => "refused",
        (Some(_), false) => "",
        (None, _) => node.hint.unwrap_or(""),
    };
    // ▌ ● title ......... hint ›
    let fixed = 4 + if branch { 2 } else { 0 };
    let room = w.saturating_sub(fixed);
    let name: String = node.title.chars().take(room).collect();
    let used = name.chars().count();
    let hint = if !hint.is_empty() && used + 2 + hint.chars().count() <= room {
        hint
    } else {
        ""
    };
    let pad = room.saturating_sub(used + hint.chars().count());
    let mut spans = vec![
        gutter,
        Span::raw(" "),
        lead,
        Span::raw(" "),
        Span::styled(name, title),
        Span::raw(" ".repeat(pad)),
        Span::styled(
            hint.to_string(),
            Style::default().fg(if dead { theme.stale } else { theme.border_dim }),
        ),
    ];
    if branch {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            "›",
            Style::default().fg(if on && !dead { tone } else { theme.border_dim }),
        ));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LayoutConfig;
    use crate::ui::menu::{model, tree};
    use ratatui::{backend::TestBackend, Terminal};

    fn builtins() -> Tree {
        tree::build(&model::build(&LayoutConfig::default_config().presets), &[])
    }

    fn path_to(t: &Tree, want: &[&str]) -> Path {
        let mut rows = t.root(Tile::Lab);
        let mut path = Path::default();
        for name in want {
            let i = rows
                .iter()
                .position(|n| n.is_stop() && n.title == *name)
                .unwrap();
            path.rows.push(i);
            rows = rows[i].children();
        }
        path
    }

    fn draw(t: &Tree, path: &Path, cursor: bool, w: u16, h: u16) -> (Vec<String>, Rect) {
        let theme = crate::Theme::sdr();
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        let mut card = Rect::default();
        terminal
            .draw(|f| card = render(f, f.size(), t, Tile::Lab, path, cursor, &theme))
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        let rows = (0..h)
            .map(|y| (0..w).map(|x| buf.get(x, y).symbol().to_string()).collect())
            .collect();
        (rows, card)
    }

    /// Every column down the path, titled, with the path's rows marked.
    #[test]
    fn the_columns_follow_the_path() {
        let t = builtins();
        let path = path_to(&t, &["Wi-Fi", "2.4 GHz", "Channel", "Frames"]);
        let (rows, card) = draw(&t, &path, true, 191, 24);
        let all = rows.join("\n");
        for word in [
            "AREA", "BAND", "GROUP", "VIEW", "Wi-Fi", "2.4 GHz", "Channel", "Frames",
        ] {
            assert!(all.contains(word), "{word}\n{all}");
        }
        assert_eq!(all.matches('▌').count(), 4, "one mark a column\n{all}");
        assert!(card.width >= CARD_MIN && card.x > 100, "{card:?}");
    }

    /// On a group, the column it opens is drawn beside it with nothing
    /// selected in it.
    #[test]
    fn a_group_previews_what_it_holds() {
        let t = builtins();
        let path = path_to(&t, &["Bluetooth", "2.4 GHz", "Low Energy"]);
        let (rows, _) = draw(&t, &path, true, 191, 24);
        let all = rows.join("\n");
        assert!(
            all.contains("LE devices") && all.contains("One LE link"),
            "{all}"
        );
        assert_eq!(all.matches('▌').count(), 3, "{all}");
    }

    /// Too narrow for every column, the leftmost go first.
    #[test]
    fn a_narrow_screen_keeps_the_rightmost_columns() {
        let t = builtins();
        let path = path_to(&t, &["Wi-Fi", "2.4 GHz", "Channel", "Frames"]);
        let (rows, _) = draw(&t, &path, true, 100, 24);
        let all = rows.join("\n");
        assert!(all.contains("Frames") && all.contains("Room"), "{all}");
        assert!(!all.contains("AREA") && !all.contains("Compact"), "{all}");
    }

    /// While the tiles have the focus, the path is drawn and the cursor not.
    #[test]
    fn the_preview_has_no_cursor() {
        let theme = crate::Theme::sdr();
        let t = builtins();
        let path = path_to(&t, &["Receiver", "IQ balance"]);
        let mut terminal = Terminal::new(TestBackend::new(191, 24)).unwrap();
        for cursor in [true, false] {
            terminal
                .draw(|f| {
                    render(f, f.size(), &t, Tile::Lab, &path, cursor, &theme);
                })
                .unwrap();
            let buf = terminal.backend().buffer().clone();
            let lit = (0..191u16)
                .flat_map(|x| (0..24u16).map(move |y| (x, y)))
                .any(|(x, y)| buf.get(x, y).fg == theme.value_hi && buf.get(x, y).symbol() != " ");
            assert_eq!(lit, cursor);
        }
    }

    /// Later and refused rows say so in their hint.
    #[test]
    fn quiet_rows_say_why() {
        let t = builtins();
        let path = path_to(&t, &["Wi-Fi", "2.4 GHz", "Channel"]);
        let (rows, _) = draw(&t, &path, true, 191, 24);
        let room = rows.iter().find(|r| r.contains("Room")).unwrap();
        assert!(room.contains("○") && room.contains("later"), "{room}");
    }

    /// A column taller than the screen scrolls to keep the selected row.
    #[test]
    fn the_selected_row_stays_in_view() {
        let t = builtins();
        let path = path_to(&t, &["Compact"]);
        let (rows, _) = draw(&t, &path, true, 191, 8);
        assert!(
            rows.iter()
                .any(|r| r.contains("▌") && r.contains("Compact")),
            "{rows:#?}"
        );
    }
}
