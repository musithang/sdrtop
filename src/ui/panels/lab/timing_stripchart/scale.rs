//! The chart's coordinate system: what the vertical axis spans, which rows carry
//! a label, and where the deadline band falls.
//!
//! The axis is **anchored to the budget**, not auto-scaled to the peak. An
//! auto-scaled axis would move under the reader every frame, so a steady stream
//! and a drifting one would look alike; anchored, the band sits at a stable
//! two-thirds out and the labels stay put across sample rates.

/// Left axis gutter width (the `+0.45 ` labels).
pub(super) const GUTTER_W: usize = 7;
/// Blank braille cell — an "empty" column with no plotted dots.
pub(super) const BLANK: char = '\u{2800}';

/// How far past the budget the axis reaches. At 1.5 the band edges sit two
/// thirds of the way out, which leaves room for a callback to visibly go over
/// without immediately pinning to the top.
const FULL_SCALE_FACTOR: f64 = 1.5;

/// Full-scale deviation of the vertical axis, in µs. Floored at 1 so a zero
/// budget still gives the strip a scale to divide by.
pub(super) fn full_scale_us(budget_us: u64) -> i32 {
    ((budget_us as f64 * FULL_SCALE_FACTOR).round() as i32).max(1)
}

/// Signed deviation (µs) at the top edge of text row `r` of an `rows`-tall chart,
/// where row 0 is `+full_scale` and the last row is `−full_scale`.
pub(super) fn axis_value_us(r: usize, rows: usize, full_scale: i32) -> i32 {
    if rows <= 1 {
        return 0;
    }
    (full_scale as f64 * (1.0 - 2.0 * r as f64 / (rows - 1) as f64)).round() as i32
}

/// Right-aligned gutter label (`+0.45`, `0`, `−0.90`). Plain spaces for an
/// unlabelled row, so every row is the same width and the plot's left edge is
/// straight.
pub(super) fn gutter_label(text: Option<String>) -> String {
    match text {
        Some(s) => format!("{s:>w$} ", w = GUTTER_W - 1),
        None => " ".repeat(GUTTER_W),
    }
}

/// Axis label text: µs under a millisecond, ms above, with an explicit sign so a
/// late and an early deviation cannot be confused at a glance.
pub(super) fn fmt_axis(us: i32) -> String {
    if us == 0 {
        "0".to_string()
    } else if us.unsigned_abs() >= 1000 {
        let sign = if us < 0 { "\u{2212}" } else { "+" };
        format!("{sign}{:.1}", us.unsigned_abs() as f64 / 1000.0)
    } else {
        let sign = if us < 0 { "\u{2212}" } else { "+" };
        format!("{sign}{}", us.unsigned_abs())
    }
}

/// The five rows that carry an axis label: top, quarter, mid, three-quarter,
/// bottom. Enough to read a value off, few enough not to crowd the plot.
pub(super) fn label_rows(chart_h: usize) -> [usize; 5] {
    let last = chart_h.saturating_sub(1);
    [0, last / 4, last / 2, last * 3 / 4, last]
}

/// The two text rows nearest the ±budget band edges, where the faint deadline
/// guide is drawn.
///
/// Braille packs four dot-rows into one text row, so the band is found in dot
/// space and divided down — otherwise the guide would land up to four dot-rows
/// away from the level it claims to mark.
pub(super) fn band_rows(chart_h: usize, budget_us: u64, full_scale: i32) -> (usize, usize) {
    let last = chart_h.saturating_sub(1);
    let span = (chart_h * 4 - 1) as f64 / 2.0;
    let frac = budget_us as f64 / full_scale as f64;
    (
        ((span * (1.0 - frac)).round() as usize / 4).min(last),
        ((span * (1.0 + frac)).round() as usize / 4).min(last),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axis_value_spans_full_scale_top_to_bottom() {
        // Row 0 = +full_scale, last row = −full_scale, middle ≈ 0.
        assert_eq!(axis_value_us(0, 9, 900), 900);
        assert_eq!(axis_value_us(8, 9, 900), -900);
        assert_eq!(axis_value_us(4, 9, 900), 0);
    }

    /// A one-row chart has no top and bottom to span, so it reads zero rather
    /// than dividing by `rows - 1 == 0`.
    #[test]
    fn a_single_row_chart_does_not_divide_by_zero() {
        assert_eq!(axis_value_us(0, 1, 900), 0);
        assert_eq!(axis_value_us(0, 0, 900), 0);
    }

    #[test]
    fn fmt_axis_picks_units_and_sign() {
        assert_eq!(fmt_axis(0), "0");
        assert_eq!(fmt_axis(450), "+450");
        assert_eq!(fmt_axis(-450), "\u{2212}450");
        // Sub-millisecond stays in µs; only ≥ 1000 µs switches to ms.
        assert_eq!(fmt_axis(900), "+900");
        assert_eq!(fmt_axis(1_200), "+1.2");
        assert_eq!(fmt_axis(-1_500), "\u{2212}1.5");
    }

    #[test]
    fn gutter_label_is_fixed_width() {
        assert_eq!(gutter_label(None).chars().count(), GUTTER_W);
        assert_eq!(gutter_label(Some("+0.9".into())).chars().count(), GUTTER_W);
        // Even a label wider than the gutter must not shrink the column, or the
        // plot's left edge would step.
        assert!(gutter_label(Some("\u{2212}12345".into())).chars().count() >= GUTTER_W);
    }

    /// The axis reaches half again past the budget, so a callback that goes over
    /// is visible before it pins to the top of the chart.
    #[test]
    fn the_axis_reaches_past_the_budget() {
        assert_eq!(full_scale_us(600), 900);
        assert!(full_scale_us(600) > 600);
        // A zero budget still leaves something to divide by.
        assert_eq!(full_scale_us(0), 1);
    }

    /// The band is symmetric about the middle and stays inside the chart.
    #[test]
    fn the_deadline_band_brackets_the_centre() {
        for h in [3usize, 5, 9, 14, 30] {
            let (top, bot) = band_rows(h, 600, full_scale_us(600));
            assert!(top <= bot, "h={h}: band inverted ({top}, {bot})");
            assert!(bot < h, "h={h}: band bottom {bot} outside the chart");
            let mid = (h - 1) / 2;
            assert!(top <= mid && bot >= mid, "h={h}: band misses the centre");
        }
    }

    #[test]
    fn the_label_rows_are_inside_the_chart_and_ordered() {
        for h in [1usize, 3, 8, 21] {
            let rows = label_rows(h);
            assert!(rows.iter().all(|&r| r < h.max(1)), "h={h}: {rows:?}");
            assert!(rows.windows(2).all(|w| w[0] <= w[1]), "h={h}: {rows:?}");
            assert_eq!(rows[0], 0);
            assert_eq!(rows[4], h.saturating_sub(1));
        }
    }
}
