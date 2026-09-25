// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `NetBleDetailPanel` - everything about one packet, the one selected in the
//! list (net-ux-polish-plan 5.4); with none selected, the session's frame
//! error rate against SNR (5.7), for all traffic or for the one advertiser
//! the list is filtered to.
//!
//! **It replaced `net_ble_rf`, which showed the latest packet's modulation
//! beside a list whose cursor could be on a different one.** The detail
//! follows the selection (`NetState::ble_shown`, the list's own account), so
//! what is read here is always about the packet the mark is on. The old
//! panel's limit rows moved here unchanged, with their limits, their caveats
//! and their tests.
//!
//! **In the order a reader asks.** What the packet is (its header, its
//! addresses, what ChSel says where the type defines it); what the device
//! advertises in it (its AD structures, named, the company from the SIG
//! snapshot with its number beside it); then how it arrived (SNR, and the
//! transmitter's crystal error in kHz and ppm, with the frame's tag saying
//! what that offset is worth); then how well it was sent (B8's modulation
//! quality and B9's drift, each against its limit, idiom B). What it
//! claims sits beside how it sent it, never instead of it (rule 3).
//!
//! **Modulation quality only from a packet whose CRC passed** (5.4.c,
//! Viktor's decision of 2026-09-22). The measurement reads the deviation at
//! the symbols it believes were ones and zeros, and a failed CRC says some of
//! them were not: the live screen once showed a 1589 kHz alternating deviation for such a
//! packet. It says "not measured: CRC failed" instead of a number built on
//! wrong bits (rule 2).
//!
//! **The refusals `net_ble_packets` established, reused here**: not decoding,
//! nothing decoded yet, and, of this panel's own, a real packet whose content
//! happened not to contain a settled run of either kind B8 needs
//! (`signal::ble::measure`), a selected packet that has left the list, and
//! the CRC gate above. Each is said, never a zeroed or invented row.

use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::signal::ble::measure::{Drift, ModulationQuality};
use crate::signal::ble::pdu::PduType;
use crate::signal::dsp::uncertainty::Uncertain;
use crate::state::{BlePacket, SdrMetrics};
use crate::ui::panel::{Panel, PanelChrome, Staleness};
use crate::ui::widgets::limit::{Limit, LimitRow, RowWidths};
use crate::ui::widgets::reading::Reading;

pub struct NetBleDetailPanel;

/// **Read from Core 5.4 Vol 6 Part A 3.1**: "The modulation index shall be
/// between 0.45 and 0.55."
const MOD_INDEX_BAND: Limit = Limit::Band {
    low: 0.45,
    high: 0.55,
};

/// `h = 2 * delta_f / symbol_rate`, so the modulation index band above maps
/// to exactly this delta-f1 band at LE 1M's 1 Mb/s symbol rate - derived
/// from the same figure rather than an independently recalled one.
const DELTA_F1_BAND_KHZ: Limit = Limit::Band {
    low: 225.0,
    high: 275.0,
};

/// **Read from Core 5.4 Vol 6 Part A 3.1**: "The minimum frequency
/// deviation shall never be less than 185 kHz when transmitting at 1
/// megasymbol per second". A floor on the minimum, held against the
/// alternating average; `ModulationQuality::delta_f2_avg_hz` says why, and
/// what that can and cannot conclude.
const DELTA_F2_MIN_FLOOR_KHZ: Limit = Limit::Min(185.0);

/// The same section: the minimum deviation, from a 1010 sequence, "shall be
/// no smaller than ±80% of the frequency deviation ... which corresponds to
/// a 00001111 sequence". What is held against it here is the ratio of the
/// two averages, the form a passive listener's many short runs support; the
/// row's reading says which it is.
const RATIO_FLOOR: Limit = Limit::Min(0.8);

/// **Read from Core 5.4 Vol 6 Part A 3.3** (Table 3.4): "The frequency
/// drift during any packet shall be less than 50 kHz", ±50 kHz, for LE as a
/// whole, so on every PHY. A symmetric band, because a burst can drift
/// either warmer or colder than its own start.
const DRIFT_BAND_KHZ: Limit = Limit::Band {
    low: -50.0,
    high: 50.0,
};

/// **Read from the same section**: "The drift rate shall be less than
/// 400 Hz/µs", allowed "anywhere in a packet".
const DRIFT_RATE_BAND: Limit = Limit::Band {
    low: -400.0,
    high: 400.0,
};

/// LE 2M's delta-f1 band: the 0.45 to 0.55 index 3.1 states for every LE
/// PHY, at 2 Msym/s, so twice LE 1M's, derived from the read band exactly as
/// [`DELTA_F1_BAND_KHZ`] is (`h = 2 * delta_f / symbol_rate`).
const DELTA_F1_BAND_2M_KHZ: Limit = Limit::Band {
    low: 450.0,
    high: 550.0,
};

/// LE 2M's floor, **read from the same sentence**: "never be less than 370
/// kHz when transmitting at 2 Msym/s".
const DELTA_F2_MIN_FLOOR_2M_KHZ: Limit = Limit::Min(370.0);

/// How much uncertainty each reading can carry before it dashes rather than
/// prints - a judgement call in the absence of a specification-stated
/// figure, made the same way `Uncertain::is_resolved`'s own doc asks for:
/// a fraction of the band width each limit states, generous enough that an
/// ordinarily noisy real packet still shows a number rather than a dash on
/// every row.
const MOD_INDEX_RESOLUTION: f64 = 0.02;
const DELTA_F1_RESOLUTION_KHZ: f64 = 10.0;
const RATIO_RESOLUTION: f64 = 0.1;
/// As delta-f1's, and doubled with the floor on LE 2M.
const DELTA_F2_RESOLUTION_KHZ: f64 = 10.0;
const DRIFT_RESOLUTION_KHZ: f64 = 10.0;
const DRIFT_RATE_RESOLUTION: f64 = 80.0;

/// The limit rows for a packet on `phy`: the modulation index band is the
/// PHY's own ratio and the same on both, delta-f1 and delta-f2 scale with the
/// symbol rate, the ratio does not, and the drift limits are LE's as a whole
/// (3.3), the same on both. LE 2M's drift was once shown without a limit,
/// for want of one read; 3.3 states it for every LE PHY.
fn rows(
    q: &ModulationQuality,
    d: Option<&Drift>,
    phy: crate::signal::ble::Phy,
) -> Vec<LimitRow<'static>> {
    let two_m = phy == crate::signal::ble::Phy::TwoM;
    let (df1_band, df2_floor, df2_resolution) = if two_m {
        (
            DELTA_F1_BAND_2M_KHZ,
            DELTA_F2_MIN_FLOOR_2M_KHZ,
            2.0 * DELTA_F2_RESOLUTION_KHZ,
        )
    } else {
        (
            DELTA_F1_BAND_KHZ,
            DELTA_F2_MIN_FLOOR_KHZ,
            DELTA_F2_RESOLUTION_KHZ,
        )
    };
    let mut out = vec![
        LimitRow::new(
            "Mod index",
            Reading::new(q.modulation_index, "", MOD_INDEX_RESOLUTION),
            MOD_INDEX_BAND,
        ),
        LimitRow::new(
            "df1 avg",
            Reading::new(
                q.delta_f1_avg_hz.scale(0.001),
                "kHz",
                DELTA_F1_RESOLUTION_KHZ,
            ),
            df1_band,
        ),
        LimitRow::new(
            "df2 avg",
            Reading::new(q.delta_f2_avg_hz.scale(0.001), "kHz", df2_resolution),
            df2_floor,
        ),
        LimitRow::new(
            "df2/df1",
            Reading::new(q.ratio, "", RATIO_RESOLUTION),
            RATIO_FLOOR,
        ),
    ];
    if let Some(d) = d {
        out.push(LimitRow::new(
            "Drift",
            Reading::new(d.drift_hz.scale(0.001), "kHz", DRIFT_RESOLUTION_KHZ),
            DRIFT_BAND_KHZ,
        ));
        out.push(LimitRow::new(
            "Drift rate",
            Reading::new(d.drift_rate_hz_per_us, "Hz/us", DRIFT_RATE_RESOLUTION),
            DRIFT_RATE_BAND,
        ));
    }
    out
}

fn fit(rows: &[LimitRow], width: usize) -> RowWidths {
    RowWidths::fit_within(rows, width)
}

/// The label column of the detail's fields.
const LABEL_W: usize = 9;

/// The selected packet, or why it cannot be shown. `None` when nothing is
/// selected: the detail is then the session's frame error curve
/// ([`fer_view`]), not a packet the reader did not choose.
fn subject(state: &SdrMetrics) -> Option<Result<&BlePacket, &'static str>> {
    let seq = state.net.ble_view.selection.selected?;
    Some(
        state
            .net
            .ble_shown()
            .into_iter()
            .find(|p| p.seq == seq)
            .ok_or("the selected packet has left the list"),
    )
}

/// Nothing selected: the frame error curve of what the list is showing, all
/// traffic, or the one advertiser the list is filtered to (its own curve,
/// from the census record).
fn fer_view(state: &SdrMetrics, iw: usize, theme: &crate::Theme) -> Vec<Line<'static>> {
    match state.net.ble_view.filter {
        None => fer_lines(&state.net.fer, "all traffic", iw, theme),
        Some(a) => match state.net.census.devices.iter().find(|d| d.address == a) {
            Some(d) => fer_lines(
                &d.fer,
                &state.net.show_address(a, d.random, None),
                iw,
                theme,
            ),
            None => vec![note("the filtered address is not in the census", theme)],
        },
    }
}

/// `label  value`, in the Lab's field idiom.
fn field_line(label: &str, value: String, theme: &crate::Theme) -> Line<'static> {
    Line::from(vec![
        crate::ui::chrome::field(label, LABEL_W, theme),
        Span::styled(value, Style::default().fg(theme.value)),
    ])
}

/// A plain sentence in the label ink, indented like a field.
fn note(text: &str, theme: &crate::Theme) -> Line<'static> {
    Line::from(Span::styled(
        format!(" {text}"),
        Style::default().fg(theme.label),
    ))
}

/// `TxAdd` / `RxAdd` as the kind they name: the advertiser's kind from its
/// address where it has one, the bit's plain meaning otherwise.
fn address_kinds(p: &BlePacket) -> String {
    use crate::signal::ble::address::kind;
    let tx = match p.adv_addr {
        Some(a) => kind(a, p.tx_add_random).label().to_string(),
        None => if p.tx_add_random { "random" } else { "public" }.to_string(),
    };
    let rx = if p.rx_add_random { "random" } else { "public" };
    // RxAdd names the target's address on the types that carry one; on the
    // others it is reserved and not shown.
    if matches!(
        p.pdu_type,
        PduType::AdvDirectInd | PduType::ScanReq | PduType::ConnectInd
    ) {
        format!("Tx {tx} \u{00b7} Rx {rx}")
    } else {
        format!("Tx {tx}")
    }
}

/// What ChSel says, on the three types that define it; `None` elsewhere,
/// where the bit is reserved and a line about it would be a claim.
fn ch_sel(p: &BlePacket) -> Option<&'static str> {
    matches!(
        p.pdu_type,
        PduType::AdvInd | PduType::AdvDirectInd | PduType::ConnectInd
    )
    .then_some(if p.ch_sel {
        "supports CSA #2"
    } else {
        "CSA #1 only"
    })
}

/// A UUID as it is written (`ad::uuid_text`), with the SIG's name beside a
/// 16-bit one the snapshot lists.
fn uuid(written: &[u8]) -> String {
    let text = crate::signal::ble::ad::uuid_text(written);
    match written {
        [hi, lo] => match crate::signal::ble::assigned::service16(u16::from_be_bytes([*hi, *lo])) {
            Some(name) => format!("{text} {name}"),
            None => text,
        },
        _ => text,
    }
}

/// A field whose value may run long, wrapped under its label so a long list
/// of services or octets is read whole rather than cut.
fn wrapped(label: &str, value: &str, iw: usize, theme: &crate::Theme) -> Vec<Line<'static>> {
    crate::ui::chrome::wrap(value, iw.saturating_sub(LABEL_W + 1).max(1), 6)
        .into_iter()
        .enumerate()
        .map(|(i, row)| field_line(if i == 0 { label } else { "" }, row, theme))
        .collect()
}

/// The ADVERTISED section: every AD structure the payload carries, in order,
/// named and made readable (net-ux-polish-plan 5.2's decode, drawn).
///
/// **Only from a packet whose CRC passed**, as the list's NAME column: a
/// failed CRC's octets could spell structures nobody sent. A malformed
/// structure is shown where it is, in the warning ink, and nothing after it
/// (the parser stops there, and so does this). A company is named from the
/// SIG snapshot, with its number beside it: the number is the reading, the
/// name is the registry's.
/// What the packet advertised, each value as the address mode allows it
/// (`NetState::show_name`, `show_bytes`): masking the address and printing
/// the name beside it would be a mask for show.
fn advertised_lines(
    p: &BlePacket,
    net: &crate::state::NetState,
    iw: usize,
    theme: &crate::Theme,
) -> Vec<Line<'static>> {
    use crate::signal::ble::ad::{self, Ad, Structure};
    let mut out = vec![crate::ui::chrome::section("advertised", "", iw, theme)];
    if p.pdu_type == PduType::Other(0x07) {
        out.push(note(
            "extended advertising: its payload is not decoded",
            theme,
        ));
        return out;
    }
    let Some(data) = ad::adv_data(p.pdu_type, &p.payload) else {
        return Vec::new();
    };
    if !p.crc_ok {
        out.push(Line::from(Span::styled(
            " not read: CRC failed".to_string(),
            Style::default().fg(theme.stale),
        )));
        return out;
    }
    let structures = ad::parse(data);
    if structures.is_empty() {
        out.push(note("nothing beyond the address", theme));
        return out;
    }
    for structure in structures {
        match structure {
            Structure::Malformed { offset, why } => {
                for row in crate::ui::chrome::wrap(
                    &format!("malformed at octet {offset}: {why}"),
                    iw.saturating_sub(1).max(1),
                    3,
                ) {
                    out.push(Line::from(Span::styled(
                        format!(" {row}"),
                        Style::default().fg(theme.status_crit),
                    )));
                }
            }
            Structure::Ad { ad, .. } => match ad {
                Ad::Flags(bits) => {
                    let names = ad::flag_names(bits);
                    let text = if names.is_empty() {
                        "none set".to_string()
                    } else {
                        names.join(", ")
                    };
                    out.extend(wrapped("flags", &text, iw, theme));
                }
                Ad::Name { complete, text } => out.extend(wrapped(
                    "name",
                    &format!(
                        "{}{}",
                        net.show_name(&text),
                        if complete { "" } else { " (shortened)" }
                    ),
                    iw,
                    theme,
                )),
                Ad::TxPower(dbm) => out.push(field_line("TX power", format!("{dbm} dBm"), theme)),
                Ad::Uuids {
                    complete, uuids, ..
                } => {
                    let list = uuids.iter().map(|u| uuid(u)).collect::<Vec<_>>().join(", ");
                    let tail = if complete { "" } else { " (incomplete)" };
                    out.extend(wrapped("services", &format!("{list}{tail}"), iw, theme));
                }
                Ad::ServiceData { uuid: u, data } => out.extend(wrapped(
                    "svc data",
                    &format!("{}: {}", uuid(&u), net.show_bytes(&data)),
                    iw,
                    theme,
                )),
                Ad::Manufacturer { company, data } => {
                    let name = crate::signal::ble::assigned::company(company)
                        .map(|n| format!("{n} (0x{company:04X})"))
                        .unwrap_or_else(|| format!("0x{company:04X}, not in the SIG snapshot"));
                    out.extend(wrapped("company", &name, iw, theme));
                    if !data.is_empty() {
                        out.extend(wrapped("mfr data", &net.show_bytes(&data), iw, theme));
                    }
                }
                Ad::Other { code, data } => {
                    let what = match ad::type_name(code) {
                        Some(name) => format!("{name} (0x{code:02X})"),
                        None => format!("AD type 0x{code:02X}"),
                    };
                    out.extend(wrapped(
                        "other",
                        &format!("{what}: {}", net.show_bytes(&data)),
                        iw,
                        theme,
                    ));
                }
            },
        }
    }
    out
}

/// The PACKET, ADVERTISED and PHYSICS sections.
fn header_lines(
    p: &BlePacket,
    state: &SdrMetrics,
    iw: usize,
    theme: &crate::Theme,
) -> Vec<Line<'static>> {
    use crate::ui::chrome::section;
    let now = std::time::Instant::now();
    let age = now.saturating_duration_since(p.seen).as_secs();
    let hint = format!("selected \u{00b7} {age} s ago");
    let mut out = vec![
        section("packet", &hint, iw, theme),
        field_line(
            "type",
            format!(
                "{} \u{00b7} ch {} \u{00b7} {} octets \u{00b7} {}",
                p.pdu_type.label(),
                p.channel,
                p.length,
                p.phy.label()
            ),
            theme,
        ),
        Line::from(vec![
            crate::ui::chrome::field("CRC", LABEL_W, theme),
            if p.crc_ok {
                Span::styled("ok", Style::default().fg(theme.status_ok))
            } else {
                Span::styled("failed", Style::default().fg(theme.status_crit))
            },
        ]),
    ];
    if let Some(a) = p.adv_addr {
        out.push(field_line(
            "address",
            state.net.show_address(a, p.tx_add_random, None),
            theme,
        ));
    }
    out.push(field_line("kinds", address_kinds(p), theme));
    if let Some(text) = ch_sel(p) {
        out.push(field_line("ChSel", text.to_string(), theme));
    }
    out.extend(advertised_lines(p, &state.net, iw, theme));
    out.extend(connection_lines(p, state, iw, theme));

    out.push(section("physics", "", iw, theme));
    out.push(field_line(
        "SNR",
        p.snr_db
            .map(|db| format!("{db:.1} dB"))
            .unwrap_or_else(|| "-".to_string()),
        theme,
    ));
    let carrier = crate::signal::ble::channel::centre_hz(p.channel);
    // The one conversion every NET offset takes, so the three lines below
    // and the CFO column in the list are one scale (rule 5).
    let offset = |hz: Uncertain| carrier.map(|c| state.radio.transmitter_offset(hz, c as f64, now));
    out.push(field_line(
        "CFO",
        match p.freq_offset_hz.and_then(offset) {
            Some(t) => format!(
                "{}  {}",
                Reading::new(t.khz, "kHz", f64::INFINITY).text(),
                Reading::new(t.ppm, "ppm", f64::INFINITY).text()
            ),
            None => "-".to_string(),
        },
        theme,
    ));
    // Bluetooth design measurement 8: the two ends of B9's drift
    // measurement, which is how the specification frames it; the drift row
    // below is their difference. Only where the drift was measured, and only
    // from a CRC-good packet, for the modulation section's reason (5.4.c).
    if let (Some(d), true) = (p.drift.as_ref(), p.crc_ok) {
        for (label, hz) in [("start", d.initial_hz), ("end", d.final_hz)] {
            if let Some(t) = offset(hz) {
                out.push(field_line(
                    label,
                    Reading::new(t.khz, "kHz", f64::INFINITY).text(),
                    theme,
                ));
            }
        }
    }
    out
}

/// The CONNECTION section of a `CONNECT_IND`: what the advertising channel
/// said about the connection being opened (`signal::ble::connect`).
///
/// **Read, not followed** (rule 1). The parameters are the packet's; the
/// hop sequence is a prediction from them, by the algorithm ChSel names
/// (#1, or #2 since 5.4.b4, each tested against the specification's own
/// numbers), labelled so and never presented as observed.
/// Units are the Link Layer's own (Core 5.4 Vol 6 Part B 2.3.3.1):
/// `Interval` and the window in 1.25 ms steps, `Timeout` in 10 ms, the SCA
/// by Table 2.11.
fn connection_lines(
    p: &BlePacket,
    state: &SdrMetrics,
    iw: usize,
    theme: &crate::Theme,
) -> Vec<Line<'static>> {
    use crate::signal::ble::connect::{decode_octets, sca_ppm, Csa1, Csa2};
    if p.pdu_type != PduType::ConnectInd {
        return Vec::new();
    }
    let mut out = vec![crate::ui::chrome::section(
        "connection",
        "read, not followed",
        iw,
        theme,
    )];
    if !p.crc_ok {
        out.push(Line::from(Span::styled(
            " not read: CRC failed".to_string(),
            Style::default().fg(theme.stale),
        )));
        return out;
    }
    let Some(c) = decode_octets(&p.payload) else {
        out.push(note(
            "payload shorter than a CONNECT_IND's 34 octets",
            theme,
        ));
        return out;
    };
    // TxAdd names the initiator's address on a CONNECT_IND, RxAdd the
    // advertiser's.
    // The Link Layer's own field names: short, and exactly what the
    // specification calls them.
    out.push(field_line(
        "InitA",
        state.net.show_address(c.init_a, p.tx_add_random, None),
        theme,
    ));
    out.push(field_line(
        "AdvA",
        state.net.show_address(c.adv_a, p.rx_add_random, None),
        theme,
    ));
    out.push(field_line(
        "access",
        format!("0x{:08X}", c.access_address),
        theme,
    ));
    out.push(field_line(
        "CRC init",
        format!("0x{:06X}", c.crc_init),
        theme,
    ));
    out.push(field_line(
        "interval",
        format!("{:.2} ms", c.interval as f64 * 1.25),
        theme,
    ));
    out.push(field_line(
        "latency",
        format!("{} events", c.latency),
        theme,
    ));
    out.push(field_line(
        "timeout",
        format!("{} ms", c.timeout as u32 * 10),
        theme,
    ));
    out.push(field_line(
        "window",
        format!(
            "{:.2} ms at +{:.2} ms",
            c.win_size as f64 * 1.25,
            c.win_offset as f64 * 1.25
        ),
        theme,
    ));
    let used = (c.channel_map & 0x1F_FFFF_FFFF).count_ones();
    out.push(field_line(
        "channels",
        format!("{used} of 37 used (map 0x{:010X})", c.channel_map),
        theme,
    ));
    let (lo, hi) = sca_ppm(c.sca);
    out.push(field_line(
        "SCA",
        format!("{} ({lo} to {hi} ppm)", c.sca),
        theme,
    ));
    out.push(field_line("hop", c.hop_increment.to_string(), theme));
    // ChSel says which algorithm the connection uses: #2 only when both
    // ends support it (`pdu::Packet::ch_sel`), and each is predicted by its
    // own rule rather than one standing in for the other.
    let first: Option<(&str, Vec<u8>)> = if p.ch_sel {
        Csa2::new(c.access_address, c.channel_map)
            .map(|csa| ("CSA #2", (0..8).map(|n| csa.channel(n).0).collect()))
    } else {
        Csa1::new(c.hop_increment, c.channel_map)
            .map(|mut csa| ("CSA #1", (0..8).map(|_| csa.next()).collect()))
    };
    let hops = match first {
        Some((algorithm, channels)) => format!(
            "{algorithm}: {} ... predicted, not followed",
            channels
                .iter()
                .map(u8::to_string)
                .collect::<Vec<_>>()
                .join(" ")
        ),
        None => "no channel used: nothing to predict".to_string(),
    };
    out.extend(wrapped("hops", &hops, iw, theme));
    out
}

/// The MODULATION section: the limit rows, or why there are none.
fn modulation_lines(p: &BlePacket, iw: usize, theme: &crate::Theme) -> Vec<Line<'static>> {
    // Where the limits come from, as the classic rows say theirs: every row
    // read, the modulation from 3.1 and the drift from 3.3.
    let mut out = vec![crate::ui::chrome::section(
        "modulation",
        "LE limits: Core 5.4 Vol 6 A 3.1, 3.3",
        iw,
        theme,
    )];
    if !p.crc_ok {
        out.push(Line::from(Span::styled(
            " not measured: CRC failed".to_string(),
            Style::default().fg(theme.stale),
        )));
        out.extend(
            crate::ui::chrome::wrap(
                "which symbols were ones decides where the deviation is read, and a failed CRC says some were not",
                iw.saturating_sub(1),
                3,
            )
            .iter()
            .map(|l| note(l, theme)),
        );
        return out;
    }
    let Some(q) = p.modulation else {
        out.push(Line::from(Span::styled(
            " not measured".to_string(),
            Style::default().fg(theme.stale),
        )));
        out.push(note(
            "packet too short, or too short a run of either kind",
            theme,
        ));
        return out;
    };
    let rows = rows(&q, p.drift.as_ref(), p.phy);
    let w = fit(&rows, iw);
    out.extend(rows.iter().map(|r| Line::from(r.spans(theme, w))));
    out
}

/// The frame error curve as rows (net-ux-polish-plan 5.7): one SNR bin a
/// row, from the lowest bin with packets to the highest, each a bar as long as
/// its failure fraction with the rate and its uncertainty beside it and how
/// many packets it rests on; a thin bin says it is thin instead of drawing.
fn fer_lines(
    curve: &crate::signal::ble::fer::FerCurve,
    scope: &str,
    iw: usize,
    theme: &crate::Theme,
) -> Vec<Line<'static>> {
    use crate::signal::ble::fer::{edges, MIN_PACKETS};
    let hint = format!("{scope} \u{00b7} {} packets", curve.total());
    let mut out = vec![crate::ui::chrome::section(
        "frame errors by SNR",
        &hint,
        iw,
        theme,
    )];
    let Some(span) = curve.span() else {
        out.push(note("no packet with an SNR yet", theme));
        return out;
    };
    const RANGE_W: usize = 10;
    const VALUE_W: usize = 14;
    const COUNT_W: usize = 7;
    let bar_w = iw
        .saturating_sub(1 + RANGE_W + 1 + 1 + VALUE_W + 1 + COUNT_W)
        .max(4);
    let range_of = |first: usize, last: usize| match (edges(first).0, edges(last).1) {
        (None, Some(hi)) => format!("<{hi:.0} dB"),
        (Some(lo), None) => format!("\u{2265}{lo:.0} dB"),
        (Some(lo), Some(hi)) => format!("{lo:.0}-{hi:.0} dB"),
        (None, None) => String::new(),
    };
    let bins: Vec<usize> = span.collect();
    let mut i = 0;
    while i < bins.len() {
        let bin = bins[i];
        let n = curve.packets(bin);
        // A run of empty bins is one row: the gap shows, the list stays short.
        if n == 0 {
            let mut last = i;
            while last + 1 < bins.len() && curve.packets(bins[last + 1]) == 0 {
                last += 1;
            }
            out.push(Line::from(vec![
                Span::styled(
                    format!(" {:>RANGE_W$} ", range_of(bin, bins[last])),
                    Style::default().fg(theme.label),
                ),
                Span::styled("no packets".to_string(), Style::default().fg(theme.label)),
            ]));
            i = last + 1;
            continue;
        }
        i += 1;
        let range = range_of(bin, bin);
        let mut spans = vec![Span::styled(
            format!(" {range:>RANGE_W$} "),
            Style::default().fg(theme.label),
        )];
        match curve.rate(bin) {
            Some(rate) => {
                let (filled, empty) = crate::ui::widgets::charts::eighth_block_bar(
                    (rate.value() * 1000.0).round() as u32,
                    1000,
                    bar_w,
                );
                spans.push(Span::styled(
                    filled,
                    Style::default().fg(theme.border_accent),
                ));
                spans.push(Span::styled(empty, Style::default().fg(theme.border_dim)));
                spans.push(Span::styled(
                    format!(
                        " {:>VALUE_W$}",
                        Reading::new(rate.scale(100.0), "%", f64::INFINITY).text()
                    ),
                    Style::default().fg(theme.value),
                ));
                spans.push(Span::styled(
                    format!(" {:>COUNT_W$}", format!("({n})")),
                    Style::default().fg(theme.label),
                ));
            }
            None => spans.push(Span::styled(
                format!("{n} packets, fewer than {MIN_PACKETS}"),
                Style::default().fg(theme.stale),
            )),
        }
        out.push(Line::from(spans));
    }
    for row in crate::ui::chrome::wrap(
        "CRC failures among packets whose length matched; gave-ups are on Feed Health",
        iw.saturating_sub(1).max(1),
        3,
    ) {
        out.push(note(&row, theme));
    }
    out
}

impl Panel for NetBleDetailPanel {
    fn name(&self) -> &'static str {
        "net_ble_detail"
    }

    fn min_size(&self) -> (u16, u16) {
        (28, 4)
    }

    fn chrome(&self, state: &SdrMetrics) -> PanelChrome {
        let mut chrome = PanelChrome::new("Packet Detail")
            .stale_when(Staleness::NotStreaming)
            .tag_if(true, state.net.mode.tag())
            .tag_if(true, crate::ui::panel::Tag::Phy(state.net.ble_phy));
        // The declarations follow what the view prints: a packet carries a
        // ppm and addresses, the error curve carries neither, except that a
        // filtered curve is headed by its address.
        if state.net.ble_refused.is_none() {
            if matches!(subject(state), Some(Ok(_))) {
                chrome = chrome.shows_offsets().shows_addresses();
            } else if subject(state).is_none() && state.net.ble_view.filter.is_some() {
                chrome = chrome.shows_addresses();
            }
        }
        chrome
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

        if let Some(reason) = &state.net.ble_refused {
            let mut lines = vec![Line::from(Span::styled(
                "not decoding".to_string(),
                Style::default().fg(theme.stale),
            ))];
            for chunk in crate::ui::chrome::wrap(reason, width, 4) {
                lines.push(Line::from(Span::styled(
                    chunk,
                    Style::default().fg(theme.label),
                )));
            }
            f.render_widget(Paragraph::new(lines), inner);
            return;
        }

        let p = match subject(state) {
            None => {
                f.render_widget(Paragraph::new(fer_view(state, width, theme)), inner);
                return;
            }
            Some(Ok(found)) => found,
            Some(Err(why)) => {
                f.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        why.to_string(),
                        Style::default().fg(theme.stale),
                    ))),
                    inner,
                );
                return;
            }
        };

        let mut lines = header_lines(p, state, width, theme);
        lines.extend(modulation_lines(p, width, theme));
        f.render_widget(Paragraph::new(lines), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::ble::pdu::PduType;
    use crate::state::fixture::draw;
    use crate::state::BlePacket;
    use std::time::Instant;

    /// `m` with its newest packet selected, where nothing was: the packet
    /// view these tests are about, which since 5.7 needs a selection (with
    /// none the detail is the frame error curve).
    fn sel(m: &SdrMetrics) -> SdrMetrics {
        let mut m = m.clone();
        if m.net.ble_view.selection.selected.is_none() {
            m.net.ble_view.selection.selected = m.net.ble_packets.front().map(|p| p.seq);
        }
        m
    }

    fn quality(deviation_hz: f64) -> ModulationQuality {
        ModulationQuality {
            delta_f1_avg_hz: Uncertain::from_sigma(deviation_hz, deviation_hz * 0.01),
            delta_f2_avg_hz: Uncertain::from_sigma(deviation_hz * 0.9, deviation_hz * 0.01),
            modulation_index: Uncertain::from_sigma(2.0 * deviation_hz / 1_000_000.0, 0.005),
            ratio: Uncertain::from_sigma(0.9, 0.02),
        }
    }

    fn drift(drift_hz: f64) -> Drift {
        let initial_hz = Uncertain::from_sigma(0.0, 500.0);
        let final_hz = Uncertain::from_sigma(drift_hz, 500.0);
        let drift = final_hz.difference(&initial_hz);
        Drift {
            initial_hz,
            final_hz,
            drift_hz: drift,
            drift_rate_hz_per_us: drift.scale(0.01),
        }
    }

    fn packet(modulation: Option<ModulationQuality>) -> BlePacket {
        packet_with_drift(modulation, None)
    }

    fn packet_with_drift(
        modulation: Option<ModulationQuality>,
        drift: Option<crate::signal::ble::measure::Drift>,
    ) -> BlePacket {
        BlePacket {
            phy: crate::signal::ble::Phy::OneM,
            seq: 0,
            channel: 37,
            pdu_type: PduType::AdvInd,
            tx_add_random: false,
            ch_sel: false,
            rx_add_random: false,
            payload: Vec::new(),
            length: 9,
            adv_addr: Some([0xaa, 0xbb, 0xcc, 0x11, 0x22, 0x33]),
            crc_ok: true,
            snr_db: Some(12.0),
            freq_offset_hz: None,
            modulation,
            drift,
            seen: Instant::now(),
        }
    }

    #[test]
    fn a_refusal_is_shown_rather_than_an_empty_panel() {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.ble_refused = Some("not tuned to an advertising channel".to_string());
        let out = draw(NetBleDetailPanel, 40, 24, &sel(&m)).join("\n");
        assert!(out.contains("not decoding"), "{out}");
        assert!(out.contains("not tuned"), "{out}");
    }

    #[test]
    fn an_empty_feed_says_nothing_decoded_yet() {
        let out = draw(
            NetBleDetailPanel,
            40,
            8,
            &sel(&SdrMetrics::fixture().streaming()),
        )
        .join("\n");
        assert!(out.contains("no packet with an SNR yet"), "{out}");
    }

    #[test]
    fn a_packet_with_nothing_measured_refuses_rather_than_inventing_rows() {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.ble_packets.push_back(packet(None));
        let out = draw(NetBleDetailPanel, 40, 24, &sel(&m)).join("\n");
        assert!(out.contains("not measured"), "{out}");
        assert!(!out.contains("Mod index"), "{out}");
    }

    /// The nominal deviation's own modulation index draws inside its band,
    /// and none of the four limits' words appear anywhere - the same
    /// discipline `the_word_pass_appears_nowhere_in_any_rendering` holds
    /// the widget itself to, now through a real panel.
    #[test]
    fn a_good_packet_draws_all_four_rows_and_never_says_pass() {
        let mut m = SdrMetrics::fixture().streaming();
        m.net
            .ble_packets
            .push_back(packet(Some(quality(250_000.0))));
        let out = draw(NetBleDetailPanel, 70, 24, &sel(&m)).join("\n");
        assert!(out.contains("Mod index"), "{out}");
        assert!(out.contains("df1 avg"), "{out}");
        assert!(out.contains("df2 avg"), "{out}");
        assert!(out.contains("df2/df1"), "{out}");
        let lower = out.to_ascii_lowercase();
        for word in ["pass", "fail"] {
            assert!(!lower.contains(word), "{word} in {out:?}");
        }
    }

    /// B9's exit condition on screen: a packet with drift measured shows
    /// two more rows than one without, and neither row's word is "pass".
    #[test]
    fn a_packet_with_drift_draws_two_more_rows() {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.ble_packets.push_back(packet_with_drift(
            Some(quality(250_000.0)),
            Some(drift(5_000.0)),
        ));
        let out = draw(NetBleDetailPanel, 70, 24, &sel(&m)).join("\n");
        assert!(out.contains("Drift"), "{out}");
        assert!(out.contains("Drift rate"), "{out}");
        let lower = out.to_ascii_lowercase();
        for word in ["pass", "fail"] {
            assert!(!lower.contains(word), "{word} in {out:?}");
        }
    }

    /// Modulation quality without a drift reading draws its own four rows
    /// and nothing more, rather than a fifth refusal state for a gap this
    /// arc has not observed to occur on its own.
    #[test]
    fn modulation_without_drift_draws_no_drift_rows() {
        let mut m = SdrMetrics::fixture().streaming();
        m.net
            .ble_packets
            .push_back(packet(Some(quality(250_000.0))));
        let out = draw(NetBleDetailPanel, 70, 24, &sel(&m)).join("\n");
        assert!(!out.contains("Drift"), "{out}");
    }

    #[test]
    fn it_fits_every_size_the_layout_can_hand_it() {
        let mut populated = SdrMetrics::fixture().streaming();
        populated.net.ble_packets.push_back(packet_with_drift(
            Some(quality(250_000.0)),
            Some(drift(5_000.0)),
        ));
        for w in 20..90u16 {
            for h in 4..16u16 {
                for m in [populated.clone(), SdrMetrics::fixture()] {
                    for line in draw(NetBleDetailPanel, w, h, &sel(&m)) {
                        assert!(line.chars().count() <= w as usize, "{w}x{h}: {line:?}");
                    }
                }
            }
        }
    }

    /// Two packets, the older selected: seq 1 on channel 37 at 12 dB, seq 2
    /// on channel 39 at 4 dB.
    fn two() -> SdrMetrics {
        let mut m = SdrMetrics::fixture().streaming();
        let mut older = packet(Some(quality(250_000.0)));
        older.seq = 1;
        let mut newer = packet(None);
        newer.seq = 2;
        newer.channel = 39;
        newer.snr_db = Some(4.0);
        m.net.ble_packets.push_front(older);
        m.net.ble_packets.push_front(newer);
        m.net.ble_heard = 2;
        m
    }

    /// **The detail is about the packet the mark is on**, not the latest:
    /// with the older one selected it reads that one's channel, SNR and
    /// modulation, and says it is the selected one.
    #[test]
    fn the_detail_follows_the_selection() {
        let mut m = two();
        // Nothing selected: the frame error curve, not a packet nobody chose.
        let none = draw(NetBleDetailPanel, 70, 24, &m).join("\n");
        assert!(none.contains("FRAME ERRORS BY SNR"), "{none}");
        assert!(!none.contains("PACKET"), "{none}");

        m.net.ble_view.selection.selected = Some(1);
        let out = draw(NetBleDetailPanel, 70, 24, &m).join("\n");
        assert!(out.contains("selected"), "{out}");
        assert!(out.contains("ch 37"), "{out}");
        assert!(out.contains("12.0 dB"), "{out}");
        assert!(out.contains("Mod index"), "{out}");

        // Gone from the list: said, not replaced by whatever is there now.
        m.net.ble_view.selection.selected = Some(99);
        let gone = draw(NetBleDetailPanel, 70, 24, &m).join("\n");
        assert!(gone.contains("has left the list"), "{gone}");
    }

    /// **5.4.c: no modulation from a failed CRC**, however plausible the
    /// numbers the measurement produced; the reason is said.
    #[test]
    fn a_failed_crc_gets_no_modulation_rows() {
        let mut m = SdrMetrics::fixture().streaming();
        let mut p = packet_with_drift(Some(quality(250_000.0)), Some(drift(5_000.0)));
        p.crc_ok = false;
        m.net.ble_packets.push_front(p);
        let out = draw(NetBleDetailPanel, 70, 24, &sel(&m)).join("\n");
        assert!(out.contains("not measured: CRC failed"), "{out}");
        assert!(!out.contains("Mod index"), "{out}");
        assert!(!out.contains("Drift"), "{out}");
        assert!(out.contains("failed"), "{out}");
    }

    /// ChSel is read out only on the three types that define it, and RxAdd
    /// only on the types that carry a target address.
    #[test]
    fn header_bits_are_shown_only_where_the_type_defines_them() {
        let mut adv = packet(None);
        adv.ch_sel = true;
        assert_eq!(ch_sel(&adv), Some("supports CSA #2"));
        assert_eq!(address_kinds(&adv), "Tx public");

        let mut scan_req = packet(None);
        scan_req.pdu_type = PduType::ScanReq;
        scan_req.adv_addr = None;
        scan_req.rx_add_random = true;
        scan_req.ch_sel = true;
        assert_eq!(ch_sel(&scan_req), None, "reserved on SCAN_REQ");
        assert_eq!(address_kinds(&scan_req), "Tx public \u{00b7} Rx random");

        let mut rpa = packet(None);
        rpa.adv_addr = Some([0x4a, 1, 2, 3, 4, 5]);
        rpa.tx_add_random = true;
        assert_eq!(address_kinds(&rpa), "Tx RPA");
    }

    /// The transmitter's offset in kHz and ppm, through the one conversion
    /// every NET offset takes, and the frame says what it is worth.
    #[test]
    fn the_offset_is_given_in_khz_and_ppm_with_its_basis_on_the_frame() {
        let mut m = SdrMetrics::fixture().streaming();
        let mut p = packet(None);
        p.freq_offset_hz = Some(Uncertain::from_sigma(-22_300.0, 500.0));
        m.net.ble_packets.push_front(p);
        let out = draw(NetBleDetailPanel, 80, 24, &sel(&m));
        assert!(out[0].contains("[RELATIVE]"), "{}", out[0]);
        let text = out.join("\n");
        assert!(text.contains("-22.3 ±0.5 kHz"), "{text}");
        assert!(text.contains("-9.28 ±0.21 ppm"), "{text}");
    }

    /// A CRC-good ADV_NONCONN_IND whose advertising data is `ad`.
    fn advertising(ad: &[u8]) -> SdrMetrics {
        let mut m = SdrMetrics::fixture().streaming();
        let mut p = packet(None);
        p.pdu_type = PduType::AdvNonconnInd;
        p.payload =
            crate::signal::ble::pdu::air_octets([0xaa, 0xbb, 0xcc, 0x11, 0x22, 0x33]).to_vec();
        p.payload.extend_from_slice(ad);
        m.net.ble_packets.push_front(p);
        m
    }

    /// **The real packet from the air**: its manufacturer data names Apple
    /// from the SIG snapshot, the number beside it, and the four octets
    /// after the identifier as they were sent.
    #[test]
    fn the_real_packet_s_advertising_reads_as_apple_with_its_octets() {
        let m = advertising(&[0x07, 0xff, 0x4c, 0x00, 0x12, 0x02, 0x00, 0x02]);
        let out = draw(NetBleDetailPanel, 70, 30, &sel(&m)).join("\n");
        assert!(out.contains("ADVERTISED"), "{out}");
        assert!(out.contains("company  Apple, Inc. (0x004C)"), "{out}");
        assert!(out.contains("mfr data 12 02 00 02"), "{out}");
    }

    /// Every decoded kind in readable form: flags by name, the name marked
    /// shortened, TX power, services with the SIG's names, service data.
    #[test]
    fn every_structure_reads_in_plain_words() {
        let m = advertising(&[
            0x02, 0x01, 0x06, // flags
            0x05, 0x03, 0x0f, 0x18, 0x0a, 0x18, // services 0x180F, 0x180A
            0x06, 0x08, b'S', b'e', b'n', b's', b'o', // shortened name
            0x02, 0x0a, 0xf4, // TX power -12
            0x05, 0x16, 0x0f, 0x18, 0x55, 0x66, // service data
        ]);
        let out = draw(NetBleDetailPanel, 90, 34, &sel(&m)).join("\n");
        for want in [
            "flags    LE General Discoverable, BR/EDR Not Supported",
            "name     Senso (shortened)",
            "TX power -12 dBm",
            "services 0x180F Battery, 0x180A Device Information",
            "svc data 0x180F Battery: 55 66",
        ] {
            assert!(out.contains(want), "{want}:\n{out}");
        }
    }

    /// **Malformed at its place, in the warning ink, and nothing after it**;
    /// a failed CRC reads nothing at all; extended advertising says it is
    /// not decoded; an address alone says so.
    #[test]
    fn what_cannot_be_read_says_why() {
        let broken = advertising(&[0x02, 0x01, 0x06, 0x09, 0xff, 0x4c]);
        let out = draw(NetBleDetailPanel, 90, 30, &sel(&broken)).join("\n");
        assert!(out.contains("flags"), "{out}");
        assert!(
            out.contains("malformed at octet 3: length runs past the end"),
            "{out}"
        );

        let mut failed = advertising(&[0x05, 0x09, b'S', b'e', b'n', b's']);
        failed.net.ble_packets[0].crc_ok = false;
        let out = draw(NetBleDetailPanel, 90, 30, &sel(&failed)).join("\n");
        assert!(out.contains("not read: CRC failed"), "{out}");
        assert!(!out.contains("Sens"), "{out}");

        let mut ext = advertising(&[]);
        ext.net.ble_packets[0].pdu_type = PduType::Other(0x07);
        let out = draw(NetBleDetailPanel, 90, 30, &sel(&ext)).join("\n");
        assert!(out.contains("extended advertising"), "{out}");

        let bare = advertising(&[]);
        let out = draw(NetBleDetailPanel, 90, 30, &sel(&bare)).join("\n");
        assert!(out.contains("nothing beyond the address"), "{out}");
    }

    #[test]
    fn uuids_are_written_the_way_the_specification_writes_them() {
        assert_eq!(uuid(&[0x18, 0x0f]), "0x180F Battery");
        assert_eq!(uuid(&[0x00, 0x01]), "0x0001");
        assert_eq!(uuid(&[0x12, 0x34, 0x56, 0x78]), "0x12345678");
        let long: Vec<u8> = (0..16).collect();
        assert_eq!(uuid(&long), "00010203-0405-0607-0809-0a0b0c0d0e0f");
    }

    /// **The two ends of the drift, on the CFO's scale**, only where the
    /// drift was measured and the CRC passed.
    #[test]
    fn start_and_end_frequency_sit_under_the_cfo() {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.ble_packets.push_front(packet_with_drift(
            Some(quality(250_000.0)),
            Some(drift(5_000.0)),
        ));
        let out = draw(NetBleDetailPanel, 80, 30, &sel(&m)).join("\n");
        assert!(out.contains("start    0.0 ±0.5 kHz"), "{out}");
        assert!(out.contains("end      5.0 ±0.5 kHz"), "{out}");

        m.net.ble_packets[0].crc_ok = false;
        let failed = draw(NetBleDetailPanel, 80, 30, &sel(&m)).join("\n");
        assert!(!failed.contains("start"), "{failed}");
        m.net.ble_packets[0] = packet(Some(quality(250_000.0)));
        let none = draw(NetBleDetailPanel, 80, 30, &sel(&m)).join("\n");
        assert!(!none.contains("start"), "{none}");
    }

    /// A CONNECT_IND's 34 octets, as sent: InitA and AdvA in air order, then
    /// LLData: AA, CRCInit, WinSize 2, WinOffset 4, Interval 24, Latency 0,
    /// Timeout 72, every channel used, hop 7 with SCA 5.
    fn connect_ind(ch_sel: bool, crc_ok: bool) -> SdrMetrics {
        use crate::signal::ble::pdu::air_octets;
        let mut payload = air_octets([0x11, 0x22, 0x33, 0x44, 0x55, 0x66]).to_vec();
        payload.extend(air_octets([0xaa, 0xbb, 0xcc, 0x11, 0x22, 0x33]));
        payload.extend(0xAF9A_B12Cu32.to_le_bytes());
        payload.extend(&0x55_5555u32.to_le_bytes()[..3]);
        payload.push(2);
        payload.extend(4u16.to_le_bytes());
        payload.extend(24u16.to_le_bytes());
        payload.extend(0u16.to_le_bytes());
        payload.extend(72u16.to_le_bytes());
        payload.extend(&0x1F_FFFF_FFFFu64.to_le_bytes()[..5]);
        payload.push(7 | (5 << 5));
        assert_eq!(payload.len(), 34);

        let mut m = SdrMetrics::fixture().streaming();
        let mut p = packet(None);
        p.pdu_type = PduType::ConnectInd;
        p.adv_addr = None;
        p.length = 34;
        p.payload = payload;
        p.ch_sel = ch_sel;
        p.crc_ok = crc_ok;
        m.net.ble_packets.push_front(p);
        m
    }

    /// **What the advertising channel told us about the connection**, in
    /// the Link Layer's units, and the first hops Algorithm #1 predicts from
    /// it, said to be predicted.
    #[test]
    fn a_connect_ind_shows_its_parameters_and_predicted_hops() {
        let out = draw(NetBleDetailPanel, 90, 40, &sel(&connect_ind(false, true))).join("\n");
        for want in [
            "CONNECTION",
            "read, not followed",
            "InitA    11:22:33:44:55:66",
            "AdvA     aa:bb:cc:11:22:33",
            "access   0xAF9AB12C",
            "CRC init 0x555555",
            "interval 30.00 ms",
            "latency  0 events",
            "timeout  720 ms",
            "window   2.50 ms at +5.00 ms",
            "channels 37 of 37 used",
            "SCA      5 (31 to 50 ppm)",
            "hop      7",
            "CSA #1: 7 14 21 28 35 5 12 19 ... predicted, not followed",
        ] {
            assert!(out.contains(want), "{want}:\n{out}");
        }
        assert!(out.contains("ChSel    CSA #1 only"), "{out}");
    }

    /// **ChSel set: Algorithm #2's sequence, never #1's**, from this
    /// connection's Access Address; and a failed CRC reads no parameters.
    #[test]
    fn a_connect_ind_on_csa2_is_predicted_by_csa2_and_a_failed_one_is_not_read() {
        let csa2 = draw(NetBleDetailPanel, 90, 40, &sel(&connect_ind(true, true))).join("\n");
        let expected = crate::signal::ble::connect::Csa2::new(0xAF9A_B12C, 0x1F_FFFF_FFFF).unwrap();
        let first: Vec<String> = (0..8).map(|n| expected.channel(n).0.to_string()).collect();
        let want = format!("CSA #2: {} ... predicted, not followed", first.join(" "));
        assert!(csa2.contains(&want), "{want}:\n{csa2}");
        assert!(!csa2.contains("7 14 21"), "{csa2}");
        assert!(csa2.contains("ChSel    supports CSA #2"), "{csa2}");

        let failed = draw(NetBleDetailPanel, 90, 40, &sel(&connect_ind(false, false))).join("\n");
        assert!(failed.contains("not read: CRC failed"), "{failed}");
        assert!(!failed.contains("0xAF9AB12C"), "{failed}");
    }

    /// **The PHY on the frame and on the packet**: the tag says what the
    /// decoder listens for, the type line what this packet came on, and an
    /// LE 2M packet's modulation is not measured against LE 1M's limits.
    #[test]
    fn the_phy_is_named_and_2m_is_judged_by_its_own_limits() {
        let mut m = SdrMetrics::fixture().streaming();
        let mut q = quality(500_000.0);
        q.modulation_index = Uncertain::from_sigma(0.5, 0.005);
        let mut p = packet_with_drift(Some(q), Some(drift(5_000.0)));
        p.phy = crate::signal::ble::Phy::TwoM;
        m.net.ble_packets.push_front(p);
        m.net.ble_phy = crate::signal::ble::Phy::TwoM;
        let out = draw(NetBleDetailPanel, 90, 34, &sel(&m));
        assert!(out[0].contains("[LE 2M]"), "{}", out[0]);
        let text = out.join("\n");
        assert!(text.contains("9 octets \u{00b7} LE 2M"), "{text}");
        assert!(text.contains("Mod index"), "{text}");
        // LE 2M's own delta-f1 band and delta-f2 floor, not LE 1M's.
        assert!(text.contains("450") && text.contains("550"), "{text}");
        assert!(text.contains("370"), "{text}");
        assert!(!text.contains("225"), "{text}");
        // Drift against the limits 3.3 states for every LE PHY, not left
        // without one as it was before they were read.
        assert!(text.contains("Drift rate"), "{text}");
        assert!(text.contains("-50") && text.contains("400"), "{text}");
        assert!(text.contains("Core 5.4 Vol 6 A 3.1, 3.3"), "{text}");
    }

    /// **Nothing selected: the session's frame error curve**, all traffic,
    /// each bin's rate with its uncertainty and count, a thin bin said to be
    /// thin; filtered to one address, that device's own curve under its name.
    #[test]
    fn nothing_selected_shows_the_frame_error_curve() {
        let mut m = SdrMetrics::fixture().streaming();
        for i in 0..30 {
            m.net.fer.record(5.0, i >= 3);
        }
        for _ in 0..4 {
            m.net.fer.record(20.0, true);
        }
        let out = draw(NetBleDetailPanel, 80, 24, &m).join("\n");
        assert!(out.contains("FRAME ERRORS BY SNR"), "{out}");
        assert!(out.contains("all traffic \u{00b7} 34 packets"), "{out}");
        assert!(out.contains("4-6 dB"), "{out}");
        assert!(out.contains("10 ±6 %"), "{out}");
        assert!(out.contains("(30)"), "{out}");
        assert!(out.contains("4 packets, fewer than 10"), "{out}");
        assert!(
            out.contains("no packets"),
            "the empty run between them: {out}"
        );

        let addr = [0xaa, 0xbb, 0xcc, 0x11, 0x22, 0x33];
        let mut d = crate::signal::net::census::Device::heard(addr, false, Instant::now());
        for _ in 0..12 {
            d.fer.record(30.0, true);
        }
        m.net.census.devices.push(d);
        m.net.ble_view.filter = Some(addr);
        let one = draw(NetBleDetailPanel, 80, 24, &m).join("\n");
        assert!(
            one.contains("aa:bb:cc:11:22:33 \u{00b7} 12 packets"),
            "{one}"
        );
        assert!(!one.contains("34 packets"), "{one}");
    }
}
