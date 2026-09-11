// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `NetBlePacketsPanel` - what this device is advertising.
//!
//! B6's exit condition, on screen: real advertising channel PDUs, CRC-checked,
//! newest first. No sorting and no cursor - a live packet feed is already in
//! the order that matters, arrival order, and B6 does not yet decode enough
//! (no SNR, no frequency offset - both B7's) to make a second ordering useful.
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

use crate::state::{BlePacket, SdrMetrics};
use crate::ui::panel::{Panel, PanelChrome, Staleness};

pub struct NetBlePacketsPanel;

const CH_W: usize = 3;
const TYPE_W: usize = 15;
const ADDR_W: usize = 17;
const ATYP_W: usize = 4;
const LEN_W: usize = 4;
const CRC_W: usize = 4;
const AGE_W: usize = 6;

fn header_line(theme: &crate::Theme) -> Line<'static> {
    Line::from(Span::styled(
        format!(
            "{:<CH_W$} {:<TYPE_W$} {:<ADDR_W$} {:<ATYP_W$} {:>LEN_W$} {:>CRC_W$} {:>AGE_W$}",
            "CH", "TYPE", "ADDRESS", "ATYP", "LEN", "CRC", "AGE"
        ),
        Style::default().fg(theme.label),
    ))
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

fn address_text(addr: Option<[u8; 6]>) -> String {
    match addr {
        Some(a) => a
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join(":"),
        None => "-".to_string(),
    }
}

fn row(p: &BlePacket, now: std::time::Instant, theme: &crate::Theme) -> Line<'static> {
    let crc_ink = if p.crc_ok {
        theme.status_ok
    } else {
        theme.status_crit
    };
    let crc_text = if p.crc_ok { "ok" } else { "bad" };
    let age = ago(now.saturating_duration_since(p.seen).as_secs());
    Line::from(vec![
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
            format!("{:<ADDR_W$}", address_text(p.adv_addr)),
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
        Span::styled(format!("{age:>AGE_W$}"), Style::default().fg(theme.label)),
    ])
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

    fn chrome(&self, state: &SdrMetrics) -> PanelChrome {
        PanelChrome::new("BLE Advertising")
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
        let mut lines = vec![header_line(theme)];

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

        let body = (inner.height as usize).saturating_sub(1);
        let now = std::time::Instant::now();
        for p in state.net.ble_packets.iter().take(body) {
            lines.push(row(p, now, theme));
        }
        f.render_widget(Paragraph::new(lines), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::ble::pdu::PduType;
    use crate::state::fixture::draw;
    use std::time::Instant;

    fn packet(channel: u8, crc_ok: bool) -> BlePacket {
        BlePacket {
            channel,
            pdu_type: PduType::AdvInd,
            tx_add_random: false,
            length: 9,
            adv_addr: Some([0xaa, 0xbb, 0xcc, 0x11, 0x22, 0x33]),
            crc_ok,
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

    #[test]
    fn it_fits_every_size_the_layout_can_hand_it() {
        let mut populated = SdrMetrics::fixture().streaming();
        for i in 0..5 {
            populated.net.ble_packets.push_back(packet(37, i % 2 == 0));
        }
        for w in 48..90u16 {
            for h in 6..20u16 {
                for m in [populated.clone(), SdrMetrics::fixture()] {
                    for line in draw(NetBlePacketsPanel, w, h, &m) {
                        assert!(line.chars().count() <= w as usize, "{w}x{h}: {line:?}");
                    }
                }
            }
        }
    }
}
