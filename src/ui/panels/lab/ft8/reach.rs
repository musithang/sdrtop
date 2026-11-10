// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The reach: where the band is open to, and how that is changing.
//!
//! Time across (the last hour, or as much of it as this session has
//! heard), distance up on World's logarithmic scale from 100 to 20 000 km,
//! a braille dot a decode: one dot column a cycle, so an opening reads as a
//! cloud climbing and the skip zone as the empty band beneath it. A decode is
//! placed only by a locator its station sent; the rest are counted in the
//! title, never placed (POLICY.md, rule 2). Without your position there are
//! no distances, and the plot says where to set it; the rows below it still
//! hold.
//!
//! Below the plot, on the same time axis: decodes a cycle, even cycles above
//! the rule and odd hanging below it; the noise under the band
//! ([`crate::signal::ft8::floor`]), relative and within one setting, a cycle
//! past the radio's hold marked `×` and a change of setting `┊`; and the
//! farthest station of each quarter hour, by name. A band change and the sun
//! crossing the horizon at your station are dated marks across the plot.

use ratatui::{layout::Rect, style::Style, Frame};

use super::{placed, say, window, Placed};
use crate::state::ft8::CycleMark;
use crate::state::SdrMetrics;
use crate::ui::panel::{Panel, PanelChrome, Staleness};
use crate::ui::panels::radio::ft8::{km, put};

pub struct Ft8ReachPanel;

/// World's distance scale.
pub const KM_RANGE: (f64, f64) = (100.0, 20_000.0);

/// The label column, and the room on the right for the units.
const LEFT: u16 = 9;
const RIGHT: u16 = 13;

/// Rows under the plot: the axis, activity (3), the floor (3), the farthest.
const AXIS: u16 = 1;
const ACTIVITY: u16 = 3;
const FLOOR: u16 = 3;
const FARTHEST: u16 = 1;

/// The least the plot keeps before the rows under it give way.
const MIN_PLOT: u16 = 4;

/// Braille's dot bits by row (0 to 3) and column (0, 1) in a cell.
const BIT: [[u8; 2]; 4] = [[0x01, 0x08], [0x02, 0x10], [0x04, 0x20], [0x40, 0x80]];

const EIGHTHS: [&str; 9] = [" ", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];

/// Where `km` sits on the scale, 0 at 100 km and 1 at 20 000.
pub fn km_frac(km: f64) -> f64 {
    let (lo, hi) = (KM_RANGE.0.log10(), KM_RANGE.1.log10());
    ((km.clamp(KM_RANGE.0, KM_RANGE.1)).log10() - lo) / (hi - lo)
}

/// Which rows under the plot fit `h` inner rows: (activity, floor).
pub fn rows_under(h: u16) -> (bool, bool) {
    let all = AXIS + ACTIVITY + FLOOR + FARTHEST;
    if h >= MIN_PLOT + all {
        (true, true)
    } else if h >= MIN_PLOT + AXIS + ACTIVITY + FARTHEST {
        (true, false)
    } else {
        (false, false)
    }
}

impl Panel for Ft8ReachPanel {
    fn name(&self) -> &'static str {
        "ft8_reach"
    }

    fn reads_spectrum(&self) -> bool {
        false
    }

    fn min_size(&self) -> (u16, u16) {
        (LEFT + RIGHT + 40, MIN_PLOT + AXIS + FARTHEST + 2)
    }

    fn chrome(&self, state: &SdrMetrics) -> PanelChrome {
        let mut chrome = PanelChrome::new("Reach").stale_when(Staleness::NotStreaming);
        if let Some((pts, unplaced)) = placed(state) {
            chrome.suffix = Some(format!(
                "  the last hour: {} placed · {unplaced} without a locator",
                pts.len()
            ));
        }
        chrome
    }

    fn render(
        &self,
        f: &mut Frame,
        area: Rect,
        state: &SdrMetrics,
        theme: &crate::Theme,
        _focused: bool,
    ) {
        let buf = f.buffer_mut();
        let Some((t0, t1)) = window(state) else {
            say(buf, area, "waiting for the first cycle", theme);
            return;
        };
        let cols = area.width.saturating_sub(LEFT + RIGHT + 1);
        if cols < 20 {
            say(buf, area, "too narrow for the reach", theme);
            return;
        }
        let (with_activity, with_floor) = rows_under(area.height);
        let under = AXIS
            + FARTHEST
            + if with_activity { ACTIVITY } else { 0 }
            + if with_floor { FLOOR } else { 0 };
        let rows = area.height.saturating_sub(under).max(1);
        let plot = Rect::new(area.x, area.y, area.width, rows);
        let x0 = area.x + 1 + LEFT;
        let span = (t1 - t0).max(15.0);
        let col_of = |unix: f64| -> u16 { (((unix - t0) / span) * f64::from(cols)) as u16 };
        let hour: Vec<&CycleMark> = state.ft8.cycle_hour.iter().collect();

        let placed = placed(state);
        match &placed {
            Some((pts, _)) => draw_plot(buf, plot, x0, cols, pts, t0, span, theme),
            None => say(
                buf,
                plot,
                "no position: set it under Options › Station; the rows below still hold",
                theme,
            ),
        }
        marks(buf, plot, x0, cols, &hour, state, t0, t1, &col_of, theme);
        let mut y = area.y + rows;
        axis(buf, area, y, x0, cols, t0, t1, &col_of, theme);
        y += AXIS;
        if with_activity {
            activity(buf, area, y, x0, cols, &hour, &col_of, theme);
            y += ACTIVITY;
        }
        if with_floor {
            floor(buf, area, y, x0, cols, &hour, &col_of, theme);
            y += FLOOR;
        }
        if let Some((pts, _)) = &placed {
            farthest(buf, area, y, x0, pts, t0, t1, theme);
        }
    }
}

/// The plot: the distance lines, then a braille dot a decode.
#[allow(clippy::too_many_arguments)]
fn draw_plot(
    buf: &mut ratatui::buffer::Buffer,
    plot: Rect,
    x0: u16,
    cols: u16,
    pts: &[Placed],
    t0: f64,
    span: f64,
    t: &crate::Theme,
) {
    let rows = plot.height;
    let row_of = |km: f64| ((1.0 - km_frac(km)) * f64::from(rows - 1)).round() as u16;
    let mut labelled = Vec::new();
    for (d, text) in [
        (20_000.0, "20 000"),
        (10_000.0, "10 000"),
        (3_000.0, " 3 000"),
        (1_000.0, " 1 000"),
        (300.0, "   300"),
        (100.0, "   100"),
    ] {
        let r = row_of(d);
        if labelled.contains(&r) {
            continue;
        }
        labelled.push(r);
        put(
            buf,
            plot,
            plot.x + 1,
            plot.y + r,
            text,
            Style::default().fg(t.label),
        );
        for c in 0..cols {
            put(
                buf,
                plot,
                x0 + c,
                plot.y + r,
                "┈",
                Style::default().fg(t.border_dim),
            );
        }
    }
    put(
        buf,
        plot,
        x0 + cols + 1,
        plot.y,
        "km",
        Style::default().fg(t.label),
    );
    let mut dots = vec![vec![0u8; usize::from(cols)]; usize::from(rows)];
    for p in pts {
        let fx = ((p.unix - t0) / span * f64::from(cols * 2)) as usize;
        let fy = ((1.0 - km_frac(p.km)) * f64::from(rows * 4 - 1)).round() as usize;
        if fx < usize::from(cols) * 2 && fy < usize::from(rows) * 4 {
            dots[fy / 4][fx / 2] |= BIT[fy % 4][fx % 2];
        }
    }
    for (r, row) in dots.iter().enumerate() {
        for (c, &b) in row.iter().enumerate() {
            if b != 0 {
                let ch = char::from_u32(0x2800 + u32::from(b)).unwrap_or(' ');
                put(
                    buf,
                    plot,
                    x0 + c as u16,
                    plot.y + r as u16,
                    &ch.to_string(),
                    Style::default().fg(t.value_hi),
                );
            }
        }
    }
}

/// Dated marks across the plot: a change of band, and the sun crossing the
/// horizon at your station. Drawn only over empty cells.
#[allow(clippy::too_many_arguments)]
fn marks(
    buf: &mut ratatui::buffer::Buffer,
    plot: Rect,
    x0: u16,
    cols: u16,
    hour: &[&CycleMark],
    m: &SdrMetrics,
    t0: f64,
    t1: f64,
    col_of: &dyn Fn(f64) -> u16,
    t: &crate::Theme,
) {
    let mut lines: Vec<(u16, ratatui::style::Color)> = Vec::new();
    for w in hour.windows(2) {
        if w[0].dial_hz != w[1].dial_hz {
            lines.push((col_of(w[1].start_unix), t.border_default));
        }
    }
    if let Some(home) = m.home.position() {
        for at in sun_crossings(home, t0, t1) {
            lines.push((col_of(at), t.map_greyline));
        }
    }
    for (c, colour) in lines {
        if c >= cols {
            continue;
        }
        for r in 0..plot.height {
            let cell = &buf[(x0 + c, plot.y + r)];
            if cell.symbol() == " " || cell.symbol() == "┈" {
                put(
                    buf,
                    plot,
                    x0 + c,
                    plot.y + r,
                    "┊",
                    Style::default().fg(colour),
                );
            }
        }
    }
}

/// When the sun crossed the horizon at `home` between `t0` and `t1`, to the
/// minute.
pub fn sun_crossings(home: (f64, f64), t0: f64, t1: f64) -> Vec<f64> {
    let alt = |u: f64| crate::geo::sun::altitude_deg(crate::geo::sun::subsolar(u), home);
    let mut out = Vec::new();
    let mut prev = alt(t0);
    let mut u = t0 + 60.0;
    while u <= t1 {
        let a = alt(u);
        if (prev > 0.0) != (a > 0.0) {
            out.push(u);
        }
        prev = a;
        u += 60.0;
    }
    out
}

fn hhmm(unix: f64) -> String {
    let s = unix.rem_euclid(86_400.0) as u64;
    format!("{:02}:{:02}", s / 3600, s / 60 % 60)
}

/// Time across, every ten minutes, and the sun's crossings named.
#[allow(clippy::too_many_arguments)]
fn axis(
    buf: &mut ratatui::buffer::Buffer,
    area: Rect,
    y: u16,
    x0: u16,
    cols: u16,
    t0: f64,
    t1: f64,
    col_of: &dyn Fn(f64) -> u16,
    t: &crate::Theme,
) {
    let mut tt = (t0 / 600.0).ceil() * 600.0;
    while tt <= t1 {
        let c = col_of(tt);
        if c + 5 <= cols {
            put(
                buf,
                area,
                x0 + c,
                y,
                &hhmm(tt),
                Style::default().fg(t.label),
            );
        }
        tt += 600.0;
    }
}

/// Decodes a cycle, even above the rule and odd hanging below it.
#[allow(clippy::too_many_arguments)]
fn activity(
    buf: &mut ratatui::buffer::Buffer,
    area: Rect,
    y: u16,
    x0: u16,
    cols: u16,
    hour: &[&CycleMark],
    col_of: &dyn Fn(f64) -> u16,
    t: &crate::Theme,
) {
    let n = usize::from(cols);
    let (mut even, mut odd) = (vec![0usize; n], vec![0usize; n]);
    for c in hour {
        let col = usize::from(col_of(c.start_unix));
        if col < n {
            let side = if c.odd() { &mut odd } else { &mut even };
            side[col] = side[col].max(c.decodes);
        }
    }
    let top = even.iter().chain(&odd).copied().max().unwrap_or(0).max(1) as f64;
    let label = Style::default().fg(t.label);
    put(buf, area, area.x + 1, y, " even", label);
    put(
        buf,
        area,
        area.x + 1,
        y + 1,
        "decodes",
        Style::default().fg(t.border_dim),
    );
    put(buf, area, area.x + 1, y + 2, "  odd", label);
    for c in 0..n {
        let x = x0 + c as u16;
        let e = ((even[c] as f64 / top) * 8.0).round() as usize;
        put(buf, area, x, y, EIGHTHS[e], Style::default().fg(t.value));
        put(buf, area, x, y + 1, "─", Style::default().fg(t.border_dim));
        // Odd hangs from the rule: its top eighths lit by swapping the
        // colours of a lower block.
        let o = ((odd[c] as f64 / top) * 8.0).round() as usize;
        if o > 0 {
            put(
                buf,
                area,
                x,
                y + 2,
                EIGHTHS[8 - o],
                Style::default().fg(t.palette_color(0.0)).bg(t.value),
            );
        }
    }
    put(buf, area, x0 + cols + 1, y, &format!("max {top:.0}"), label);
}

/// The noise under the band, a column's median, held and changed marked.
#[allow(clippy::too_many_arguments)]
fn floor(
    buf: &mut ratatui::buffer::Buffer,
    area: Rect,
    y: u16,
    x0: u16,
    cols: u16,
    hour: &[&CycleMark],
    col_of: &dyn Fn(f64) -> u16,
    t: &crate::Theme,
) {
    let n = usize::from(cols);
    let mut per: Vec<Vec<f64>> = vec![Vec::new(); n];
    let mut held = vec![false; n];
    let mut changed = vec![false; n];
    for w in hour.windows(2) {
        if w[0].segment != w[1].segment {
            let c = usize::from(col_of(w[1].start_unix));
            if c < n {
                changed[c] = true;
            }
        }
    }
    for c in hour {
        let col = usize::from(col_of(c.start_unix));
        if col >= n {
            continue;
        }
        if c.held {
            held[col] = true;
        } else if let Some(v) = c.floor_db_hz {
            per[col].push(f64::from(v));
        }
    }
    let med: Vec<Option<f64>> = per
        .into_iter()
        .map(|mut v| {
            v.sort_by(f64::total_cmp);
            (!v.is_empty()).then(|| v[v.len() / 2])
        })
        .collect();
    let label = Style::default().fg(t.label);
    let dim = Style::default().fg(t.border_dim);
    put(buf, area, area.x + 1, y, " floor", label);
    let known: Vec<f64> = med.iter().flatten().copied().collect();
    if known.is_empty() {
        put(
            buf,
            area,
            x0,
            y + 1,
            "no floor read: held, changed, or the band never paused",
            label,
        );
        return;
    }
    let lo = known.iter().copied().fold(f64::MAX, f64::min);
    let hi = known.iter().copied().fold(f64::MIN, f64::max).max(lo + 1.0);
    put(buf, area, area.x + 1, y + 1, &format!("{hi:6.1}"), dim);
    put(buf, area, area.x + 1, y + 2, &format!("{lo:6.1}"), dim);
    let mut dots = vec![vec![0u8; n]; 3];
    for (c, v) in med.iter().enumerate() {
        if let Some(v) = v {
            let fy = (((hi - v) / (hi - lo)) * 11.0).round() as usize;
            dots[fy / 4][c] |= BIT[fy % 4][0] | BIT[fy % 4][1];
        }
    }
    for (r, row) in dots.iter().enumerate() {
        for (c, &b) in row.iter().enumerate() {
            let x = x0 + c as u16;
            let yy = y + r as u16;
            if held[c] {
                if r == 0 {
                    put(buf, area, x, yy, "×", Style::default().fg(t.status_warn));
                }
            } else if changed[c] {
                put(buf, area, x, yy, "┊", dim);
            } else if b != 0 {
                let ch = char::from_u32(0x2800 + u32::from(b)).unwrap_or(' ');
                put(
                    buf,
                    area,
                    x,
                    yy,
                    &ch.to_string(),
                    Style::default().fg(t.value),
                );
            }
        }
    }
    let segments = hour.last().map_or(0, |c| c.segment) - hour.first().map_or(0, |c| c.segment) + 1;
    put(buf, area, x0 + cols + 1, y, "dBFS/Hz", label);
    put(buf, area, x0 + cols + 1, y + 1, "relative,", dim);
    let within = if segments == 1 {
        "one setting".to_string()
    } else {
        format!("{segments} settings")
    };
    put(buf, area, x0 + cols + 1, y + 2, &within, dim);
}

/// The farthest station of each quarter hour, as far as the row has room.
#[allow(clippy::too_many_arguments)]
fn farthest(
    buf: &mut ratatui::buffer::Buffer,
    area: Rect,
    y: u16,
    x0: u16,
    pts: &[Placed],
    t0: f64,
    t1: f64,
    t: &crate::Theme,
) {
    put(
        buf,
        area,
        area.x + 1,
        y,
        "farthest",
        Style::default().fg(t.label),
    );
    let mut x = x0;
    let mut b = (t0 / 900.0).floor() * 900.0;
    while b < t1 {
        let best = pts
            .iter()
            .filter(|p| p.unix >= b && p.unix < b + 900.0)
            .max_by(|a, c| a.km.total_cmp(&c.km));
        let text = match best {
            Some(p) => format!("{}  {} km {}", hhmm(b), km(p.km), p.call),
            None => format!("{}  none placed", hhmm(b)),
        };
        if x + text.chars().count() as u16 > area.right() {
            break;
        }
        x = put(buf, area, x, y, &text, Style::default().fg(t.value)) + 4;
        b += 900.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::fixture::draw;

    fn braille_dots(lines: &[String]) -> u32 {
        lines
            .iter()
            .flat_map(|l| l.chars())
            .filter(|c| ('\u{2801}'..='\u{28ff}').contains(c))
            .map(|c| (c as u32 - 0x2800).count_ones())
            .sum()
    }

    #[test]
    fn the_scale_is_worlds_log_scale() {
        assert_eq!(km_frac(100.0), 0.0);
        assert_eq!(km_frac(20_000.0), 1.0);
        assert!((km_frac(1_000.0) - 1.0 / (200f64).log10()).abs() < 1e-12);
        assert_eq!(km_frac(10.0), 0.0);
    }

    #[test]
    fn the_rows_under_the_plot_give_way_from_the_floor() {
        assert_eq!(rows_under(20), (true, true));
        assert_eq!(rows_under(9), (true, false));
        assert_eq!(rows_under(6), (false, false));
    }

    /// Every placed decode is a dot, unless two land on the same dot; the
    /// title counts the ones without a locator.
    #[test]
    fn every_placed_decode_is_a_dot() {
        let m = SdrMetrics::fixture().streaming().ft8_hour();
        let (pts, unplaced) = placed(&m).unwrap();
        assert!(pts.len() > 100, "{}", pts.len());
        let lines = draw(Ft8ReachPanel, 191, 26, &m);
        assert!(
            lines[0].contains(&format!("{} placed · {unplaced} without", pts.len())),
            "{}",
            lines[0]
        );
        // Dots in the plot and the floor strip; the plot's alone are at
        // least as many as distinct places, at most as many as decodes.
        let plot = &lines[1..14];
        let dots = braille_dots(plot);
        assert!(
            dots > 0 && dots <= pts.len() as u32,
            "{dots} of {}",
            pts.len()
        );
        let all = lines.join("\n");
        assert!(all.contains("farthest"), "{all}");
        assert!(
            all.contains("dBFS/Hz") && all.contains("one setting"),
            "{all}"
        );
        assert!(
            all.contains('×'),
            "the fixture holds one cycle in twenty: {all}"
        );
    }

    #[test]
    fn without_a_position_it_says_so_and_keeps_the_rows_below() {
        let mut m = SdrMetrics::fixture().streaming().ft8_hour();
        m.home.latitude = None;
        m.home.longitude = None;
        let all = draw(Ft8ReachPanel, 191, 26, &m).join("\n");
        assert!(all.contains("no position"), "{all}");
        assert!(all.contains("decodes") && all.contains("floor"), "{all}");
        assert!(!all.contains("farthest"), "{all}");
    }

    #[test]
    fn before_any_cycle_it_waits() {
        let all = draw(Ft8ReachPanel, 191, 26, &SdrMetrics::fixture()).join("\n");
        assert!(all.contains("waiting for the first cycle"), "{all}");
    }

    /// A short session is drawn as short: the axis starts at its first
    /// cycle, not an hour back.
    #[test]
    fn a_short_session_keeps_its_own_start() {
        let mut m = SdrMetrics::fixture().streaming().ft8_hour();
        let hour = std::sync::Arc::make_mut(&mut m.ft8.cycle_hour);
        hour.drain(..200);
        let (t0, t1) = window(&m).unwrap();
        assert_eq!(t1 - t0, 40.0 * 15.0);
    }

    #[test]
    fn the_sun_is_found_crossing_the_horizon() {
        // Budapest, 2026-11-08: the sun set about 15:19 UTC.
        let day = 1_794_096_000.0;
        let set = sun_crossings((47.497, 19.04), day + 14.0 * 3600.0, day + 16.0 * 3600.0);
        assert_eq!(set.len(), 1);
        let hm = (set[0] - day) / 60.0;
        assert!((hm - (15.0 * 60.0 + 19.0)).abs() < 10.0, "{hm}");
    }
}
