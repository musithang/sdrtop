// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `NetCoexistPanel` - what is stepping on your link, and when.
//!
//! The band across, time running down from now at the top, and each cell
//! coloured by how busy that megahertz was at that moment: the waterfall's
//! orientation, so the occupancy profile above it and this history below it
//! read as one instrument with one frequency ruler (net-ux-polish-plan Stop 3).
//! Design section 9.1 calls it the single most immediately legible thing either
//! arc produces, and section 8's acceptance criterion for it is not a test:
//! **readable at two metres.**
//!
//! **Frequency through the band axis, never its own.** A column covers the
//! cells `band_axis::cells_of` says, the same cells the occupancy profile's
//! column above it covers, because a shared ruler is only honest if a column
//! means the same megahertz in both. It was frequency down the side until
//! 2026-09-19, which gave the two panels no axis in common.
//!
//! **What it is fed on, and what the plan said it would be fed on.** The plan
//! has this panel drawing bursts from N14, colour-coded by protocol. N14 landed
//! per-cell duty cycle and no burst detector - "no demodulation anywhere" was
//! its own instruction - so there are no bursts to draw and there will be none
//! until an arc lands one. What there is instead is real and is the same
//! picture at a coarser grain: the occupancy history, half a second to a row
//! half. Colour carries the duty cycle.
//!
//! **What is identified, and what is only energy.** Over the ramp, every BLE
//! packet that passed its CRC and every classic-BT access-code hit is marked at
//! its moment and its channel, in the theme's per-protocol colour (`●` BLE,
//! `■` BT). Everything unmarked is energy nobody decoded: the ramp does not
//! imply Wi-Fi, or anything else. A failed-CRC packet is not marked, because it
//! is not identified. The marks go back as far as the packet and hit lists
//! hold (`BLE_PACKET_LIMIT`, `BT_HOP_LIMIT`), and the legend counts what is
//! drawn, so an old stretch without marks reads as "not kept", not "none".

use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use super::band_axis;
use super::band_axis::bonded_frame;
use crate::state::{SdrMetrics, COLUMN_INTERVAL};
use crate::ui::panel::{Bond, Bonding, FeedSpan, Panel, PanelChrome, Staleness};
use crate::ui::widgets::canvas::{fold, row, Duty};
use ratatui::widgets::Borders;

pub struct NetCoexistPanel;

/// Rows reserved under the canvas: the channel ruler and the band's edges with
/// the time the canvas spans.
const AXIS_ROWS: u16 = 2;

/// The canvas: `rows` character rows, two moments each (a half block's upper
/// and lower halves), `offset` moments back at the top (0: now), each moment
/// folded into `width` columns by the band axis. A moment older than the
/// history holds is `None` throughout, drawn as unlooked-at rather than as a
/// quiet band.
fn canvas(history: &[Vec<f32>], rows: usize, width: usize, offset: usize) -> Vec<Vec<Duty>> {
    (offset..offset + rows * 2)
        .map(|step| {
            history
                .len()
                .checked_sub(1 + step)
                .map(|i| fold(&history[i], width))
                .unwrap_or_else(|| vec![None; width])
        })
        .collect()
}

/// A decoded protocol, marked over the ramp.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Proto {
    Ble,
    Bt,
}

impl Proto {
    fn glyph(self) -> &'static str {
        match self {
            Proto::Ble => "\u{25cf}",
            Proto::Bt => "\u{25a0}",
        }
    }

    fn colour(self, theme: &crate::Theme) -> ratatui::style::Color {
        match self {
            Proto::Ble => theme.net_ble,
            Proto::Bt => theme.net_bt,
        }
    }
}

/// Every decoded packet the canvas has room for, as `(moment, column, proto)`:
/// the moment counted back from the newest column (`BandOccupancy::last_column`,
/// one every `COLUMN_INTERVAL`), the column the channel's centre falls in on the
/// band axis. Nothing is placed before the first column exists: without it
/// there is no time to place a mark at.
fn marks(state: &SdrMetrics, moments: usize, width: usize) -> Vec<(usize, usize, Proto)> {
    let Some(last) = state.net.band.last_column else {
        return Vec::new();
    };
    let step = |seen: std::time::Instant| {
        let back = last.saturating_duration_since(seen).as_secs_f64();
        (back / COLUMN_INTERVAL.as_secs_f64()) as usize
    };
    let at = |hz: Option<u64>| {
        hz.and_then(|hz| crate::signal::net::occupancy::cell_of(hz as f64))
            .map(|cell| band_axis::column_of(cell, width))
    };
    let ble = state
        .net
        .ble_packets
        .iter()
        .filter(|p| p.crc_ok)
        .filter_map(|p| {
            Some((
                step(p.seen),
                at(crate::signal::ble::channel::centre_hz(p.channel))?,
                Proto::Ble,
            ))
        });
    let bt = state.net.bt_hops.iter().filter_map(|h| {
        Some((
            step(h.seen),
            at(crate::signal::bt::channel::centre_hz(h.channel))?,
            Proto::Bt,
        ))
    });
    // Classic first, so a BLE mark in the same cell is the one drawn.
    bt.chain(ble).filter(|(s, _, _)| *s < moments).collect()
}

/// `2400 MHz   now at the top, 12 s down   ● BLE 14  ■ BT 3   2483 MHz`: the
/// band's edges, how far back the bottom reaches, and what the marks are, each
/// part only when it fits whole between the edges.
fn footer(
    width: usize,
    (top_s, bottom_s): (f64, f64),
    counts: (usize, usize),
    theme: &crate::Theme,
) -> Line<'static> {
    let dim = Style::default().fg(theme.label);
    let (left, right) = ("2400 MHz", "2483 MHz");
    let mut middle: Vec<Span<'static>> = Vec::new();
    let time = if top_s == 0.0 {
        format!("now at the top, {bottom_s:.0} s down")
    } else {
        format!("{top_s:.1} s ago at the top, {bottom_s:.0} s down")
    };
    let room = width.saturating_sub(left.len() + right.len() + 4);
    let mut used = 0;
    if time.len() <= room {
        used += time.len();
        middle.push(Span::styled(time, dim));
    }
    for (proto, name, n) in [(Proto::Ble, "BLE", counts.0), (Proto::Bt, "BT", counts.1)] {
        let text = format!("{name} {n}");
        let need = 3 + proto.glyph().chars().count() + 1 + text.len();
        if used + need > room {
            break;
        }
        used += need;
        middle.push(Span::raw("   "));
        middle.push(Span::styled(
            proto.glyph(),
            Style::default().fg(proto.colour(theme)),
        ));
        middle.push(Span::styled(format!(" {text}"), dim));
    }
    let gap = width.saturating_sub(left.len() + right.len() + used);
    let mut spans = vec![Span::styled(left, dim), Span::raw(" ".repeat(gap / 2))];
    spans.extend(middle);
    spans.push(Span::raw(" ".repeat(gap - gap / 2)));
    spans.push(Span::styled(right, dim));
    Line::from(spans)
}

impl Panel for NetCoexistPanel {
    fn name(&self) -> &'static str {
        "net_coexist"
    }

    fn min_size(&self) -> (u16, u16) {
        (30, 8)
    }

    fn chrome(&self, state: &SdrMetrics) -> PanelChrome {
        PanelChrome::new("Coexistence")
            .stale_when(Staleness::NotStreaming)
            .tag_if(true, state.net.mode.tag())
            // The canvas holds the band's last HISTORY_COLUMNS moments, one
            // every COLUMN_INTERVAL: a drop inside that stretch thins a moment.
            .counts_from_feed(FeedSpan::Window(
                crate::state::COLUMN_INTERVAL * crate::state::HISTORY_COLUMNS as u32,
            ))
    }

    /// `z`: every letter of the panel's name is another panel's focus key or a
    /// global one, so the engine draws `[Z]`.
    fn focus_key(&self) -> Option<char> {
        Some('z')
    }

    fn focus_bindings(&self) -> &'static [(&'static str, &'static str)] {
        &[
            ("\u{2193}", "back in time: the profile shows that moment"),
            ("\u{2191}", "forward in time"),
            ("N", "back to now"),
        ]
    }

    /// The lower half of the survey instrument, under the occupancy profile.
    fn bonding(&self) -> Option<Bonding> {
        Some(Bonding {
            role: Bond::Above,
            partner: "net_occupancy",
        })
    }

    /// Bonded: the top edge is the shared ruler, the channel numbers let into
    /// the rule the way the waterfall's frequency axis is, and the nameplate
    /// moves to the bottom edge.
    fn render_bonded(
        &self,
        f: &mut Frame,
        area: Rect,
        state: &SdrMetrics,
        theme: &crate::Theme,
        focused: bool,
        _bond: Bond,
    ) {
        let sides = Borders::LEFT | Borders::RIGHT | Borders::BOTTOM;
        let Some(inner) = bonded_frame(self, f, area, state, theme, focused, sides, true) else {
            return;
        };
        let rule = crate::ui::chrome::frame::frame_color(
            &self.chrome(state).with_engine_tags(state),
            state,
            focused,
            theme,
        );
        let seam: Vec<Span<'static>> = band_axis::ruler(inner.width as usize)
            .chars()
            .map(|c| match c {
                ' ' => Span::styled("\u{2500}", Style::default().fg(rule)),
                d => Span::styled(d.to_string(), Style::default().fg(theme.label)),
            })
            .collect();
        f.render_widget(
            Paragraph::new(Line::from(seam)),
            Rect { height: 1, ..inner },
        );
        if inner.height > 1 {
            let body = Rect {
                y: inner.y + 1,
                height: inner.height - 1,
                ..inner
            };
            draw(f, body, state, theme, false);
        }
    }

    fn render(
        &self,
        f: &mut Frame,
        inner: Rect,
        state: &SdrMetrics,
        theme: &crate::Theme,
        _focused: bool,
    ) {
        draw(f, inner, state, theme, true);
    }
}

/// The canvas and what is under it, into `inner`. `ruler` is false when the
/// panel is bonded and the seam above already carries it.
fn draw(f: &mut Frame, inner: Rect, state: &SdrMetrics, theme: &crate::Theme, ruler: bool) {
    let axis_rows = if ruler { AXIS_ROWS } else { AXIS_ROWS - 1 };
    if inner.width == 0 || inner.height <= axis_rows {
        return;
    }
    let width = inner.width as usize;
    let rows = (inner.height - axis_rows) as usize;
    let history: Vec<Vec<f32>> = state.net.band.history.iter().cloned().collect();

    if history.is_empty() {
        // A pass that will never come is not a pass to wait for. The same
        // distinction the occupancy profile makes, for the same reason.
        let said = match &state.net.survey_refused {
            Some(why) => format!("no pass is possible: {why}"),
            None => "waiting for the first pass".to_string(),
        };
        f.render_widget(
            Paragraph::new(vec![Line::from(Span::styled(
                said,
                Style::default().fg(theme.stale),
            ))]),
            inner,
        );
        return;
    }

    // The time cursor, and how far the canvas has to scroll to keep it in
    // view: while it is on a moment the canvas already shows, nothing moves;
    // past the bottom, the canvas follows it back, so the profile above and
    // the history below always show the same moment.
    let cursor = state
        .net
        .band_scrub
        .and_then(|id| state.net.band.back_of(id));
    let visible = rows * 2;
    let offset = cursor.map_or(0, |b| (b + 1).saturating_sub(visible));
    let moments = canvas(&history, rows, width, offset);
    let mut lines: Vec<Line<'static>> = moments
        .chunks(2)
        .map(|pair| row(&pair[0], &pair[1], theme))
        .collect();

    // The marks, each on the half of its row that is its moment, keeping that
    // half's duty colour behind it so the energy still shows.
    let placed: Vec<(usize, usize, Proto)> = marks(state, offset + visible, width)
        .into_iter()
        .filter_map(|(s, x, p)| Some((s.checked_sub(offset)?, x, p)))
        .collect();
    for &(step, x, proto) in &placed {
        let Some(span) = lines.get_mut(step / 2).and_then(|l| l.spans.get_mut(x)) else {
            continue;
        };
        let behind = if step % 2 == 0 {
            span.style.fg
        } else {
            span.style.bg
        };
        let mut style = Style::default().fg(proto.colour(theme));
        if let Some(c) = behind {
            style = style.bg(c);
        }
        *span = Span::styled(proto.glyph(), style);
    }
    // The time cursor's moment, marked on the left edge of its row: outside
    // the canvas, so it hides no cell. Drawn over the frame the engine or the
    // bond already drew, one column left of the canvas.
    if let Some(back) = cursor {
        let row = (back - offset) / 2;
        if row < rows && inner.x > 0 {
            f.render_widget(
                Paragraph::new(Span::styled(
                    "\u{25b6}",
                    Style::default().fg(theme.value_hi),
                )),
                Rect {
                    x: inner.x - 1,
                    y: inner.y + row as u16,
                    width: 1,
                    height: 1,
                },
            );
        }
    }
    let counts = (
        placed.iter().filter(|m| m.2 == Proto::Ble).count(),
        placed.iter().filter(|m| m.2 == Proto::Bt).count(),
    );

    // How far back the top and the bottom of the canvas reach: the moments it
    // holds, not the ones it has room for, so a short history says it is short.
    let interval = COLUMN_INTERVAL.as_secs_f64();
    let shown = history.len().saturating_sub(offset).min(visible);
    let span_s = (offset as f64 * interval, (offset + shown) as f64 * interval);
    let dim = Style::default().fg(theme.label);
    if ruler {
        lines.push(Line::from(Span::styled(band_axis::ruler(width), dim)));
    }
    lines.push(footer(width, span_s, counts, theme));
    f.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::net::occupancy;
    use crate::state::{fixture::draw, CellReading};
    use crate::ui::widgets::canvas::ink;
    use ratatui::{backend::TestBackend, Terminal};

    /// One moment of the band with `busy` set on exactly `cell`.
    fn column(cell: usize) -> Vec<f32> {
        let mut c = vec![0.0f32; occupancy::CELLS];
        c[cell] = 1.0;
        c
    }

    /// `columns` oldest first, the way the history holds them.
    fn with(columns: Vec<Vec<f32>>) -> SdrMetrics {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.band.cells = vec![CellReading::default(); occupancy::CELLS];
        m.net.band.history = columns.into();
        m
    }

    /// Render through a real buffer and return the styled cells, so a test can
    /// ask about **colour** - which is the whole reading on this panel and is
    /// invisible in the text `state::fixture::draw` returns.
    fn cells(m: &SdrMetrics, w: u16, h: u16) -> ratatui::buffer::Buffer {
        let theme = crate::Theme::sdr();
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal
            .draw(|f| NetCoexistPanel.render(f, f.size(), m, &theme, false))
            .unwrap();
        terminal.backend().buffer().clone()
    }

    /// **A burst at a known frequency and moment lands in the known cell**, at
    /// three sizes: the column the band axis gives its megahertz, the upper
    /// half of the top row for the newest moment and the lower half for the
    /// one before. Catches an axis drawn upside down, off by a row, or mapped
    /// differently from the occupancy profile above it.
    #[test]
    fn a_busy_cell_lands_where_the_band_axis_says_it_should() {
        let theme = crate::Theme::sdr();
        let busy = ink(Some(1.0), &theme);
        for (w, h) in [(40u16, 12u16), (90, 24), (120, 45)] {
            for cell in [0usize, occupancy::CELLS / 2, occupancy::CELLS - 1] {
                let other = (cell + 30) % occupancy::CELLS;
                // Oldest first: `other` a moment ago, `cell` now.
                let buf = cells(&with(vec![column(other), column(cell)]), w, h);
                let x = band_axis::column_of(cell, w as usize) as u16;
                assert_eq!(buf.get(x, 0).style().fg, Some(busy), "{w}x{h}: now, {cell}");
                let x = band_axis::column_of(other, w as usize) as u16;
                assert_eq!(
                    buf.get(x, 0).style().bg,
                    Some(busy),
                    "{w}x{h}: before, {other}"
                );
            }
        }
    }

    /// The ruler and the band's edges are the band axis's, and the time the
    /// canvas reaches back is said in seconds.
    #[test]
    fn the_axis_is_the_band_axis_and_the_time_is_said() {
        let m = with(vec![column(0); 20]);
        let lines = draw(NetCoexistPanel, 90, 24, &m);
        let text = lines.join("\n");
        assert!(text.contains(&band_axis::ruler(88)), "{text}");
        assert!(
            text.contains("2400 MHz") && text.contains("2483 MHz"),
            "{text}"
        );
        // Twenty moments at half a second each.
        assert!(text.contains("now at the top, 10 s down"), "{text}");
        assert!(text.contains("BLE 0") && text.contains("BT 0"), "{text}");
    }

    fn packet(channel: u8, crc_ok: bool, seen: std::time::Instant) -> crate::state::BlePacket {
        crate::state::BlePacket {
            channel,
            pdu_type: crate::signal::ble::pdu::PduType::AdvInd,
            tx_add_random: false,
            length: 20,
            adv_addr: Some([1, 2, 3, 4, 5, 6]),
            crc_ok,
            snr_db: None,
            freq_offset_hz: None,
            modulation: None,
            drift: None,
            seen,
        }
    }

    /// **Identified traffic is marked where and when it was heard.** A BLE
    /// packet that passed its CRC now, on channel 37, is a `●` in the BLE
    /// colour on the top row's upper half at 2402 MHz's column; a classic hit
    /// on channel 39 1.1 s ago is a `■` two moments down; a failed-CRC packet
    /// is not marked; one older than the canvas is not placed. The legend
    /// counts what was drawn.
    #[test]
    fn decoded_traffic_is_marked_at_its_channel_and_moment() {
        let theme = crate::Theme::sdr();
        let now = std::time::Instant::now();
        let mut m = with(vec![column(0); 10]);
        m.net.band.last_column = Some(now);
        m.net.ble_packets.push_front(packet(37, true, now));
        m.net.ble_packets.push_front(packet(38, false, now));
        m.net
            .ble_packets
            .push_front(packet(39, true, now - std::time::Duration::from_secs(600)));
        m.net.bt_hops.push_front(crate::state::BtHop {
            channel: 39,
            lap: 0x9e8b33,
            seen: now - std::time::Duration::from_millis(1100),
        });
        let (w, h) = (90u16, 20u16);
        let buf = cells(&m, w, h);
        let col = |hz: u64| {
            band_axis::column_of(occupancy::cell_of(hz as f64).unwrap(), w as usize) as u16
        };

        let ble = buf.get(col(2_402_000_000), 0);
        assert_eq!(ble.symbol(), "\u{25cf}");
        assert_eq!(ble.style().fg, Some(theme.net_ble));

        // 1.1 s back is the third moment: row 1, its upper half.
        let bt = buf.get(col(2_441_000_000), 1);
        assert_eq!(bt.symbol(), "\u{25a0}");
        assert_eq!(bt.style().fg, Some(theme.net_bt));

        assert_ne!(
            buf.get(col(2_426_000_000), 0).symbol(),
            "\u{25cf}",
            "a failed CRC is not identified"
        );
        for y in 0..h - 2 {
            assert_ne!(
                buf.get(col(2_480_000_000), y).symbol(),
                "\u{25cf}",
                "ten minutes back is off the canvas"
            );
        }
        let footer: String = (0..w).map(|x| buf.get(x, h - 1).symbol()).collect();
        assert!(
            footer.contains("BLE 1") && footer.contains("BT 1"),
            "{footer}"
        );
    }

    /// The time cursor's moment is marked on the left edge of its row, the
    /// fourth moment back on the second row.
    #[test]
    fn the_time_cursor_is_marked_beside_its_row() {
        let mut m = with(vec![column(0); 10]);
        m.net.band.columns_taken = 10;
        m.net.band_scrub = m.net.band.id_back(3);
        let theme = crate::Theme::sdr();
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        terminal
            .draw(|f| {
                let inner = Rect {
                    x: 1,
                    y: 0,
                    width: 59,
                    height: 20,
                };
                NetCoexistPanel.render(f, inner, &m, &theme, false)
            })
            .unwrap();
        let buf = terminal.backend().buffer();
        assert_eq!(buf.get(0, 1).symbol(), "\u{25b6}");
        assert_eq!(buf.get(0, 0).symbol(), " ");
    }

    /// **The history follows the cursor back.** Past the moments the canvas
    /// shows, it scrolls so the cursor's moment stays in view: the cursor 50
    /// moments back on an 18-row canvas (36 moments) puts the moment 15 back
    /// at the top, the marker on the last row, and the footer says how old
    /// the top is. Found by Viktor's two-metre check (Stop 3.3.d): the profile
    /// went back and the history under it did not.
    #[test]
    fn the_history_scrolls_to_keep_the_cursors_moment_in_view() {
        let theme = crate::Theme::sdr();
        let mut columns = vec![column(0); 60];
        // The moment 15 back: busy at cell 20 alone.
        columns[60 - 1 - 15] = column(20);
        let mut m = with(columns);
        m.net.band.columns_taken = 60;
        m.net.band_scrub = m.net.band.id_back(50);
        let mut terminal = Terminal::new(TestBackend::new(61, 20)).unwrap();
        terminal
            .draw(|f| {
                let inner = Rect {
                    x: 1,
                    y: 0,
                    width: 60,
                    height: 20,
                };
                NetCoexistPanel.render(f, inner, &m, &theme, false)
            })
            .unwrap();
        let buf = terminal.backend().buffer();
        let x = 1 + band_axis::column_of(20, 60) as u16;
        assert_eq!(
            buf.get(x, 0).style().fg,
            Some(ink(Some(1.0), &theme)),
            "the top is 15 back"
        );
        assert_eq!(
            buf.get(0, 17).symbol(),
            "\u{25b6}",
            "the cursor's row is in view"
        );
        let footer: String = (0..61).map(|x| buf.get(x, 19).symbol()).collect();
        assert!(footer.contains("7.5 s ago at the top"), "{footer}");
    }

    /// Every colour is the theme's.
    #[test]
    fn the_colours_come_from_the_theme() {
        let theme = crate::Theme::sdr();
        let allowed: Vec<_> = (0..=20)
            .map(|i| theme.palette_color(i as f32 / 20.0))
            .chain([
                theme.border_dim,
                theme.label,
                theme.stale,
                theme.net_ble,
                theme.net_bt,
            ])
            .collect();
        let buf = cells(&with(vec![column(10), column(40), column(70)]), 60, 20);
        for y in 0..20u16 {
            for x in 0..60 {
                let style = buf.get(x, y).style();
                for c in [style.fg, style.bg] {
                    if let Some(c) = c.filter(|c| *c != ratatui::style::Color::Reset) {
                        assert!(allowed.contains(&c), "{c:?} at {x},{y} is not the theme's");
                    }
                }
            }
        }
    }

    /// Before the first pass there is nothing to draw, and the panel says so
    /// rather than showing an empty grid that reads as a silent band.
    #[test]
    fn an_empty_history_says_it_is_waiting() {
        let out = draw(NetCoexistPanel, 60, 20, &SdrMetrics::fixture().streaming()).join("\n");
        assert!(out.contains("waiting for the first pass"), "{out}");
    }

    /// A pass that will never come is not a pass to wait for.
    #[test]
    fn a_survey_that_cannot_run_is_not_a_pass_to_wait_for() {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.survey_refused = Some("1.8 MHz of view is too narrow".to_string());
        let out = draw(NetCoexistPanel, 60, 20, &m).join("\n");
        assert!(out.contains("no pass is possible"), "{out}");
        assert!(out.contains("1.8 MHz"), "{out}");
        assert!(!out.contains("waiting"), "{out}");
    }

    /// A history shorter than the canvas leaves the old end, the bottom, dark
    /// rather than stretching what there is down it.
    #[test]
    fn a_short_history_does_not_pretend_to_fill_the_panel() {
        let theme = crate::Theme::sdr();
        let buf = cells(&with(vec![column(40); 4]), 60, 20);
        let x = band_axis::column_of(40, 60) as u16;
        // Four moments fill the top two rows; the bottom of the canvas is dark.
        assert_ne!(buf.get(x, 0).style().fg, Some(ink(None, &theme)));
        assert_eq!(buf.get(x, 17).style().fg, Some(ink(None, &theme)));
    }

    #[test]
    fn it_fits_every_size_the_layout_can_hand_it() {
        for w in 10..130u16 {
            for h in 3..30u16 {
                for line in draw(NetCoexistPanel, w, h, &with(vec![column(40); 10])) {
                    assert!(line.chars().count() <= w as usize, "{w}x{h}: {line:?}");
                }
            }
        }
    }
}
