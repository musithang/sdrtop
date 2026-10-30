// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! A braille dot canvas with layers: two dots across and four down a cell,
//! every dot set with a priority and a colour.
//!
//! **One colour a cell, the most important dot's.** A braille cell has one
//! foreground, so when a coastline dot and a grid dot share a cell, the cell
//! is drawn in the colour of whichever was set with the higher priority, and
//! every dot of the cell is lit. The map sets land low, lines above it,
//! stations on top, so what matters wins the cell.

use ratatui::{buffer::Buffer, layout::Rect, style::Color};

/// The bit of a braille cell for the dot at (`dx`, `dy`) inside it.
const BITS: [[u8; 2]; 4] = [[0x01, 0x08], [0x02, 0x10], [0x04, 0x20], [0x40, 0x80]];

#[derive(Clone, Copy, Default)]
struct Cell {
    bits: u8,
    priority: u8,
    color: Option<Color>,
}

pub struct Dots {
    cols: usize,
    rows: usize,
    cells: Vec<Cell>,
}

impl Dots {
    /// A canvas of `cols` by `rows` cells: twice as many dots across, four
    /// times as many down.
    pub fn new(cols: usize, rows: usize) -> Self {
        Self {
            cols,
            rows,
            cells: vec![Cell::default(); cols * rows],
        }
    }

    pub fn width(&self) -> usize {
        self.cols * 2
    }

    pub fn height(&self) -> usize {
        self.rows * 4
    }

    /// Light the dot at (`x`, `y`); outside the canvas, nothing.
    pub fn set(&mut self, x: i64, y: i64, priority: u8, color: Color) {
        if x < 0 || y < 0 || x as usize >= self.width() || y as usize >= self.height() {
            return;
        }
        let (x, y) = (x as usize, y as usize);
        let cell = &mut self.cells[(y / 4) * self.cols + x / 2];
        cell.bits |= BITS[y % 4][x % 2];
        if cell.color.is_none() || priority >= cell.priority {
            cell.priority = priority;
            cell.color = Some(color);
        }
    }

    /// Whether any dot of cell (`col`, `row`) is lit.
    #[cfg(test)]
    pub fn lit(&self, col: usize, row: usize) -> bool {
        col < self.cols && row < self.rows && self.cells[row * self.cols + col].bits != 0
    }

    /// Draw into `area` from its top left; cells with no dot are left as
    /// they are.
    pub fn render(&self, buf: &mut Buffer, area: Rect) {
        for row in 0..self.rows.min(area.height as usize) {
            for col in 0..self.cols.min(area.width as usize) {
                let c = self.cells[row * self.cols + col];
                let Some(color) = c.color.filter(|_| c.bits != 0) else {
                    continue;
                };
                let ch = char::from_u32(0x2800 + u32::from(c.bits)).unwrap_or(' ');
                buf.get_mut(area.x + col as u16, area.y + row as u16)
                    .set_char(ch)
                    .set_fg(color);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dots_land_in_their_braille_bits() {
        let mut d = Dots::new(2, 1);
        d.set(0, 0, 1, Color::Red);
        d.set(1, 3, 1, Color::Red);
        d.set(3, 1, 1, Color::Blue);
        let mut buf = Buffer::empty(Rect::new(0, 0, 2, 1));
        d.render(&mut buf, Rect::new(0, 0, 2, 1));
        assert_eq!(buf.get(0, 0).symbol(), "\u{2881}");
        assert_eq!(buf.get(1, 0).symbol(), "\u{2810}");
        assert_eq!(buf.get(1, 0).fg, Color::Blue);
    }

    #[test]
    fn the_higher_priority_dot_colours_the_cell() {
        let mut d = Dots::new(1, 1);
        d.set(0, 0, 5, Color::Yellow);
        d.set(1, 1, 1, Color::Gray);
        let mut buf = Buffer::empty(Rect::new(0, 0, 1, 1));
        d.render(&mut buf, Rect::new(0, 0, 1, 1));
        assert_eq!(buf.get(0, 0).fg, Color::Yellow);
        assert_eq!(buf.get(0, 0).symbol(), "\u{2811}");
    }

    #[test]
    fn outside_the_canvas_is_ignored() {
        let mut d = Dots::new(1, 1);
        d.set(-1, 0, 1, Color::Red);
        d.set(2, 0, 1, Color::Red);
        d.set(0, 4, 1, Color::Red);
        assert!(!d.lit(0, 0));
    }
}
