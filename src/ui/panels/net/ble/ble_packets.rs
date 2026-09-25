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
use crate::ui::panel::{FeedSpan, Panel, PanelChrome, Staleness, Tag};
use crate::ui::widgets::reading::Reading;

pub struct NetBlePacketsPanel;

const CH_W: usize = 3;
const TYPE_W: usize = 15;
/// The narrowest the address column is drawn: a full address. It grows from
/// the spare width towards the widest address shown ([`Widths`]).
const ADDR_W: usize = crate::state::FULL_ADDRESS_WIDTH;
/// The advertised name, cut and marked beyond this: long enough for most
/// device names, and a column that grew with them would push the physics off
/// the screen.
const NAME_W: usize = 16;
const ATYP_W: usize = 4;
const LEN_W: usize = 4;
const CRC_W: usize = 4;
const SNR_W: usize = 7;
/// The offset columns at their narrowest, `-111.8 ±1.6 kHz` and
/// `-123.45 ±0.21 ppm`. They grow to the widest reading on screen
/// ([`Widths`]): a declared width is a guess, and a live room outgrew
/// this one (`-10.21 ±0.26 kH`).
const CFO_W: usize = 15;
const PPM_W: usize = 17;
const AGE_W: usize = 6;

/// Every column at its narrowest, with the single spaces between them and the
/// selection gutter every row keeps (`chrome::SELECTION_GUTTER`).
const FIXED_W: usize = crate::ui::chrome::SELECTION_GUTTER
    + CH_W
    + TYPE_W
    + ADDR_W
    + NAME_W
    + ATYP_W
    + LEN_W
    + CRC_W
    + SNR_W
    + CFO_W
    + PPM_W
    + AGE_W
    + 10;

/// This frame's widths for the columns whose contents vary, measured over the
/// rows on screen so the header and the rows agree.
struct Widths {
    addr: usize,
    cfo: usize,
    ppm: usize,
}

impl Widths {
    /// The offset columns hold their widest reading whole. The address takes
    /// what the panel can spare beyond every other column, up to the widest
    /// address shown, so a registrant's whole name appears when there is room
    /// and is cut and marked when there is not (`state::AddressDisplay::show`).
    fn of(
        visible: &[&BlePacket],
        state: &SdrMetrics,
        now: std::time::Instant,
        width: usize,
    ) -> Self {
        let (mut cfo, mut ppm) = (CFO_W, PPM_W);
        for p in visible {
            let (k, pp) = fmt_offset(p, &state.radio, now);
            cfo = cfo.max(k.chars().count());
            ppm = ppm.max(pp.chars().count());
        }
        let fixed = FIXED_W + (cfo - CFO_W) + (ppm - PPM_W);
        let want = visible
            .iter()
            .filter_map(|p| {
                p.adv_addr
                    .map(|a| state.net.address_width(a, p.tx_add_random))
            })
            .max()
            .unwrap_or(ADDR_W);
        Self {
            addr: ADDR_W + want.saturating_sub(ADDR_W).min(width.saturating_sub(fixed)),
            cfo,
            ppm,
        }
    }
}

fn header_line(w: &Widths, theme: &crate::Theme) -> Line<'static> {
    let (addr_w, cfo_w, ppm_w) = (w.addr, w.cfo, w.ppm);
    Line::from(Span::styled(
        format!(
            "{}{:<CH_W$} {:<TYPE_W$} {:<addr_w$} {:<NAME_W$} {:<ATYP_W$} {:>LEN_W$} {:>CRC_W$} {:>SNR_W$} {:>cfo_w$} {:>ppm_w$} {:>AGE_W$}",
            " ".repeat(crate::ui::chrome::SELECTION_GUTTER),
            "CH", "TYPE", "ADDRESS", "NAME", "ATYP", "LEN", "CRC", "SNR", "CFO", "PPM", "AGE"
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

/// What the packet says its device is called (`signal::ble::ad`), made safe
/// to print, cut and marked at [`NAME_W`]; `-` where it names nothing.
///
/// **Only from a packet whose CRC passed.** A failed CRC says some octet is
/// wrong and not which, and a name read from one could be a name nobody sent
/// (rule 2). An extended advertising header says `(not decoded)`: its payload
/// is a different format, and a dash would read as "no name advertised".
fn name_text(p: &BlePacket, net: &crate::state::NetState) -> String {
    use crate::signal::ble::ad;
    if p.pdu_type == crate::signal::ble::pdu::PduType::Other(0x07) {
        return "(not decoded)".to_string();
    }
    if !p.crc_ok {
        return "-".to_string();
    }
    let structures = ad::adv_data(p.pdu_type, &p.payload)
        .map(ad::parse)
        .unwrap_or_default();
    match ad::name(&structures) {
        Some((name, _)) => cut(&net.show_name(name), NAME_W),
        None => "-".to_string(),
    }
}

/// `text` in at most `width` columns, with `…` in the last where it was cut.
fn cut(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        text.to_string()
    } else {
        let mut out: String = text.chars().take(width.saturating_sub(1)).collect();
        out.push('\u{2026}');
        out
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
    w: &Widths,
    selected: bool,
    theme: &crate::Theme,
) -> Line<'static> {
    let (addr_w, cfo_w, ppm_w) = (w.addr, w.cfo, w.ppm);
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
            format!("{:<NAME_W$}", name_text(p, &state.net)),
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
        Span::styled(format!("{khz:>cfo_w$}"), Style::default().fg(theme.value)),
        Span::raw(" "),
        Span::styled(format!("{ppm:>ppm_w$}"), Style::default().fg(theme.value)),
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
/// channel, with the dwell fraction stated, and each channel's CRC pass
/// rate beside its count (net-ux-polish-plan 5.8). In LOCK the one channel
/// the radio sits on, with no dwell fraction - a radio locked to one channel
/// is not dividing its time between three, so the fraction would be a claim
/// this mode does not make.
///
/// The rate follows the frame error curve's rule
/// ([`crate::signal::ble::fer::fraction`]): a channel under ten packets has
/// its count and no rate.
fn channel_summary(
    state: &SdrMetrics,
    theme: &crate::Theme,
    width: usize,
) -> Option<Line<'static>> {
    let one = |i: usize| {
        let (n, ok) = (
            state.net.ble_channel_packets[i],
            state.net.ble_channel_crc_ok[i],
        );
        let rate = crate::signal::ble::fer::fraction(ok, n)
            .map(|r| {
                format!(
                    " {} CRC ok",
                    Reading::new(r.scale(100.0), "%", f64::INFINITY).text()
                )
            })
            .unwrap_or_default();
        format!("CH{} {n}{rate}", 37 + i)
    };
    let text = match state.net.mode {
        crate::state::NetMode::Survey => {
            let n = crate::signal::ble::channel::advertising_channels_hz().len();
            format!("{}  {}  {}  (1/{n} dwell each)", one(0), one(1), one(2))
        }
        crate::state::NetMode::Lock => {
            let ch = state.net.ble_channel?;
            let i = crate::signal::ble::channel::advertising_channel_index(ch)?;
            format!("{}  (this session)", one(i))
        }
    };
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
        &[
            ("↑↓", "select a packet"),
            ("Enter", "only this address, or all again"),
            ("H", "hold the list, or let it run"),
        ]
    }

    fn chrome(&self, state: &SdrMetrics) -> PanelChrome {
        PanelChrome::new("BLE Ad_vertising")
            .stale_when(Staleness::NotStreaming)
            .tag_if(true, state.net.mode.tag())
            .tag_if(true, Tag::Phy(state.net.ble_phy))
            // The per-channel packet counts run for the session; a dropped
            // block is packets this feed never saw.
            .counts_from_feed(FeedSpan::Session)
            .shows_offsets()
            .shows_addresses()
            .tag_if(state.net.ble_view.filter.is_some(), Tag::Filtered)
            // Held, the list is paused by the user, which the engine draws
            // cooled and never as stale; and it says what the pause costs.
            .tag_if(state.net.ble_view.held.is_some(), Tag::Paused)
            .tag_if(
                state.net.ble_behind() > 0,
                Tag::Behind(state.net.ble_behind()),
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
        let shown = state.net.ble_shown();
        let now = std::time::Instant::now();
        let summary = channel_summary(state, theme, inner.width as usize);
        let body = (inner.height as usize)
            .saturating_sub(1)
            .saturating_sub(summary.is_some() as usize);
        let order: Vec<u64> = shown.iter().map(|p| p.seq).collect();
        let view = &state.net.ble_view.selection;
        let cursor = view.cursor(&order);
        let start = crate::ui::widgets::table::viewport_start(
            view.first_visible,
            cursor.unwrap_or(0),
            order.len(),
            body,
        );
        // Sized over the rows actually on screen, so the header and the rows
        // agree on one width for the frame, and a scrolled list is measured
        // where it is scrolled to.
        let visible: Vec<&BlePacket> = shown.iter().copied().skip(start).take(body).collect();
        let widths = Widths::of(&visible, state, now, inner.width as usize);
        let mut lines = vec![header_line(&widths, theme)];

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

        if shown.is_empty() {
            // Only reachable filtered: the address has no packets in the list
            // any more, which is a fact about the ring, not a quiet device.
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "no packets from this address in the list; Enter shows all".to_string(),
                Style::default().fg(theme.label),
            )));
            f.render_widget(Paragraph::new(lines), inner);
            return;
        }

        for (i, p) in shown.iter().enumerate().skip(start).take(body) {
            lines.push(row(p, state, now, &widths, Some(i) == cursor, theme));
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
            phy: crate::signal::ble::Phy::OneM,
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

    /// An offset wider than the column's declared width widens the column
    /// rather than losing its unit, and the header's title stays over it.
    #[test]
    fn a_wide_offset_is_shown_whole_under_its_title() {
        let mut m = SdrMetrics::fixture().streaming();
        let mut wide = packet(37, true);
        wide.freq_offset_hz = Some(Uncertain::from_sigma(-499_880.0, 240.0));
        let mut narrow = packet(38, true);
        narrow.freq_offset_hz = Some(Uncertain::from_sigma(-10_210.0, 260.0));
        m.net.ble_packets.push_back(wide);
        m.net.ble_packets.push_back(narrow);
        let out = draw(NetBlePacketsPanel, 170, 8, &m);
        let text = out.join("\n");
        assert!(text.contains("-499.88 ±0.24 kHz"), "{text}");
        assert!(text.contains("-10.21 ±0.26 kHz"), "{text}");
        let ends = |line: &str, s: &str| {
            line.find(s)
                .map(|i| line[..i].chars().count() + s.chars().count())
        };
        let header = out.iter().find(|l| l.contains("CFO")).unwrap();
        let row = out.iter().find(|l| l.contains("-499.88")).unwrap();
        assert_eq!(ends(header, "CFO"), ends(row, "kHz"), "{text}");
        assert_eq!(ends(header, "PPM"), ends(row, "ppm"), "{text}");
    }

    /// No key on the list offers a PHY: LE 1M is what it hears, and the
    /// frame says so (`NetState::ble_phy` has why there is no switch).
    #[test]
    fn the_list_offers_no_phy_switch_and_names_its_phy() {
        assert!(NetBlePacketsPanel
            .focus_bindings()
            .iter()
            .all(|(k, what)| *k != "P" && !what.contains("LE 2M")));
        let out = draw(
            NetBlePacketsPanel,
            90,
            8,
            &SdrMetrics::fixture().streaming(),
        )
        .join("\n");
        assert!(out.contains("[LE 1M]"), "{out}");
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
        let out = draw(NetBlePacketsPanel, 90, 10, &m).join("\n");
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

    /// **5.8: each advertising channel's CRC pass rate beside its count**,
    /// under the error curve's rule: a rate once a channel has ten packets,
    /// its count alone before that, never a rate from a handful.
    #[test]
    fn survey_mode_shows_each_channels_crc_pass_rate_where_it_stands() {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.mode = crate::state::NetMode::Survey;
        m.net.ble_channel_packets = [400, 9, 40];
        m.net.ble_channel_crc_ok = [396, 9, 20];
        m.net.ble_packets.push_back(packet(37, true));
        let out = draw(NetBlePacketsPanel, 120, 10, &m).join("\n");
        assert!(out.contains("CH37 400 99.0"), "{out}");
        assert!(
            out.contains("CH38 9  CH39"),
            "a thin channel has no rate: {out}"
        );
        assert!(out.contains("CH39 40 50"), "{out}");
        assert!(out.contains("CRC ok"), "{out}");
        assert!(out.contains("1/3 dwell"), "{out}");
    }

    /// In LOCK the one channel the radio sits on, its count and its rate,
    /// and no dwell fraction.
    #[test]
    fn lock_mode_shows_the_one_channels_rate() {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.mode = crate::state::NetMode::Lock;
        m.net.ble_channel = Some(38);
        m.net.ble_channel_packets = [400, 30, 40];
        m.net.ble_channel_crc_ok = [396, 15, 20];
        m.net.ble_packets.push_back(packet(37, true));
        let out = draw(NetBlePacketsPanel, 90, 10, &m).join("\n");
        assert!(out.contains("CH38 30 50"), "{out}");
        assert!(!out.contains("CH37"), "{out}");
        assert!(!out.contains("CH39"), "{out}");
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
        populated_survey.net.ble_channel_crc_ok = [3, 0, 7];
        let mut populated_lock = populated_survey.clone();
        populated_lock.net.mode = crate::state::NetMode::Lock;
        populated_lock.net.ble_channel = Some(39);
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

    /// A packet whose payload is `ad` behind its address.
    fn advertising(ad: &[u8], crc_ok: bool) -> BlePacket {
        let mut p = packet(37, crc_ok);
        p.payload =
            crate::signal::ble::pdu::air_octets([0xaa, 0xbb, 0xcc, 0x11, 0x22, 0x33]).to_vec();
        p.payload.extend_from_slice(ad);
        p
    }

    /// **The name the device advertises, and only where it can be believed.**
    /// Read from its AD structures when the CRC passed; a dash when it failed,
    /// however readable the octets; control characters never reach the
    /// terminal; and an extended advertising header says it was not decoded.
    #[test]
    fn the_name_column_shows_what_was_advertised_and_can_be_believed() {
        let name = [0x05, 0x09, b'S', b'e', b'n', b's'];
        assert_eq!(
            name_text(
                &advertising(&name, true),
                &crate::state::NetState::default()
            ),
            "Sens"
        );
        assert_eq!(
            name_text(
                &advertising(&name, false),
                &crate::state::NetState::default()
            ),
            "-"
        );
        let hostile = [0x04, 0x09, b'A', 0x1b, b'B'];
        assert_eq!(
            name_text(
                &advertising(&hostile, true),
                &crate::state::NetState::default()
            ),
            "A\u{fffd}B"
        );
        let long = [
            0x15, 0x09, b'A', b'A', b'A', b'A', b'A', b'A', b'A', b'A', b'A', b'A', b'A', b'A',
            b'A', b'A', b'A', b'A', b'A', b'A', b'A', b'A',
        ];
        let cut_name = name_text(
            &advertising(&long, true),
            &crate::state::NetState::default(),
        );
        assert_eq!(cut_name.chars().count(), NAME_W);
        assert!(cut_name.ends_with('\u{2026}'));
        assert_eq!(
            name_text(
                &advertising(&[0x02, 0x01, 0x06], true),
                &crate::state::NetState::default()
            ),
            "-"
        );

        let mut ext = packet(37, true);
        ext.pdu_type = PduType::Other(0x07);
        assert_eq!(
            name_text(&ext, &crate::state::NetState::default()),
            "(not decoded)"
        );
        let mut m = feed(0);
        m.net.ble_packets.push_front(ext);
        let out = draw(NetBlePacketsPanel, 130, 8, &m).join("\n");
        assert!(out.contains("ADV_EXT_IND"), "{out}");
        assert!(out.contains("(not decoded)"), "{out}");
    }

    /// **Filtered, the list is one address and the frame says so.**
    #[test]
    fn a_filtered_list_shows_one_address_and_says_so() {
        let mut m = feed(4);
        m.net.ble_view.filter = Some([0xaa, 0, 0, 0, 0, 2]);
        let out = draw(NetBlePacketsPanel, 130, 10, &m);
        assert!(out[0].contains("[FILTERED]"), "{}", out[0]);
        let text = out.join("\n");
        assert!(text.contains("00:00:00:00:02"), "{text}");
        assert!(!text.contains("00:00:00:00:03"), "{text}");

        // An address gone from the ring says that, rather than a quiet room.
        m.net.ble_view.filter = Some([0xaa, 0, 0, 0, 0, 9]);
        let gone = draw(NetBlePacketsPanel, 130, 10, &m).join("\n");
        assert!(gone.contains("no packets from this address"), "{gone}");
    }

    /// **Held, the list stops and counts what it is missing**, and says it is
    /// paused rather than stale; the live ring goes on underneath.
    #[test]
    fn a_held_list_stays_put_and_counts_what_it_misses() {
        let mut m = feed(3);
        m.net.ble_view.held = Some((m.net.ble_packets.clone(), m.net.ble_heard));
        let mut newer = packet(37, true);
        newer.seq = 4;
        newer.adv_addr = Some([0xaa, 0, 0, 0, 0, 4]);
        m.net.ble_packets.push_front(newer);
        m.net.ble_heard = 4;

        let out = draw(NetBlePacketsPanel, 130, 10, &m);
        assert!(out[0].contains("[PAUSED]"), "{}", out[0]);
        assert!(out[0].contains("[+1 NEW]"), "{}", out[0]);
        assert!(!out[0].contains("STALE"), "{}", out[0]);
        let text = out.join("\n");
        assert!(!text.contains("00:00:00:00:04"), "held: {text}");
        assert!(text.contains("00:00:00:00:03"), "{text}");
    }

    /// **One device, one name, in every panel**: an RPA whose manufacturer
    /// data named Apple reads `Apple·mfr` in the packet list and in the
    /// census alike, from the section's one record of it.
    #[test]
    fn the_list_and_the_census_name_a_random_device_by_its_company_alike() {
        let rpa = [0x4a, 0x11, 0x22, 0x33, 0x09, 0xbe];
        let mut m = feed(0);
        m.net.address_display = crate::state::AddressDisplay::Oui;
        m.net.advertised.entry(rpa).or_default().company = Some(0x004C);
        let mut p = packet(37, true);
        p.seq = 1;
        p.adv_addr = Some(rpa);
        p.tx_add_random = true;
        m.net.ble_packets.push_front(p);
        m.net
            .census
            .devices
            .push(crate::signal::net::census::Device::heard(
                rpa,
                true,
                Instant::now(),
            ));

        let list = draw(NetBlePacketsPanel, 140, 8, &m).join("\n");
        let census = draw(crate::ui::NetCensusPanel, 120, 10, &m).join("\n");
        for out in [&list, &census] {
            assert!(out.contains("Apple\u{00b7}mfr ..09:be"), "{out}");
        }
    }
}
