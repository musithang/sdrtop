// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The room's clocks: every measured crystal offset in the census on one ppm
//! axis, as a histogram and as each device's own error bar.
//!
//! Bluetooth design measurement 18: "every Bluetooth device in range, sorted by
//! how bad its clock is", plotted as a distribution. B10 met it as a sorted
//! column; this is the picture (net-ux-polish-plan 4.3). Rendering only: the
//! offsets are the census's, corrected exactly as the CFO column corrects them
//! (`RadioState::corrected_ppm`), so the picture and the table cannot disagree
//! about a clock.
//!
//! **Two drawings on one axis, because each hides what the other shows.** The
//! histogram says where the room's clocks bunch, and cannot say how well any
//! one of them is known; an error bar says how well one clock is known, and a
//! row of fifty of them says nothing about where they bunch. So the bars sit
//! under the histogram on the same columns.
//!
//! **Every bar is drawn or counted, never quietly dropped.** Bars that overlap
//! are stacked into rows; when the panel has fewer rows than the room needs,
//! the last row says how many were not drawn. The selected device is placed
//! first, so the one the reader asked about is never among them.
//!
//! **The bars are one standard uncertainty either side**, the same `±` every
//! reading in the app prints, so a bar here and the figure in the CFO column
//! are one number drawn two ways (rule 5). A bar wider than the axis runs off
//! it and says so at the edge (`◂`, `▸`) rather than stretching the axis until
//! every other clock is one column wide.

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::signal::net::census::Device;
use crate::state::SdrMetrics;

/// Eighth-block heights for the histogram, as the occupancy profile draws its
/// bars.
const BARS: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// Rows the histogram's bars stand in.
const HISTOGRAM_ROWS: usize = 2;

/// The most rows of stacked error bars the block asks for. More than this and
/// the picture is taller than the table it illustrates; a real room bunches
/// within a few ppm of its median, and six rows is what twenty clocks in one
/// bunch needed on a 70-column panel.
const MAX_BAR_ROWS: usize = 6;

/// The narrowest span the axis shows, in ppm. One device, or a room of clocks
/// that agree to a part per million, would otherwise be drawn across the
/// whole width at a zoom that makes a 0.3 ppm uncertainty look like a
/// disagreement.
const MIN_SPAN_PPM: f64 = 10.0;

/// Columns between two ruler labels, at the least: the width of `+100` and a
/// space either side.
const LABEL_SPACING: usize = 7;

/// One measured clock, as the picture needs it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Clock {
    ppm: f64,
    sigma: f64,
    selected: bool,
}

/// The measured clocks in `devices`, corrected as the table corrects them.
fn clocks(devices: &[Device], state: &SdrMetrics, now: std::time::Instant) -> Vec<Clock> {
    let selected = state.net.census.selection.selected;
    devices
        .iter()
        .filter_map(|d| {
            let (u, _) = state.radio.corrected_ppm(d.crystal_offset_ppm?, now);
            u.value().is_finite().then_some(Clock {
                ppm: u.value(),
                sigma: u.sigma(),
                selected: Some(d.address) == selected,
            })
        })
        .collect()
}

/// The ppm axis: its ends, the step its labels are laid on, and how its bins
/// sit on columns.
///
/// **Bins are whole columns.** Each histogram bin is `per_bin` columns wide
/// and the axis is exactly `bins * per_bin` columns, so a bin edge is a
/// column edge and one `floor` maps a ppm to its column, its bin and its
/// ruler position alike. The first version rounded to columns and floored to
/// bins, and a clock sitting a hair inside a bin had its dot one column
/// outside that bin's bar.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Axis {
    lo: f64,
    hi: f64,
    /// Between two labels, in ppm. A bin is half of it.
    step: f64,
    bins: usize,
    per_bin: usize,
    /// `bins * per_bin`: the columns actually drawn, never more than asked.
    width: usize,
}

impl Axis {
    /// The axis that holds every clock and zero, widened to at least
    /// [`MIN_SPAN_PPM`] and rounded out to whole steps, in at most `room`
    /// columns.
    ///
    /// **Zero is always on it.** Zero is where a clock agrees with ours, or,
    /// with a reference, where it is right; an axis that left it off would
    /// make the reader work out which side of it the room is on.
    ///
    /// **Every value, and every bar that is not absurd.** A bar is held whole
    /// when its sigma is no wider than the room's values already spread (or
    /// [`MIN_SPAN_PPM`]); one wider than that would squash every other clock
    /// to hold it, so it runs off the edge instead and is marked there.
    fn new(clocks: &[Clock], room: usize) -> Self {
        let room = room.max(1);
        let (vlo, vhi) = clocks
            .iter()
            .fold((0.0f64, 0.0f64), |(l, h), c| (l.min(c.ppm), h.max(c.ppm)));
        let tolerated = (vhi - vlo).max(MIN_SPAN_PPM);
        let (mut lo, mut hi) = clocks
            .iter()
            .filter(|c| c.sigma.is_finite() && c.sigma <= tolerated)
            .fold((vlo, vhi), |(l, h), c| {
                (l.min(c.ppm - c.sigma), h.max(c.ppm + c.sigma))
            });
        let short = MIN_SPAN_PPM - (hi - lo);
        if short > 0.0 {
            lo -= short / 2.0;
            hi += short / 2.0;
        }
        let labels = (room / LABEL_SPACING).max(2) as f64;
        let mut step = nice_step((hi - lo) / labels);
        // A panel narrower than two columns a bin gets a coarser step rather
        // than bins thinner than a column.
        loop {
            let (a, b) = ((lo / step).floor() * step, (hi / step).ceil() * step);
            let bins = ((b - a) / (step / 2.0)).round() as usize;
            if bins <= room || step > 1e6 {
                let bins = bins.clamp(1, room);
                let per_bin = (room / bins).max(1);
                return Self {
                    lo: a,
                    hi: b,
                    step,
                    bins,
                    per_bin,
                    width: bins * per_bin,
                };
            }
            step = nice_step(step * 1.01);
        }
    }

    /// The column `ppm` falls in, clamped to the axis, and whether it had to be.
    fn column(&self, ppm: f64) -> (usize, bool) {
        let x = (ppm - self.lo) / (self.hi - self.lo) * self.width as f64;
        if x.is_nan() || x < 0.0 {
            // Minus infinity from an infinite sigma, or anything left of the
            // axis; NaN never arises, and would be off it too.
            (0, true)
        } else if x > self.width as f64 {
            (self.width - 1, true)
        } else {
            ((x.floor() as usize).min(self.width - 1), false)
        }
    }

    /// The first column of the bin that starts at `ppm`, for a ruler label.
    fn edge(&self, ppm: f64) -> usize {
        let bin = ((ppm - self.lo) / (self.step / 2.0)).round().max(0.0) as usize;
        (bin * self.per_bin).min(self.width - 1)
    }
}

/// The smallest of 1, 2 and 5 times a power of ten that is at least `raw`.
fn nice_step(raw: f64) -> f64 {
    let raw = raw.max(1e-3);
    let decade = 10f64.powf(raw.log10().floor());
    [1.0, 2.0, 5.0, 10.0]
        .into_iter()
        .map(|m| m * decade)
        .find(|s| *s >= raw)
        .unwrap_or(10.0 * decade)
}

/// Devices per bin. A bin is half a label step, so two sit between
/// neighbouring labels and every bin edge lands on a whole ppm figure; a
/// clock's bin is its column's (`Axis::column`), so the dot and the bar it
/// adds to are always in one place.
fn histogram(clocks: &[Clock], axis: &Axis) -> Vec<usize> {
    let mut counts = vec![0; axis.bins];
    for c in clocks {
        counts[axis.column(c.ppm).0 / axis.per_bin] += 1;
    }
    counts
}

/// One error bar laid on the axis: the columns it covers, where its centre
/// is, and whether either end ran off the axis.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Bar {
    from: usize,
    to: usize,
    centre: usize,
    clipped_left: bool,
    clipped_right: bool,
    selected: bool,
}

impl Bar {
    /// Narrower than three columns and on the axis: drawn as its dot alone
    /// ([`bar_row`]).
    fn is_dot(&self) -> bool {
        self.to - self.from < 2 && !self.clipped_left && !self.clipped_right
    }

    /// Whether `self` and `other` can share a row. Two whiskers need a column
    /// between them or `┤├` reads as one bar; a lone dot needs only its own
    /// column.
    fn clears(&self, other: &Bar) -> bool {
        let (a, b) = if self.is_dot() || other.is_dot() {
            (
                if self.is_dot() {
                    self.centre..=self.centre
                } else {
                    self.from..=self.to
                },
                if other.is_dot() {
                    other.centre..=other.centre
                } else {
                    other.from..=other.to
                },
            )
        } else {
            (self.from..=self.to + 1, other.from..=other.to + 1)
        };
        a.end() < b.start() || b.end() < a.start()
    }
}

fn bar(c: &Clock, axis: &Axis) -> Bar {
    let (from, clipped_left) = axis.column(c.ppm - c.sigma);
    let (to, clipped_right) = axis.column(c.ppm + c.sigma);
    Bar {
        from,
        to: to.max(from),
        centre: axis.column(c.ppm).0,
        clipped_left,
        clipped_right,
        selected: c.selected,
    }
}

/// Stack `bars` into at most `rows` rows, two sharing a row only where they
/// clear each other ([`Bar::clears`]): the selected one first, then left to
/// right, each into the first row it fits. Returns the rows and how many bars did not fit in any.
fn pack(mut bars: Vec<Bar>, rows: usize) -> (Vec<Vec<Bar>>, usize) {
    bars.sort_by_key(|b| (!b.selected, b.from, b.to));
    let mut placed: Vec<Vec<Bar>> = vec![Vec::new(); rows];
    let mut left_over = 0;
    for b in bars {
        let row = placed.iter_mut().find(|r| r.iter().all(|o| b.clears(o)));
        match row {
            Some(r) => r.push(b),
            None => left_over += 1,
        }
    }
    (placed, left_over)
}

/// The glyphs of one bar row, `width` wide, with which columns are the
/// selected bar's.
fn bar_row(row: &[Bar], width: usize) -> Vec<(char, bool)> {
    let mut out = vec![(' ', false); width];
    for b in row {
        let heavy = b.selected;
        // A bar narrower than three columns has no room for both whiskers
        // and a dot: it is a clock known better than one column of this axis,
        // and the dot alone says so.
        if b.is_dot() {
            out[b.centre.min(width - 1)] = ('●', heavy);
            continue;
        }
        for (x, slot) in out.iter_mut().enumerate().take(b.to + 1).skip(b.from) {
            *slot = (if heavy { '━' } else { '─' }, heavy);
            if x == b.from {
                slot.0 = match (b.clipped_left, heavy) {
                    (true, _) => '◂',
                    (false, true) => '┣',
                    (false, false) => '├',
                };
            }
            if x == b.to {
                slot.0 = match (b.clipped_right, heavy) {
                    (true, _) => '▸',
                    (false, true) => '┫',
                    (false, false) => '┤',
                };
            }
        }
        out[b.centre.min(width - 1)] = ('●', heavy);
    }
    out
}

/// `-20   -10    0   +10   +20`: a label on every step, where it fits whole
/// without touching its neighbour.
fn ruler(axis: &Axis) -> String {
    let width = axis.width;
    let mut row = vec![' '; width];
    let steps = ((axis.hi - axis.lo) / axis.step).round() as i64;
    for k in 0..=steps {
        let ppm = axis.lo + k as f64 * axis.step;
        let label = if ppm.abs() < axis.step / 2.0 {
            "0".to_string()
        } else {
            format!("{ppm:+.0}")
        };
        let at = axis.edge(ppm);
        let n = label.chars().count();
        let start = at.saturating_sub(n / 2).min(width.saturating_sub(n));
        let free = row[start..start + n].iter().all(|c| *c == ' ')
            && (start == 0 || row[start - 1] == ' ')
            && (start + n >= width || row[start + n] == ' ');
        if free {
            for (i, ch) in label.chars().enumerate() {
                row[start + i] = ch;
            }
        }
    }
    row.into_iter().collect()
}

/// Written after the ruler, past the axis's last column, so the label on the
/// axis's right end and the unit never compete for one place.
const UNIT: &str = " ppm";

/// The block, at most `max_lines` tall and `iw` wide, or nothing when that is
/// too little to draw it honestly: a section rule, the histogram, at least one
/// row of bars and the ruler.
///
/// Returns as few lines as the room needs: a census whose bars all fit on two
/// rows does not get four.
pub(super) fn lines(
    devices: &[Device],
    state: &SdrMetrics,
    now: std::time::Instant,
    iw: usize,
    max_lines: usize,
    theme: &crate::Theme,
) -> Vec<Line<'static>> {
    use crate::ui::chrome::section;
    let clocks = clocks(devices, state, now);
    if clocks.is_empty() {
        if max_lines < 2 {
            return Vec::new();
        }
        // Said, not left blank: a picture with nothing on it would read as a
        // room of perfect clocks.
        return vec![
            section("clocks", "", iw, theme),
            Line::from(Span::styled(
                " no packet has reported an offset yet".to_string(),
                Style::default().fg(theme.label),
            )),
        ];
    }

    // Everything but the bar rows: the rule, the histogram and the ruler.
    let fixed = 1 + HISTOGRAM_ROWS + 1;
    if max_lines < fixed + 1 {
        return Vec::new();
    }
    // A column of margin on the left, so an edge marker is not lost against
    // the frame, and the unit's columns on the right.
    let room_x = iw.saturating_sub(1 + UNIT.len());
    if room_x < 2 {
        return Vec::new();
    }
    let axis = Axis::new(&clocks, room_x);
    let width = axis.width;
    let bars: Vec<Bar> = clocks.iter().map(|c| bar(c, &axis)).collect();

    // As many rows as the room needs, up to what the panel can give. When that
    // is not enough, the last row is the count of what was left off instead.
    let room = (max_lines - fixed).min(MAX_BAR_ROWS);
    let needed = pack(bars.clone(), bars.len())
        .0
        .iter()
        .filter(|r| !r.is_empty())
        .count();
    // With a single row there is no second one to say it on, so the count
    // goes on the section rule instead.
    let (rows, left_over) = if needed <= room {
        pack(bars, needed)
    } else {
        pack(bars, (room - 1).max(1))
    };
    let note_in_hint = left_over > 0 && room < 2;

    let hint = format!(
        "{} of {} measured, {} ppm bins",
        clocks.len(),
        devices.len(),
        fmt_ppm(axis.step / 2.0)
    );
    let short = format!("{} of {} measured", clocks.len(), devices.len());
    let dropped = format!("+{left_over} not drawn");
    let candidates = if note_in_hint {
        vec![dropped]
    } else {
        vec![hint, short]
    };
    let hint = candidates
        .into_iter()
        .find(|h| h.chars().count() + 12 <= iw)
        .unwrap_or_default();
    let mut out = vec![section("clocks", &hint, iw, theme)];

    // The histogram, its selected bin in the highlight ink.
    let counts = histogram(&clocks, &axis);
    let tallest = counts.iter().copied().max().unwrap_or(1).max(1);
    let selected_bin = clocks
        .iter()
        .find(|c| c.selected)
        .map(|c| axis.column(c.ppm).0 / axis.per_bin);
    for r in 0..HISTOGRAM_ROWS {
        let from_bottom = HISTOGRAM_ROWS - 1 - r;
        let mut spans = vec![Span::raw(" ")];
        for bin in (0..axis.width).map(|x| x / axis.per_bin) {
            let filled = counts[bin] as f64 / tallest as f64 * (HISTOGRAM_ROWS * 8) as f64;
            let here = (filled - (from_bottom * 8) as f64).clamp(0.0, 8.0);
            // A bin with anyone in it shows at least a sliver: one device is
            // not nothing, however tall the tallest bin.
            let eighths = if counts[bin] > 0 && from_bottom == 0 {
                (here.round() as usize).max(1)
            } else {
                here.round() as usize
            };
            // In the label ink: the histogram is the setting, the bars below
            // are the testimony, and the louder of the two should be the one
            // with the uncertainty on it.
            let ink = if Some(bin) == selected_bin {
                theme.value_hi
            } else {
                theme.label
            };
            spans.push(Span::styled(
                BARS[eighths].to_string(),
                Style::default().fg(ink),
            ));
        }
        out.push(Line::from(spans));
    }

    // The bars.
    for row in &rows {
        out.push(glyph_line(&bar_row(row, width), theme));
    }
    if left_over > 0 && !note_in_hint {
        out.push(Line::from(Span::styled(
            format!(" +{left_over} not drawn: the panel is too short to stack them all"),
            Style::default().fg(theme.label),
        )));
    }

    out.push(Line::from(vec![
        Span::raw(" "),
        Span::styled(ruler(&axis) + UNIT, Style::default().fg(theme.label)),
    ]));
    out
}

/// `5` or `0.5`: a bin width without a trailing `.0`.
fn fmt_ppm(ppm: f64) -> String {
    if ppm.fract() == 0.0 {
        format!("{ppm:.0}")
    } else {
        format!("{ppm}")
    }
}

/// A bar row as spans: the selected bar bold in the highlight ink, every
/// other bar's whiskers in the label ink with its centre in the value ink.
fn glyph_line(glyphs: &[(char, bool)], theme: &crate::Theme) -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];
    for &(ch, selected) in glyphs {
        let style = if selected {
            Style::default()
                .fg(theme.value_hi)
                .add_modifier(Modifier::BOLD)
        } else if ch == '●' {
            Style::default().fg(theme.value)
        } else {
            Style::default().fg(theme.label)
        };
        spans.push(Span::styled(ch.to_string(), style));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An axis of `per_bin` columns to a bin, from `lo` to `hi`.
    fn axis(lo: f64, hi: f64, step: f64, per_bin: usize) -> Axis {
        let bins = ((hi - lo) / (step / 2.0)).round() as usize;
        Axis {
            lo,
            hi,
            step,
            bins,
            per_bin,
            width: bins * per_bin,
        }
    }

    fn clock(ppm: f64, sigma: f64) -> Clock {
        Clock {
            ppm,
            sigma,
            selected: false,
        }
    }

    /// Zero is on every axis, the values are inside it, its ends are whole
    /// steps, and one lonely clock does not fill the width at a silly zoom.
    #[test]
    fn the_axis_holds_the_room_and_zero() {
        let a = Axis::new(&[clock(12.0, 0.5), clock(35.4, 0.5), clock(18.0, 0.5)], 60);
        assert!(a.lo <= 0.0 && a.hi >= 35.9, "{a:?}");
        assert_eq!(a.lo / a.step, (a.lo / a.step).round());
        assert_eq!(a.hi / a.step, (a.hi / a.step).round());

        let lonely = Axis::new(&[clock(0.2, 0.1)], 60);
        assert!(lonely.hi - lonely.lo >= MIN_SPAN_PPM, "{lonely:?}");

        let negative = Axis::new(&[clock(-40.0, 1.0), clock(-12.0, 1.0)], 60);
        assert!(negative.lo <= -41.0 && negative.hi >= 0.0, "{negative:?}");
    }

    /// A bar at the axis's own edge is held whole, marker and all; one whose
    /// sigma dwarfs the room runs off instead of squashing everyone else.
    #[test]
    fn a_reasonable_bar_is_held_and_an_absurd_one_is_not() {
        let held = Axis::new(&[clock(-5.0, 0.5), clock(35.4, 0.5)], 60);
        assert!(held.lo <= -5.5, "{held:?}");

        let absurd = Axis::new(&[clock(-5.0, 400.0), clock(35.4, 0.5)], 60);
        assert!(absurd.lo > -100.0, "{absurd:?}");
    }

    #[test]
    fn a_step_is_one_two_or_five_of_a_decade() {
        assert_eq!(nice_step(3.2), 5.0);
        assert_eq!(nice_step(5.0), 5.0);
        assert_eq!(nice_step(0.14), 0.2);
        assert_eq!(nice_step(71.0), 100.0);
    }

    /// **The bar is one standard uncertainty either side**, the same `±` the
    /// CFO column prints: on an axis of one column per ppm, a clock at +10
    /// ±3 covers +7 to +13.
    #[test]
    fn a_bar_spans_one_sigma_either_side() {
        // Eight bins of five columns: one column a ppm.
        let axis = axis(0.0, 40.0, 10.0, 5);
        let b = bar(&clock(10.0, 3.0), &axis);
        assert_eq!((b.from, b.centre, b.to), (7, 10, 13));
        assert!(!b.clipped_left && !b.clipped_right);
    }

    /// A bar wider than the axis runs off it and says so, rather than the
    /// axis stretching to hold it.
    #[test]
    fn a_bar_off_the_axis_is_marked_at_the_edge() {
        // Eight bins of five columns: one column a ppm.
        let axis = axis(0.0, 40.0, 10.0, 5);
        let b = bar(&clock(20.0, 30.0), &axis);
        assert!(b.clipped_left && b.clipped_right, "{b:?}");
        let row = bar_row(&[b], 40);
        assert_eq!(row[0].0, '◂');
        assert_eq!(row[39].0, '▸');
        assert_eq!(row[20].0, '●');

        let infinite = bar(&clock(20.0, f64::INFINITY), &axis);
        assert!(
            infinite.clipped_left && infinite.clipped_right,
            "{infinite:?}"
        );
    }

    /// **Overlapping bars stack; what does not fit is counted, and the
    /// selected bar always fits.** Three clocks on one spot need three rows.
    #[test]
    fn bars_that_overlap_stack_and_the_selected_is_never_left_off() {
        // Eight bins of five columns: one column a ppm.
        let axis = axis(0.0, 40.0, 10.0, 5);
        let mut bars: Vec<Bar> = [10.0, 10.5, 11.0, 30.0]
            .iter()
            .map(|&p| bar(&clock(p, 2.0), &axis))
            .collect();
        bars[2].selected = true;

        let (rows, left) = pack(bars.clone(), 3);
        assert_eq!(left, 0);
        assert_eq!(rows.iter().map(Vec::len).sum::<usize>(), 4);
        // The far one shares the first row with whatever is there.
        assert_eq!(rows[0].len(), 2);

        let (rows, left) = pack(bars, 1);
        assert_eq!(left, 2, "two of the three stacked clocks do not fit");
        assert!(rows[0].iter().any(|b| b.selected), "{rows:?}");
    }

    /// The histogram counts every measured clock once, in the bin its value
    /// falls in.
    #[test]
    fn the_histogram_counts_every_clock_once() {
        let axis = axis(-20.0, 40.0, 10.0, 3);
        let cs = [
            clock(-12.0, 1.0),
            clock(1.0, 1.0),
            clock(3.0, 1.0),
            clock(39.0, 1.0),
        ];
        let counts = histogram(&cs, &axis);
        assert_eq!(counts.iter().sum::<usize>(), 4);
        // Bins of 5 ppm: 0 to 5 holds two.
        assert_eq!(counts[4], 2, "{counts:?}");
    }

    /// **A clock's dot sits in its own bin's bar**, at every width and for
    /// every value: the defect this axis was rebuilt to remove.
    #[test]
    fn a_dot_is_always_inside_the_bar_of_its_own_bin() {
        for room in 10..160 {
            for tenth in -400..400 {
                let c = clock(tenth as f64 / 10.0, 0.5);
                let a = Axis::new(&[c, clock(35.4, 0.5)], room);
                assert!(a.width <= room, "{room}: {a:?}");
                let x = a.column(c.ppm).0;
                let bin = x / a.per_bin;
                let (lo, hi) = (
                    a.lo + bin as f64 * a.step / 2.0,
                    a.lo + (bin + 1) as f64 * a.step / 2.0,
                );
                assert!(
                    c.ppm >= lo - 1e-9 && c.ppm <= hi + 1e-9,
                    "{room}: {} in [{lo}, {hi}) {a:?}",
                    c.ppm
                );
            }
        }
    }

    /// Labels on the steps, zero written as `0`, signs on the rest, the
    /// axis's own ends labelled.
    #[test]
    fn the_ruler_labels_the_steps() {
        let axis = Axis::new(&[clock(-18.0, 0.5), clock(35.0, 0.5)], 60);
        let r = ruler(&axis);
        assert_eq!(r.chars().count(), 60);
        assert!(r.contains(" 0 "), "{r:?}");
        assert!(r.contains("+20"), "{r:?}");
        assert!(r.starts_with("-20"), "{r:?}");
        assert!(r.ends_with("+40"), "{r:?}");
    }

    /// Two dots in neighbouring columns share a row; two whiskers touching
    /// do not, because `┤├` reads as one bar.
    #[test]
    fn dots_may_touch_and_whiskers_may_not() {
        let b = |from, centre, to| Bar {
            from,
            to,
            centre,
            clipped_left: false,
            clipped_right: false,
            selected: false,
        };
        assert!(b(10, 10, 10).clears(&b(11, 11, 11)));
        assert!(!b(10, 10, 10).clears(&b(10, 10, 11)));
        assert!(b(10, 10, 10).clears(&b(11, 13, 15)));
        assert!(!b(5, 7, 9).clears(&b(10, 12, 14)), "touching whiskers");
        assert!(b(5, 7, 9).clears(&b(11, 13, 15)));
    }
}
