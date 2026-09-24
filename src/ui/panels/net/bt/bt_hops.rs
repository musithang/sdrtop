// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `NetBtHopsPanel` - B15's own exit condition made visible: a
//! time-versus-channel scatter of classic Bluetooth access-code hits,
//! design section 2.3's measurement 11 ("Bluetooth hop timing... makes a
//! piconet's existence obvious without decoding anything").
//!
//! **What is actually on screen is capped, and says so.** `signal::net::
//! worker` watches at most `[net].bt_channels` of however many classic BT
//! channels the current tuning lets it see at all - `signal::bt::receive`'s
//! own doc has the measured reason - so this panel's own "watching N
//! channels" line is not decoration, it is the honest scope of what the dots
//! below it could possibly show.
//!
//! Shares the refused-state shape `net_bt_piconets` uses: check
//! `state.net.bt_refused` first, because "no receiver exists" and "a
//! receiver exists and has heard nothing in the last window" are different
//! sentences.
//!
//! **Two questions, two zones (net-ux-polish-plan 6.2).** The first
//! version was a time-by-channel scatter, and on the air it showed almost
//! nothing: in SURVEY the rows were only the channels watched *now*, so hits
//! heard at an earlier survey position fell off the plot, and a live room
//! gave a few hits a minute, which a scatter built for 1600 hops a second
//! draws as a flicker. So the panel asks the two questions separately:
//!
//! - **WHERE**: all 79 channels, bars of the session's hits on each, so a
//!   piconet heard anywhere in the survey keeps its place. Each bar wears the
//!   colour of the piconet heard most on it; with one selected, its own hits
//!   are the coloured part and the rest stand faint above them. The channels
//!   watched now are underlined, so the reader sees where the radio is
//!   listening and where it is not.
//! - **WHEN**: one lane per piconet, the roster's order and colours, each hit
//!   a tick at its time in the window, like a logic analyser's traces. A few
//!   hits a minute read as a few ticks, and a busy piconet as a dense lane.
//!
//! Each piconet's colour is `Theme::series` at its place in `bt_piconets`, the
//! order first heard, so it holds for the session and the roster's rows wear
//! the same one; one selection (`bt_view`) drives both panels. The window is
//! zoomed and scrubbed through `state::HopView`, and the chrome's
//! `TimeWindow` tag says which stretch of the past the lanes show. The list
//! of hits is capped (`BT_HOP_LIMIT`), so a window reaching back before the
//! oldest kept hit says "older hits not kept" rather than drawing quiet lanes.
//!
//! The UAP each piconet has narrowed to is the roster's to show
//! (`net_bt_piconets`), one place for it (rule 6).

use std::collections::HashMap;

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::state::SdrMetrics;
use crate::ui::panel::{FeedSpan, Panel, PanelChrome, Staleness, Tag};
use crate::ui::panels::net::bt::bt_piconets::CHIP;
use crate::ui::widgets::timing_fmt::seconds_ms;

pub struct NetBtHopsPanel;

/// Classic Bluetooth channels, 0 to 78.
const CHANNELS: usize = 79;
/// The bars' left gutter: the count scale.
const SCALE: usize = 4;
/// A lane's left part: gutter, chip, space, LAP, space.
const LANE_LEAD: usize = 12;
/// A lane's right part: its hits in the window.
const LANE_TAIL: usize = 6;
/// The tallest the bars grow. Taller only magnified a count of one or two
/// into half a panel of block; the rows it leaves go between the zones.
const MAX_BAR_ROWS: usize = 8;
/// Vertical eighths, empty to full.
const EIGHTHS: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// Which colour index each LAP wears: its place in the roster, which is the
/// order piconets were first heard.
fn colour_index(state: &SdrMetrics) -> HashMap<u32, usize> {
    state
        .net
        .bt_piconets
        .iter()
        .enumerate()
        .map(|(i, p)| (p.lap, i))
        .collect()
}

/// The channels bar column `c` of `cols` covers: one each where 79 fit,
/// otherwise ranges in proportion, one or two channels wide, so the bars
/// fill the width and no channel is dropped. (Whole multiples first halved
/// the zone on a panel a few columns short of 79.)
fn channels_of(c: usize, cols: usize) -> std::ops::Range<usize> {
    let cols = cols.clamp(1, CHANNELS);
    c * CHANNELS / cols..(c + 1) * CHANNELS / cols
}

/// The column channel `ch` falls in.
fn column_of(ch: usize, cols: usize) -> usize {
    let cols = cols.clamp(1, CHANNELS);
    (0..cols)
        .find(|&c| channels_of(c, cols).contains(&ch))
        .unwrap_or(cols - 1)
}

/// The channel axis under the bars: every tenth channel labelled where the
/// label fits without touching the one before, and 78 at the end.
fn channel_axis(cols: usize) -> String {
    let mut axis = vec![' '; cols];
    let mut free = 0;
    for ch in (0..CHANNELS).step_by(10).chain([CHANNELS - 1]) {
        let at = column_of(ch, cols);
        let text = ch.to_string();
        let at = at.min(cols.saturating_sub(text.len()));
        if at < free || at + text.len() > cols {
            continue;
        }
        for (i, c) in text.chars().enumerate() {
            axis[at + i] = c;
        }
        free = at + text.len() + 1;
    }
    axis.into_iter().collect()
}

impl Panel for NetBtHopsPanel {
    fn name(&self) -> &'static str {
        "net_bt_hops"
    }

    fn min_size(&self) -> (u16, u16) {
        (40, 10)
    }

    fn focus_key(&self) -> Option<char> {
        // The Lab bars' letter too: no layout shows both (`app::FocusKeys`).
        Some('b')
    }

    fn focus_bindings(&self) -> &'static [(&'static str, &'static str)] {
        &[
            ("↑↓", "select a piconet"),
            ("+ -", "zoom in time"),
            ("← →", "back and forward in time"),
            ("End", "back to now"),
        ]
    }

    fn chrome(&self, state: &SdrMetrics) -> PanelChrome {
        let v = state.net.hop_view;
        PanelChrome::new("Classic _Bluetooth Hops")
            .stale_when(Staleness::NotStreaming)
            .tag_if(true, state.net.mode.tag())
            .tag_if(
                true,
                Tag::TimeWindow {
                    span_ms: v.span_ms(),
                    back_ms: v.back_ms,
                },
            )
            // The bars count the whole session, so a drop at any point in it
            // undercounts them.
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
        let (width, height) = (inner.width as usize, inner.height as usize);
        let note = |text: &str, ink| {
            crate::ui::chrome::wrap(text, width, 4)
                .into_iter()
                .map(move |chunk| Line::from(Span::styled(chunk, Style::default().fg(ink))))
        };

        if let Some(reason) = &state.net.bt_refused {
            let mut lines: Vec<Line> = note("not watching", theme.stale).collect();
            lines.extend(note(reason, theme.label));
            f.render_widget(Paragraph::new(lines), inner);
            return;
        }
        let watched = &state.net.bt_channels_watched;
        // Nothing heard, or no room for two zones: the receiver's own state
        // is the thing worth saying either way.
        if state.net.bt_piconets.is_empty() || height < 8 || width < SCALE + 20 {
            let text = if watched.is_empty() {
                "no classic receiver running".to_string()
            } else {
                format!("watching {} channels - no access codes yet", watched.len())
            };
            f.render_widget(
                Paragraph::new(note(&text, theme.stale).collect::<Vec<_>>()),
                inner,
            );
            return;
        }

        let colours = colour_index(state);
        let selected = state
            .net
            .bt_view
            .selected
            .filter(|s| colours.contains_key(s));
        let colour_of = |lap: u32| {
            colours
                .get(&lap)
                .map_or(theme.value_hi, |&i| theme.series_color(i))
        };

        // Rows: WHERE heading, bars, underline, axis; WHEN heading, lanes,
        // axis. The lanes are kept first, the bars take the rest, and the
        // "watched now" line only where there is room to spare.
        let roster = crate::signal::bt::piconet::ordered(&state.net.bt_piconets);
        let fixed = 5;
        let lane_room = height.saturating_sub(fixed + 2);
        let (lanes, more) = if roster.len() <= lane_room {
            (roster.len(), 0)
        } else {
            (
                lane_room.saturating_sub(1),
                roster.len() - lane_room.saturating_sub(1),
            )
        };
        let lane_rows = lanes + (more > 0) as usize;
        let spare = height - fixed - lane_rows;
        let watched_line = spare >= 4;
        let bar_rows = (spare - watched_line as usize).min(MAX_BAR_ROWS);
        let leftover = spare - watched_line as usize - bar_rows;
        // Past the row that parts the zones, the lanes grow taller, up to
        // three rows: a tick three rows high reads across the room.
        let lane_h = 1 + leftover
            .saturating_sub(1)
            .checked_div(lanes)
            .unwrap_or(0)
            .min(2);

        let mut lines = Vec::with_capacity(height);
        lines.push(crate::ui::chrome::section(
            "where",
            "hits per channel, session",
            width,
            theme,
        ));
        lines.extend(where_zone(
            state, &colours, selected, width, bar_rows, theme,
        ));
        if watched_line {
            let text = match (watched.first(), watched.last()) {
                (Some(lo), Some(hi)) if lo != hi => format!(" watched now: {lo}-{hi}"),
                (Some(lo), _) => format!(" watched now: {lo}"),
                _ => " watching nothing now".to_string(),
            };
            lines.push(Line::from(vec![
                Span::raw(" ".repeat(SCALE)),
                Span::styled("\u{2594}", Style::default().fg(theme.border_accent)),
                Span::styled(text, Style::default().fg(theme.label)),
            ]));
        }

        // What the capped bars leave: one row to part the zones, the rest
        // under the lanes.
        if leftover > 0 {
            lines.push(Line::from(""));
        }
        let view = state.net.hop_view;
        lines.push(crate::ui::chrome::section(
            "when",
            &format!("hits in the last {}", seconds_ms(view.span_ms())),
            width,
            theme,
        ));
        let lane_w = width.saturating_sub(LANE_LEAD + LANE_TAIL).max(1);
        let now = std::time::Instant::now();
        let (span, back) = (view.span_ms() as f64, view.back_ms as f64);
        for p in roster.iter().take(lanes) {
            let mut ticks = vec![0u32; lane_w];
            for hop in state.net.bt_hops.iter().filter(|h| h.lap == p.lap) {
                let age = now.saturating_duration_since(hop.seen).as_secs_f64() * 1e3;
                if age < back || age >= back + span {
                    continue;
                }
                ticks[lane_w - 1 - (((age - back) / span) * lane_w as f64) as usize] += 1;
            }
            let n: u32 = ticks.iter().sum();
            let is = Some(p.lap) == selected;
            let recede = selected.is_some() && !is;
            let c = colour_of(p.lap);
            let ink = if recede { theme.receded(c) } else { c };
            let tick_style = if is {
                Style::default().fg(ink).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(ink)
            };
            let name_style = match (is, recede) {
                (true, _) => Style::default()
                    .fg(theme.value_hi)
                    .add_modifier(Modifier::BOLD),
                (_, true) => Style::default().fg(theme.label),
                _ => Style::default().fg(theme.value),
            };
            // The rows above the last carry the ticks only; the last carries
            // the name, the dotted guide and the count, so a lane reads as a
            // trace on its baseline.
            for row in 0..lane_h {
                let base = row + 1 == lane_h;
                let mut spans = if base {
                    vec![
                        crate::ui::chrome::selection_gutter(is, theme),
                        Span::styled(format!("{CHIP} "), Style::default().fg(ink)),
                        Span::styled(
                            format!("{} ", super::bt_piconets::lap_name(p.lap)),
                            name_style,
                        ),
                    ]
                } else {
                    vec![
                        crate::ui::chrome::selection_gutter(is, theme),
                        Span::raw(" ".repeat(LANE_LEAD - 1)),
                    ]
                };
                for (k, &t) in ticks.iter().enumerate() {
                    spans.push(match (t, base) {
                        (0, true) => Span::styled(
                            if k % 10 == 0 { "\u{250a}" } else { "\u{00b7}" },
                            Style::default().fg(theme.stale),
                        ),
                        (0, false) => Span::raw(" "),
                        (1, _) => Span::styled("\u{2503}", tick_style),
                        _ => Span::styled("\u{2588}", tick_style),
                    });
                }
                if base {
                    spans.push(Span::styled(
                        format!("{n:>LANE_TAIL$}"),
                        Style::default().fg(theme.label),
                    ));
                }
                lines.push(Line::from(spans));
            }
        }
        if more > 0 {
            lines.push(Line::from(Span::styled(
                format!(" +{more} more piconets in the roster"),
                Style::default().fg(theme.label),
            )));
        }

        // The time axis, and whether the window reaches before what is kept.
        let left = format!("-{}", seconds_ms((back + span) as u64));
        let right = if back == 0.0 {
            "now".to_string()
        } else {
            format!("-{}", seconds_ms(back as u64))
        };
        let oldest_age = state
            .net
            .bt_hops
            .back()
            .map(|h| now.saturating_duration_since(h.seen).as_secs_f64() * 1e3);
        let truncated = state.net.bt_hops.len() >= crate::state::BT_HOP_LIMIT
            && oldest_age.is_some_and(|a| a < back + span);
        let gap = lane_w.saturating_sub(left.chars().count() + right.chars().count());
        let middle = if truncated && "older hits not kept".len() + 2 <= gap {
            "older hits not kept"
        } else {
            ""
        };
        let axis = format!("{:LANE_LEAD$}{left}{middle:^gap$}{right}", "");
        lines.push(Line::from(Span::styled(
            axis.chars().take(width).collect::<String>(),
            Style::default().fg(theme.label),
        )));

        f.render_widget(Paragraph::new(lines), inner);
    }
}

/// The WHERE zone: `rows` of bars, the underline of channels watched now,
/// and the channel axis.
fn where_zone(
    state: &SdrMetrics,
    colours: &HashMap<u32, usize>,
    selected: Option<u32>,
    width: usize,
    rows: usize,
    theme: &crate::Theme,
) -> Vec<Line<'static>> {
    let cols = width.saturating_sub(SCALE).clamp(1, CHANNELS);

    // Per column: all hits, the selected piconet's, the piconet heard most
    // there, and the one heard most among the others (whose colour the
    // faint part wears when one is selected).
    let mut total = vec![0u64; cols];
    let mut mine = vec![0u64; cols];
    let mut lead: Vec<Option<(u64, usize)>> = vec![None; cols];
    let mut lead_other: Vec<Option<(u64, usize)>> = vec![None; cols];
    for p in &state.net.bt_piconets {
        let idx = colours.get(&p.lap).copied().unwrap_or(0);
        for c in 0..cols {
            let n: u64 = p.per_channel[channels_of(c, cols)]
                .iter()
                .map(|&n| n as u64)
                .sum();
            if n == 0 {
                continue;
            }
            total[c] += n;
            if Some(p.lap) == selected {
                mine[c] += n;
            }
            // Most hits wins; a tie goes to the one heard first.
            let wins =
                |l: Option<(u64, usize)>| l.is_none_or(|(m, i)| n > m || (n == m && idx < i));
            if wins(lead[c]) {
                lead[c] = Some((n, idx));
            }
            if Some(p.lap) != selected && wins(lead_other[c]) {
                lead_other[c] = Some((n, idx));
            }
        }
    }
    let max = total.iter().copied().max().unwrap_or(0).max(1);
    let eighths = |n: u64| (n as f64 / max as f64 * rows as f64 * 8.0).round() as usize;

    let mut out = Vec::with_capacity(rows + 2);
    for r in 0..rows {
        let base = (rows - 1 - r) * 8;
        let scale = if r == 0 {
            format!("{max:>3} ")
        } else if r + 1 == rows {
            format!("{:>3} ", 0)
        } else {
            " ".repeat(SCALE)
        };
        let mut spans = vec![Span::styled(scale, Style::default().fg(theme.label))];
        for c in 0..cols {
            let fill = |n: u64| eighths(n).saturating_sub(base).min(8);
            let (own, ink) = match selected {
                Some(s) => (
                    fill(mine[c]),
                    Style::default()
                        .fg(theme.series_color(colours.get(&s).copied().unwrap_or(0)))
                        .add_modifier(Modifier::BOLD),
                ),
                None => (
                    fill(total[c]),
                    Style::default()
                        .fg(lead[c].map_or(theme.label, |(_, i)| theme.series_color(i))),
                ),
            };
            let rest = fill(total[c]);
            spans.push(if own > 0 {
                Span::styled(EIGHTHS[own].to_string(), ink)
            } else if rest > 0 {
                // The others' hits above the selected piconet's, receded in
                // the colour of the one heard most among them, so a faint
                // bar still says whose it is.
                Span::styled(
                    EIGHTHS[rest].to_string(),
                    Style::default().fg(theme.receded(
                        lead_other[c].map_or(theme.label, |(_, i)| theme.series_color(i)),
                    )),
                )
            } else {
                Span::raw(" ")
            });
        }
        out.push(Line::from(spans));
    }

    let watched = &state.net.bt_channels_watched;
    let mut under = vec![Span::raw(" ".repeat(SCALE))];
    for c in 0..cols {
        let chans = channels_of(c, cols);
        under.push(if watched.iter().any(|&w| chans.contains(&(w as usize))) {
            Span::styled("\u{2594}", Style::default().fg(theme.border_accent))
        } else if chans.clone().any(|ch| ch % 10 == 0) {
            Span::styled("\u{00b7}", Style::default().fg(theme.stale))
        } else {
            Span::raw(" ")
        });
    }
    out.push(Line::from(under));
    out.push(Line::from(Span::styled(
        format!("{}{}", " ".repeat(SCALE), channel_axis(cols)),
        Style::default().fg(theme.label),
    )));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::bt::piconet::observe;
    use crate::state::{fixture::draw, BtHop};
    use std::time::{Duration, Instant};

    /// A hit on `channel` from `lap`, `ago_ms` before now, counted in the
    /// roster too, the way the worker does both.
    fn hit(m: &mut SdrMetrics, lap: u32, channel: u8, ago_ms: u64) {
        let seen = Instant::now() - Duration::from_millis(ago_ms);
        observe(&mut m.net.bt_piconets, lap, channel, seen);
        m.net.bt_hops.push_front(BtHop {
            channel,
            lap,
            seen,
            at_us: 0.0,
            stream: 0,
            header: None,
        });
    }

    fn lane<'a>(out: &'a [String], lap: &str) -> &'a String {
        out.iter()
            .find(|l| l.contains(lap) && l.contains(CHIP))
            .unwrap_or_else(|| panic!("no lane for {lap}:\n{}", out.join("\n")))
    }

    fn ticks(line: &str) -> usize {
        line.chars()
            .filter(|&c| c == '\u{2503}' || c == '\u{2588}')
            .count()
    }

    /// "No receiver at all" and "a receiver has heard nothing" are different
    /// claims, the same distinction the roster's refusal test makes.
    #[test]
    fn a_refusal_is_shown_rather_than_the_zones() {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.bt_refused =
            Some("no classic Bluetooth channel fits inside the current 2.0 MHz view".to_string());
        let out = draw(NetBtHopsPanel, 60, 12, &m).join("\n");
        assert!(out.contains("not watching"), "{out}");
        assert!(out.contains("2.0") && out.contains("MHz"), "{out}");
    }

    #[test]
    fn no_hits_yet_says_how_many_channels_are_watched() {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.bt_channels_watched = vec![10, 20, 30];
        let out = draw(NetBtHopsPanel, 60, 12, &m).join("\n");
        assert!(out.contains("watching 3 channels"), "{out}");
        assert!(!out.contains("WHERE"), "{out}");
    }

    /// **WHERE keeps every hit in its place**, on all 79 channels: in SURVEY
    /// a hit heard at an earlier survey position is not lost because the
    /// radio is listening elsewhere now. At 83 columns one channel is one
    /// column, so a hit on channel 68 is a bar at column 68 while only 75 to
    /// 78 are watched, and those four are underlined.
    #[test]
    fn where_places_every_hit_on_its_channel_whatever_is_watched_now() {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.bt_channels_watched = vec![75, 76, 77, 78];
        hit(&mut m, 0xa4b109, 68, 43_000);
        let out = draw(NetBtHopsPanel, 85, 16, &m);
        let bar = out
            .iter()
            .find(|l| l.contains('\u{2588}') && !l.contains(CHIP))
            .expect("a bar");
        let col = |line: &str, c: char| line.chars().position(|x| x == c);
        // Border, then the scale, then channel 0.
        assert_eq!(col(bar, '\u{2588}'), Some(1 + SCALE + 68), "{bar}");
        let under = out.iter().find(|l| l.contains('\u{2594}')).unwrap();
        assert_eq!(col(under, '\u{2594}'), Some(1 + SCALE + 75), "{under}");
        assert!(out.join("\n").contains("watched now: 75-78"));
    }

    /// **WHEN gives each piconet a lane**, a tick per hit in the window, and
    /// counts them; a hit older than the window is in WHERE but not in WHEN.
    #[test]
    fn when_gives_each_piconet_a_lane_of_its_hits_in_the_window() {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.bt_channels_watched = (0..20).collect();
        for i in 0..3 {
            hit(&mut m, 0x0a3777, 5, 1_000 + i * 4_000);
        }
        hit(&mut m, 0xba5a11, 7, 2_000);
        hit(&mut m, 0xba5a11, 8, 40_000);
        let out = draw(NetBtHopsPanel, 80, 16, &m);
        let a = lane(&out, "0x0a3777");
        assert_eq!(ticks(a), 3, "{a}");
        assert!(
            a.trim_end_matches('\u{2502}').trim_end().ends_with('3'),
            "{a}"
        );
        let b = lane(&out, "0xba5a11");
        assert_eq!(ticks(b), 1, "the 40 s old hit is outside 20 s: {b}");

        m.net.hop_view.zoom = 6;
        let out = draw(NetBtHopsPanel, 80, 16, &m);
        assert_eq!(ticks(lane(&out, "0xba5a11")), 2, "a minute holds both");
        assert!(out[0].contains("60 s \u{25c2} now"), "{}", out[0]);
    }

    /// Scrubbed back, the window moves, the tag and the axis say where to.
    #[test]
    fn scrubbing_moves_the_window_and_says_where() {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.bt_channels_watched = vec![10];
        hit(&mut m, 0x112233, 10, 30_000);
        let out = draw(NetBtHopsPanel, 70, 14, &m);
        assert_eq!(ticks(lane(&out, "0x112233")), 0);
        m.net.hop_view.back_ms = 20_000;
        let out = draw(NetBtHopsPanel, 70, 14, &m);
        assert_eq!(ticks(lane(&out, "0x112233")), 1);
        assert!(out[0].contains("20 s \u{25c2} -20 s"), "{}", out[0]);
        assert!(out.join("\n").contains("-40 s"), "the axis starts there");
    }

    /// **One selection, both zones.** With a piconet selected its lane is
    /// marked and bold, and the other lanes and bars recede. Checked through
    /// the buffer's styles, which is what colour is.
    #[test]
    fn a_selected_piconet_leads_and_the_rest_recede() {
        use ratatui::{backend::TestBackend, Terminal};
        let mut m = SdrMetrics::fixture().streaming();
        m.net.bt_channels_watched = vec![10];
        hit(&mut m, 0xaaaaaa, 10, 0);
        hit(&mut m, 0xbbbbbb, 20, 0);
        m.net.bt_view.selected = Some(0xbbbbbb);
        let theme = crate::Theme::sdr();
        let mut t = Terminal::new(TestBackend::new(90, 16)).unwrap();
        t.draw(|f| NetBtHopsPanel.render(f, f.size(), &m, &theme, false))
            .unwrap();
        let buf = t.backend().buffer().clone();
        let row_text = |y: u16| (0..90).map(|x| buf.get(x, y).symbol()).collect::<String>();
        let tick_on = |lap: &str| {
            let y = (0..16).find(|&y| row_text(y).contains(lap)).unwrap();
            let x = (0..90)
                .find(|&x| buf.get(x, y).symbol() == "\u{2503}")
                .unwrap();
            buf.get(x, y).clone()
        };
        let sel = tick_on("0xbbbbbb");
        assert_eq!(sel.fg, theme.series_color(1));
        assert!(sel.modifier.contains(Modifier::BOLD));
        assert_eq!(tick_on("0xaaaaaa").fg, theme.receded(theme.series_color(0)));
        let y = (0..16).find(|&y| row_text(y).contains("0xbbbbbb")).unwrap();
        assert_eq!(buf.get(0, y).symbol(), "\u{258c}", "the lane is marked");
    }

    /// **A count of one is not half a panel.** On a tall panel the bars
    /// stop at [`MAX_BAR_ROWS`], and with one piconet selected another's
    /// faint bar wears its own colour, receded, so it still says whose it is.
    #[test]
    fn bars_are_capped_and_a_faint_bar_keeps_its_owners_colour() {
        use ratatui::{backend::TestBackend, Terminal};
        let mut m = SdrMetrics::fixture().streaming();
        m.net.bt_channels_watched = vec![10];
        hit(&mut m, 0xaaaaaa, 10, 0);
        hit(&mut m, 0xbbbbbb, 20, 0);
        let out = draw(NetBtHopsPanel, 90, 40, &m);
        let bar_rows = out
            .iter()
            .filter(|l| l.contains('\u{2588}') && !l.contains(CHIP))
            .count();
        assert_eq!(bar_rows, MAX_BAR_ROWS, "{}", out.join("\n"));

        m.net.bt_view.selected = Some(0xbbbbbb);
        let theme = crate::Theme::sdr();
        let mut t = Terminal::new(TestBackend::new(90, 20)).unwrap();
        t.draw(|f| NetBtHopsPanel.render(f, f.size(), &m, &theme, false))
            .unwrap();
        let buf = t.backend().buffer().clone();
        // Column of channel 10: the scale, then channel 0.
        let x = (SCALE + 10) as u16;
        let y = (0..20)
            .find(|&y| buf.get(x, y).symbol() == "\u{2588}")
            .expect("channel 10's bar");
        assert_eq!(buf.get(x, y).fg, theme.receded(theme.series_color(0)));
    }

    /// **The room the capped bars leave goes to the lanes**: on a tall panel
    /// a hit is a tick three rows high, its lane's name on the baseline.
    #[test]
    fn a_tall_panel_gives_the_lanes_three_rows() {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.bt_channels_watched = vec![10];
        hit(&mut m, 0xaaaaaa, 10, 1_000);
        hit(&mut m, 0xbbbbbb, 20, 2_000);
        let out = draw(NetBtHopsPanel, 90, 40, &m);
        let base = out.iter().position(|l| l.contains("0xaaaaaa")).unwrap();
        let col = out[base].chars().position(|c| c == '\u{2503}').unwrap();
        let tall = (base - 2..=base)
            .filter(|&y| out[y].chars().nth(col) == Some('\u{2503}'))
            .count();
        assert_eq!(tall, 3, "{}", out.join("\n"));
    }

    /// With the capped list full and the window reaching before its oldest
    /// hit, the axis says the rest is not kept rather than letting the lanes
    /// read as quiet.
    #[test]
    fn a_window_older_than_what_is_kept_says_so() {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.bt_channels_watched = vec![10];
        for i in 0..crate::state::BT_HOP_LIMIT as u64 {
            hit(&mut m, 0x112233, 10, 5_000 - i * 5);
        }
        let out = draw(NetBtHopsPanel, 70, 12, &m).join("\n");
        assert!(out.contains("older hits not kept"), "{out}");
        m.net.hop_view.zoom = 0;
        let out = draw(NetBtHopsPanel, 70, 12, &m).join("\n");
        assert!(
            !out.contains("not kept"),
            "half a second is all kept: {out}"
        );
    }

    /// More piconets than lanes: the rest are counted, not dropped quietly.
    #[test]
    fn piconets_beyond_the_lanes_are_counted() {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.bt_channels_watched = vec![10];
        for i in 0..12u32 {
            hit(&mut m, 0x100000 + i, 10, i as u64 * 100);
        }
        let out = draw(NetBtHopsPanel, 70, 12, &m).join("\n");
        assert!(out.contains("more piconets in the roster"), "{out}");
    }

    #[test]
    fn the_channel_axis_labels_what_fits() {
        assert!(channel_axis(79).starts_with("0         10        20"));
        assert!(channel_axis(79).ends_with("78"));
        let narrow = channel_axis(40);
        assert_eq!(narrow.chars().count(), 40);
        assert!(narrow.starts_with("0    10"), "{narrow}");
    }

    /// **A panel a few columns short of 79 keeps its width**: every column
    /// is used, every channel is in exactly one, and none is more than two
    /// wide.
    #[test]
    fn channel_columns_fill_the_width_and_cover_every_channel() {
        for cols in [20, 40, 76, 78, 79] {
            let mut seen = [0; CHANNELS];
            for c in 0..cols {
                let r = channels_of(c, cols);
                assert!((1..=4).contains(&r.len()), "{cols}: column {c} is {r:?}");
                for ch in r {
                    seen[ch] += 1;
                }
            }
            assert!(seen.iter().all(|&n| n == 1), "{cols}: {seen:?}");
        }
        assert!(channels_of(10, 76).len() <= 2);
    }

    #[test]
    fn it_fits_every_size_the_layout_can_hand_it() {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.bt_channels_watched = (10..30).collect();
        for i in 0..40u64 {
            hit(
                &mut m,
                0x100000 + (i % 9) as u32,
                (i * 7 % 79) as u8,
                i * 300,
            );
        }
        m.net.bt_view.selected = Some(0x100003);
        for w in 30..100u16 {
            for h in 6..24u16 {
                for line in draw(NetBtHopsPanel, w, h, &m) {
                    assert!(line.chars().count() <= w as usize, "{w}x{h}: {line:?}");
                }
            }
        }
    }
}
