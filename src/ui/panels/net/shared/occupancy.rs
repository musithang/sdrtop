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

use crate::signal::net::{band, occupancy};
use crate::state::{CellReading, SdrMetrics};
use crate::ui::panel::{Panel, PanelChrome, Staleness};
use crate::ui::widgets::reading::Reading;

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

/// One column of the profile: the glyph for each of the [`ROWS`] rows.
///
/// The duty cycle is spread over the rows from the bottom up, so a cell busy a
/// third of the time fills the bottom row and no more. Eight levels a row and
/// three rows is twenty-four, which is finer than a terminal column deserves and
/// is what stops a band of quiet channels reading as a flat run of identical
/// stubs.
fn column(cell: &CellReading) -> [char; ROWS] {
    if !cell.observed() {
        return [UNSEEN; ROWS];
    }
    let filled = cell.duty.clamp(0.0, 1.0) * (ROWS * 8) as f64;
    let mut out = [' '; ROWS];
    for (i, slot) in out.iter_mut().enumerate() {
        // Row 0 is the top, so the bottom row is the last one.
        let from_bottom = ROWS - 1 - i;
        let here = (filled - (from_bottom * 8) as f64).clamp(0.0, 8.0);
        *slot = BARS[here.round() as usize];
    }
    // An observed cell with nothing in it still shows where the floor is.
    if out[ROWS - 1] == ' ' {
        out[ROWS - 1] = FLOOR;
    }
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
            let lo = x * cells.len() / width;
            let hi = ((x + 1) * cells.len() / width).max(lo + 1).min(cells.len());
            cells[lo..hi]
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

/// The channel numbers written under the columns they are centred on.
fn ruler(width: usize) -> String {
    let mut row = vec![b' '; width];
    for ch in 1..=13u8 {
        let Some(hz) = band::wifi_centre_hz(ch) else {
            continue;
        };
        let Some(cell) = occupancy::cell_of(hz as f64) else {
            continue;
        };
        let at = cell * width / occupancy::CELLS;
        let label = ch.to_string();
        // Centred on the channel, and only when it does not tread on the number
        // beside it: a ruler that overwrites its own labels is worse than one
        // with gaps.
        let start = at.saturating_sub(label.len() / 2);
        if start + label.len() > width {
            continue;
        }
        if row[start..start + label.len()].iter().all(|c| *c == b' ')
            && (start == 0 || row[start - 1] == b' ')
        {
            row[start..start + label.len()].copy_from_slice(label.as_bytes());
        }
    }
    String::from_utf8(row).unwrap_or_default()
}

fn lines(state: &SdrMetrics, theme: &crate::Theme, width: usize) -> Vec<Line<'static>> {
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

    // The headline: the one cell a person would want named. Ties go to the
    // lower frequency, which is arbitrary but is at least the same arbitrary
    // choice every frame.
    let busiest = occ
        .cells
        .iter()
        .enumerate()
        .filter(|(_, c)| c.observed() && c.duty > 0.0)
        .max_by(|a, b| a.1.duty.total_cmp(&b.1.duty));
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

    // How much of the time the band was actually under the receiver, which is
    // what the mode costs and the one number that says it in figures rather than
    // as a word in the chrome.
    let covered: Vec<f64> = occ.cells.iter().filter_map(|c| c.coverage).collect();
    out.push(Line::from(Span::styled(
        match covered.len() {
            0 => "coverage     — · every cell measured once so far".to_string(),
            n => format!(
                "coverage     {:.0} % of the time, on {n} of {} cells",
                covered.iter().sum::<f64>() / n as f64 * 100.0,
                occupancy::CELLS
            ),
        },
        dim,
    )));

    let cols = columns(&occ.cells, width);
    for row in 0..ROWS {
        let spans = cols
            .iter()
            .map(|c| {
                let ch = column(c)[row];
                let colour = if !c.observed() {
                    theme.stale
                } else if ch == ' ' || ch == FLOOR {
                    theme.label
                } else {
                    // Duty is the height; the power is what colours it, on the
                    // same ramp the waterfall uses.
                    theme.palette_color(((c.peak_dbfs + 90.0) / 90.0).clamp(0.0, 1.0) as f32)
                };
                Span::styled(ch.to_string(), Style::default().fg(colour))
            })
            .collect::<Vec<_>>();
        out.push(Line::from(spans));
    }
    out.push(Line::from(Span::styled(ruler(width), dim)));
    out.push(Line::from(Span::styled(
        format!(
            "{:<width$}",
            format!("{} MHz", band::LOW_HZ / 1_000_000),
            width = width.saturating_sub(8)
        ) + &format!("{} MHz", band::HIGH_HZ / 1_000_000),
        dim,
    )));
    out
}

impl Panel for NetOccupancyPanel {
    fn name(&self) -> &'static str {
        "net_occupancy"
    }

    fn min_size(&self) -> (u16, u16) {
        (40, 8)
    }

    fn chrome(&self, state: &SdrMetrics) -> PanelChrome {
        PanelChrome::new("Band Occupancy")
            .stale_when(Staleness::NotStreaming)
            .tag_if(true, state.net.mode.tag())
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
            Paragraph::new(lines(state, theme, inner.width as usize)),
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
        let quiet = column(&CellReading {
            windows: 10,
            duty: 0.1,
            ..Default::default()
        });
        let busy = column(&CellReading {
            windows: 10,
            duty: 0.9,
            ..Default::default()
        });
        let height = |c: [char; ROWS]| c.iter().filter(|ch| **ch != ' ').count();
        assert!(height(busy) > height(quiet), "{busy:?} vs {quiet:?}");
        // Full is full, and empty still shows where the floor is.
        let full = column(&CellReading {
            windows: 10,
            duty: 1.0,
            ..Default::default()
        });
        assert_eq!(full, ['█'; ROWS]);
        let empty = column(&CellReading {
            windows: 10,
            duty: 0.0,
            ..Default::default()
        });
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
    #[test]
    fn it_fits_every_size_the_layout_can_hand_it() {
        for w in 20..120u16 {
            let r = ruler(w as usize);
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
