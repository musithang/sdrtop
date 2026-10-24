// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The pager's face: a pager, drawn as one, showing the page the POCSAG
//! receiver read.
//!
//! **An object, so it is drawn as one.** A plastic case with a moulded edge,
//! the dot-matrix display in its bezel (`widgets::dotmatrix`), a NEW lamp,
//! four buttons with the keys that press them, a speaker grille, and along
//! the bottom an engraved strip with the link's state. Every colour is the
//! theme's (`pager_*`, `lcd_*`), the display's in the backlight the user
//! chose.
//!
//! **What it says is what the receiver read.** The page's text with every
//! lost character drawn as a lost one, its address and function, the bits
//! it took repairing, whether it was cut; the signal bars are its own
//! measured level. Nothing heard yet says so, and a stopped radio dims the
//! display rather than leaving the last page lit as if it were live (rule 4).
//! The page's full physics is the card's; the strip carries the three a pager
//! would: sync, level, and what the work is costing.
//!
//! **It sizes to the space**: three lines of sixteen characters at the
//! reference size, fewer lines on a shorter screen, fewer characters on a
//! narrower one.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    Frame,
};

use super::{fill, kind, mhz, put};
use crate::state::SdrMetrics;
use crate::ui::panel::{Panel, PanelChrome, Staleness, Tag};
use crate::ui::widgets::charts::lerp_color;
use crate::ui::widgets::dotmatrix::{self, Look};

pub struct PagerFacePanel;

/// The widest the case grows: room for sixteen characters and its margins.
const CASE_MAX_W: u16 = 112;
/// Characters across the display, at most and at least.
const CHARS_MAX: usize = 16;
const CHARS_MIN: usize = 8;

/// The case's rows besides the display's text lines: edges, brand, bezel,
/// the display's own status and info rows, buttons, keys and the strip's
/// two lines.
const CASE_FIXED_H: u16 = 18;

/// A level, as bars on the display: four from -70 to -30 dBFS, 10 dB a bar.
fn bars(level_dbfs: f64) -> usize {
    (((level_dbfs + 70.0) / 10.0).floor().clamp(0.0, 4.0)) as usize
}

impl Panel for PagerFacePanel {
    fn name(&self) -> &'static str {
        "pager_face"
    }

    fn reads_spectrum(&self) -> bool {
        false
    }

    fn min_size(&self) -> (u16, u16) {
        (60, 21)
    }

    fn chrome(&self, state: &SdrMetrics) -> PanelChrome {
        let v = &state.pager.view;
        PanelChrome::new("Pager")
            .stale_when(Staleness::NotStreaming)
            .tag_if(v.masked, Tag::Masked)
            .tag_if(v.mine.is_some(), Tag::Filtered)
    }

    fn render(
        &self,
        f: &mut Frame,
        inner: Rect,
        state: &SdrMetrics,
        theme: &crate::Theme,
        _focused: bool,
    ) {
        face(f.buffer_mut(), inner, state, theme);
    }
}

/// What the display says when there is no page to show, or the page as its
/// lines, its screen count and which screen.
struct Display {
    lines: Vec<String>,
    screens: usize,
    screen: usize,
}

fn face(buf: &mut Buffer, area: Rect, m: &SdrMetrics, theme: &crate::Theme) {
    let p = &m.pager;
    let live = m.radio.hw_streaming;

    // How big: as many lines as the height allows, as many characters as
    // the width does, within the display's limits.
    let rows: u16 = match area.height {
        h if h >= CASE_FIXED_H + 12 => 3,
        h if h >= CASE_FIXED_H + 8 => 2,
        _ => 1,
    };
    let case_w = area.width.min(CASE_MAX_W);
    let chars = dotmatrix::chars_across(case_w.saturating_sub(16)).clamp(CHARS_MIN, CHARS_MAX);
    let case_h = (CASE_FIXED_H + 4 * rows).min(area.height);
    let case = Rect::new(
        area.x + (area.width - case_w) / 2,
        area.y + (area.height - case_h) / 2,
        case_w,
        case_h,
    );

    // The case: a filled body between two edge rows of half blocks, which
    // leave the screen's own background round them, so the corners read as
    // moulded rather than drawn.
    let shell = Style::default().bg(theme.pager_case);
    fill(
        buf,
        area,
        Rect::new(
            case.x,
            case.y + 1,
            case.width,
            case.height.saturating_sub(2),
        ),
        theme.pager_case,
    );
    let rim = Style::default().fg(theme.pager_case);
    for x in case.left() + 1..case.right().saturating_sub(1) {
        put(buf, area, x, case.top(), "\u{2584}", rim);
        put(buf, area, x, case.bottom() - 1, "\u{2580}", rim);
    }

    // The brand row, whose pager it is, and the NEW lamp.
    let unread = p.unread();
    let y = case.top() + 1;
    put(
        buf,
        area,
        case.x + 3,
        y,
        "sdrtop",
        shell.fg(theme.label).add_modifier(Modifier::BOLD),
    );
    put(
        buf,
        area,
        case.x + 10,
        y,
        "POCSAG",
        shell.fg(theme.border_dim),
    );
    if let Some(ric) = p.view.mine {
        put(
            buf,
            area,
            case.x + 18,
            y,
            &format!("\u{b7} my pager {ric:07}"),
            shell.fg(theme.value),
        );
    }
    let lamp = if unread > 0 && live {
        theme.status_crit
    } else {
        theme.stale
    };
    put(
        buf,
        area,
        case.right() - 9,
        y,
        "NEW",
        shell.fg(theme.border_dim),
    );
    put(
        buf,
        area,
        case.right() - 5,
        y,
        "\u{25cf}",
        shell.fg(lamp).add_modifier(Modifier::BOLD),
    );

    // The display in its bezel.
    let lw = dotmatrix::width_of(chars) + 2;
    let lh = 4 + 4 * rows;
    let lcd = Rect::new(
        case.x + (case.width.saturating_sub(lw)) / 2,
        case.top() + 3,
        lw,
        lh,
    );
    fill(
        buf,
        area,
        Rect::new(lcd.x - 1, lcd.y - 1, lw + 2, lh + 2),
        theme.pager_bezel,
    );
    let look = display_look(
        theme.display(p.view.display.as_deref(), p.view.night),
        theme.pager_bezel,
        live,
    );
    fill(buf, area, lcd, look.glass);
    let glass = Style::default().bg(look.glass).fg(look.ink);

    let current = p.current();
    let shown = p.shown();
    // The status row: the page's level as bars, what is unread, and the
    // page's time, function and kind.
    if let Some(e) = current {
        let n = bars(e.link.level_dbfs);
        for i in 0..4 {
            let ink = if i < n { look.ink } else { look.ghost };
            put(
                buf,
                area,
                lcd.x + 1 + i as u16,
                lcd.y,
                ["\u{2582}", "\u{2584}", "\u{2586}", "\u{2588}"][i],
                glass.fg(ink),
            );
        }
        if unread > 0 {
            put(
                buf,
                area,
                lcd.x + 7,
                lcd.y,
                &format!("\u{2709} {unread} NEW"),
                glass.add_modifier(Modifier::BOLD),
            );
        }
        let clock = crate::ui::panels::core::log::fmt_clock(e.unix_s as u64);
        let right = format!(
            "{}  {}  {}",
            &clock[..5],
            e.page.function.letter(),
            kind(&e.page.content)
        );
        let x = lcd.right().saturating_sub(right.chars().count() as u16 + 1);
        put(
            buf,
            area,
            x,
            lcd.y,
            &right,
            glass.add_modifier(Modifier::BOLD),
        );
    } else {
        let what = if !live {
            "RX STOPPED"
        } else if p.locked.is_some() {
            "RECEIVING"
        } else {
            "SEARCHING"
        };
        put(
            buf,
            area,
            lcd.x + 1,
            lcd.y,
            what,
            glass.add_modifier(Modifier::BOLD),
        );
    }

    let d = display(m, chars, rows as usize);
    let screen: Vec<&str> = d
        .lines
        .iter()
        .skip(d.screen * rows as usize)
        .take(rows as usize)
        .map(String::as_str)
        .collect();
    let text_area = Rect::new(lcd.x + 1, lcd.y + 2, dotmatrix::width_of(chars), 4 * rows);
    dotmatrix::render(buf, text_area.intersection(area), &screen, look);

    // The info row: whose page, which of how many, what it took.
    let info = match current {
        Some(e) => {
            let at = shown.iter().position(|s| s.n == e.n).map_or(0, |i| i + 1);
            let mut s = format!("RIC {:07}  #{at} OF {}", e.page.ric, shown.len());
            if e.page.repairs() > 0 {
                s.push_str(&format!("  FIX {}", e.page.repairs()));
            }
            if e.page.refused() > 0 {
                s.push_str(&format!("  LOST {}", e.page.refused()));
            }
            if e.page.end == crate::signal::pocsag::page::End::Cut {
                s.push_str("  CUT");
            }
            if d.screens > 1 {
                s.push_str(&format!("  {}/{}", d.screen + 1, d.screens));
            }
            s
        }
        None => format!("LISTENING ON {} MHz", mhz(m.radio.frequency)),
    };
    put(buf, area, lcd.x + 1, lcd.bottom() - 1, &info, glass);

    // The buttons, each with the key that presses it, and the grille.
    let by = lcd.bottom() + 2;
    let buttons = [
        ("\u{25c0}", "\u{2190}"),
        ("READ", "Enter"),
        ("\u{25b6}", "\u{2192}"),
        ("MASK", "m"),
    ];
    let (bw, gap) = (12u16, 3u16);
    let total = 4 * bw + 3 * gap;
    let mut bx = case.x + case.width.saturating_sub(total) / 2;
    let edge = Style::default().fg(theme.pager_edge).bg(theme.pager_case);
    for (label, key) in buttons {
        put(
            buf,
            area,
            bx,
            by,
            &format!("\u{256d}{}\u{256e}", "\u{2500}".repeat(usize::from(bw - 2))),
            edge,
        );
        put(buf, area, bx, by + 1, "\u{2502}", edge);
        put(buf, area, bx + bw - 1, by + 1, "\u{2502}", edge);
        put(
            buf,
            area,
            bx,
            by + 2,
            &format!("\u{2570}{}\u{256f}", "\u{2500}".repeat(usize::from(bw - 2))),
            edge,
        );
        let lx = bx + (bw - label.chars().count() as u16) / 2;
        put(
            buf,
            area,
            lx,
            by + 1,
            label,
            shell.fg(theme.value).add_modifier(Modifier::BOLD),
        );
        let kx = bx + (bw - key.chars().count() as u16) / 2;
        put(buf, area, kx, by + 3, key, shell.fg(theme.border_dim));
        bx += bw + gap;
    }
    // The rest of the keys, engraved under the buttons as a pager's side
    // carries its functions.
    let more = "U my pager   B day/night   L colour   C clear";
    let mx = case.x + case.width.saturating_sub(more.chars().count() as u16) / 2;
    put(buf, area, mx, by + 5, more, shell.fg(theme.border_dim));
    if bx + 9 < case.right() {
        for i in 0..3 {
            put(
                buf,
                area,
                bx + 1,
                by + i,
                "\u{25ac} \u{25ac} \u{25ac} \u{25ac}",
                edge,
            );
        }
    }

    // The strip: the link, the level, the cost; under it the page's link in
    // short, the physics a pager never shows, so it is on screen however
    // narrow the screen and whether the card is beside it or not.
    if let Some(e) = current {
        let mut line = super::link_line(&e.link);
        if e.page.repairs() > 0 {
            line.push_str(&format!("  {} repaired", e.page.repairs()));
        }
        put(
            buf,
            area,
            case.x + 3,
            case.bottom() - 2,
            "LINK",
            shell.fg(theme.border_dim),
        );
        put(
            buf,
            area,
            case.x + 9,
            case.bottom() - 2,
            &line,
            shell.fg(theme.value),
        );
    }
    let sy = case.bottom() - 3;
    let mut x = case.x + 3;
    let mut part = |buf: &mut Buffer, text: &str, ink: Color| {
        put(buf, area, x, sy, text, shell.fg(ink));
        x += text.chars().count() as u16 + 2;
    };
    part(buf, "SYNC", theme.border_dim);
    match (live, p.locked) {
        (true, Some(baud)) => part(buf, &format!("locked, {baud} baud"), theme.status_ok),
        (true, None) => part(buf, "searching", theme.label),
        (false, _) => part(buf, "RX stopped", theme.stale),
    }
    if let Some(e) = current {
        part(buf, "LEVEL", theme.border_dim);
        let n = ((e.link.level_dbfs + 70.0) / 40.0 * 14.0).clamp(0.0, 14.0) as usize;
        part(
            buf,
            &format!(
                "\u{2595}{}{}\u{258f} {:.0} dBFS",
                "\u{2588}".repeat(n),
                "\u{2581}".repeat(14 - n),
                e.link.level_dbfs
            ),
            theme.value,
        );
    }
    if let Some(load) = p.feed.load {
        part(buf, "LOAD", theme.border_dim);
        let ink = if load > 0.8 {
            theme.status_warn
        } else {
            theme.value
        };
        part(buf, &format!("{:.0} %", load * 100.0), ink);
    }
    if p.feed.last_loss.is_some_and(|t| t.elapsed().as_secs() < 10) {
        part(buf, "FEED LOSS", theme.status_warn);
    }
}

/// The display's inks: as the theme gives them while the radio streams; with
/// the radio stopped, the glass dimmed toward the bezel, the ink faded toward
/// the glass, and no cells, so a stopped pager does not look like one
/// showing a live page.
fn display_look(look: Look, bezel: Color, live: bool) -> Look {
    if live {
        return look;
    }
    let glass = lerp_color(look.glass, bezel, 0.55);
    Look {
        glass,
        ink: lerp_color(look.ink, glass, 0.55),
        ghost: glass,
    }
}

fn display(m: &SdrMetrics, chars: usize, rows: usize) -> Display {
    let p = &m.pager;
    let one = |lines: Vec<String>| Display {
        lines,
        screens: 1,
        screen: 0,
    };
    let Some(e) = p.current() else {
        return one(vec![String::new(), "NO PAGES YET".to_string()]);
    };
    let text = e.page.text();
    if matches!(e.page.content, crate::signal::pocsag::page::Content::Tone) {
        return one(vec![String::new(), "TONE ONLY".to_string()]);
    }
    if p.view.masked {
        let unit = match e.page.content {
            crate::signal::pocsag::page::Content::Numeric(_) => "DIGITS",
            _ => "CHARACTERS",
        };
        return one(vec![
            String::new(),
            format!("{} \u{b7} {}", kind(&e.page.content), text.chars().count()),
            unit.to_string(),
        ]);
    }
    let lines = dotmatrix::wrap(&text, chars);
    let screens = lines.len().div_ceil(rows.max(1));
    Display {
        screen: p.screen_of(screens),
        lines,
        screens,
    }
}

#[cfg(test)]
mod tests {
    use super::super::sample::with_pages;
    use super::*;
    use crate::state::fixture::draw;

    /// The display's dots, read back as text is hard; what can be read is
    /// every line of plain text on the face.
    fn face_text(m: &SdrMetrics, w: u16, h: u16) -> String {
        draw(PagerFacePanel, w, h, m).join("\n")
    }

    #[test]
    fn the_face_carries_its_parts() {
        let m = with_pages(&[(200_001, "WX BUDAPEST 14C")]);
        let all = face_text(&m, 120, 34);
        for want in [
            "sdrtop",
            "POCSAG",
            "NEW",
            "READ",
            "MASK",
            "Enter",
            "RIC 0200001",
            "#1 OF 1",
            "SYNC",
            "LEVEL",
            "-38 dBFS",
            "ALPHA",
            "LINK",
            "1200 baud +0 ppm",
            "\u{b1}4.50 kHz",
            "SNR 30 dB",
            "U my pager   B day/night   L colour   C clear",
        ] {
            assert!(all.contains(want), "'{want}' missing:\n{all}");
        }
    }

    /// Nothing heard: the display says so, and the frequency it listens on.
    #[test]
    fn nothing_heard_says_so() {
        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = crate::signal::pocsag::SECTION.to_string();
        m.radio.frequency = 439_987_500;
        let all = face_text(&m, 120, 34);
        assert!(all.contains("SEARCHING"), "{all}");
        assert!(all.contains("LISTENING ON 439.9875 MHz"), "{all}");
    }

    /// A stopped radio: the frame says stale and the display says why.
    #[test]
    fn a_stopped_radio_is_said() {
        let mut m = with_pages(&[(8, "OLD PAGE")]);
        m.radio.hw_streaming = false;
        let all = face_text(&m, 120, 34);
        assert!(all.contains("[STALE]"), "{all}");
        assert!(all.contains("RX stopped"), "{all}");
    }

    /// Masked, the chrome says so; my pager, the chrome says the list is
    /// narrowed and the brand row whose it is.
    #[test]
    fn masked_and_mine_are_said() {
        let mut m = with_pages(&[(8, "SECRET"), (16, "OTHER")]);
        m.pager.view.masked = true;
        m.pager.view.mine = Some(8);
        let all = face_text(&m, 120, 34);
        assert!(
            all.contains("[MASKED]") && all.contains("[FILTERED]"),
            "{all}"
        );
        assert!(all.contains("my pager 0000008"), "{all}");
        assert!(all.contains("#1 OF 1"), "{all}");
    }

    /// A long page has screens, and READ is on the one the info row says.
    #[test]
    fn a_long_page_has_screens() {
        let mut m = with_pages(&[(
            8,
            "ONE TWO THREE FOUR FIVE SIX SEVEN EIGHT NINE TEN ELEVEN TWELVE THIRTEEN",
        )]);
        assert!(face_text(&m, 120, 34).contains("1/2"));
        m.pager.read_on();
        assert!(face_text(&m, 120, 34).contains("2/2"));
    }

    /// It fits every size it is given, and draws nothing outside it.
    #[test]
    fn it_fits() {
        let m = with_pages(&[(8, "A PAGE")]);
        for (w, h) in [(120, 34), (100, 30), (96, 26), (70, 22), (60, 21), (40, 10)] {
            let lines = draw(PagerFacePanel, w, h, &m);
            assert!(
                lines.iter().all(|l| l.chars().count() <= w as usize),
                "{w}x{h}"
            );
        }
    }

    /// The display is drawn in the colour chosen, by day and by night.
    #[test]
    fn the_display_takes_the_colour_chosen() {
        let theme = crate::Theme::sdr();
        let mut m = with_pages(&[(8, "COLOUR")]);
        m.pager.view.display = Some("blue".to_string());
        m.pager.view.night = true;
        let glass = theme.display(Some("blue"), true).glass;
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 34)).unwrap();
        terminal
            .draw(|f| PagerFacePanel.render(f, f.size(), &m, &theme, false))
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        let found = (0..34u16)
            .flat_map(|y| (0..120u16).map(move |x| (x, y)))
            .any(|(x, y)| buf.get(x, y).bg == glass);
        assert!(found, "no cell in the blue display's night glass");
    }

    #[test]
    fn the_bars_are_ten_db_each() {
        assert_eq!(bars(-75.0), 0);
        assert_eq!(bars(-60.0), 1);
        assert_eq!(bars(-38.0), 3);
        assert_eq!(bars(-20.0), 4);
    }
}
