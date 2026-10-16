// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `NetWifiRiverPanel` - the channel's time, frame by frame: lanes read
//! left to right and top to bottom like lines of text, the newest frame
//! ending the last lane.
//!
//! **One glyph language with the list.** A frame is drawn in its format's
//! colour, as tall as its SNR in sixteenths of a fixed 0 to 24 dB over the
//! lane's two rows (the list's eighth is the same height halved), and as
//! wide as the airtime it stated; a frame that stated no length is its
//! start, `▌`; one whose signalling did not read is the unreadable shade.
//! At 1 ms a cell the river is the channel's load: each cell's share on
//! air, by quarters, in the colour of the format with most of it.
//!
//! **The scale is a key's, never the traffic's** (`z`): a scale that
//! followed the bursts would change every frame's width from one second to
//! the next. **The selected frame is always on it**, lit: the window ends
//! at the newest frame shown, and slides back to the selected one when it
//! would fall outside. Held, the river holds with the list. Only the run
//! the newest frame is in is drawn: across a break the stream's clock
//! starts again, and a frame from before it has no place on this one.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    Frame,
};

use super::frames::{capture, ink, selection};
use super::{frames_silence, plate, put};
use crate::state::{Family, FrameZoom, RowKind, SdrMetrics, WifiRow};
use crate::ui::panel::{FeedSpan, Panel, PanelChrome, Staleness};

pub struct NetWifiRiverPanel;

/// Columns of a lane's label, before its cells.
const LABEL: u16 = 9;

const EIGHTHS: [char; 9] = [
    ' ', '\u{2581}', '\u{2582}', '\u{2583}', '\u{2584}', '\u{2585}', '\u{2586}', '\u{2587}',
    '\u{2588}',
];

/// The cells a capture spans when the river is drawn in `outer`: its
/// lanes times their cells, the same count [`draw`] lays out.
pub fn capture_cells(outer: Rect) -> u64 {
    let inner_h = outer.height.saturating_sub(2);
    let lanes = (inner_h.saturating_sub(1) / 2).clamp(1, 4);
    let cells = outer.width.saturating_sub(2 + LABEL);
    u64::from(lanes) * u64::from(cells)
}

/// Lanes in a column of `rows`: four where there is room, two otherwise.
fn lanes_for(rows: u16) -> u16 {
    if rows >= 24 {
        4
    } else {
        2
    }
}

/// A frame on the window's clock, in microseconds from the newest frame's
/// start.
#[derive(Clone, Copy)]
struct Placed {
    start: f64,
    air: Option<f64>,
    colour: Color,
    unreadable: bool,
    snr_db: Option<f64>,
    /// How long its preamble is, tinted at 4 us a cell.
    preamble: f64,
}

impl Placed {
    fn of(row: &WifiRow, zero: u64, theme: &crate::Theme) -> Self {
        use crate::signal::wifi::dsss::header::Preamble;
        let preamble = match row.kind {
            RowKind::Dsss {
                preamble: Preamble::Long,
                ..
            }
            | RowKind::OtherDsss {
                preamble: Preamble::Long,
                ..
            } => 192.0,
            RowKind::Dsss { .. } | RowKind::OtherDsss { .. } => 96.0,
            RowKind::Ofdm(_) | RowKind::OtherOfdm { .. } => 20.0,
        };
        Placed {
            start: (row.start as f64 - zero as f64) / 20.0,
            air: row.airtime_us.map(f64::from),
            colour: ink(row, theme),
            unreadable: row.family() == Some(Family::Undecoded),
            snr_db: row.snr_db,
            preamble,
        }
    }

    /// Where it ends: its stated airtime, or one cell for a start alone.
    fn end(&self, d: f64) -> f64 {
        self.start + self.air.unwrap_or(d)
    }
}

/// A cell's glyphs, top row and bottom, and their colour.
type Cell = (char, char, Color);

/// What the cell from `a`, `d` microseconds long, shows.
fn cell(near: &[Placed], a: f64, d: f64, zoom: FrameZoom, theme: &crate::Theme) -> Option<Cell> {
    let b = a + d;
    let overlap = |f: &Placed, air: f64| (f.start + air).min(b) - f.start.max(a);
    if zoom == FrameZoom::Load {
        // The load: the share on air as shade, the colour with most of it.
        let mut by: Vec<(Color, f64)> = Vec::new();
        let mut tick = None;
        for f in near {
            match f.air {
                Some(air) => {
                    let o = overlap(f, air);
                    if o > 0.0 {
                        match by.iter_mut().find(|(c, _)| *c == f.colour) {
                            Some((_, sum)) => *sum += o,
                            None => by.push((f.colour, o)),
                        }
                    }
                }
                None if f.start >= a && f.start < b => tick = Some(f.colour),
                None => {}
            }
        }
        let share = by.iter().map(|(_, o)| o).sum::<f64>() / d;
        if share <= 0.0 {
            return tick.map(|c| ('\u{258c}', '\u{258c}', c));
        }
        let colour = by
            .iter()
            .max_by(|x, y| x.1.total_cmp(&y.1))
            .map_or(theme.value, |(c, _)| *c);
        let g = match share {
            s if s <= 0.25 => '\u{2591}',
            s if s <= 0.5 => '\u{2592}',
            s if s <= 0.75 => '\u{2593}',
            _ => '\u{2588}',
        };
        return Some((g, g, colour));
    }
    // Finer: the frame most of the cell belongs to, or a short one that
    // starts in it; failing that, a start alone.
    let mut best: Option<(f64, &Placed)> = None;
    let mut tick = None;
    for f in near {
        match f.air {
            Some(air) => {
                let o = overlap(f, air);
                let starts_here = f.start >= a && f.start < b;
                if (o >= d / 2.0 || (starts_here && air < d)) && best.is_none_or(|(bo, _)| o > bo) {
                    best = Some((o, f));
                }
            }
            None if f.start >= a && f.start < b => tick = Some(f.colour),
            None => {}
        }
    }
    let Some((_, f)) = best else {
        return tick.map(|c| ('\u{258c}', '\u{258c}', c));
    };
    if f.unreadable {
        return Some(('\u{2592}', '\u{2592}', f.colour));
    }
    let level = f.snr_db.map_or(1, |snr| {
        ((snr / 24.0 * 16.0).ceil() as i64).clamp(1, 16) as usize
    });
    let colour = if zoom == FrameZoom::Anatomy && a + d / 2.0 < f.start + f.preamble {
        crate::ui::widgets::charts::lerp_color(f.colour, theme.value, 0.55)
    } else {
        f.colour
    };
    let top = if level > 8 { EIGHTHS[level - 8] } else { ' ' };
    Some((top, EIGHTHS[level.min(8)], colour))
}

/// A time on the river, short enough for a lane's label.
fn time(us: f64, signed: bool) -> String {
    let a = us.abs();
    let (v, unit, places) = if a >= 1e6 {
        (us / 1e6, "s", 2)
    } else if a >= 1e5 {
        (us / 1e3, "ms", 0)
    } else if a >= 1e3 {
        (us / 1e3, "ms", 1)
    } else {
        (us, "\u{b5}s", 0)
    };
    if signed {
        format!("{v:+.places$} {unit}")
    } else {
        format!("{v:.places$} {unit}")
    }
}

/// What the river draws of the capture, in microseconds from the frame
/// that set it off.
struct Window {
    placed: Vec<Placed>,
    /// The first lane's start.
    t0: f64,
    /// The capture's end: nothing is drawn past it.
    end: f64,
    lit: Option<Placed>,
    lost: Vec<(f64, f64)>,
}

/// The capture's frames, its window, the selected frame where it can be lit
/// and the spans a feed gap took; before the first capture, an empty
/// window with its trigger 10 % in.
fn window(state: &SdrMetrics, span: f64, lanes: u16, theme: &crate::Theme) -> Window {
    let whole = span * f64::from(lanes);
    let Some((c, at)) = selection(state) else {
        return Window {
            placed: Vec::new(),
            t0: -0.1 * whole,
            end: f64::INFINITY,
            lit: None,
            lost: Vec::new(),
        };
    };
    let zero = c.trigger_row().map_or(c.from, |t| t.start);
    let us = |pair: u64| (pair as f64 - zero as f64) / 20.0;
    let placed: Vec<Placed> = c.rows.iter().map(|r| Placed::of(r, zero, theme)).collect();
    let lit = Placed::of(&c.rows[at], zero, theme);
    // A capture longer than this river (the screen shrank since) slides
    // to keep the selection in view.
    let mut t0 = us(c.from);
    if lit.start >= t0 + whole {
        t0 = lit.start - 0.3 * span;
    }
    Window {
        placed,
        t0,
        end: us(c.to),
        lit: Some(lit),
        lost: c.lost.iter().map(|l| (us(l.from), us(l.to))).collect(),
    }
}

fn draw(buf: &mut Buffer, area: Rect, state: &SdrMetrics, theme: &crate::Theme) {
    let lanes = ((area.height.saturating_sub(1)) / 2).clamp(1, 4);
    let lw = area.width.saturating_sub(LABEL);
    if lw < 10 {
        return;
    }
    // A capture is drawn at the scale it was taken at, until the next.
    let zoom = capture(state).map_or(state.net.wifi.frames_view.zoom, |c| c.zoom);
    let d = zoom.cell_us();
    let span = d * f64::from(lw);
    let silent = frames_silence(state);
    let ink_label = Style::default().fg(if silent.is_some() {
        theme.stale
    } else {
        theme.label
    });
    let Window {
        placed,
        t0,
        end,
        lit,
        lost,
    } = window(state, span, lanes, theme);
    // The selection, highlighted: its glyphs in the focus colour on the dim
    // border colour (a tall frame fills its cells, so the colour carries it
    // there), and a pointer at its lane's label.
    let mark = Style::default()
        .fg(theme.border_focused)
        .add_modifier(Modifier::BOLD);
    for i in 0..lanes {
        let (top_y, bottom_y) = (area.y + 2 * i, area.y + 2 * i + 1);
        let a0 = t0 + f64::from(i) * span;
        put(
            buf,
            area.x,
            bottom_y,
            &format!("{:>8} ", time(a0, true)),
            ink_label,
            area,
        );
        if silent.is_some() {
            for x in 0..lw {
                buf.get_mut(area.x + LABEL + x, bottom_y)
                    .set_char('\u{2581}')
                    .set_style(Style::default().fg(theme.border_dim));
            }
            continue;
        }
        let near: Vec<Placed> = placed
            .iter()
            .filter(|f| f.start < a0 + span && f.end(d) > a0)
            .copied()
            .collect();
        for x in 0..lw {
            let a = a0 + f64::from(x) * d;
            if a >= end {
                break;
            }
            let cx = area.x + LABEL + x;
            // Never heard: hatched, so it does not pass for quiet air.
            if lost.iter().any(|&(from, to)| a < to && a + d > from) {
                let hatch = Style::default().fg(theme.stale);
                buf.get_mut(cx, top_y).set_char('\u{2571}').set_style(hatch);
                buf.get_mut(cx, bottom_y)
                    .set_char('\u{2571}')
                    .set_style(hatch);
                continue;
            }
            let selected = lit.is_some_and(|s| a < s.end(d) && a + d > s.start);
            if selected {
                buf.get_mut(cx, top_y).set_bg(theme.border_dim);
                buf.get_mut(cx, bottom_y).set_bg(theme.border_dim);
                for y in [top_y, bottom_y] {
                    buf.get_mut(area.x + LABEL - 1, y)
                        .set_char('\u{25b8}')
                        .set_style(mark);
                }
            }
            if let Some((t, b, colour)) = cell(&near, a, d, zoom, theme) {
                // Its anatomy kept: at 4 us a cell the preamble paler, here
                // from the focus colour.
                let colour = match lit.filter(|_| selected) {
                    Some(s) if zoom == FrameZoom::Anatomy && a + d / 2.0 < s.start + s.preamble => {
                        crate::ui::widgets::charts::lerp_color(
                            theme.border_focused,
                            theme.value,
                            0.55,
                        )
                    }
                    Some(_) => theme.border_focused,
                    None => colour,
                };
                buf.get_mut(cx, top_y).set_char(t).set_fg(colour);
                buf.get_mut(cx, bottom_y).set_char(b).set_fg(colour);
                if selected {
                    buf.get_mut(cx, top_y).set_style(mark.fg(colour));
                    buf.get_mut(cx, bottom_y).set_style(mark.fg(colour));
                }
            }
        }
    }
    let axis = area.y + 2 * lanes;
    if axis < area.bottom() {
        for x in (0..lw.saturating_sub(8)).step_by(30) {
            let at = put(buf, area.x + LABEL + x, axis, "\u{250a}", ink_label, area);
            put(
                buf,
                at,
                axis,
                &time(f64::from(x) * d, false),
                ink_label,
                area,
            );
        }
    }
    if let Some(s) = &silent {
        plate(buf, area, s, theme);
    }
}

impl Panel for NetWifiRiverPanel {
    fn name(&self) -> &'static str {
        "net_wifi_river"
    }

    fn reads_spectrum(&self) -> bool {
        false
    }

    fn min_size(&self) -> (u16, u16) {
        (40, 5)
    }

    fn column_height(&self, column: u16) -> Option<u16> {
        // Two rows a lane, the axis, and the frame's two borders.
        Some(2 * lanes_for(column) + 3)
    }

    fn chrome(&self, state: &SdrMetrics) -> PanelChrome {
        let scope = &state.net.wifi.scope;
        let wide = super::card::on_screen(state);
        let mut suffix = match capture(state) {
            Some(c) => format!(" \u{b7} capture {} \u{b7} {}", c.number, c.zoom.label()),
            None => format!(
                " \u{b7} {} \u{b7} armed",
                state.net.wifi.frames_view.zoom.label()
            ),
        };
        if wide {
            suffix.push_str(&format!(
                " \u{b7} trigger: {} \u{b7} holdoff {:.1} s \u{b7} z t",
                scope.trigger.label(),
                scope.holdoff.millis() as f64 / 1e3
            ));
        } else if let Some(w) = super::card::reading_words(state) {
            // Where the card has no room, its reading rides here, and the
            // trigger and holdoff give way: the list's keys name them.
            suffix.push_str(&format!(" \u{b7} {w}"));
        }
        if scope.stopped {
            suffix.push_str(" \u{b7} STOP");
        }
        PanelChrome::new("River")
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
    use super::NetWifiRiverPanel;
    use crate::signal::dsp::uncertainty::Uncertain;
    use crate::signal::wifi::format::{Format, Heard};
    use crate::signal::wifi::lsig::{LSig, Refused};
    use crate::signal::wifi::signal_field::Rate;
    use crate::state::fixture::draw;
    use crate::state::{Capture, FrameZoom, Lost, RowKind, SdrMetrics, Trigger, WifiRow};

    /// The Frames view on channel 11 at 20 Msps, streaming.
    fn on_eleven(zoom: FrameZoom) -> SdrMetrics {
        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = "wifi_channel".to_string();
        m.ui.active_preset = "wifi_frames".to_string();
        m.radio.frequency = 2_462_000_000;
        m.radio.config_sample_rate = 20e6;
        m.net.wifi.ran = true;
        m.net.wifi.channel = Some(11);
        m.net.mode = crate::state::NetMode::Lock;
        m.net.wifi.frames_view.zoom = zoom;
        m
    }

    fn non_ht() -> RowKind {
        RowKind::Ofdm(Heard {
            lsig: Some(Ok(LSig {
                rate: Rate::R6,
                length: 14,
            })),
            format: Some(Format::NonHt),
        })
    }

    /// A non-HT frame `start_us` into the run, `air_us` long, at `snr_db`.
    fn frame(start_us: f64, air_us: Option<u32>, snr_db: f64) -> WifiRow {
        WifiRow {
            seq: 0,
            start: (start_us * 20.0) as u64,
            run: 0,
            when: std::time::Instant::now(),
            carrier_hz: 2_462e6,
            kind: non_ht(),
            airtime_us: air_us,
            gap_us: None,
            offset_hz: Some(Uncertain::from_sigma(-48_000.0, 200.0)),
            snr_db: Some(snr_db),
            channel: None,
            clock: None,
            epoch: 0,
        }
    }

    /// 100 wide and 11 tall: 98 inner columns, so 89 cells a lane after its
    /// label, and four lanes over the axis.
    fn lines(m: &SdrMetrics) -> Vec<String> {
        draw(NetWifiRiverPanel, 100, 11, m)
    }

    /// Lane `i`'s two rows' cells, top and bottom.
    fn lane(l: &[String], i: usize) -> (Vec<char>, Vec<char>) {
        let cells = |y: usize| l[y].chars().skip(10).take(89).collect();
        (cells(1 + 2 * i), cells(2 + 2 * i))
    }

    /// The river drawn into a buffer, for its colours.
    fn buffer(m: &SdrMetrics) -> ratatui::buffer::Buffer {
        let theme = crate::Theme::sdr();
        let mut registry = crate::ui::PanelRegistry::new();
        registry.register(NetWifiRiverPanel);
        let mut t = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 11)).unwrap();
        t.draw(|f| registry.render_panel("net_wifi_river", f, f.size(), m, &theme, false))
            .unwrap();
        t.backend().buffer().clone()
    }

    fn count(cells: &[char], c: char) -> usize {
        cells.iter().filter(|&&x| x == c).count()
    }

    /// The ring as one capture set off by its `trig`th frame, its window
    /// from `from_us` across the river's four lanes of 89 cells at its zoom.
    fn captured(m: &mut SdrMetrics, trig: usize, from_us: f64) {
        let rows: Vec<WifiRow> = m.net.wifi.rows.iter().copied().collect();
        let zoom = m.net.wifi.frames_view.zoom;
        let from = (from_us * 20.0) as u64;
        m.net.wifi.scope.capture = Some(Capture {
            number: 7,
            zoom,
            trigger: Trigger::Any,
            epoch: 0,
            from,
            to: from + 4 * 89 * zoom.cell_pairs(),
            trigger_seq: rows[trig].seq,
            rows,
            lost: Vec::new(),
        });
    }

    /// **At 40 us a cell a frame is as wide as its airtime and as tall as
    /// its SNR**, in sixteenths of 0 to 24 dB over the lane's two rows: a
    /// 200 us frame at 24 dB is five cells full in both rows, the 40 us
    /// frame after it at 12 dB one cell full in the lower row only. The
    /// capture's window starts the first lane.
    #[test]
    fn a_frame_is_as_wide_as_its_airtime_and_as_tall_as_its_snr() {
        let mut m = on_eleven(FrameZoom::Rhythm);
        m.net.wifi.push_row(frame(0.0, Some(200), 24.0));
        m.net.wifi.push_row(frame(216.0, Some(40), 12.0));
        captured(&mut m, 0, 0.0);
        let l = lines(&m);
        let (top, bottom) = lane(&l, 0);
        assert_eq!(count(&top, '\u{2588}'), 5, "{l:#?}");
        assert_eq!(count(&bottom, '\u{2588}'), 6, "{l:#?}");
        assert_eq!(bottom[0], '\u{2588}', "the window starts the lane: {l:#?}");
        assert_eq!((top[5], bottom[5]), (' ', '\u{2588}'), "{l:#?}");
        let title = &l[0];
        for want in [
            "capture 7",
            "40 \u{b5}s a cell",
            "trigger: any frame",
            "holdoff 1.0 s",
        ] {
            assert!(title.contains(want), "{want} in {title}");
        }
    }

    /// A frame that stated no length is drawn as its start; one whose
    /// signalling did not read as the unreadable shade.
    #[test]
    fn a_start_alone_and_an_unreadable_frame_are_told_apart() {
        let mut m = on_eleven(FrameZoom::Rhythm);
        m.net.wifi.push_row(frame(0.0, None, 20.0));
        m.net.wifi.push_row(WifiRow {
            kind: RowKind::Ofdm(Heard {
                lsig: Some(Err(Refused::Parity)),
                format: None,
            }),
            ..frame(400.0, Some(80), 20.0)
        });
        captured(&mut m, 0, 0.0);
        let l = lines(&m);
        let (top, bottom) = lane(&l, 0);
        assert_eq!(count(&top, '\u{258c}'), 1, "{l:#?}");
        assert_eq!(count(&bottom, '\u{258c}'), 1, "{l:#?}");
        assert_eq!(count(&bottom, '\u{2592}'), 2, "{l:#?}");
    }

    /// **At 1 ms a cell the river is the load**: a cell half on air reads
    /// as the half shade.
    #[test]
    fn at_a_millisecond_a_cell_the_share_on_air_is_the_shade() {
        let mut m = on_eleven(FrameZoom::Load);
        m.net.wifi.push_row(frame(0.0, Some(500), 20.0));
        captured(&mut m, 0, 0.0);
        let l = lines(&m);
        let (top, bottom) = lane(&l, 0);
        assert_eq!((top[0], bottom[0]), ('\u{2592}', '\u{2592}'), "{l:#?}");
        assert!(l[0].contains("1 ms a cell"), "{l:#?}");
    }

    /// **Time zero is the trigger**: the lanes' labels count from the frame
    /// that set the capture off, before it negative and after it positive.
    #[test]
    fn the_lanes_count_from_the_trigger() {
        let mut m = on_eleven(FrameZoom::Rhythm);
        m.net.wifi.push_row(frame(10_000.0, Some(40), 27.0));
        // 10 % of four lanes of 89 cells at 40 us before it.
        captured(&mut m, 0, 10_000.0 - 1_424.0);
        let l = lines(&m);
        assert!(l[2].contains("-1.4 ms"), "{l:#?}");
        assert!(l[4].contains("+2.1 ms"), "{l:#?}");
    }

    /// **The selected frame is highlighted**: its glyphs in the focus
    /// colour on the dim border colour, and a pointer at its lane's label;
    /// nothing around it and nothing on the axis.
    #[test]
    fn the_selected_frame_is_highlighted() {
        let mut m = on_eleven(FrameZoom::Rhythm);
        m.net.wifi.push_row(frame(0.0, Some(80), 27.0));
        m.net.wifi.push_row(frame(4_000.0, Some(80), 27.0));
        captured(&mut m, 0, 0.0);
        m.net.wifi.frames_view.selected = Some(2);
        let buf = buffer(&m);
        let theme = crate::Theme::sdr();
        // The second frame, 4 ms in: lane 1 (lanes of 89 cells of 40 us),
        // cells 11 and 12, rows 3 and 4.
        let at = |x: u16, y: u16| buf.get(x, y);
        for y in [3, 4] {
            for cell in [11, 12] {
                assert_eq!(at(10 + cell, y).fg, theme.border_focused, "{cell},{y}");
                assert_eq!(at(10 + cell, y).bg, theme.border_dim, "{cell},{y}");
            }
            assert_ne!(at(10 + 10, y).bg, theme.border_dim, "nothing beside it");
            assert_ne!(at(10 + 13, y).bg, theme.border_dim, "nothing beside it");
            assert_eq!(at(9, y).symbol(), "\u{25b8}", "the pointer at lane 1");
        }
        assert_ne!(
            at(10, 2).fg,
            theme.border_focused,
            "the other frame keeps its colour"
        );
        assert_ne!(at(9, 2).symbol(), "\u{25b8}", "no pointer at lane 0");
        let axis: String = (0..100).map(|x| at(x, 9).symbol()).collect();
        assert!(
            !axis.contains('\u{2501}'),
            "nothing under it on the axis: {axis}"
        );
    }

    /// **Not locked on a channel**: the survey walks the band, and a view of
    /// one channel's frames has none to show; the river says so in the
    /// shared plate, with the keys that lock it here and where else to.
    #[test]
    fn unlocked_the_river_says_how_to_lock_a_channel() {
        let mut m = on_eleven(FrameZoom::Rhythm);
        m.net.mode = crate::state::NetMode::Survey;
        let out = lines(&m).join("\n");
        assert!(out.contains("NOT LOCKED ON A CHANNEL"), "{out}");
        assert!(
            out.contains("m") && out.contains("\u{2190} \u{2192}"),
            "{out}"
        );
        assert!(out.contains("Survey"), "{out}");
    }

    /// At 4 us a cell the preamble is told from the rest of the frame: its
    /// 20 us drawn toward the value colour, the rest in the format's own.
    #[test]
    fn at_four_microseconds_a_cell_the_preamble_is_tinted() {
        let mut m = on_eleven(FrameZoom::Anatomy);
        m.net.wifi.push_row(frame(0.0, Some(40), 27.0));
        m.net.wifi.push_row(frame(200.0, Some(40), 27.0));
        captured(&mut m, 0, 0.0);
        m.net.wifi.frames_view.selected = Some(2);
        let buf = buffer(&m);
        let theme = crate::Theme::sdr();
        let fg = |cell: u16| buf.get(10 + cell, 2).fg;
        let lerp = crate::ui::widgets::charts::lerp_color;
        let tinted = lerp(theme.wifi_non_ht, theme.value, 0.55);
        assert_eq!(fg(0), tinted);
        assert_eq!(fg(4), tinted);
        assert_eq!(fg(5), theme.wifi_non_ht);
        assert_eq!(fg(9), theme.wifi_non_ht);
        // The selected frame keeps its anatomy, in the focus colour.
        assert_eq!(fg(50), lerp(theme.border_focused, theme.value, 0.55));
        assert_eq!(fg(55), theme.border_focused);
    }

    /// **A span a feed gap took is hatched**, never drawn as quiet air.
    #[test]
    fn a_lost_span_is_hatched() {
        let mut m = on_eleven(FrameZoom::Rhythm);
        m.net.wifi.push_row(frame(0.0, Some(40), 27.0));
        captured(&mut m, 0, 0.0);
        if let Some(c) = m.net.wifi.scope.capture.as_mut() {
            c.lost = vec![Lost {
                epoch: 0,
                from: 2_000 * 20,
                to: 3_000 * 20,
            }];
        }
        let l = lines(&m);
        let (top, bottom) = lane(&l, 0);
        assert_eq!(count(&top, '\u{2571}'), 25, "{l:#?}");
        assert_eq!(count(&bottom, '\u{2571}'), 25, "{l:#?}");
        assert_eq!(bottom[50], '\u{2571}', "{l:#?}");
    }

    /// Single says STOP; before the first capture the lanes are empty and
    /// the title says what it is armed for.
    #[test]
    fn stopped_and_armed_are_said() {
        let mut m = on_eleven(FrameZoom::Rhythm);
        m.net.wifi.push_row(frame(0.0, Some(40), 27.0));
        assert!(lines(&m)[0].contains("armed"), "{:#?}", lines(&m));
        captured(&mut m, 0, 0.0);
        m.net.wifi.scope.stopped = true;
        assert!(lines(&m)[0].contains("STOP"), "{:#?}", lines(&m));
    }

    /// Not measured: the lanes dimmed under the shared plate, its head and
    /// the key that helps; listening: the lanes and the axis, empty.
    #[test]
    fn the_silences_are_drawn() {
        let mut m = on_eleven(FrameZoom::Load);
        m.radio.config_sample_rate = 2e6;
        m.net.wifi.refused = crate::signal::net::wifi::rate_refusal(2e6);
        let out = lines(&m).join("\n");
        assert!(out.contains("WRONG SAMPLE RATE"), "{out}");

        let l = lines(&on_eleven(FrameZoom::Load));
        assert!(l[9].contains('\u{250a}'), "the axis: {l:#?}");
        let drawn: usize = (0..4)
            .map(|i| {
                let (t, b) = lane(&l, i);
                t.iter().chain(&b).filter(|c| **c != ' ').count()
            })
            .sum();
        assert_eq!(drawn, 0, "{l:#?}");
    }

    /// **The capture is as long as the river shows**: the engine says where
    /// the river is drawn, by the same placement it draws from, and its
    /// lanes times their cells are the capture's window.
    #[test]
    fn the_capture_window_is_the_rivers_lanes_times_their_cells() {
        let (engine, _) =
            crate::app::App::build_ui("wifi_frames", &std::collections::HashMap::new(), None, true);
        let m = on_eleven(FrameZoom::Load);
        let theme = crate::Theme::sdr();
        for (w, h, lanes) in [(191u16, 41u16, 4u64), (152, 32, 2), (100, 30, 2)] {
            let size = ratatui::layout::Rect::new(0, 0, w, h);
            let at = engine.area_of("net_wifi_river", size, &m).expect("a river");
            let mut t = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
            t.draw(|f| engine.draw(f, &m, &theme)).unwrap();
            let buf = t.backend().buffer().clone();
            assert_eq!(
                buf.get(at.x, at.y).symbol(),
                "\u{256d}",
                "{w} x {h}: {at:?}"
            );
            assert_eq!(buf.get(at.x + at.width - 1, at.y).symbol(), "\u{256e}");
            let cells = u64::from(at.width - 2 - super::LABEL);
            assert_eq!(super::capture_cells(at), lanes * cells, "{w} x {h}");
        }
        let narrow = ratatui::layout::Rect::new(0, 0, 100, 30);
        assert_eq!(engine.area_of("net_wifi_card", narrow, &m), None);
    }

    /// **The river's height is the column's to decide**: four lanes when the
    /// body has 24 rows or more, two below, and the list takes the rest.
    #[test]
    fn the_river_takes_four_lanes_or_two_by_the_room() {
        let (engine, _) =
            crate::app::App::build_ui("wifi_frames", &std::collections::HashMap::new(), None, true);
        let m = on_eleven(FrameZoom::Load);
        let theme = crate::Theme::sdr();
        for (w, h, rows) in [(191u16, 41u16, 11usize), (152, 32, 7)] {
            let mut t = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
            t.draw(|f| engine.draw(f, &m, &theme)).unwrap();
            let buf = t.backend().buffer().clone();
            let text: Vec<String> = (0..h)
                .map(|y| (0..w).map(|x| buf.get(x, y).symbol()).collect())
                .collect();
            let at = |title: &str| {
                text.iter()
                    .position(|l| l.starts_with(&format!("\u{256d} {title}")))
                    .unwrap_or_else(|| panic!("no {title} at {w} x {h}:\n{}", text.join("\n")))
            };
            assert_eq!(at("Frames") - at("River"), rows, "{w} x {h}");
        }
    }
}
