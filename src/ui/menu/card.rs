// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The card: what the row under the cursor is, in plain words.
//!
//! Top to bottom: its name and the key that opens it, the blurb, a refusal
//! when this radio cannot open it, what it shows, the live line, where it
//! listens, and a map of what opens. Every part is plain data handed in, so
//! the card says nothing the tree, the live line or the placement did not:
//! a refusal is the gate's sentence, the reach is the radio's declaration,
//! the map is the deck's own arithmetic.

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use super::live::Live;
use super::ruler::{self, Mark};
use super::tree::{Kind, Node};
use super::{ink, map};

/// Everything the card shows.
pub struct Card<'a> {
    /// The rows from the first column down to the one under the cursor,
    /// that one last ([`super::tree::Tree::trail`]).
    pub trail: &'a [&'a Node],
    /// The board's name, its reach `min_hz..=max_hz`, and its gaps.
    pub radio: ruler::Reach<'a>,
    pub live: Option<Live>,
    /// The view's panels where the deck would put them on `screen`
    /// (`LayoutEngine::placement_of`); empty for anything but a view.
    pub placed: &'a [(String, Rect)],
    pub screen: Rect,
    /// On Resume: where the radio was left, said above the view's card.
    pub left_off: Option<&'a str>,
}

/// Width below which the map goes under the words instead of beside them.
const SIDE_BY_SIDE: u16 = 84;

pub fn render(f: &mut Frame, area: Rect, card: &Card<'_>, theme: &crate::Theme) {
    let Some(node) = card.trail.last().copied() else {
        return;
    };
    if area.width < 12 || area.height == 0 {
        return;
    }
    let beside = area.width >= SIDE_BY_SIDE && !card.placed.is_empty();
    let text_w = if beside {
        (area.width - 34).min(64)
    } else {
        area.width.min(64)
    };
    let (lines, used) = words(node, card, text_w as usize, theme);
    let text = Rect::new(area.x, area.y, text_w, area.height);
    f.render_widget(Paragraph::new(lines), text);

    let mut y = area.y + used;
    if let Some(mark) = mark(card) {
        if y + 1 + ruler::HEIGHT <= area.bottom() {
            y += 1;
            heading(f, area.x, y, "WHERE IT LISTENS", theme);
            let at = Rect::new(area.x, y + 1, text_w, ruler::HEIGHT);
            ruler::render(f.buffer_mut(), at, card.radio, mark, theme);
            y += 1 + ruler::HEIGHT;
        }
    }

    if card.placed.is_empty() {
        return;
    }
    let (mx, my, mw) = if beside {
        let x = area.x + text_w + 4;
        let top = if card.left_off.is_some() { 6 } else { 3 };
        (x, area.y + top, (area.right() - x).min(48))
    } else {
        (area.x, y + 1, text_w.min(48))
    };
    let mh = area.bottom().saturating_sub(my + 1).min(12);
    if mh >= 6 {
        heading(f, mx, my, "WHAT OPENS", theme);
        map::render(
            f.buffer_mut(),
            Rect::new(mx, my + 1, mw, mh),
            card.screen,
            card.placed,
            theme,
        );
    }
}

fn heading(f: &mut Frame, x: u16, y: u16, text: &str, theme: &crate::Theme) {
    f.buffer_mut().set_string(
        x,
        y,
        text,
        Style::default()
            .fg(theme.border_dim)
            .add_modifier(Modifier::BOLD),
    );
}

/// The ruler's mark: the band of the nearest row on the trail that has one;
/// a Lab view or area with none works wherever the radio is tuned; a Radio
/// row with none has nothing to mark yet.
fn mark<'a>(card: &'a Card<'a>) -> Option<Mark<'a>> {
    let node = card.trail.last()?;
    if let Some(banded) = card.trail.iter().rev().find(|n| n.band.is_some()) {
        let band = banded.band?;
        // The band's own row is named for it (`2.4 GHz`); an area above it
        // carries the same words as its hint.
        let name = if banded.title.contains("Hz") {
            banded.title.as_str()
        } else {
            banded.hint.unwrap_or("band")
        };
        return Some(Mark::Band(band, name));
    }
    // No band anywhere above: a view, or a group of views, works wherever
    // the radio is tuned. An area over several bands (Wi-Fi) says nothing
    // here; its bands say where they are.
    let tuned_anywhere = match &node.kind {
        Kind::View(_) => true,
        Kind::Branch { section, .. } => section.is_some(),
        Kind::Heading => false,
    };
    (!node.later && tuned_anywhere).then_some(Mark::Anywhere)
}

/// The words, and how many rows they took.
fn words<'a>(node: &Node, card: &Card<'_>, w: usize, theme: &crate::Theme) -> (Vec<Line<'a>>, u16) {
    let dead = node.later || node.refused.is_some();
    let mut lines = Vec::new();

    if let Some(left_off) = card.left_off {
        lines.push(Line::from(Span::styled(
            "WHERE YOU LEFT OFF",
            Style::default()
                .fg(theme.status_ok)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(Span::styled(
            left_off.to_string(),
            Style::default().fg(theme.value),
        )));
        lines.push(Line::raw(""));
    }

    // The name, and how to open it, on one row.
    let title = Style::default()
        .fg(if dead {
            theme.stale
        } else {
            ink::of(node.tone, theme)
        })
        .add_modifier(Modifier::BOLD);
    let title = if matches!(node.kind, Kind::View(_)) && !dead {
        title.fg(theme.value_hi)
    } else {
        title
    };
    let how = match &node.kind {
        _ if dead => String::new(),
        Kind::View(e) => match e.slot {
            Some(s) => format!("Enter or {s} opens it"),
            None => "Enter opens it".to_string(),
        },
        Kind::Branch { .. } => "→ goes in".to_string(),
        Kind::Heading => String::new(),
    };
    let pad = w.saturating_sub(node.title.chars().count() + how.chars().count());
    lines.push(Line::from(vec![
        Span::styled(node.title.clone(), title),
        Span::raw(" ".repeat(pad)),
        Span::styled(how, Style::default().fg(theme.border_accent)),
    ]));

    let view = node.view();
    if let Some(blurb) = view.and_then(|e| e.blurb.as_deref()) {
        lines.push(Line::from(Span::styled(
            blurb.to_string(),
            Style::default().fg(theme.value),
        )));
    }
    lines.push(Line::raw(""));

    if let Some(why) = &node.refused {
        lines.push(Line::from(Span::styled(
            "✗ Not on this radio",
            Style::default()
                .fg(theme.status_crit)
                .add_modifier(Modifier::BOLD),
        )));
        let mut chars = why.chars();
        let why = match chars.next() {
            Some(c) => format!("{}{}.", c.to_uppercase(), chars.as_str()),
            None => String::new(),
        };
        for row in crate::ui::chrome::wrap(&why, w, 4) {
            lines.push(Line::from(Span::styled(
                row,
                Style::default().fg(theme.label),
            )));
        }
        lines.push(Line::raw(""));
    }

    let about = view.and_then(|e| e.about.as_deref()).or(node.about);
    if let Some(about) = about {
        let ink = if dead { theme.label } else { theme.value };
        for row in crate::ui::chrome::wrap(about, w, 5) {
            lines.push(Line::from(Span::styled(row, Style::default().fg(ink))));
        }
    }
    if let Some(live) = &card.live {
        let (dot, ink) = if live.running {
            ("●", theme.status_ok)
        } else {
            ("○", theme.stale)
        };
        lines.push(Line::raw(""));
        lines.push(Line::from(vec![
            Span::styled(format!("{dot} "), Style::default().fg(ink)),
            Span::styled(live.text.clone(), Style::default().fg(theme.label)),
        ]));
    }
    if !dead {
        if let Kind::Branch { .. } = node.kind {
            let n = node.view_count();
            if n > 0 {
                lines.push(Line::raw(""));
                lines.push(Line::from(Span::styled(
                    format!("{n} view{} inside", if n == 1 { "" } else { "s" }),
                    Style::default().fg(theme.label),
                )));
            }
        }
    }
    let used = lines.len() as u16;
    (lines, used)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LayoutConfig;
    use crate::ui::menu::tree::Tile;
    use crate::ui::menu::{model, tree};
    use ratatui::{backend::TestBackend, Terminal};

    const HACKRF: ruler::Reach<'static> = ("HackRF One", 1_000_000, 6_000_000_000, &[]);
    const AIRSPY: ruler::Reach<'static> = ("Airspy R2", 24_000_000, 1_800_000_000, &[]);

    fn draw(
        t: &tree::Tree,
        tile: Tile,
        want: &[&str],
        radio: ruler::Reach<'_>,
        placed: &[(String, Rect)],
    ) -> Vec<String> {
        let mut rows = t.root(tile);
        let mut path = tree::Path::default();
        for name in want {
            let i = rows
                .iter()
                .position(|n| n.is_stop() && n.title == *name)
                .unwrap();
            path.rows.push(i);
            rows = rows[i].children();
        }
        let trail = t.trail(tile, &path);
        let card = Card {
            trail: &trail,
            radio,
            live: None,
            placed,
            screen: Rect::new(0, 0, 191, 41),
            left_off: None,
        };
        let theme = crate::Theme::sdr();
        let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
        terminal
            .draw(|f| render(f, f.size(), &card, &theme))
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..28)
            .map(|y| {
                (0..100)
                    .map(|x| buf.get(x, y).symbol().to_string())
                    .collect()
            })
            .collect()
    }

    fn builtins() -> tree::Tree {
        tree::build(&model::build(&LayoutConfig::default_config().presets), &[])
    }

    /// A view's card: its name, its key, its blurb and what it shows, and the
    /// band it listens in on the ruler.
    #[test]
    fn a_view_says_what_it_is_and_how_to_open_it() {
        let t = builtins();
        let rows = draw(
            &t,
            Tile::Lab,
            &["Wi-Fi", "2.4 GHz", "Channel", "Frames"],
            HACKRF,
            &[],
        );
        let all = rows.join("\n");
        assert!(
            rows[0].starts_with("Frames") && rows[0].contains("Enter or 2 opens it"),
            "{}",
            rows[0]
        );
        assert!(all.contains("frame by frame"), "{all}");
        assert!(all.contains("coloured by Wi-Fi"), "{all}");
        assert!(
            all.contains("WHERE IT LISTENS") && all.contains("▼ 2.4 GHz"),
            "{all}"
        );
        assert!(all.contains("HackRF One reaches 1 MHz to 6 GHz"), "{all}");
    }

    /// A refused view says so and why, opens nothing, and the band is drawn
    /// out of the radio's reach.
    #[test]
    fn a_refused_view_says_why() {
        let mut presets = LayoutConfig::default_config().presets;
        let net: std::collections::HashMap<_, _> = presets
            .extract_if(|_, p| p.section.as_deref().is_some_and(crate::signal::net::is_net))
            .collect();
        let refusals: Vec<tree::Refusal> = model::build(&net)
            .sections
            .into_iter()
            .map(|section| tree::Refusal {
                section,
                why: "this radio tunes 24.000 to 1800.000 MHz".into(),
            })
            .collect();
        let t = tree::build(&model::build(&presets), &refusals);
        let rows = draw(
            &t,
            Tile::Lab,
            &["Wi-Fi", "2.4 GHz", "Channel", "Frames"],
            AIRSPY,
            &[],
        );
        let all = rows.join("\n");
        assert!(!rows[0].contains("opens it"), "{}", rows[0]);
        assert!(all.contains("✗ Not on this radio"), "{all}");
        assert!(
            all.contains("This radio tunes 24.000 to 1800.000 MHz."),
            "{all}"
        );
        assert!(all.contains("Airspy R2 reaches 24 MHz to 1.8 GHz"), "{all}");
    }

    /// A Lab view with no band of its own works wherever the radio is tuned.
    #[test]
    fn a_view_without_a_band_listens_anywhere() {
        let t = builtins();
        let all = draw(&t, Tile::Lab, &["Receiver", "IQ balance"], HACKRF, &[]).join("\n");
        assert!(all.contains("wherever you tune"), "{all}");
    }

    /// Wi-Fi's 5 GHz row marks 5 GHz, and Wi-Fi itself, over two bands,
    /// draws no ruler at all rather than one of them.
    #[test]
    fn five_gigahertz_is_drawn_at_five() {
        let t = builtins();
        let all = draw(&t, Tile::Lab, &["Wi-Fi", "5 GHz"], HACKRF, &[]).join("\n");
        assert!(all.contains("5 GHz ▼") || all.contains("▼ 5 GHz"), "{all}");
        assert!(
            !all.contains("2.4 GHz ▼") && !all.contains("▼ 2.4 GHz"),
            "{all}"
        );
        let all = draw(&t, Tile::Lab, &["Wi-Fi"], HACKRF, &[]).join("\n");
        assert!(!all.contains("WHERE IT LISTENS"), "{all}");
    }

    /// A later Radio row has no band and no ruler; it says it is not built.
    #[test]
    fn a_later_row_has_no_ruler() {
        let t = builtins();
        let all = draw(&t, Tile::Radio, &["Ham bands"], HACKRF, &[]).join("\n");
        assert!(all.contains("built yet"), "{all}");
        assert!(!all.contains("WHERE IT LISTENS"), "{all}");
        assert!(!all.contains("goes in"), "{all}");
    }

    /// The map is drawn beside the words on a wide card.
    #[test]
    fn the_map_sits_beside_the_words() {
        let t = builtins();
        let placed = vec![
            ("header".to_string(), Rect::new(0, 0, 191, 5)),
            ("census".to_string(), Rect::new(0, 5, 191, 31)),
            ("log".to_string(), Rect::new(0, 36, 191, 5)),
        ];
        let rows = draw(
            &t,
            Tile::Lab,
            &["Bluetooth", "2.4 GHz", "Low Energy", "LE devices"],
            HACKRF,
            &placed,
        );
        let map_row = rows
            .iter()
            .position(|r| r.contains("WHAT OPENS"))
            .expect("map drawn");
        assert!(map_row < 6, "beside the words, near the top: row {map_row}");
        assert!(rows.join("\n").contains("census"));
    }

    /// A group counts what it holds.
    #[test]
    fn a_group_counts_its_views() {
        let t = builtins();
        let all = draw(
            &t,
            Tile::Lab,
            &["Bluetooth", "2.4 GHz", "Low Energy"],
            HACKRF,
            &[],
        )
        .join("\n");
        assert!(
            all.contains("3 views inside") && all.contains("→ goes in"),
            "{all}"
        );
    }
}
