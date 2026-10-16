// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `NetWifiLedgerPanel` - what spent this channel's time: a row a format
//! and rate, under the Airtime hero.
//!
//! **Rows in the standards' own order** (802.11b, a/g, n, ac, ax, be, then
//! another channel's and the frames not decoded), never sorted by share, so
//! a row a reader is reaching for stays where it was. Each says its share
//! of the time heard from the airtime its frames stated, its frames a
//! second and the airtime a frame; a row whose frames stated none says so
//! rather than show a zero (rule 2). Short of rows, a format's rates fold
//! into one.
//!
//! The rows are the frames kept for the Frames view on this centre, on the
//! stream's newest clock, so the ledger reaches back as far as they do and
//! says how far, and how much of it a feed gap took, in its title. **What,
//! never who**: a row a transmitter needs the MAC header, which is not read,
//! and the foot says so rather than group by anything that would pass for
//! one.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    Frame,
};

use super::{channel_silence, locked, put};
use crate::state::{SdrMetrics, Spender};
use crate::ui::panel::{FeedSpan, Panel, PanelChrome, Staleness};

pub struct NetWifiLedgerPanel;

/// Where the bar starts, after the swatch and the row's name.
const BAR_X: u16 = 20;

/// The columns past the bar: the share, frames a second, airtime a frame.
const SHARE_W: u16 = 9;
const PER_S_W: u16 = 12;
const EACH_W: u16 = 14;

fn ink(spender: Spender, theme: &crate::Theme) -> Color {
    match spender {
        Spender::Format(f) => theme.wifi(f),
        Spender::OtherChannel => theme.wifi_other_channel,
    }
}

fn draw(buf: &mut Buffer, area: Rect, state: &SdrMetrics, theme: &crate::Theme) {
    let label = Style::default().fg(theme.label);
    let value = Style::default().fg(theme.value);
    let words = channel_silence(state, "Airtime")
        .map(|s| format!("{}: {}", s.line, s.keys_text()))
        .or_else(|| {
            let ch = locked(state)?;
            state
                .net
                .wifi
                .ledger(state.radio.frequency as f64)
                .is_none()
                .then(|| format!("nothing heard on channel {ch} yet"))
        });
    if let Some(w) = words {
        put(buf, area.x + 1, area.y, &w, label, area);
        return;
    }
    let Some(ledger) = state.net.wifi.ledger(state.radio.frequency as f64) else {
        return;
    };
    let room = usize::from(area.height.saturating_sub(2));
    let rows = if ledger.rows.len() > room {
        ledger.folded()
    } else {
        ledger.rows.clone()
    };
    let bw = area
        .width
        .saturating_sub(BAR_X + 2 + SHARE_W + PER_S_W + EACH_W + 1)
        .min(50);
    let xp = area.x + BAR_X + bw + 2;
    let xf = xp + SHARE_W;
    let xm = xf + PER_S_W;
    let y = area.y;
    put(buf, area.x + 3, y, "format and rate", label, area);
    put(buf, xp + SHARE_W - 5, y, "share", label, area);
    put(buf, xf + PER_S_W - 8, y, "frames/s", label, area);
    put(buf, xm + EACH_W - 12, y, "airtime each", label, area);
    let max = rows
        .iter()
        .map(|r| ledger.share(r))
        .fold(0.0, f64::max)
        .max(1e-12);
    for (i, r) in rows.iter().take(room).enumerate() {
        let y = area.y + 1 + i as u16;
        let colour = Style::default().fg(ink(r.spender, theme));
        put(buf, area.x, y, "▆▆", colour, area);
        put(buf, area.x + 3, y, &r.label(), value, area);
        let share = ledger.share(r);
        if share > 0.0 {
            let cells = ((share / max) * f64::from(bw)).round().max(1.0) as usize;
            put(buf, area.x + BAR_X, y, &"█".repeat(cells), colour, area);
        }
        let pct = match r.mean_us() {
            Some(_) => format!("{:>7.2} %", share * 100.0),
            None => format!("{:>9}", "\u{b7}"),
        };
        put(buf, xp, y, &pct, value, area);
        let per_s = format!("{:>12.1}", ledger.per_s(r));
        put(buf, xf, y, &per_s, value, area);
        let each = r.mean_us().map_or(format!("{:>14}", "not stated"), |m| {
            format!("{m:>11.0} \u{b5}s")
        });
        put(buf, xm, y, &each, label, area);
    }
    put(
        buf,
        area.x + 3,
        area.bottom().saturating_sub(1),
        "who: a row a transmitter needs the MAC header, which is not read yet",
        Style::default().fg(theme.stale),
        area,
    );
}

impl Panel for NetWifiLedgerPanel {
    fn name(&self) -> &'static str {
        "net_wifi_ledger"
    }

    fn reads_spectrum(&self) -> bool {
        false
    }

    fn min_size(&self) -> (u16, u16) {
        (60, 4)
    }

    fn chrome(&self, state: &SdrMetrics) -> PanelChrome {
        let mut suffix = " \u{b7} by format and rate".to_string();
        if let Some(l) = locked(state).and(state.net.wifi.ledger(state.radio.frequency as f64)) {
            suffix.push_str(&format!(" \u{b7} over the last {:.1} s", l.span_s));
            if l.heard_s < l.span_s - 0.05 {
                suffix.push_str(&format!(
                    " \u{b7} {:.1} s heard of {:.1} s",
                    l.heard_s, l.span_s
                ));
            }
        }
        PanelChrome::new("Ledger")
            .stale_when(Staleness::NotStreaming)
            .tag_if(true, state.net.mode.tag())
            .counts_from_feed(FeedSpan::Session)
            .suffix(suffix)
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
        draw(f.buffer_mut(), inner, state, theme);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::wifi::dsss::header::{Header, Preamble, Rate as DsssRate};
    use crate::signal::wifi::format::{Format, Heard};
    use crate::signal::wifi::lsig::LSig;
    use crate::signal::wifi::signal_field::Rate;
    use crate::state::fixture::draw;
    use crate::state::{Family, Lost, NetMode, RowKind, WifiRow};
    use ratatui::{backend::TestBackend, Terminal};

    /// A frame `at_s` into channel 11's stream, stating `air_us`.
    fn row(kind: RowKind, at_s: f64, air_us: Option<u32>) -> WifiRow {
        WifiRow {
            seq: 0,
            start: (at_s * 20e6) as u64,
            run: 0,
            when: std::time::Instant::now(),
            carrier_hz: 2_462e6,
            kind,
            airtime_us: air_us,
            gap_us: None,
            offset_hz: None,
            snr_db: None,
            channel: None,
            clock: None,
            epoch: 0,
        }
    }

    fn b() -> RowKind {
        RowKind::Dsss {
            preamble: Preamble::Long,
            header: Ok(Header {
                rate: DsssRate::M1,
                locked: true,
                extension: false,
                length_us: 2000,
            }),
        }
    }

    fn ofdm(format: Option<Format>, rate: Rate) -> RowKind {
        RowKind::Ofdm(Heard {
            lsig: format.map(|_| Ok(LSig { rate, length: 100 })),
            format,
        })
    }

    /// The Airtime view locked on channel 11, a second of it heard: two
    /// 802.11b beacons, two non-HT rates and a frame not decoded.
    fn on_eleven() -> SdrMetrics {
        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = "wifi_channel".to_string();
        m.ui.active_preset = "wifi_airtime".to_string();
        m.radio.frequency = 2_462_000_000;
        m.radio.config_sample_rate = 20e6;
        m.net.wifi.ran = true;
        m.net.wifi.channel = Some(11);
        m.net.mode = NetMode::Lock;
        for r in [
            row(b(), 0.0, Some(2000)),
            row(ofdm(Some(Format::NonHt), Rate::R6), 0.1, Some(100)),
            row(ofdm(Some(Format::NonHt), Rate::R24), 0.2, Some(50)),
            row(ofdm(None, Rate::R6), 0.3, None),
            row(b(), 0.6, Some(2000)),
        ] {
            m.net.wifi.push_row(r);
        }
        m.net.wifi.heard_to = Some((0, 20_000_000));
        m
    }

    fn line<'a>(l: &'a [String], has: &str) -> &'a str {
        l.iter()
            .find(|r| r.contains(has))
            .unwrap_or_else(|| panic!("no line with {has}: {l:#?}"))
    }

    /// **A row a format and rate, in the standards' order**, each its share
    /// of the time heard, frames a second and airtime a frame; a row that
    /// stated no airtime says so rather than a zero.
    #[test]
    fn a_row_a_format_and_rate_with_its_share() {
        let l = draw(NetWifiLedgerPanel, 120, 10, &on_eleven());
        let head = &l[1];
        assert!(
            head.contains("format and rate") && head.contains("share"),
            "{l:#?}"
        );
        assert!(
            head.contains("frames/s") && head.contains("airtime each"),
            "{l:#?}"
        );
        let b = line(&l, "802.11b 1 Mb/s");
        assert!(b.contains("0.40 %") && b.contains("2.0"), "{b}");
        assert!(b.contains("2000 \u{b5}s"), "{b}");
        let undecoded = line(&l, "not decoded");
        assert!(undecoded.contains("not stated"), "{undecoded}");
        assert!(!undecoded.contains('%'), "{undecoded}");
        let at = |s: &str| l.iter().position(|r| r.contains(s)).unwrap();
        assert!(at("802.11b") < at("non-HT 6 Mb/s"));
        assert!(at("non-HT 6 Mb/s") < at("non-HT 24 Mb/s"));
        assert!(at("non-HT 24 Mb/s") < at("not decoded"));
    }

    /// The swatch and the bar in the format's colour.
    #[test]
    fn a_row_is_in_its_formats_colour() {
        let m = on_eleven();
        let theme = crate::Theme::sdr();
        let mut t = Terminal::new(TestBackend::new(118, 8)).unwrap();
        t.draw(|f| NetWifiLedgerPanel.render(f, f.size(), &m, &theme, false))
            .unwrap();
        let b = t.backend().buffer();
        assert_eq!(b.get(0, 1).fg, theme.wifi(Family::Dsss));
        assert_eq!(b.get(20, 1).fg, theme.wifi(Family::Dsss));
    }

    /// **Short of rows, a format's rates fold into one**: four rows will
    /// not fit in three, so non-HT's two are one row.
    #[test]
    fn short_of_rows_the_rates_fold() {
        let l = draw(NetWifiLedgerPanel, 120, 7, &on_eleven());
        assert!(
            line(&l, "non-HT \u{b7} 2 rates").contains("75 \u{b5}s"),
            "{l:#?}"
        );
        assert!(!l.iter().any(|r| r.contains("24 Mb/s")), "{l:#?}");
    }

    /// Who spent it is not read: the ledger says so at its foot, never
    /// grouping by anything that would pass for a transmitter.
    #[test]
    fn the_foot_says_who_is_not_read() {
        let l = draw(NetWifiLedgerPanel, 120, 10, &on_eleven());
        assert!(l[8].contains("needs the MAC header"), "{l:#?}");
    }

    /// The title says how far back the ledger reaches, and how much of it
    /// was heard where a feed gap took some.
    #[test]
    fn the_title_says_the_span_and_what_was_heard() {
        let mut m = on_eleven();
        let l = draw(NetWifiLedgerPanel, 120, 10, &m);
        assert!(l[0].contains("by format and rate"), "{}", l[0]);
        assert!(l[0].contains("over the last 1.0 s"), "{}", l[0]);
        assert!(!l[0].contains("heard of"), "{}", l[0]);
        m.net.wifi.lose(Lost {
            epoch: 0,
            from: 16_000_000,
            to: 20_000_000,
        });
        let l = draw(NetWifiLedgerPanel, 120, 10, &m);
        assert!(l[0].contains("0.8 s heard of 1.0 s"), "{}", l[0]);
    }

    /// Nothing heard yet, and not locked, are said in a line.
    #[test]
    fn listening_and_not_locked_are_said() {
        let mut m = on_eleven();
        m.net.wifi.rows.clear();
        let l = draw(NetWifiLedgerPanel, 120, 10, &m);
        line(&l, "nothing heard on channel 11 yet");
        m.net.mode = NetMode::Survey;
        let l = draw(NetWifiLedgerPanel, 120, 10, &m);
        line(&l, "not locked on a channel");
    }
}
