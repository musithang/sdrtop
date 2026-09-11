// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `NetBtRfPanel` - is this transmitter's own modulation any good?
//!
//! B8's exit condition, on screen: modulation index, delta-f1 average,
//! delta-f2 maximum and their ratio, each against its stated limit, for the
//! most recently decoded advertising packet. Design section 9.1's idiom B
//! (`ui::widgets::limit`) is drawn here for the first time - see that
//! module's own doc for the row shape and why the word "pass" never
//! appears.
//!
//! **The three states `net_ble_packets` already established, reused
//! here.** Not decoding, nothing decoded yet, and - a fourth this panel
//! adds of its own - a real packet whose own random content happened not
//! to contain a settled run of one of the two kinds this measurement needs
//! (`signal::ble::measure`'s own doc explains which). All three are
//! refusals, never a zeroed or invented row.

use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::signal::ble::measure::ModulationQuality;
use crate::signal::dsp::uncertainty::Uncertain;
use crate::state::SdrMetrics;
use crate::ui::panel::{Panel, PanelChrome, Staleness};
use crate::ui::widgets::limit::{Limit, LimitRow, RowWidths};
use crate::ui::widgets::reading::Reading;

pub struct NetBtRfPanel;

/// Design section 2.1: "roughly 0.45 to 0.55 for BLE". Cross-checked
/// informally, not read from a primary copy of the specification this
/// session - the same standing `signal::ble::channel`'s own table has, and
/// the same figure `ui::widgets::limit`'s own tests were written against
/// before this panel gave the widget a real consumer.
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

/// Recalled from the BLE RF-PHY test specification's own delta-f2 floor,
/// **not checked against a primary source this session** - design section
/// 6's own "facts to verify before building" table lists this exact figure
/// as open. Kept as a stated limit rather than left out, on the same
/// reasoning `signal::ble::channel`'s table gives for shipping a
/// cross-checked-but-unread number: real, and owed a real read before
/// anything downstream trusts it to the last kHz.
const DELTA_F2_MAX_FLOOR_KHZ: Limit = Limit::Min(185.0);

/// Recalled from the same source and under the same caveat as
/// [`DELTA_F2_MAX_FLOOR_KHZ`]: the specification's own ratio requirement
/// between delta-f2 and delta-f1 averages.
const RATIO_FLOOR: Limit = Limit::Min(0.8);

/// How much uncertainty each reading can carry before it dashes rather than
/// prints - a judgement call in the absence of a specification-stated
/// figure, made the same way `Uncertain::is_resolved`'s own doc asks for:
/// a fraction of the band width each limit states, generous enough that an
/// ordinarily noisy real packet still shows a number rather than a dash on
/// every row.
const MOD_INDEX_RESOLUTION: f64 = 0.02;
const DELTA_F1_RESOLUTION_KHZ: f64 = 10.0;
const RATIO_RESOLUTION: f64 = 0.1;

fn rows(q: &ModulationQuality) -> Vec<LimitRow<'static>> {
    vec![
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
            DELTA_F1_BAND_KHZ,
        ),
        LimitRow::new(
            "df2 max",
            // Not an `Uncertain` this measurement carries - see
            // `ModulationQuality::delta_f2_max_hz`'s own doc for why a
            // maximum gets none - so this reads it as exact rather than
            // inventing a sigma, and `f64::INFINITY` so it is never dashed
            // for a reason it did not earn.
            Reading::new(
                Uncertain::exact(q.delta_f2_max_hz * 0.001),
                "kHz",
                f64::INFINITY,
            ),
            DELTA_F2_MAX_FLOOR_KHZ,
        ),
        LimitRow::new(
            "df2/df1",
            Reading::new(q.ratio, "", RATIO_RESOLUTION),
            RATIO_FLOOR,
        ),
    ]
}

/// The widest bar that still keeps every row inside `width` columns.
///
/// A row's overall length depends on the margin text too, which varies with
/// the actual value - not just the label, value and sigma columns
/// [`RowWidths::fit`] measures - so this checks the real rendered length
/// rather than budgeting columns by hand and hoping.
fn fit(rows: &[LimitRow], width: usize) -> RowWidths {
    let mut bar = width;
    loop {
        let w = RowWidths::fit(rows, bar);
        let longest = rows
            .iter()
            .map(|r| r.text(w).chars().count())
            .max()
            .unwrap_or(0);
        if longest <= width || bar == 0 {
            return w;
        }
        bar -= 1;
    }
}

impl Panel for NetBtRfPanel {
    fn name(&self) -> &'static str {
        "net_bt_rf"
    }

    fn min_size(&self) -> (u16, u16) {
        (28, 4)
    }

    fn chrome(&self, state: &SdrMetrics) -> PanelChrome {
        PanelChrome::new("Modulation Quality")
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

        let Some(latest) = state.net.ble_packets.front() else {
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    "no packets yet".to_string(),
                    Style::default().fg(theme.stale),
                ))),
                inner,
            );
            return;
        };

        let Some(q) = latest.modulation else {
            f.render_widget(
                Paragraph::new(vec![
                    Line::from(Span::styled(
                        "not measured".to_string(),
                        Style::default().fg(theme.stale),
                    )),
                    Line::from(Span::styled(
                        "latest packet too short, or too short a run of either kind",
                        Style::default().fg(theme.label),
                    )),
                ]),
                inner,
            );
            return;
        };

        let rows = rows(&q);
        let w = fit(&rows, width);
        let lines: Vec<Line> = rows.iter().map(|r| Line::from(r.spans(theme, w))).collect();
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

    fn quality(deviation_hz: f64) -> ModulationQuality {
        ModulationQuality {
            delta_f1_avg_hz: Uncertain::from_sigma(deviation_hz, deviation_hz * 0.01),
            delta_f2_max_hz: deviation_hz * 0.9,
            modulation_index: Uncertain::from_sigma(2.0 * deviation_hz / 1_000_000.0, 0.005),
            ratio: Uncertain::from_sigma(0.9, 0.02),
        }
    }

    fn packet(modulation: Option<ModulationQuality>) -> BlePacket {
        BlePacket {
            channel: 37,
            pdu_type: PduType::AdvInd,
            tx_add_random: false,
            length: 9,
            adv_addr: Some([0xaa, 0xbb, 0xcc, 0x11, 0x22, 0x33]),
            crc_ok: true,
            snr_db: Some(12.0),
            freq_offset_hz: None,
            modulation,
            seen: Instant::now(),
        }
    }

    #[test]
    fn a_refusal_is_shown_rather_than_an_empty_panel() {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.ble_refused = Some("not tuned to an advertising channel".to_string());
        let out = draw(NetBtRfPanel, 40, 8, &m).join("\n");
        assert!(out.contains("not decoding"), "{out}");
        assert!(out.contains("not tuned"), "{out}");
    }

    #[test]
    fn an_empty_feed_says_nothing_decoded_yet() {
        let out = draw(NetBtRfPanel, 40, 8, &SdrMetrics::fixture().streaming()).join("\n");
        assert!(out.contains("no packets yet"), "{out}");
    }

    #[test]
    fn a_packet_with_nothing_measured_refuses_rather_than_inventing_rows() {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.ble_packets.push_back(packet(None));
        let out = draw(NetBtRfPanel, 40, 8, &m).join("\n");
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
        let out = draw(NetBtRfPanel, 70, 8, &m).join("\n");
        assert!(out.contains("Mod index"), "{out}");
        assert!(out.contains("df1 avg"), "{out}");
        assert!(out.contains("df2 max"), "{out}");
        assert!(out.contains("df2/df1"), "{out}");
        let lower = out.to_ascii_lowercase();
        for word in ["pass", "fail"] {
            assert!(!lower.contains(word), "{word} in {out:?}");
        }
    }

    #[test]
    fn it_fits_every_size_the_layout_can_hand_it() {
        let mut populated = SdrMetrics::fixture().streaming();
        populated
            .net
            .ble_packets
            .push_back(packet(Some(quality(250_000.0))));
        for w in 20..90u16 {
            for h in 4..16u16 {
                for m in [populated.clone(), SdrMetrics::fixture()] {
                    for line in draw(NetBtRfPanel, w, h, &m) {
                        assert!(line.chars().count() <= w as usize, "{w}x{h}: {line:?}");
                    }
                }
            }
        }
    }
}
