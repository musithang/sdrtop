// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `NetBtPacketsPanel` - one classic piconet, packet by packet: the
//! Piconet view's list, newest first.
//!
//! **Which piconet.** The one selected in the Classic view: the selection
//! (`NetState::bt_view`) is the section's, so choosing a piconet there and
//! opening this view is one gesture. With none heard, or none chosen, the
//! list says which of those it is and draws no table.
//!
//! **Who sent it.** A header read at one clock tells the slot's parity, and
//! with it the sender (`piconet::Direction`): `M ▶` the master in the
//! piconet's own colour, `◀ S` the slave. A packet before the clock was
//! known, or an ID with no header at all, gets a dot, never a guess, and the
//! last line counts it apart from both sides (rule 2).
//!
//! **Each packet's own figures.** The index and f0 are one header's
//! readings, noisier than the bench's pooled ones, and printed to the
//! places their own uncertainty allows, dashed where it allows none. A
//! value outside its BR limit wears the limit's colour, as on the bench;
//! f0 is judged only once a reference makes it absolute. The slot residual
//! is against the piconet's fitted grid, and only for a packet on the
//! stream the grid was fitted on: on another, its time is on another clock.
//!
//! **The link's rhythm.** CLK is the CLK1-6 the header was read at, the
//! piconet's clock as sdrtop followed it; ΔSLOT the slots since the packet
//! before on the same stream, so a master and slave taking turns read
//! `+1 +1 +1`, a three-slot packet is followed by `+3`, and a quiet spell
//! shows as a jump. Both are read straight off the stream's own clock.
//!
//! **The payload's verdict in exact words.** `✗ CRC` says the check failed
//! and nothing about why: an encrypted payload and a damaged capture fail
//! alike, and the air alone cannot tell them apart. A payload left
//! unchecked says why (`payload::Unchecked`), and never "PSK": whether a
//! link has gone EDR is not something one packet can show.
//!
//! **What a passing payload carries.** The LMP column reads a DM1 whose
//! LLID is 0b11 as the link manager's message, by its Core name and with
//! its parameters in words (`signal::bt::lmp`); nothing is read from a
//! payload whose CRC failed. `M:` or `S:` before the name is the message's
//! transaction ID, who began the exchange, which is not DIR, who sent this
//! packet: the answer to a master's request is sent by the slave and still
//! reads `M:`. A message Table 5.1 allows one way only, seen going the
//! other way by its slot parity, says so in the warning colour: a check on
//! the direction reading itself, packet by packet. L2CAP payloads show
//! their length and no more, and LLID 3 in another type is named, not read.
//!
//! **A message's names follow `i`.** A device's name (`name_res`), a
//! BD_ADDR (`slot_offset`) and the body of an opcode Table 5.1 does not
//! list are shown as the address mode allows
//! (`NetState::show_lmp_identity`), as an advertised name and an address
//! are, so a masked screen is masked in this column too.

use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use super::bt_piconets::{silence, uap_text};
use super::sections::{
    counted, index_of, BR_INDEX, F0_LIMIT_KHZ, F0_RESOLUTION_KHZ, INDEX_RESOLUTION,
};
use crate::signal::bt::header::PacketType;
use crate::signal::bt::lmp::{Identifying, Initiator};
use crate::signal::bt::piconet::{
    ordered, BtPacket, Direction, HeaderRead, PayloadContent, PayloadVerdict, Piconet,
};
use crate::signal::dsp::uncertainty::Uncertain;
use crate::state::{Provenance, SdrMetrics};
use crate::ui::panel::{FeedSpan, Panel, PanelChrome, Staleness, Tag};
use crate::ui::widgets::limit::LimitRow;
use crate::ui::widgets::reading::Reading;
use crate::ui::widgets::table::{
    breathe, columns_that_fit, grow_to_contents, header, row, widen, Align, Column, Sort,
};

pub struct NetBtPacketsPanel;

const COLUMNS: &[Column] = &[
    Column {
        title: "AGE",
        width: 6,
        align: Align::Right,
    },
    Column {
        title: "CH",
        width: 2,
        align: Align::Right,
    },
    Column {
        title: "DIR",
        width: 3,
        align: Align::Left,
    },
    Column {
        // `DM3/2-DH3`: every reading of the type code (`PacketType::shown`).
        title: "TYPE",
        width: 9,
        align: Align::Left,
    },
    Column {
        title: "LT",
        width: 2,
        align: Align::Right,
    },
    Column {
        // FLOW, ARQN, SEQN, in the order Core 5.4 Vol 2 Part B 6.4 lists them.
        title: "F A S",
        width: 5,
        align: Align::Left,
    },
    Column {
        // CLK1-6, the clock the header was read at: the slot's parity in it
        // is what says who sent the packet.
        title: "CLK",
        width: 3,
        align: Align::Right,
    },
    Column {
        // Slots since the packet before it, on the same stream.
        title: "ΔSLOT",
        width: 5,
        align: Align::Right,
    },
    Column {
        title: "SLOT µs",
        width: 7,
        align: Align::Right,
    },
    Column {
        title: "MOD",
        width: 5,
        align: Align::Right,
    },
    Column {
        title: "f0 kHz",
        width: 6,
        align: Align::Right,
    },
    Column {
        title: "PAYLOAD",
        width: 7,
        align: Align::Left,
    },
    Column {
        // What a passing payload carries: the link manager's message by
        // its Core name, or what else it is. Sized by the panel, last, and
        // cut with an ellipsis where it runs out of room.
        title: "LMP",
        width: 12,
        align: Align::Left,
    },
];

const AGE: usize = 0;
const DIR: usize = 2;
const LT: usize = 4;
const FLAGS: usize = 5;
const CLK: usize = 6;
const MOD: usize = 9;
const F0: usize = 10;
const PAYLOAD: usize = 11;
const LMP: usize = 12;

/// The most a wide panel spaces its columns out by, beyond the one-column
/// gap: enough to read calmly, not so much that a row stops reading as one.
const BREATHING: usize = 2;

/// A dot for a field that does not apply to this packet, or is not known.
const NOT_KNOWN: &str = "·";

/// How long ago the worker took the packet. One decimal under ten seconds:
/// the time is the block's, not the packet's own, so finer would be a
/// precision the list does not have (the order and the slot residual come
/// from the stream's clock instead).
fn age(secs: f64) -> String {
    if secs < 10.0 {
        format!("{secs:.1} s")
    } else if secs < 90.0 {
        format!("{} s", secs as u64)
    } else {
        format!("{} min", secs as u64 / 60)
    }
}

/// A reading's value alone, signed where the sign is the point, or the
/// house dash where its uncertainty cannot support it.
fn value_cell(reading: &Reading, signed: bool) -> String {
    match reading.value_text() {
        Some(t) if signed && !t.starts_with('-') && t.parse::<f64>().is_ok_and(|v| v != 0.0) => {
            format!("+{t}")
        }
        Some(t) => t,
        None => "—".to_string(),
    }
}

/// What a packet's payload carries, in the LMP column's words, and the
/// colour the cell wears where it is not the row's ordinary ink. An LMP
/// message leads with its transaction ID, `M:` or `S:`, who began the
/// exchange, which DIR does not say: an answer is sent by the other side.
/// One Table 5.1 forbids in the direction it was sent says so, and the
/// whole cell wears the warning colour.
pub(super) fn content_cell(
    k: &BtPacket,
    net: &crate::state::NetState,
    theme: &crate::Theme,
) -> (String, Option<Color>) {
    match &k.content {
        Some(PayloadContent::Lmp(m)) => {
            let tid = match m.initiator {
                Initiator::Central => "M",
                Initiator::Peripheral => "S",
            };
            let text = format!("{tid}: {}", m.words(|i| net.show_lmp_identity(i)));
            match k.direction.and_then(|d| m.against(d)) {
                Some(against) => (format!("{text}  ({against})"), Some(theme.status_warn)),
                None => (text, None),
            }
        }
        Some(PayloadContent::LmpElsewhere(t)) => (format!("LLID 3 in a {}", t.shown()), None),
        Some(PayloadContent::L2cap { start, bytes }) => (
            format!(
                "L2CAP {}, {bytes} bytes",
                if *start { "start" } else { "continuation" }
            ),
            Some(theme.label),
        ),
        Some(PayloadContent::Reserved) => ("LLID 0".to_string(), Some(theme.label)),
        None => (String::new(), None),
    }
}

/// `text` cut to `width` characters, the last one an ellipsis where it had
/// to be cut.
fn cut(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let mut out: String = text.chars().take(width.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// One packet's cells, and the colour each wears where it is not the row's
/// ordinary ink.
fn cells(
    k: &BtPacket,
    before: Option<&BtPacket>,
    p: &Piconet,
    master: Color,
    state: &SdrMetrics,
    now: std::time::Instant,
    theme: &crate::Theme,
) -> (Vec<String>, Vec<Option<Color>>) {
    let quiet = Some(theme.stale);
    let mut ink = vec![None; COLUMNS.len()];
    let decoded = match k.header {
        Some(HeaderRead::Decoded(h)) => Some(h),
        _ => None,
    };
    let (dir, dir_ink) = match k.direction {
        Some(Direction::Master) => ("M ▶", Some(master)),
        Some(Direction::Slave) => ("◀ S", Some(theme.value)),
        None => (NOT_KNOWN, quiet),
    };
    ink[DIR] = dir_ink;
    let kind = match k.header {
        None => "ID".to_string(),
        Some(HeaderRead::Decoded(h)) => PacketType::from_code(h.packet_type.code()).shown(),
        // Captured, not read: the words the Piconets panel's HEADERS uses.
        Some(HeaderRead::Unresolved) => "(no UAP)".to_string(),
        Some(HeaderRead::Undecoded) => "(no clock)".to_string(),
    };
    let lt = decoded.map_or(NOT_KNOWN.to_string(), |h| h.lt_addr.to_string());
    let flags = decoded.map_or(format!("{NOT_KNOWN} {NOT_KNOWN} {NOT_KNOWN}"), |h| {
        format!("{} {} {}", h.flags & 1, h.flags >> 1 & 1, h.flags >> 2 & 1)
    });
    let clk = decoded.map_or(NOT_KNOWN.to_string(), |h| h.clk6.to_string());
    if decoded.is_none() {
        ink[LT] = quiet;
        ink[FLAGS] = quiet;
        ink[CLK] = quiet;
    }

    // Slots since the packet before, to the half slot inquiry and paging
    // keep; nothing to count from across a stream break, whose times are on
    // another clock.
    let gap = before
        .filter(|b| b.stream == k.stream)
        .map(|b| {
            let halves = ((k.at_us - b.at_us) / (crate::signal::bt::header::SLOT_US / 2.0)).round();
            if halves % 2.0 == 0.0 {
                format!("+{}", halves as i64 / 2)
            } else {
                format!("+{:.1}", halves / 2.0)
            }
        })
        .unwrap_or_default();

    // The packet's own index, against the BR band.
    let index = index_of(&k.deviation);
    let modulation = match index {
        Some(i) => {
            let row = LimitRow::new("", Reading::new(i, "", INDEX_RESOLUTION), BR_INDEX);
            ink[MOD] = row.flag_colour(theme);
            value_cell(&Reading::new(i, "", INDEX_RESOLUTION), false)
        }
        None => "—".to_string(),
    };
    if index.is_none() {
        ink[MOD] = quiet;
    }

    // Its f0 in kHz of its own channel, corrected by the reference where
    // there is one and judged only then, as the CARRIER section does.
    let f0 = k
        .f0_ppm
        .zip(crate::signal::bt::channel::centre_hz(k.channel));
    let f0_cell = match f0 {
        Some((ppm, hz)) => {
            let (ppm, provenance) = state.radio.corrected_ppm(ppm, now);
            let khz: Uncertain = ppm.scale(hz as f64 / 1e9);
            let reading = Reading::new(khz, "kHz", F0_RESOLUTION_KHZ);
            if provenance != Provenance::Unreferenced {
                ink[F0] = LimitRow::new(
                    "",
                    Reading::new(khz, "kHz", F0_RESOLUTION_KHZ),
                    F0_LIMIT_KHZ,
                )
                .flag_colour(theme);
            }
            value_cell(&reading, true)
        }
        None => {
            ink[F0] = quiet;
            "—".to_string()
        }
    };

    // Against the grid fitted on this packet's own stream, or not at all.
    let slot = p
        .slots
        .as_ref()
        .and_then(|s| s.as_ref().ok())
        .filter(|_| p.slots_stream == k.stream)
        .and_then(|fit| fit.residual_at(k.at_us))
        .map(|r| {
            let r = if r.abs() < 0.05 { 0.0 } else { r };
            if r > 0.0 {
                format!("+{r:.1}")
            } else {
                format!("{r:.1}")
            }
        })
        .unwrap_or_default();

    let (payload, payload_ink) = match k.payload {
        PayloadVerdict::NoPayload => ("—".to_string(), theme.stale),
        PayloadVerdict::Crc(true) => ("✓ CRC".to_string(), theme.status_ok),
        PayloadVerdict::Crc(false) => ("✗ CRC".to_string(), theme.status_warn),
        PayloadVerdict::NotRead(why) => (format!("{NOT_KNOWN} {why}"), theme.label),
    };
    ink[PAYLOAD] = Some(payload_ink);
    ink[AGE] = Some(theme.label);
    let (content, content_ink) = content_cell(k, &state.net, theme);
    ink[LMP] = content_ink;

    let secs = now.saturating_duration_since(k.seen).as_secs_f64();
    (
        vec![
            age(secs),
            k.channel.to_string(),
            dir.to_string(),
            kind,
            lt,
            flags,
            clk,
            gap,
            slot,
            modulation,
            f0_cell,
            payload,
            content,
        ],
        ink,
    )
}

/// The last line: the session's packets by side, the unplaced ones named
/// for what they are. As long as the width allows, then shorter.
fn tally(p: &Piconet, width: usize, theme: &crate::Theme) -> Line<'static> {
    let s = &p.headers.sides;
    let total = s.master.packets + s.slave.packets + s.unknown.packets;
    let counts = format!(
        " {total} packets · {} master · {} slave · {} not yet placed",
        s.master.packets, s.slave.packets, s.unknown.packets
    );
    let why = ": ID, or before the clock was known";
    let text = if counts.chars().count() + why.chars().count() <= width {
        format!("{counts}{why}")
    } else {
        counts
    };
    Line::from(Span::styled(text, Style::default().fg(theme.label)))
}

/// The list the view shows: the piconet's ring of every packet, or its LMP
/// log (`l`). The keys and the panel both read it from here, so they move
/// through the same list.
/// Whether a packet's LMP message carries a BD_ADDR, the one device
/// address this list prints.
fn carries_address(k: &BtPacket) -> bool {
    matches!(
        &k.content,
        Some(PayloadContent::Lmp(m)) if matches!(m.identifying, Some(Identifying::Address(_)))
    )
}

pub(crate) fn shown<'a>(
    p: &'a Piconet,
    view: &crate::state::PacketsView,
) -> &'a std::collections::VecDeque<BtPacket> {
    if view.lmp_only {
        &p.lmp
    } else {
        &p.packets
    }
}

/// Where the list starts in what it shows: the newest packet while live,
/// the held one while held, which is also how many arrived since. `None`
/// when the held packet has left what is kept.
fn start(p: &Piconet, view: &crate::state::PacketsView) -> Option<usize> {
    match view.held {
        None => Some(0),
        Some((stream, at)) => shown(p, view)
            .iter()
            .position(|k| k.stream == stream && k.at_us == at),
    }
}

/// The LMP log's last line: every message read on the piconet, past what
/// the log keeps.
fn lmp_tally(p: &Piconet, theme: &crate::Theme) -> Line<'static> {
    Line::from(Span::styled(
        format!(
            " {} · {} kept",
            counted(p.lmp_heard, "LMP message"),
            p.lmp.len()
        ),
        Style::default().fg(theme.label),
    ))
}

/// What the link has done since its newest LMP message, as counts: payloads
/// checked and passed. No cause is named: an encrypted link and a damaged
/// capture both fail their CRC, and the air alone cannot tell them apart.
/// `None` when nothing has been checked since.
fn since_last(p: &Piconet) -> Option<String> {
    let s = p.since_lmp;
    (s.checked > 0).then(|| {
        let passed = match s.passed {
            0 => "none passing".to_string(),
            n => format!("{n} passing"),
        };
        format!(
            "since the last: {} checked, {passed}",
            counted(s.checked, "payload")
        )
    })
}

/// The selected piconet, if the roster still has it.
pub(super) fn selected(state: &SdrMetrics) -> Option<&Piconet> {
    let roster = ordered(&state.net.bt_piconets);
    let laps: Vec<u32> = roster.iter().map(|p| p.lap).collect();
    state.net.bt_view.cursor(&laps).map(|i| roster[i])
}

impl Panel for NetBtPacketsPanel {
    fn name(&self) -> &'static str {
        "net_bt_packets"
    }

    fn min_size(&self) -> (u16, u16) {
        (30, 6)
    }

    fn focus_key(&self) -> Option<char> {
        // The BLE packet list's letter: "the packets" is one key across NET,
        // and the two lists never share a screen (`app::FocusKeys`).
        Some('v')
    }

    fn focus_bindings(&self) -> &'static [(&'static str, &'static str)] {
        &[
            ("↑↓", "scroll, holding the list at its newest"),
            ("H", "hold the list, or let it run"),
            ("End", "back to live"),
            ("l", "the LMP messages only, or every packet"),
            ("← →", "the previous or next piconet"),
        ]
    }

    fn chrome(&self, state: &SdrMetrics) -> PanelChrome {
        let chrome = PanelChrome::new("Packets")
            .stale_when(Staleness::NotStreaming)
            .shows_laps()
            .shows_offsets()
            .tag_if(true, state.net.mode.tag())
            // The side counts run for the session.
            .counts_from_feed(FeedSpan::Session);
        let Some(p) = selected(state) else {
            return chrome;
        };
        let view = &state.net.packets_view;
        // Held, the list is paused by the user, drawn cooled and never as
        // stale, and says what the pause is costing.
        let behind = start(p, view).unwrap_or(shown(p, view).len()) as u64;
        // An address is printed here only inside an LMP message, so the
        // list earns the address tag by holding one, not by being this list.
        let mut chrome = chrome;
        chrome.addresses = shown(p, view).iter().any(carries_address);
        chrome
            .tag_if(view.held.is_some(), Tag::Paused)
            .tag_if(view.held.is_some() && behind > 0, Tag::Behind(behind))
            .tag_if(view.first_visible > 0, Tag::Scroll(view.first_visible))
            .suffix(format!(
                " {} · UAP {}{}",
                state.net.show_lap(p.lap),
                uap_text(p.lap, &state.net),
                if view.lmp_only { " · LMP only" } else { "" }
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
        if let Some(lines) = silence(state, width, theme) {
            f.render_widget(Paragraph::new(lines), inner);
            return;
        }
        let note = |text: &str| {
            crate::ui::chrome::wrap(text, width, 4)
                .into_iter()
                .map(|chunk| Line::from(Span::styled(chunk, Style::default().fg(theme.stale))))
                .collect::<Vec<_>>()
        };
        let Some(p) = selected(state) else {
            let lines = note("no piconet selected: select a piconet in Classic 1 and press Enter");
            f.render_widget(Paragraph::new(lines), inner);
            return;
        };
        let view = &state.net.packets_view;
        let list = shown(p, view);
        if list.is_empty() {
            let lines = note(if view.lmp_only {
                "no LMP read on this piconet yet: only a DM1 whose CRC passes is read, \
                 and an encrypted link shows its LMP only at a reconnect"
            } else {
                "no packets of this piconet kept yet"
            });
            f.render_widget(Paragraph::new(lines), inner);
            return;
        }
        let Some(top) = start(p, view) else {
            let kept = if view.lmp_only {
                crate::signal::bt::piconet::LMP_KEPT
            } else {
                crate::signal::bt::piconet::PACKETS_KEPT
            };
            let lines = note(&format!(
                "the held packet has left the {kept} kept; End: back to live"
            ));
            f.render_widget(Paragraph::new(lines), inner);
            return;
        };

        let now = std::time::Instant::now();
        // The master wears the piconet's own colour, its chip on the hop
        // chart and in the roster; the slave the ordinary ink, which no
        // piconet's colour is, so the two sides never look alike.
        let master = state
            .net
            .bt_piconets
            .iter()
            .position(|q| q.lap == p.lap)
            .map_or(theme.value_hi, |k| theme.series_color(k));
        let height = inner.height as usize;
        // As far as the keys scroll it: until the oldest kept is on top, so
        // every press moves the list and none is spent at its end.
        let rows = list.len() - top;
        let from = top + view.first_visible.min(rows.saturating_sub(1));
        // In the log, what the link did since its newest message, above that
        // message while it is the top row: the newest fact goes first, as
        // in the list.
        let since = (view.lmp_only && from == 0)
            .then(|| since_last(p))
            .flatten();
        let body = height.saturating_sub(2 + since.is_some() as usize);
        let visible: Vec<(Vec<String>, Vec<Option<Color>>)> = list
            .iter()
            .enumerate()
            .skip(from)
            .take(body)
            .map(|(i, k)| {
                // The packet before in the log is not the one before on the
                // air, so the log counts no slots between its rows.
                let before = if view.lmp_only { None } else { list.get(i + 1) };
                cells(k, before, p, master, state, now, theme)
            })
            .collect();
        let texts: Vec<Vec<String>> = visible.iter().map(|(c, _)| c.clone()).collect();
        // Every reading whole; the LMP column takes what is left, up to its
        // widest message, and the rest of the room spaces the others out.
        let widest = texts
            .iter()
            .filter_map(|t| t.get(LMP))
            .map(|t| t.chars().count())
            .max()
            .unwrap_or(0);
        let columns = grow_to_contents(COLUMNS, &texts, &[LMP]);
        let columns = breathe(&widen(&columns, width, LMP, widest), width, BREATHING);
        let fit = columns_that_fit(&columns, width);
        let mut visible = visible;
        for (text, _) in &mut visible {
            text[LMP] = cut(&text[LMP], columns[LMP].width);
        }
        let mut lines = vec![header(
            &columns,
            fit,
            Sort {
                column: 0,
                descending: false,
            },
            theme,
        )];
        if let Some(since) = since {
            lines.push(Line::from(Span::styled(
                format!(" {since}"),
                Style::default().fg(theme.label),
            )));
        }
        for (text, ink) in &visible {
            let mut line = row(&columns, fit, text, false, theme);
            // Cell `i` is span `1 + 2i`: the gutter first, a gap between.
            for (i, colour) in ink.iter().enumerate().take(fit) {
                if let (Some(colour), Some(span)) = (colour, line.spans.get_mut(1 + 2 * i)) {
                    span.style = span.style.fg(*colour);
                }
            }
            lines.push(line);
        }
        if height >= 3 {
            lines.push(if view.lmp_only {
                lmp_tally(p, theme)
            } else {
                tally(p, width, theme)
            });
        }
        f.render_widget(Paragraph::new(lines), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::bt::header::{Header, PacketType};
    use crate::signal::bt::piconet::{
        observe, observe_packet, BtPacket, Carrier, Deviation, Direction, HeaderRead,
        PayloadContent, PayloadVerdict, SinceLmp,
    };
    use crate::signal::dsp::uncertainty::Uncertain;
    use crate::state::fixture::draw;
    use std::time::{Duration, Instant};

    const LAP: u32 = 0xc3_d318;

    /// A piconet heard on 20 watched channels, its UAP resolved, selected.
    fn heard() -> SdrMetrics {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.bt_channels_watched = (60..80).collect();
        observe(&mut m.net.bt_piconets, LAP, 73, Instant::now());
        m.net.bt_uap.insert(LAP, vec![0x67]);
        m.net.bt_view.selected = Some(LAP);
        m
    }

    fn header(code: u8, lt_addr: u8, flags: u8) -> HeaderRead {
        HeaderRead::Decoded(Header {
            lt_addr,
            packet_type: PacketType::from_code(code),
            flags,
            hec: 0,
            clk6: 0,
        })
    }

    fn packet(
        slot: u32,
        header: Option<HeaderRead>,
        direction: Option<Direction>,
        payload: PayloadVerdict,
    ) -> BtPacket {
        BtPacket {
            seen: Instant::now() - Duration::from_millis(300),
            at_us: 1_000.0 + slot as f64 * 625.0,
            stream: 1,
            channel: 73,
            header,
            direction,
            deviation: Deviation::default(),
            carrier: Carrier::default(),
            f0_ppm: None,
            payload,
            content: None,
        }
    }

    /// Records the packets oldest first, as the worker does.
    fn record(m: &mut SdrMetrics, packets: Vec<BtPacket>) {
        for p in packets {
            observe_packet(&mut m.net.bt_piconets, LAP, p);
        }
    }

    /// The row a text lands on, after the header.
    fn row_of(out: &[String], text: &str) -> usize {
        out.iter()
            .skip(2)
            .position(|l| l.contains(text))
            .unwrap_or_else(|| panic!("{text}:\n{}", out.join("\n")))
    }

    /// With no piconet to show, the list says which silence it is: the
    /// section's three, or a roster with nothing chosen from it.
    #[test]
    fn no_piconet_names_the_silence() {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.bt_channels_watched = vec![10, 20];
        let out = draw(NetBtPacketsPanel, 80, 10, &m).join("\n");
        assert!(out.contains("no piconet heard"), "{out}");

        m.net.bt_refused = Some("no classic channel fits the view".to_string());
        let out = draw(NetBtPacketsPanel, 80, 10, &m).join("\n");
        assert!(out.contains("no classic channel fits"), "{out}");

        let mut m = heard();
        m.net.bt_view.selected = None;
        let out = draw(NetBtPacketsPanel, 80, 10, &m).join("\n");
        assert!(out.contains("select a piconet in Classic 1"), "{out}");

        // Selected, but aged out of the roster: the same, not a panic.
        m.net.bt_view.selected = Some(0x12_3456);
        let out = draw(NetBtPacketsPanel, 80, 10, &m).join("\n");
        assert!(out.contains("select a piconet in Classic 1"), "{out}");
    }

    /// Every column, newest first, each verdict in its own words, and the
    /// type code in every reading the header allows.
    #[test]
    fn a_resolved_piconet_lists_its_packets_newest_first() {
        let mut m = heard();
        record(
            &mut m,
            vec![
                packet(0, None, None, PayloadVerdict::NoPayload),
                packet(
                    2,
                    Some(header(1, 1, 0b101)),
                    Some(Direction::Master),
                    PayloadVerdict::NoPayload,
                ),
                packet(
                    3,
                    Some(header(0, 1, 0b011)),
                    Some(Direction::Slave),
                    PayloadVerdict::NoPayload,
                ),
                packet(
                    5,
                    Some(header(3, 1, 0b111)),
                    Some(Direction::Slave),
                    PayloadVerdict::Crc(true),
                ),
                packet(
                    6,
                    Some(header(4, 1, 0b001)),
                    Some(Direction::Master),
                    PayloadVerdict::Crc(false),
                ),
                packet(
                    8,
                    Some(header(10, 1, 0b001)),
                    Some(Direction::Master),
                    PayloadVerdict::NotRead("FEC failed"),
                ),
            ],
        );
        let out = draw(NetBtPacketsPanel, 200, 20, &m);
        let text = out.join("\n");
        let head = &out[1];
        let mut at = 0;
        for title in [
            "AGE", "CH", "DIR", "TYPE", "LT", "F A S", "CLK", "ΔSLOT", "SLOT µs", "MOD", "f0 kHz",
            "PAYLOAD",
        ] {
            let i = head[at..].find(title).map(|i| i + at);
            at = i.unwrap_or_else(|| panic!("{title} in order: {head}"));
        }
        let rows =
            ["DM3/2-DH3", "DH1/2-DH1", "DM1 ", "NULL", "POLL", " ID "].map(|t| row_of(&out, t));
        assert!(rows.windows(2).all(|w| w[0] < w[1]), "{rows:?}\n{text}");
        let line = |t: &str| &out[2 + row_of(&out, t)];
        assert!(line("POLL").contains("M ▶"), "{text}");
        assert!(line("POLL").contains("1 0 1"), "FLOW ARQN SEQN: {text}");
        assert!(line("NULL").contains("◀ S"), "{text}");
        assert!(line("POLL").contains('—'), "no payload: {text}");
        assert!(line("DM1 ").contains("✓ CRC"), "{text}");
        assert!(line("DH1/2-DH1").contains("✗ CRC"), "{text}");
        assert!(line("DM3/2-DH3").contains("· FEC failed"), "{text}");
        assert!(!text.contains("encrypt"), "a failed CRC claims no cause");
    }

    /// A packet's own index and f0 print to their own precision; a packet
    /// with no readings dashes them.
    #[test]
    fn a_packets_own_readings_are_shown_or_dashed() {
        let mut m = heard();
        let mut read = packet(
            2,
            Some(header(1, 1, 0)),
            Some(Direction::Master),
            PayloadVerdict::NoPayload,
        );
        read.deviation = Deviation::from_readings(&[158_000.0, 160_000.0, 162_000.0], &[]);
        read.f0_ppm = Some(Uncertain::from_sigma(1.3, 0.05));
        record(
            &mut m,
            vec![
                read,
                packet(
                    3,
                    Some(header(0, 1, 0)),
                    Some(Direction::Slave),
                    PayloadVerdict::NoPayload,
                ),
            ],
        );
        let out = draw(NetBtPacketsPanel, 200, 12, &m);
        let text = out.join("\n");
        let poll = &out[2 + row_of(&out, "POLL")];
        assert!(poll.contains("0.320"), "h = 2 * 160 kHz / 1 Msym/s: {text}");
        // 1.3 ppm of 2475 MHz, relative: no limit is judged, the value shown.
        assert!(poll.contains("+3.22"), "{text}");
        let null = &out[2 + row_of(&out, "NULL")];
        assert!(null.matches('—').count() >= 2, "{text}");
    }

    /// A packet whose direction is not known shows a dot, never a guess, and
    /// the last line counts it apart from both sides.
    #[test]
    fn an_unknown_direction_says_so() {
        let mut m = heard();
        record(
            &mut m,
            vec![
                packet(
                    0,
                    Some(HeaderRead::Unresolved),
                    None,
                    PayloadVerdict::NotRead("clock not known"),
                ),
                packet(
                    2,
                    Some(header(1, 1, 0)),
                    Some(Direction::Master),
                    PayloadVerdict::NoPayload,
                ),
            ],
        );
        // As the worker does: a header read moves the packet off `unknown`.
        let sides = &mut m.net.bt_piconets[0].headers.sides;
        sides.unknown.packets = 1;
        sides.master.packets = 1;
        let out = draw(NetBtPacketsPanel, 200, 12, &m);
        let text = out.join("\n");
        let row = &out[2 + row_of(&out, "UAP")];
        assert!(!row.contains('▶') && !row.contains('◀'), "{text}");
        let last = out.iter().rev().find(|l| l.contains("packets")).unwrap();
        assert!(last.contains("1 master"), "{last}");
        assert!(last.contains("0 slave"), "{last}");
        assert!(last.contains("before the clock was known"), "{last}");
    }

    /// A slot residual only against a grid fitted on the packet's own
    /// stream: on another, its time is on another clock.
    #[test]
    fn a_residual_from_another_stream_is_blank() {
        let mut m = heard();
        let times: Vec<f64> = [0u32, 2, 3, 5, 6, 8, 10, 11, 13, 15]
            .iter()
            .map(|&k| 1_000.0 + k as f64 * 625.0)
            .collect();
        let p = &mut m.net.bt_piconets[0];
        p.slots = Some(crate::signal::bt::slots::fit(&times));
        p.slots_stream = 1;
        let mut here = packet(
            2,
            Some(header(1, 1, 0)),
            Some(Direction::Master),
            PayloadVerdict::NoPayload,
        );
        here.at_us += 0.4;
        let mut elsewhere = packet(
            3,
            Some(header(0, 1, 0)),
            Some(Direction::Slave),
            PayloadVerdict::NoPayload,
        );
        elsewhere.stream = 2;
        record(&mut m, vec![here, elsewhere]);
        let out = draw(NetBtPacketsPanel, 200, 12, &m);
        let text = out.join("\n");
        let slot = out[1].find("SLOT µs").unwrap();
        let cell = |row: &str| -> String {
            row.chars()
                .skip(out[1][..slot].chars().count())
                .take(7)
                .collect::<String>()
        };
        assert!(
            cell(&out[2 + row_of(&out, "POLL")]).contains("+0.4"),
            "{text}"
        );
        assert_eq!(
            cell(&out[2 + row_of(&out, "NULL")]).trim(),
            "",
            "another stream's: {text}"
        );
    }

    /// Masked, the title names the piconet by its roster number, and its
    /// UAP not at all.
    #[test]
    fn masked_the_title_shows_numbers() {
        let mut m = heard();
        let out = draw(NetBtPacketsPanel, 100, 8, &m);
        assert!(out[0].contains("c3d318"), "{}", out[0]);
        assert!(out[0].contains("0x67"), "{}", out[0]);
        m.net.address_display = crate::state::AddressDisplay::Masked;
        let out = draw(NetBtPacketsPanel, 100, 8, &m);
        assert!(!out[0].contains("c3d318"), "{}", out[0]);
        assert!(!out[0].contains("0x67"), "{}", out[0]);
    }

    /// No size the layout can hand it panics, full or empty.
    #[test]
    fn it_fits_every_size() {
        let mut m = heard();
        record(
            &mut m,
            (0..40)
                .map(|k| {
                    packet(
                        k * 2,
                        Some(header(1, 1, 0)),
                        Some(Direction::Master),
                        PayloadVerdict::NoPayload,
                    )
                })
                .collect(),
        );
        for (w, h) in [(20, 5), (40, 8), (120, 30), (200, 50), (1, 1), (3, 2)] {
            draw(NetBtPacketsPanel, w, h, &m);
            draw(NetBtPacketsPanel, w, h, &heard());
            draw(NetBtPacketsPanel, w, h, &SdrMetrics::fixture());
        }
    }

    /// The master wears its piconet's colour and the slave the theme's
    /// ordinary ink, which no piconet's colour is, in any built-in theme:
    /// the two sides are never one colour.
    #[test]
    fn the_two_sides_never_share_a_colour() {
        for name in crate::Theme::builtin_names() {
            let theme = crate::Theme::by_name(name);
            for k in 0..theme.series.len() {
                assert_ne!(theme.series_color(k), theme.value, "{name}: series {k}");
            }
        }
        let m = heard();
        let theme = crate::Theme::sdr();
        let p = &m.net.bt_piconets[0];
        let master = theme.series_color(0);
        let slave_packet = packet(
            3,
            Some(header(0, 1, 0)),
            Some(Direction::Slave),
            PayloadVerdict::NoPayload,
        );
        let (_, ink) = cells(&slave_packet, None, p, master, &m, Instant::now(), &theme);
        assert_eq!(ink[DIR], Some(theme.value));
    }

    /// The clock a header was read at, CLK1-6: the piconet's own clock as
    /// sdrtop followed it, and a dot where no header was read.
    #[test]
    fn the_clock_a_header_was_read_at_is_shown() {
        let mut m = heard();
        let mut read = header(1, 1, 0);
        if let HeaderRead::Decoded(h) = &mut read {
            h.clk6 = 22;
        }
        record(
            &mut m,
            vec![
                packet(0, None, None, PayloadVerdict::NoPayload),
                packet(
                    2,
                    Some(read),
                    Some(Direction::Master),
                    PayloadVerdict::NoPayload,
                ),
            ],
        );
        let out = draw(NetBtPacketsPanel, 200, 12, &m);
        let text = out.join("\n");
        let col = out[1].find("CLK").expect(&text);
        let cell = |row: &str| -> String {
            row.chars()
                .skip(out[1][..col].chars().count())
                .take(3)
                .collect()
        };
        assert_eq!(cell(&out[2 + row_of(&out, "POLL")]).trim(), "22", "{text}");
        assert_eq!(cell(&out[2 + row_of(&out, " ID ")]).trim(), "·", "{text}");
    }

    /// Slots since the packet before it on the same stream: the master and
    /// slave taking turns read `+1`, a three-slot packet `+3`; the oldest
    /// kept, and the first after a stream break, have nothing to count from.
    #[test]
    fn the_gap_counts_slots_since_the_packet_before() {
        let mut m = heard();
        let mut after_break = packet(
            9,
            Some(header(0, 1, 0)),
            Some(Direction::Slave),
            PayloadVerdict::NoPayload,
        );
        after_break.stream = 2;
        let mut late = packet(
            4,
            Some(header(3, 1, 0)),
            Some(Direction::Master),
            PayloadVerdict::Crc(true),
        );
        // Dated a little off the grid, as a real capture is.
        late.at_us += 0.7;
        record(
            &mut m,
            vec![
                packet(
                    0,
                    Some(header(10, 1, 0)),
                    Some(Direction::Master),
                    PayloadVerdict::NotRead("FEC failed"),
                ),
                packet(
                    3,
                    Some(header(1, 1, 0)),
                    Some(Direction::Slave),
                    PayloadVerdict::NoPayload,
                ),
                late,
                after_break,
            ],
        );
        let out = draw(NetBtPacketsPanel, 200, 12, &m);
        let text = out.join("\n");
        let col = out[1].find("ΔSLOT").expect(&text);
        let cell = |t: &str| -> String {
            out[2 + row_of(&out, t)]
                .chars()
                .skip(out[1][..col].chars().count())
                .take(5)
                .collect::<String>()
                .trim()
                .to_string()
        };
        assert_eq!(cell("NULL"), "", "after a stream break: {text}");
        assert_eq!(cell("DM1 "), "+1", "{text}");
        assert_eq!(cell("POLL"), "+3", "{text}");
        assert_eq!(cell("DM3/2-DH3"), "", "the oldest kept: {text}");
    }

    /// Held, the list stays at its packet while newer ones arrive above,
    /// and says how many it is not showing; scrolled, it starts that many
    /// rows further down; a held packet gone from the ring is said, not a
    /// blank list.
    #[test]
    fn a_held_list_stays_at_its_packet_and_counts_what_arrived() {
        let mut m = heard();
        let numbered = |k: u32| {
            let mut p = packet(
                k,
                Some(header(1, (k % 8) as u8, 0)),
                Some(Direction::Master),
                PayloadVerdict::NoPayload,
            );
            p.channel = k as u8;
            p
        };
        record(&mut m, (0..10).map(numbered).collect());
        m.net.packets_view.held = Some((1, 1_000.0 + 9.0 * 625.0));
        record(&mut m, (10..13).map(numbered).collect());

        let out = draw(NetBtPacketsPanel, 120, 12, &m);
        let text = out.join("\n");
        assert!(out[0].contains("PAUSED"), "{}", out[0]);
        assert!(out[0].contains("+3 NEW"), "{}", out[0]);
        let first = &out[2];
        assert!(first.contains("  9 "), "the held packet on top: {text}");
        assert!(!text.contains(" 12 "), "a newer one shown: {text}");

        m.net.packets_view.first_visible = 2;
        let out = draw(NetBtPacketsPanel, 120, 12, &m);
        assert!(
            out[2].contains("  7 "),
            "two rows further: {}",
            out.join("\n")
        );

        // No further than the oldest kept.
        m.net.packets_view.first_visible = 50;
        let out = draw(NetBtPacketsPanel, 120, 12, &m);
        assert!(
            out[2].contains("  0 "),
            "the oldest on top: {}",
            out.join("\n")
        );

        m.net.packets_view.held = Some((1, 99.0));
        let out = draw(NetBtPacketsPanel, 120, 12, &m).join("\n");
        assert!(out.contains("End: back to live"), "{out}");
    }

    fn lmp(body: &[u8]) -> Option<PayloadContent> {
        Some(PayloadContent::Lmp(
            crate::signal::bt::lmp::parse(body).unwrap(),
        ))
    }

    /// A device's name, a BD_ADDR and a body no one can read follow `i`, as
    /// an advertised name and an address do: in full as sent, in oui the
    /// name and the address's "who", in masked none of them.
    #[test]
    fn lmp_identities_follow_the_address_mode() {
        use crate::state::AddressDisplay;
        let mut name = vec![2 << 1 | 1, 0, 22];
        name.extend(b"WH-1000XM4\0\0\0\0");
        let bodies: [&[u8]; 3] = [
            &name,
            // The piconet's own LAP, little-endian like the rest.
            &[52 << 1, 0x71, 0x02, 0x18, 0xd3, 0xc3, 0xe7, 0x83, 0xa4],
            &[67 << 1, 0xde, 0xad],
        ];
        let mut m = heard();
        // Numbered as it reached the state, after another device.
        m.net.address_book.number([9; 6]);
        m.net
            .address_book
            .number([0xa4, 0x83, 0xe7, 0xc3, 0xd3, 0x18]);
        record(
            &mut m,
            bodies
                .iter()
                .enumerate()
                .map(|(i, b)| {
                    let mut k = packet(
                        i as u32,
                        Some(header(3, 1, 0)),
                        Some(Direction::Master),
                        PayloadVerdict::Crc(true),
                    );
                    k.content = lmp(b);
                    k
                })
                .collect(),
        );
        let at = |m: &SdrMetrics| draw(NetBtPacketsPanel, 191, 12, m).join("\n");

        let text = at(&m);
        assert!(text.contains("\"WH-1000XM4\""), "{text}");
        assert!(text.contains("625 µs · a4:83:e7:c3:d3:18"), "{text}");
        assert!(text.contains("not in Table 5.1  de ad"), "{text}");

        m.net.address_display = AddressDisplay::Oui;
        let text = at(&m);
        assert!(text.contains("\"WH-1000XM4\""), "{text}");
        assert!(text.contains("..d3:18"), "{text}");
        assert!(!text.contains("c3:d3:18"), "{text}");
        // An address shown in part is said so on the title, as anywhere.
        let title = text.lines().next().unwrap_or_default();
        assert!(title.contains("OUI"), "{title}");

        m.net.address_display = AddressDisplay::Masked;
        let text = at(&m);
        assert!(!text.contains("WH-1000XM4"), "{text}");
        assert!(text.contains("10 of 22 bytes · name, 10 chars"), "{text}");
        assert!(!text.contains("d3:18"), "{text}");
        let row = text
            .lines()
            .find(|l| l.contains("slot_offset"))
            .expect(&text);
        assert!(row.contains("625 µs ·") && row.contains(" #2"), "{row}");
        assert!(text.contains("not in Table 5.1  2 bytes"), "{text}");
        assert!(!text.contains("de ad"), "{text}");
    }

    /// An LMP packet shows who began the exchange and the message; the
    /// packet's own sender stays in DIR.
    #[test]
    fn an_lmp_packet_shows_its_message() {
        let mut m = heard();
        let mut k = packet(
            1,
            Some(header(3, 1, 0)),
            Some(Direction::Slave),
            PayloadVerdict::Crc(true),
        );
        k.content = lmp(&[3 << 1, 15]);
        record(&mut m, vec![k]);
        let out = draw(NetBtPacketsPanel, 191, 20, &m);
        let row = out
            .iter()
            .find(|l| l.contains("accepted"))
            .expect("the LMP row");
        assert!(row.contains("◀ S"), "{row}");
        assert!(row.contains("M: accepted  encryption_mode_req"), "{row}");
        // Line 0 is the frame's title; the column titles are the first row in it.
        assert!(out[1].contains("LMP"), "the column's title: {}", out[1]);
    }

    /// A direction Table 5.1 forbids is named on the row.
    #[test]
    fn a_forbidden_direction_is_named() {
        let mut m = heard();
        let mut k = packet(
            1,
            Some(header(3, 1, 0)),
            Some(Direction::Slave),
            PayloadVerdict::Crc(true),
        );
        k.content = lmp(&[17 << 1; 17]);
        record(&mut m, vec![k]);
        let text = draw(NetBtPacketsPanel, 191, 20, &m).join("\n");
        assert!(text.contains("Table 5.1: C → P only"), "{text}");
    }

    /// L2CAP and LLID 3 outside a DM1 say what they are, nothing more.
    #[test]
    fn other_content_says_what_it_is() {
        let mut m = heard();
        let mut a = packet(
            1,
            Some(header(4, 1, 0)),
            Some(Direction::Master),
            PayloadVerdict::Crc(true),
        );
        a.content = Some(PayloadContent::L2cap {
            start: true,
            bytes: 12,
        });
        let mut b = packet(
            2,
            Some(header(4, 1, 0)),
            Some(Direction::Master),
            PayloadVerdict::Crc(true),
        );
        b.content = Some(PayloadContent::LmpElsewhere(PacketType::Dh1));
        record(&mut m, vec![a, b]);
        let text = draw(NetBtPacketsPanel, 191, 20, &m).join("\n");
        assert!(text.contains("L2CAP start, 12 bytes"), "{text}");
        assert!(text.contains("LLID 3 in a DH1"), "{text}");
    }

    /// Narrow, the message is cut with an ellipsis or left off whole, and
    /// no row is wider than the panel.
    #[test]
    fn a_narrow_list_cuts_the_message() {
        let mut m = heard();
        let mut k = packet(
            1,
            Some(header(3, 1, 0)),
            Some(Direction::Master),
            PayloadVerdict::Crc(true),
        );
        k.content = lmp(&[38 << 1 | 1, 0x0b, 0x1d, 0x00, 0x00, 0x21]);
        record(&mut m, vec![k]);
        for w in [40u16, 90, 110] {
            for line in draw(NetBtPacketsPanel, w, 12, &m) {
                assert!(line.chars().count() <= w as usize, "{w}: {line}");
            }
        }
        let text = draw(NetBtPacketsPanel, 110, 12, &m).join("\n");
        assert!(text.contains('…'), "{text}");
    }

    /// `l`'s view lists the LMP log only, old ones included, and closes
    /// with what the link has done since, claiming no cause for it.
    #[test]
    fn the_lmp_view_lists_the_log_and_what_came_after() {
        let mut m = heard();
        // One LMP packet, in the ring and in the log, and 412 payloads
        // checked since it without one passing.
        let mut k = packet(
            1,
            Some(header(3, 1, 0)),
            Some(Direction::Master),
            PayloadVerdict::Crc(true),
        );
        k.content = lmp(&[16 << 1, 16]);
        record(&mut m, vec![k.clone()]);
        let p = m.net.bt_piconets.iter_mut().find(|p| p.lap == LAP).unwrap();
        p.lmp.push_front(k);
        p.lmp_heard = 1;
        p.since_lmp = SinceLmp {
            checked: 412,
            passed: 0,
        };
        m.net.packets_view.lmp_only = true;
        let out = draw(NetBtPacketsPanel, 191, 20, &m);
        let text = out.join("\n");
        assert!(text.contains("encryption_key_size_req  16 bytes"), "{text}");
        let closing = out
            .iter()
            .find(|l| l.contains("since the last"))
            .unwrap_or_else(|| panic!("the closing line: {text}"));
        assert!(
            closing.contains("since the last: 412 payloads checked, none passing"),
            "{closing}"
        );
        assert!(
            !closing.contains("encrypt"),
            "no cause is claimed: {closing}"
        );
        assert!(
            out[0].contains("LMP only"),
            "the title says which: {}",
            out[0]
        );
        assert!(
            text.contains("1 LMP message · 1 kept"),
            "one is one message: {text}"
        );
    }

    /// One payload is one payload, not "1 payloads".
    #[test]
    fn one_payload_since_the_last_is_singular() {
        let mut m = heard();
        record(
            &mut m,
            vec![packet(1, None, None, PayloadVerdict::NoPayload)],
        );
        let p = m.net.bt_piconets.iter_mut().find(|p| p.lap == LAP).unwrap();
        p.since_lmp = SinceLmp {
            checked: 1,
            passed: 0,
        };
        assert_eq!(
            since_last(p).as_deref(),
            Some("since the last: 1 payload checked, none passing")
        );
    }

    #[test]
    fn an_empty_lmp_log_says_why() {
        let mut m = heard();
        record(
            &mut m,
            vec![packet(1, None, None, PayloadVerdict::NoPayload)],
        );
        m.net.packets_view.lmp_only = true;
        let text = draw(NetBtPacketsPanel, 120, 12, &m).join("\n");
        assert!(text.contains("no LMP read on this piconet yet"), "{text}");
    }
}
