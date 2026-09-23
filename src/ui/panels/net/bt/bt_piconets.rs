// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `NetBtPiconetsPanel` - the classic preset's roster: one row per piconet
//! heard, and the selected one spelled out below (net-ux-polish-plan 6.1).
//!
//! **A roster of piconets, not of devices.** A LAP is the master's lower
//! address part and every member of its piconet sends it, so the title and
//! every label say piconet (`signal::bt::piconet` has the reasoning).
//!
//! **The UAP column says how far the narrowing got**, in the words
//! `net_bt_hops` already uses: one value once it is down to one, the number
//! of candidates left while a header alone cannot choose
//! (`signal::bt::header::PiconetClock` has the measured floor of two), and a
//! dash before any header of it has been decoded. The detail block says
//! which of those it is in a sentence.
//!
//! Replaces `net_bt_census`, which only ever said that nothing was decoded.

use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::signal::bt::piconet::{ordered, Piconet};
use crate::state::SdrMetrics;
use crate::ui::panel::{FeedSpan, Panel, PanelChrome, Staleness};
use crate::ui::widgets::table::{
    columns_that_fit, header, row, viewport_start, Align, Column, Sort,
};

pub struct NetBtPiconetsPanel;

const COLUMNS: &[Column] = &[
    Column {
        title: "LAP",
        // `● 0x9e8b33`: the hop panel's colour chip, then 24 bits.
        width: 10,
        align: Align::Left,
    },
    Column {
        title: "LAST",
        width: 6,
        align: Align::Right,
    },
    Column {
        title: "HITS",
        width: 6,
        align: Align::Right,
    },
    Column {
        title: "CH",
        width: 3,
        align: Align::Right,
    },
    Column {
        title: "UAP",
        // `32 left`: one header leaves 32 (`header::PiconetClock`).
        width: 7,
        align: Align::Right,
    },
    Column {
        title: "FIRST",
        width: 6,
        align: Align::Right,
    },
];

/// The column the roster is ordered by: the most recently heard first.
const ORDERED_BY: usize = 1;

/// The colour chip a piconet wears here and on the hop panel: solid, so the
/// colour carries in any font (a braille block drew as faint dots).
pub(crate) const CHIP: char = '\u{25cf}';

/// Rows the table keeps before the detail block may take any.
const TABLE_KEEPS: usize = 3;

/// Label width in the detail block.
const LABEL_W: usize = 9;

/// The same shape the census uses: a bench glances, it does not time.
fn ago(secs: u64) -> String {
    if secs < 90 {
        format!("{secs} s")
    } else {
        format!("{} min", secs / 60)
    }
}

/// The UAP cell: one value, candidates left, or a dash before any header.
fn uap_cell(uaps: Option<&Vec<u8>>) -> String {
    match uaps.map(|u| u.as_slice()) {
        Some([one]) => format!("{one:#04x}"),
        Some(many) if !many.is_empty() => format!("{} left", many.len()),
        _ => "-".to_string(),
    }
}

/// The UAP as a sentence, for the detail block.
fn uap_sentence(uaps: Option<&Vec<u8>>) -> String {
    match uaps.map(|u| u.as_slice()) {
        Some([one]) => format!("{one:#04x}"),
        // Listed while a reader can take them in; a first header leaves 32,
        // and 32 values are a wall, not a reading.
        Some(many) if (2..=4).contains(&many.len()) => format!(
            "{} candidates ({}); a header alone does not choose",
            many.len(),
            many.iter()
                .map(|u| format!("{u:#04x}"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Some(many) if !many.is_empty() => format!(
            "{} candidates; each further header narrows them",
            many.len()
        ),
        _ => "not narrowed: no header of it decoded yet".to_string(),
    }
}

fn cells(p: &Piconet, state: &SdrMetrics, now: std::time::Instant) -> Vec<String> {
    let since = |t: std::time::Instant| ago(now.saturating_duration_since(t).as_secs());
    vec![
        format!("{CHIP} {:#08x}", p.lap),
        since(p.last_seen),
        p.hits.to_string(),
        p.channels_hit().to_string(),
        uap_cell(state.net.bt_uap.get(&p.lap)),
        since(p.first_seen),
    ]
}

/// The channels a piconet was heard on, as runs: `2-5, 17, 40-41`.
fn channel_runs(mask: u128) -> String {
    let mut runs = Vec::new();
    let mut ch = 0u8;
    while ch < 79 {
        if mask & (1 << ch) == 0 {
            ch += 1;
            continue;
        }
        let start = ch;
        while ch + 1 < 79 && mask & (1 << (ch + 1)) != 0 {
            ch += 1;
        }
        runs.push(if start == ch {
            start.to_string()
        } else {
            format!("{start}-{ch}")
        });
        ch += 1;
    }
    runs.join(", ")
}

fn detail(
    p: &Piconet,
    state: &SdrMetrics,
    now: std::time::Instant,
    iw: usize,
    theme: &crate::Theme,
) -> Vec<Line<'static>> {
    let field = |label: &str, value: String| {
        Line::from(vec![
            crate::ui::chrome::field(label, LABEL_W, theme),
            Span::styled(value, Style::default().fg(theme.value)),
        ])
    };
    let since =
        |t: std::time::Instant| format!("{} ago", ago(now.saturating_duration_since(t).as_secs()));
    let mut out = vec![
        crate::ui::chrome::section("piconet", "", iw, theme),
        field("LAP", format!("{:#08x}  the master's", p.lap)),
        field("hits", p.hits.to_string()),
        field(
            "heard",
            format!("first {}, last {}", since(p.first_seen), since(p.last_seen)),
        ),
    ];
    let room = iw.saturating_sub(LABEL_W + 1);
    for (label, text) in [
        (
            "channels",
            // Of 79, not of the channels watched now: in SURVEY a piconet
            // was heard wherever the survey stood at the time.
            format!(
                "{} of 79: {}",
                p.channels_hit(),
                channel_runs(p.channel_mask())
            ),
        ),
        ("UAP", uap_sentence(state.net.bt_uap.get(&p.lap))),
    ] {
        for (i, chunk) in crate::ui::chrome::wrap(&text, room, 3)
            .into_iter()
            .enumerate()
        {
            out.push(field(if i == 0 { label } else { "" }, chunk));
        }
    }
    out
}

impl Panel for NetBtPiconetsPanel {
    fn name(&self) -> &'static str {
        "net_bt_piconets"
    }

    fn min_size(&self) -> (u16, u16) {
        (30, 6)
    }

    fn focus_key(&self) -> Option<char> {
        // The command rail's letter too: no NET layout shows the rail
        // (`app::FocusKeys`).
        Some('c')
    }

    fn focus_bindings(&self) -> &'static [(&'static str, &'static str)] {
        &[("↑↓", "select a piconet")]
    }

    fn chrome(&self, state: &SdrMetrics) -> PanelChrome {
        PanelChrome::new("Pi_conets")
            .stale_when(Staleness::NotStreaming)
            .tag_if(true, state.net.mode.tag())
            // Hits and first sightings accumulate for the session, so a drop
            // at any point in it undercounts them.
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
        let width = inner.width as usize;
        let note = |text: &str, ink| {
            crate::ui::chrome::wrap(text, width, 4)
                .into_iter()
                .map(move |chunk| Line::from(Span::styled(chunk, Style::default().fg(ink))))
        };

        // The three silences, each named (bar item 3).
        let roster = ordered(&state.net.bt_piconets);
        if roster.is_empty() {
            let mut lines: Vec<Line> = Vec::new();
            match &state.net.bt_refused {
                Some(reason) => {
                    lines.extend(note("not watching", theme.stale));
                    lines.extend(note(reason, theme.label));
                }
                None if state.net.bt_channels_watched.is_empty() => {
                    lines.extend(note("no classic receiver running", theme.stale));
                }
                None => lines.extend(note(
                    &format!(
                        "watching {} channels - no piconet heard yet",
                        state.net.bt_channels_watched.len()
                    ),
                    theme.stale,
                )),
            }
            f.render_widget(Paragraph::new(lines), inner);
            return;
        }

        let now = std::time::Instant::now();
        let fit = columns_that_fit(COLUMNS, width);
        let laps: Vec<u32> = roster.iter().map(|p| p.lap).collect();
        let cursor = state.net.bt_view.cursor(&laps);
        let height = inner.height as usize;

        // The detail block gives way to the table, as the census's does.
        let mut extra = cursor
            .map(|i| detail(roster[i], state, now, width, theme))
            .unwrap_or_default();
        if height < 1 + roster.len().min(TABLE_KEEPS) + extra.len() {
            extra.clear();
        }

        let mut lines = vec![header(
            COLUMNS,
            fit,
            Sort {
                column: ORDERED_BY,
                descending: false,
            },
            theme,
        )];
        // The rows the roster has, up to what the block leaves: the block
        // follows the last row rather than the foot of the panel, so a short
        // roster does not hold its detail a screen away from it.
        let body = height.saturating_sub(1 + extra.len()).min(roster.len());
        let start = viewport_start(
            state.net.bt_view.first_visible,
            cursor.unwrap_or(0),
            roster.len(),
            body,
        );
        for (i, p) in roster.iter().enumerate().skip(start).take(body) {
            let mut line = row(
                COLUMNS,
                fit,
                &cells(p, state, now),
                Some(i) == cursor,
                theme,
            );
            // The chip wears the colour the scatter draws this piconet in:
            // its place in `bt_piconets`, the order first heard.
            let colour = state
                .net
                .bt_piconets
                .iter()
                .position(|q| q.lap == p.lap)
                .map(|k| theme.series_color(k));
            if let (Some(colour), Some(cell)) = (colour, line.spans.get(1).cloned()) {
                let text = cell.content.to_string();
                if let Some(rest) = text.strip_prefix(CHIP) {
                    line.spans.splice(
                        1..2,
                        [
                            Span::styled(CHIP.to_string(), cell.style.fg(colour)),
                            Span::styled(rest.to_string(), cell.style),
                        ],
                    );
                }
            }
            lines.push(line);
        }
        if !extra.is_empty() && lines.len() + extra.len() < height {
            lines.push(Line::from(""));
        }
        lines.extend(extra);
        f.render_widget(Paragraph::new(lines), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::bt::piconet::observe;
    use crate::state::fixture::draw;
    use std::time::{Duration, Instant};

    fn heard() -> SdrMetrics {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.bt_channels_watched = (0..20).collect();
        let t = Instant::now() - Duration::from_secs(30);
        for (i, ch) in [2u8, 3, 4, 5, 17].into_iter().enumerate() {
            observe(
                &mut m.net.bt_piconets,
                0x9e8b33,
                ch,
                t + Duration::from_secs(i as u64),
            );
        }
        observe(&mut m.net.bt_piconets, 0x123456, 9, Instant::now());
        m.net.bt_uap.insert(0x9e8b33, vec![0x4c, 0x9a]);
        m.net.bt_uap.insert(0x123456, vec![0x21]);
        m
    }

    /// The three silences are three sentences (bar item 3): refused, not
    /// running, and listening with nothing heard.
    #[test]
    fn an_empty_roster_says_which_silence_it_is() {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.bt_refused = Some("no classic channel fits the view".to_string());
        let out = draw(NetBtPiconetsPanel, 50, 8, &m).join("\n");
        assert!(out.contains("not watching"), "{out}");
        assert!(out.contains("no classic channel fits"), "{out}");

        m.net.bt_refused = None;
        let out = draw(NetBtPiconetsPanel, 50, 8, &m).join("\n");
        assert!(out.contains("no classic receiver running"), "{out}");

        m.net.bt_channels_watched = vec![10, 20];
        let out = draw(NetBtPiconetsPanel, 50, 8, &m).join("\n");
        assert!(out.contains("watching 2 channels"), "{out}");
    }

    /// **One row per piconet, the most recently heard first**, with its
    /// hits, how many channels, and how far its UAP has narrowed.
    #[test]
    fn each_piconet_gets_a_row_the_newest_first() {
        let out = draw(NetBtPiconetsPanel, 50, 8, &heard());
        let text = out.join("\n");
        let newest = text.find("0x123456").expect(&text);
        let older = text.find("0x9e8b33").expect(&text);
        assert!(newest < older, "{text}");
        let row = out.iter().find(|l| l.contains("0x9e8b33")).unwrap();
        assert!(row.contains("2 left"), "{row}");
        assert!(row.contains(" 5 "), "five hits: {row}");
        let row = out.iter().find(|l| l.contains("0x123456")).unwrap();
        assert!(row.contains("0x21"), "{row}");
    }

    /// The selected piconet spelled out: the channels as runs, and the
    /// UAP's state in words.
    #[test]
    fn the_selected_piconet_is_spelled_out() {
        let mut m = heard();
        m.net.bt_view.selected = Some(0x9e8b33);
        let out = draw(NetBtPiconetsPanel, 60, 16, &m).join("\n");
        assert!(out.contains("PICONET"), "{out}");
        assert!(out.contains("5 of 79: 2-5, 17"), "{out}");
        assert!(out.contains("2 candidates (0x4c, 0x9a)"), "{out}");

        let none = draw(NetBtPiconetsPanel, 60, 16, &heard()).join("\n");
        assert!(!none.contains("PICONET"), "{none}");
    }

    /// **A first header leaves 32 candidates**, seen on the air: the cell
    /// holds `32 left` whole, and the detail says how many rather than
    /// listing a wall of values.
    #[test]
    fn many_candidates_are_counted_not_listed() {
        let mut m = heard();
        m.net
            .bt_uap
            .insert(0x9e8b33, (0..32).map(|i| i * 8 + 1).collect());
        m.net.bt_view.selected = Some(0x9e8b33);
        let out = draw(NetBtPiconetsPanel, 60, 16, &m).join("\n");
        assert!(out.contains("32 left"), "{out}");
        assert!(
            out.contains("32 candidates; each further header narrows them"),
            "{out}"
        );
        assert!(!out.contains("0x09"), "no list: {out}");
    }

    /// The detail follows the last row, not the foot of the panel: a short
    /// roster keeps its detail beside it.
    #[test]
    fn the_detail_follows_the_roster_rather_than_the_foot() {
        let mut m = heard();
        m.net.bt_view.selected = Some(0x9e8b33);
        let out = draw(NetBtPiconetsPanel, 60, 30, &m);
        let block = out.iter().position(|l| l.contains("PICONET")).unwrap();
        // The frame, the header, two rows, a gap.
        assert_eq!(block, 5, "{}", out.join("\n"));
    }

    #[test]
    fn channel_runs_join_neighbours() {
        assert_eq!(channel_runs(0), "");
        assert_eq!(channel_runs(0b1111 << 2 | 1 << 17 | 1 << 78), "2-5, 17, 78");
    }

    #[test]
    fn it_fits_every_size_the_layout_can_hand_it() {
        let mut m = heard();
        m.net.bt_view.selected = Some(0x9e8b33);
        for w in 30..70u16 {
            for h in 6..24u16 {
                for s in [&m, &SdrMetrics::fixture()] {
                    for line in draw(NetBtPiconetsPanel, w, h, s) {
                        assert!(line.chars().count() <= w as usize, "{w}x{h}: {line:?}");
                    }
                }
            }
        }
    }
}
