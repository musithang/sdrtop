// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The time-frequency canvas: what was on the band, and when.
//!
//! Design section 9.1 calls this idiom C and the most demanding drawing in the
//! app. X is time, Y is frequency across the band - the other way round from the
//! waterfall, deliberately, because a coexistence picture is read as a timeline
//! with the band down its side.
//!
//! **Half blocks, not braille.** A braille cell packs four rows of dots into one
//! character, but all eight dots share one colour, and this canvas is a heatmap:
//! the colour *is* the reading. A half block carries two frequency cells with
//! two independent colours (the foreground of `▀` and the background behind it),
//! so it fits twice the band into a row and still says something different about
//! each half.
//!
//! **Three states, as everywhere else in this section.** Busy is a colour on the
//! intensity ramp, observed-and-empty is the ramp's floor, and never-observed is
//! left dark. A canvas that painted "nobody looked" in the same ink as "nothing
//! was there" would be inventing testimony over whichever part of the band the
//! receiver could not reach.

use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};

/// The character that carries two frequency cells: its foreground is the upper
/// one, its background the lower.
const HALF: char = '\u{2580}';

/// What a cell of the canvas holds.
///
/// `None` is "nobody looked here", which is a different answer from `Some(0.0)`
/// and is drawn differently.
pub(crate) type Duty = Option<f32>;

/// The colour for one cell.
///
/// **Unobserved is `theme.stale`**, which is the ink the occupancy profile
/// already uses for a cell nobody looked at, so one meaning has one colour
/// across the section (rule 5). Everything else rides the theme's palette, the
/// same ramp the waterfall uses, so one intensity means one thing across the
/// app.
///
/// The first version used `border_dim`, which passed the test that the three
/// inks differ and failed on the radio: in the default theme it is *lighter*
/// than the palette's floor, so the part of the band nobody had measured was
/// the brightest thing on the panel. An ink for absence has to sit below the
/// scale, not above it, and the test below now says so rather than only that it
/// is different.
pub(crate) fn ink(duty: Duty, theme: &crate::Theme) -> Color {
    match duty.filter(|d| d.is_finite()) {
        Some(d) => theme.palette_color(d.clamp(0.0, 1.0)),
        // Not the palette's floor: that is a measurement of an empty channel,
        // and this is the absence of one.
        None => theme.stale,
    }
}

/// One row of the canvas: `upper` and `lower` are the two lines of cells this
/// row of characters carries in its half blocks, one duty per column. On the
/// coexistence heatmap they are two moments, the newer on top.
pub(crate) fn row(upper: &[Duty], lower: &[Duty], theme: &crate::Theme) -> Line<'static> {
    let spans = (0..upper.len())
        .map(|x| {
            // A band with no lower half - the last row of an odd count - is not
            // drawn as quiet, it is drawn as unlooked-at, because that is what
            // it is.
            let below = lower.get(x).copied().flatten();
            Span::styled(
                HALF.to_string(),
                Style::default()
                    .fg(ink(upper[x], theme))
                    .bg(ink(below, theme)),
            )
        })
        .collect::<Vec<_>>();
    Line::from(spans)
}

/// Fold a column of frequency cells into `bands` bins, one per canvas column on
/// the coexistence heatmap.
///
/// **The busiest, not the mean**, for the reason the occupancy profile folds the
/// same way: averaging a saturated megahertz with a quiet one produces two
/// half-busy ones and hides the thing the panel exists to show. A band of cells
/// nobody looked at stays unlooked-at.
/// The range of cells one band covers. The single account of the mapping, so
/// [`fold`] and anything that needs its inverse cannot disagree: the NET band
/// axis (`ui::panels::net::shared::band_axis`) is this function, not a copy.
pub(crate) fn band_range(cells: usize, bands: usize, band: usize) -> std::ops::Range<usize> {
    let lo = band * cells / bands.max(1);
    let hi = ((band + 1) * cells / bands.max(1)).max(lo + 1).min(cells);
    lo..hi
}

/// Which band a cell falls in.
///
/// **Not `cell * bands / cells`.** That is the obvious inverse and it is wrong:
/// [`fold`] slices with two independent floors, so the boundaries do not line
/// up with a single division, and cell 41 of 83 into 22 bands comes out one band
/// low. Derived from the same ranges instead.
#[cfg(test)] // the inverse only a test needs: the panel folds forwards
pub(crate) fn band_of(cell: usize, cells: usize, bands: usize) -> usize {
    (0..bands)
        .find(|b| band_range(cells, bands, *b).contains(&cell))
        .unwrap_or(bands.saturating_sub(1))
}

pub(crate) fn fold(column: &[f32], bands: usize) -> Vec<Duty> {
    (0..bands)
        .map(|b| {
            if column.is_empty() {
                return None;
            }
            column[band_range(column.len(), bands, b)]
                .iter()
                // A negative is the history's mark for a cell nobody looked at.
                .filter(|d| **d >= 0.0)
                .copied()
                .fold(None, |best: Option<f32>, d| {
                    Some(best.map_or(d, |b| b.max(d)))
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> crate::Theme {
        crate::Theme::sdr()
    }

    /// Never-observed is not drawn as empty, and empty is not drawn as busy.
    #[test]
    fn the_three_states_are_three_inks() {
        let t = theme();
        let unseen = ink(None, &t);
        let empty = ink(Some(0.0), &t);
        let busy = ink(Some(1.0), &t);
        assert_ne!(unseen, empty, "nobody looked is not nothing was there");
        assert_ne!(empty, busy);
        assert_ne!(unseen, busy);
        // And the ramp is the theme's, not a literal.
        assert_eq!(busy, t.palette_color(1.0));
        assert_eq!(empty, t.palette_color(0.0));

        // **Absence must not be a colour the scale can produce.** Differing from
        // the two ends is not enough: an ink that lands anywhere on the ramp can
        // be read as a duty cycle, which is the one thing it must never be
        // mistaken for.
        //
        // The first version used `border_dim` and asserted only that the three
        // differ. It passed, and on a radio the part of the band nobody had
        // measured was the *brightest* thing on the panel - a light blue-grey
        // above the whole ramp. Only visible by looking, which is why this panel
        // has an acceptance criterion that is not a test.
        for i in 0..=100 {
            assert_ne!(
                unseen,
                t.palette_color(i as f32 / 100.0),
                "the ink for absence is on the intensity ramp at {i} %"
            );
        }

        // And it is the same ink the occupancy profile uses for the same thing.
        assert_eq!(unseen, t.stale);
    }

    /// A busier cell is further up the ramp, monotonically.
    #[test]
    fn intensity_follows_the_duty_cycle() {
        let t = theme();
        let at = |d: f32| ink(Some(d), &t);
        assert_eq!(at(0.5), t.palette_color(0.5));
        // Out of range is clamped rather than wrapping round the ramp.
        assert_eq!(at(-3.0), at(0.0));
        assert_eq!(at(9.0), at(1.0));
        assert_eq!(at(f32::NAN), ink(None, &t), "not a number is not a reading");
    }

    /// Two frequency cells to a character, each with its own colour.
    #[test]
    fn a_row_carries_two_bands_with_two_colours() {
        let t = theme();
        let line = row(
            &[Some(1.0), Some(0.0), None],
            &[Some(0.0), Some(1.0), None],
            &t,
        );
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text.chars().count(), 3, "one character per column");
        assert!(text.chars().all(|c| c == HALF), "{text:?}");
        // The upper band is the foreground, the lower the background, so one
        // character says two different things.
        let first = &line.spans[0];
        assert_eq!(first.style.fg, Some(ink(Some(1.0), &t)));
        assert_eq!(first.style.bg, Some(ink(Some(0.0), &t)));
        let second = &line.spans[1];
        assert_eq!(second.style.fg, Some(ink(Some(0.0), &t)));
        assert_eq!(second.style.bg, Some(ink(Some(1.0), &t)));
    }

    /// A short lower row is not a reason to misalign the upper one: the band has
    /// an odd number of cells and the last row has only a top half.
    #[test]
    fn an_odd_band_leaves_the_last_half_unobserved() {
        let t = theme();
        let line = row(&[Some(1.0)], &[], &t);
        assert_eq!(line.spans.len(), 1);
        assert_eq!(line.spans[0].style.bg, Some(ink(None, &t)));
    }

    /// **The inverse of the fold is the fold's own ranges, not a division.**
    ///
    /// `cell * bands / cells` is the obvious guess and it is off by one wherever
    /// the two floors disagree - cell 41 of 83 into 22 bands lands in band 11 and
    /// the guess says 10. A test that predicted the position that way failed on
    /// correct code, which is the expensive way to find this out.
    #[test]
    fn a_cell_is_in_the_band_that_folds_it() {
        for bands in [6usize, 22, 40, 46, 83, 90] {
            for cell in 0..83usize {
                let band = band_of(cell, 83, bands);
                assert!(band < bands, "{bands} bands: cell {cell} -> {band}");
                // The band that claims it is the band that folds it.
                let mut column = vec![0.0f32; 83];
                column[cell] = 1.0;
                let folded = fold(&column, bands);
                assert_eq!(
                    folded[band],
                    Some(1.0),
                    "{bands} bands: cell {cell} should fold into band {band}"
                );
                // One band per cell only while there are cells to spare: past
                // that a cell is repeated across the rows it spans, which is
                // the same behaviour `folding_keeps_the_busiest_cell...` pins.
                if bands <= 83 {
                    assert_eq!(
                        folded.iter().filter(|d| **d == Some(1.0)).count(),
                        1,
                        "{bands} bands: cell {cell} lit more than one band"
                    );
                }
            }
        }
    }

    /// Folding keeps the busiest cell, and keeps "nobody looked" honest.
    #[test]
    fn folding_keeps_the_busiest_cell_and_the_unobserved_ones() {
        // Eighty-three cells, one of them saturated, into twenty bands.
        let mut column = vec![0.0f32; 83];
        column[41] = 1.0;
        let folded = fold(&column, 20);
        assert_eq!(folded.len(), 20);
        assert_eq!(
            folded.iter().filter(|d| **d == Some(1.0)).count(),
            1,
            "one saturated megahertz is still saturated"
        );

        // A band of cells nobody looked at stays unlooked-at; one with a single
        // measured cell reports that cell.
        let mut column = vec![-1.0f32; 83];
        let folded = fold(&column, 20);
        assert!(folded.iter().all(|d| d.is_none()), "{folded:?}");
        column[40] = 0.25;
        let folded = fold(&column, 20);
        assert_eq!(folded.iter().filter(|d| d.is_some()).count(), 1);
        assert!(folded.contains(&Some(0.25)));

        // Degenerate shapes do not panic and do not invent rows.
        assert!(fold(&[], 20).iter().all(|d| d.is_none()));
        assert_eq!(fold(&column, 0).len(), 0);
        // More rows than cells: a cell is *repeated* across the rows it spans,
        // because a megahertz really does occupy that much of the picture.
        // Interpolating would invent readings that were never measured, and
        // leaving the extra rows dark would claim nobody looked at a band that
        // was measured.
        assert_eq!(
            fold(&[0.5, 0.25], 6),
            vec![
                Some(0.5),
                Some(0.5),
                Some(0.5),
                Some(0.25),
                Some(0.25),
                Some(0.25)
            ]
        );
    }
}
