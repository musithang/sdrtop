// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `NetWifiSurveyPanel` - which Wi-Fi channel is busy, and which is quiet:
//! thirteen columns, one a channel.
//!
//! **Two shares side by side, never one inside the other.** The left bar
//! of a column is how busy the channel's busiest megahertz was, whatever
//! made it (the band's occupancy measure: what a station's carrier sense
//! defers to); the right bar is the airtime the frames read on its centre
//! stated, stacked by format in the theme's Wi-Fi colours. Frames are not
//! always inside the energy: a weak 802.11b beacon is found by its chips
//! 10 dB under the energy threshold, so nesting the two would draw a bar
//! sticking out of its own container.
//!
//! **One scale, 0 to 40 %, fixed,** so a column is comparable with itself a
//! minute later and with the column beside it now. The +1 sigma tick is the
//! standard error of the visits' mean, drawn only from the third visit on:
//! one or two have no spread worth a mark. Channels 1, 6 and 11, the three
//! whose 20 MHz do not overlap, are labelled bright. A channel not visited
//! yet is a dotted floor, not a zero; while the radio is locked on one
//! channel, every other column is held at its last pass and drawn in the
//! stale colour.
//!
//! What cannot be measured says so in a plate where the columns would be
//! (rule 2), and the plate names the key that helps.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    Frame,
};

use super::{locked, pct, plate, put, silence};
use crate::state::{Family, SdrMetrics};
use crate::ui::panel::{FeedSpan, Panel, PanelChrome, Staleness};

pub struct NetWifiSurveyPanel;

/// The top of the fixed scale.
const FULL: f64 = 0.40;

/// Columns: the scale's labels, then the plot.
const PLOT_X: u16 = 8;

/// A bar from the floor up, `w` columns wide, in half rows.
fn bar(buf: &mut Buffer, x: u16, floor: u16, rows: u16, w: u16, share: f64, colour: Color) {
    let halves = ((share / FULL) * f64::from(rows * 2)).round() as u16;
    for h in 0..halves.min(rows * 2) {
        let y = floor - h / 2;
        let glyph = if h % 2 == 0 && h + 1 == halves {
            '▄'
        } else {
            '█'
        };
        for dx in 0..w {
            buf.get_mut(x + dx, y)
                .set_char(glyph)
                .set_style(Style::default().fg(colour));
        }
    }
}

fn draw(buf: &mut Buffer, area: Rect, state: &SdrMetrics, theme: &crate::Theme) {
    let survey = &state.net.wifi.survey;
    let quiet = Style::default().fg(theme.label);
    let silent = silence(state);
    let held_all = silent.is_some();
    let lock = locked(state);
    // Rows: the plot, the channel, its value, a gap, the legend; the plot
    // takes whatever height is left.
    let rows = area.height.saturating_sub(5).max(3);
    let top = area.y;
    let floor = top + rows;
    let marks: &[(u16, &str)] = if rows >= 12 {
        &[
            (0, "40 %"),
            (1, "30 %"),
            (2, "20 %"),
            (3, "10 %"),
            (4, " 0 %"),
        ]
    } else {
        &[(0, "40 %"), (2, "20 %"), (4, " 0 %")]
    };
    let grid = Style::default().fg(theme.border_dim);
    for (q, label) in marks {
        let y = top + rows * q / 4;
        put(
            buf,
            area.x + 1,
            y,
            label,
            Style::default().fg(if held_all { theme.stale } else { theme.label }),
            area,
        );
        for x in area.x + PLOT_X..area.right().saturating_sub(1) {
            buf.get_mut(x, y).set_char('┈').set_style(grid);
        }
    }
    let col = (area.width.saturating_sub(PLOT_X + 1)) / 13;
    let bw = if col >= 10 {
        3
    } else if col >= 6 {
        2
    } else {
        1
    };
    let mut ticked = false;
    let mut families = [false; 7];
    for ch in 1..=13u8 {
        let x = area.x + PLOT_X + col * u16::from(ch - 1);
        let held = held_all || lock.is_some_and(|l| l != ch);
        if !held_all {
            match survey.busy(ch) {
                None => {
                    let dots = "·".repeat(usize::from(2 * bw + 1));
                    put(buf, x, floor - 1, &dots, grid, area);
                }
                Some(b) => {
                    let ink = if held { theme.stale } else { theme.value };
                    bar(buf, x, floor - 1, rows, bw, b.mean, ink);
                    // Frames, stacked by family: drawn tallest first so each
                    // shows above the one below it.
                    let air = survey.airtime(ch);
                    let mut layers = Vec::new();
                    let mut sum = 0.0;
                    for (i, f) in Family::ALL.iter().enumerate() {
                        if air[i] > 0.0 {
                            families[i] = true;
                            sum += air[i];
                            layers.push((sum, if held { theme.stale } else { theme.wifi(*f) }));
                        }
                    }
                    for (share, colour) in layers.iter().rev() {
                        bar(buf, x + bw + 1, floor - 1, rows, bw, *share, *colour);
                    }
                    if let Some(sigma) = b.sigma {
                        let h = (((b.mean + sigma) / FULL) * f64::from(rows * 2)).round() as u16;
                        let y = floor - 1 - h.min(rows * 2 - 1) / 2;
                        let tick = "─".repeat(usize::from(bw));
                        put(buf, x, y, &tick, Style::default().fg(ink), area);
                        ticked = true;
                    }
                }
            }
        }
        let bright = matches!(ch, 1 | 6 | 11);
        let label = if survey.selected == Some(ch) {
            Style::default()
                .fg(theme.value_hi)
                .add_modifier(Modifier::BOLD)
        } else if held {
            Style::default().fg(theme.stale)
        } else if bright {
            Style::default().fg(theme.value_hi)
        } else {
            Style::default().fg(theme.value)
        };
        let lx = x + bw.saturating_sub(1);
        if survey.selected == Some(ch) {
            put(buf, lx, floor + 1, &format!("▶{ch}"), label, area);
        } else {
            put(buf, lx + 1, floor + 1, &ch.to_string(), label, area);
        }
        if !held_all && col >= 5 {
            let v = survey.busy(ch).map_or("·".to_string(), |b| pct(b.mean));
            put(
                buf,
                x + bw.saturating_sub(1),
                floor + 2,
                &format!("{v:>4}"),
                quiet,
                area,
            );
        }
    }
    let y = floor + 4;
    if !held_all {
        let wide = col >= 10;
        let mut x = put(
            buf,
            area.x + 1,
            y,
            "█",
            Style::default().fg(theme.value),
            area,
        );
        x = put(
            buf,
            x + 1,
            y,
            if wide {
                "busy, any transmitter"
            } else {
                "busy"
            },
            Style::default().fg(theme.value),
            area,
        ) + 3;
        for (i, f) in Family::ALL.iter().enumerate() {
            if families[i] {
                x = put(buf, x, y, "█", Style::default().fg(theme.wifi(*f)), area);
                let name = if wide {
                    format!("{} frames", f.label())
                } else {
                    f.label().to_string()
                };
                x = put(buf, x + 1, y, &name, Style::default().fg(theme.value), area) + 3;
            }
        }
        if ticked {
            x = put(buf, x, y, "─ +1σ", quiet, area) + 3;
        }
        let bright = if wide {
            "1, 6, 11 bright: the three that do not overlap"
        } else {
            "bright: 1, 6, 11"
        };
        put(buf, x, y, bright, quiet, area);
    }
    if let Some(s) = silent {
        plate(buf, area, &s, theme);
    }
}

impl Panel for NetWifiSurveyPanel {
    fn name(&self) -> &'static str {
        "net_wifi_survey"
    }

    fn reads_spectrum(&self) -> bool {
        false
    }

    fn min_size(&self) -> (u16, u16) {
        (60, 12)
    }

    fn chrome(&self, state: &SdrMetrics) -> PanelChrome {
        let survey = &state.net.wifi.survey;
        let visits: usize = survey.visits.iter().map(Vec::len).sum();
        let visited = survey.visits.iter().filter(|v| !v.is_empty()).count();
        // SURVEY or LOCK is the mode's tag, as on every NET panel; the
        // suffix says where and how far.
        let place = match locked(state) {
            Some(ch) => format!(" · channel {ch}"),
            None => " · 1 to 13".to_string(),
        };
        let pass = if visited < 13 {
            format!(" · first pass, {visited} of 13")
        } else {
            String::new()
        };
        let mut chrome = PanelChrome::new("S_urvey")
            .stale_when(Staleness::NotStreaming)
            .tag_if(true, state.net.mode.tag())
            .counts_from_feed(FeedSpan::Session);
        chrome.suffix = Some(format!("{place} · {visits} visits{pass}"));
        chrome
    }

    fn focus_key(&self) -> Option<char> {
        Some('u')
    }

    fn focus_bindings(&self) -> &'static [(&'static str, &'static str)] {
        &[
            ("\u{2190}\u{2192}", "select a channel"),
            ("L", "lock the radio on it"),
            ("C", "start the survey again"),
        ]
    }

    fn render(
        &self,
        f: &mut Frame,
        inner: Rect,
        state: &SdrMetrics,
        theme: &crate::Theme,
        _focused: bool,
    ) {
        if inner.width < 20 || inner.height < 8 {
            return;
        }
        draw(f.buffer_mut(), inner, state, theme);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::fixture::draw as drawn;
    use crate::state::{NetMode, Visit, WifiSurvey};
    use ratatui::{backend::TestBackend, Terminal};

    /// The busiest megahertz of each channel at each of its six visits, as
    /// the survey recorded on 2026-10-14 heard them, and channel 11's DSSS
    /// frames.
    fn recorded(visits: usize) -> WifiSurvey {
        const BUSY: [[f64; 6]; 13] = [
            [0.0005, 0.0, 0.0, 0.0, 0.0, 0.0],
            [0.0005, 0.0, 0.0010, 0.0, 0.0, 0.0],
            [0.0, 0.0004, 0.0005, 0.0, 0.0, 0.0],
            [0.0013, 0.0115, 0.0008, 0.0, 0.0, 0.0],
            [0.0004, 0.0664, 0.0, 0.0, 0.0, 0.0],
            [0.0, 0.0648, 0.0, 0.0, 0.0, 0.0],
            [0.0, 0.0993, 0.0, 0.0, 0.0, 0.0],
            [0.0, 0.1051, 0.0, 0.0, 0.0002, 0.0],
            [0.0196, 0.6634, 0.0195, 0.0284, 0.0209, 0.0267],
            [0.0191, 0.7678, 0.0197, 0.0379, 0.0203, 0.6827],
            [0.0196, 0.5527, 0.0247, 0.0291, 0.0242, 0.5197],
            [0.0396, 0.2029, 0.0265, 0.0191, 0.0253, 0.5636],
            [0.0242, 0.0016, 0.0245, 0.0196, 0.0395, 0.7191],
        ];
        let mut s = WifiSurvey::default();
        for (i, row) in BUSY.iter().enumerate() {
            s.visits[i] = row[..visits]
                .iter()
                .map(|b| {
                    let mut v = Visit {
                        seconds: 0.475,
                        busy: *b,
                        ..Default::default()
                    };
                    if i == 10 {
                        v.airtime[5] = 0.18;
                        v.airtime[0] = 0.001;
                    }
                    v
                })
                .collect();
        }
        s
    }

    fn state(visits: usize) -> SdrMetrics {
        let mut m = SdrMetrics::fixture().streaming();
        m.radio.config_sample_rate = 20e6;
        m.net.wifi.survey = recorded(visits);
        m
    }

    fn buffer(m: &SdrMetrics, w: u16, h: u16) -> Buffer {
        let theme = crate::Theme::sdr();
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| NetWifiSurveyPanel.render(f, f.size(), m, &theme, false))
            .unwrap();
        t.backend().buffer().clone()
    }

    /// Where channel `ch`'s label is drawn: on the row that labels them
    /// all, the one holding both 12 and 13 below the plot.
    fn label_at(b: &Buffer, ch: u8) -> (u16, u16) {
        let rows: Vec<String> = (0..b.area.height)
            .map(|y| (0..b.area.width).map(|x| b.get(x, y).symbol()).collect())
            .collect();
        let y = rows
            .iter()
            .position(|r| r.contains(" 12") && r.contains(" 13") && !r.contains('%'))
            .expect("a label row");
        let at = rows[y]
            .match_indices(&format!(" {ch}"))
            .find(|(i, _)| {
                !rows[y][i + 1 + ch.to_string().len()..].starts_with(|c: char| c.is_ascii_digit())
            })
            .map(|(i, _)| rows[y][..i].chars().count() as u16 + 1)
            .expect("the label");
        (at, y as u16)
    }

    /// The labels row of drawn text: below the title, holding 12 and 13.
    fn labels(text: &str) -> &str {
        text.lines()
            .find(|l| {
                l.contains(" 12") && l.contains(" 13") && !l.contains("SURVEY") && !l.contains('%')
            })
            .expect("labels")
    }

    /// At the laptop's size every channel is labelled and carries its busy
    /// share; the title says how the survey stands.
    #[test]
    fn every_channel_is_labelled_with_its_busy_share() {
        let text = drawn(NetWifiSurveyPanel, 147, 27, &state(6)).join("\n");
        assert!(
            text.contains("SURVEY") && text.contains("1 to 13 · 78 visits"),
            "{text}"
        );
        let labels = labels(&text);
        for ch in 1..=13 {
            assert!(labels.contains(&ch.to_string()), "{ch}: {labels}");
        }
        assert!(text.contains("25.8") && text.contains("19.5"), "{text}");
        assert!(text.contains("busy, any transmitter"), "{text}");
        assert!(
            text.contains("DSSS frames") && text.contains("non-HT frames"),
            "{text}"
        );
        assert!(
            !text.contains("HT frames ") || text.contains("non-HT"),
            "{text}"
        );
    }

    /// The spread is ticked only from the third visit: with two, no tick
    /// and no `+1σ` in the legend.
    #[test]
    fn the_spread_is_ticked_only_from_three_visits() {
        let two = drawn(NetWifiSurveyPanel, 147, 27, &state(2)).join("\n");
        assert!(!two.contains("+1σ"), "{two}");
        let three = drawn(NetWifiSurveyPanel, 147, 27, &state(3)).join("\n");
        assert!(three.contains("+1σ"), "{three}");
    }

    /// A channel the survey has not reached is a dotted floor and a dot,
    /// not a zero; the title says how far the first pass has come.
    #[test]
    fn a_channel_not_visited_is_a_dotted_floor() {
        let mut m = state(1);
        for v in &mut m.net.wifi.survey.visits[4..] {
            v.clear();
        }
        let text = drawn(NetWifiSurveyPanel, 147, 27, &m).join("\n");
        assert!(text.contains("first pass, 4 of 13"), "{text}");
        assert!(text.contains("·······"), "{text}");
        let values = text.lines().find(|l| l.contains("0.1")).expect("values");
        assert_eq!(values.matches('·').count(), 9, "{values}");
    }

    /// Locked on 11, every other column is held in the stale colour and
    /// 11's is live; the title says LOCK.
    #[test]
    fn locked_the_other_channels_are_held_in_the_stale_colour() {
        let mut m = state(6);
        m.net.mode = NetMode::Lock;
        m.radio.frequency = 2_462_000_000;
        let theme = crate::Theme::sdr();
        let b = buffer(&m, 145, 25);
        let colour_at = |ch: u8| {
            let (x, y) = label_at(&b, ch);
            b.get(x, y).fg
        };
        assert_eq!(colour_at(10), theme.stale);
        assert_eq!(colour_at(11), theme.value_hi);
        assert!(drawn(NetWifiSurveyPanel, 147, 27, &m)
            .join("\n")
            .contains("channel 11 · 78 visits"));
    }

    /// 1, 6 and 11 are labelled bright, the others in the plain value
    /// colour; the selected channel is marked.
    #[test]
    fn the_three_that_do_not_overlap_are_bright_and_a_selection_is_marked() {
        let theme = crate::Theme::sdr();
        let mut m = state(6);
        let b = buffer(&m, 145, 25);
        let (x, y) = label_at(&b, 6);
        assert_eq!(b.get(x, y).fg, theme.value_hi);
        let (x, y) = label_at(&b, 7);
        assert_eq!(b.get(x, y).fg, theme.value);
        m.net.wifi.survey.selected = Some(10);
        let text = drawn(NetWifiSurveyPanel, 147, 27, &m).join("\n");
        assert!(text.contains("▶10"), "{text}");
    }

    /// What cannot be measured is said in a plate, with the key that helps,
    /// and no column is drawn under it.
    #[test]
    fn the_silences_say_what_and_which_key() {
        let mut idle = state(6);
        idle.radio.hw_streaming = false;
        let text = drawn(NetWifiSurveyPanel, 147, 27, &idle).join("\n");
        assert!(
            text.contains("NOT RECEIVING") && text.contains("Space"),
            "{text}"
        );
        assert!(!text.contains("25.8"), "{text}");
        let mut rate = state(6);
        rate.radio.config_sample_rate = 10e6;
        rate.net.wifi.refused = crate::signal::net::wifi::rate_refusal(10e6);
        let text = drawn(NetWifiSurveyPanel, 147, 27, &rate).join("\n");
        assert!(text.contains("WRONG SAMPLE RATE"), "{text}");
        assert!(text.contains("a Wi-Fi view needs 20.000 Msps"), "{text}");
    }

    /// At the three sizes D1 drew (the panel's share of 191 x 41, 152 x 32
    /// and 100 x 30), every channel's label is on screen and nothing is
    /// drawn past the frame.
    #[test]
    fn it_fits_every_size_the_view_is_drawn_at() {
        for (w, h) in [(147u16, 27u16), (114, 18), (100, 15), (60, 12)] {
            let lines = drawn(NetWifiSurveyPanel, w, h, &state(6));
            assert_eq!(lines.len(), usize::from(h));
            let labels = lines
                .iter()
                .find(|l| l.contains(" 12") && l.contains(" 13") && !l.contains("SURVEY"))
                .unwrap_or_else(|| panic!("{w}x{h}: {lines:#?}"));
            assert!(labels.chars().count() <= usize::from(w));
        }
    }
}
