// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `NetCensusPanel` - who is here.
//!
//! Design section 9.3 gives this preset one question and this is it: the
//! population of the band, one row per transmitter, ordered by whichever column
//! the user picked. The table itself is `ui::widgets::table`; what is here is
//! the column list, the empty state, and the keys.
//!
//! **An empty table says which of two empties it is.** "We listened and nobody
//! transmitted" and "nothing is listening" are different claims, and which one
//! holds now depends on whether a decoder is reading addresses: BLE has filled
//! this table since B10, and does so only while it has a channel. So the empty
//! state reads the same condition the feed-health panel dashes its BLE rows on,
//! and says either that the room was quiet or that nobody was counting.
//! Printing a bare empty table would let a reader take the flattering one.
//!
//! **Under the table, the room's clocks and then the selected device**
//! (`clock_error`, `detail`), in the order they give way: the table keeps its
//! rows first, the detail next, and the meters take what is left or are not
//! drawn.

use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::signal::ble::address::AddressKind;
use crate::signal::dsp::uncertainty::Uncertain;
use crate::signal::net::census::{Device, SORT_KEYS};
use crate::state::{RadioState, SdrMetrics};
use crate::ui::panel::{FeedSpan, Panel, PanelChrome, Staleness, Tag};
use crate::ui::widgets::reading::Reading;
use crate::ui::widgets::table::{
    columns_that_fit, header, row, viewport_start, widen, Align, Column, Sort,
};

mod clock_dial;
mod clock_error;

pub struct NetCensusPanel;

/// Rows the table keeps before the clock meters may take any: the panel is
/// the table, and the meters illustrate it.
const TABLE_KEEPS: usize = 5;

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
        title: "KIND",
        // `reserved`, the longest of `signal::ble::address::AddressKind`'s
        // labels.
        width: 8,
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
        title: "BEST SNR",
        width: 9,
        align: Align::Right,
    },
    Column {
        title: "CRC",
        width: 6,
        align: Align::Right,
    },
    Column {
        title: "CFO",
        // `-100.43 ±0.07 ppm`: three integer digits and a sign, which a
        // cheap crystal reaches (a live room had two at -95).
        width: 17,
        align: Align::Right,
    },
    Column {
        title: "MEAN SNR",
        // `-12.34 ±0.15 dB`.
        width: 15,
        align: Align::Right,
    },
    Column {
        title: "TYPES",
        width: 6,
        align: Align::Right,
    },
    Column {
        title: "MOD",
        // `0.4828 ±0.0015`: a tight uncertainty with a leading 1 earns
        // four places.
        width: 14,
        align: Align::Right,
    },
    Column {
        title: "INTERVAL",
        // `100.000 ±0.005 ms`: the estimate at the timebase's own floor.
        width: 17,
        align: Align::Right,
    },
];

/// `+15.4 ±0.5 ppm` - a device's own refined crystal-error estimate
/// ([`crate::signal::net::census::observe`]), corrected for our oscillator
/// when a reference allows, through the same value-with-uncertainty cell
/// every measurement in the app uses. What the number is worth is the
/// chrome's tag, not this cell's. ppm only, no kHz beside it: a device is
/// heard on three channels, and a kHz figure would have to pick one. `-`
/// before any packet from this device has reported one: an absent
/// measurement, not a zero-error clock.
fn fmt_cfo(offset: Option<Uncertain>, radio: &RadioState, now: std::time::Instant) -> String {
    match offset {
        Some(u) => Reading::new(radio.corrected_ppm(u, now).0, "ppm", f64::INFINITY).text(),
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

fn cells(
    d: &Device,
    state: &SdrMetrics,
    now: std::time::Instant,
    address_width: usize,
) -> Vec<String> {
    let radio = &state.radio;
    vec![
        d.address_text(&state.net, Some(address_width)),
        d.kind().label().to_string(),
        ago(now.saturating_duration_since(d.last_seen).as_secs()),
        d.packets.to_string(),
        fmt_best_snr(d),
        fmt_crc(d),
        fmt_cfo(d.crystal_offset_ppm, radio, now),
        fmt_mean_snr(d),
        d.ble_pdu_type_count().to_string(),
        fmt_modulation(d),
        fmt_interval(d),
    ]
}

/// `100.02 ±0.03 ms`, the advertising interval where one has been read
/// (`signal::ble::interval`); `-` otherwise, and the detail block says why.
fn fmt_interval(d: &Device) -> String {
    match d.advertising() {
        Some(Ok(e)) => Reading::new(e.interval_s.scale(1e3), "ms", f64::INFINITY).text(),
        _ => "-".to_string(),
    }
}

/// The detail block's advertising timing (net-ux-polish-plan 4.5): the
/// interval and where it sits on the 0.625 ms grid, the random delay, and
/// what it was timed on; or, where there is no estimate, why not.
///
/// **In SURVEY the reading stays, marked as not updating.** Arrivals are
/// only ever recorded in LOCK (`signal::net::worker::census_from_ble`), so a
/// log that exists was measured the right way; leaving the survey does not
/// make it wrong, only old, and the line says so (rule 4). Without one,
/// SURVEY refuses as the plan put it: the interval needs LOCK on one channel.
fn advertising_lines(
    d: &Device,
    state: &SdrMetrics,
    iw: usize,
    theme: &crate::Theme,
) -> Vec<Line<'static>> {
    use crate::signal::ble::interval::{Delay, Grid, Refusal, ADV_DELAY_MAX_S, INTERVAL_STEP_S};
    let locked = state.net.mode == crate::state::NetMode::Lock;
    let dash = || "-".to_string();
    let why = |text: String| vec![noted("interval", dash(), &text, iw, theme)];
    let e = match d.advertising() {
        None if locked => return why("none of its advertising heard in this LOCK yet".to_string()),
        None => return why("needs LOCK on one channel".to_string()),
        Some(Err(Refusal::Collecting { have, need })) => {
            return why(format!("collecting: {have} of {need} events"))
        }
        Some(Err(Refusal::BelowMinimum(gap))) => {
            return why(format!(
                "packets {:.2} ms apart: faster than any legacy interval",
                gap * 1e3
            ))
        }
        Some(Err(Refusal::NoSingleEvents(_))) => {
            return why("no two consecutive events heard".to_string())
        }
        Some(Ok(e)) => e,
    };
    let step_ms = INTERVAL_STEP_S * 1e3;
    let grid = match e.grid {
        Grid::On(n) => format!("{n} × {step_ms} ms"),
        Grid::Off { by_s, .. } => {
            format!("off the {step_ms} ms grid by {:.2} ms", by_s.abs() * 1e3)
        }
        Grid::CannotTell => format!("too long to tell the {step_ms} ms grid from clock drift"),
    };
    let (delay, delay_note) = match e.delay {
        Delay::Absent { .. } => (
            "none".to_string(),
            format!(
                "no random delay; the specification asks for 0 to {:.0} ms",
                ADV_DELAY_MAX_S * 1e3
            ),
        ),
        Delay::Spread {
            width_s,
            uniform: true,
            ..
        } => (
            format!("{:.1} ms wide", width_s * 1e3),
            format!("of {:.0} allowed, a uniform draw", ADV_DELAY_MAX_S * 1e3),
        ),
        Delay::Spread {
            width_s,
            ks,
            critical,
            ..
        } => (
            format!("{:.1} ms wide", width_s * 1e3),
            format!("not a uniform draw (KS {ks:.2} over {critical:.2})"),
        ),
    };
    let channel = d.arrivals.as_ref().map(|a| a.channel).unwrap_or_default();
    vec![
        noted(
            "interval",
            Reading::new(e.interval_s.scale(1e3), "ms", f64::INFINITY).text(),
            &grid,
            iw,
            theme,
        ),
        noted("adv delay", delay, &delay_note, iw, theme),
        noted(
            "timed on",
            format!("ch {channel}, {} events", e.events),
            if locked { "" } else { "not updating in SURVEY" },
            iw,
            theme,
        ),
    ]
}

/// `12.3 dB`, or `-` before any packet reported an SNR.
fn fmt_best_snr(d: &Device) -> String {
    d.best_snr_db
        .map(|db| format!("{db:.1} dB"))
        .unwrap_or_else(|| "-".to_string())
}

/// `98 %`: the pass rate, rounded *down*, so a device that failed once in a
/// thousand never reads `100 %`. The rate is already a ceiling
/// (`census::Device::crc_pass_rate`); rounding it up as well would print a
/// perfect link nobody measured.
fn fmt_crc(d: &Device) -> String {
    format!("{:.0} %", (d.crc_pass_rate() * 100.0).floor())
}

/// `8.4 ±0.6 dB`, through the reading cell. The resolution is infinite
/// because no SNR difference is too small to show; what dashes it is a mean
/// from one packet, whose uncertainty is unknown (`Device::mean_snr_db`).
fn fmt_mean_snr(d: &Device) -> String {
    match d.mean_snr_db() {
        Some(u) => Reading::new(u, "dB", f64::INFINITY).text(),
        None => "-".to_string(),
    }
}

/// `0.50 ±0.01`, the index B8 measured, refined across packets. `-` until a
/// packet had the settled runs B8 needs.
fn fmt_modulation(d: &Device) -> String {
    match d.modulation_index {
        Some(u) => Reading::new(u, "", f64::INFINITY).text(),
        None => "-".to_string(),
    }
}

/// `ADV_IND, SCAN_RSP`: the PDU types the census kept as codes, named.
fn pdu_types(d: &Device) -> String {
    let names: Vec<String> = d
        .ble_pdu_codes()
        .map(|c| crate::signal::ble::pdu::PduType::from_bits(c).label())
        .collect();
    if names.is_empty() {
        "-".to_string()
    } else {
        names.join(", ")
    }
}

/// B13's own window for "per unit time": five minutes, long enough to see a
/// handful of rotations from a device turning its address over on the
/// specification's own cadence (roughly every 15 minutes) without the
/// figure jumping to zero every time nobody new has shown up in the last
/// few seconds.
const TURNOVER_WINDOW: std::time::Duration = std::time::Duration::from_secs(300);

/// B13's exit condition, on screen, split by kind (4.4): how many distinct
/// addresses have appeared per minute, and of which kind, so the line says
/// whether it is a rotation rate. `1.2 new/min: 0.8 resolvable private, 0.4
/// public` reads as devices here changing their addresses and one device
/// arriving; a bare `1.2` could be either. Still a measurement about the
/// protocol, not a claim that two addresses are one device (see
/// [`crate::signal::net::census::turnover_by_kind`]).
///
/// The kinds' full names where they fit, the table's abbreviations where they
/// do not, the total alone where neither does: a kind cut in half would read
/// as a different kind.
fn turnover_text(devices: &[Device], now: std::time::Instant, width: usize) -> String {
    let split = crate::signal::net::census::turnover_by_kind(devices, TURNOVER_WINDOW, now);
    let total: f64 = split.iter().map(|(_, r)| r).sum();
    let tail = " (last 5 min)";
    let candidates: Vec<String> = match split.as_slice() {
        [] => vec!["no new addresses in the last 5 min".to_string()],
        [(k, r)] => vec![
            format!("{r:.1} new/min, all {}{tail}", k.name()),
            format!("{r:.1} new/min, all {}{tail}", k.label()),
            format!("{r:.1} new/min{tail}"),
        ],
        many => {
            let list = |name: fn(AddressKind) -> &'static str| {
                many.iter()
                    .map(|(k, r)| format!("{r:.1} {}", name(*k)))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            vec![
                format!("{total:.1} new/min: {}{tail}", list(AddressKind::name)),
                format!("{total:.1} new/min: {}{tail}", list(AddressKind::label)),
                format!("{total:.1} new/min{tail}"),
            ]
        }
    };
    let last = candidates.last().cloned().unwrap_or_default();
    candidates
        .into_iter()
        .find(|c| c.chars().count() < width)
        .unwrap_or(last)
}

fn turnover_line(
    devices: &[Device],
    now: std::time::Instant,
    width: usize,
    theme: &crate::Theme,
) -> Line<'static> {
    Line::from(Span::styled(
        format!(" {}", turnover_text(devices, now, width)),
        Style::default().fg(theme.label),
    ))
}

/// The empty table's own account of itself: quiet room, or nobody counting.
///
/// **The same condition the feed-health panel dashes its BLE rows on**
/// (`decode_health`): a decoder that has a channel, or has fired this
/// session, is one that was listening, and only then is an empty list a
/// statement about the room. What became of the triggers that never reached
/// a row is that panel's account, not restated here (rule 6); this one says
/// where to read it.
fn empty_state(state: &SdrMetrics, width: usize, theme: &crate::Theme) -> Vec<Line<'static>> {
    let net = &state.net;
    let counting = net.counting_addresses();
    let (headline, body) = if counting {
        (
            "nothing heard yet",
            "the decoder is reading addresses and none has passed a CRC yet; \
             what happened to the triggers is on Feed Health"
                .to_string(),
        )
    } else {
        (
            "no census yet",
            match &net.ble_refused {
                Some(why) => format!(
                    "nothing is decoding addresses here ({why}), so nobody has \
                     been counted - this is not an empty room"
                ),
                None => "nothing is decoding addresses here, so nobody has been \
                         counted - this is not an empty room"
                    .to_string(),
            },
        )
    };
    // Indented one column, like the header row above it, so the empty state
    // lines up with the table it stands in for.
    let mut out = vec![Line::from(vec![
        Span::raw(" "),
        Span::styled(headline.to_string(), Style::default().fg(theme.stale)),
    ])];
    for row in crate::ui::chrome::wrap(&body, width.saturating_sub(1), 4) {
        out.push(Line::from(vec![
            Span::raw(" "),
            Span::styled(row, Style::default().fg(theme.label)),
        ]));
    }
    out
}

/// The label column of the detail block's fields, wide enough for the longest
/// of them (`first seen`) and the same on both halves of a two-up line.
const DETAIL_LABEL_W: usize = 11;

/// What the cursor is on, spelled out: the fields the table's columns cut
/// short, and the one thing the table cannot show at all - what the crystal
/// offset is worth.
///
/// **It says who as well as where.** In `Full` the table prints the address
/// and nothing else, so the holder is named here (`state::who`); in the other
/// modes the address as shown already carries it, and repeating it would be
/// the same fact twice on one screen.
///
/// What the device advertises belongs in this block too and is not here yet:
/// payloads arrive in Stop 5.9, and a placeholder for them would be a promise
/// (rule 2).
///
/// `with_crystal` is false while the clock dial is drawn above it
/// (`clock_dial`), which shows the same offset and its basis.
fn detail(
    d: &Device,
    state: &SdrMetrics,
    now: std::time::Instant,
    iw: usize,
    with_crystal: bool,
    theme: &crate::Theme,
) -> Vec<Line<'static>> {
    let net = &state.net;
    let shown = net.show_address(d.address, d.random, None);
    let identity = match net.address_display {
        crate::state::AddressDisplay::Full => {
            format!("{shown}  {}", crate::state::who(d.address, d.random))
        }
        _ => shown,
    };

    let mut out = vec![
        crate::ui::chrome::section("selected", "", iw, theme),
        Line::from(vec![
            Span::raw(" "),
            Span::styled(identity, Style::default().fg(theme.value)),
        ]),
    ];
    let seen = |at: std::time::Instant| {
        format!("{} ago", ago(now.saturating_duration_since(at).as_secs()))
    };
    out.extend(pairs(
        &[
            ("first seen", seen(d.first_seen)),
            ("packets", d.packets.to_string()),
            ("last seen", seen(d.last_seen)),
            ("best SNR", fmt_best_snr(d)),
            ("mean SNR", fmt_mean_snr(d)),
            ("mod index", fmt_modulation(d)),
        ],
        iw,
        theme,
    ));
    if with_crystal {
        out.push(crystal_line(d, state, now, iw, theme));
    }
    out.push(noted(
        "CRC",
        format!("{} passed, {} failed", fmt_crc(d), d.crc_failed),
        // Said where it fits: a failure is only ever credited to an address
        // that survived it, so the rate can only flatter.
        "a ceiling: a corrupted address is nobody's failure",
        iw,
        theme,
    ));
    out.extend(advertising_lines(d, state, iw, theme));
    for (i, row) in crate::ui::chrome::wrap(&pdu_types(d), iw.saturating_sub(DETAIL_LABEL_W + 1), 2)
        .into_iter()
        .enumerate()
    {
        out.push(Line::from(vec![
            crate::ui::chrome::field(if i == 0 { "PDU types" } else { "" }, DETAIL_LABEL_W, theme),
            Span::styled(row, Style::default().fg(theme.value)),
        ]));
    }
    out
}

/// `label  value  note`: a field whose value wants a sentence beside it,
/// with the sentence dropped whole where it does not fit whole - half a
/// sentence about provenance is worse than none.
fn noted(label: &str, value: String, note: &str, iw: usize, theme: &crate::Theme) -> Line<'static> {
    let used = DETAIL_LABEL_W + 1 + value.chars().count();
    let mut spans = vec![
        crate::ui::chrome::field(label, DETAIL_LABEL_W, theme),
        Span::styled(value, Style::default().fg(theme.value)),
    ];
    if !note.is_empty() && iw >= used + note.chars().count() + 2 {
        spans.push(Span::styled(
            format!("  {note}"),
            Style::default().fg(theme.label),
        ));
    }
    Line::from(spans)
}

/// The fields two to a line where the width allows it and one to a line where
/// it does not, in the order given: a detail block that grew a scroll bar on a
/// narrow terminal would be a list, and this is meant to be read at a glance.
fn pairs(fields: &[(&str, String)], iw: usize, theme: &crate::Theme) -> Vec<Line<'static>> {
    // Two columns of the same width, so the second half of every line starts
    // in the same place whatever the values are.
    let widest = fields
        .iter()
        .map(|(_, v)| v.chars().count() + 2)
        .max()
        .unwrap_or(0);
    let half = DETAIL_LABEL_W + 1 + widest;
    let per_line = if iw >= half * 2 { 2 } else { 1 };
    let mut out = Vec::new();
    for chunk in fields.chunks(per_line) {
        let mut spans = Vec::new();
        for (name, value) in chunk {
            spans.push(crate::ui::chrome::field(name, DETAIL_LABEL_W, theme));
            spans.push(Span::styled(
                format!("{value:<widest$}"),
                Style::default().fg(theme.value),
            ));
        }
        out.push(Line::from(spans));
    }
    out
}

/// `crystal  +35.4 ±0.5 ppm  relative to our own oscillator` - the offset and,
/// beside it, what it was measured against.
///
/// **The provenance names the source, where the chrome's tag names the
/// class.** `[RELATIVE]` on the frame says what every offset in the panel is
/// worth; this says which oscillator this one is a difference from, which is
/// the fact a reader needs to know whether the device's clock or ours is the
/// one that is out. A device no packet has reported an offset for dashes,
/// exactly as its cell in the table does.
fn crystal_line(
    d: &Device,
    state: &SdrMetrics,
    now: std::time::Instant,
    iw: usize,
    theme: &crate::Theme,
) -> Line<'static> {
    let basis = state.radio.offset_basis(now);
    let against = match (d.crystal_offset_ppm, basis.provenance) {
        (None, _) => "no packet from it has reported one".to_string(),
        (Some(_), crate::state::Provenance::Unreferenced) if basis.expired => {
            "relative to our own oscillator, the reference expired".to_string()
        }
        (Some(_), crate::state::Provenance::Unreferenced) => {
            "relative to our own oscillator".to_string()
        }
        (Some(_), _) => match state.radio.reference.as_ref() {
            Some(r) => format!("against {}", r.source),
            None => "relative to our own oscillator".to_string(),
        },
    };
    let value = fmt_cfo(d.crystal_offset_ppm, &state.radio, now);
    noted("crystal", value, &against, iw, theme)
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
            .shows_offsets()
            .shows_addresses()
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
        let census = &state.net.census;
        let now = std::time::Instant::now();

        let devices: Vec<Device> = census.ordered(now, &state.radio);
        // The address column takes what the terminal can spare, up to the
        // widest address on the list: a registrant's whole name on a wide
        // screen, cut and marked on a narrow one.
        let want = devices
            .iter()
            .map(|d| state.net.address_width(d.address, d.random))
            .max()
            .unwrap_or(0);
        let columns = widen(COLUMNS, width, 0, want);
        let fit = columns_that_fit(&columns, width);
        let addresses: Vec<[u8; 6]> = devices.iter().map(|d| d.address).collect();

        let mut lines = vec![header(
            &columns,
            fit,
            Sort {
                column: census.sort,
                descending: census.descending,
            },
            theme,
        )];

        if devices.is_empty() {
            lines.push(Line::from(""));
            lines.extend(empty_state(state, width, theme));
            f.render_widget(Paragraph::new(lines), inner);
            return;
        }

        // No selection highlights no row: a highlight on row zero that nobody
        // chose would claim a selection that does not exist.
        let cursor = census.selection.cursor(&addresses);
        let picked = cursor.and_then(|i| devices.get(i));
        // The detail block is a footnote to the table, so it gives way to it:
        // on a panel too short to hold both it and a couple of rows, the rows
        // win and the block is not drawn.
        let mut extra = picked
            .map(|d| detail(d, state, now, width, true, theme))
            .unwrap_or_default();
        let height = inner.height as usize;
        if height < extra.len() + 4 {
            extra.clear();
        }
        // The clock block gives way to both the table and the detail: it
        // takes what is left once the table has kept its rows, and draws
        // nothing rather than a squeezed block. A selected device gets the
        // dial where it fits and its one meter where it does not; the whole
        // room only when nothing is selected.
        let room = height.saturating_sub(2 + extra.len() + devices.len().min(TABLE_KEEPS));
        let view = picked
            .and_then(|d| clock_dial::view(&devices, d.address, state, now, width, room, theme));
        let (meters, dial) = match view {
            Some((block, dial)) => {
                // The dial carries the crystal offset and what it was
                // measured against, so the detail block does not say it
                // twice.
                if let (Some(d), false) = (picked, extra.is_empty()) {
                    extra = detail(d, state, now, width, false, theme);
                }
                (block, Some(dial))
            }
            None => (
                clock_error::lines(
                    &devices,
                    state,
                    now,
                    width,
                    room,
                    picked.map(|d| d.address),
                    theme,
                ),
                None,
            ),
        };
        // One row for the header and one for the turnover summary, plus
        // whatever the two blocks took, so the list gets the rest.
        let body = height.saturating_sub(2 + extra.len() + meters.len());
        let start = viewport_start(
            census.selection.first_visible,
            cursor.unwrap_or(0),
            devices.len(),
            body,
        );
        for (i, d) in devices.iter().enumerate().skip(start).take(body) {
            lines.push(row(
                &columns,
                fit,
                &cells(d, state, now, columns[0].width),
                Some(i) == cursor,
                theme,
            ));
        }
        lines.push(turnover_line(&devices, now, width, theme));
        // The first line under the block's section rule.
        let dial_row = lines.len() + 1;
        lines.extend(meters);
        lines.extend(extra);
        f.render_widget(Paragraph::new(lines), inner);
        // The dial goes over the space its text was indented to leave.
        if let Some(d) = dial {
            let area = Rect {
                x: inner.x + 1,
                y: inner.y + dial_row as u16,
                width: clock_dial::DIAL_COLS as u16,
                height: clock_dial::DIAL_ROWS as u16,
            }
            .intersection(inner);
            d.render(f, area, theme);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::net::census::Device;
    use crate::state::fixture::draw;
    use std::time::{Duration, Instant};

    /// `d` with `n` SNR readings of mean `mean` and sample spread `spread`,
    /// as the sums `census::observe` would have left.
    fn with_snr(d: Device, n: u64, mean: f64, spread: f64) -> Device {
        let k = n as f64;
        Device {
            snr_count: n,
            snr_sum: k * mean,
            snr_sum_sq: (k - 1.0) * spread * spread + k * mean * mean,
            ..d
        }
    }

    fn populated() -> SdrMetrics {
        let now = Instant::now();
        let mut m = SdrMetrics::fixture().streaming();
        m.net.census.devices = vec![
            // Everything measured: two PDU types, a modulation index, and 22
            // failures credited against 1204 good packets.
            with_snr(
                Device {
                    packets: 1_204,
                    best_snr_db: Some(12.3),
                    last_seen: now - Duration::from_secs(2),
                    crystal_offset_ppm: Some(Uncertain::from_sigma(35.4, 0.5)),
                    ble_pdu_types: 1 << 0x0 | 1 << 0x4,
                    modulation_index: Some(Uncertain::from_sigma(0.50, 0.01)),
                    crc_failed: 22,
                    ..Device::heard(
                        [0xa4, 0x83, 0xe7, 0x1c, 0x09, 0xbe],
                        false,
                        now - Duration::from_secs(600),
                    )
                },
                1_204,
                8.4,
                3.0,
            ),
            // Nearly nothing: one SNR reading, so a mean nobody can vouch
            // for, and none of the other measurements.
            with_snr(
                Device {
                    packets: 7,
                    best_snr_db: Some(2.4),
                    last_seen: now - Duration::from_secs(240),
                    ble_pdu_types: 1 << 0x2,
                    // Clearly inside the five-minute turnover window, not on
                    // its boundary: the panel calls `Instant::now()` again at
                    // render time, later than this fixture's own `now`, so a
                    // value exactly at the window's edge could land either
                    // side of it depending on how much time the test takes.
                    ..Device::heard(
                        [0xf0, 0x18, 0x98, 0x00, 0x11, 0x22],
                        false,
                        now - Duration::from_secs(250),
                    )
                },
                1,
                2.4,
                0.0,
            ),
            with_snr(
                Device {
                    packets: 96,
                    best_snr_db: Some(6.9),
                    last_seen: now - Duration::from_secs(31),
                    crystal_offset_ppm: Some(Uncertain::from_sigma(-5.0, 0.5)),
                    ble_pdu_types: 1 << 0x0,
                    crc_failed: 4,
                    ..Device::heard(
                        [0x00, 0x1a, 0x11, 0xaa, 0xbb, 0xcc],
                        false,
                        now - Duration::from_secs(90),
                    )
                },
                96,
                5.1,
                2.0,
            ),
        ];
        m
    }

    /// **An empty census says which of the two empties it is.**
    ///
    /// With nothing decoding addresses, "we listened and nobody transmitted"
    /// is not a claim this panel can make, so it makes the other one. A bare
    /// empty table would let a reader take the flattering one.
    #[test]
    fn an_empty_census_with_no_decoder_says_nobody_is_counting() {
        // Wide enough for the columns through CFO, the selection gutter
        // included.
        let out = draw(NetCensusPanel, 90, 10, &SdrMetrics::fixture().streaming()).join("\n");
        assert!(out.contains("no census yet"), "{out}");
        // Read as prose, so where the sentence wraps is not the test.
        let prose = out
            .lines()
            .map(|l| l.trim_matches(['│', ' ']))
            .collect::<Vec<_>>()
            .join(" ");
        assert!(prose.contains("this is not an empty room"), "{out}");
        // The columns are still shown, so the shape of the answer is visible.
        assert!(out.contains("ADDRESS"), "{out}");
        assert!(out.contains("SNR"), "{out}");
        assert!(out.contains("CFO"), "{out}");
    }

    /// With a decoder on a channel, the same empty table is the other claim:
    /// the room was listened to and stayed quiet. The two must not read the
    /// same, because they are not the same fact.
    #[test]
    fn an_empty_census_with_a_decoder_running_says_the_room_was_quiet() {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.ble_channel = Some(37);
        let out = draw(NetCensusPanel, 64, 10, &m).join("\n");
        assert!(out.contains("nothing heard yet"), "{out}");
        assert!(!out.contains("not an empty room"), "{out}");
        // Where the triggers went is the feed-health panel's account.
        assert!(out.contains("Feed Health"), "{out}");
    }

    /// A refused decoder says why in the same breath, rather than leaving the
    /// reader to find the refusal on another panel.
    #[test]
    fn a_refused_decoder_gives_its_reason_in_the_empty_state() {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.ble_refused = Some("2.0 Msps is below the 4 Msps BLE needs".to_string());
        let out = draw(NetCensusPanel, 64, 12, &m).join("\n");
        assert!(out.contains("no census yet"), "{out}");
        assert!(out.contains("2.0 Msps is below"), "{out}");
    }

    /// The CFO cell shows the value once a device has one, and dashes when it
    /// does not - an absent measurement, not a zero-error clock.
    #[test]
    fn cfo_shows_when_measured_and_dashes_when_not() {
        let out = draw(NetCensusPanel, 81, 10, &populated()).join("\n");
        assert!(
            out.contains("35.4 ±0.5 ppm"),
            "measured CFO should show: {out}"
        );
        assert!(
            out.contains("-5.0 ±0.5 ppm"),
            "a negative CFO should show: {out}"
        );
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
        assert!(
            out.contains("0.4 new/min, all public (last 5 min)"),
            "{out}"
        );
    }

    /// The chrome says how the table is ordered, so the answer does not depend
    /// on spotting a marker halfway across the header.
    #[test]
    fn the_chrome_says_what_orders_the_table() {
        let mut m = populated();
        m.net.census.sort = crate::signal::net::census::column("PKTS");
        m.net.census.descending = true;
        let out = draw(NetCensusPanel, 60, 10, &m);
        assert!(out[0].contains("\u{2193}PKTS"), "{}", out[0]);
        assert!(out[1].contains("PKTS\u{25be}"), "{}", out[1]);

        m.net.census.sort = crate::signal::net::census::column("ADDRESS");
        m.net.census.descending = false;
        let out = draw(NetCensusPanel, 60, 10, &m);
        assert!(out[0].contains("\u{2191}ADDRESS"), "{}", out[0]);
    }

    #[test]
    fn the_rows_come_out_in_the_order_the_state_asked_for() {
        let mut m = populated();
        m.net.census.sort = crate::signal::net::census::column("PKTS");
        m.net.census.descending = true;
        let out = draw(NetCensusPanel, 60, 10, &m).join("\n");
        let at = |s: &str| out.find(s).unwrap_or(usize::MAX);
        assert!(at("a4:83:e7") < at("00:1a:11"), "1204 before 96:\n{out}");
        assert!(at("00:1a:11") < at("f0:18:98"), "96 before 7:\n{out}");

        // Ascending by address is a different order, and the panel follows it.
        m.net.census.sort = crate::signal::net::census::column("ADDRESS");
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

        m.net.census.sort = crate::signal::net::census::column("PKTS");
        m.net.census.descending = true;
        let rows = draw(NetCensusPanel, 60, 10, &m);
        let picked = rows.iter().position(|l| l.contains("a4:83:e7")).unwrap();
        assert_eq!(marked(&rows), vec![picked], "the mark is on its row");

        m.net.census.sort = crate::signal::net::census::column("ADDRESS");
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

    /// Twenty devices, for the size sweep: a list longer than any panel.
    fn crowded() -> SdrMetrics {
        let now = Instant::now();
        let mut m = SdrMetrics::fixture().streaming();
        let offsets = [
            -3.1, -2.4, -1.9, -1.2, -0.8, -0.3, 0.2, 0.6, 1.1, 1.4, 2.0, 2.6, 3.3, 4.8, 7.5, 12.0,
            18.4, -22.0, 31.0, 9.0,
        ];
        m.net.census.devices = offsets
            .iter()
            .enumerate()
            .map(|(i, &ppm)| Device {
                packets: 10 + i as u64,
                crystal_offset_ppm: Some(Uncertain::from_sigma(
                    ppm,
                    if i == 19 { 6.0 } else { 0.4 + i as f64 * 0.05 },
                )),
                ..Device::heard([0x10, 0, 0, 0, 0, i as u8], false, now)
            })
            .collect();
        m
    }

    fn selected() -> SdrMetrics {
        let mut m = populated();
        m.net.census.selection.selected = Some([0xa4, 0x83, 0xe7, 0x1c, 0x09, 0xbe]);
        m
    }

    /// **The table cuts, the block spells out.** Every field the plan asked
    /// for is in it, the crystal offset with its uncertainty, and the holder
    /// named beside an address the table shows bare.
    #[test]
    fn the_detail_block_spells_out_the_selected_device() {
        let out = draw(NetCensusPanel, 76, 15, &selected()).join("\n");
        assert!(out.contains("SELECTED"), "{out}");
        assert!(out.contains("a4:83:e7:1c:09:be  Apple"), "{out}");
        assert!(out.contains("first seen 10 min ago"), "{out}");
        assert!(out.contains("last seen  2 s ago"), "{out}");
        assert!(out.contains("packets    1204"), "{out}");
        assert!(out.contains("best SNR   12.3 dB"), "{out}");
        assert!(out.contains("crystal    35.4 ±0.5 ppm"), "{out}");

        // Nothing selected, no block: it describes a choice, and there is none.
        let none = draw(NetCensusPanel, 76, 14, &populated()).join("\n");
        assert!(!none.contains("SELECTED"), "{none}");
    }

    /// The offset's provenance is what it was measured *against*, named: the
    /// chrome's tag says what class of claim it is, and this says whose
    /// oscillator the difference is from.
    #[test]
    fn the_detail_block_says_what_the_offset_was_measured_against() {
        let mut m = selected();
        let relative = draw(NetCensusPanel, 90, 15, &m).join("\n");
        assert!(
            relative.contains("relative to our own oscillator"),
            "{relative}"
        );

        m.radio.reference = Some(crate::state::FrequencyReference {
            ppm: 10.0,
            sigma_ppm: 0.1,
            provenance: crate::state::Provenance::Traceable,
            source: "WWV 10 MHz".to_string(),
            at: Instant::now(),
            efficiency: None,
        });
        let referenced = draw(NetCensusPanel, 90, 15, &m).join("\n");
        assert!(referenced.contains("against WWV 10 MHz"), "{referenced}");
        // And the number is the corrected one, the same arithmetic the CFO
        // column does: 35.4 read plus our own 10 ppm.
        assert!(referenced.contains("45.4"), "{referenced}");
    }

    /// A device no packet has reported an offset for says that, rather than
    /// leaving a dash whose reason the reader has to guess.
    #[test]
    fn a_device_with_no_offset_says_why_the_cell_is_a_dash() {
        let mut m = populated();
        m.net.census.selection.selected = Some([0xf0, 0x18, 0x98, 0x00, 0x11, 0x22]);
        let out = draw(NetCensusPanel, 90, 15, &m).join("\n");
        assert!(out.contains("no packet from it has reported one"), "{out}");
    }

    /// **The block is a footnote to the table and gives way to it.** On a
    /// panel too short for both, the rows are what the panel is for.
    #[test]
    fn the_detail_block_gives_way_when_there_is_no_room_for_the_rows() {
        let short = draw(NetCensusPanel, 76, 10, &selected()).join("\n");
        assert!(!short.contains("SELECTED"), "{short}");
        assert!(
            short.contains("a4:83:e7"),
            "the rows are still there:\n{short}"
        );

        let tall = draw(NetCensusPanel, 76, 15, &selected()).join("\n");
        assert!(tall.contains("SELECTED"), "{tall}");
    }

    /// The masked display mode is a promise about every address on screen, and
    /// the detail block keeps it: it shows what the mode shows and no more.
    #[test]
    fn the_detail_block_masks_when_the_section_masks() {
        let mut m = selected();
        m.net.address_display = crate::state::AddressDisplay::Masked;
        let out = draw(NetCensusPanel, 76, 15, &m).join("\n");
        assert!(out.contains("SELECTED"), "{out}");
        assert!(!out.contains("a4:83:e7"), "the address leaked:\n{out}");
    }

    /// **Measurement 17's record, on screen.** Every new column shows what
    /// the record holds, each in the form its uncertainty earns, on a
    /// terminal wide enough for all of them.
    #[test]
    fn the_new_columns_show_what_the_record_holds() {
        let out = draw(NetCensusPanel, 150, 10, &populated());
        for title in ["BEST SNR", "CRC", "MEAN SNR", "TYPES", "MOD"] {
            assert!(out[1].contains(title), "{title}: {}", out[1]);
        }
        let busiest = out.iter().find(|l| l.contains("a4:83:e7")).unwrap();
        assert!(busiest.contains("98 %"), "{busiest}");
        assert!(busiest.contains("8.40 ±0.09 dB"), "{busiest}");
        assert!(busiest.contains("0.500 ±0.010"), "{busiest}");
    }

    /// A mean from one packet dashes, rather than passing its one reading
    /// off as an average; a device with no modulation index dashes too.
    #[test]
    fn a_mean_from_one_packet_is_a_dash_not_a_number() {
        let out = draw(NetCensusPanel, 150, 10, &populated());
        let quiet = out.iter().find(|l| l.contains("f0:18:98")).unwrap();
        assert!(quiet.contains("— dB"), "{quiet}");
        assert!(!quiet.contains("2.40"), "{quiet}");
        assert!(quiet.trim_end_matches(['│', ' ']).ends_with('-'), "{quiet}");
    }

    /// One failure in a thousand is not a perfect link, and the column never
    /// rounds it into one.
    #[test]
    fn one_failure_in_a_thousand_never_reads_as_a_perfect_link() {
        let now = Instant::now();
        let d = Device {
            packets: 999,
            crc_failed: 1,
            ..Device::heard([1; 6], false, now)
        };
        assert_eq!(fmt_crc(&d), "99 %");
        let clean = Device {
            packets: 999,
            ..Device::heard([1; 6], false, now)
        };
        assert_eq!(fmt_crc(&clean), "100 %");
    }

    /// The detail block names the PDU types, gives the mean and the index
    /// with their uncertainties, and says which way the CRC rate can be
    /// wrong where there is room to say it.
    #[test]
    fn the_detail_block_carries_the_record() {
        let out = draw(NetCensusPanel, 120, 16, &selected()).join("\n");
        assert!(out.contains("PDU types  ADV_IND, SCAN_RSP"), "{out}");
        assert!(out.contains("mean SNR   8.40 ±0.09 dB"), "{out}");
        assert!(out.contains("mod index  0.500 ±0.010"), "{out}");
        assert!(out.contains("98 % passed, 22 failed"), "{out}");
        assert!(out.contains("a ceiling"), "{out}");
    }

    /// **No cell is cut.** The widest readings a live room produced (two
    /// crystals near -95 ppm, a modulation index whose uncertainty earned four
    /// places) printed as `-94.43 ±0.07 pp` and `0.4828 ±0.00` with the
    /// columns 4.2.b first gave them: the table widget cuts a cell wider than
    /// its column, and a cut reading reads as a different reading.
    #[test]
    fn the_widest_live_readings_fit_their_columns_whole() {
        let now = Instant::now();
        let mut m = SdrMetrics::fixture().streaming();
        m.net.census.devices = vec![with_snr(
            Device {
                packets: 11,
                best_snr_db: Some(-12.3),
                crystal_offset_ppm: Some(Uncertain::from_sigma(-100.43, 0.07)),
                modulation_index: Some(Uncertain::from_sigma(0.4828, 0.0015)),
                ..Device::heard([0x51, 0x7f, 0xa9, 0xca, 0xf7, 0x65], true, now)
            },
            11,
            -12.34,
            0.5,
        )];
        let out = draw(NetCensusPanel, 150, 8, &m).join("\n");
        for cell in ["-100.43 ±0.07 ppm", "0.4828 ±0.0015", "-12.34 ±0.15 dB"] {
            assert!(out.contains(cell), "{cell}:\n{out}");
        }
    }

    /// The seven clocks a live room gave on 2026-09-22, two of them near
    /// -95 ppm, with the Gree device selected.
    fn live_room() -> SdrMetrics {
        let now = Instant::now();
        let mut m = SdrMetrics::fixture().streaming();
        let live = [
            ([0x20, 0xc9, 0x70, 0x44, 0x40, 0x13], -0.32, 0.12),
            ([0x36, 0x9a, 0x90, 0xcd, 0x23, 0x00], -40.36, 0.04),
            ([0x50, 0x2c, 0xc6, 0xc2, 0xaf, 0x64], -9.30, 0.23),
            ([0x51, 0x7f, 0xa9, 0xca, 0xf7, 0x65], -94.43, 0.07),
            ([0x6c, 0x93, 0x70, 0x77, 0x66, 0xd7], -2.50, 0.11),
            ([0xb0, 0x99, 0xd7, 0x40, 0xb3, 0x8b], -1.32, 0.06),
            ([0xe7, 0xc1, 0xf2, 0xd3, 0x7b, 0x09], -95.59, 0.09),
        ];
        m.net.census.devices = live
            .iter()
            .map(|&(a, v, sigma)| Device {
                packets: 5,
                best_snr_db: Some(15.0),
                crystal_offset_ppm: Some(Uncertain::from_sigma(v, sigma)),
                ..Device::heard(a, false, now)
            })
            .collect();
        m.net.census.selection.selected = Some([0x50, 0x2c, 0xc6, 0xc2, 0xaf, 0x64]);
        m
    }

    /// `m` with nothing selected: the room's meters, not one clock's dial.
    fn unselected(mut m: SdrMetrics) -> SdrMetrics {
        m.net.census.selection.selected = None;
        m
    }

    fn referenced(mut m: SdrMetrics) -> SdrMetrics {
        m.radio.reference = Some(crate::state::FrequencyReference {
            ppm: 38.0,
            sigma_ppm: 0.3,
            provenance: crate::state::Provenance::Traceable,
            source: "WWV 10 MHz".to_string(),
            at: Instant::now(),
            efficiency: None,
        });
        m
    }

    /// The meter rows, top to bottom, as the addresses they are labelled with.
    fn meter_rows(out: &[String]) -> Vec<String> {
        out.iter()
            .filter(|l| l.contains('◄'))
            .map(|l| l.chars().skip(2).take(17).collect())
            .collect()
    }

    /// **With nothing selected, the room: worst clock first, every row
    /// labelled**, and each meter's reading beside it.
    #[test]
    fn the_clock_meters_rank_the_room_worst_first() {
        let out = draw(NetCensusPanel, 120, 30, &unselected(live_room()));
        let rows = meter_rows(&out);
        assert_eq!(rows.len(), 7, "{}", out.join("\n"));
        assert_eq!(rows[0], "e7:c1:f2:d3:7b:09");
        assert_eq!(rows[1], "51:7f:a9:ca:f7:65");
        assert_eq!(rows[6], "20:c9:70:44:40:13");
        let gree = out
            .iter()
            .find(|l| l.contains('◄') && l.contains("50:2c:c6"))
            .unwrap();
        assert!(gree.contains("-9.30 ±0.23 ppm"), "{gree}");
    }

    /// **Without a reference, no limit.** Every offset still carries our own
    /// oscillator's error, so no marks are drawn, and the rule says why.
    #[test]
    fn relative_meters_draw_no_limit_and_say_why() {
        let out = draw(NetCensusPanel, 120, 30, &unselected(live_room())).join("\n");
        assert!(out.contains("no limit without a reference"), "{out}");
        assert!(!out.contains('╎'), "{out}");
    }

    /// With one, the limit is marked on every track and labelled on the
    /// scale, and the rule names the specification's figure.
    #[test]
    fn referenced_meters_mark_the_limit_and_label_it() {
        let out = draw(
            NetCensusPanel,
            120,
            30,
            &referenced(unselected(live_room())),
        );
        let text = out.join("\n");
        assert!(text.contains("spec ±150 kHz"), "{text}");
        let meters: Vec<&String> = out.iter().filter(|l| l.contains('◄')).collect();
        assert!(meters.iter().all(|l| l.matches('╎').count() == 2), "{text}");
        assert!(text.contains("+60"), "{text}");
    }

    /// **The scale reads the meters above it.** Zero on the scale is in the
    /// column of every meter's `┃`, and the limit's label under its marks: a
    /// scale one column off reads a different number off every row.
    #[test]
    fn the_scale_sits_under_the_meters_it_labels() {
        let out = draw(
            NetCensusPanel,
            120,
            30,
            &referenced(unselected(live_room())),
        );
        let col = |l: &str, ch: char| l.chars().position(|c| c == ch);
        let meter = out.iter().find(|l| l.contains('┃')).unwrap();
        let scale = out
            .iter()
            .find(|l| l.trim_end_matches(['│', ' ']).ends_with(" ppm") && l.contains("+60"))
            .unwrap();
        let zero = scale
            .chars()
            .collect::<Vec<_>>()
            .windows(3)
            .position(|w| w == [' ', '0', ' '])
            .map(|p| p + 1);
        assert_eq!(zero, col(meter, '┃'), "{meter}\n{scale}");
        let right_mark = meter
            .chars()
            .enumerate()
            .filter(|(_, c)| *c == '╎')
            .map(|(i, _)| i)
            .last();
        assert_eq!(
            col(scale, '+').map(|p| p + 1),
            right_mark,
            "{meter}\n{scale}"
        );
    }

    /// What does not fit is counted, worst first so it is the better clocks,
    /// and the meters give way entirely on a panel too short for the table.
    #[test]
    fn the_meters_count_what_they_leave_off_and_give_way_to_the_table() {
        let out = draw(NetCensusPanel, 120, 17, &unselected(live_room())).join("\n");
        assert!(out.contains("more, better clocks"), "{out}");

        // A device without an offset is not a good clock, and is counted as
        // what it is.
        let some = draw(NetCensusPanel, 120, 30, &unselected(populated())).join("\n");
        assert!(some.contains("1 not measured yet"), "{some}");

        let short = draw(NetCensusPanel, 120, 12, &unselected(live_room())).join("\n");
        assert!(!short.contains("CLOCK ERROR"), "{short}");
        assert!(
            short.contains("e7:c1:f2"),
            "the table keeps its rows:\n{short}"
        );
    }

    /// Characters of the braille block: what the dial is drawn in.
    fn braille(out: &str) -> usize {
        out.chars()
            .filter(|c| ('\u{2801}'..='\u{28ff}').contains(c))
            .count()
    }

    /// **A selection gets its own clock, as a dial, and none of the room's
    /// meters.** Beside it the reading, the rank, the kHz on each advertising
    /// channel, the watch line and the basis; below it the detail block,
    /// without the crystal line the dial now carries.
    #[test]
    fn a_selected_clock_is_a_dial_and_only_its_own() {
        let out = draw(NetCensusPanel, 120, 34, &live_room()).join("\n");
        assert!(out.contains("CLOCK ERROR"), "{out}");
        assert!(braille(&out) > 50, "the dial is drawn:\n{out}");
        assert!(
            !out.contains('◄'),
            "no room meters beside a selection:\n{out}"
        );
        assert!(out.contains("±0.23 ppm"), "{out}");
        assert!(out.contains("4th worst of 7"), "{out}");
        assert!(out.contains("-22.3 · -22.6 · -23.1 kHz"), "{out}");
        assert!(
            out.contains("0.804 ±0.020 s a day slower than ours"),
            "{out}"
        );
        assert!(out.contains("no limit without a reference"), "{out}");
        assert!(out.contains("vs      our own oscillator"), "{out}");
        assert!(out.contains("SELECTED"), "{out}");
        assert!(!out.contains("crystal "), "said once, on the dial:\n{out}");
    }

    /// With a reference the dial judges: the worst clock loses its seconds as
    /// its own, is outside the limit by a stated margin, and names what it was
    /// measured against.
    #[test]
    fn a_referenced_dial_judges_against_the_spec() {
        let mut m = referenced(live_room());
        if let Some(r) = m.radio.reference.as_mut() {
            r.ppm = 0.0;
        }
        m.net.census.selection.selected = Some([0xe7, 0xc1, 0xf2, 0xd3, 0x7b, 0x09]);
        let out = draw(NetCensusPanel, 120, 34, &m).join("\n");
        assert!(out.contains("worst of 7"), "{out}");
        assert!(out.contains("loses 8.2"), "{out}");
        assert!(out.contains("outside by 33.1 ppm"), "{out}");
        assert!(out.contains("against WWV 10 MHz"), "{out}");
    }

    /// **No room for the dial: the selected device's one meter, never the
    /// room's.** The detail block keeps its crystal line, since nothing
    /// above it carries the basis.
    #[test]
    fn a_selection_without_room_for_the_dial_gets_its_one_meter() {
        let out = draw(NetCensusPanel, 92, 26, &live_room());
        let meters: Vec<&String> = out.iter().filter(|l| l.contains('◄')).collect();
        assert_eq!(meters.len(), 1, "{}", out.join("\n"));
        assert!(meters[0].contains("50:2c:c6:c2:af:64"), "{}", meters[0]);
        let text = out.join("\n");
        assert!(text.contains("selected ·"), "{text}");
        assert_eq!(braille(&text), 0, "{text}");
        assert!(text.contains("crystal "), "{text}");
    }

    /// A selected device no packet has given an offset says so, rather than
    /// falling back to everyone else's.
    #[test]
    fn a_selected_device_without_an_offset_says_so() {
        let mut m = populated();
        m.net.census.selection.selected = Some([0xf0, 0x18, 0x98, 0x00, 0x11, 0x22]);
        let out = draw(NetCensusPanel, 120, 30, &m).join("\n");
        assert!(
            out.contains("no packet has reported an offset yet"),
            "{out}"
        );
        assert!(!out.contains('◄'), "{out}");
    }

    /// **KIND is read from the address, and sorts.** A public address, a
    /// resolvable private one and a static one, each labelled as the table
    /// abbreviates it, and ordered public first.
    #[test]
    fn the_kind_column_names_each_address_and_sorts_by_it() {
        let now = Instant::now();
        let mut m = SdrMetrics::fixture().streaming();
        m.net.census.devices = vec![
            Device::heard([0x4a, 1, 2, 3, 4, 5], true, now),
            Device::heard([0xc7, 1, 2, 3, 4, 6], true, now),
            Device::heard([0xa4, 0x83, 0xe7, 3, 4, 7], false, now),
        ];
        m.net.census.sort = crate::signal::net::census::column("KIND");
        let out = draw(NetCensusPanel, 90, 10, &m);
        assert!(out[1].contains("KIND"), "{}", out[1]);
        let row = |tail: &str| out.iter().position(|l| l.contains(tail)).unwrap();
        assert!(out[row("04:07")].contains(" public "), "{}", out.join("\n"));
        assert!(out[row("04:05")].contains(" RPA "), "{}", out.join("\n"));
        assert!(out[row("04:06")].contains(" static "), "{}", out.join("\n"));
        assert!(row("04:07") < row("04:06") && row("04:06") < row("04:05"));
    }

    /// **The turnover line says which kind is turning over.** Full names
    /// where they fit, the table's abbreviations where they do not, the total
    /// alone where neither does, and plain words when nothing is new.
    #[test]
    fn the_turnover_line_splits_by_kind_and_narrows_whole() {
        let now = Instant::now();
        let ago = |s| now - Duration::from_secs(s);
        let devices = vec![
            Device::heard([0x40, 0, 0, 0, 0, 1], true, ago(10)),
            Device::heard([0x41, 0, 0, 0, 0, 2], true, ago(20)),
            Device::heard([0x11, 0, 0, 0, 0, 3], false, ago(30)),
        ];
        assert_eq!(
            turnover_text(&devices, now, 120),
            "0.6 new/min: 0.4 resolvable private, 0.2 public (last 5 min)"
        );
        assert_eq!(
            turnover_text(&devices, now, 50),
            "0.6 new/min: 0.4 RPA, 0.2 public (last 5 min)"
        );
        assert_eq!(turnover_text(&devices, now, 30), "0.6 new/min (last 5 min)");

        let rpa_only = &devices[..2];
        assert_eq!(
            turnover_text(rpa_only, now, 120),
            "0.4 new/min, all resolvable private (last 5 min)"
        );
        let old = vec![Device::heard([0x40, 0, 0, 0, 0, 1], true, ago(900))];
        assert_eq!(
            turnover_text(&old, now, 120),
            "no new addresses in the last 5 min"
        );
    }

    /// One device, selected, heard `events` times on channel 37 every
    /// `interval_s` plus a delay spread evenly over `delay_s`, in `mode`.
    fn advertiser(
        interval_s: f64,
        delay_s: f64,
        events: u64,
        mode: crate::state::NetMode,
    ) -> SdrMetrics {
        use crate::signal::net::census::{observe, Arrival, Sighting};
        let now = Instant::now();
        let mut m = SdrMetrics::fixture().streaming();
        m.net.mode = mode;
        let addr = [0x4a, 1, 2, 3, 4, 5];
        let mut t = 1.0;
        for k in 0..events {
            t += interval_s + (k * 7919 % 1000) as f64 / 1000.0 * delay_s;
            let s = Sighting {
                address: addr,
                random: true,
                snr_db: Some(12.0),
                crystal_offset_ppm: None,
                ble_pdu_code: Some(0x0),
                modulation_index: None,
                arrival: Some(Arrival {
                    channel: 37,
                    rate_hz: 8e6,
                    pair: (t * 8e6) as u64,
                }),
            };
            observe(&mut m.net.census.devices, &s, now);
        }
        m.net.census.selection.selected = Some(addr);
        m
    }

    /// **In LOCK, the interval, its place on the 0.625 ms grid, the delay and
    /// what it was timed on**, and the INTERVAL column carrying the same
    /// reading.
    #[test]
    fn a_locked_advertiser_shows_its_interval_and_delay() {
        // A full log: the grid is judged only once the interval is known to a
        // small fraction of a step (`census::ARRIVALS_KEPT`).
        let m = advertiser(0.100, 10e-3, 300, crate::state::NetMode::Lock);
        let out = draw(NetCensusPanel, 150, 34, &m).join("\n");
        assert!(out.contains("interval   100."), "{out}");
        assert!(out.contains("160 × 0.625 ms"), "{out}");
        assert!(out.contains("adv delay  10.0 ms wide"), "{out}");
        assert!(out.contains("of 10 allowed, a uniform draw"), "{out}");
        assert!(out.contains("timed on   ch 37, 255 events"), "{out}");
        assert!(!out.contains("not updating"), "{out}");
        assert!(out.contains("INTERVAL"), "{out}");
        let row = out
            .lines()
            .find(|l| l.contains("4a:01:02") && l.contains("RPA"))
            .unwrap();
        assert!(row.contains(" ms"), "the column carries it: {row}");
    }

    /// Back in SURVEY the reading stays, marked as not updating: it was taken
    /// in LOCK and is old, not wrong.
    #[test]
    fn a_reading_from_an_earlier_lock_is_kept_and_marked_in_survey() {
        let m = advertiser(0.100, 10e-3, 300, crate::state::NetMode::Survey);
        let out = draw(NetCensusPanel, 150, 34, &m).join("\n");
        assert!(out.contains("interval   100."), "{out}");
        assert!(out.contains("not updating in SURVEY"), "{out}");
    }

    /// **What cannot be read says why**: no LOCK yet, a LOCK that has not
    /// heard it, too few events, and a device that adds no random delay.
    #[test]
    fn the_interval_line_says_why_when_it_has_no_reading() {
        use crate::state::NetMode;
        let mut m = advertiser(0.100, 10e-3, 0, NetMode::Survey);
        m.net.census.devices = vec![Device::heard([0x4a, 1, 2, 3, 4, 5], true, Instant::now())];
        let text = |m: &SdrMetrics| draw(NetCensusPanel, 150, 34, m).join("\n");
        assert!(
            text(&m).contains("needs LOCK on one channel"),
            "{}",
            text(&m)
        );
        m.net.mode = NetMode::Lock;
        assert!(text(&m).contains("none of its advertising heard in this LOCK yet"));

        let few = advertiser(0.100, 10e-3, 4, NetMode::Lock);
        assert!(
            text(&few).contains("collecting: 3 of 8 events"),
            "{}",
            text(&few)
        );

        let fixed = advertiser(0.100, 0.0, 30, NetMode::Lock);
        let out = text(&fixed);
        assert!(out.contains("adv delay  none"), "{out}");
        assert!(out.contains("no random delay"), "{out}");
    }

    #[test]
    fn it_fits_every_size_the_layout_can_hand_it() {
        for w in 20..90u16 {
            for h in 4..20u16 {
                for m in [
                    populated(),
                    selected(),
                    crowded(),
                    live_room(),
                    referenced(live_room()),
                    SdrMetrics::fixture(),
                ] {
                    for line in draw(NetCensusPanel, w, h, &m) {
                        assert!(line.chars().count() <= w as usize, "{w}x{h}: {line:?}");
                    }
                }
            }
        }
    }
}
