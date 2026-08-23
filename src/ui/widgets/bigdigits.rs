//! Three-row block-glyph digits for the Command Rail's frequency hero.
//!
//! Each character is a 3×3 cell sprite drawn with the half-block elements
//! `▀ ▄ █` (each exactly one terminal column), so `92.800` renders as a big,
//! instrument-panel readout instead of a normal line of text. The caller draws
//! the three rows itself (one styled `Span` per character) so it can colour the
//! actively-tuned digit differently — see [`glyph`].

/// Cell width of a single big glyph (columns).
pub const GLYPH_W: usize = 3;

/// The 3-row sprite for one character. Unknown characters render as blank cells,
/// so callers never have to pre-filter the string.
pub fn glyph(c: char) -> [&'static str; 3] {
    match c {
        '0' => ["█▀█", "█ █", "▀▀▀"],
        '1' => [" █ ", " █ ", " ▀ "],
        '2' => ["▀▀█", "█▀▀", "▀▀▀"],
        '3' => ["▀▀█", " ▀█", "▀▀▀"],
        '4' => ["█ █", "▀▀█", "  ▀"],
        '5' => ["█▀▀", "▀▀█", "▀▀▀"],
        '6' => ["█▀▀", "█▀█", "▀▀▀"],
        '7' => ["▀▀█", "  █", "  ▀"],
        '8' => ["█▀█", "█▀█", "▀▀▀"],
        '9' => ["█▀█", "▀▀█", "▀▀▀"],
        '.' => ["   ", "   ", " ▄ "],
        _   => ["   ", "   ", "   "],
    }
}

/// Rendered width (columns) of `s` in big glyphs, including one blank column of
/// gap between adjacent characters. Used to decide whether the rail is wide
/// enough for the big readout or must fall back to a single line.
pub fn big_width(s: &str) -> usize {
    let n = s.chars().count();
    if n == 0 { 0 } else { n * GLYPH_W + (n - 1) }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Every glyph cell here is a single-column char (space or a block element),
    // so `chars().count()` equals the display width — no unicode-width needed.

    #[test]
    fn every_glyph_row_is_three_columns() {
        for c in "0123456789. ".chars() {
            for (r, row) in glyph(c).iter().enumerate() {
                assert_eq!(row.chars().count(), GLYPH_W, "char {c:?} row {r} not {GLYPH_W} cols");
            }
        }
    }

    #[test]
    fn big_width_counts_glyphs_and_gaps() {
        assert_eq!(big_width(""), 0);
        assert_eq!(big_width("9"), 3);
        assert_eq!(big_width("92"), 3 + 1 + 3);
        // "92.800" → 6 chars × 3 + 5 gaps = 23
        assert_eq!(big_width("92.800"), 23);
    }
}
