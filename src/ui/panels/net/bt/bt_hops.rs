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
//! **Piconets told apart (net-ux-polish-plan 6.2).** Each hit is drawn in
//! its piconet's series colour (`Theme::series`), the colour given in the
//! order piconets were first heard, so it holds for the session and the
//! roster's rows wear the same one. With a piconet selected (here or in the
//! roster: one selection) it is drawn bold and the rest recede toward the
//! stale ink, so two overlapping hop patterns become one pattern and its
//! background. The grid is braille, two dots across and four down a cell,
//! and a hit fills its channel's whole band of dots, so twenty channels
//! keep twenty rows of their own on a ten-row panel and a single hit is
//! still visible at a glance. Where two piconets share a cell the selected
//! one owns its colour, then the one heard first.
//!
//! **Zoomed and scrubbed through `state::HopView`**, from half a second to a
//! minute, and back through what is kept; the chrome's `TimeWindow` tag says
//! which stretch of the past is on screen. The list of hits is capped
//! (`BT_HOP_LIMIT`), so a window reaching back before the oldest kept hit
//! says "older hits not kept" rather than drawing that part as a quiet band.
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
use crate::ui::widgets::timing_fmt::seconds_ms;

pub struct NetBtHopsPanel;

/// Columns reserved on the left for the channel-number scale, and the space
/// after it.
const SCALE_COLS: usize = 3;
/// Rows under the grid: the time axis and the legend.
const FOOT_ROWS: usize = 2;

/// Braille dot bits by `[row][column]` inside one cell (U+2800 block).
const DOT: [[u8; 2]; 4] = [[0x01, 0x08], [0x02, 0x10], [0x04, 0x20], [0x40, 0x80]];

/// The band of dot rows, counted from the bottom, that channel `idx` of `n`
/// watched ones fills across `dots` dot rows: at least one row each, the
/// lowest channel at the bottom the way frequency runs up any plot.
fn band(idx: usize, n: usize, dots: usize) -> std::ops::Range<usize> {
    let n = n.max(1);
    let lo = idx * dots / n;
    let hi = ((idx + 1) * dots / n).max(lo + 1).min(dots.max(1));
    lo..hi
}

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

/// One grid cell: its dots, and which piconet's colour it takes.
#[derive(Clone, Copy, Default)]
struct Cell {
    dots: u8,
    /// `(rank, lap)`, lowest rank wins: the selected piconet first, then by
    /// colour index.
    owner: Option<(usize, u32)>,
}

impl Panel for NetBtHopsPanel {
    fn name(&self) -> &'static str {
        "net_bt_hops"
    }

    fn min_size(&self) -> (u16, u16) {
        (30, 6)
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
            // A drop inside the stretch on screen is dots that should be
            // there and are not.
            .counts_from_feed(FeedSpan::Window(std::time::Duration::from_millis(
                v.span_ms() + v.back_ms,
            )))
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

        if let Some(reason) = &state.net.bt_refused {
            let mut lines = vec![Line::from(Span::styled(
                "not watching".to_string(),
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

        let channels = &state.net.bt_channels_watched;
        // "Nothing heard yet" and "too small to draw a grid" both fall back
        // to the same one-line status - the receiver's own state is the
        // thing worth saying either way, not an empty grid.
        let rows = (inner.height as usize).saturating_sub(FOOT_ROWS);
        let cols = width.saturating_sub(SCALE_COLS + 1);
        if state.net.bt_hops.is_empty() || channels.is_empty() || rows == 0 || cols < 8 {
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    format!("watching {} channels - no access codes yet", channels.len()),
                    Style::default().fg(theme.stale),
                ))),
                inner,
            );
            return;
        }

        let now = std::time::Instant::now();
        let view = state.net.hop_view;
        let (span, back) = (view.span_ms() as f64, view.back_ms as f64);
        let selected = state.net.bt_view.selected;
        let colours = colour_index(state);
        let rank = |lap: u32| {
            if Some(lap) == selected {
                0
            } else {
                1 + colours.get(&lap).copied().unwrap_or(usize::MAX - 1)
            }
        };

        let (dots_x, dots_y) = (cols * 2, rows * 4);
        let mut grid = vec![vec![Cell::default(); cols]; rows];
        // Hits in view per LAP, for the legend: what the plot shows.
        let mut in_view: HashMap<u32, u64> = HashMap::new();
        for hop in &state.net.bt_hops {
            let Some(idx) = channels.iter().position(|&c| c == hop.channel) else {
                continue;
            };
            let age = now.saturating_duration_since(hop.seen).as_secs_f64() * 1e3;
            if age < back || age >= back + span {
                continue;
            }
            *in_view.entry(hop.lap).or_default() += 1;
            let x = dots_x - 1 - (((age - back) / span) * dots_x as f64) as usize;
            for from_bottom in band(idx, channels.len(), dots_y) {
                let y = dots_y - 1 - from_bottom;
                let cell = &mut grid[y / 4][x / 2];
                cell.dots |= DOT[y % 4][x % 2];
                let claim = (rank(hop.lap), hop.lap);
                if cell.owner.is_none_or(|o| claim < o) {
                    cell.owner = Some(claim);
                }
            }
        }

        let ink = |lap: u32| {
            let c = colours
                .get(&lap)
                .map_or(theme.value_hi, |&i| theme.series_color(i));
            match selected {
                Some(s) if s != lap => Style::default().fg(theme.receded(c)),
                Some(_) => Style::default().fg(c).add_modifier(Modifier::BOLD),
                None => Style::default().fg(c),
            }
        };

        let mut lines = Vec::with_capacity(rows + FOOT_ROWS);
        for (r, cells) in grid.iter().enumerate() {
            // The channel whose band holds this row's lowest dot row.
            let bottom = dots_y - 1 - (r * 4 + 3);
            let label = (0..channels.len())
                .find(|&i| band(i, channels.len(), dots_y).contains(&bottom))
                .map(|i| channels[i].to_string())
                .unwrap_or_default();
            let mut spans = vec![Span::styled(
                format!("{label:>SCALE_COLS$} "),
                Style::default().fg(theme.label),
            )];
            for cell in cells {
                spans.push(match cell.owner {
                    Some((_, lap)) => Span::styled(
                        char::from_u32(0x2800 + cell.dots as u32)
                            .unwrap_or(' ')
                            .to_string(),
                        ink(lap),
                    ),
                    None => Span::raw(" "),
                });
            }
            lines.push(Line::from(spans));
        }

        // The time axis: where the window starts and where it ends, and, if
        // it reaches before the oldest hit the capped list still holds, that
        // the rest is not kept rather than quiet.
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
        let middle = if truncated { "older hits not kept" } else { "" };
        let gap = cols.saturating_sub(left.chars().count() + right.chars().count());
        let axis = format!(
            "{:SCALE_COLS$} {left}{:^gap$}{right}",
            "",
            if middle.chars().count() + 2 <= gap {
                middle
            } else {
                ""
            },
        );
        lines.push(Line::from(Span::styled(
            axis.chars().take(width).collect::<String>(),
            Style::default().fg(theme.label),
        )));

        lines.push(legend(&in_view, &colours, selected, width, theme));
        f.render_widget(Paragraph::new(lines), inner);
    }
}

/// `in view  ⣿ 0x9e8b33 145  ⣿ 0x123456 88`: each piconet on screen in its
/// colour, with its hits in the window, in colour order; what does not fit
/// is counted, and colours shared by two piconets in view are said.
fn legend(
    in_view: &HashMap<u32, u64>,
    colours: &HashMap<u32, usize>,
    selected: Option<u32>,
    width: usize,
    theme: &crate::Theme,
) -> Line<'static> {
    let mut shown: Vec<(usize, u32, u64)> = in_view
        .iter()
        .map(|(&lap, &n)| (colours.get(&lap).copied().unwrap_or(usize::MAX), lap, n))
        .collect();
    shown.sort();
    let repeats = {
        let mut seen = std::collections::HashSet::new();
        shown
            .iter()
            .any(|(i, ..)| !seen.insert(i % theme.series.len().max(1)))
    };
    let lead = if shown.is_empty() {
        "nothing in view".to_string()
    } else {
        "in view".to_string()
    };
    let mut spans = vec![Span::styled(lead.clone(), Style::default().fg(theme.label))];
    let mut used = lead.chars().count();
    let tail = if repeats { "  colours repeat" } else { "" };
    for (k, &(i, lap, n)) in shown.iter().enumerate() {
        let text = format!(" {lap:#08x} {n}");
        let more = shown.len() - k - 1;
        let reserve = if more > 0 {
            format!("  +{more}").chars().count()
        } else {
            0
        } + tail.chars().count();
        if used + 3 + text.chars().count() + reserve > width {
            let rest = format!("  +{}", shown.len() - k);
            if used + rest.chars().count() <= width {
                spans.push(Span::styled(rest.clone(), Style::default().fg(theme.label)));
                used += rest.chars().count();
            }
            break;
        }
        let c = theme.series_color(i);
        let (chip, word) = match selected {
            Some(s) if s != lap => (theme.receded(c), theme.label),
            Some(_) => (c, theme.value_hi),
            None => (c, theme.value),
        };
        spans.push(Span::raw("  "));
        spans.push(Span::styled("\u{28ff}", Style::default().fg(chip)));
        spans.push(Span::styled(text.clone(), Style::default().fg(word)));
        used += 3 + text.chars().count();
    }
    if repeats && used + tail.chars().count() <= width {
        spans.push(Span::styled(tail, Style::default().fg(theme.label)));
    }
    Line::from(spans)
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
        m.net.bt_hops.push_front(BtHop { channel, lap, seen });
    }

    fn braille(line: &str) -> usize {
        line.chars()
            .filter(|c| ('\u{2801}'..='\u{28ff}').contains(c))
            .count()
    }

    /// "No receiver at all" and "a receiver has heard nothing" are different
    /// claims, the same distinction the roster's refusal test makes.
    #[test]
    fn a_refusal_is_shown_rather_than_a_scatter() {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.bt_refused =
            Some("no classic Bluetooth channel fits inside the current 2.0 MHz view".to_string());
        let out = draw(NetBtHopsPanel, 60, 10, &m).join("\n");
        assert!(out.contains("not watching"), "{out}");
        assert!(out.contains("2.0") && out.contains("MHz"), "{out}");
    }

    /// The scatter spans the window on screen only: a loss minutes ago says
    /// nothing about the dots now drawn, and a loss inside it means dots that
    /// should be there are not.
    #[test]
    fn only_a_loss_inside_the_window_is_a_caveat_on_the_scatter() {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.bt_channels_watched = vec![10, 20, 30];

        m.net.health.last_loss = Some(Instant::now() - Duration::from_secs(300));
        let old = draw(NetBtHopsPanel, 80, 10, &m);
        assert!(!old[0].contains("FEED LOSS"), "{}", old[0]);

        m.net.health.last_loss = Some(Instant::now() - Duration::from_secs(2));
        let recent = draw(NetBtHopsPanel, 80, 10, &m);
        assert!(recent[0].contains("[FEED LOSS]"), "{}", recent[0]);
    }

    #[test]
    fn no_hits_yet_says_how_many_channels_are_watched() {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.bt_channels_watched = vec![10, 20, 30];
        let out = draw(NetBtHopsPanel, 60, 10, &m).join("\n");
        assert!(out.contains("watching 3 channels"), "{out}");
        assert!(!out.contains("not watching"), "{out}");
    }

    /// **Each channel keeps its own band.** Eight channels on eight grid
    /// rows: a hit just now on the fifth-lowest lands on the fourth row
    /// from the top, at the right edge, and nowhere else.
    #[test]
    fn a_recent_hit_lands_on_its_own_channels_row_near_now() {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.bt_channels_watched = (10..18).collect();
        hit(&mut m, 0x112233, 14, 0);
        // Inner 38 x 10: eight grid rows, the axis, the legend.
        let lines = draw(NetBtHopsPanel, 40, 12, &m);
        let grid: Vec<&String> = lines[1..9].iter().collect();
        let marked: Vec<usize> = (0..8).filter(|&r| braille(grid[r]) > 0).collect();
        assert_eq!(marked, vec![3], "{}", lines.join("\n"));
        let row = grid[3];
        let at = row
            .chars()
            .position(|c| ('\u{2801}'..='\u{28ff}').contains(&c))
            .unwrap();
        assert!(at + 4 >= row.chars().count(), "not near now: {row:?}");
    }

    /// A hit older than the window is not drawn: it happened, but not in
    /// the stretch on screen. Scrubbed back, it is.
    #[test]
    fn the_window_decides_what_is_drawn_and_scrubbing_moves_it() {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.bt_channels_watched = vec![10];
        hit(&mut m, 0x112233, 10, 30_000);
        let out = draw(NetBtHopsPanel, 40, 8, &m).join("\n");
        assert_eq!(braille(&out), 0, "{out}");
        assert!(out.contains("nothing in view"), "{out}");

        m.net.hop_view.back_ms = 20_000;
        let out = draw(NetBtHopsPanel, 70, 8, &m);
        assert!(braille(&out.join("\n")) > 0, "{}", out.join("\n"));
        assert!(out[0].contains("20 s \u{25c2} -20 s"), "{}", out[0]);
        assert!(out.join("\n").contains("-40 s"), "the axis starts there");
    }

    /// A hit on a channel no longer watched (a retune reshuffled the list)
    /// is not drawn on some other channel's row.
    #[test]
    fn a_hit_on_an_unwatched_channel_is_not_drawn() {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.bt_channels_watched = vec![10, 20];
        hit(&mut m, 0x112233, 50, 0);
        let out = draw(NetBtHopsPanel, 40, 8, &m).join("\n");
        assert_eq!(braille(&out), 0, "{out}");
    }

    /// **The legend counts what is drawn**, each piconet in view with its
    /// hits in the window, in the order they were first heard.
    #[test]
    fn the_legend_names_each_piconet_in_view_with_its_hits() {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.bt_channels_watched = (10..20).collect();
        for i in 0..5 {
            hit(&mut m, 0x9e8b33, 10 + i, 100 * i as u64);
        }
        hit(&mut m, 0x123456, 15, 50);
        hit(&mut m, 0x123456, 16, 60_000);
        let out = draw(NetBtHopsPanel, 60, 10, &m);
        let legend = out[out.len() - 2].clone();
        assert!(legend.contains("in view"), "{legend}");
        let a = legend.find("0x9e8b33 5").expect(&legend);
        let b = legend.find("0x123456 1").expect(&legend);
        assert!(a < b, "first heard first: {legend}");
    }

    /// **The selected piconet owns a shared cell.** Two piconets on one
    /// channel at one moment: with the second selected, the cell is drawn
    /// bold in its colour; with nothing selected, the one heard first owns
    /// it. Checked through the buffer's styles, which is what colour is.
    #[test]
    fn the_selected_piconet_owns_a_cell_it_shares() {
        use ratatui::{backend::TestBackend, Terminal};
        let mut m = SdrMetrics::fixture().streaming();
        m.net.bt_channels_watched = vec![10];
        hit(&mut m, 0xaaaaaa, 10, 0);
        hit(&mut m, 0xbbbbbb, 10, 0);
        let theme = crate::Theme::sdr();
        let cell_style = |m: &SdrMetrics| {
            let mut t = Terminal::new(TestBackend::new(30, 8)).unwrap();
            t.draw(|f| NetBtHopsPanel.render(f, f.size(), m, &theme, false))
                .unwrap();
            let buf = t.backend().buffer().clone();
            let at = buf
                .content()
                .iter()
                .find(|c| ('\u{2801}'..='\u{28ff}').contains(&c.symbol().chars().next().unwrap()))
                .expect("a mark")
                .clone();
            (at.fg, at.modifier.contains(Modifier::BOLD))
        };
        assert_eq!(cell_style(&m), (theme.series_color(0), false));
        m.net.bt_view.selected = Some(0xbbbbbb);
        assert_eq!(cell_style(&m), (theme.series_color(1), true));
    }

    /// With the capped list full and the window reaching before its oldest
    /// hit, the axis says the rest is not kept rather than letting it read
    /// as quiet.
    #[test]
    fn a_window_older_than_what_is_kept_says_so() {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.bt_channels_watched = vec![10];
        for i in 0..crate::state::BT_HOP_LIMIT as u64 {
            hit(&mut m, 0x112233, 10, 5_000 - i * 5);
        }
        let out = draw(NetBtHopsPanel, 70, 8, &m).join("\n");
        assert!(out.contains("older hits not kept"), "{out}");
        m.net.hop_view.zoom = 0;
        let out = draw(NetBtHopsPanel, 70, 8, &m).join("\n");
        assert!(
            !out.contains("not kept"),
            "half a second is all kept: {out}"
        );
    }

    #[test]
    fn it_fits_every_size_the_layout_can_hand_it() {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.bt_channels_watched = (10..30).collect();
        for i in 0..40u64 {
            hit(
                &mut m,
                0x100000 + (i % 9) as u32,
                10 + (i % 20) as u8,
                i * 300,
            );
        }
        m.net.bt_view.selected = Some(0x100003);
        for w in 30..80u16 {
            for h in 6..20u16 {
                for line in draw(NetBtHopsPanel, w, h, &m) {
                    assert!(line.chars().count() <= w as usize, "{w}x{h}: {line:?}");
                }
            }
        }
    }
}
