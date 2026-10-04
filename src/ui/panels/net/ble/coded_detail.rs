// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `NetCodedDetailPanel` - one LE Coded packet, beside the LE Coded list.
//!
//! What only a Coded packet has, first: the scheme its Coding Indicator
//! named and how many symbols the FEC decoder overruled to get its bits.
//! Then what the measurement path read of it as RFPHY.TS defines it for
//! LE Coded (S=8): the preamble's f0, Δf1, and the carrier through the
//! payload; an S=2 packet has none of those, and says so rather than
//! borrowing LE 1M's definitions. Then its payload, as sent.

use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::signal::ble::Phy;
use crate::state::{BlePacket, SdrMetrics};
use crate::ui::panel::{FeedSpan, Panel, PanelChrome, Staleness, Tag};
use crate::ui::widgets::reading::Reading;

pub struct NetCodedDetailPanel;

/// The label column's width.
const LABEL_W: usize = 9;

/// The selected packet, or why it cannot be shown; `None` with nothing
/// selected.
fn subject(state: &SdrMetrics) -> Option<Result<&BlePacket, &'static str>> {
    let seq = state.net.coded_view.selection.selected?;
    Some(
        state
            .net
            .coded_shown()
            .into_iter()
            .find(|p| p.seq == seq)
            .ok_or("the selected packet has left the list"),
    )
}

fn row(label: &str, value: Vec<Span<'static>>, theme: &crate::Theme) -> Line<'static> {
    let mut spans = vec![crate::ui::chrome::field(label, LABEL_W, theme)];
    spans.extend(value);
    Line::from(spans)
}

fn plain(text: String, theme: &crate::Theme) -> Vec<Span<'static>> {
    vec![Span::styled(text, Style::default().fg(theme.value))]
}

fn quiet(text: &str, theme: &crate::Theme) -> Vec<Span<'static>> {
    vec![Span::styled(
        text.to_string(),
        Style::default().fg(theme.label),
    )]
}

fn lines(p: &BlePacket, width: usize, theme: &crate::Theme) -> Vec<Line<'static>> {
    let scheme = match p.phy {
        Phy::Coded(c) => c.label(),
        _ => "not LE Coded",
    };
    let mut out = vec![row(
        "type",
        plain(format!("{} · LE Coded {scheme}", p.pdu_type.label()), theme),
        theme,
    )];
    let crc = if p.crc_ok { "CRC ok" } else { "CRC failed" };
    let repairs = p.coded.as_ref().map_or(String::new(), |c| {
        format!(" · FEC repaired {} symbols", c.fec_repairs)
    });
    out.push(row(
        "packet",
        plain(format!("ch {} · {crc}{repairs}", p.channel), theme),
        theme,
    ));
    let reading = p.coded.as_ref().and_then(|c| c.reading.as_ref());
    out.push(row(
        "SNR",
        match reading.and_then(|r| r.snr_db) {
            Some(db) => plain(format!("{db:.1} dB"), theme),
            None => quiet("not read: its samples were no longer held", theme),
        },
        theme,
    ));
    if let Phy::Coded(crate::signal::ble::coded::Coding::S2) = p.phy {
        out.push(row(
            "carrier",
            quiet(
                "not defined for S=2: the test suite defines LE Coded's for S=8",
                theme,
            ),
            theme,
        ));
    } else {
        let drift = reading.and_then(|r| r.drift);
        let modulation = reading.and_then(|r| r.modulation);
        match drift {
            Some(d) => {
                let mut f0 =
                    Reading::new(d.initial_hz.scale(1e-3), "kHz", f64::INFINITY).spans(theme);
                f0.push(Span::styled(
                    " from the channel's centre".to_string(),
                    Style::default().fg(theme.label),
                ));
                out.push(row("f0", f0, theme));
            }
            None => out.push(row(
                "f0",
                quiet("not read: too short, or its samples no longer held", theme),
                theme,
            )),
        }
        let rows = super::ble_detail::coded_rows(modulation.as_ref(), drift.as_ref());
        out.extend(super::ble_detail::limit_lines(&rows, width, theme));
        if let Some(m) = modulation {
            let said = format!(
                "{:.1} % of df1max above 185 kHz (the suite asks 99.9 %)",
                m.share_f1max_above_limit * 100.0
            );
            for chunk in crate::ui::chrome::wrap(&said, width.saturating_sub(LABEL_W + 1), 2) {
                out.push(row("", quiet(&chunk, theme), theme));
            }
        }
    }
    let hex: Vec<String> = p.payload.iter().map(|b| format!("{b:02x}")).collect();
    let room = width.saturating_sub(LABEL_W + 1).max(3) / 3;
    for (i, chunk) in hex.chunks(room.max(1)).enumerate() {
        out.push(row(
            if i == 0 { "payload" } else { "" },
            plain(chunk.join(" "), theme),
            theme,
        ));
    }
    out
}

impl Panel for NetCodedDetailPanel {
    fn name(&self) -> &'static str {
        "net_coded_detail"
    }

    fn min_size(&self) -> (u16, u16) {
        (28, 4)
    }

    fn chrome(&self, state: &SdrMetrics) -> PanelChrome {
        // With nothing selected it counts the session's packets, which a
        // dropped block is packets this feed never saw.
        let mut chrome = PanelChrome::new("Packet Detail")
            .stale_when(Staleness::NotStreaming)
            .tag_if(true, state.net.mode.tag())
            .tag_if(true, Tag::Listening("LE CODED"))
            .counts_from_feed(FeedSpan::Session);
        if matches!(subject(state), Some(Ok(_))) {
            chrome = chrome.shows_offsets();
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
        let text = match subject(state) {
            Some(Ok(p)) => lines(p, width, theme),
            Some(Err(why)) => vec![Line::from(Span::styled(
                why.to_string(),
                Style::default().fg(theme.stale),
            ))],
            None => {
                let heard = state.net.coded_heard;
                vec![
                    Line::from(Span::styled(
                        format!("{heard} LE Coded packets heard this session"),
                        Style::default().fg(theme.value),
                    )),
                    Line::from(Span::styled(
                        "select one in the list (v, then ↑↓) to read it here".to_string(),
                        Style::default().fg(theme.label),
                    )),
                ]
            }
        };
        f.render_widget(Paragraph::new(text), inner);
    }
}
