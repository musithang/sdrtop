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
    style::{Color, Style},
    Frame,
};

use super::frames::{at, ink, shown, top};
use super::{plate, put, silence};
use crate::state::{Family, FrameZoom, RowKind, SdrMetrics, WifiRow};
use crate::ui::panel::{FeedSpan, Panel, PanelChrome, Staleness, Tag};

pub struct NetWifiRiverPanel;

/// Columns of a lane's label, before its cells.
const LABEL: u16 = 9;

const EIGHTHS: [char; 9] = [
    ' ', '\u{2581}', '\u{2582}', '\u{2583}', '\u{2584}', '\u{2585}', '\u{2586}', '\u{2587}',
    '\u{2588}',
];

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
    fn of(row: &WifiRow, newest: &WifiRow, theme: &crate::Theme) -> Self {
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
            start: (row.start as f64 - newest.start as f64) / 20.0,
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

/// The frames to draw, oldest first, the window's start, and the selected
/// frame where it can be lit.
fn window(
    state: &SdrMetrics,
    d: f64,
    span: f64,
    lanes: u16,
    theme: &crate::Theme,
) -> (Vec<Placed>, f64, Option<Placed>) {
    let whole = span * f64::from(lanes);
    let shown = shown(state);
    let view = &state.net.wifi.frames_view;
    let Some(t) = top(&shown, view).filter(|&t| t < shown.len()) else {
        // Nothing to end on: the window ends now, and is empty.
        return (Vec::new(), -whole, None);
    };
    let newest = shown[t];
    let placed: Vec<Placed> = shown[t..]
        .iter()
        .rev()
        .filter(|r| r.run == newest.run)
        .map(|r| Placed::of(r, newest, theme))
        .collect();
    let lit = at(&shown, view)
        .map(|i| shown[i])
        .filter(|r| r.run == newest.run)
        .map(|r| Placed::of(r, newest, theme));
    let mut t0 = Placed::of(newest, newest, theme).end(d) - whole;
    if let Some(s) = lit.filter(|s| s.start < t0) {
        t0 = s.start - 0.3 * span;
    }
    (placed, t0, lit)
}

fn draw(buf: &mut Buffer, area: Rect, state: &SdrMetrics, theme: &crate::Theme) {
    let lanes = ((area.height.saturating_sub(1)) / 2).clamp(1, 4);
    let lw = area.width.saturating_sub(LABEL);
    if lw < 10 {
        return;
    }
    let zoom = state.net.wifi.frames_view.zoom;
    let d = zoom.cell_us();
    let span = d * f64::from(lw);
    let silent = silence(state);
    let ink_label = Style::default().fg(if silent.is_some() {
        theme.stale
    } else {
        theme.label
    });
    let (placed, t0, lit) = window(state, d, span, lanes, theme);
    let lit_bg = theme.border_dim;
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
            let cx = area.x + LABEL + x;
            if lit.is_some_and(|s| a < s.end(d) && a + d > s.start) {
                buf.get_mut(cx, top_y).set_bg(lit_bg);
                buf.get_mut(cx, bottom_y).set_bg(lit_bg);
            }
            if let Some((t, b, colour)) = cell(&near, a, d, zoom, theme) {
                buf.get_mut(cx, top_y).set_char(t).set_fg(colour);
                buf.get_mut(cx, bottom_y).set_char(b).set_fg(colour);
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

    fn min_size(&self) -> (u16, u16) {
        (40, 5)
    }

    fn column_height(&self, column: u16) -> Option<u16> {
        // Two rows a lane, the axis, and the frame's two borders.
        Some(2 * lanes_for(column) + 3)
    }

    fn chrome(&self, state: &SdrMetrics) -> PanelChrome {
        let view = &state.net.wifi.frames_view;
        // Held with the list, and paused by the user as the list is.
        PanelChrome::new("River")
            .stale_when(Staleness::NotStreaming)
            .tag_if(true, state.net.mode.tag())
            .counts_from_feed(FeedSpan::Session)
            .tag_if(view.held.is_some(), Tag::Paused)
            .suffix(format!(" \u{b7} {} \u{b7} z", view.zoom.label()))
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
    use crate::state::{FrameZoom, RowKind, SdrMetrics, WifiRow};

    /// The Frames view on channel 11 at 20 Msps, streaming.
    fn on_eleven(zoom: FrameZoom) -> SdrMetrics {
        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = "wifi_channel".to_string();
        m.ui.active_preset = "wifi_frames".to_string();
        m.radio.frequency = 2_462_000_000;
        m.radio.config_sample_rate = 20e6;
        m.net.wifi.ran = true;
        m.net.wifi.channel = Some(11);
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

    fn count(cells: &[char], c: char) -> usize {
        cells.iter().filter(|&&x| x == c).count()
    }

    /// **At 40 us a cell a frame is as wide as its airtime and as tall as
    /// its SNR**, in sixteenths of 0 to 24 dB over the lane's two rows: a
    /// 200 us frame at 24 dB is five cells full in both rows, the 40 us
    /// frame after it at 12 dB one cell full in the lower row only. The
    /// newest frame ends the last lane.
    #[test]
    fn a_frame_is_as_wide_as_its_airtime_and_as_tall_as_its_snr() {
        let mut m = on_eleven(FrameZoom::Rhythm);
        m.net.wifi.push_row(frame(0.0, Some(200), 24.0));
        m.net.wifi.push_row(frame(216.0, Some(40), 12.0));
        let l = lines(&m);
        let (top, bottom) = lane(&l, 3);
        assert_eq!(count(&top, '\u{2588}'), 5, "{l:#?}");
        assert_eq!(count(&bottom, '\u{2588}'), 6, "{l:#?}");
        assert_eq!(bottom[88], '\u{2588}', "the newest ends the lane: {l:#?}");
        assert_eq!(top[88], ' ', "{l:#?}");
        assert!(l.join("\n").contains("40 \u{b5}s a cell"), "{l:#?}");
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
        let l = lines(&m);
        let (top, bottom) = lane(&l, 3);
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
        let l = lines(&m);
        let (top, bottom) = lane(&l, 3);
        assert_eq!(top[88], '\u{2592}', "{l:#?}");
        assert_eq!(bottom[88], '\u{2592}', "{l:#?}");
        assert!(l.join("\n").contains("1 ms a cell"), "{l:#?}");
    }

    /// **The selected frame is always on the river**: selected a second
    /// before the newest, the window slides back to put it 30 % into the
    /// first lane, and the lane's label counts back from the newest.
    #[test]
    fn the_window_slides_to_the_selected_frame() {
        let mut m = on_eleven(FrameZoom::Rhythm);
        m.net.wifi.push_row(frame(0.0, Some(100), 27.0));
        m.net.wifi.push_row(frame(1_000_000.0, Some(40), 27.0));
        m.net.wifi.frames_view.held = Some(2);
        m.net.wifi.frames_view.cursor = 1;
        let l = lines(&m);
        let (_, bottom) = lane(&l, 0);
        assert_eq!(bottom[27], '\u{2588}', "{l:#?}");
        assert_eq!(bottom[28], '\u{2588}', "{l:#?}");
        assert_eq!(count(&bottom, '\u{2588}'), 2, "{l:#?}");
        assert!(l[2].contains("-1.00 s"), "{l:#?}");
    }

    /// The selected frame is lit: its cells on the border's dim colour.
    #[test]
    fn the_selected_frame_is_lit() {
        let mut m = on_eleven(FrameZoom::Rhythm);
        m.net.wifi.push_row(frame(0.0, Some(80), 27.0));
        m.net.wifi.push_row(frame(200.0, Some(80), 27.0));
        let theme = crate::Theme::sdr();
        let mut registry = crate::ui::PanelRegistry::new();
        registry.register(NetWifiRiverPanel);
        let mut t = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 11)).unwrap();
        t.draw(|f| registry.render_panel("net_wifi_river", f, f.size(), &m, &theme, false))
            .unwrap();
        let buf = t.backend().buffer().clone();
        // The newest (selected) ends the last lane: cells 87 and 88; the
        // one before is 82 and 83.
        let bg = |cell: u16| buf.get(10 + cell, 8).bg;
        assert_eq!(bg(88), theme.border_dim);
        assert_eq!(bg(87), theme.border_dim);
        assert_ne!(bg(83), theme.border_dim);
    }

    /// At 4 us a cell the preamble is told from the rest of the frame: its
    /// 20 us drawn toward the value colour, the rest in the format's own.
    #[test]
    fn at_four_microseconds_a_cell_the_preamble_is_tinted() {
        let mut m = on_eleven(FrameZoom::Anatomy);
        m.net.wifi.push_row(frame(0.0, Some(40), 27.0));
        let theme = crate::Theme::sdr();
        let mut registry = crate::ui::PanelRegistry::new();
        registry.register(NetWifiRiverPanel);
        let mut t = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 11)).unwrap();
        t.draw(|f| registry.render_panel("net_wifi_river", f, f.size(), &m, &theme, false))
            .unwrap();
        let buf = t.backend().buffer().clone();
        // Ten cells end the last lane, 79 to 88: five of preamble, five after.
        let fg = |cell: u16| buf.get(10 + cell, 8).fg;
        let tinted = crate::ui::widgets::charts::lerp_color(theme.wifi_non_ht, theme.value, 0.55);
        assert_eq!(fg(79), tinted);
        assert_eq!(fg(83), tinted);
        assert_eq!(fg(84), theme.wifi_non_ht);
        assert_eq!(fg(88), theme.wifi_non_ht);
    }

    /// **Held, the river holds at the same moment** as the list: nothing
    /// newer than the held frame is drawn.
    #[test]
    fn a_held_river_draws_nothing_newer_than_the_held_frame() {
        let mut m = on_eleven(FrameZoom::Rhythm);
        // On the cells' grid, so each frame fills exactly one.
        for i in 0..3 {
            m.net
                .wifi
                .push_row(frame(320.0 * f64::from(i), Some(40), 27.0));
        }
        m.net.wifi.frames_view.held = Some(2);
        let l = lines(&m);
        let full: usize = (0..4).map(|i| count(&lane(&l, i).1, '\u{2588}')).sum();
        assert_eq!(full, 2, "{l:#?}");
        assert!(l[0].contains("PAUSED"), "{l:#?}");
    }

    /// Across a break the stream's clock starts again: a frame of another
    /// run is not placed on this run's clock.
    #[test]
    fn a_frame_of_another_run_is_not_placed() {
        let mut m = on_eleven(FrameZoom::Rhythm);
        m.net.wifi.push_row(frame(100.0, Some(40), 27.0));
        m.net.wifi.push_row(WifiRow {
            run: 1,
            ..frame(50.0, Some(40), 27.0)
        });
        let l = lines(&m);
        let full: usize = (0..4).map(|i| count(&lane(&l, i).1, '\u{2588}')).sum();
        assert_eq!(full, 1, "{l:#?}");
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
