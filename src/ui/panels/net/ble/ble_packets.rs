// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `NetBlePacketsPanel` - what this device is advertising.
//!
//! B6's exit condition, on screen: real advertising channel PDUs, CRC-checked,
//! newest first. B7 added the SNR and CFO columns. No sorting - a live packet
//! feed is already in the order that matters, arrival order, and a second
//! ordering has not earned its own column yet.
//!
//! **A cursor since net-ux-polish-plan 5.3**, on the packet rather than the
//! row (`state::BlePacketView`): the list is newest first, so each arrival
//! moves every row down one, and the mark goes with its packet, the view
//! following it down.
//!
//! **Three states, not two.** Design section 13.2's lesson for this section:
//! silence has more than one cause, and printing zero for all of them is a
//! lie by omission. Nothing decoding because the radio is not on an
//! advertising channel, nothing decoding because the sample rate cannot
//! reach the working rate, and genuinely nothing heard yet on a channel that
//! is being watched correctly, are three different claims - only the last one
//! is "we listened and nobody transmitted".

use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::state::{BlePacket, RadioState, SdrMetrics};
use crate::ui::panel::{FeedSpan, Panel, PanelChrome, Staleness};
use crate::ui::widgets::reading::Reading;

pub struct NetBlePacketsPanel;

const CH_W: usize = 3;
const TYPE_W: usize = 15;
/// The narrowest the address column is drawn: a full address. It grows from
/// the spare width towards the widest address shown (`addr_width`).
const ADDR_W: usize = crate::state::FULL_ADDRESS_WIDTH;
const ATYP_W: usize = 4;
const LEN_W: usize = 4;
const CRC_W: usize = 4;
const SNR_W: usize = 7;
const CFO_W: usize = 15;
/// Room for `-123.45 ±0.21 ppm`: a crystal can be a hundred ppm out, and
/// `Reading` keeps two decimals when the uncertainty is a fraction of one.
const PPM_W: usize = 17;
const AGE_W: usize = 6;

/// Every column at its narrowest, with the single spaces between them and the
/// selection gutter every row keeps (`chrome::SELECTION_GUTTER`).
const FIXED_W: usize = crate::ui::chrome::SELECTION_GUTTER
    + CH_W
    + TYPE_W
    + ADDR_W
    + ATYP_W
    + LEN_W
    + CRC_W
    + SNR_W
    + CFO_W
    + PPM_W
    + AGE_W
    + 9;

/// The address column's width for this frame: what the panel can spare beyond
/// every column at its narrowest, up to the widest address among `shown`, so
/// a registrant's whole name appears when there is room and is cut and marked
/// when there is not (`state::AddressDisplay::show`).
fn addr_width<'a>(
    shown: impl Iterator<Item = &'a BlePacket>,
    net: &crate::state::NetState,
    width: usize,
) -> usize {
    let want = shown
        .filter_map(|p| p.adv_addr.map(|a| net.address_width(a, p.tx_add_random)))
        .max()
        .unwrap_or(ADDR_W);
    ADDR_W
        + want
            .saturating_sub(ADDR_W)
            .min(width.saturating_sub(FIXED_W))
}

fn header_line(addr_w: usize, theme: &crate::Theme) -> Line<'static> {
    Line::from(Span::styled(
        format!(
            "{}{:<CH_W$} {:<TYPE_W$} {:<addr_w$} {:<ATYP_W$} {:>LEN_W$} {:>CRC_W$} {:>SNR_W$} {:>CFO_W$} {:>PPM_W$} {:>AGE_W$}",
            " ".repeat(crate::ui::chrome::SELECTION_GUTTER),
            "CH", "TYPE", "ADDRESS", "ATYP", "LEN", "CRC", "SNR", "CFO", "PPM", "AGE"
        ),
        Style::default().fg(theme.label),
    ))
}

/// `12.3` or a dash - B7's per-packet SNR, in dB. No unit in the cell itself;
/// the column header carries it, the way every table in this deck does.
fn fmt_snr(snr_db: Option<f64>) -> String {
    match snr_db {
        Some(db) => format!("{db:.1}"),
        None => "-".to_string(),
    }
}

/// `37.0 ±1.2 kHz` and `+15.4 ±0.5 ppm` - the transmitter's crystal error
/// from B7's frequency offset, through `RadioState::transmitter_offset`, the
/// one conversion every NET offset goes through: corrected for our own
/// oscillator when a reference allows, and what that makes it worth is the
/// chrome's engine tag, not these cells. kHz because a crystal's error is
/// tens to hundreds of kHz at 2.4 GHz and a raw Hz figure would be seven
/// digits of which the last five are noise; ppm because that is the unit a
/// crystal is specified in, and the one the census compares clocks in.
/// Both dash together when the packet carried no offset, or its channel has
/// no frequency to take a fraction of.
fn fmt_offset(p: &BlePacket, radio: &RadioState, now: std::time::Instant) -> (String, String) {
    let carrier = crate::signal::ble::channel::centre_hz(p.channel);
    match p.freq_offset_hz.zip(carrier) {
        // No resolution threshold of our own yet to dash against, so this
        // reads the same way a caller with none of its own does everywhere
        // else in the app: always show the value.
        Some((hz, c)) => {
            let t = radio.transmitter_offset(hz, c as f64, now);
            (
                Reading::new(t.khz, "kHz", f64::INFINITY).text(),
                Reading::new(t.ppm, "ppm", f64::INFINITY).text(),
            )
        }
        None => ("-".to_string(), "-".to_string()),
    }
}

/// `2 s` / `4 min` - how long ago, at the resolution anybody reads it at.
/// The same shape `NetCensusPanel::ago` uses, for the same reason: one bench
/// glances at a packet feed, it does not need a stopwatch.
fn ago(secs: u64) -> String {
    if secs < 90 {
        format!("{secs} s")
    } else {
        format!("{} min", secs / 60)
    }
}

/// The advertiser's address as the section's display mode shows it, or a
/// dash for a PDU type that carries none.
fn address_text(p: &BlePacket, net: &crate::state::NetState, width: usize) -> String {
    match p.adv_addr {
        Some(a) => net.show_address(a, p.tx_add_random, Some(width)),
        None => "-".to_string(),
    }
}

fn row(
    p: &BlePacket,
    state: &SdrMetrics,
    now: std::time::Instant,
    addr_w: usize,
    selected: bool,
    theme: &crate::Theme,
) -> Line<'static> {
    let radio = &state.radio;
    let (khz, ppm) = fmt_offset(p, radio, now);
    let crc_ink = if p.crc_ok {
        theme.status_ok
    } else {
        theme.status_crit
    };
    let crc_text = if p.crc_ok { "ok" } else { "bad" };
    let age = ago(now.saturating_duration_since(p.seen).as_secs());
    let mut spans = vec![crate::ui::chrome::selection_gutter(selected, theme)];
    spans.extend([
        Span::styled(
            format!("{:<CH_W$}", p.channel),
            Style::default().fg(theme.value),
        ),
        Span::raw(" "),
        Span::styled(
            format!("{:<TYPE_W$}", truncate(&p.pdu_type.label(), TYPE_W)),
            Style::default().fg(theme.value),
        ),
        Span::raw(" "),
        Span::styled(
            format!("{:<addr_w$}", address_text(p, &state.net, addr_w)),
            Style::default().fg(theme.value),
        ),
        Span::raw(" "),
        Span::styled(
            format!("{:<ATYP_W$}", if p.tx_add_random { "rnd" } else { "pub" }),
            Style::default().fg(theme.label),
        ),
        Span::raw(" "),
        Span::styled(
            format!("{:>LEN_W$}", p.length),
            Style::default().fg(theme.value),
        ),
        Span::raw(" "),
        Span::styled(format!("{crc_text:>CRC_W$}"), Style::default().fg(crc_ink)),
        Span::raw(" "),
        Span::styled(
            format!("{:>SNR_W$}", fmt_snr(p.snr_db)),
            Style::default().fg(theme.value),
        ),
        Span::raw(" "),
        Span::styled(
            format!("{:>CFO_W$}", truncate(&khz, CFO_W)),
            Style::default().fg(theme.value),
        ),
        Span::raw(" "),
        Span::styled(
            format!("{:>PPM_W$}", truncate(&ppm, PPM_W)),
            Style::default().fg(theme.value),
        ),
        Span::raw(" "),
        Span::styled(format!("{age:>AGE_W$}"), Style::default().fg(theme.label)),
    ]);
    // The selected packet in bold, its cells keeping their colours: the CRC
    // column's red or green is a reading, and a selection must not hide it.
    if selected {
        for span in spans.iter_mut().skip(1) {
            span.style = span.style.add_modifier(ratatui::style::Modifier::BOLD);
        }
    }
    Line::from(spans)
}

/// B11's own exit condition, on screen: packet counts per advertising
/// channel, with the dwell fraction stated. `None` outside survey - a radio
/// locked to one channel is not dividing its time between three, so a dwell
/// fraction would be a claim this mode does not make.
fn channel_summary(
    state: &SdrMetrics,
    theme: &crate::Theme,
    width: usize,
) -> Option<Line<'static>> {
    if state.net.mode != crate::state::NetMode::Survey {
        return None;
    }
    let n = crate::signal::ble::channel::advertising_channels_hz().len();
    let c = state.net.ble_channel_packets;
    let text = format!(
        "CH37 {}  CH38 {}  CH39 {}  (1/{n} dwell each)",
        c[0], c[1], c[2]
    );
    Some(Line::from(Span::styled(
        truncate(&text, width),
        Style::default().fg(theme.label),
    )))
}

fn truncate(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        s.to_string()
    } else {
        s.chars().take(width).collect()
    }
}

impl Panel for NetBlePacketsPanel {
    fn name(&self) -> &'static str {
        "net_ble_packets"
    }

    fn min_size(&self) -> (u16, u16) {
        (48, 6)
    }

    fn focus_key(&self) -> Option<char> {
        // The Lab timing vitals panel's letter too: no layout shows both, so
        // the letter is shared (`app::FocusKeys`), and it is the one in the
        // title.
        Some('v')
    }

    fn focus_bindings(&self) -> &'static [(&'static str, &'static str)] {
        &[("↑↓", "select a packet")]
    }

    fn chrome(&self, state: &SdrMetrics) -> PanelChrome {
        PanelChrome::new("BLE Ad_vertising")
            .stale_when(Staleness::NotStreaming)
            .tag_if(true, state.net.mode.tag())
            // The per-channel packet counts run for the session; a dropped
            // block is packets this feed never saw.
            .counts_from_feed(FeedSpan::Session)
            .shows_offsets()
            .shows_addresses()
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
        // Sized over every row the panel could show, so the header and the
        // rows agree on one width for the frame.
        let addr_w = addr_width(
            state.net.ble_packets.iter().take(inner.height as usize),
            &state.net,
            inner.width as usize,
        );
        let mut lines = vec![header_line(addr_w, theme)];

        if let Some(reason) = &state.net.ble_refused {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "not decoding".to_string(),
                Style::default().fg(theme.stale),
            )));
            for chunk in crate::ui::chrome::wrap(reason, inner.width as usize, 4) {
                lines.push(Line::from(Span::styled(
                    chunk,
                    Style::default().fg(theme.label),
                )));
            }
            f.render_widget(Paragraph::new(lines), inner);
            return;
        }

        if state.net.ble_packets.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "no packets yet".to_string(),
                Style::default().fg(theme.stale),
            )));
            lines.push(Line::from(Span::styled(
                "watching an advertising channel; nothing decoded so far this",
                Style::default().fg(theme.label),
            )));
            lines.push(Line::from(Span::styled(
                "session",
                Style::default().fg(theme.label),
            )));
            f.render_widget(Paragraph::new(lines), inner);
            return;
        }

        let summary = channel_summary(state, theme, inner.width as usize);
        let body = (inner.height as usize)
            .saturating_sub(1)
            .saturating_sub(summary.is_some() as usize);
        let now = std::time::Instant::now();
        let order: Vec<u64> = state.net.ble_packets.iter().map(|p| p.seq).collect();
        let view = &state.net.ble_view.selection;
        let cursor = view.cursor(&order);
        let start = crate::ui::widgets::table::viewport_start(
            view.first_visible,
            cursor.unwrap_or(0),
            order.len(),
            body,
        );
        for (i, p) in state
            .net
            .ble_packets
            .iter()
            .enumerate()
            .skip(start)
            .take(body)
        {
            lines.push(row(p, state, now, addr_w, Some(i) == cursor, theme));
        }
        if let Some(summary) = summary {
            lines.push(summary);
        }
        f.render_widget(Paragraph::new(lines), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::ble::pdu::PduType;
    use crate::signal::dsp::uncertainty::Uncertain;
    use crate::state::fixture::draw;
    use std::time::Instant;

    fn packet(channel: u8, crc_ok: bool) -> BlePacket {
        BlePacket {
            seq: 0,
            channel,
            pdu_type: PduType::AdvInd,
            tx_add_random: false,
            ch_sel: false,
            rx_add_random: false,
            payload: Vec::new(),
            length: 9,
            adv_addr: Some([0xaa, 0xbb, 0xcc, 0x11, 0x22, 0x33]),
            crc_ok,
            snr_db: Some(12.3),
            freq_offset_hz: Some(Uncertain::from_sigma(37_000.0, 1_200.0)),
            modulation: None,
            drift: None,
            seen: Instant::now(),
        }
    }

    /// Nothing decoding because the tuning is wrong says so, distinctly from
    /// nothing decoding because nothing has arrived yet.
    #[test]
    fn a_refusal_is_shown_rather_than_an_empty_table() {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.ble_refused = Some("not tuned to an advertising channel".to_string());
        let out = draw(NetBlePacketsPanel, 60, 10, &m).join("\n");
        assert!(out.contains("not decoding"), "{out}");
        assert!(out.contains("not tuned"), "{out}");
    }

    /// A channel correctly watched with nothing heard yet says that, plainly
    /// distinct from a refusal.
    #[test]
    fn an_empty_feed_says_nothing_decoded_yet_rather_than_a_refusal() {
        let out = draw(
            NetBlePacketsPanel,
            60,
            10,
            &SdrMetrics::fixture().streaming(),
        )
        .join("\n");
        assert!(out.contains("no packets yet"), "{out}");
        assert!(!out.contains("not decoding"), "{out}");
    }

    /// A good and a bad CRC read distinctly, and the newest packet - the
    /// front of the deque - draws first.
    #[test]
    fn packets_show_crc_status_and_arrival_order() {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.ble_packets.push_back(packet(37, true));
        m.net.ble_packets.push_front(packet(38, false));
        let out = draw(NetBlePacketsPanel, 60, 10, &m).join("\n");
        assert!(out.contains("bad"), "{out}");
        assert!(out.contains("ok"), "{out}");
        let bad_line = out.find("bad").unwrap();
        let ok_line = out.find(" ok").unwrap();
        assert!(bad_line < ok_line, "channel 38 (bad) should draw first");
    }

    /// B11's own exit condition: packet counts per advertising channel, with
    /// the dwell fraction stated, on the same screen as the packets
    /// themselves - and only while surveying, since a locked radio is not
    /// dividing its time between the three at all.
    #[test]
    fn survey_mode_shows_per_channel_counts_and_the_dwell_fraction() {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.mode = crate::state::NetMode::Survey;
        m.net.ble_channel_packets = [4, 0, 9];
        m.net.ble_packets.push_back(packet(37, true));
        let out = draw(NetBlePacketsPanel, 70, 10, &m).join("\n");
        assert!(out.contains("CH37 4"), "{out}");
        assert!(out.contains("CH38 0"), "{out}");
        assert!(out.contains("CH39 9"), "{out}");
        assert!(out.contains("1/3 dwell"), "{out}");
    }

    /// A radio locked to one channel is not dividing its time between three,
    /// so the dwell fraction the survey summary states would be a claim
    /// this mode does not make.
    #[test]
    fn lock_mode_hides_the_dwell_summary() {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.mode = crate::state::NetMode::Lock;
        m.net.ble_channel_packets = [4, 0, 9];
        m.net.ble_packets.push_back(packet(37, true));
        let out = draw(NetBlePacketsPanel, 70, 10, &m).join("\n");
        assert!(!out.contains("dwell"), "{out}");
    }

    #[test]
    fn it_fits_every_size_the_layout_can_hand_it() {
        let mut populated_survey = SdrMetrics::fixture().streaming();
        populated_survey.net.mode = crate::state::NetMode::Survey;
        populated_survey.net.ble_channel_packets = [4, 0, 9];
        for i in 0..5 {
            populated_survey
                .net
                .ble_packets
                .push_back(packet(37, i % 2 == 0));
        }
        let mut populated_lock = populated_survey.clone();
        populated_lock.net.mode = crate::state::NetMode::Lock;
        for w in 48..90u16 {
            for h in 6..20u16 {
                for m in [
                    populated_survey.clone(),
                    populated_lock.clone(),
                    SdrMetrics::fixture(),
                ] {
                    for line in draw(NetBlePacketsPanel, w, h, &m) {
                        assert!(line.chars().count() <= w as usize, "{w}x{h}: {line:?}");
                    }
                }
            }
        }
    }

    /// `n` packets, newest first, each from its own address so a row can be
    /// found by it: seq `n` is on top.
    fn feed(n: u64) -> SdrMetrics {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.ble_channel = Some(37);
        for seq in 1..=n {
            let mut p = packet(37, true);
            p.seq = seq;
            p.adv_addr = Some([0xaa, 0, 0, 0, 0, seq as u8]);
            m.net.ble_packets.push_front(p);
        }
        m.net.ble_heard = n;
        m
    }

    fn marked(rows: &[String]) -> Vec<usize> {
        rows.iter()
            .enumerate()
            .filter(|(_, l)| l.contains('\u{258c}'))
            .map(|(i, _)| i)
            .collect()
    }

    /// **The mark is on the packet, not the row.** A new arrival pushes every
    /// row down one, and the mark goes down with its packet.
    #[test]
    fn the_selection_follows_its_packet_as_new_ones_arrive() {
        let mut m = feed(5);
        m.net.ble_view.selection.selected = Some(3);
        let rows = draw(NetBlePacketsPanel, 130, 12, &m);
        let at = rows
            .iter()
            .position(|l| l.contains("00:00:00:00:03"))
            .unwrap();
        assert_eq!(marked(&rows), vec![at]);

        let mut newer = packet(37, true);
        newer.seq = 6;
        newer.adv_addr = Some([0xaa, 0, 0, 0, 0, 6]);
        m.net.ble_packets.push_front(newer);
        let rows = draw(NetBlePacketsPanel, 130, 12, &m);
        let moved = rows
            .iter()
            .position(|l| l.contains("00:00:00:00:03"))
            .unwrap();
        assert_eq!(moved, at + 1, "a row down");
        assert_eq!(marked(&rows), vec![moved], "and the mark with it");
    }

    /// Nothing selected, nothing marked; and a selection that has aged out of
    /// the ring marks nothing rather than whatever took its row.
    #[test]
    fn no_selection_and_an_aged_out_one_mark_no_row() {
        let mut m = feed(5);
        assert!(marked(&draw(NetBlePacketsPanel, 130, 12, &m)).is_empty());
        m.net.ble_view.selection.selected = Some(99);
        assert!(marked(&draw(NetBlePacketsPanel, 130, 12, &m)).is_empty());
    }

    /// The focus letter is the one the title shows.
    #[test]
    fn the_title_shows_the_focus_letter() {
        let chrome = NetBlePacketsPanel.chrome(&feed(1));
        assert!(chrome.title.contains("Ad_vertising"), "{}", chrome.title);
        assert_eq!(NetBlePacketsPanel.focus_key(), Some('v'));
    }
}
