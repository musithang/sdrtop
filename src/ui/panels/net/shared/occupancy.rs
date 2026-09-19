// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `NetOccupancyPanel` - who is spending the airtime, one megahertz at a time.
//!
//! The band across the width of the panel, with the duty cycle of each cell as
//! the height of its bar and the Wi-Fi channel numbering underneath it. Design
//! section 11 calls it `net_occupancy` and the question it answers is the one
//! `net_survey` is built around: what is in this band?
//!
//! **Three states, drawn three ways, and the distinction is the panel.**
//!
//! - *Busy*: a bar, its height the duty cycle.
//! - *Observed and empty*: the baseline, drawn. Nothing was transmitting there
//!   and that is a measurement.
//! - *Not observed*: a dim dash. Nobody looked. Rule 2, and the reason the panel
//!   cannot simply draw zero for both of the last two: a receiver seeing
//!   eighteen megahertz of an eighty-three megahertz band that reported the
//!   other sixty-five as empty would be lying about most of the screen.
//!
//! **And a fourth state above all of them**: when the noise floor's
//! preconditions failed, the bars are not drawn at all. Everything here is
//! measured against that floor, so if it is not a floor then none of this is a
//! measurement. See `signal::net::occupancy`.

use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use super::band_axis;
use super::band_axis::bonded_frame;
use crate::signal::net::occupancy;
use crate::state::{CellReading, SdrMetrics};
use crate::ui::panel::{Bond, Bonding, FeedSpan, Panel, PanelChrome, Staleness};
use crate::ui::widgets::reading::Reading;
use ratatui::widgets::Borders;

pub struct NetOccupancyPanel;

/// Eighth-block heights, so one row of bars carries eight levels of duty cycle.
const BARS: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// What an unobserved cell is drawn as: nobody looked here.
const UNSEEN: char = '·';

/// The baseline an observed, empty cell rests on. Distinct from [`UNSEEN`] on
/// purpose, and the whole reason this panel has two glyphs for "no bar".
const FLOOR: char = '▁';

/// Bar rows, so the duty cycle has more than eight levels to sit on.
const ROWS: usize = 3;

/// One column of the profile: the glyph for each of `rows` rows, top first.
///
/// The duty cycle is spread over the rows from the bottom up, so a cell busy a
/// third of the time fills the bottom row and no more. Eight levels a row and
/// three rows is twenty-four, which is finer than a terminal column deserves and
/// is what stops a band of quiet channels reading as a flat run of identical
/// stubs.
fn column(cell: &CellReading, rows: usize) -> Vec<char> {
    if !cell.observed() {
        return vec![UNSEEN; rows];
    }
    let filled = cell.duty.clamp(0.0, 1.0) * (rows * 8) as f64;
    let mut out = vec![' '; rows];
    for (i, slot) in out.iter_mut().enumerate() {
        // Row 0 is the top, so the bottom row is the last one.
        let from_bottom = rows - 1 - i;
        let here = (filled - (from_bottom * 8) as f64).clamp(0.0, 8.0);
        *slot = BARS[here.round() as usize];
    }
    // An observed cell with nothing in it still shows where the floor is.
    if out[rows - 1] == ' ' {
        out[rows - 1] = FLOOR;
    }
    out
}

/// `head` and then as many of `groups` as fit in `width`, each whole or not at
/// all, three spaces apart: a readout cut mid-figure reads as another figure.
fn fit_groups(
    head: Vec<Span<'static>>,
    groups: Vec<Vec<Span<'static>>>,
    width: usize,
) -> Line<'static> {
    let len = |g: &[Span<'_>]| g.iter().map(|s| s.content.chars().count()).sum::<usize>();
    let mut used = len(&head);
    let mut spans = head;
    for group in groups {
        let need = 3 + len(&group);
        if used + need > width {
            break;
        }
        used += need;
        spans.push(Span::raw("   "));
        spans.extend(group);
    }
    Line::from(spans)
}

/// The profile at a past moment, from the history: each cell's duty as it
/// was, and nothing else, because nothing else is kept. Observed-ness comes
/// from the history's own mark (negative: nobody looked).
fn cells_then(column: &[f32]) -> Vec<CellReading> {
    column
        .iter()
        .map(|&v| CellReading {
            windows: u64::from(v >= 0.0),
            duty: f64::from(v.max(0.0)),
            ..Default::default()
        })
        .collect()
}

/// What the profile says at a past moment, above its bars: when it was, what
/// the history keeps, and the cursor's cell or the busiest one then, duty only
/// and without a spread (the window counts that would give one are not kept,
/// and a spread made up for them would be the invented number rule 2 forbids).
fn moment_lines(
    state: &SdrMetrics,
    cells: &[CellReading],
    back: usize,
    width: usize,
    theme: &crate::Theme,
) -> Vec<Line<'static>> {
    let dim = Style::default().fg(theme.label);
    let hi = Style::default().fg(theme.value_hi);
    let ago = back as f64 * crate::state::COLUMN_INTERVAL.as_secs_f64();
    let mut out = vec![fit_groups(
        vec![
            Span::styled("moment       ", dim),
            Span::styled(format!("{ago:.1} s ago"), hi),
        ],
        vec![vec![Span::styled(
            "duty only, as the history keeps it",
            dim,
        )]],
        width,
    )];
    let pick = match state.net.band_cursor.selected {
        Some(cell) => Some(("cursor       ", cell)),
        None => cells
            .iter()
            .enumerate()
            .filter(|(_, c)| c.observed() && c.duty > 0.0)
            .max_by(|a, b| a.1.duty.total_cmp(&b.1.duty))
            .map(|(cell, _)| ("busiest      ", cell)),
    };
    out.push(match pick {
        None => Line::from(Span::styled(
            "busiest      nothing above the floor then",
            dim,
        )),
        Some((label, cell)) => {
            let c = cells.get(cell).copied().unwrap_or_default();
            let said = if c.observed() {
                vec![Span::styled(
                    format!("{:.0} % busy", c.duty * 100.0),
                    Style::default().fg(theme.value),
                )]
            } else {
                vec![Span::styled(
                    "not observed then",
                    Style::default().fg(theme.stale),
                )]
            };
            fit_groups(
                vec![
                    Span::styled(label, dim),
                    Span::styled(
                        format!("{} MHz", occupancy::cell_centre_hz(cell) / 1_000_000),
                        hi,
                    ),
                ],
                vec![said],
                width,
            )
        }
    });
    out
}

/// The band mapped onto `width` columns, each column the busiest cell it covers.
///
/// **The busiest, not the mean.** Squeezing eighty-three cells into forty
/// columns by averaging turns one saturated megahertz next to a quiet one into
/// two half-busy ones, which is the opposite of what this panel is for. A column
/// says "the worst thing in here", and a column of unobserved cells stays
/// unobserved.
fn columns(cells: &[CellReading], width: usize) -> Vec<CellReading> {
    if width == 0 || cells.is_empty() {
        return Vec::new();
    }
    (0..width)
        .map(|x| {
            let range = band_axis::cells_of(x, width);
            cells[range.start.min(cells.len())..range.end.min(cells.len())]
                .iter()
                .copied()
                .reduce(|a, b| {
                    if b.duty > a.duty || !a.observed() {
                        b
                    } else {
                        a
                    }
                })
                .unwrap_or_default()
        })
        .collect()
}

/// The panel's lines, `width` wide. Bonded over the coexistence heatmap the
/// ruler and the band's edges are not drawn here: the seam below carries the
/// ruler for both halves (`render_bonded`).
fn lines(
    state: &SdrMetrics,
    theme: &crate::Theme,
    width: usize,
    bonded: bool,
    bar_rows: usize,
) -> Vec<Line<'static>> {
    let occ = &state.net.band;
    let mut out = Vec::new();
    let dim = Style::default().fg(theme.label);

    let floor = match occ.noise_dbfs {
        Some(db) => format!("{db:.1} dBFS"),
        None => "—".to_string(),
    };
    let observed = occ.cells.iter().filter(|c| c.observed()).count();
    out.push(Line::from(vec![
        Span::styled("noise floor  ", dim),
        Span::styled(floor, Style::default().fg(theme.value)),
        Span::styled(
            format!("   {observed} of {} MHz in view", occupancy::CELLS),
            dim,
        ),
    ]));

    if occ.cells.is_empty() {
        // **Two different silences.** No samples yet is a wait; a view too
        // narrow to hold a whole megahertz is a refusal that will never end.
        // Saying "waiting for RX" while the feed panel beside this one counts
        // blocks arriving is the version a reader cannot act on.
        match &state.net.survey_refused {
            Some(why) => {
                // Wrapped rather than truncated: the sentence is the whole
                // content here, and half of it cut mid-word is the shape the
                // feed-health notes already refuse.
                for row in crate::ui::chrome::wrap(&format!("no band measurement: {why}"), width, 3)
                {
                    out.push(Line::from(Span::styled(
                        row,
                        Style::default().fg(theme.stale),
                    )));
                }
                for row in crate::ui::chrome::wrap(
                    "widen the sample rate, or press [M] to lock to one channel",
                    width,
                    2,
                ) {
                    out.push(Line::from(Span::styled(row, dim)));
                }
            }
            None => out.push(Line::from(Span::styled("waiting for RX", dim))),
        }
        return out;
    }

    // Everything below is measured against the floor, so a floor that failed its
    // own preconditions takes the whole profile with it rather than colouring it
    // a warning shade and leaving it up.
    if !occ.trusted {
        out.push(Line::from(""));
        out.push(Line::from(Span::styled(
            "no floor: the samples are not noise plus signal".to_string(),
            Style::default().fg(theme.stale),
        )));
        out.push(Line::from(Span::styled(
            format!(
                "tail {:.2} (max {:.2})   spread {:.1} (min {:.1})",
                occ.tail,
                occupancy::TAIL_LIMIT,
                occ.spread,
                occupancy::SPREAD_FLOOR
            ),
            dim,
        )));
        out.push(Line::from(Span::styled(
            "a saturated front end, or a band busy everywhere".to_string(),
            dim,
        )));
        return out;
    }

    // The time cursor on the history below: the profile shows that moment,
    // from what the history kept. A moment that has scrolled out of it is no
    // longer there to show, and the profile goes back to now.
    let past = state
        .net
        .band_scrub
        .and_then(|id| Some((occ.back_of(id)?, cells_then(occ.column(id)?))));
    if let Some((back, cells)) = past {
        // The floor line above is today's; the history kept no floor for the
        // moment shown, so the line says whose it is rather than passing for it.
        if let Some(first) = out.first_mut() {
            if let Some(label) = first.spans.first_mut() {
                *label = Span::styled("floor (now)  ", dim);
            }
        }
        out.extend(moment_lines(state, &cells, back, width, theme));
        bars(&mut out, state, &cells, width, bar_rows, theme, true);
        if !bonded {
            out.push(Line::from(Span::styled(band_axis::ruler(width), dim)));
            out.push(Line::from(Span::styled(band_axis::edges(width), dim)));
        }
        return out;
    }

    // The headline: the one cell a person would want named. Ties go to the
    // lower frequency, which is arbitrary but is at least the same arbitrary
    // choice every frame.
    let busiest = occ
        .cells
        .iter()
        .enumerate()
        .filter(|(_, c)| c.observed() && c.duty > 0.0)
        .max_by(|a, b| a.1.duty.total_cmp(&b.1.duty));
    // With a cursor set, the selected cell takes the headline's place: it is
    // the cell the reader chose, and `B` puts the cursor on the busiest one.
    let selected = state
        .net
        .band_cursor
        .selected
        .and_then(|cell| occ.cells.get(cell).map(|c| (cell, c)));
    if let Some((cell, c)) = selected {
        out.push(cursor_line(cell, c, occ.window_s, width, theme));
    } else {
        out.push(match busiest {
            Some((cell, c)) => {
                let mut spans = vec![
                    Span::styled("busiest      ", dim),
                    Span::styled(
                        format!("{:.0} MHz", occupancy::cell_centre_hz(cell) / 1_000_000),
                        Style::default().fg(theme.value_hi),
                    ),
                    Span::raw("  "),
                ];
                // The duty cycle through idiom A, with the spread its window count
                // supports: a cell watched for a sixth of the time is not a sixth as
                // busy, it is as busy with a wider bar. `resolution` is the tenth of
                // a percent the reading is shown to, which is the difference that
                // matters here by construction.
                spans.extend(
                    Reading::new(
                        occupancy::duty_uncertain(c.duty, c.windows).scale(100.0),
                        "% busy",
                        occupancy::DUTY_RESOLUTION * 100.0,
                    )
                    .spans(theme),
                );
                spans.push(Span::styled(
                    format!("   {:.1} peak, {:.1} mean dBFS", c.peak_dbfs, c.mean_dbfs),
                    dim,
                ));
                Line::from(spans)
            }
            None => Line::from(Span::styled("busiest      nothing above the floor", dim)),
        });
    }

    // How much of the time the band was actually under the receiver, which is
    // what the mode costs and the one number that says it in figures rather than
    // as a word in the chrome.
    let covered: Vec<f64> = occ.cells.iter().filter_map(|c| c.coverage).collect();
    // Over how long: the watch this coverage was accumulated across, which is
    // held in the state and was never shown.
    let over = occ
        .watch_start
        .map(|t| format!(", over {}", seconds(t.elapsed().as_secs_f64())))
        .unwrap_or_default();
    out.push(Line::from(Span::styled(
        match covered.len() {
            0 => "coverage     — · every cell measured once so far".to_string(),
            n => format!(
                "coverage     {:.0} % of the time, on {n} of {} cells{over}",
                covered.iter().sum::<f64>() / n as f64 * 100.0,
                occupancy::CELLS
            ),
        },
        dim,
    )));

    bars(&mut out, state, &occ.cells, width, bar_rows, theme, false);
    if !bonded {
        out.push(Line::from(Span::styled(band_axis::ruler(width), dim)));
        out.push(Line::from(Span::styled(band_axis::edges(width), dim)));
    }
    out
}

/// The bars for `cells`, `rows` tall, and the cursor's mark under them.
///
/// Duty is the height. Live, the power colours it on the waterfall's ramp;
/// at a past moment (`by_duty`) there is no power to colour by, the history
/// keeps none, so the duty colours it on the same ramp the heatmap below uses.
fn bars(
    out: &mut Vec<Line<'static>>,
    state: &SdrMetrics,
    cells: &[CellReading],
    width: usize,
    rows: usize,
    theme: &crate::Theme,
    by_duty: bool,
) {
    let cols = columns(cells, width);
    let glyphs: Vec<Vec<char>> = cols.iter().map(|c| column(c, rows)).collect();
    for row in 0..rows {
        let spans = cols
            .iter()
            .zip(&glyphs)
            .map(|(c, g)| {
                let ch = g[row];
                let colour = if !c.observed() {
                    theme.stale
                } else if ch == ' ' || ch == FLOOR {
                    theme.label
                } else if by_duty {
                    theme.palette_color(c.duty.clamp(0.0, 1.0) as f32)
                } else {
                    theme.palette_color(((c.peak_dbfs + 90.0) / 90.0).clamp(0.0, 1.0) as f32)
                };
                Span::styled(ch.to_string(), Style::default().fg(colour))
            })
            .collect::<Vec<_>>();
        out.push(Line::from(spans));
    }
    // The cursor, under the column its cell is drawn in: on its own row so it
    // never hides a bar or a channel number.
    if let Some(cell) = state.net.band_cursor.selected {
        let x = band_axis::column_of(cell, width);
        out.push(Line::from(vec![
            Span::raw(" ".repeat(x)),
            Span::styled("\u{25b2}", Style::default().fg(theme.value_hi)),
        ]));
    }
}

/// The selected cell, read out: where it is, how busy with the uncertainty its
/// window count supports, its power, how much it was watched and when.
///
/// A cell nobody looked at says so and gives no figures (rule 2); its
/// neighbours' are not borrowed.
fn cursor_line(
    cell: usize,
    c: &CellReading,
    window_s: f64,
    width: usize,
    theme: &crate::Theme,
) -> Line<'static> {
    let dim = Style::default().fg(theme.label);
    let head = vec![
        Span::styled("cursor       ", dim),
        Span::styled(
            format!("{} MHz", occupancy::cell_centre_hz(cell) / 1_000_000),
            Style::default().fg(theme.value_hi),
        ),
    ];
    // Each group is drawn whole or not at all, in this order: a readout cut
    // mid-figure reads as a different figure.
    let groups: Vec<Vec<Span<'static>>> = if !c.observed() {
        vec![vec![Span::styled(
            "not observed: nobody looked here",
            Style::default().fg(theme.stale),
        )]]
    } else {
        let watched = c.windows as f64 * window_s;
        let ago = c
            .measured
            .map(|t| crate::ui::widgets::timing_fmt::ago(t.elapsed()))
            .unwrap_or_else(|| "—".to_string());
        vec![
            Reading::new(
                occupancy::duty_uncertain(c.duty, c.windows).scale(100.0),
                "% busy",
                occupancy::DUTY_RESOLUTION * 100.0,
            )
            .spans(theme),
            vec![Span::styled(
                format!("{:.1} peak, {:.1} mean dBFS", c.peak_dbfs, c.mean_dbfs),
                dim,
            )],
            vec![Span::styled(
                format!("{} windows, {} watched", c.windows, seconds(watched)),
                dim,
            )],
            vec![Span::styled(ago, dim)],
        ]
    };
    fit_groups(head, groups, width)
}

/// A span of time at the resolution it is worth reading at.
fn seconds(s: f64) -> String {
    if s < 1e-3 {
        format!("{:.0} us", s * 1e6)
    } else if s < 1.0 {
        format!("{:.0} ms", s * 1e3)
    } else if s < 120.0 {
        format!("{s:.0} s")
    } else {
        format!("{:.0} min", s / 60.0)
    }
}

/// How long ago the oldest cell on screen was measured. `None` when no cell
/// has been.
fn oldest_reading_age(state: &SdrMetrics) -> Option<std::time::Duration> {
    let oldest = state
        .net
        .band
        .cells
        .iter()
        .filter_map(|c| c.measured)
        .min()?;
    Some(oldest.elapsed())
}

impl Panel for NetOccupancyPanel {
    fn name(&self) -> &'static str {
        "net_occupancy"
    }

    fn min_size(&self) -> (u16, u16) {
        (40, 8)
    }

    /// `j`: every letter of the panel's name is another panel's focus key or a
    /// global one, so the engine draws `[J]`.
    fn focus_key(&self) -> Option<char> {
        Some('j')
    }

    fn focus_bindings(&self) -> &'static [(&'static str, &'static str)] {
        &[
            ("\u{2190}\u{2192}", "move the cursor 1 MHz"),
            ("B", "cursor to the busiest cell"),
        ]
    }

    /// The upper half of the survey instrument: the profile over the
    /// coexistence heatmap, one ruler between them.
    fn bonding(&self) -> Option<Bonding> {
        Some(Bonding {
            role: Bond::Below,
            partner: "net_coexist",
        })
    }

    /// Bonded: the frame without its bottom edge, and the profile pushed down
    /// against the seam so the bars stand on the ruler they are read by.
    fn render_bonded(
        &self,
        f: &mut Frame,
        area: Rect,
        state: &SdrMetrics,
        theme: &crate::Theme,
        focused: bool,
        _bond: Bond,
    ) {
        let Some(inner) = bonded_frame(
            self,
            f,
            area,
            state,
            theme,
            focused,
            Borders::TOP | Borders::LEFT | Borders::RIGHT,
            false,
        ) else {
            return;
        };
        // The bars grow into the height the bond gives the profile: more rows
        // are finer duty levels (eight to a row), standing on the seam, rather
        // than three rows under an empty band.
        let width = inner.width as usize;
        let spare =
            (inner.height as usize).saturating_sub(lines(state, theme, width, true, ROWS).len());
        let out = lines(state, theme, width, true, ROWS + spare);
        f.render_widget(Paragraph::new(out), inner);
    }

    fn chrome(&self, state: &SdrMetrics) -> PanelChrome {
        let chrome = PanelChrome::new("Band Occupancy")
            .stale_when(Staleness::NotStreaming)
            .tag_if(true, state.net.mode.tag());
        // The bars are the latest reading of each cell, so what they span is
        // back to the oldest of those readings: a whole pass while surveying,
        // moments while locked. No measured cell, no numbers, nothing to caveat.
        match oldest_reading_age(state) {
            Some(age) => chrome.counts_from_feed(FeedSpan::Window(age)),
            None => chrome,
        }
    }

    fn render(
        &self,
        f: &mut Frame,
        inner: Rect,
        state: &SdrMetrics,
        theme: &crate::Theme,
        _focused: bool,
    ) {
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        f.render_widget(
            Paragraph::new(lines(state, theme, inner.width as usize, false, ROWS)),
            inner,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::fixture::draw;

    /// Cells 12 to 46 observed - eighteen megahertz around channel 6 plus a
    /// little - with three busy runs in it.
    fn surveyed() -> SdrMetrics {
        let mut m = SdrMetrics::fixture().streaming();
        let mut cells = vec![CellReading::default(); occupancy::CELLS];
        for (i, c) in cells.iter_mut().enumerate() {
            if (12..=46).contains(&i) {
                c.windows = 8_000;
                c.peak_dbfs = -70.0;
            }
        }
        for (i, duty, peak) in [(24usize, 0.62, -32.0), (36, 0.18, -48.0), (41, 0.95, -22.0)] {
            cells[i].duty = duty;
            cells[i].peak_dbfs = peak;
        }
        m.net.band = crate::state::BandOccupancy {
            cells,
            noise_dbfs: Some(-78.4),
            trusted: true,
            tail: 2.07,
            spread: 40.2,
            window_s: 6.4e-6,
            watch_start: None,
            // A dwell has no past: the history is the band's, and `absorb`
            // owns it.
            history: Default::default(),
            last_column: None,
            columns_taken: 0,
        };
        m
    }

    /// The panel's whole job: a cell nobody looked at does not read as a cell
    /// with nothing in it.
    /// The mode costs coverage, and the panel says so in figures as well as in
    /// the word on its nameplate.
    #[test]
    fn the_panel_reports_what_fraction_of_the_time_it_was_looking() {
        let mut m = surveyed();
        for c in m.net.band.cells.iter_mut().filter(|c| c.observed()) {
            c.coverage = Some(0.16);
        }
        let out = draw(NetOccupancyPanel, 80, 12, &m).join("\n");
        assert!(out.contains("16 % of the time"), "{out}");
        assert!(out.contains("[SURVEY]"), "{out}");

        // Locked, the same cells are watched all the time.
        let mut m = surveyed();
        m.net.mode = crate::state::NetMode::Lock;
        for c in m.net.band.cells.iter_mut().filter(|c| c.observed()) {
            c.coverage = Some(1.0);
        }
        let out = draw(NetOccupancyPanel, 80, 12, &m).join("\n");
        assert!(out.contains("100 % of the time"), "{out}");
        assert!(out.contains("[LOCK]"), "{out}");

        // Before a cell has been measured twice the question has no answer, and
        // the panel says that rather than guessing at one.
        let out = draw(NetOccupancyPanel, 80, 12, &surveyed()).join("\n");
        assert!(out.contains("every cell measured once so far"), "{out}");
    }

    /// The busiest reading prints, which is a claim about the dwell as much as
    /// about the widget.
    ///
    /// `Reading` dashes a value its uncertainty cannot support, so this passes
    /// only while the duty cycle's binomial error is inside the resolution the
    /// panel shows it at. It was not, at first: N14 declared a tenth of a
    /// percent that a fifty-millisecond dwell cannot pay for, and this is the
    /// assertion that would have caught it.
    #[test]
    fn the_busiest_cell_prints_a_number_rather_than_a_dash() {
        let mut m = surveyed();
        m.net.band.cells[41].windows = 7_800;
        let out = draw(NetOccupancyPanel, 80, 12, &m);
        let line = out
            .iter()
            .find(|l| l.contains("busiest"))
            .expect("a busiest line");
        assert!(line.contains("2441 MHz"), "{line}");
        assert!(line.contains("95."), "{line}");
        assert!(
            line.contains('±'),
            "the sampling spread travels with it: {line}"
        );
        assert!(!line.contains('—'), "{line}");
    }

    #[test]
    fn unobserved_and_empty_are_drawn_differently() {
        let m = surveyed();
        let out = draw(NetOccupancyPanel, 85, 10, &m);
        let baseline = out.iter().find(|l| l.contains(FLOOR)).expect("a baseline");
        assert!(baseline.contains(UNSEEN), "{baseline}");
        assert!(baseline.contains(FLOOR), "{baseline}");
        // And the run of unobserved cells is on the outside, where it belongs.
        let body: String = baseline
            .chars()
            .filter(|c| *c != '│' && *c != ' ')
            .collect();
        assert!(body.starts_with(UNSEEN), "{baseline}");
        assert!(body.ends_with(UNSEEN), "{baseline}");
        assert!(out.join("\n").contains("35 of 83 MHz in view"));
    }

    /// A duty cycle is a height, and a bigger one is taller.
    #[test]
    fn a_busier_cell_is_a_taller_bar() {
        let quiet = column(
            &CellReading {
                windows: 10,
                duty: 0.1,
                ..Default::default()
            },
            ROWS,
        );
        let busy = column(
            &CellReading {
                windows: 10,
                duty: 0.9,
                ..Default::default()
            },
            ROWS,
        );
        let height = |c: &Vec<char>| c.iter().filter(|ch| **ch != ' ').count();
        assert!(height(&busy) > height(&quiet), "{busy:?} vs {quiet:?}");
        // Full is full, and empty still shows where the floor is.
        let full = column(
            &CellReading {
                windows: 10,
                duty: 1.0,
                ..Default::default()
            },
            ROWS,
        );
        assert_eq!(full, vec!['█'; ROWS]);
        let empty = column(
            &CellReading {
                windows: 10,
                duty: 0.0,
                ..Default::default()
            },
            ROWS,
        );
        assert_eq!(empty, [' ', ' ', FLOOR]);
    }

    /// Squeezing the band into fewer columns keeps the worst cell, not the mean.
    #[test]
    fn a_narrow_panel_shows_the_busiest_cell_and_not_the_average() {
        let mut cells = vec![CellReading::default(); occupancy::CELLS];
        for c in cells.iter_mut() {
            c.windows = 100;
        }
        cells[41].duty = 1.0;
        let cols = columns(&cells, 20);
        assert_eq!(cols.len(), 20);
        let peak = cols.iter().filter(|c| c.duty > 0.5).count();
        assert_eq!(peak, 1, "one saturated megahertz is still saturated");
        // A column of cells nobody looked at stays unlooked-at.
        let none = columns(&vec![CellReading::default(); occupancy::CELLS], 20);
        assert!(none.iter().all(|c| !c.observed()));
    }

    /// Everything here rests on the floor, so a floor that failed takes the
    /// profile with it rather than being a colour on the same picture.
    #[test]
    fn an_untrusted_floor_draws_no_profile_at_all() {
        let mut m = surveyed();
        m.net.band.trusted = false;
        m.net.band.tail = 3.9;
        let out = draw(NetOccupancyPanel, 85, 10, &m).join("\n");
        assert!(out.contains("no floor"), "{out}");
        assert!(out.contains("tail 3.90 (max 2.41)"), "{out}");
        assert!(!out.contains('█'), "{out}");
        assert!(!out.contains(UNSEEN), "{out}");
    }

    /// **Two silences, told apart.** A survey that cannot run is not a survey
    /// that has not started, and the panel must not tell a reader to wait for
    /// something that will never arrive.
    ///
    /// This was found on a radio at 2 Msps: the profile said "waiting for RX"
    /// while the feed panel beside it counted two hundred blocks in, and the
    /// only true sentence was in a log neither of that preset's panels carries.
    #[test]
    fn a_survey_that_cannot_run_says_so_rather_than_asking_for_patience() {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.survey_refused =
            Some("1.8 MHz of view is too narrow to hold a whole megahertz".to_string());
        let out = draw(NetOccupancyPanel, 80, 12, &m).join("\n");
        assert!(out.contains("1.8 MHz"), "{out}");
        assert!(out.contains("too narrow"), "{out}");
        assert!(!out.contains("waiting for RX"), "{out}");
        // And what to do about it, since neither answer is obvious.
        assert!(out.contains("[M]") || out.contains("sample rate"), "{out}");
    }

    #[test]
    fn a_band_never_measured_says_so_rather_than_drawing_an_empty_one() {
        let m = SdrMetrics::fixture();
        let out = draw(NetOccupancyPanel, 85, 10, &m).join("\n");
        assert!(out.contains("waiting for RX"), "{out}");
        assert!(out.contains('—'), "no floor to report yet: {out}");
        assert!(!out.contains(FLOOR), "{out}");
    }

    /// The ruler never writes a channel number over its neighbour, at any width.
    /// The cursor's readout: the selected cell's frequency, its duty with
    /// the uncertainty its windows support, its power, how long it was watched
    /// and when, and a mark under the column the cell is drawn in.
    #[test]
    fn the_cursor_reads_out_the_cell_it_is_on() {
        let mut m = surveyed();
        m.net.band.cells[24].measured = Some(std::time::Instant::now());
        m.net.band_cursor.selected = Some(24);
        let out = draw(NetOccupancyPanel, 120, 14, &m);
        let all = out.join("\n");
        let readout = out.iter().find(|l| l.contains("cursor")).expect(&all);
        assert!(readout.contains("2424 MHz"), "{readout}");
        assert!(readout.contains("% busy"), "{readout}");
        assert!(readout.contains("-32.0 peak"), "{readout}");
        assert!(readout.contains("8000 windows, 51 ms watched"), "{readout}");
        assert!(readout.contains("0 s ago"), "{readout}");
        assert!(
            !all.contains("busiest"),
            "the readout takes the headline's place"
        );
        // The mark sits under the column cell 24 is drawn in.
        let mark = out.iter().find(|l| l.contains('\u{25b2}')).expect(&all);
        let col = mark.chars().position(|c| c == '\u{25b2}').unwrap() - 1;
        assert_eq!(col, band_axis::column_of(24, 118), "{mark}");
    }

    /// **At a past moment the profile shows what the history kept, and says
    /// so.** The bars are that moment's duties; the headline is when it was
    /// and that only duty is kept; the busiest cell then is named with its
    /// duty and no spread, since no window count survives to give one.
    #[test]
    fn a_past_moment_shows_the_history_duty_only() {
        let mut m = surveyed();
        let mut then = vec![-1.0f32; occupancy::CELLS];
        then[30] = 0.75;
        then[31] = 0.0;
        m.net.band.history = vec![then, vec![0.1; occupancy::CELLS]].into();
        m.net.band.columns_taken = 2;
        m.net.band_scrub = m.net.band.id_back(1);
        let all = draw(NetOccupancyPanel, 100, 14, &m).join("\n");
        assert!(all.contains("moment       0.5 s ago"), "{all}");
        assert!(all.contains("floor (now)"), "the floor is today's: {all}");
        assert!(all.contains("duty only"), "{all}");
        assert!(all.contains("busiest      2430 MHz   75 % busy"), "{all}");
        assert!(!all.contains('\u{00b1}'), "no spread is invented: {all}");
        assert!(!all.contains("coverage"), "coverage is about now: {all}");

        m.net.band_cursor.selected = Some(5);
        let all = draw(NetOccupancyPanel, 100, 14, &m).join("\n");
        assert!(all.contains("2405 MHz   not observed then"), "{all}");
    }

    /// Bonded, the bars grow into the height the bond gives the profile, and
    /// stand on the seam: the last row of the half is a bar row.
    #[test]
    fn bonded_bars_fill_the_half_they_are_given() {
        use ratatui::{backend::TestBackend, Terminal};
        let m = surveyed();
        let theme = crate::Theme::sdr();
        let mut term = Terminal::new(TestBackend::new(100, 20)).unwrap();
        term.draw(|f| NetOccupancyPanel.render_bonded(f, f.size(), &m, &theme, false, Bond::Below))
            .unwrap();
        let buf = term.backend().buffer();
        let bar_rows = (0..20u16)
            .filter(|&y| {
                let row: String = (1..99).map(|x| buf.get(x, y).symbol()).collect();
                row.contains(UNSEEN)
            })
            .count();
        assert!(bar_rows > ROWS, "{bar_rows} bar rows in a 20-row half");
        let last: String = (1..99).map(|x| buf.get(x, 19).symbol()).collect();
        assert!(
            last.contains(UNSEEN),
            "the bars stand on the seam: {last:?}"
        );
    }

    /// A cell nobody looked at says so and borrows nobody's figures.
    #[test]
    fn a_cursor_on_an_unobserved_cell_says_nobody_looked() {
        let mut m = surveyed();
        m.net.band_cursor.selected = Some(3);
        let all = draw(NetOccupancyPanel, 100, 14, &m).join("\n");
        assert!(all.contains("2403 MHz"), "{all}");
        assert!(all.contains("not observed"), "{all}");
        assert!(!all.contains("% busy"), "{all}");
    }

    /// The readout is built of whole groups: at any width the panel can have,
    /// its visible text ends at the end of a group, never inside a figure.
    #[test]
    fn the_readout_drops_whole_groups_on_a_narrow_panel() {
        let mut m = surveyed();
        m.net.band_cursor.selected = Some(24);
        // From the panel's declared minimum width, 40, frame included.
        for w in 40..140u16 {
            let theme = crate::Theme::sdr();
            let line = cursor_line(24, &m.net.band.cells[24], 6.4e-6, (w - 2) as usize, &theme);
            let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            assert!(text.chars().count() <= (w - 2) as usize, "{w}: {text:?}");
            assert!(
                text.ends_with(" MHz")
                    || text.ends_with("% busy")
                    || text.ends_with("dBFS")
                    || text.ends_with("watched")
                    || text.ends_with('\u{2014}')
                    || text.ends_with("ago"),
                "{w}: {text:?}"
            );
        }
        m.net.band_cursor.selected = None;
    }

    #[test]
    fn it_fits_every_size_the_layout_can_hand_it() {
        for w in 20..120u16 {
            let r = band_axis::ruler(w as usize);
            assert_eq!(r.chars().count(), w as usize);
            // Every run of digits in the ruler is a whole channel number, so
            // "12" is never a 1 and a 2 from different channels touching.
            for run in r.split(' ').filter(|s| !s.is_empty()) {
                let n: u8 = run.parse().unwrap_or_else(|_| panic!("{w}: {r:?}"));
                assert!((1..=13).contains(&n), "{w}: {r:?}");
            }
            for h in 4..20u16 {
                for line in draw(NetOccupancyPanel, w, h, &surveyed()) {
                    assert!(line.chars().count() <= w as usize, "{w}x{h}: {line:?}");
                }
            }
        }
    }
}
