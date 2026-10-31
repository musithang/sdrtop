// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The map: the world around you, as you hear it.
//!
//! **Azimuthal equidistant about you** (`geo::projection`). Every station
//! sits in the true direction of the great circle that reaches it, so the
//! line from you to it is the path, straight, and its bearing is the one an
//! antenna would turn to. By default the distance is logarithmic, the whole
//! world in the disc with the antipode at its edge and the near ground opened
//! out; `z` steps to linear ranges, where the locator grid is drawn.
//!
//! **The sun colours the land** for the time of the cycle being heard: day,
//! civil twilight (the grey line, in its own ink, never a station's, because
//! the lines to this cycle's stations cross it) and night.
//!
//! **A station is drawn where it said it was** (POLICY.md, rule 2): only one
//! that sent its own locator is on the map; the others are in the list. Its
//! colour says how fresh it is, red if it called you, and the selected one is
//! ringed in white. The freshest and strongest are named first; where names
//! would collide the rest stay dots.
//!
//! **No position, no map.** The world is drawn around where you listen from,
//! and that is never guessed: without one the panel says where to set it.
//!
//! **The land is cached.** Which dots are land depends on the panel's size,
//! the range and your position, not on the frame; it is worked out again only
//! when one of those changes. The light depends on the minute, and is cached
//! for it.

use std::sync::Mutex;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    Frame,
};

use super::{age, hhmm, ink, newest_cycle, sun_time, Age};
use crate::geo::maidenhead;
use crate::geo::projection::{Azimuthal, Scale};
use crate::geo::sun::{self, Light};
use crate::geo::{land, sphere};
use crate::state::{MapRange, SdrMetrics};
use crate::ui::panel::{Panel, PanelChrome, Staleness};
use crate::ui::widgets::charts::lerp_color;
use crate::ui::widgets::dots::Dots;

/// Range rings, km, drawn where they fall inside the map.
const RINGS_KM: [f64; 6] = [500.0, 1000.0, 2500.0, 5000.0, 10_000.0, 15_000.0];

/// The locator grid is drawn on linear ranges up to this, km: beyond it the
/// fields crowd into hatching.
const GRID_MAX_KM: f64 = 4000.0;

// Layer priorities: what matters wins a cell. The rings and the grid sit
// under the land: a cell takes one colour, and with the land drawn whole a
// ring above it cut a dark band through every continent it crossed.
const P_SEA_TWILIGHT: u8 = 1;
const P_GRID: u8 = 1;
const P_RING: u8 = 1;
const P_NIGHT: u8 = 2;
const P_LAND: u8 = 3;
const P_RAY: u8 = 5;

/// What each dot of the canvas is, for one size, range and position.
struct Land {
    key: (u16, u16, MapRange, i64, i64),
    /// Per dot: off the world (beyond the antipode), or its place and
    /// whether it is land, coast or the land's inside.
    places: Vec<Option<(f32, f32)>>,
    land: Vec<bool>,
    coast: Vec<bool>,
    /// The light per dot, and the minute it is for.
    light: Option<(i64, Vec<Light>)>,
}

#[derive(Default)]
pub struct Ft8MapPanel {
    cache: Mutex<Option<Land>>,
}

impl Ft8MapPanel {
    pub fn new() -> Self {
        Self::default()
    }
}

/// The map's geometry on a canvas `w` by `h` dots.
struct Geometry {
    proj: Azimuthal,
    cx: f64,
    cy: f64,
    half: f64,
}

impl Geometry {
    fn dot(&self, x: f64, y: f64) -> (f64, f64) {
        (self.cx + x * self.half, self.cy - y * self.half)
    }

    fn place_dot(&self, place: (f64, f64)) -> (f64, f64) {
        let (x, y) = self.proj.forward(place);
        self.dot(x, y)
    }
}

fn build_land(key: (u16, u16, MapRange, i64, i64), g: &Geometry, w: usize, h: usize) -> Land {
    let mut places = Vec::with_capacity(w * h);
    let mut is_land = Vec::with_capacity(w * h);
    for py in 0..h {
        for px in 0..w {
            let x = (px as f64 + 0.5 - g.cx) / g.half;
            let y = (g.cy - (py as f64 + 0.5)) / g.half;
            match g.proj.inverse(x, y) {
                Some((lat, lon)) => {
                    places.push(Some((lat as f32, lon as f32)));
                    is_land.push(land::on_land(lat, lon));
                }
                None => {
                    places.push(None);
                    is_land.push(false);
                }
            }
        }
    }
    let at = |x: i64, y: i64| {
        x >= 0
            && y >= 0
            && (x as usize) < w
            && (y as usize) < h
            && is_land[y as usize * w + x as usize]
    };
    let coast = (0..w * h)
        .map(|i| {
            let (x, y) = ((i % w) as i64, (i / w) as i64);
            is_land[i] && !(at(x - 1, y) && at(x + 1, y) && at(x, y - 1) && at(x, y + 1))
        })
        .collect();
    Land {
        key,
        places,
        land: is_land,
        coast,
        light: None,
    }
}

impl Panel for Ft8MapPanel {
    fn name(&self) -> &'static str {
        "ft8_map"
    }

    fn reads_spectrum(&self) -> bool {
        false
    }

    fn min_size(&self) -> (u16, u16) {
        (40, 12)
    }

    fn chrome(&self, state: &SdrMetrics) -> PanelChrome {
        let f = &state.ft8;
        let mut suffix = String::new();
        if let Some((lat, lon)) = state.home.position() {
            suffix.push_str(&format!(
                " {}",
                maidenhead::locator(lat, lon, 3).unwrap_or_default()
            ));
            suffix.push_str(&format!(" · {}", f.view.range.label()));
            if let Some(t) = sun_time(state) {
                suffix.push_str(&format!(" · greyline {} UTC", hhmm(t)));
            }
        } else {
            suffix.push_str(" no position");
        }
        PanelChrome::new("Map")
            .stale_when(Staleness::NotStreaming)
            .suffix(suffix)
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
        let Some(home) = state.home.position() else {
            refuse(buf, area, theme);
            return;
        };
        let legend = area.height >= 8;
        let rows = area.height as usize - usize::from(legend);
        let cols = area.width as usize;
        let mut dots = Dots::new(cols, rows);
        let (w, h) = (dots.width(), dots.height());
        let range = state.ft8.view.range;
        let g = Geometry {
            proj: Azimuthal::new(home, range.scale()),
            cx: w as f64 / 2.0,
            cy: h as f64 / 2.0,
            half: h as f64 / 2.0,
        };

        // The land, from the cache or worked out again.
        let key = (
            area.width,
            area.height,
            range,
            (home.0 * 1e4).round() as i64,
            (home.1 * 1e4).round() as i64,
        );
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if cache.as_ref().is_none_or(|c| c.key != key) {
            *cache = Some(build_land(key, &g, w, h));
        }
        let land_cache = cache.as_mut().expect("just built");
        let time = sun_time(state);
        let minute = time.map(|t| (t / 60.0).floor() as i64);
        if let Some((t, m)) = time.zip(minute) {
            if land_cache.light.as_ref().is_none_or(|(lm, _)| *lm != m) {
                let s = sun::subsolar(t);
                let light = land_cache
                    .places
                    .iter()
                    .map(|p| match p {
                        Some((lat, lon)) => {
                            Light::of(sun::altitude_deg(s, (f64::from(*lat), f64::from(*lon))))
                        }
                        None => Light::Night,
                    })
                    .collect();
                land_cache.light = Some((m, light));
            }
        }
        let light = land_cache
            .light
            .as_ref()
            .filter(|(lm, _)| Some(*lm) == minute)
            .map(|(_, l)| l);

        let greyline_sea = lerp_color(theme.map_greyline, theme.map_night, 0.55);
        for i in 0..w * h {
            let Some(_) = land_cache.places[i] else {
                continue;
            };
            let (x, y) = ((i % w) as i64, (i / w) as i64);
            let l = light.map_or(Light::Day, |l| l[i]);
            if land_cache.land[i] {
                // Every dot of the land, and the coast a shade brighter so
                // the outline reads first. A checker of every other dot was
                // tried first: in a terminal font's thin braille the night
                // side of the world all but vanished.
                let (p, c) = match l {
                    Light::Day => (P_LAND, theme.map_land),
                    Light::Twilight => (P_LAND, theme.map_greyline),
                    Light::Night => (P_NIGHT, theme.map_night),
                };
                let c = if land_cache.coast[i] {
                    lerp_color(c, theme.value, 0.4)
                } else {
                    c
                };
                dots.set(x, y, p, c);
            } else if l == Light::Twilight && x % 4 == 0 && y % 4 == 0 {
                dots.set(x, y, P_SEA_TWILIGHT, greyline_sea);
            }
        }
        let lit = light.is_some();
        drop(cache);

        // The edge of the world, where it shows: the antipode's ring.
        let edge_r = g.proj.radius(sphere::ANTIPODE_KM);
        circle(&mut dots, &g, edge_r, P_RING, theme.map_grid, 1);
        let rings: Vec<f64> = match range.scale() {
            Scale::Log => RINGS_KM.to_vec(),
            Scale::Linear { range_km } => RINGS_KM
                .iter()
                .copied()
                .filter(|&r| r < range_km * 1.8)
                .collect(),
        };
        for &r in &rings {
            circle(&mut dots, &g, g.proj.radius(r), P_RING, theme.map_grid, 2);
        }
        let fields = matches!(range.scale(), Scale::Linear { range_km } if range_km <= GRID_MAX_KM);
        if fields {
            grid(&mut dots, &g, theme.map_grid);
        }

        // The stations on the map, and the rays to this cycle's.
        let newest = newest_cycle(&state.ft8);
        let selected = state.ft8.view.selected.as_deref();
        let mut on_map: Vec<(&crate::state::Station, Age, (f64, f64))> = state
            .ft8
            .stations
            .values()
            .filter_map(|s| {
                let place = maidenhead::centre(s.locator.as_deref()?)?;
                Some((s, age(s, newest), g.place_dot(place)))
            })
            .collect();
        // The rays on a canvas of their own, drawn over the map: a cell a
        // ray crosses shows the ray's dots alone, so it reads as one thin
        // line. Drawn into the map's canvas it took the colour of every cell
        // it touched, land dots and all, and became a ragged band.
        let mut rays = Dots::new(cols, rows);
        for (s, a, (px, py)) in &on_map {
            if *a == Age::Now {
                let c = lerp_color(ink(s, *a, theme), theme.map_night, 0.2);
                line(&mut rays, (g.cx, g.cy), (*px, *py), P_RAY, c);
            }
        }
        dots.render(buf, area);
        rays.render(buf, area);

        let put = |buf: &mut Buffer, x: i64, y: i64, text: &str, style: Style| {
            for (k, ch) in text.chars().enumerate() {
                let xx = x + k as i64;
                if xx >= 0 && y >= 0 && (xx as u16) < area.width && (y as usize) < rows {
                    buf.get_mut(area.x + xx as u16, area.y + y as u16)
                        .set_char(ch)
                        .set_style(style);
                }
            }
        };
        let cell = |(px, py): (f64, f64)| ((px / 2.0).floor() as i64, (py / 4.0).floor() as i64);
        let mut used: Vec<(i64, i64, i64)> = Vec::new(); // row, from, to
        let free = |used: &[(i64, i64, i64)], y: i64, a: i64, b: i64| {
            a >= 0
                && b < cols as i64
                && used.iter().all(|&(uy, ua, ub)| uy != y || b < ua || a > ub)
        };

        // Ring labels on the 30 degree bearing, the compass at the edges.
        let dim = Style::default().fg(theme.map_grid);
        for &r in &rings {
            let rr = g.proj.radius(r);
            let (px, py) = g.dot(rr * 30f64.to_radians().sin(), rr * 30f64.to_radians().cos());
            let (x, y) = cell((px, py));
            let t = if r >= 1000.0 {
                format!("{}k", r / 1000.0)
            } else {
                format!("{r}")
            };
            if free(&used, y, x, x + t.len() as i64) {
                put(buf, x, y, &t, dim);
            }
        }
        let compass = Style::default()
            .fg(theme.label)
            .add_modifier(Modifier::BOLD);
        put(buf, cols as i64 / 2, 0, "N", compass);
        put(buf, cols as i64 / 2, rows as i64 - 1, "S", compass);
        put(buf, 0, rows as i64 / 2, "W", compass);
        put(buf, cols as i64 - 1, rows as i64 / 2, "E", compass);
        if fields {
            field_names(buf, &g, cols, rows, theme, &put);
        }

        // Oldest first, so the freshest is drawn on top.
        on_map.sort_by_key(|(s, a, _)| {
            (
                std::cmp::Reverse(*a as u8),
                s.called_you,
                Some(s.call.as_str()) == selected,
            )
        });
        let (hx, hy) = cell((g.cx, g.cy));
        for (s, a, p) in &on_map {
            let (x, y) = cell(*p);
            if x <= 0 || y <= 0 || x >= cols as i64 - 1 || y >= rows as i64 - 1 {
                continue;
            }
            let is_sel = Some(s.call.as_str()) == selected;
            let (ch, color) = if is_sel {
                ("\u{25c9}", theme.border_focused)
            } else if *a == Age::Old {
                ("\u{2022}", ink(s, *a, theme))
            } else {
                ("\u{25cf}", ink(s, *a, theme))
            };
            put(
                buf,
                x,
                y,
                ch,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            );
            used.push((y, x, x));
        }
        put(
            buf,
            hx,
            hy,
            "\u{25c6}",
            Style::default()
                .fg(theme.map_home)
                .add_modifier(Modifier::BOLD),
        );
        used.push((hy, hx, hx));

        // Names: the selected, then whoever called you, then the freshest,
        // then the strongest; a name that would cover another stays a dot.
        let mut named: Vec<&(&crate::state::Station, Age, (f64, f64))> = on_map.iter().collect();
        named.sort_by(|a, b| {
            let key = |e: &(&crate::state::Station, Age, (f64, f64))| {
                let snr = e.0.snr.back().map_or(f64::MIN, |x| x.1);
                (
                    Some(e.0.call.as_str()) != selected,
                    !e.0.called_you,
                    e.1 as u8,
                    std::cmp::Reverse((snr * 10.0) as i64),
                )
            };
            key(a).cmp(&key(b))
        });
        for (s, a, p) in named {
            let (x, y) = cell(*p);
            if x <= 0 || y <= 0 || x >= cols as i64 - 1 || y >= rows as i64 - 1 {
                continue;
            }
            let is_sel = Some(s.call.as_str()) == selected;
            let n = s.call.chars().count() as i64;
            for tx in [x + 2, x - n - 1] {
                if free(&used, y, tx - 1, tx + n) {
                    let mut style = Style::default().fg(if is_sel {
                        theme.border_focused
                    } else {
                        ink(s, *a, theme)
                    });
                    if is_sel || s.called_you {
                        style = style.add_modifier(Modifier::BOLD);
                    }
                    put(buf, tx, y, &s.call, style);
                    used.push((y, tx - 1, tx + n));
                    break;
                }
            }
        }
        let loc = maidenhead::locator(home.0, home.1, 2).unwrap_or_default();
        if free(&used, hy, hx + 1, hx + 2 + loc.len() as i64) {
            put(buf, hx + 2, hy, &loc, Style::default().fg(theme.map_home));
        }

        if legend {
            draw_legend(buf, area, theme, lit);
        }
    }
}

/// A circle of radius `r` (in edge units) about the middle, every `every`th
/// dot.
fn circle(dots: &mut Dots, g: &Geometry, r: f64, p: u8, c: Color, every: usize) {
    let n = ((std::f64::consts::TAU * r * g.half) * 1.5).max(8.0) as usize;
    for k in (0..n).step_by(every) {
        let t = k as f64 / n as f64 * std::f64::consts::TAU;
        let (x, y) = g.dot(r * t.sin(), r * t.cos());
        dots.set(x as i64, y as i64, p, c);
    }
}

/// A straight line between two dot positions, one dot per step along its
/// longer axis so it never thickens, leaving two dots clear at each end.
fn line(dots: &mut Dots, a: (f64, f64), b: (f64, f64), p: u8, c: Color) {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let n = dx.abs().max(dy.abs()).round() as usize;
    for k in 2..n.saturating_sub(1) {
        let t = k as f64 / n as f64;
        dots.set(
            (a.0 + dx * t).floor() as i64,
            (a.1 + dy * t).floor() as i64,
            p,
            c,
        );
    }
}

/// The Maidenhead field boundaries: every 20 degrees of longitude and 10 of
/// latitude, sampled finely and dotted.
fn grid(dots: &mut Dots, g: &Geometry, c: Color) {
    let mut plot = |lat: f64, lon: f64, k: usize| {
        if k.is_multiple_of(3) {
            let (x, y) = g.place_dot((lat, lon));
            if x.is_finite() && y.is_finite() {
                dots.set(x as i64, y as i64, P_GRID, c);
            }
        }
    };
    for lon in (-180..180).step_by(20) {
        for (k, i) in (-356..=356).enumerate() {
            plot(f64::from(i) / 4.0, f64::from(lon), k);
        }
    }
    for lat in (-80..90).step_by(10) {
        for (k, i) in (-720..720).enumerate() {
            plot(f64::from(lat), f64::from(i) / 4.0, k);
        }
    }
}

/// Field names at their centres, where the cell is free of anything but
/// the map's own dots.
fn field_names(
    buf: &mut Buffer,
    g: &Geometry,
    cols: usize,
    rows: usize,
    theme: &crate::Theme,
    put: &dyn Fn(&mut Buffer, i64, i64, &str, Style),
) {
    let style = Style::default().fg(lerp_color(theme.label, theme.map_grid, 0.4));
    for fl in 0..18u8 {
        for fa in 0..18u8 {
            let (lat, lon) = (
                -90.0 + f64::from(fa) * 10.0 + 5.0,
                -180.0 + f64::from(fl) * 20.0 + 10.0,
            );
            let (px, py) = g.place_dot((lat, lon));
            let (x, y) = ((px / 2.0) as i64, (py / 4.0) as i64);
            if x >= 1 && y >= 1 && x + 2 < cols as i64 && y + 1 < rows as i64 {
                let name: String = [(b'A' + fl) as char, (b'A' + fa) as char].iter().collect();
                put(buf, x, y, &name, style);
            }
        }
    }
}

fn draw_legend(buf: &mut Buffer, area: Rect, theme: &crate::Theme, lit: bool) {
    let y = area.bottom() - 1;
    let mut x = area.x + 1;
    let mut parts: Vec<(&str, Color, &str)> = vec![
        ("\u{25c6}", theme.map_home, " you  "),
        ("\u{25cf}", theme.value_hi, " this cycle  "),
        ("\u{25cf}", theme.value, " last 45 s  "),
        ("\u{2022}", theme.label, " older  "),
        ("\u{25cf}", theme.status_crit, " called you  "),
    ];
    if lit {
        parts.extend([
            ("\u{28ff}", theme.map_land, " day  "),
            ("\u{28ff}", theme.map_greyline, " greyline  "),
            ("\u{28ff}", theme.map_night, " night"),
        ]);
    }
    for (mark, color, text) in parts {
        let need = 1 + text.chars().count() as u16;
        if x + need > area.right() {
            break;
        }
        buf.get_mut(x, y)
            .set_char(mark.chars().next().unwrap())
            .set_fg(color);
        for (k, ch) in text.chars().enumerate() {
            buf.get_mut(x + 1 + k as u16, y)
                .set_char(ch)
                .set_fg(theme.label);
        }
        x += need;
    }
}

/// Without a position there is no map, and the panel says where to set one.
fn refuse(buf: &mut Buffer, area: Rect, theme: &crate::Theme) {
    let lines: [(&str, Color); 6] = [
        ("No position set, so no map.", theme.value),
        ("", theme.label),
        (
            "The map is drawn around where you listen from,",
            theme.label,
        ),
        (
            "and sdrtop will not guess it. Set it once under",
            theme.label,
        ),
        (
            "Options \u{203a} Station: latitude and longitude.",
            theme.value_hi,
        ),
        (
            "Decodes, the band and the clock work without it.",
            theme.label,
        ),
    ];
    let top = area.y + area.height.saturating_sub(lines.len() as u16) / 2;
    for (k, (text, color)) in lines.iter().enumerate() {
        let y = top + k as u16;
        if y >= area.bottom() {
            break;
        }
        let w = text.chars().count() as u16;
        let x = area.x + area.width.saturating_sub(w) / 2;
        for (i, ch) in text.chars().enumerate() {
            let xx = x + i as u16;
            if xx < area.right() {
                buf.get_mut(xx, y).set_char(ch).set_fg(*color);
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::state::fixture::draw;
    use crate::state::{CycleEntry, CycleOutcome, Station};
    use std::collections::VecDeque;
    use std::time::Instant;

    /// 2026-10-30 13:15:00 UTC, the sheet's moment.
    pub const NOW: f64 = 1_793_366_100.0;

    /// The sheet's evening on 20 m: stations at their own locators, heard
    /// at various ages, one with no locator, one that called you.
    pub fn evening() -> SdrMetrics {
        let mut m = SdrMetrics::fixture().streaming();
        m.home.latitude = Some(47.497);
        m.home.longitude = Some(19.04);
        m.home.callsign = Some("HA5ZZT".into());
        let at = Instant::now();
        let st = |call: &str, loc: Option<&str>, ago: f64, snr: f64, you: bool| Station {
            call: call.into(),
            locator: loc.map(str::to_string),
            last_unix: NOW - ago,
            last_at: at,
            heard: 1,
            snr: VecDeque::from([(NOW - ago, snr)]),
            called_you: you,
        };
        for s in [
            st("JA1VQD", Some("PM95"), 0.0, -21.0, true),
            st("EA8ZQ", Some("IL18"), 0.0, -9.0, false),
            st("W2XK", Some("FN20"), 0.0, -17.0, false),
            st("VK3FTX", Some("QF22"), 0.0, -22.0, false),
            st("DL7ANR", Some("JO62"), 0.0, -3.0, false),
            st("OK1HZD", Some("JO70"), 0.0, 2.0, false),
            st("PY2WQS", Some("GG66"), 15.0, -15.0, false),
            st("ZS6DWR", Some("KG44"), 30.0, -18.0, false),
            st("RA0FWL", Some("PN46"), 600.0, -21.0, false),
            st("YO3GCI", None, 0.0, -1.0, false),
        ] {
            m.ft8.stations.insert(s.call.clone(), s);
        }
        m.ft8.cycles.push_back(CycleEntry {
            start_unix: NOW,
            dial_hz: 14_074_000,
            outcome: CycleOutcome::Decoded,
            decodes: 16,
            candidates: 120,
            malformed: 0,
            seconds: 1.8,
            band: Vec::new(),
        });
        m.ft8.hearing = Some(NOW + 15.0);
        m
    }

    fn text(lines: &[String]) -> String {
        lines.join("\n")
    }

    #[test]
    fn without_a_position_the_map_says_where_to_set_one() {
        let mut m = evening();
        m.home.latitude = None;
        let t = text(&draw(Ft8MapPanel::new(), 100, 24, &m));
        assert!(t.contains("No position set, so no map."), "{t}");
        assert!(t.contains("Options \u{203a} Station"), "{t}");
        assert!(t.contains("no position"), "{t}");
        assert!(!t.contains("JA1VQD"), "{t}");
    }

    #[test]
    fn stations_are_drawn_where_they_said_they_were_and_only_those() {
        let t = text(&draw(Ft8MapPanel::new(), 112, 37, &evening()));
        // The frame names you, the scale and the sun's time.
        assert!(t.contains("JN97ml"), "{t}");
        assert!(t.contains("log distance"), "{t}");
        assert!(t.contains("greyline 13:15 UTC"), "{t}");
        // You, and far stations by name.
        assert!(t.contains('\u{25c6}'), "{t}");
        for call in ["JA1VQD", "VK3FTX", "W2XK", "PY2WQS"] {
            assert!(t.contains(call), "{call} missing:\n{t}");
        }
        // A call heard without a locator is not on the map.
        assert!(!t.contains("YO3GCI"), "{t}");
        // The legend, with the sun's three.
        assert!(t.contains("greyline") && t.contains("night"), "{t}");
    }

    #[test]
    fn the_selected_station_is_ringed() {
        let mut m = evening();
        m.ft8.view.selected = Some("VK3FTX".into());
        let t = text(&draw(Ft8MapPanel::new(), 112, 37, &m));
        assert!(t.contains('\u{25c9}'), "{t}");
    }

    #[test]
    fn a_linear_range_draws_the_locator_fields() {
        let mut m = evening();
        m.ft8.view.range = MapRange::Km3000;
        let t = text(&draw(Ft8MapPanel::new(), 112, 37, &m));
        assert!(t.contains("3 000 km"), "{t}");
        // Budapest's own field and its neighbours.
        assert!(
            t.contains("JN") && t.contains("KN") && t.contains("JO"),
            "{t}"
        );
        // Tokyo is off a 3000 km map.
        assert!(!t.contains("JA1VQD"), "{t}");
    }

    #[test]
    fn a_ray_is_one_thin_unbroken_line() {
        // Flat along a row: every cell between the ends lit, in that row
        // alone, the ends left clear.
        let mut d = Dots::new(24, 3);
        line(&mut d, (0.0, 5.0), (40.0, 5.0), P_RAY, Color::Red);
        assert!(!d.lit(0, 1) && !d.lit(20, 1));
        assert!((2..19).all(|c| d.lit(c, 1)));
        assert!((0..24).all(|c| !d.lit(c, 0) && !d.lit(c, 2)));
    }

    #[test]
    fn before_the_stream_has_a_time_there_is_no_sun() {
        let mut m = evening();
        m.ft8.hearing = None;
        m.ft8.cycles.clear();
        let t = text(&draw(Ft8MapPanel::new(), 112, 37, &m));
        assert!(!t.contains("greyline"), "{t}");
    }

    /// What the map costs: the land worked out (a resize, a new range or
    /// position), a frame from the cache, and a frame at a new minute.
    /// `cargo test --release time_the_map -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn time_the_map() {
        let m = evening();
        let panel = Ft8MapPanel::new();
        let backend = ratatui::backend::TestBackend::new(112, 37);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        let theme = crate::Theme::sdr();
        let mut frame = |m: &SdrMetrics| {
            let t = Instant::now();
            term.draw(|f| panel.render(f, f.size(), m, &theme, false))
                .unwrap();
            t.elapsed()
        };
        println!("first frame (land and light): {:?}", frame(&m));
        let mut cached = std::time::Duration::ZERO;
        for _ in 0..20 {
            cached += frame(&m);
        }
        println!("cached frame: {:?}", cached / 20);
        let mut later = m.clone();
        later.ft8.hearing = Some(NOW + 75.0);
        println!("new minute (light only): {:?}", frame(&later));
    }

    /// Writes the map's real cells, with their colours, as HTML to the path
    /// in `MAP_HTML`, for looking at beside the sheet.
    #[test]
    #[ignore]
    fn render_the_map_to_html() {
        let Ok(path) = std::env::var("MAP_HTML") else {
            return;
        };
        let mut out = String::from("<div class=t>");
        let night = {
            let mut m = evening();
            m.home.latitude = Some(46.2917);
            m.home.longitude = Some(18.5254);
            m.ft8.hearing = Some(1_793_393_280.0); // 2026-10-30 20:48 UTC
            m
        };
        for (title, m) in [
            ("bonyhad night", night),
            ("log", evening()),
            ("3000", {
                let mut m = evening();
                m.ft8.view.range = MapRange::Km3000;
                m.ft8.view.selected = Some("DL7ANR".into());
                m
            }),
        ] {
            let mut registry = crate::ui::PanelRegistry::new();
            registry.register(Ft8MapPanel::new());
            let theme = crate::Theme::sdr();
            let mut t =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(112, 37)).unwrap();
            t.draw(|f| registry.render_panel("ft8_map", f, f.size(), &m, &theme, false))
                .unwrap();
            let buf = t.backend().buffer().clone();
            out.push_str(&format!("<h2>{title}</h2><div class=g>"));
            for y in 0..37 {
                out.push_str("<div class=r>");
                for x in 0..112 {
                    let c = buf.get(x, y);
                    let fg = match c.fg {
                        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
                        _ => "#c3d2dc".into(),
                    };
                    let sym = c.symbol().replace('&', "&amp;").replace('<', "&lt;");
                    out.push_str(&format!("<i style=\"color:{fg}\">{sym}</i>"));
                }
                out.push_str("</div>");
            }
            out.push_str("</div>");
        }
        out.push_str("</div>");
        std::fs::write(path, out).unwrap();
    }

    /// Prints the map at the reference size, for looking at.
    #[test]
    #[ignore]
    fn show_the_map() {
        for line in draw(Ft8MapPanel::new(), 112, 37, &evening()) {
            println!("{line}");
        }
    }
}
