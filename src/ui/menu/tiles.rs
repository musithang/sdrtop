// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The menu's top row: the tiles.
//!
//! **Square on screen.** A terminal cell is about twice as tall as it is
//! wide, so a large tile is 20 x 10 cells and a small one 10 x 5, and both
//! look square. Which size is drawn is decided by the space the caller gives
//! ([`Size::for_area`]), never by which screen asked: the whole terminal at
//! startup and the box over the deck are the same function.
//!
//! Draws only. Which tile is selected and whether the focus is on the row are
//! the menu state's; what each tile opens is the caller's.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Widget},
    Frame,
};

use super::ink;

/// One tile. The order of [`Face::ALL`] is the order on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Face {
    Resume,
    Radio,
    Lab,
    Options,
    Keys,
    Quit,
}

impl Face {
    pub const ALL: [Face; 6] = [
        Face::Resume,
        Face::Radio,
        Face::Lab,
        Face::Options,
        Face::Keys,
        Face::Quit,
    ];

    /// The tiles on screen: Resume only when there is something to resume.
    pub fn shown(resume: bool) -> Vec<Face> {
        Self::ALL
            .into_iter()
            .filter(|f| resume || *f != Face::Resume)
            .collect()
    }

    pub fn name(self) -> &'static str {
        match self {
            Face::Resume => "Resume",
            Face::Radio => "Radio",
            Face::Lab => "Lab",
            Face::Options => "Options",
            Face::Keys => "Keys",
            Face::Quit => "Quit",
        }
    }

    /// The key that selects it, as drawn in its corner.
    pub fn key(self) -> &'static str {
        match self {
            Face::Resume => "Enter",
            Face::Radio => "R",
            Face::Lab => "L",
            Face::Options => "O",
            Face::Keys => "K",
            Face::Quit => "Q",
        }
    }

    /// What it is, in a few words. Resume's is the view it resumes, which
    /// the caller passes.
    fn line(self) -> &'static str {
        match self {
            Face::Resume => "",
            Face::Radio => "listen to the air",
            Face::Lab => "measure and decode",
            Face::Options => "radio settings",
            Face::Keys => "every key",
            Face::Quit => "close sdrtop",
        }
    }

    fn color(self, theme: &crate::Theme) -> ratatui::style::Color {
        match self {
            Face::Resume => theme.status_ok,
            Face::Radio => theme.border_accent,
            Face::Lab => theme.value_hi,
            Face::Options | Face::Keys => theme.label,
            Face::Quit => theme.status_crit,
        }
    }

    /// The large tile's picture, four rows at most.
    fn icon(self) -> &'static [&'static str] {
        match self {
            Face::Resume => &["▐█▄    ", "▐███▄  ", "▐███▀  ", "▐█▀    "],
            Face::Radio => &["((( ╻ )))", "    ┃    ", "    ┃    ", "  ▁▁┻▁▁  "],
            Face::Lab => &["╭─────────╮", "│    ▆    │", "│▁▁▂▅█▅▂▁▁│", "╰─────────╯"],
            Face::Options => &["──●───────", "──────●───", "────●─────"],
            Face::Keys => &["┌───┐ ┌───┐", "│Esc│ │ ↵ │", "└───┘ └───┘"],
            Face::Quit => &[" ╭─  │  ─╮ ", " │   ╵   │ ", " ╰───────╯ "],
        }
    }

    /// The small tile's picture, one row.
    fn glyph(self) -> &'static str {
        match self {
            Face::Resume => "▶",
            Face::Radio => "((╻))",
            Face::Lab => "▁▅█▅▁",
            Face::Options => "─●──",
            Face::Keys => "[↵]",
            Face::Quit => "×",
        }
    }
}

/// How the row is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Size {
    /// 20 x 10: the icon, the name, a line saying what it is.
    Large,
    /// 10 x 5: a glyph, the name, the key.
    Small,
    /// One row of names, for a screen too narrow for squares.
    Names,
}

impl Size {
    const LARGE: (u16, u16, u16) = (20, 10, 2);
    const SMALL: (u16, u16, u16) = (10, 5, 1);

    /// The largest size whose row fits `width`, and that leaves the menu
    /// below it at least `below` rows of a `height` tall body.
    pub fn for_area(width: u16, height: u16, tiles: usize, below: u16) -> Size {
        let fits = |(w, h, gap): (u16, u16, u16)| {
            let row = tiles as u16 * w + (tiles as u16).saturating_sub(1) * gap;
            row <= width && h + below <= height
        };
        if fits(Self::LARGE) {
            Size::Large
        } else if fits(Self::SMALL) {
            Size::Small
        } else {
            Size::Names
        }
    }

    /// Rows the tile row takes.
    pub fn height(self) -> u16 {
        match self {
            Size::Large => Self::LARGE.1,
            Size::Small => Self::SMALL.1,
            Size::Names => 1,
        }
    }

    fn dims(self) -> (u16, u16, u16) {
        match self {
            Size::Large => Self::LARGE,
            Size::Small => Self::SMALL,
            Size::Names => (0, 1, 2),
        }
    }
}

/// Draw the row of `faces`, centred in `area`.
///
/// `focused` says the focus is on the row rather than on the tree below:
/// the selected tile is then drawn at full strength, and at rest otherwise,
/// so the eye goes where the keys go. `resume` is the line under Resume: the
/// view it would resume.
#[allow(clippy::too_many_arguments)]
pub fn render(
    f: &mut Frame,
    area: Rect,
    size: Size,
    faces: &[Face],
    selected: Face,
    focused: bool,
    resume: &str,
    theme: &crate::Theme,
) {
    let buf = f.buffer_mut();
    if size == Size::Names {
        names(buf, area, faces, selected, focused, theme);
        return;
    }
    let (w, h, gap) = size.dims();
    let n = faces.len() as u16;
    let row = n * w + n.saturating_sub(1) * gap;
    let mut x = area.x + area.width.saturating_sub(row) / 2;
    for &face in faces {
        let rect = Rect::new(x, area.y, w, h).intersection(area);
        if rect.width == w && rect.height == h {
            let line = if face == Face::Resume {
                resume
            } else {
                face.line()
            };
            tile(
                buf,
                rect,
                size,
                face,
                face == selected,
                focused,
                line,
                theme,
            );
        }
        x += w + gap;
    }
}

#[allow(clippy::too_many_arguments)]
fn tile(
    buf: &mut Buffer,
    rect: Rect,
    size: Size,
    face: Face,
    selected: bool,
    focused: bool,
    line: &str,
    theme: &crate::Theme,
) {
    let color = face.color(theme);
    let (frame, kind) = match (selected, focused) {
        (true, true) => (color, BorderType::Thick),
        (true, false) => (ink::rest(color, theme), BorderType::Thick),
        (false, _) => (theme.border_dim, BorderType::Rounded),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(kind)
        .border_style(Style::default().fg(frame));
    let inner = block.inner(rect);
    block.render(rect, buf);

    let picture = if selected {
        color
    } else {
        ink::rest(color, theme)
    };
    let name = if selected {
        Style::default().fg(color).add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(theme.value)
            .add_modifier(Modifier::BOLD)
    };
    let quiet = Style::default().fg(if selected {
        theme.label
    } else {
        theme.border_dim
    });
    let centred =
        |text: &str, style: Style| Line::from(Span::styled(text.to_string(), style)).centered();

    let mut lines: Vec<Line> = Vec::new();
    match size {
        Size::Large => {
            let key_style = Style::default().fg(if selected {
                theme.value_hi
            } else {
                theme.label
            });
            lines.push(
                Line::from(vec![Span::styled(face.key(), key_style), Span::raw(" ")])
                    .right_aligned(),
            );
            // The icon sits in four rows whatever its height, so the names
            // line up across the row.
            let icon = face.icon();
            let pad = 4usize.saturating_sub(icon.len());
            let width = icon.iter().map(|r| r.chars().count()).max().unwrap_or(0);
            for _ in 0..pad / 2 {
                lines.push(Line::raw(""));
            }
            for row in icon {
                lines.push(centred(
                    &format!("{row:<width$}"),
                    Style::default().fg(picture),
                ));
            }
            for _ in 0..pad - pad / 2 {
                lines.push(Line::raw(""));
            }
            lines.push(Line::raw(""));
            lines.push(centred(face.name(), name));
            lines.push(centred(&clip(line, inner.width as usize), quiet));
        }
        Size::Small => {
            lines.push(centred(face.glyph(), Style::default().fg(picture)));
            lines.push(centred(face.name(), name));
            lines.push(centred(face.key(), Style::default().fg(theme.label)));
        }
        Size::Names => {}
    }
    Paragraph::new(lines).render(inner, buf);
}

/// The narrow form: the names in a row, the selected one marked as a row
/// in a list is.
fn names(
    buf: &mut Buffer,
    area: Rect,
    faces: &[Face],
    selected: Face,
    focused: bool,
    theme: &crate::Theme,
) {
    let mut spans = Vec::new();
    for &face in faces {
        let on = face == selected;
        let color = face.color(theme);
        let ink = match (on, focused) {
            (true, true) => color,
            (true, false) => ink::rest(color, theme),
            (false, _) => theme.value,
        };
        spans.push(Span::styled(
            if on { "▌" } else { " " },
            Style::default().fg(ink),
        ));
        let mut style = Style::default().fg(ink);
        if on {
            style = style.add_modifier(Modifier::BOLD);
        }
        spans.push(Span::styled(face.name(), style));
        spans.push(Span::raw(" "));
    }
    Paragraph::new(Line::from(spans)).render(area, buf);
}

fn clip(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let mut out: String = text.chars().take(width.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    fn draw(
        w: u16,
        h: u16,
        size: Size,
        resume: bool,
        selected: Face,
        focused: bool,
    ) -> Vec<String> {
        let theme = crate::Theme::sdr();
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal
            .draw(|f| {
                render(
                    f,
                    f.size(),
                    size,
                    &Face::shown(resume),
                    selected,
                    focused,
                    "Wi-Fi · Frames",
                    &theme,
                )
            })
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..h)
            .map(|y| (0..w).map(|x| buf.get(x, y).symbol().to_string()).collect())
            .collect()
    }

    /// Resume is first when there is something to resume, and absent when
    /// there is not.
    #[test]
    fn resume_leads_only_when_there_is_something_to_resume() {
        assert_eq!(Face::shown(true)[0], Face::Resume);
        assert_eq!(Face::shown(false)[0], Face::Radio);
        assert_eq!(Face::shown(false).len(), 5);
        let rows = draw(191, 10, Size::Large, false, Face::Radio, true).join("\n");
        assert!(!rows.contains("Resume"), "{rows}");
        let rows = draw(191, 10, Size::Large, true, Face::Resume, true).join("\n");
        assert!(
            rows.contains("Resume") && rows.contains("Wi-Fi · Frames"),
            "{rows}"
        );
    }

    /// A large tile is 20 x 10 and a small one 10 x 5: square at a 1:2 cell.
    #[test]
    fn the_tiles_are_square_on_screen() {
        let rows = draw(191, 10, Size::Large, true, Face::Lab, true);
        let top = &rows[0];
        let left = top.find('╭').unwrap();
        let right = top[left..].find('╮').unwrap();
        assert_eq!(top[left..left + right].chars().count() + 1, 20);
        assert!(rows[9].contains('╰'), "{}", rows[9]);
        let rows = draw(80, 5, Size::Small, true, Face::Lab, true);
        let left = rows[0].find('╭').unwrap();
        let right = rows[0][left..].find('╮').unwrap();
        assert_eq!(rows[0][left..left + right].chars().count() + 1, 10);
        assert!(rows[4].contains('╰'));
    }

    /// The selected tile is the one heavy frame, focused or not.
    #[test]
    fn the_selected_tile_has_the_heavy_frame() {
        for focused in [true, false] {
            let rows = draw(191, 10, Size::Large, true, Face::Lab, focused);
            assert_eq!(rows[0].matches('┏').count(), 1);
            let col = |row: &str, pat: &str| row[..row.find(pat).unwrap()].chars().count();
            let heavy = col(&rows[0], "┏");
            let lab = col(&rows[7], "Lab");
            assert!(heavy < lab && lab < heavy + 20, "{}", rows[7]);
        }
    }

    /// Every tile says its key: in the corner when large, under the name
    /// when small.
    #[test]
    fn every_tile_shows_its_key() {
        for size in [Size::Large, Size::Small] {
            let rows = draw(191, size.height(), size, true, Face::Lab, true).join("\n");
            for face in Face::ALL {
                assert!(rows.contains(face.key()), "{size:?} {face:?}");
                assert!(rows.contains(face.name()), "{size:?} {face:?}");
            }
        }
    }

    /// The size follows the space: large at Viktor's 191 x 41, small in the
    /// box over the deck and at 100 columns, names below that.
    #[test]
    fn the_size_follows_the_space() {
        assert_eq!(Size::for_area(189, 37, 6, 20), Size::Large);
        assert_eq!(Size::for_area(150, 28, 6, 20), Size::Small);
        assert_eq!(Size::for_area(98, 26, 6, 18), Size::Small);
        assert_eq!(Size::for_area(60, 26, 6, 18), Size::Names);
        assert_eq!(Size::for_area(189, 20, 6, 18), Size::Names);
    }

    /// Too narrow for squares, the names still all fit and say which is
    /// selected.
    #[test]
    fn the_narrow_form_names_every_tile() {
        let rows = draw(60, 1, Size::Names, true, Face::Keys, true);
        for face in Face::ALL {
            assert!(rows[0].contains(face.name()), "{}", rows[0]);
        }
        assert!(rows[0].contains("▌Keys"), "{}", rows[0]);
    }
}
