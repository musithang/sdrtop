// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! A character display's dot matrix, drawn in terminal cells.
//!
//! **Two dots a cell.** A cell's upper half block takes its top dot as the
//! foreground and its bottom dot as the background, so a cell is one dot wide
//! and two tall. A terminal cell is about twice as tall as it is wide, so the
//! dots come out square, and a character of 5 x 8 dots is 5 cells by 4.
//!
//! **Our own font.** Every printable 7-bit character, 5 dots wide and 7 tall,
//! lowercase with its descender in an eighth row, as character displays draw
//! them; drawn for this display, not taken from anywhere. A character a page
//! lost is [`LOST`], a checker no letter looks like, so a hole in a message
//! reads as a hole; anything outside 7-bit is a hollow box.
//!
//! **Unlit dots show.** A real display's cells are visible when off: the dots
//! of a character's 5 x 8 that are not lit are drawn in the `ghost` ink, a
//! shade off the glass, and the gaps between characters are bare glass.
//!
//! Every colour comes from the caller's [`Look`], which comes from the theme.

use ratatui::{buffer::Buffer, layout::Rect, style::Color};

/// A character that was lost on the way: drawn as a checker. The page's
/// own mark for one (`signal::pocsag::page::LOST`), so a page's text goes
/// to the display as it is.
pub const LOST: char = crate::signal::pocsag::page::LOST;

/// Dots across one character, and the gap after it.
const DOTS: u16 = 5;
/// Cells one character takes across, gap included.
pub const CHAR_CELLS: u16 = DOTS + 1;
/// Cells one line takes down: eight dots, two a cell.
pub const LINE_CELLS: u16 = 4;

/// The display's three inks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Look {
    pub glass: Color,
    pub ink: Color,
    pub ghost: Color,
}

/// Characters that fit across `width` cells: the last needs no gap after it.
pub fn chars_across(width: u16) -> usize {
    usize::from((width + 1) / CHAR_CELLS)
}

/// Cells across for `chars` characters.
pub fn width_of(chars: usize) -> u16 {
    (chars as u16 * CHAR_CELLS).saturating_sub(1)
}

/// Break `text` into lines of at most `chars`, at spaces where it can, and
/// mid-word where a word is longer than a line, as a pager does.
pub fn wrap(text: &str, chars: usize) -> Vec<String> {
    let chars = chars.max(1);
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split(' ') {
        let mut word: Vec<char> = word.chars().collect();
        loop {
            let used = line.chars().count();
            let gap = usize::from(used > 0);
            if used + gap + word.len() <= chars {
                if gap == 1 {
                    line.push(' ');
                }
                line.extend(word.iter());
                break;
            }
            if used > 0 {
                lines.push(std::mem::take(&mut line));
                continue;
            }
            let rest = word.split_off(chars);
            lines.push(word.iter().collect());
            word = rest;
        }
    }
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

/// Draw `lines` into `area`, one line every [`LINE_CELLS`] rows, as many
/// characters as fit across. The whole area is glass first.
pub fn render(buf: &mut Buffer, area: Rect, lines: &[&str], look: Look) {
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            buf.get_mut(x, y).set_symbol(" ").set_bg(look.glass);
        }
    }
    let across = chars_across(area.width);
    for (li, text) in lines.iter().enumerate() {
        let top = area.y + li as u16 * LINE_CELLS;
        if top + LINE_CELLS > area.bottom() {
            break;
        }
        for (ci, ch) in text.chars().take(across).enumerate() {
            let left = area.x + ci as u16 * CHAR_CELLS;
            let rows = glyph(ch);
            for cy in 0..LINE_CELLS {
                for cx in 0..DOTS {
                    let dot = |row: u16| {
                        if rows[usize::from(row)] >> (DOTS - 1 - cx) & 1 == 1 {
                            look.ink
                        } else {
                            look.ghost
                        }
                    };
                    buf.get_mut(left + cx, top + cy)
                        .set_symbol("\u{2580}")
                        .set_fg(dot(cy * 2))
                        .set_bg(dot(cy * 2 + 1));
                }
            }
        }
    }
}

/// A character's eight rows of five dots, the leftmost dot the highest bit.
pub fn glyph(c: char) -> [u8; 8] {
    match c {
        ' ' => [0, 0, 0, 0, 0, 0, 0, 0],
        '!' => [0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0, 0b00100, 0],
        '"' => [0b01010, 0b01010, 0b01010, 0, 0, 0, 0, 0],
        '#' => [
            0b01010, 0b01010, 0b11111, 0b01010, 0b11111, 0b01010, 0b01010, 0,
        ],
        '$' => [
            0b00100, 0b01111, 0b10100, 0b01110, 0b00101, 0b11110, 0b00100, 0,
        ],
        '%' => [
            0b11000, 0b11001, 0b00010, 0b00100, 0b01000, 0b10011, 0b00011, 0,
        ],
        '&' => [
            0b01100, 0b10010, 0b10100, 0b01000, 0b10101, 0b10010, 0b01101, 0,
        ],
        '\'' => [0b00100, 0b00100, 0b01000, 0, 0, 0, 0, 0],
        '(' => [
            0b00010, 0b00100, 0b01000, 0b01000, 0b01000, 0b00100, 0b00010, 0,
        ],
        ')' => [
            0b01000, 0b00100, 0b00010, 0b00010, 0b00010, 0b00100, 0b01000, 0,
        ],
        '*' => [0, 0b00100, 0b10101, 0b01110, 0b10101, 0b00100, 0, 0],
        '+' => [0, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0, 0],
        ',' => [0, 0, 0, 0, 0b01100, 0b00100, 0b01000, 0],
        '-' => [0, 0, 0, 0b11111, 0, 0, 0, 0],
        '.' => [0, 0, 0, 0, 0, 0b01100, 0b01100, 0],
        '/' => [0, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0, 0],
        '0' => [
            0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110, 0,
        ],
        '1' => [
            0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110, 0,
        ],
        '2' => [
            0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111, 0,
        ],
        '3' => [
            0b11111, 0b00010, 0b00100, 0b00010, 0b00001, 0b10001, 0b01110, 0,
        ],
        '4' => [
            0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010, 0,
        ],
        '5' => [
            0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110, 0,
        ],
        '6' => [
            0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110, 0,
        ],
        '7' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000, 0,
        ],
        '8' => [
            0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110, 0,
        ],
        '9' => [
            0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100, 0,
        ],
        ':' => [0, 0b01100, 0b01100, 0, 0b01100, 0b01100, 0, 0],
        ';' => [0, 0b01100, 0b01100, 0, 0b01100, 0b00100, 0b01000, 0],
        '<' => [
            0b00010, 0b00100, 0b01000, 0b10000, 0b01000, 0b00100, 0b00010, 0,
        ],
        '=' => [0, 0, 0b11111, 0, 0b11111, 0, 0, 0],
        '>' => [
            0b01000, 0b00100, 0b00010, 0b00001, 0b00010, 0b00100, 0b01000, 0,
        ],
        '?' => [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0, 0b00100, 0],
        '@' => [
            0b01110, 0b10001, 0b00001, 0b01101, 0b10101, 0b10101, 0b01110, 0,
        ],
        'A' => [
            0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001, 0,
        ],
        'B' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110, 0,
        ],
        'C' => [
            0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110, 0,
        ],
        'D' => [
            0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110, 0,
        ],
        'E' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111, 0,
        ],
        'F' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000, 0,
        ],
        'G' => [
            0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01111, 0,
        ],
        'H' => [
            0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001, 0,
        ],
        'I' => [
            0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110, 0,
        ],
        'J' => [
            0b00111, 0b00010, 0b00010, 0b00010, 0b00010, 0b10010, 0b01100, 0,
        ],
        'K' => [
            0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001, 0,
        ],
        'L' => [
            0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111, 0,
        ],
        'M' => [
            0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001, 0,
        ],
        'N' => [
            0b10001, 0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0,
        ],
        'O' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110, 0,
        ],
        'P' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000, 0,
        ],
        'Q' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101, 0,
        ],
        'R' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001, 0,
        ],
        'S' => [
            0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110, 0,
        ],
        'T' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0,
        ],
        'U' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110, 0,
        ],
        'V' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100, 0,
        ],
        'W' => [
            0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b10101, 0b01010, 0,
        ],
        'X' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001, 0,
        ],
        'Y' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100, 0,
        ],
        'Z' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111, 0,
        ],
        '[' => [
            0b01110, 0b01000, 0b01000, 0b01000, 0b01000, 0b01000, 0b01110, 0,
        ],
        '\\' => [0, 0b10000, 0b01000, 0b00100, 0b00010, 0b00001, 0, 0],
        ']' => [
            0b01110, 0b00010, 0b00010, 0b00010, 0b00010, 0b00010, 0b01110, 0,
        ],
        '^' => [0b00100, 0b01010, 0b10001, 0, 0, 0, 0, 0],
        '_' => [0, 0, 0, 0, 0, 0, 0b11111, 0],
        '`' => [0b01000, 0b00100, 0b00010, 0, 0, 0, 0, 0],
        'a' => [0, 0, 0b01110, 0b00001, 0b01111, 0b10001, 0b01111, 0],
        'b' => [
            0b10000, 0b10000, 0b10110, 0b11001, 0b10001, 0b10001, 0b11110, 0,
        ],
        'c' => [0, 0, 0b01110, 0b10000, 0b10000, 0b10001, 0b01110, 0],
        'd' => [
            0b00001, 0b00001, 0b01101, 0b10011, 0b10001, 0b10001, 0b01111, 0,
        ],
        'e' => [0, 0, 0b01110, 0b10001, 0b11111, 0b10000, 0b01110, 0],
        'f' => [
            0b00110, 0b01001, 0b01000, 0b11100, 0b01000, 0b01000, 0b01000, 0,
        ],
        'g' => [0, 0, 0b01111, 0b10001, 0b10001, 0b01111, 0b00001, 0b01110],
        'h' => [
            0b10000, 0b10000, 0b10110, 0b11001, 0b10001, 0b10001, 0b10001, 0,
        ],
        'i' => [0b00100, 0, 0b01100, 0b00100, 0b00100, 0b00100, 0b01110, 0],
        'j' => [
            0b00010, 0, 0b00110, 0b00010, 0b00010, 0b00010, 0b10010, 0b01100,
        ],
        'k' => [
            0b10000, 0b10000, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0,
        ],
        'l' => [
            0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110, 0,
        ],
        'm' => [0, 0, 0b11010, 0b10101, 0b10101, 0b10001, 0b10001, 0],
        'n' => [0, 0, 0b10110, 0b11001, 0b10001, 0b10001, 0b10001, 0],
        'o' => [0, 0, 0b01110, 0b10001, 0b10001, 0b10001, 0b01110, 0],
        'p' => [0, 0, 0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000],
        'q' => [0, 0, 0b01101, 0b10011, 0b10011, 0b01101, 0b00001, 0b00001],
        'r' => [0, 0, 0b10110, 0b11001, 0b10000, 0b10000, 0b10000, 0],
        's' => [0, 0, 0b01110, 0b10000, 0b01110, 0b00001, 0b11110, 0],
        't' => [
            0b01000, 0b01000, 0b11100, 0b01000, 0b01000, 0b01001, 0b00110, 0,
        ],
        'u' => [0, 0, 0b10001, 0b10001, 0b10001, 0b10011, 0b01101, 0],
        'v' => [0, 0, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100, 0],
        'w' => [0, 0, 0b10001, 0b10001, 0b10101, 0b10101, 0b01010, 0],
        'x' => [0, 0, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0],
        'y' => [0, 0, 0b10001, 0b10001, 0b10001, 0b01111, 0b00001, 0b01110],
        'z' => [0, 0, 0b11111, 0b00010, 0b00100, 0b01000, 0b11111, 0],
        '{' => [
            0b00010, 0b00100, 0b00100, 0b01000, 0b00100, 0b00100, 0b00010, 0,
        ],
        '|' => [
            0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0,
        ],
        '}' => [
            0b01000, 0b00100, 0b00100, 0b00010, 0b00100, 0b00100, 0b01000, 0,
        ],
        '~' => [0, 0, 0b01000, 0b10101, 0b00010, 0, 0, 0],
        '\u{B7}' => [0, 0, 0, 0b00100, 0, 0, 0, 0],
        LOST => [
            0b10101, 0b01010, 0b10101, 0b01010, 0b10101, 0b01010, 0b10101, 0,
        ],
        _ => [
            0b11111, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11111, 0,
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOOK: Look = Look {
        glass: Color::Rgb(150, 170, 130),
        ink: Color::Rgb(20, 30, 20),
        ghost: Color::Rgb(140, 160, 125),
    };

    /// The lit dots of the first character drawn, as rows of `#` and `.`,
    /// read back off the buffer's halves.
    fn dots(text: &str) -> Vec<String> {
        let area = Rect::new(0, 0, width_of(text.chars().count()), LINE_CELLS);
        let mut buf = Buffer::empty(area);
        render(&mut buf, area, &[text], LOOK);
        (0..8)
            .map(|row| {
                (0..DOTS)
                    .map(|x| {
                        let c = buf.get(x, row / 2);
                        let ink = if row % 2 == 0 { c.fg } else { c.bg };
                        if ink == LOOK.ink {
                            '#'
                        } else {
                            '.'
                        }
                    })
                    .collect()
            })
            .collect()
    }

    /// A character is its glyph, dot for dot, top dot as the half block's
    /// foreground, bottom as its background.
    #[test]
    fn a_character_is_its_dots() {
        assert_eq!(
            dots("A"),
            [".###.", "#...#", "#...#", "#####", "#...#", "#...#", "#...#", "....."]
        );
        assert_eq!(dots("g")[7], ".###.", "the descender is in the eighth row");
    }

    /// Unlit dots are ghosts; the gap after a character is bare glass.
    #[test]
    fn unlit_dots_are_ghosts_and_gaps_are_glass() {
        let area = Rect::new(0, 0, width_of(2), LINE_CELLS);
        let mut buf = Buffer::empty(area);
        render(&mut buf, area, &["AB"], LOOK);
        assert_eq!(buf.get(0, 0).fg, LOOK.ghost, "A's top-left dot is off");
        assert_eq!(buf.get(DOTS, 0).bg, LOOK.glass, "the gap column");
        assert_eq!(buf.get(DOTS, 0).symbol(), " ");
    }

    /// Every printable 7-bit character has a glyph of its own: none falls
    /// through to the hollow box, and no two share one where they should not.
    #[test]
    fn every_printable_character_is_drawn() {
        let unknown = glyph('\u{2603}');
        for b in 0x20u8..=0x7E {
            let c = b as char;
            assert_ne!(glyph(c), unknown, "{c:?} has no glyph");
        }
        assert_ne!(glyph(LOST), unknown);
        assert_ne!(glyph('0'), glyph('O'));
        assert_ne!(glyph('1'), glyph('l'));
        assert_ne!(glyph('['), glyph(']'));
    }

    /// Nothing is drawn past the area, and what does not fit is left off,
    /// not squeezed.
    #[test]
    fn it_stays_in_its_area() {
        let area = Rect::new(2, 1, width_of(3), LINE_CELLS);
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 10));
        render(&mut buf, area, &["ABCDEFG", "second", "third"], LOOK);
        for y in 0..10 {
            for x in 0..30 {
                if !area.contains(ratatui::layout::Position { x, y }) {
                    assert_eq!(buf.get(x, y).symbol(), " ", "{x},{y}");
                }
            }
        }
    }

    #[test]
    fn sizes_add_up() {
        assert_eq!(width_of(16), 95);
        assert_eq!(chars_across(95), 16);
        assert_eq!(chars_across(100), 16);
        assert_eq!(chars_across(101), 17);
    }

    /// Lines break at spaces, and a word longer than a line is cut, as a
    /// pager cuts it.
    #[test]
    fn text_wraps_as_a_pager_wraps_it() {
        assert_eq!(
            wrap("WX BUDAPEST 14C, SZEL DNY", 16),
            ["WX BUDAPEST 14C,", "SZEL DNY"]
        );
        assert_eq!(wrap("ABCDEFGHIJ", 4), ["ABCD", "EFGH", "IJ"]);
        assert_eq!(wrap("", 16), [""]);
    }
}
