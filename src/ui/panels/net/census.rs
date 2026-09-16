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
use crate::signal::net::census::{order, Device, SORT_KEYS};
use crate::state::SdrMetrics;
use crate::ui::panel::{Panel, PanelChrome, Staleness, Tag};
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

        let mut devices: Vec<Device> = census.devices.clone();
        order(&mut devices, census.sort, census.descending, now);
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

        // One row for the header, so the list gets the rest.
        let body = inner.height.saturating_sub(1) as usize;
        let cursor = census.cursor(&addresses).unwrap_or(0);
        let start = viewport_start(census.first_visible, cursor, devices.len(), body);
        for (i, d) in devices.iter().enumerate().skip(start).take(body) {
            lines.push(row(COLUMNS, fit, &cells(d, now), i == cursor, theme));
        }
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
                last_seen: now - Duration::from_secs(2),
                crystal_offset_hz: Some(Uncertain::exact(85_000.0)),
            },
            Device {
                address: [0xf0, 0x18, 0x98, 0x00, 0x11, 0x22],
                packets: 7,
                best_snr_db: 2.4,
                last_seen: now - Duration::from_secs(240),
                crystal_offset_hz: None,
            },
            Device {
                address: [0x00, 0x1a, 0x11, 0xaa, 0xbb, 0xcc],
                packets: 96,
                best_snr_db: 6.9,
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
        let out = draw(NetCensusPanel, 60, 10, &SdrMetrics::fixture().streaming()).join("\n");
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
        m.net.census.selected = Some(busiest);

        m.net.census.sort = 2;
        m.net.census.descending = true;
        let rows = draw(NetCensusPanel, 60, 10, &m);
        let picked = rows.iter().position(|l| l.contains("a4:83:e7")).unwrap();

        m.net.census.sort = 0;
        m.net.census.descending = false;
        let rows = draw(NetCensusPanel, 60, 10, &m);
        let moved = rows.iter().position(|l| l.contains("a4:83:e7")).unwrap();
        assert_eq!(moved, picked + 1, "the re-sort moved it down one");

        // And the state still points at it: the cursor is an address, so
        // nothing in the panel had to follow the row.
        let by_address = [
            [0x00, 0x1a, 0x11, 0xaa, 0xbb, 0xcc],
            busiest,
            [0xf0, 0x18, 0x98, 0x00, 0x11, 0x22],
        ];
        assert_eq!(m.net.census.cursor(&by_address), Some(1));
        let by_packets = [busiest, by_address[0], by_address[2]];
        assert_eq!(m.net.census.cursor(&by_packets), Some(0));
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
