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
use crate::state::{BlePacket, ExtInfo, SdrMetrics};
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

/// The extended header's lines: the event and set, the advertiser and its
/// name, its power, where it came from, and what became of its own AuxPtr.
fn ext_lines(
    p: &BlePacket,
    ext: &ExtInfo,
    state: &SdrMetrics,
    theme: &crate::Theme,
) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    out.push(row(
        "event",
        plain(super::ext_text::event(&ext.header), theme),
        theme,
    ));
    if let Some(a) = ext.header.adv_a {
        let mut who = plain(state.net.show_address(a, p.tx_add_random, None), theme);
        let structures = crate::signal::ble::ad::parse(&ext.header.adv_data);
        if let Some((name, _)) = crate::signal::ble::ad::name(&structures) {
            who.push(Span::styled(
                format!(" · {}", state.net.show_name(name)),
                Style::default().fg(theme.value),
            ));
        }
        out.push(row("from", who, theme));
    }
    if let Some(dbm) = ext.header.tx_power_dbm {
        out.push(row("TxPower", plain(format!("{dbm} dBm"), theme), theme));
    }
    if let Some(said) = super::ext_text::pointed(ext, state.net.coded_list()) {
        out.push(row("pointed", quiet(&said, theme), theme));
    }
    out.push(row(
        "aux",
        plain(super::ext_text::aux(ext, state.net.coded_list()), theme),
        theme,
    ));
    out
}

fn lines(
    p: &BlePacket,
    state: &SdrMetrics,
    width: usize,
    theme: &crate::Theme,
) -> Vec<Line<'static>> {
    let scheme = match p.phy {
        Phy::Coded(c) => c.label(),
        _ => "not LE Coded",
    };
    // An extended PDU's type code is one for three PDUs; its role says which.
    let name = p
        .ext
        .as_ref()
        .map_or(p.pdu_type.label(), |e| e.role.label().to_string());
    let mut out = vec![row(
        "type",
        plain(format!("{name} · LE Coded {scheme}"), theme),
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
    if let Some(ext) = &p.ext {
        out.extend(ext_lines(p, ext, state, theme));
    }
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
            // No-break spaces hold each figure to its unit across a wrap.
            let said = format!(
                "{:.1}\u{a0}% of df1max above 185\u{a0}kHz (the suite asks 99.9\u{a0}%)",
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

/// Nothing selected: the session's account, the decoder's funnel and every
/// AuxPtr by how it ended, which the health panel also keeps but which this
/// view does not show.
fn session_lines(state: &SdrMetrics, width: usize, theme: &crate::Theme) -> Vec<Line<'static>> {
    let f = state.net.health.coded;
    let a = state.net.health.aux;
    let mut promises = format!(
        "{} heard · {} missed · {} not in view · {} feed lost",
        a.heard, a.missed, a.not_in_view, a.feed_lost
    );
    if a.none_promised > 0 {
        promises += &format!(" · {} none promised", a.none_promised);
    }
    if a.refused > 0 {
        promises += &format!(" · {} refused", a.refused);
    }
    let mut out = vec![Line::from(Span::styled(
        format!(
            "{} LE Coded packets heard this session",
            state.net.coded_heard
        ),
        Style::default().fg(theme.value),
    ))];
    let mut block = |label: &str, text: String| {
        for (i, chunk) in crate::ui::chrome::wrap(&text, width.saturating_sub(LABEL_W + 1), 3)
            .into_iter()
            .enumerate()
        {
            out.push(row(
                if i == 0 { label } else { "" },
                plain(chunk, theme),
                theme,
            ));
        }
    };
    block(
        "decoder",
        format!(
            "{} triggers · {} CRC ok · {} CRC failed · {} gave up",
            f.triggered, f.decoded, f.crc_failed, f.gave_up
        ),
    );
    block("AuxPtr", promises);
    out.push(Line::from(Span::styled(
        "select a packet in the list (v, then ↑↓) to read it here".to_string(),
        Style::default().fg(theme.label),
    )));
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
        if let Some(Ok(p)) = subject(state) {
            chrome = chrome.shows_offsets();
            if p.ext.as_ref().is_some_and(|e| e.header.adv_a.is_some()) {
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
        let text = match subject(state) {
            Some(Ok(p)) => lines(p, state, width, theme),
            Some(Err(why)) => vec![Line::from(Span::styled(
                why.to_string(),
                Style::default().fg(theme.stale),
            ))],
            None => session_lines(state, width, theme),
        };
        f.render_widget(Paragraph::new(text), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::ble::aux::AuxOutcome;
    use crate::signal::ble::coded::Coding;
    use crate::signal::ble::pdu::PduType;
    use crate::state::fixture::draw;
    use crate::state::{CodedFacts, ExtInfo, ExtRole};

    /// An LE Coded packet, extended, in `role`, with `aux` its AuxPtr's
    /// outcome; the header from `payload` (`ext::parse`).
    fn packet(
        seq: u64,
        ch: u8,
        coding: Coding,
        payload: Vec<u8>,
        role: ExtRole,
        aux: AuxOutcome,
    ) -> BlePacket {
        let header = crate::signal::ble::ext::parse(&payload).unwrap();
        BlePacket {
            phy: Phy::Coded(coding),
            seq,
            channel: ch,
            pdu_type: PduType::Other(0x07),
            ch_sel: false,
            tx_add_random: true,
            rx_add_random: false,
            length: payload.len() as u8,
            adv_addr: header.adv_a,
            payload,
            crc_ok: true,
            snr_db: Some(18.0),
            freq_offset_hz: None,
            modulation: None,
            drift: None,
            seen: std::time::Instant::now(),
            coded: Some(CodedFacts {
                fec_repairs: 2,
                reading: None,
            }),
            ext: Some(ExtInfo { header, role, aux }),
        }
    }

    /// ADI SID 3 DID 0x123, AuxPtr to channel 9, LE Coded.
    fn adv_ext_ind() -> Vec<u8> {
        let v: u32 = 9 | 1 << 6 | 100 << 8 | 0b010 << 21;
        vec![
            6,
            0b0001_1000,
            0x23,
            0x31,
            v as u8,
            (v >> 8) as u8,
            (v >> 16) as u8,
        ]
    }

    /// AdvA, ADI, TxPower -10 dBm; AdvData: the name "Pixel".
    fn aux_adv_ind() -> Vec<u8> {
        let mut p = vec![
            10,
            0b0100_1001,
            0x11,
            0x22,
            0x33,
            0x44,
            0x55,
            0x66,
            0x23,
            0x31,
            0xF6,
        ];
        p.extend([6, 0x09, b'P', b'i', b'x', b'e', b'l']);
        p
    }

    fn selected(packets: Vec<BlePacket>, seq: u64) -> SdrMetrics {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.coded_heard = packets.len() as u64;
        for p in packets.into_iter().rev() {
            m.net.coded_packets.push_front(p);
        }
        m.net.coded_view.selection.selected = Some(seq);
        m
    }

    /// **What became of an ADV_EXT_IND's AuxPtr, said as it was.**
    #[test]
    fn the_detail_tells_an_adv_ext_inds_aux() {
        for (outcome, said) in [
            (
                AuxOutcome::Heard {
                    seq: 2,
                    after_us: 3001.4,
                },
                "aux on ch 9, LE Coded, heard 3.00 ms later",
            ),
            (
                AuxOutcome::NotInView,
                "aux on ch 9: not in the radio's view",
            ),
            (AuxOutcome::Missed, "aux on ch 9: listened, not heard"),
            (
                AuxOutcome::FeedLost,
                "aux on ch 9: its samples were not held",
            ),
            (AuxOutcome::Pending, "aux on ch 9: waiting for its window"),
            (AuxOutcome::NonePromised, "no auxiliary packet promised"),
            (
                AuxOutcome::Refused("a reserved Aux PHY"),
                "aux not followed: a reserved Aux PHY",
            ),
        ] {
            let m = selected(
                vec![packet(
                    1,
                    38,
                    Coding::S8,
                    adv_ext_ind(),
                    ExtRole::AdvExt,
                    outcome,
                )],
                1,
            );
            let text = draw(NetCodedDetailPanel, 70, 20, &m).join("\n");
            assert!(text.contains("ADV_EXT_IND · LE Coded S8"), "{text}");
            assert!(text.contains(said), "{outcome:?}: {text}");
        }
    }

    /// The AuxPtr promises only "LE Coded"; the packet heard says which
    /// scheme it came in, and the line names that one.
    #[test]
    fn a_heard_aux_names_the_scheme_it_came_in() {
        let heard = AuxOutcome::Heard {
            seq: 2,
            after_us: 3010.0,
        };
        let m = selected(
            vec![
                packet(1, 38, Coding::S8, adv_ext_ind(), ExtRole::AdvExt, heard),
                packet(
                    2,
                    9,
                    Coding::S2,
                    aux_adv_ind(),
                    ExtRole::AuxAdv {
                        superior_seq: Some(1),
                    },
                    AuxOutcome::NonePromised,
                ),
            ],
            1,
        );
        let text = draw(NetCodedDetailPanel, 70, 20, &m).join("\n");
        assert!(
            text.contains("aux on ch 9, LE Coded S2, heard 3.01 ms later"),
            "{text}"
        );
    }

    /// An auxiliary packet names the packet that pointed to it.
    #[test]
    fn an_aux_row_names_its_superior() {
        let m = selected(
            vec![
                packet(
                    2,
                    9,
                    Coding::S8,
                    aux_adv_ind(),
                    ExtRole::AuxAdv {
                        superior_seq: Some(1),
                    },
                    AuxOutcome::NonePromised,
                ),
                packet(
                    1,
                    38,
                    Coding::S8,
                    adv_ext_ind(),
                    ExtRole::AdvExt,
                    AuxOutcome::Heard {
                        seq: 2,
                        after_us: 3000.0,
                    },
                ),
            ],
            2,
        );
        let text = draw(NetCodedDetailPanel, 70, 20, &m).join("\n");
        assert!(text.contains("AUX_ADV_IND · LE Coded S8"), "{text}");
        assert!(text.contains("from the ADV_EXT_IND on ch 38"), "{text}");
    }

    /// The extended header, field by field: the event, the set, the
    /// advertiser, its power and its name.
    #[test]
    fn the_detail_shows_the_extended_header() {
        let m = selected(
            vec![packet(
                2,
                9,
                Coding::S8,
                aux_adv_ind(),
                ExtRole::AuxAdv { superior_seq: None },
                AuxOutcome::NonePromised,
            )],
            2,
        );
        let text = draw(NetCodedDetailPanel, 70, 20, &m).join("\n");
        assert!(
            text.contains("non-connectable, non-scannable · SID 3 · DID 0x123"),
            "{text}"
        );
        assert!(text.contains("66:55:44:33:22:11"), "{text}");
        assert!(text.contains("Pixel"), "{text}");
        assert!(text.contains("-10 dBm"), "{text}");
        assert!(text.contains("FEC repaired 2 symbols"), "{text}");
    }

    /// S=2 has none of the suite's figures, and says why.
    #[test]
    fn the_detail_refuses_modulation_on_s2() {
        let m = selected(
            vec![packet(
                1,
                38,
                Coding::S2,
                adv_ext_ind(),
                ExtRole::AdvExt,
                AuxOutcome::Missed,
            )],
            1,
        );
        let text = draw(NetCodedDetailPanel, 70, 20, &m).join("\n");
        assert!(text.contains("not defined for S=2"), "{text}");
        assert!(!text.contains("df1 avg"), "{text}");
    }

    /// The share line wraps, but never between a figure and its unit: at the
    /// panel's width on a 191-column screen it once read "99.9" with its "%"
    /// alone on the next row.
    #[test]
    fn the_share_keeps_its_figures_and_units_together() {
        let mut p = packet(
            1,
            38,
            Coding::S8,
            adv_ext_ind(),
            ExtRole::AdvExt,
            AuxOutcome::Missed,
        );
        p.coded = Some(CodedFacts {
            fec_repairs: 0,
            reading: Some(crate::signal::net::measure::CodedReading {
                snr_db: Some(29.5),
                modulation: Some(crate::signal::ble::measure::CodedModulation {
                    delta_f1_avg_hz: crate::signal::dsp::uncertainty::Uncertain::from_sigma(
                        250_550.0, 250.0,
                    ),
                    share_f1max_above_limit: 1.0,
                }),
                initial: None,
                drift: None,
            }),
        });
        let m = selected(vec![p], 1);
        for width in 60..=80 {
            let text = draw(NetCodedDetailPanel, width, 24, &m).join("\n");
            assert!(text.contains("100.0\u{a0}%"), "{width}: {text}");
            assert!(text.contains("99.9\u{a0}%"), "{width}: {text}");
        }
    }

    /// Nothing selected: the session's funnel and every AuxPtr's ending.
    #[test]
    fn with_nothing_selected_the_detail_counts_the_funnel_and_the_promises() {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.coded_heard = 40;
        m.net.health.coded = crate::signal::ble::receive::Funnel {
            triggered: 45,
            decoded: 40,
            crc_failed: 3,
            gave_up: 2,
        };
        let aux = &mut m.net.health.aux;
        aux.heard = 7;
        aux.missed = 2;
        aux.not_in_view = 25;
        aux.feed_lost = 1;
        let text = draw(NetCodedDetailPanel, 70, 20, &m).join("\n");
        assert!(
            text.contains("45 triggers · 40 CRC ok · 3 CRC failed · 2 gave up"),
            "{text}"
        );
        assert!(
            text.contains("7 heard · 2 missed · 25 not in view · 1 feed lost"),
            "{text}"
        );
    }
}
