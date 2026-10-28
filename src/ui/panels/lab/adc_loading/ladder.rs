// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The bit ladder: the panel's hero visual. One column a 6 dB band below
//! full scale, which is one bit of the converter, the least significant at
//! the left and full scale at the right, each as tall as its share of the
//! samples and labelled with it.
//!
//! It replaced a bell of 32 linear bins across -FS..+FS, which put a signal
//! at -13 dBFS into the middle fifth of the panel and could not show the
//! thing the bench is for: how many of the converter's bits the signal
//! actually uses. On a HackRF tuned to FM at a modest gain the answer is
//! three of eight, and the ladder says so at a glance.
//!
//! Under it, on the same dBFS axis as every other level on the bench: where
//! the staging verdict wants the peak (green), where this radio clips (red,
//! through the ladder where the radio clips before the converter), the peak
//! (`▲`) and the rms (`△`).

use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};

/// 6 dB, one bit.
pub(super) const BAND_DB: f64 = 6.020_6;

/// Vertical ⅛-block ramp, 0 = blank … 8 = full cell.
const EIGHTHS: [char; 9] = [
    ' ', '\u{2581}', '\u{2582}', '\u{2583}', '\u{2584}', '\u{2585}', '\u{2586}', '\u{2587}',
    '\u{2588}',
];

/// Everything the ladder needs besides the counts.
pub(super) struct Levels {
    /// The converter's depth: how many bands there are.
    pub bits: usize,
    pub peak_dbfs: f64,
    /// Per axis, as the samples in the bands are.
    pub rms_dbfs: f64,
    /// Where this radio stops its samples, dBFS.
    pub ceiling_dbfs: f64,
    /// Whether a part clips or holds them there; `None` at the converter.
    pub ceiling_kind: Option<crate::hardware::CeilingKind>,
    /// The staging verdict's colour, for the peak's mark.
    pub sev_col: Color,
}

/// Rows the ladder takes besides its bars: the caption, the row the tallest
/// bar's label sits on, the axis, its numbers, the marks and the key.
pub(super) const CHROME_ROWS: usize = 6;

/// Where the columns go across `w`: the first column, the columns per band
/// and the bar's own width within them.
struct Across {
    x0: usize,
    per_band: usize,
    bar: usize,
    used: usize,
}

impl Across {
    fn new(w: usize, bits: usize) -> Self {
        let avail = w.saturating_sub(2).max(bits);
        let per_band = (avail / bits).max(1);
        let used = per_band * bits;
        Across {
            x0: 1 + avail.saturating_sub(used) / 2,
            per_band,
            // A column's gap to the next, where there is room for one.
            bar: if per_band >= 2 { per_band - 1 } else { 1 },
            used,
        }
    }
    /// The column a level falls on, full scale at the right.
    fn x_of(&self, dbfs: f64, bits: usize) -> usize {
        let span = BAND_DB * bits as f64;
        let t = ((dbfs + span) / span).clamp(0.0, 1.0);
        self.x0 + ((t * self.used as f64).round() as usize).min(self.used - 1)
    }
    /// The first column of band `k` (0 = the top bit, at the right).
    fn band_x(&self, k: usize, bits: usize) -> usize {
        self.x0 + (bits - 1 - k) * self.per_band
    }
}

/// Push the ladder `w` wide with `bars` rows of bars.
pub(super) fn draw(
    out: &mut Vec<Line<'static>>,
    bands: &[u64],
    lv: &Levels,
    w: usize,
    bars: usize,
    theme: &crate::Theme,
) {
    let dim = theme.border_dim;
    let bits = lv.bits.clamp(1, bands.len());
    let n: u64 = bands[..bits].iter().sum();
    out.push(Line::from(vec![
        Span::styled(" where the samples sit", Style::default().fg(theme.label)),
        Span::styled(
            format!(" \u{00b7} % per 6 dB, one bit each \u{00b7} {bits}-bit"),
            Style::default().fg(dim),
        ),
    ]));

    let across = Across::new(w, bits);
    // Never narrower than the ladder: a row past the panel is cut at its
    // edge, an index past the row would not be.
    let w = w.max(across.x0 + across.used + 1);
    let mut grid = vec![vec![(' ', dim); w]; bars + 1];
    if n == 0 {
        let text = " no samples yet";
        for (i, c) in text.chars().enumerate().take(w) {
            grid[bars / 2][i] = (c, theme.label);
        }
    } else {
        let share: Vec<f64> = bands[..bits].iter().map(|&c| c as f64 / n as f64).collect();
        let top = share.iter().copied().fold(0.0, f64::max);
        for (k, &s) in share.iter().enumerate() {
            let bx = across.band_x(k, bits);
            // Tallest band full height; any band with a sample in it shows.
            let mut eighths = (s / top * (bars * 8) as f64).round() as usize;
            if s > 0.0 {
                eighths = eighths.max(1);
            }
            for (r, row) in grid.iter_mut().skip(1).enumerate() {
                let cell = eighths.saturating_sub((bars - 1 - r) * 8).min(8);
                if cell > 0 {
                    row[bx..bx + across.bar].fill((EIGHTHS[cell], theme.value_hi));
                }
            }
            // Its share over it, where it is a whole percent and fits.
            let label = format!("{:.0}", s * 100.0);
            if s >= 0.01 && label.len() <= across.bar {
                let r = bars - eighths.div_ceil(8);
                let lx = bx + (across.bar - label.len()) / 2;
                for (i, c) in label.chars().enumerate() {
                    grid[r][lx + i] = (c, theme.label);
                }
            }
        }
    }
    // A radio that stops its samples before its converter: the line runs
    // through the ladder, so a band past it is seen to be past it.
    if lv.ceiling_dbfs < 0.0 {
        let cx = across.x_of(lv.ceiling_dbfs, bits);
        for row in grid.iter_mut() {
            row[cx] = ('\u{250a}', theme.status_crit);
        }
    }
    out.extend(grid.iter().map(|row| cells_line(row)));

    // The axis: a tick between bands, and the staging verdict's landing zone.
    let mut axis = vec![(' ', dim); w];
    for cell in axis.iter_mut().skip(across.x0).take(across.used) {
        *cell = ('\u{2500}', dim);
    }
    for d in 0..=bits {
        let x = (across.x0 + d * across.per_band).min(across.x0 + across.used - 1);
        axis[x] = ('\u{2534}', dim);
    }
    let (zlo, zhi) = (
        across.x_of(lv.ceiling_dbfs - 14.0, bits),
        across.x_of(lv.ceiling_dbfs - 4.0, bits),
    );
    for cell in axis.iter_mut().take(zhi + 1).skip(zlo) {
        *cell = ('\u{2501}', theme.status_ok);
    }
    out.push(cells_line(&axis));

    // dBFS every two bands, a number left out rather than run into another.
    let mut ticks = vec![(' ', dim); w];
    let mut free = 0;
    for k in (0..=bits).rev().filter(|k| k % 2 == 0) {
        let db = -BAND_DB * k as f64;
        let t = format!("{:.0}", db + 0.0);
        let x = across.x_of(db, bits);
        let start = x.saturating_sub(t.len() / 2).min(w.saturating_sub(t.len()));
        if start >= free {
            put(&mut ticks, start, &t, theme.label);
            free = start + t.len() + 1;
        }
    }
    out.push(cells_line(&ticks));

    // The peak and the rms, marked under the axis.
    let mut marks = vec![(' ', dim); w];
    let px = across.x_of(lv.peak_dbfs, bits);
    let rx = across.x_of(lv.rms_dbfs, bits);
    if rx >= 4 && rx + 1 < px {
        put(&mut marks, rx - 4, "rms", theme.label);
    }
    put(&mut marks, rx, "\u{25b3}", theme.label);
    put(&mut marks, px, "\u{25b2}", lv.sev_col);
    if px + 6 <= w {
        put(&mut marks, px + 2, "peak", lv.sev_col);
    } else if px >= 5 && px.saturating_sub(5) > rx {
        put(&mut marks, px - 5, "peak", lv.sev_col);
    }
    out.push(cells_line(&marks));

    let mut key = vec![
        Span::raw(" "),
        Span::styled(
            "\u{2501} where the peak belongs",
            Style::default().fg(theme.status_ok),
        ),
    ];
    if lv.ceiling_dbfs < 0.0 {
        let verb = lv.ceiling_kind.map_or("clips", |k| k.verb());
        key.push(Span::styled(
            format!("   \u{250a} where it {verb}"),
            Style::default().fg(theme.status_crit),
        ));
    }
    out.push(Line::from(key));
}

fn put(cells: &mut [(char, Color)], x: usize, s: &str, col: Color) {
    for (i, c) in s.chars().enumerate() {
        if let Some(cell) = cells.get_mut(x + i) {
            *cell = (c, col);
        }
    }
}

/// A row of cells as styled runs.
fn cells_line(cells: &[(char, Color)]) -> Line<'static> {
    let mut spans = Vec::new();
    let mut run = String::new();
    let mut cur: Option<Color> = None;
    for &(c, col) in cells {
        if cur != Some(col) {
            if let Some(cc) = cur {
                spans.push(Span::styled(
                    std::mem::take(&mut run),
                    Style::default().fg(cc),
                ));
            }
            cur = Some(col);
        }
        run.push(c);
    }
    if let Some(cc) = cur {
        spans.push(Span::styled(run, Style::default().fg(cc)));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_scale_is_at_the_right_and_the_lsb_at_the_left() {
        let a = Across::new(50, 12);
        assert_eq!(a.per_band, 4);
        assert_eq!(a.band_x(0, 12), a.x0 + 44, "the top bit, rightmost");
        assert_eq!(a.band_x(11, 12), a.x0, "the last band, leftmost");
        assert_eq!(a.x_of(0.0, 12), a.x0 + a.used - 1);
        assert_eq!(a.x_of(-200.0, 12), a.x0);
        // -12 dBFS is the boundary between the second and third bands.
        assert_eq!(a.x_of(-2.0 * BAND_DB, 12), a.band_x(1, 12));
    }

    /// A converter deeper than the panel is wide still fits, one column a
    /// band with no gap, rather than running off the edge.
    #[test]
    fn sixteen_bands_fit_a_narrow_panel() {
        let a = Across::new(20, 16);
        assert_eq!((a.per_band, a.bar), (1, 1));
        assert!(a.x0 + a.used <= 20);
    }
}
