// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `NetBleConnectionPanel`: one BLE connection, event by event.
//!
//! **Which connection.** Every connection whose CONNECT_IND was heard is
//! followed (`signal::ble::follow`, `NetState::ble_connections`); this shows
//! the one selected, or the newest. With none followed, it says what starts
//! one.
//!
//! **What an event's account means.** `followed`: a packet with the
//! connection's access address was heard in it. `missed`: its channel was in
//! view and nothing was heard, which says nothing about why (the Peripheral
//! may skip events, the Central may send nothing). `not in view`: its
//! channel is outside the band the radio sees. `feed lost`: the samples
//! were not there to listen to. Four words, four different things.
//!
//! **Who sent it.** The Central opens each event at its anchor and the two
//! take turns T_IFS apart (Core 5.4 Vol 6 Part B 4.5.1, 4.1.1): `C→P` the
//! Central's, in the connection's colour, `P→C` the Peripheral's, in the
//! ordinary ink; a packet neither rule places gets a dot.

use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::signal::ble::follow::{Account, Event, HeardPdu, Sender, State};
use crate::state::{FollowedConnection, SdrMetrics};
use crate::ui::panel::{FeedSpan, Panel, PanelChrome, Staleness};
use crate::ui::widgets::table::{
    breathe, columns_that_fit, grow_to_contents, header, row, widen, Align, Column, Sort,
};

pub struct NetBleConnectionPanel;

const COLUMNS: &[Column] = &[
    Column {
        title: "EVENT",
        width: 5,
        align: Align::Right,
    },
    Column {
        title: "CH",
        width: 2,
        align: Align::Right,
    },
    Column {
        title: "WHERE",
        width: 11,
        align: Align::Left,
    },
    Column {
        title: "DIR",
        width: 3,
        align: Align::Left,
    },
    Column {
        title: "CRC",
        width: 5,
        align: Align::Left,
    },
    Column {
        title: "PDU",
        width: 12,
        align: Align::Left,
    },
];

const WHERE: usize = 2;
const DIR: usize = 3;
const CRC: usize = 4;
const PDU: usize = 5;

/// The most a wide panel spaces its columns out by.
const BREATHING: usize = 2;

const NOT_KNOWN: &str = "·";

/// What a clock reading is printed to, ppm, at most: its own uncertainty
/// decides below that.
const CLOCK_RESOLUTION_PPM: f64 = 0.1;

/// What T_IFS is printed to, us, at most.
const T_IFS_RESOLUTION_US: f64 = 0.1;

/// The connection the view shows: the one selected, if still followed, or
/// the newest.
pub(crate) fn selected(state: &SdrMetrics) -> Option<&FollowedConnection> {
    let list = &state.net.ble_connections;
    state
        .net
        .connection_view
        .selected
        .and_then(|aa| list.iter().find(|f| f.connection.access_address() == aa))
        .or_else(|| list.first())
}

/// How following stands, in words.
fn state_words(state: &State) -> String {
    match state {
        State::Following => "following".to_string(),
        State::Lost { after: Some(n) } => format!("lost after event {n}"),
        State::Lost { after: None } => "lost before any event was heard".to_string(),
        State::Terminated { reason } => {
            format!("terminated: {}", crate::signal::errors::error_name(*reason))
        }
        State::NotFollowed { why } => why.to_string(),
    }
}

fn account_words(a: Account) -> &'static str {
    match a {
        Account::Followed => "followed",
        Account::Missed => "missed",
        Account::NotInView => "not in view",
        Account::FeedLost => "feed lost",
    }
}

/// What a packet carried, as far as it can be read.
fn pdu_words(h: &HeardPdu, encrypted: bool) -> String {
    let n = h.pdu.payload.len();
    if !h.pdu.crc_ok {
        return format!("LLID {}, {n} octets", h.pdu.llid);
    }
    if h.pdu.llid == 1 && n == 0 {
        return "empty".to_string();
    }
    if let Some(k) = &h.control {
        return match k.name {
            Some(name) if k.words.is_empty() => name.to_string(),
            Some(name) => format!("{name}  {}", k.words),
            None => k.words.clone(),
        };
    }
    if encrypted {
        return format!("encrypted, {n} octets");
    }
    match h.pdu.llid {
        1 => format!("L2CAP continuation, {n} octets"),
        2 => format!("L2CAP start, {n} octets"),
        3 => format!("LL control, {n} octets"),
        _ => format!("LLID 0, {n} octets"),
    }
}

/// What the list shows, newest first: an event, or a run of two or more
/// events in a row whose channels were all out of view, folded into one: at
/// 8 channels of 37 three events in four are, and a row each buried the
/// ones that were heard.
enum Item<'a> {
    Event(&'a Event),
    OutOfView {
        newest: u16,
        oldest: u16,
        count: usize,
    },
}

fn items(c: &crate::signal::ble::follow::Connection) -> Vec<Item<'_>> {
    fn close<'a>(run: &mut Vec<&'a Event>, out: &mut Vec<Item<'a>>) {
        match run.as_slice() {
            [] => {}
            [one] => out.push(Item::Event(one)),
            [first, .., last] => out.push(Item::OutOfView {
                newest: first.counter,
                oldest: last.counter,
                count: run.len(),
            }),
        }
        run.clear();
    }
    let mut out = Vec::new();
    let mut run: Vec<&Event> = Vec::new();
    for e in c.events() {
        if e.account == Account::NotInView {
            run.push(e);
        } else {
            close(&mut run, &mut out);
            out.push(Item::Event(e));
        }
    }
    close(&mut run, &mut out);
    out
}

/// How many rows the list has: what `↓` scrolls through.
pub(crate) fn row_count(c: &crate::signal::ble::follow::Connection) -> usize {
    items(c)
        .iter()
        .map(|i| match i {
            Item::Event(e) => e.pdus.len().max(1),
            Item::OutOfView { .. } => 1,
        })
        .sum()
}

/// A folded run's one row.
fn run_row(
    newest: u16,
    oldest: u16,
    count: usize,
    theme: &crate::Theme,
) -> (Vec<String>, Vec<Option<Color>>) {
    let mut ink = vec![None; COLUMNS.len()];
    ink[WHERE] = Some(theme.stale);
    ink[0] = Some(theme.stale);
    (
        vec![
            format!("{newest}-{oldest}"),
            String::new(),
            format!("not in view ({count})"),
            String::new(),
            String::new(),
            String::new(),
        ],
        ink,
    )
}

/// One event's rows: one a packet, the event's own fields on the first.
fn event_rows(
    e: &Event,
    encrypted_from: Option<u16>,
    central: Color,
    theme: &crate::Theme,
) -> Vec<(Vec<String>, Vec<Option<Color>>)> {
    let where_ink = match e.account {
        Account::Followed => theme.value,
        Account::Missed => theme.label,
        Account::NotInView => theme.stale,
        Account::FeedLost => theme.status_warn,
    };
    let encrypted = encrypted_from.is_some_and(|from| e.counter.wrapping_sub(from) < 0x8000);
    let first = |ink: &mut Vec<Option<Color>>| -> Vec<String> {
        ink[WHERE] = Some(where_ink);
        vec![
            e.counter.to_string(),
            e.channel.to_string(),
            account_words(e.account).to_string(),
        ]
    };
    if e.pdus.is_empty() {
        let mut ink = vec![None; COLUMNS.len()];
        let mut cells = first(&mut ink);
        cells.extend([String::new(), String::new(), String::new()]);
        return vec![(cells, ink)];
    }
    e.pdus
        .iter()
        .enumerate()
        .map(|(i, h)| {
            let mut ink = vec![None; COLUMNS.len()];
            let mut cells = if i == 0 {
                first(&mut ink)
            } else {
                vec![String::new(), String::new(), String::new()]
            };
            let (dir, dir_ink) = match h.sender {
                Some(Sender::Central) => ("C→P", Some(central)),
                Some(Sender::Peripheral) => ("P→C", Some(theme.value)),
                None => (NOT_KNOWN, Some(theme.stale)),
            };
            ink[DIR] = dir_ink;
            let (crc, crc_ink) = if h.pdu.crc_ok {
                ("✓ CRC", theme.status_ok)
            } else {
                ("✗ CRC", theme.status_warn)
            };
            ink[CRC] = Some(crc_ink);
            cells.extend([dir.to_string(), crc.to_string(), pdu_words(h, encrypted)]);
            (cells, ink)
        })
        .collect()
}

/// The parameters now in force, and what the window holds of them.
fn parameter_lines(
    f: &FollowedConnection,
    state: &SdrMetrics,
    width: usize,
    theme: &crate::Theme,
) -> Vec<Line<'static>> {
    let c = &f.connection;
    let p = c.params();
    let span = if state.radio.bb_filter_hz > 0 {
        (state.radio.bb_filter_hz as f64).min(state.radio.config_sample_rate)
    } else {
        state.radio.config_sample_rate
    };
    let view =
        crate::signal::ble::channel::data_channels_in_view(state.radio.frequency as f64, span);
    let used: Vec<u8> = (0..37u8)
        .filter(|ch| p.channel_map >> ch & 1 != 0)
        .collect();
    let seen = view.iter().filter(|ch| used.contains(ch)).count();
    let range = match (view.first(), view.last()) {
        (Some(a), Some(b)) => format!(" ({a}-{b})"),
        _ => String::new(),
    };
    let csa = c.algorithm();
    let (_, sca) = crate::signal::ble::connect::sca_ppm(p.sca);
    let one = format!(
        "AA {} · CSA #{csa} · interval {:.2} ms · latency {} · timeout {} ms · Central SCA ≤{sca} ppm · {} of 37 used · {seen} in view{range}",
        state.net.show_access_address(c.access_address()),
        p.interval as f64 * 1.25,
        p.latency,
        p.timeout as u32 * 10,
        used.len(),
    );
    let phy = |x: crate::signal::ble::Phy| x.label();
    let phys = if c.phy() == c.phy_peripheral() {
        format!("{} both ways", phy(c.phy()))
    } else {
        format!("C→P {} · P→C {}", phy(c.phy()), phy(c.phy_peripheral()))
    };
    let enc = match c.encrypted_from() {
        Some(n) => format!("encrypted from event {n}"),
        None => "not encrypted yet".to_string(),
    };
    let ago = f.seen.elapsed().as_secs();
    let two = format!(
        "Central {} → Peripheral {} · set up {ago} s ago · {phys} · {enc}",
        state.net.show_address(p.init_a, f.init_random, None),
        state.net.show_address(p.adv_a, f.adv_random, None),
    );
    [one, two]
        .into_iter()
        .flat_map(|s| crate::ui::chrome::wrap(&s, width, 2))
        .map(|s| Line::from(Span::styled(s, Style::default().fg(theme.value))))
        .collect()
}

/// The width of a MEASURED row's label.
const LABEL_W: usize = 8;

/// What the events followed measure, read off them: the Central's clock
/// against this radio's, the Peripheral's turns against T_IFS, and the CRC
/// per channel heard.
fn measured_lines(
    f: &FollowedConnection,
    state: &SdrMetrics,
    width: usize,
    theme: &crate::Theme,
) -> Vec<Line<'static>> {
    use crate::signal::ble::follow::T_IFS_TOLERANCE_US;
    use crate::ui::widgets::reading::Reading;
    let c = &f.connection;
    let row = |label: &str, text: String, ink: Color| {
        Line::from(vec![
            crate::ui::chrome::field(label, LABEL_W, theme),
            Span::styled(text, Style::default().fg(ink)),
        ])
    };
    let mut out = vec![crate::ui::chrome::section(
        "measured",
        "Core 5.4 Vol 6 Part B 4.1.1, 4.2.1, 4.5.1",
        width,
        theme,
    )];
    let room = width.saturating_sub(LABEL_W + 1);

    // The clock: relative to this radio until a reference makes it
    // absolute, then judged against the SCA the Central declared, the
    // accuracy its anchors are scheduled by between events.
    let clock = match c.clock_ppm() {
        None => (
            format!("collecting: {} of 3 anchors", c.anchors_heard()),
            theme.label,
        ),
        Some(raw) => {
            let (clock, provenance) = state.radio.corrected_ppm(raw, std::time::Instant::now());
            let (_, sca) = crate::signal::ble::connect::sca_ppm(c.params().sca);
            let reading = Reading::new(clock, "ppm", CLOCK_RESOLUTION_PPM).text();
            let anchors = c.anchors_heard();
            if provenance == crate::state::Provenance::Unreferenced {
                (
                    format!("{reading} against this radio, relative · {anchors} anchors"),
                    theme.value,
                )
            } else {
                // Outside, at the edge within twice its uncertainty, or
                // inside: the classic slot clock's three words.
                let (v, s, limit) = (clock.value().abs(), clock.sigma(), sca as f64);
                let (word, ink) = if v > limit {
                    ("outside", theme.status_warn)
                } else if v + 2.0 * s > limit {
                    ("at the edge of", theme.status_warn)
                } else {
                    ("inside", theme.value)
                };
                (
                    format!("{reading}, {word} its declared ≤{sca} ppm · {anchors} anchors"),
                    ink,
                )
            }
        }
    };
    out.push(row("clock", clock.0, clock.1));

    let t_ifs = match c.t_ifs() {
        None => ("no turn heard yet".to_string(), theme.label),
        Some(t) => (
            format!(
                "{} over {} · {} outside 150 ±{} us",
                Reading::new(t.mean, "us", T_IFS_RESOLUTION_US).text(),
                t.count,
                t.outside,
                T_IFS_TOLERANCE_US
            ),
            if t.outside > 0 {
                theme.status_warn
            } else {
                theme.value
            },
        ),
    };
    out.push(row("T_IFS", t_ifs.0, t_ifs.1));

    let heard: Vec<String> = c
        .per_channel()
        .iter()
        .enumerate()
        .filter(|(_, n)| n.0 > 0)
        .map(|(ch, n)| format!("ch {ch}: {} of {}", n.1, n.0))
        .collect();
    let crc = if heard.is_empty() {
        "nothing heard yet".to_string()
    } else {
        heard.join(" · ")
    };
    for (i, chunk) in crate::ui::chrome::wrap(&crc, room, 2)
        .into_iter()
        .enumerate()
    {
        out.push(row(if i == 0 { "CRC" } else { "" }, chunk, theme.value));
    }
    out
}

/// The last line: the events kept, by account.
fn tally(
    c: &crate::signal::ble::follow::Connection,
    width: usize,
    theme: &crate::Theme,
) -> Line<'static> {
    let count = |a: Account| c.events().iter().filter(|e| e.account == a).count();
    let all = c.events().len();
    let in_view = all - count(Account::NotInView);
    let text = format!(
        " {all} events · {in_view} in view · {} followed · {} missed · {} feed lost",
        count(Account::Followed),
        count(Account::Missed),
        count(Account::FeedLost)
    );
    let text: String = text.chars().take(width).collect();
    Line::from(Span::styled(text, Style::default().fg(theme.label)))
}

impl Panel for NetBleConnectionPanel {
    fn name(&self) -> &'static str {
        "net_ble_connection"
    }

    fn min_size(&self) -> (u16, u16) {
        (30, 6)
    }

    fn focus_key(&self) -> Option<char> {
        Some('e')
    }

    fn focus_bindings(&self) -> &'static [(&'static str, &'static str)] {
        &[
            ("↑↓", "scroll the events"),
            ("End", "back to the newest"),
            ("← →", "the previous or next connection"),
        ]
    }

    fn chrome(&self, state: &SdrMetrics) -> PanelChrome {
        // The events are the session's, counted from the feed.
        let chrome = PanelChrome::new("Connection")
            .stale_when(Staleness::NotStreaming)
            .tag_if(true, state.net.mode.tag())
            .counts_from_feed(FeedSpan::Session);
        let Some(f) = selected(state) else {
            return chrome;
        };
        chrome.shows_addresses().suffix(format!(
            " {} · {}",
            state.net.show_access_address(f.connection.access_address()),
            state_words(f.connection.state())
        ))
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
        let height = inner.height as usize;
        let Some(followed) = selected(state) else {
            let lines = crate::ui::chrome::wrap(
                "no connection followed yet: a CONNECT_IND on the advertising channel in view starts one; reconnect a device near the radio",
                width,
                height.max(1),
            )
            .into_iter()
            .map(|s| Line::from(Span::styled(s, Style::default().fg(theme.stale))))
            .collect::<Vec<_>>();
            f.render_widget(Paragraph::new(lines), inner);
            return;
        };
        let c = &followed.connection;
        let k = state
            .net
            .ble_connections
            .iter()
            .position(|g| g.connection.access_address() == c.access_address())
            .unwrap_or(0);
        let central = theme.series_color(k);

        let mut lines = parameter_lines(followed, state, width, theme);
        lines.extend(measured_lines(followed, state, width, theme));
        let body = height.saturating_sub(lines.len() + 2);
        let rows: Vec<(Vec<String>, Vec<Option<Color>>)> = items(c)
            .into_iter()
            .flat_map(|item| match item {
                Item::Event(e) => event_rows(e, c.encrypted_from(), central, theme),
                Item::OutOfView {
                    newest,
                    oldest,
                    count,
                } => vec![run_row(newest, oldest, count, theme)],
            })
            .skip(state.net.connection_view.first_visible)
            .take(body)
            .collect();
        let texts: Vec<Vec<String>> = rows.iter().map(|(t, _)| t.clone()).collect();
        let grown = grow_to_contents(COLUMNS, &texts, &[PDU]);
        let want = texts
            .iter()
            .map(|t| t[PDU].chars().count())
            .max()
            .unwrap_or(0);
        let columns = breathe(&widen(&grown, width, PDU, want), width, BREATHING);
        let fit = columns_that_fit(&columns, width);
        if height > lines.len() {
            lines.push(header(
                &columns,
                fit,
                Sort {
                    column: 0,
                    descending: true,
                },
                theme,
            ));
        }
        for (text, ink) in &rows {
            let mut cells = text.clone();
            if let Some(col) = columns.get(PDU) {
                let w = col.width;
                if cells[PDU].chars().count() > w && w > 0 {
                    cells[PDU] = cells[PDU].chars().take(w - 1).collect::<String>() + "…";
                }
            }
            let mut line = row(&columns, fit, &cells, false, theme);
            for (i, colour) in ink.iter().enumerate().take(fit) {
                if let (Some(colour), Some(span)) = (colour, line.spans.get_mut(1 + 2 * i)) {
                    span.style = span.style.fg(*colour);
                }
            }
            lines.push(line);
        }
        if height > lines.len() {
            lines.push(tally(c, width, theme));
        }
        lines.truncate(height);
        f.render_widget(Paragraph::new(lines), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::ble::connect::ConnectIndData;
    use crate::signal::ble::data::DataPdu;
    use crate::signal::ble::receive::DataTiming;
    use crate::state::fixture::draw;
    use std::time::Instant;

    const RATE: f64 = 20e6;

    fn pdu(llid: u8, payload: &[u8]) -> DataPdu {
        DataPdu {
            llid,
            nesn: false,
            sn: false,
            md: false,
            cte_info: None,
            payload: payload.to_vec(),
            crc_ok: true,
        }
    }

    fn at(start: f64, payload_len: usize) -> DataTiming {
        let bits = 8 + 32 + 16 + payload_len * 8 + 24;
        DataTiming {
            start_pair: start,
            end_pair: start + bits as f64 * 1e-6 * RATE,
        }
    }

    /// A state following one connection through four events: one followed
    /// (the Central's LL_VERSION_IND, the Peripheral's empty PDU), one out
    /// of view, one missed, one lost to the feed.
    fn followed() -> SdrMetrics {
        let mut m = SdrMetrics::fixture().streaming();
        m.radio.frequency = 2_426_000_000;
        m.radio.config_sample_rate = RATE;
        m.radio.bb_filter_hz = 0;
        let c = ConnectIndData {
            init_a: [0x11, 0x22, 0x33, 0x44, 0x55, 0x66],
            adv_a: [0xa1, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6],
            access_address: 0x5065_4b6a,
            crc_init: 0x3a_5b7c,
            win_size: 1,
            win_offset: 0,
            interval: 6,
            latency: 0,
            timeout: 100,
            channel_map: (1u64 << 37) - 1,
            hop_increment: 7,
            sca: 0,
        };
        m.net
            .follow(&c, (Some(false), true, false), 0.0, RATE, Instant::now());
        let conn = &mut m.net.ble_connections[0].connection;
        let a = conn.next_due().anchor_pair;
        let version = at(a, 6);
        let empty = at(version.end_pair + 150e-6 * RATE, 0);
        conn.account(
            true,
            false,
            vec![
                (pdu(3, &[0x0c, 0x0c, 0x4c, 0x00, 0x34, 0x12]), version),
                (pdu(1, &[]), empty),
            ],
        );
        conn.account(false, false, Vec::new());
        conn.account(true, false, Vec::new());
        conn.account(true, true, Vec::new());
        m
    }

    #[test]
    fn a_followed_connection_lists_its_events() {
        let text = draw(NetBleConnectionPanel, 191, 20, &followed()).join("\n");
        for want in [
            "0x50654b6a",
            "CSA #1",
            "interval 7.50 ms",
            "8 in view (7-14)",
            "C→P",
            "P→C",
            "LL_VERSION_IND",
            "version 0x0c · company 0x004c",
            "empty",
            "followed",
            "not in view",
            "missed",
            "feed lost",
            "4 events · 3 in view · 1 followed · 1 missed · 1 feed lost",
        ] {
            assert!(text.contains(want), "{want}:\n{text}");
        }
    }

    /// MEASURED: the Central's clock once three anchors are heard, said as
    /// relative without a reference; T_IFS pooled and judged; CRC per
    /// channel heard.
    #[test]
    fn the_measured_section_reads_the_events() {
        let m = followed();
        let text = draw(NetBleConnectionPanel, 191, 24, &m).join("\n");
        for want in [
            "MEASURED",
            "collecting: 1 of 3 anchors",
            "T_IFS",
            "over 1 · 0 outside 150 ±2 us",
            "ch 7: 2 of 2",
        ] {
            assert!(text.contains(want), "{want}:\n{text}");
        }
        let mut m = followed();
        let conn = &mut m.net.ble_connections[0].connection;
        for _ in 0..4 {
            let a = conn.next_due().anchor_pair;
            conn.account(true, false, vec![(pdu(1, &[]), at(a, 0))]);
        }
        let text = draw(NetBleConnectionPanel, 191, 24, &m).join("\n");
        assert!(
            text.contains("ppm against this radio, relative · 5 anchors"),
            "{text}"
        );
    }

    /// A run of events out of view is one row, newest to oldest and how
    /// many; one alone keeps its own row and channel.
    #[test]
    fn a_run_out_of_view_is_one_row() {
        let mut m = followed();
        let conn = &mut m.net.ble_connections[0].connection;
        for _ in 0..3 {
            conn.account(false, false, Vec::new());
        }
        conn.account(true, false, Vec::new());
        let text = draw(NetBleConnectionPanel, 191, 24, &m).join("\n");
        assert!(text.contains("6-4"), "{text}");
        assert!(text.contains("not in view (3)"), "{text}");
        // Event 1, out of view alone between two in view, keeps its row.
        let one = text
            .lines()
            .find(|l| l.contains("not in view") && !l.contains('('))
            .unwrap();
        assert!(one.contains(" 1 "), "{one}");
        assert_eq!(row_count(&m.net.ble_connections[0].connection), 7);
    }

    /// Masked, the access address is the connection's number and the two
    /// device addresses are masked as everywhere else.
    #[test]
    fn masked_the_connection_is_a_number() {
        let mut m = followed();
        m.net.address_display = crate::state::AddressDisplay::Masked;
        let text = draw(NetBleConnectionPanel, 191, 20, &m).join("\n");
        assert!(text.contains("#1"), "{text}");
        assert!(!text.contains("50654b6a"), "{text}");
        assert!(!text.contains("11:22:33"), "{text}");
    }

    /// With none followed, the panel says so and what starts one.
    #[test]
    fn with_none_followed_it_says_what_starts_one() {
        let m = SdrMetrics::fixture().streaming();
        let text = draw(NetBleConnectionPanel, 120, 12, &m).join("\n");
        assert!(text.contains("no connection followed yet"), "{text}");
    }

    /// An ended connection says how it ended, in the title.
    #[test]
    fn an_ended_connection_says_how() {
        let mut m = followed();
        let conn = &mut m.net.ble_connections[0].connection;
        let a = conn.next_due().anchor_pair;
        conn.account(true, false, vec![(pdu(3, &[0x02, 0x13]), at(a, 2))]);
        let text = draw(NetBleConnectionPanel, 191, 20, &m).join("\n");
        assert!(
            text.contains("terminated: Remote User Terminated Connection (0x13)"),
            "{text}"
        );
    }

    #[test]
    fn it_fits_every_size() {
        let m = followed();
        for (w, h) in [(40u16, 10u16), (120, 20), (191, 27), (20, 5), (1, 1)] {
            for line in draw(NetBleConnectionPanel, w, h, &m) {
                assert!(line.chars().count() <= w as usize, "{w}x{h}: {line}");
            }
        }
    }
}
