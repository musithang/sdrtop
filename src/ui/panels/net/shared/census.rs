// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `NetCensusPanel` - who is here.
//!
//! Design section 9.3 gives this preset one question and this is it: the
//! population of the band, one row per transmitter, ordered by whichever column
//! the user picked. The table itself is `ui::widgets::table`; what is here is
//! the column list, the empty state, and the keys.
//!
//! **Nothing fills it yet, and the panel says which of two things that means.**
//! An empty census could be "we listened and nobody transmitted" or "nothing is
//! listening". Those are different claims and only the second is true today, so
//! that is the one it makes. Printing a bare empty table would let a reader
//! infer the first - the same reason the feed-health panel dashes its burst
//! count instead of showing zero.

use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::signal::dsp::uncertainty::Uncertain;
use crate::signal::net::census::{Device, SORT_KEYS};
use crate::state::SdrMetrics;
use crate::ui::panel::{FeedSpan, Panel, PanelChrome, Staleness, Tag};
use crate::ui::widgets::reading::Reading;
use crate::ui::widgets::table::{
    columns_that_fit, header, row, viewport_start, Align, Column, Sort,
};

pub struct NetCensusPanel;

/// The columns, in the order they are drawn and in the same order as
/// [`SORT_KEYS`], so the header, the chrome tag and the ordering cannot disagree
/// about which column is which.
const COLUMNS: &[Column] = &[
    Column {
        title: "ADDRESS",
        width: 17,
        align: Align::Left,
    },
    Column {
        title: "SEEN",
        width: 7,
        align: Align::Right,
    },
    Column {
        title: "PKTS",
        width: 7,
        align: Align::Right,
    },
    Column {
        title: "SNR",
        width: 8,
        align: Align::Right,
    },
    Column {
        title: "CFO",
        width: 15,
        align: Align::Right,
    },
];

/// `37.0 ±1.2 kHz` - a device's own refined crystal-error estimate
/// ([`crate::signal::net::census::observe`]), through the same
/// value-with-uncertainty cell every measurement in the app uses. `-`
/// before any packet from this device has reported one: an absent
/// measurement, not a zero-error clock.
fn fmt_cfo(offset: Option<Uncertain>) -> String {
    match offset {
        Some(u) => Reading::new(u.scale(0.001), "kHz", f64::INFINITY).text(),
        None => "-".to_string(),
    }
}

/// `2 s` / `4 min` - how long ago, at the resolution anybody reads it at.
fn ago(secs: u64) -> String {
    if secs < 90 {
        format!("{secs} s")
    } else {
        format!("{} min", secs / 60)
    }
}

fn cells(d: &Device, now: std::time::Instant) -> Vec<String> {
    vec![
        d.address_text(),
        ago(now.saturating_duration_since(d.last_seen).as_secs()),
        d.packets.to_string(),
        format!("{:.1} dB", d.best_snr_db),
        fmt_cfo(d.crystal_offset_hz),
    ]
}

/// B13's own window for "per unit time": five minutes, long enough to see a
/// handful of rotations from a device turning its address over on the
/// specification's own cadence (roughly every 15 minutes) without the
/// figure jumping to zero every time nobody new has shown up in the last
/// few seconds.
const TURNOVER_WINDOW: std::time::Duration = std::time::Duration::from_secs(300);

/// B13's exit condition, on screen: how many distinct addresses have
/// appeared per unit time - a measurement about the protocol's own address
/// rotation, not a claim about which of them are the same device wearing a
/// new one. See [`crate::signal::net::census::turnover_per_minute`]'s own
/// doc for why that claim is not this measurement's to make.
fn turnover_line(
    devices: &[Device],
    now: std::time::Instant,
    theme: &crate::Theme,
) -> Line<'static> {
    let rate = crate::signal::net::census::turnover_per_minute(devices, TURNOVER_WINDOW, now);
    Line::from(Span::styled(
        format!("{rate:.1} new addresses/min (last 5 min)"),
        Style::default().fg(theme.label),
    ))
}

impl Panel for NetCensusPanel {
    fn name(&self) -> &'static str {
        "net_census"
    }

    fn min_size(&self) -> (u16, u16) {
        (34, 8)
    }

    fn focus_key(&self) -> Option<char> {
        // The obvious letters are taken: `c` by the constellation, `d` by the
        // demod, `n` and `l` elsewhere. `u` is free and is the one this panel
        // gets.
        Some('u')
    }

    fn focus_bindings(&self) -> &'static [(&'static str, &'static str)] {
        &[
            ("↑↓", "select"),
            ("S", "sort by the next column"),
            ("R", "reverse"),
        ]
    }

    fn chrome(&self, state: &SdrMetrics) -> PanelChrome {
        let c = &state.net.census;
        PanelChrome::new("Band Cens_us")
            .stale_when(Staleness::NotStreaming)
            // Packet counts and first/last sightings accumulate for the whole
            // session, so a drop at any point in it undercounts them.
            .counts_from_feed(FeedSpan::Session)
            .tag_if(true, state.net.mode.tag())
            .tag_if(
                true,
                Tag::Sorted(SORT_KEYS.get(c.sort).copied().unwrap_or("?"), c.descending),
            )
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
        let width = inner.width as usize;
        let fit = columns_that_fit(COLUMNS, width);
        let census = &state.net.census;
        let now = std::time::Instant::now();

        let devices: Vec<Device> = census.ordered(now);
        let addresses: Vec<[u8; 6]> = devices.iter().map(|d| d.address).collect();

        let mut lines = vec![header(
            COLUMNS,
            fit,
            Sort {
                column: census.sort,
                descending: census.descending,
            },
            theme,
        )];

        if devices.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "no census yet".to_string(),
                Style::default().fg(theme.stale),
            )));
            lines.push(Line::from(Span::styled(
                "nothing decodes an address on this band yet, so nobody has".to_string(),
                Style::default().fg(theme.label),
            )));
            lines.push(Line::from(Span::styled(
                "been counted - this is not an empty room".to_string(),
                Style::default().fg(theme.label),
            )));
            f.render_widget(Paragraph::new(lines), inner);
            return;
        }

        // One row for the header and one for the turnover summary, so the
        // list gets the rest.
        let body = (inner.height as usize).saturating_sub(2);
        // No selection highlights no row: a highlight on row zero that nobody
        // chose would claim a selection that does not exist.
        let cursor = census.selection.cursor(&addresses);
        let start = viewport_start(
            census.selection.first_visible,
            cursor.unwrap_or(0),
            devices.len(),
            body,
        );
        for (i, d) in devices.iter().enumerate().skip(start).take(body) {
            lines.push(row(COLUMNS, fit, &cells(d, now), Some(i) == cursor, theme));
        }
        lines.push(turnover_line(&devices, now, theme));
        f.render_widget(Paragraph::new(lines), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::net::census::Device;
    use crate::state::fixture::draw;
    use std::time::{Duration, Instant};

    fn populated() -> SdrMetrics {
        let now = Instant::now();
        let mut m = SdrMetrics::fixture().streaming();
        m.net.census.devices = vec![
            Device {
                address: [0xa4, 0x83, 0xe7, 0x1c, 0x09, 0xbe],
                packets: 1_204,
                best_snr_db: 12.3,
                first_seen: now - Duration::from_secs(600),
                last_seen: now - Duration::from_secs(2),
                crystal_offset_hz: Some(Uncertain::exact(85_000.0)),
            },
            Device {
                address: [0xf0, 0x18, 0x98, 0x00, 0x11, 0x22],
                packets: 7,
                best_snr_db: 2.4,
                // Clearly inside the five-minute turnover window, not on
                // its boundary: the panel calls `Instant::now()` again at
                // render time, later than this fixture's own `now`, so a
                // value exactly at the window's edge could land either side
                // of it depending on how much time the test itself takes.
                first_seen: now - Duration::from_secs(250),
                last_seen: now - Duration::from_secs(240),
                crystal_offset_hz: None,
            },
            Device {
                address: [0x00, 0x1a, 0x11, 0xaa, 0xbb, 0xcc],
                packets: 96,
                best_snr_db: 6.9,
                first_seen: now - Duration::from_secs(90),
                last_seen: now - Duration::from_secs(31),
                crystal_offset_hz: Some(Uncertain::exact(-12_000.0)),
            },
        ];
        m
    }

    /// **An empty census says which of the two empties it is.**
    ///
    /// "We listened and nobody transmitted" and "nothing is listening" are
    /// different claims and only the second is true today. A bare empty table
    /// would let a reader take the first, which is the flattering one.
    #[test]
    fn an_empty_census_says_nothing_is_counting_rather_than_nobody_is_there() {
        // Wide enough for every column, the selection gutter included.
        let out = draw(NetCensusPanel, 64, 10, &SdrMetrics::fixture().streaming()).join("\n");
        assert!(out.contains("no census yet"), "{out}");
        assert!(out.contains("not an empty room"), "{out}");
        // The columns are still shown, so the shape of the answer is visible.
        assert!(out.contains("ADDRESS"), "{out}");
        assert!(out.contains("SNR"), "{out}");
        assert!(out.contains("CFO"), "{out}");
    }

    /// The CFO cell shows the value once a device has one, and dashes when it
    /// does not - an absent measurement, not a zero-error clock.
    #[test]
    fn cfo_shows_when_measured_and_dashes_when_not() {
        let out = draw(NetCensusPanel, 70, 10, &populated()).join("\n");
        assert!(out.contains("85.0"), "measured CFO should show: {out}");
        assert!(out.contains("-12.0"), "a negative CFO should show: {out}");
        // The unmeasured device's row still has a dash, not a blank cell
        // that could be misread as zero.
        let f0_row = out
            .lines()
            .find(|l| l.contains("f0:18:98"))
            .expect("the unmeasured device's row");
        assert!(
            f0_row.trim_end_matches(['│', ' ']).ends_with('-'),
            "{f0_row:?}"
        );
    }

    /// B13's own exit condition: two of `populated`'s three devices were
    /// first seen inside the five-minute window (-250 s and -90 s; -600 s
    /// was not), so the rate is `2 / 5 minutes`.
    #[test]
    fn the_turnover_line_counts_only_recent_first_sightings() {
        let out = draw(NetCensusPanel, 70, 12, &populated()).join("\n");
        assert!(out.contains("0.4 new addresses/min"), "{out}");
    }

    /// The chrome says how the table is ordered, so the answer does not depend
    /// on spotting a marker halfway across the header.
    #[test]
    fn the_chrome_says_what_orders_the_table() {
        let mut m = populated();
        m.net.census.sort = 2;
        m.net.census.descending = true;
        let out = draw(NetCensusPanel, 60, 10, &m);
        assert!(out[0].contains("\u{2193}PKTS"), "{}", out[0]);
        assert!(out[1].contains("PKTS\u{25be}"), "{}", out[1]);

        m.net.census.sort = 0;
        m.net.census.descending = false;
        let out = draw(NetCensusPanel, 60, 10, &m);
        assert!(out[0].contains("\u{2191}ADDRESS"), "{}", out[0]);
    }

    #[test]
    fn the_rows_come_out_in_the_order_the_state_asked_for() {
        let mut m = populated();
        m.net.census.sort = 2;
        m.net.census.descending = true;
        let out = draw(NetCensusPanel, 60, 10, &m).join("\n");
        let at = |s: &str| out.find(s).unwrap_or(usize::MAX);
        assert!(at("a4:83:e7") < at("00:1a:11"), "1204 before 96:\n{out}");
        assert!(at("00:1a:11") < at("f0:18:98"), "96 before 7:\n{out}");

        // Ascending by address is a different order, and the panel follows it.
        m.net.census.sort = 0;
        m.net.census.descending = false;
        let out = draw(NetCensusPanel, 60, 10, &m).join("\n");
        let at = |s: &str| out.find(s).unwrap_or(usize::MAX);
        assert!(at("00:1a:11") < at("a4:83:e7"), "{out}");
    }

    /// The cursor is on the device it was put on, wherever a re-sort moved it.
    ///
    /// The device has to be one that actually moves: the quietest is last both
    /// by packet count and by address, so selecting it would have proved
    /// nothing. The busiest is first by packets and second by address.
    #[test]
    fn the_cursor_stays_on_its_device_across_a_resort() {
        let busiest = [0xa4, 0x83, 0xe7, 0x1c, 0x09, 0xbe];
        let mut m = populated();
        m.net.census.selection.selected = Some(busiest);

        m.net.census.sort = 2;
        m.net.census.descending = true;
        let rows = draw(NetCensusPanel, 60, 10, &m);
        let picked = rows.iter().position(|l| l.contains("a4:83:e7")).unwrap();
        assert_eq!(marked(&rows), vec![picked], "the mark is on its row");

        m.net.census.sort = 0;
        m.net.census.descending = false;
        let rows = draw(NetCensusPanel, 60, 10, &m);
        let moved = rows.iter().position(|l| l.contains("a4:83:e7")).unwrap();
        assert_eq!(moved, picked + 1, "the re-sort moved it down one");
        assert_eq!(marked(&rows), vec![moved], "and the mark went with it");
    }

    /// The rows carrying the selection mark.
    fn marked(rows: &[String]) -> Vec<usize> {
        rows.iter()
            .enumerate()
            .filter(|(_, l)| l.contains('\u{258c}'))
            .map(|(i, _)| i)
            .collect()
    }

    /// The census accumulates for the session, so any loss in it - however
    /// long ago - makes its counts lower bounds, and the frame says so.
    #[test]
    fn a_session_loss_marks_the_census_as_a_lower_bound() {
        let mut m = populated();
        let clean = draw(NetCensusPanel, 70, 10, &m);
        assert!(!clean[0].contains("FEED LOSS"), "{}", clean[0]);

        m.net.health.last_loss = Some(Instant::now() - Duration::from_secs(600));
        let lossy = draw(NetCensusPanel, 90, 10, &m);
        assert!(lossy[0].contains("[FEED LOSS]"), "{}", lossy[0]);
    }

    /// Nothing selected, nothing marked. It used to mark the first row
    /// regardless, which claimed a selection nobody had made.
    #[test]
    fn no_selection_marks_no_row() {
        let rows = draw(NetCensusPanel, 60, 10, &populated());
        assert!(marked(&rows).is_empty(), "{}", rows.join("\n"));
    }

    #[test]
    fn it_fits_every_size_the_layout_can_hand_it() {
        for w in 20..90u16 {
            for h in 4..20u16 {
                for m in [populated(), SdrMetrics::fixture()] {
                    for line in draw(NetCensusPanel, w, h, &m) {
                        assert!(line.chars().count() <= w as usize, "{w}x{h}: {line:?}");
                    }
                }
            }
        }
    }
}
