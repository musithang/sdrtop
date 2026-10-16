// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `NetWifiSharePanel` - the Airtime view's card: how much of the channel's
//! time its frames took, with its spread, and the gaps between them.
//!
//! **The reading is over whole seconds**: the hero's columns of the last
//! [`READING_COLUMNS`] seconds that were heard whole, their mean and its
//! 1 sigma (the standard error over them, from the third on, as the survey
//! gives its own). The energy's busy share is beside it on a line of its
//! own, never folded into it, and where the frames stand taller than the
//! energy that is said: weak frames are found under its threshold. How much
//! of the time was heard is said under them, since a feed gap is not quiet
//! air.
//!
//! **The rhythm at its foot** is the gaps between frames, ten bins a decade
//! from 1 us to 100 ms, with the signal extension taken off where the
//! standard adds one, marked at SIFS (10 us), DIFS (28 us, the short slot)
//! and 1 ms; what fell under 1 us or past 100 ms is counted under it. It is
//! the first thing to give way when the card is short.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    Frame,
};

use super::{channel_silence, locked, pct, put};
use crate::state::{Family, Rhythm, SdrMetrics, READING_COLUMNS, RHYTHM_BINS};
use crate::ui::panel::{FeedSpan, Panel, PanelChrome, Staleness};

pub struct NetWifiSharePanel;

/// The fewest plot rows the rhythm is drawn in.
const RHYTHM_MIN_ROWS: u16 = 2;

/// Its rule, labels and footer.
const RHYTHM_FRAME_ROWS: u16 = 3;

/// The marks on the rhythm: SIFS, the short slot's DIFS (SIFS and two
/// slots of 9 us), 1 ms; each label's place against its mark.
const MARKS: [(f64, &str, i32); 3] = [(10.0, "SIFS", -3), (28.0, "DIFS", 1), (1e3, "1 ms", 0)];

/// The card's state in a head and its words, where there is no reading.
fn said(buf: &mut Buffer, area: Rect, head: &str, words: &str, theme: &crate::Theme) {
    let bold = Style::default()
        .fg(theme.value)
        .add_modifier(Modifier::BOLD);
    put(buf, area.x + 1, area.y + 1, head, bold, area);
    put(
        buf,
        area.x + 1,
        area.y + 2,
        words,
        Style::default().fg(theme.label),
        area,
    );
}

/// The reading on top, from row `area.y`; the row it ended under.
fn reading(buf: &mut Buffer, area: Rect, state: &SdrMetrics, ch: u8, theme: &crate::Theme) -> u16 {
    let label = Style::default().fg(theme.label);
    let value = Style::default().fg(theme.value);
    let now = std::time::Instant::now();
    let survey = &state.net.wifi.survey;
    let Some(r) = survey.share(ch, now, READING_COLUMNS) else {
        let cut = survey
            .columns(ch, now, READING_COLUMNS)
            .iter()
            .any(|c| c.visits > 0);
        let words = if cut {
            "no second heard whole yet: a feed gap cut into each".to_string()
        } else {
            format!("nothing heard on channel {ch} yet")
        };
        said(buf, area, "LISTENING", &words, theme);
        return area.y + 4;
    };
    let digits = pct(r.frames.mean);
    let hi = Style::default()
        .fg(theme.value_hi)
        .add_modifier(Modifier::BOLD);
    let seconds = format!(
        "{} second{} heard whole",
        r.columns,
        if r.columns == 1 { "" } else { "s" }
    );
    let over = format!("over the last {READING_COLUMNS} s");
    let side = match r.frames.sigma {
        Some(s) => [
            over,
            format!("\u{b1} {} %  frames", pct(s)),
            format!("1 \u{3c3}, {seconds}"),
        ],
        None => [
            over,
            "%  frames".to_string(),
            format!("{seconds}, \u{3c3} from 3"),
        ],
    };
    for (i, t) in side.iter().enumerate() {
        let big: Vec<&str> = digits
            .chars()
            .map(|c| crate::ui::widgets::bigdigits::glyph(c)[i])
            .collect();
        let y = area.y + i as u16;
        let x = put(buf, area.x + 1, y, &big.join(" "), hi, area);
        put(buf, x + 2, y, t, label, area);
    }
    let busy = match r.busy.sigma {
        Some(s) => format!("busy, its energy  {} \u{b1} {} %", pct(r.busy.mean), pct(s)),
        None => format!("busy, its energy  {} %", pct(r.busy.mean)),
    };
    let mut y = area.y + 3;
    put(buf, area.x + 1, y, &busy, value, area);
    y += 1;
    let heard = format!("{:.1} s heard of {:.1} s", r.heard_s, r.span_s);
    put(buf, area.x + 1, y, &heard, label, area);
    y += 1;
    if r.frames.mean > r.busy.mean {
        put(
            buf,
            area.x + 1,
            y,
            "over the energy: weak frames under its threshold",
            label,
            area,
        );
        y += 1;
    }
    // The split by format: one bar, then its key.
    let total: f64 = r.airtime.iter().sum();
    if total > 0.0 {
        y += 1;
        let bar = f64::from(area.width.saturating_sub(2));
        let mut x = area.x + 1;
        for (f, a) in Family::ALL.iter().zip(r.airtime) {
            let cells = ((a / total) * bar).round() as usize;
            x = put(
                buf,
                x,
                y,
                &"\u{2588}".repeat(cells),
                Style::default().fg(theme.wifi(*f)),
                area,
            );
        }
        y += 1;
        let mut x = area.x + 1;
        for (f, a) in Family::ALL.iter().zip(r.airtime) {
            if a <= 0.0 {
                continue;
            }
            let item = format!("{} {:.0}%", f.label(), 100.0 * a / total);
            if x + 2 + item.chars().count() as u16 > area.right() && x > area.x + 1 {
                y += 1;
                x = area.x + 1;
            }
            x = put(
                buf,
                x,
                y,
                "\u{25a0} ",
                Style::default().fg(theme.wifi(*f)),
                area,
            );
            x = put(buf, x, y, &item, label, area) + 3;
        }
        y += 1;
    }
    y
}

/// The rhythm in the rows from `top` to the card's foot.
fn rhythm(buf: &mut Buffer, area: Rect, top: u16, r: &Rhythm, theme: &crate::Theme) {
    let label = Style::default().fg(theme.label);
    let dim = Style::default().fg(theme.border_dim);
    let rows = area.bottom().saturating_sub(top + RHYTHM_FRAME_ROWS).min(6);
    if rows < RHYTHM_MIN_ROWS || area.width < RHYTHM_BINS as u16 + 2 {
        return;
    }
    let rule_y = area.bottom() - RHYTHM_FRAME_ROWS - rows;
    let head = " \u{2500} RHYTHM \u{b7} the gaps between frames ";
    let rest = usize::from(area.width).saturating_sub(head.chars().count());
    put(
        buf,
        area.x,
        rule_y,
        &format!("{head}{}", "\u{2500}".repeat(rest)),
        dim,
        area,
    );
    let x0 = area.x + 2;
    let floor = rule_y + rows;
    let max = f64::from(*r.bins.iter().max().unwrap_or(&0)).max(1.0);
    for (k, n) in r.bins.iter().enumerate() {
        for row in 0..rows {
            let c = super::airtime::eighth(f64::from(*n) / max, rows, row);
            if c != ' ' {
                buf.get_mut(x0 + k as u16, floor - row)
                    .set_char(c)
                    .set_style(Style::default().fg(theme.value));
            }
        }
    }
    let marks_y = floor + 1;
    for (us, name, shift) in MARKS {
        let k = (us.log10() * 10.0).floor() as u16;
        let cx = x0 + k;
        for y in rule_y + 1..=floor {
            if buf.get(cx, y).symbol() == " " {
                buf.get_mut(cx, y).set_char('\u{250a}').set_style(dim);
            }
        }
        let lx = (i32::from(cx) + shift).max(i32::from(area.x)) as u16;
        put(buf, lx, marks_y, name, label, area);
    }
    let mut foot = format!(
        "1 \u{b5}s to 100 ms \u{b7} under 1 \u{b5}s {} \u{b7} past {}",
        r.under, r.past
    );
    if r.unplaced > 0 {
        foot.push_str(&format!(" \u{b7} {} after frames not decoded", r.unplaced));
    }
    put(buf, x0, marks_y + 1, &foot, label, area);
}

fn draw(buf: &mut Buffer, area: Rect, state: &SdrMetrics, theme: &crate::Theme) {
    if let Some(s) = channel_silence(state, "Airtime") {
        said(buf, area, s.card, &s.keys_text(), theme);
        return;
    }
    let Some(ch) = locked(state) else {
        return;
    };
    let top = reading(buf, area, state, ch, theme);
    if let Some(r) = state.net.wifi.rhythm(state.radio.frequency as f64) {
        rhythm(buf, area, top + 1, &r, theme);
    }
}

impl Panel for NetWifiSharePanel {
    fn name(&self) -> &'static str {
        "net_wifi_share"
    }

    fn reads_spectrum(&self) -> bool {
        false
    }

    fn min_size(&self) -> (u16, u16) {
        (40, 10)
    }

    fn column_width(&self, body: u16) -> Option<u16> {
        Some(match body {
            b if !super::card::fits(b) => 0,
            180.. => 70,
            _ => 54,
        })
    }

    fn chrome(&self, state: &SdrMetrics) -> PanelChrome {
        PanelChrome::new("Share of the time")
            .stale_when(Staleness::NotStreaming)
            .tag_if(true, state.net.mode.tag())
            .counts_from_feed(FeedSpan::Session)
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
    use crate::signal::wifi::format::{Format, Heard};
    use crate::signal::wifi::lsig::LSig;
    use crate::signal::wifi::signal_field::Rate;
    use crate::state::fixture::draw;
    use crate::state::{Family, NetMode, RowKind, Visit, WifiRow};
    use std::time::{Duration, Instant};

    /// The Airtime view locked on channel 11 at 20 Msps, streaming.
    fn on_eleven() -> SdrMetrics {
        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = "wifi_channel".to_string();
        m.ui.active_preset = "wifi_airtime".to_string();
        m.radio.frequency = 2_462_000_000;
        m.radio.config_sample_rate = 20e6;
        m.net.wifi.ran = true;
        m.net.wifi.channel = Some(11);
        m.net.mode = NetMode::Lock;
        m
    }

    /// A half-second visit to channel 11 that ended `ago` seconds back,
    /// heard `heard` of its span, `busy` and with 802.11b `air` of it.
    fn visit(ago: f64, heard: f64, busy: f64, air: f64) -> Visit {
        let mut airtime = [0.0; 7];
        airtime[Family::ALL.iter().position(|f| *f == Family::Dsss).unwrap()] = air;
        Visit {
            seconds: 0.5 * heard,
            span_s: 0.5,
            ended: Some(Instant::now() - Duration::from_secs_f64(ago)),
            busy,
            airtime,
            ..Default::default()
        }
    }

    /// Three whole seconds, 10, 20 and 30 % 802.11b, 20 % busy.
    fn three_seconds() -> SdrMetrics {
        let mut m = on_eleven();
        m.net.wifi.survey.visits[10] = vec![
            visit(2.5, 1.0, 0.2, 0.1),
            visit(1.5, 1.0, 0.2, 0.2),
            visit(0.5, 1.0, 0.2, 0.3),
        ];
        m
    }

    /// An ERP-OFDM frame and its acknowledgement 17 us after its end.
    fn with_an_exchange(mut m: SdrMetrics) -> SdrMetrics {
        let frame = |at_us: f64| WifiRow {
            seq: 0,
            start: (at_us * 20.0) as u64,
            run: 0,
            when: Instant::now(),
            carrier_hz: 2_462e6,
            kind: RowKind::Ofdm(Heard {
                lsig: Some(Ok(LSig {
                    rate: Rate::R24,
                    length: 100,
                })),
                format: Some(Format::NonHt),
            }),
            airtime_us: Some(100),
            gap_us: None,
            offset_hz: None,
            snr_db: None,
            channel: None,
            clock: None,
            epoch: 0,
        };
        m.net.wifi.push_row(frame(0.0));
        m.net.wifi.push_row(frame(117.0));
        m.net.wifi.heard_to = Some((0, 20_000));
        m
    }

    fn line<'a>(l: &'a [String], has: &str) -> &'a str {
        l.iter()
            .find(|r| r.contains(has))
            .unwrap_or_else(|| panic!("no line with {has}: {l:#?}"))
    }

    /// **The reading is the whole seconds' mean with its 1 sigma**, the
    /// energy's beside it on its own line, how much was heard, and the
    /// frames' share split by format.
    #[test]
    fn the_reading_its_spread_the_energy_and_the_split() {
        let l = draw(NetWifiSharePanel, 70, 27, &three_seconds());
        line(&l, "\u{b1} 5.8 %  frames");
        line(&l, "over the last 20 s");
        line(&l, "1 \u{3c3}, 3 seconds heard whole");
        line(&l, "busy, its energy  20.0 \u{b1} 0.0 %");
        line(&l, "1.5 s heard of 1.5 s");
        line(&l, "802.11b 100%");
    }

    /// Frames standing taller than the energy are said, not clipped: weak
    /// frames are found under the energy's threshold.
    #[test]
    fn frames_over_the_energy_are_said() {
        let mut m = three_seconds();
        for v in &mut m.net.wifi.survey.visits[10] {
            v.busy = 0.05;
        }
        let l = draw(NetWifiSharePanel, 70, 27, &m);
        line(&l, "over the energy: weak frames under its threshold");
        let quiet = draw(NetWifiSharePanel, 70, 27, &three_seconds());
        assert!(!quiet.join("\n").contains("over the energy"), "{quiet:#?}");
    }

    /// A second a gap cut into is left out of the reading and counted in
    /// what was heard.
    #[test]
    fn a_cut_second_is_counted_in_what_was_heard() {
        let mut m = three_seconds();
        m.net.wifi.survey.visits[10].push(visit(3.5, 0.4, 0.9, 0.9));
        let l = draw(NetWifiSharePanel, 70, 27, &m);
        line(&l, "1.7 s heard of 2.0 s");
        line(&l, "\u{b1} 5.8 %  frames");
    }

    /// **The rhythm at the card's foot**: ten bins a decade from 1 us to
    /// 100 ms, marked at SIFS, DIFS and 1 ms, what fell outside counted
    /// under it; the exchange's corrected gap is in the SIFS bin.
    #[test]
    fn the_rhythm_is_at_the_foot() {
        let l = draw(
            NetWifiSharePanel,
            70,
            27,
            &with_an_exchange(three_seconds()),
        );
        line(&l, "RHYTHM");
        let marks = line(&l, "SIFS");
        assert!(marks.contains("DIFS") && marks.contains("1 ms"), "{marks}");
        line(
            &l,
            "1 \u{b5}s to 100 ms \u{b7} under 1 \u{b5}s 0 \u{b7} past 0",
        );
        // The plot's left edge is two cells in: the SIFS bin is column 12
        // of the inner area, 13 of the drawn one.
        let at = l.iter().position(|r| r.contains("SIFS")).unwrap();
        assert_eq!(l[at - 1].chars().nth(13), Some('\u{2588}'), "{l:#?}");
    }

    /// At 54 columns nothing the reading says is cut off.
    #[test]
    fn at_fifty_four_columns_the_reading_is_whole() {
        let mut m = three_seconds();
        for v in &mut m.net.wifi.survey.visits[10] {
            v.busy = 0.05;
        }
        let l = draw(NetWifiSharePanel, 54, 27, &m);
        line(&l, "1 \u{3c3}, 3 seconds heard whole");
        line(&l, "weak frames under its threshold");
    }

    /// Short of rows, the rhythm gives way and the reading stays.
    #[test]
    fn short_of_rows_the_rhythm_gives_way() {
        let l = draw(
            NetWifiSharePanel,
            54,
            12,
            &with_an_exchange(three_seconds()),
        );
        line(&l, "busy, its energy");
        assert!(!l.join("\n").contains("RHYTHM"), "{l:#?}");
    }

    /// Nothing heard yet, and not locked, are said in place of a reading.
    #[test]
    fn listening_and_not_locked_are_said() {
        let l = draw(NetWifiSharePanel, 70, 27, &on_eleven());
        line(&l, "LISTENING");
        line(&l, "nothing heard on channel 11 yet");
        let mut m = on_eleven();
        m.net.mode = NetMode::Survey;
        let l = draw(NetWifiSharePanel, 70, 27, &m);
        line(&l, "NOT LOCKED");
        line(&l, "m locks here");
    }

    /// Seventy columns at 191, fifty-four at 152, none at 100, where the
    /// hero and the ledger need every column.
    #[test]
    fn the_card_is_seventy_wide_fifty_four_or_gone() {
        assert_eq!(NetWifiSharePanel.column_width(191), Some(70));
        assert_eq!(NetWifiSharePanel.column_width(152), Some(54));
        assert_eq!(NetWifiSharePanel.column_width(100), Some(0));
    }
}
