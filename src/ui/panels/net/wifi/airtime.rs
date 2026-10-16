// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `NetWifiAirtimePanel` - what is spending this channel's time, second by
//! second: a column a second, the newest on the right.
//!
//! **Two shares side by side, never one inside the other**, as on the
//! survey: the left bar of a column is how busy the channel's busiest
//! megahertz was, whatever made it; the right bar is the airtime the
//! frames read on its centre stated, stacked by format in the theme's
//! Wi-Fi colours. A weak 802.11b beacon is found by its chips under the
//! energy threshold, so the frames can stand taller than the energy.
//!
//! **One scale, 0 to 100 %, fixed**, so a burst of contention and a quiet
//! minute read as what they were. A column is the survey's visits to the
//! locked channel that ended in its second (two of half a second each);
//! one a gap in the feed cut into, or a second after the first one heard
//! that no visit ended in, is hatched in the stale colour and left out of
//! the reading (a gap is not quiet air, rule 2). The reading, over the last
//! [`READING_COLUMNS`] seconds with its 1 sigma, is the card's; where the
//! card has no room it rides in the title.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    Frame,
};

use super::{channel_silence, locked, pct, plate, put};
use crate::state::{AirColumn, Family, SdrMetrics, READING_COLUMNS};
use crate::ui::panel::{FeedSpan, Panel, PanelChrome, Staleness};

pub struct NetWifiAirtimePanel;

/// Columns: the scale's labels, then the plot.
const PLOT_X: u16 = 7;

/// The cells a column takes: its two bars and a gap.
const COLUMN_CELLS: u16 = 3;

/// A bar's cell `r` rows from the floor, of a bar `share` of `rows` tall.
pub(super) fn eighth(share: f64, rows: u16, r: u16) -> char {
    const EIGHTHS: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let e = (share.clamp(0.0, 1.0) * f64::from(rows) * 8.0).round() as u16;
    let lo = r * 8;
    if e >= lo + 8 {
        '█'
    } else if e > lo {
        EIGHTHS[usize::from(e - lo)]
    } else {
        ' '
    }
}

/// The family whose band of the stack covers row `r`'s middle, of `rows`.
fn family_at(c: &AirColumn, rows: u16, r: u16) -> Family {
    let mid = (f64::from(r) + 0.5) / f64::from(rows);
    let top = c.frames().min(1.0);
    let mut acc = 0.0;
    let mut last = Family::Dsss;
    for (f, v) in Family::ALL.iter().zip(c.airtime) {
        if v <= 0.0 {
            continue;
        }
        acc += v;
        last = *f;
        if acc >= mid.min(top) - 1e-12 {
            break;
        }
    }
    last
}

fn draw(buf: &mut Buffer, area: Rect, state: &SdrMetrics, theme: &crate::Theme) {
    if let Some(s) = channel_silence(state, "Airtime") {
        plate(buf, area, &s, theme);
        return;
    }
    let Some(ch) = locked(state) else {
        return;
    };
    let legend = area.height >= 8;
    let rows = area.height.saturating_sub(1 + u16::from(legend));
    if rows < 2 || area.width < PLOT_X + COLUMN_CELLS {
        return;
    }
    let label = Style::default().fg(theme.label);
    for (y, mark) in [(0, "100 %"), (rows / 2, " 50 %"), (rows - 1, "  0 %")] {
        put(buf, area.x, area.y + y, mark, label, area);
    }
    let x0 = area.x + PLOT_X;
    let n = usize::from((area.width - PLOT_X) / COLUMN_CELLS);
    let cols = state
        .net
        .wifi
        .survey
        .columns(ch, std::time::Instant::now(), n);
    let first = cols.iter().position(|c| c.visits > 0);
    let floor = area.y + rows - 1;
    let mut shown = [false; 7];
    for (k, c) in cols.iter().enumerate() {
        let cx = x0 + COLUMN_CELLS * k as u16;
        if first.is_none_or(|f| k < f) {
            continue;
        }
        if !c.whole() {
            let hatch = Style::default().fg(theme.stale);
            for y in area.y..=floor {
                put(buf, cx, y, "╱╱", hatch, area);
            }
            continue;
        }
        for r in 0..rows {
            let y = floor - r;
            let e = eighth(c.busy, rows, r);
            if e != ' ' {
                buf.get_mut(cx, y)
                    .set_char(e)
                    .set_style(Style::default().fg(theme.value));
            }
            let g = eighth(c.frames(), rows, r);
            if g != ' ' {
                let colour: Color = theme.wifi(family_at(c, rows, r));
                buf.get_mut(cx + 1, y)
                    .set_char(g)
                    .set_style(Style::default().fg(colour));
            }
        }
        for (s, v) in shown.iter_mut().zip(c.airtime) {
            *s |= v > 0.0;
        }
    }
    if first.is_none() {
        let words = format!("nothing heard on channel {ch} yet");
        put(buf, x0 + 2, area.y + rows / 2, &words, label, area);
    }
    // The axis: a tick every ten seconds back from the newest, which is now.
    let axis = area.y + rows;
    for back in (10..n).step_by(10) {
        let cx = x0 + COLUMN_CELLS * (n - 1 - back) as u16;
        let x = put(buf, cx, axis, "┊", label, area);
        put(buf, x, axis, &format!("-{back} s"), label, area);
    }
    put(
        buf,
        area.right().saturating_sub(3),
        axis,
        "now",
        label,
        area,
    );
    if legend {
        let y = axis + 1;
        let mut x = put(buf, x0, y, "▌", Style::default().fg(theme.value), area);
        x = put(buf, x + 1, y, "busy (energy)", label, area) + 3;
        for (f, on) in Family::ALL.iter().zip(shown) {
            if on {
                x = put(buf, x, y, "▌", Style::default().fg(theme.wifi(*f)), area);
                x = put(buf, x + 1, y, f.label(), label, area) + 3;
            }
        }
        put(
            buf,
            x,
            y,
            "frames, by format: what they stated",
            label,
            area,
        );
    }
}

impl Panel for NetWifiAirtimePanel {
    fn name(&self) -> &'static str {
        "net_wifi_airtime"
    }

    fn reads_spectrum(&self) -> bool {
        false
    }

    fn min_size(&self) -> (u16, u16) {
        (40, 7)
    }

    fn column_height(&self, column: u16) -> Option<u16> {
        // Eight plot rows, the axis and the legend where the column is
        // tall; four plot rows and the axis where it is not.
        Some(if column >= 24 { 11 } else { 7 })
    }

    fn chrome(&self, state: &SdrMetrics) -> PanelChrome {
        let mut suffix = match locked(state) {
            Some(ch) => format!(" \u{b7} channel {ch}"),
            None => String::new(),
        };
        suffix.push_str(" \u{b7} a column a second \u{b7} 0 to 100 %");
        // Where the card is on screen it carries the reading; here only
        // where it has no room.
        let room = !super::card::on_screen(state);
        let reading = locked(state).filter(|_| room).and_then(|ch| {
            state
                .net
                .wifi
                .survey
                .share(ch, std::time::Instant::now(), READING_COLUMNS)
        });
        if let Some(r) = reading {
            let spread = r
                .frames
                .sigma
                .map_or(String::new(), |s| format!(" \u{b1} {}", pct(s)));
            suffix.push_str(&format!(
                " \u{b7} {}{spread} % frames over {READING_COLUMNS} s",
                pct(r.frames.mean)
            ));
        }
        PanelChrome::new("Airtime")
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
    use crate::state::fixture::draw;
    use crate::state::{Family, NetMode, Visit};
    use ratatui::{backend::TestBackend, Terminal};
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

    fn dsss() -> usize {
        Family::ALL.iter().position(|f| *f == Family::Dsss).unwrap()
    }

    /// A half-second visit to channel 11 that ended `ago` seconds back,
    /// heard `heard` of its span, `busy` and with 802.11b `air` of it.
    fn visit(ago: f64, heard: f64, busy: f64, air: f64) -> Visit {
        let mut airtime = [0.0; 7];
        airtime[dsss()] = air;
        Visit {
            seconds: 0.5 * heard,
            span_s: 0.5,
            ended: Some(Instant::now() - Duration::from_secs_f64(ago)),
            busy,
            airtime,
            ..Default::default()
        }
    }

    /// 100 wide and 11 tall, the hero as the view draws it on a wide
    /// screen: 98 inner columns, 7 of the scale, 30 columns of three.
    fn lines(m: &SdrMetrics) -> Vec<String> {
        draw(NetWifiAirtimePanel, 100, 11, m)
    }

    /// Inner row `r`'s cells from the plot's left edge.
    fn plot_row(l: &[String], r: usize) -> Vec<char> {
        l[1 + r].chars().skip(8).collect()
    }

    /// **A column a second, the newest on the right, the energy's bar
    /// beside the frames' stacked bar, 0 to 100 %.** A visit wholly busy
    /// and wholly 802.11b fills both bars of the newest column to the top.
    #[test]
    fn the_newest_second_is_the_rightmost_column() {
        let mut m = on_eleven();
        m.net.wifi.survey.visits[10] = vec![visit(0.6, 1.0, 1.0, 1.0), visit(0.2, 1.0, 1.0, 1.0)];
        let l = lines(&m);
        assert!(l[1].contains("100 %"), "{l:#?}");
        let top = plot_row(&l, 0);
        // The newest column: the 30th, its two bars at cells 87 and 88.
        assert_eq!((top[87], top[88]), ('█', '█'), "{l:#?}");
        assert!(top[..87].iter().all(|c| *c == ' '), "{l:#?}");
    }

    /// The frames' bar is drawn in its format's colour, the energy's in the
    /// value colour.
    #[test]
    fn the_frames_bar_is_in_its_formats_colour() {
        let mut m = on_eleven();
        m.net.wifi.survey.visits[10] = vec![visit(0.2, 1.0, 1.0, 1.0)];
        let theme = crate::Theme::sdr();
        let mut t = Terminal::new(TestBackend::new(98, 9)).unwrap();
        t.draw(|f| NetWifiAirtimePanel.render(f, f.size(), &m, &theme, false))
            .unwrap();
        let b = t.backend().buffer();
        assert_eq!(b.get(7 + 87, 0).fg, theme.value);
        assert_eq!(b.get(7 + 88, 0).fg, theme.wifi(Family::Dsss));
    }

    /// **A column a gap in the feed cut into is hatched**, and so is a
    /// second after the first heard that no visit ended in; the seconds
    /// before the first visit are empty, not hatched.
    #[test]
    fn a_column_not_heard_whole_is_hatched() {
        let mut m = on_eleven();
        m.net.wifi.survey.visits[10] = vec![visit(3.5, 1.0, 0.5, 0.5), visit(0.5, 0.6, 0.5, 0.5)];
        let l = lines(&m);
        let mid = plot_row(&l, 3);
        // Columns 26 (whole), 27 and 28 (nothing ended), 29 (cut into).
        assert_ne!(mid[78], '╱', "{l:#?}");
        assert_eq!((mid[81], mid[84], mid[87]), ('╱', '╱', '╱'), "{l:#?}");
        assert!(mid[..78].iter().all(|c| *c == ' '), "{l:#?}");
    }

    /// The title names the channel and the scale; where the card has no
    /// room, the reading over the last 20 s with its 1 sigma once three
    /// seconds were heard whole rides in it, and where the card is on
    /// screen the card alone carries it.
    #[test]
    fn the_title_carries_the_reading_where_the_card_has_no_room() {
        let mut m = on_eleven();
        m.ui.screen_cols = 100;
        m.net.wifi.survey.visits[10] = vec![
            visit(2.5, 1.0, 0.2, 0.1),
            visit(1.5, 1.0, 0.2, 0.2),
            visit(0.5, 1.0, 0.2, 0.3),
        ];
        let l = lines(&m);
        assert!(l[0].contains("channel 11"), "{}", l[0]);
        assert!(l[0].contains("a column a second"), "{}", l[0]);
        assert!(l[0].contains("20.0 ± 5.8 % frames over 20 s"), "{}", l[0]);
        m.ui.screen_cols = 191;
        let l = lines(&m);
        assert!(l[0].contains("a column a second"), "{}", l[0]);
        assert!(!l[0].contains("frames over"), "{}", l[0]);
    }

    /// Nothing heard on the channel yet: the columns empty, and said so.
    #[test]
    fn listening_says_nothing_was_heard_yet() {
        let l = lines(&on_eleven());
        assert!(
            l.iter()
                .any(|r| r.contains("nothing heard on channel 11 yet")),
            "{l:#?}"
        );
    }

    /// While the survey walks the band the view has no channel to read, and
    /// says so in the plate Frames shows, naming itself.
    #[test]
    fn not_locked_is_the_frames_plate() {
        let mut m = on_eleven();
        m.net.mode = NetMode::Survey;
        let l = lines(&m);
        let all = l.join("\n");
        assert!(all.contains("NOT LOCKED ON A CHANNEL"), "{all}");
        assert!(all.contains("The Airtime view reads one channel"), "{all}");
    }

    /// The legend where there is a row for it, naming only the formats on
    /// screen; none in a short hero.
    #[test]
    fn the_legend_names_the_formats_on_screen() {
        let mut m = on_eleven();
        m.net.wifi.survey.visits[10] = vec![visit(0.2, 1.0, 0.3, 0.3)];
        let tall = lines(&m);
        let legend = &tall[9];
        assert!(legend.contains("busy (energy)"), "{tall:#?}");
        assert!(legend.contains("802.11b"), "{tall:#?}");
        assert!(!legend.contains("non-HT"), "{tall:#?}");
        let short = draw(NetWifiAirtimePanel, 100, 7, &m);
        assert!(!short.join("\n").contains("busy (energy)"), "{short:#?}");
    }

    /// Eleven rows where the column is tall, seven where it is short.
    #[test]
    fn the_hero_is_eleven_rows_tall_or_seven() {
        assert_eq!(NetWifiAirtimePanel.column_height(27), Some(11));
        assert_eq!(NetWifiAirtimePanel.column_height(18), Some(7));
    }

    /// On the view, the hero takes its rows at the top of the body and the
    /// ledger the rest under it, the card beside both at 191 and 152 and
    /// gone at 100.
    #[test]
    fn on_the_view_the_ledger_takes_the_rows_under_the_hero() {
        let (engine, _) = crate::app::App::build_ui(
            "wifi_airtime",
            &std::collections::HashMap::new(),
            None,
            true,
        );
        let m = on_eleven();
        for (w, h, hero, card) in [
            (191u16, 41u16, 11u16, Some(70u16)),
            (152, 32, 7, Some(54)),
            (100, 30, 7, None),
        ] {
            let size = Rect::new(0, 0, w, h);
            let a = engine
                .area_of("net_wifi_airtime", size, &m)
                .expect("the hero");
            let l = engine
                .area_of("net_wifi_ledger", size, &m)
                .expect("the ledger");
            assert_eq!(a.height, hero, "{w} x {h}");
            assert_eq!(l.y, a.y + a.height, "{w} x {h}");
            assert!(l.height >= 7, "{w} x {h}: {l:?}");
            let c = engine.area_of("net_wifi_share", size, &m);
            assert_eq!(c.map(|c| c.width), card, "{w} x {h}");
            if let Some(c) = c {
                assert_eq!(c.x, a.x + a.width, "{w} x {h}");
                assert_eq!(c.height, a.height + l.height, "{w} x {h}");
            }
        }
    }
}
