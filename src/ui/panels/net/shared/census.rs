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
//! **Three blocks under the table, in the order they give way:** the table
//! itself, the selected device's detail, and the room's clocks (`clocks`),
//! each drawn only when the panel has room for it whole.

use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::signal::dsp::uncertainty::Uncertain;
use crate::signal::net::census::{Device, SORT_KEYS};
use crate::state::{RadioState, SdrMetrics};
use crate::ui::panel::{FeedSpan, Panel, PanelChrome, Staleness, Tag};
use crate::ui::widgets::reading::Reading;
use crate::ui::widgets::table::{
    columns_that_fit, header, row, viewport_start, widen, Align, Column, Sort,
};

mod clocks;

pub struct NetCensusPanel;

/// Rows the table keeps before the clocks picture may take any: enough to
/// read the room's top few, which is what the panel is for.
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
        width: 15,
        align: Align::Right,
    },
    Column {
        title: "MEAN SNR",
        width: 14,
        align: Align::Right,
    },
    Column {
        title: "TYPES",
        width: 6,
        align: Align::Right,
    },
    Column {
        title: "MOD",
        width: 12,
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
        ago(now.saturating_duration_since(d.last_seen).as_secs()),
        d.packets.to_string(),
        fmt_best_snr(d),
        fmt_crc(d),
        fmt_cfo(d.crystal_offset_ppm, radio, now),
        fmt_mean_snr(d),
        d.ble_pdu_type_count().to_string(),
        fmt_modulation(d),
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
    let counting = net.ble_channel.is_some() || net.health.ble.triggered > 0;
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
fn detail(
    d: &Device,
    state: &SdrMetrics,
    now: std::time::Instant,
    iw: usize,
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
    out.push(crystal_line(d, state, now, iw, theme));
    out.push(noted(
        "CRC",
        format!("{} passed, {} failed", fmt_crc(d), d.crc_failed),
        // Said where it fits: a failure is only ever credited to an address
        // that survived it, so the rate can only flatter.
        "a ceiling: a corrupted address is nobody's failure",
        iw,
        theme,
    ));
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
    if iw >= used + note.chars().count() + 2 {
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
        // The detail block is a footnote to the table, so it gives way to it:
        // on a panel too short to hold both it and a couple of rows, the rows
        // win and the block is not drawn.
        let mut extra = cursor
            .and_then(|i| devices.get(i))
            .map(|d| detail(d, state, now, width, theme))
            .unwrap_or_default();
        let height = inner.height as usize;
        if height < extra.len() + 4 {
            extra.clear();
        }
        // The clocks picture illustrates the table and gives way to both it
        // and the detail: it takes what is left once the table has kept a
        // handful of rows, and draws nothing rather than a squashed picture.
        let room = height.saturating_sub(2 + extra.len() + devices.len().min(TABLE_KEEPS));
        let picture = clocks::lines(&devices, state, now, width, room, theme);
        // One row for the header and one for the turnover summary, plus
        // whatever the two blocks took, so the list gets the rest.
        let body = height.saturating_sub(2 + extra.len() + picture.len());
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
        lines.push(turnover_line(&devices, now, theme));
        lines.extend(picture);
        lines.extend(extra);
        f.render_widget(Paragraph::new(lines), inner);
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
        let out = draw(NetCensusPanel, 70, 10, &SdrMetrics::fixture().streaming()).join("\n");
        assert!(out.contains("no census yet"), "{out}");
        assert!(out.contains("not an empty room"), "{out}");
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
        let out = draw(NetCensusPanel, 70, 10, &populated()).join("\n");
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

    /// Twenty clocks, bunched the way a real room bunches: most within a
    /// few ppm of each other, a tail of cheap crystals, one badly known.
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
        let out = draw(NetCensusPanel, 76, 14, &selected()).join("\n");
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
        let relative = draw(NetCensusPanel, 90, 14, &m).join("\n");
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
        let referenced = draw(NetCensusPanel, 90, 14, &m).join("\n");
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
        let out = draw(NetCensusPanel, 90, 14, &m).join("\n");
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

        let tall = draw(NetCensusPanel, 76, 14, &selected()).join("\n");
        assert!(tall.contains("SELECTED"), "{tall}");
    }

    /// The masked display mode is a promise about every address on screen, and
    /// the detail block keeps it: it shows what the mode shows and no more.
    #[test]
    fn the_detail_block_masks_when_the_section_masks() {
        let mut m = selected();
        m.net.address_display = crate::state::AddressDisplay::Masked;
        let out = draw(NetCensusPanel, 76, 14, &m).join("\n");
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

    /// A line that is the clocks ruler: zero on it, and the unit at its end.
    fn ruler_line(l: &str) -> bool {
        l.trim_end_matches(['│', ' ']).ends_with(" ppm") && l.contains(" 0 ")
    }

    /// **The room's clocks under the table**: how many are measured out of
    /// how many there are, and the selected one's bar drawn heavy.
    #[test]
    fn the_clocks_picture_shows_the_room_and_marks_the_selected_clock() {
        let out = draw(NetCensusPanel, 100, 30, &selected()).join("\n");
        assert!(out.contains("CLOCKS"), "{out}");
        assert!(out.contains("2 of 3 measured"), "{out}");
        assert!(out.contains("┣●┫"), "the selected bar, heavy:\n{out}");
        assert!(
            out.lines().any(ruler_line),
            "the ruler names its unit:\n{out}"
        );
        // The picture sits between the table and the detail block.
        assert!(out.find("CLOCKS") < out.find("SELECTED"), "{out}");
    }

    /// A census whose devices have reported no offset says so, rather than
    /// drawing an empty axis that would read as a room of perfect clocks.
    #[test]
    fn a_census_with_no_offsets_says_so_instead_of_drawing_an_empty_axis() {
        let mut m = populated();
        for d in &mut m.net.census.devices {
            d.crystal_offset_ppm = None;
        }
        let out = draw(NetCensusPanel, 90, 24, &m).join("\n");
        assert!(
            out.contains("no packet has reported an offset yet"),
            "{out}"
        );
        assert!(!out.lines().any(ruler_line), "{out}");
    }

    /// **The picture gives way to the table.** On a short panel it is not
    /// drawn at all and the rows keep the space.
    #[test]
    fn the_clocks_picture_gives_way_to_the_table() {
        let out = draw(NetCensusPanel, 90, 10, &crowded()).join("\n");
        assert!(!out.contains("CLOCKS"), "{out}");
        assert_eq!(out.matches("10:00:00:00:00:").count(), 6, "{out}");
    }

    /// Bars that do not fit are counted, never dropped silently: with room
    /// for one row of bars only, the count goes on the section rule.
    #[test]
    fn bars_left_off_a_short_picture_are_counted() {
        let out = draw(NetCensusPanel, 70, 14, &crowded()).join("\n");
        assert!(out.contains("CLOCKS"), "{out}");
        assert!(out.contains("not drawn"), "{out}");
    }

    #[test]
    fn it_fits_every_size_the_layout_can_hand_it() {
        for w in 20..90u16 {
            for h in 4..20u16 {
                for m in [populated(), selected(), crowded(), SdrMetrics::fixture()] {
                    for line in draw(NetCensusPanel, w, h, &m) {
                        assert!(line.chars().count() <= w as usize, "{w}x{h}: {line:?}");
                    }
                }
            }
        }
    }
}
