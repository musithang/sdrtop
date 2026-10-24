// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The card's frequency ruler: where a view listens, against what the radio
//! can reach.
//!
//! One log scale, 1 MHz to 7 GHz, for every radio and every view, so a band
//! sits in the same place whichever card shows it. The radio's reach is drawn
//! solid and the rest dotted; the band is a block on it, in the ink of a
//! reading when the radio covers all of it and of a fault when it does not.
//! The reach is the radio's own declaration (`DeviceCapabilities`), never a
//! table of what such a radio usually does.

use ratatui::{buffer::Buffer, layout::Rect, style::Style};

use super::tree::Band;

/// What the ruler marks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark<'a> {
    /// A band, and the name drawn over it.
    Band(Band, &'a str),
    /// A view that works wherever the radio is tuned: the reach is the mark.
    Anywhere,
}

/// Rows the ruler takes: the mark's name, the bar, the ticks, their labels,
/// and the reach in words.
pub const HEIGHT: u16 = 5;

const LOW_HZ: f64 = 1e6;
const HIGH_HZ: f64 = 7e9;

/// The column of `hz` across `width` cells, clamped to the ends: a radio
/// that reaches below 1 MHz is drawn from the left edge, not off it.
fn column(hz: f64, width: u16) -> u16 {
    let span = HIGH_HZ.log10() - LOW_HZ.log10();
    let t = ((hz.max(1.0).log10() - LOW_HZ.log10()) / span).clamp(0.0, 1.0);
    (t * f64::from(width.saturating_sub(1))).round() as u16
}

/// A frequency as the ruler and the card say it: `24 MHz`, `1.8 GHz`.
pub fn hz(f: u64) -> String {
    let f = f as f64;
    if f >= 1e9 {
        format!("{} GHz", trim(f / 1e9))
    } else if f >= 1e6 {
        format!("{} MHz", trim(f / 1e6))
    } else {
        format!("{} kHz", trim(f / 1e3))
    }
}

fn trim(v: f64) -> String {
    let s = format!("{v:.4}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// `radio` is the board's name and its reach, `min_hz..=max_hz`.
pub fn render(
    buf: &mut Buffer,
    area: Rect,
    radio: (&str, u64, u64),
    mark: Mark<'_>,
    theme: &crate::Theme,
) {
    if area.height < HEIGHT || area.width < 24 {
        return;
    }
    let (name, min_hz, max_hz) = radio;
    let w = area.width;
    let (x, y) = (area.x, area.y);
    let (lo, hi) = (column(min_hz as f64, w), column(max_hz as f64, w));
    for i in 0..w {
        let (glyph, ink) = if (lo..=hi).contains(&i) {
            ("━", theme.border_default)
        } else {
            ("┈", theme.stale)
        };
        buf.set_string(x + i, y + 1, glyph, Style::default().fg(ink));
    }
    let dim = Style::default().fg(theme.border_dim);
    let mut taken = 0u16;
    for (f, label) in [
        (1e6, "1 MHz"),
        (10e6, "10"),
        (100e6, "100 MHz"),
        (1e9, "1 GHz"),
        (6e9, "6 GHz"),
    ] {
        let at = column(f, w);
        buf.set_string(x + at, y + 2, "╵", dim);
        let len = label.chars().count() as u16;
        let start = at
            .saturating_sub(len / 2)
            .min(w.saturating_sub(len))
            .max(taken);
        if start + len <= w {
            buf.set_string(x + start, y + 3, label, dim);
            taken = start + len + 1;
        }
    }
    match mark {
        Mark::Band(band, label) => {
            let ok = band.within(min_hz, max_hz);
            let ink = if ok {
                theme.value_hi
            } else {
                theme.status_crit
            };
            let (a, b) = (column(band.lo_hz as f64, w), column(band.hi_hz as f64, w));
            for i in a..=b.max(a) {
                buf.set_string(x + i, y + 1, "█", Style::default().fg(ink));
            }
            // The name over the band, pointing down at it; turned round when
            // there is no room to its right.
            let right = format!("▼ {label}");
            let len = right.chars().count() as u16;
            let (text, at) = if a + len <= w {
                (right, a)
            } else {
                (format!("{label} ▼"), a.saturating_sub(len - 1))
            };
            buf.set_string(
                x + at,
                y,
                text,
                Style::default()
                    .fg(ink)
                    .add_modifier(ratatui::style::Modifier::BOLD),
            );
        }
        Mark::Anywhere => {
            buf.set_string(
                x,
                y,
                "wherever you tune: the whole reach",
                Style::default().fg(theme.label),
            );
        }
    }
    buf.set_string(
        x,
        y + 4,
        format!("{name} reaches {} to {}", hz(min_hz), hz(max_hz)),
        Style::default().fg(theme.label),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draw(radio: (&str, u64, u64), mark: Mark<'_>) -> Vec<String> {
        let area = Rect::new(0, 0, 64, HEIGHT);
        let mut buf = Buffer::empty(area);
        render(&mut buf, area, radio, mark, &crate::Theme::sdr());
        (0..HEIGHT)
            .map(|y| {
                (0..64)
                    .map(|x| buf.get(x, y).symbol().to_string())
                    .collect()
            })
            .collect()
    }

    const BAND: Band = Band {
        lo_hz: 2_402_000_000,
        hi_hz: 2_483_500_000,
    };

    /// The reach is drawn solid, the rest dotted, and said in words.
    #[test]
    fn the_reach_is_the_radios_own() {
        let rows = draw(("Airspy R2", 24_000_000, 1_800_000_000), Mark::Anywhere);
        assert!(rows[1].starts_with('┈'), "{}", rows[1]);
        assert!(rows[1].ends_with('┈'), "{}", rows[1]);
        assert!(rows[1].contains('━'));
        assert_eq!(rows[4].trim_end(), "Airspy R2 reaches 24 MHz to 1.8 GHz");
        let rows = draw(("HackRF One", 1_000_000, 6_000_000_000), Mark::Anywhere);
        assert!(rows[1].starts_with('━'), "{}", rows[1]);
    }

    /// A band is marked where it lies, named, in the colour that says
    /// whether the radio covers it.
    #[test]
    fn the_band_is_marked_and_named() {
        let theme = crate::Theme::sdr();
        for (top, ok) in [(6_000_000_000, true), (1_800_000_000, false)] {
            let area = Rect::new(0, 0, 64, HEIGHT);
            let mut buf = Buffer::empty(area);
            render(
                &mut buf,
                area,
                ("radio", 24_000_000, top),
                Mark::Band(BAND, "2.4 GHz"),
                &theme,
            );
            let bar: String = (0..64)
                .map(|x| buf.get(x, 1).symbol().to_string())
                .collect();
            let at = bar
                .chars()
                .position(|c| c == '█')
                .expect("the band is drawn");
            let want = if ok {
                theme.value_hi
            } else {
                theme.status_crit
            };
            assert_eq!(buf.get(at as u16, 1).fg, want);
            let top_row: String = (0..64)
                .map(|x| buf.get(x, 0).symbol().to_string())
                .collect();
            assert!(top_row.contains("2.4 GHz"), "{top_row}");
        }
    }

    #[test]
    fn frequencies_read_as_a_person_says_them() {
        assert_eq!(hz(24_000_000), "24 MHz");
        assert_eq!(hz(1_800_000_000), "1.8 GHz");
        assert_eq!(hz(2_483_500_000), "2.4835 GHz");
        assert_eq!(hz(500_000), "500 kHz");
    }
}
