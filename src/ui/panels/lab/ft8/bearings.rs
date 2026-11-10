// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Which way the band is open: the hour's placed stations in sixteen
//! directions from your station (short path), each a bar of its station
//! count with the median of their best SNR beside it, in two columns of
//! eight so the sixteen fit ten rows.
//!
//! The direction with the most stations is named in the title, with how far
//! they are, but only from [`NAME_FROM`] stations up: fewer than that is not
//! a direction, it is a handful. Without your position there are no
//! bearings, and the panel says where to set it.

use std::collections::BTreeMap;

use ratatui::{layout::Rect, style::Style, Frame};

use super::{placed, say, Placed, NARROW_BODY};
use crate::state::SdrMetrics;
use crate::ui::panel::{Panel, PanelChrome, Staleness};
use crate::ui::panels::radio::ft8::{km, put, signed};

pub struct Ft8BearingsPanel;

/// The sixteen directions, clockwise from north.
pub const SECTORS: [&str; 16] = [
    "N", "NNE", "NE", "ENE", "E", "ESE", "SE", "SSE", "S", "SSW", "SW", "WSW", "W", "WNW", "NW",
    "NNW",
];

/// Stations placed before a direction is named.
pub const NAME_FROM: usize = 20;

/// The sector a bearing falls in: each 22.5 degrees wide, centred on its
/// direction.
pub fn sector(bearing: f64) -> usize {
    (((bearing + 11.25).rem_euclid(360.0)) / 22.5) as usize % 16
}

/// Per sector: its stations and the median of their best SNR.
pub fn sectors(pts: &[Placed]) -> [(usize, Option<f64>); 16] {
    let mut best: Vec<BTreeMap<&str, f64>> = vec![BTreeMap::new(); 16];
    for p in pts {
        let e = best[sector(p.bearing)].entry(&p.call).or_insert(f64::MIN);
        *e = e.max(p.snr);
    }
    std::array::from_fn(|k| {
        let mut v: Vec<f64> = best[k].values().copied().collect();
        v.sort_by(f64::total_cmp);
        (v.len(), (!v.is_empty()).then(|| v[v.len() / 2]))
    })
}

/// The open direction in words, or how many stations there are when too
/// few to name one.
pub fn open_words(pts: &[Placed]) -> String {
    let s = sectors(pts);
    let n: usize = s.iter().map(|x| x.0).sum();
    if n < NAME_FROM {
        return format!("{n} stations placed: too few to name a direction");
    }
    let (k, (count, _)) = s
        .iter()
        .enumerate()
        .max_by_key(|(k, x)| (x.0, std::cmp::Reverse(*k)))
        .unwrap_or((0, &(0, None)));
    let far: Vec<f64> = pts
        .iter()
        .filter(|p| sector(p.bearing) == k)
        .map(|p| p.km)
        .collect();
    let lo = far.iter().copied().fold(f64::MAX, f64::min);
    let hi = far.iter().copied().fold(f64::MIN, f64::max);
    format!(
        "open most to the {}: {count} stations, {} to {} km",
        SECTORS[k],
        km(lo),
        km(hi)
    )
}

impl Panel for Ft8BearingsPanel {
    fn name(&self) -> &'static str {
        "ft8_bearings"
    }

    fn reads_spectrum(&self) -> bool {
        false
    }

    fn min_size(&self) -> (u16, u16) {
        (40, 10)
    }

    /// The right column gives way on a narrow body, with this panel first
    /// in it.
    fn column_width(&self, body: u16) -> Option<u16> {
        (body < NARROW_BODY).then_some(0)
    }

    fn column_height(&self, _column: u16) -> Option<u16> {
        Some(10)
    }

    fn chrome(&self, state: &SdrMetrics) -> PanelChrome {
        let mut chrome = PanelChrome::new("Bearings").stale_when(Staleness::NotStreaming);
        if let Some((pts, _)) = placed(state) {
            chrome.suffix = Some(format!("  {}", open_words(&pts)));
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
        let Some((pts, _)) = placed(state) else {
            say(
                buf,
                area,
                "no position: set it under Options › Station",
                theme,
            );
            return;
        };
        if pts.is_empty() {
            say(buf, area, "no station placed yet", theme);
            return;
        }
        let s = sectors(&pts);
        let top = s.iter().map(|x| x.0).max().unwrap_or(1).max(1);
        let half = area.width / 2;
        let bar_w = usize::from(half.saturating_sub(17));
        for (k, (n, snr)) in s.iter().enumerate() {
            let (col, row) = (k / 8, k % 8);
            if row as u16 >= area.height {
                continue;
            }
            let x = area.x + 1 + col as u16 * half;
            let y = area.y + row as u16;
            put(
                buf,
                area,
                x,
                y,
                &format!("{:>3}", SECTORS[k]),
                Style::default().fg(theme.label),
            );
            let len = (*n * bar_w).div_ceil(top).min(bar_w);
            let x = put(
                buf,
                area,
                x + 4,
                y,
                &"█".repeat(len),
                Style::default().fg(theme.value),
            );
            if *n > 0 {
                let text = match snr {
                    Some(v) => format!(" {n}  {} dB", signed(*v, 0)),
                    None => format!(" {n}"),
                };
                put(buf, area, x, y, &text, Style::default().fg(theme.label));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::fixture::draw;
    use std::sync::Arc;

    fn at(bearing: f64, call: &str, snr: f64) -> Placed {
        Placed {
            unix: 0.0,
            km: 1_000.0,
            bearing,
            snr,
            call: Arc::from(call),
        }
    }

    #[test]
    fn a_bearing_falls_in_the_sector_centred_on_it() {
        assert_eq!(sector(0.0), 0);
        assert_eq!(sector(11.2), 0);
        assert_eq!(sector(11.3), 1);
        assert_eq!(sector(359.0), 0);
        assert_eq!(sector(292.5), 13);
        assert_eq!(SECTORS[13], "WNW");
    }

    /// A station counts once a sector, at its best SNR; the median is of
    /// the stations' bests.
    #[test]
    fn a_station_counts_once_at_its_best() {
        let pts = [
            at(90.0, "A", -20.0),
            at(90.0, "A", -10.0),
            at(91.0, "B", -4.0),
            at(88.0, "C", -16.0),
        ];
        let s = sectors(&pts);
        assert_eq!(s[4], (3, Some(-10.0)));
        assert_eq!(s[0], (0, None));
    }

    #[test]
    fn a_direction_is_named_from_twenty_stations() {
        let few: Vec<Placed> = (0..19)
            .map(|i| at(270.0, &format!("K{i}"), -10.0))
            .collect();
        assert!(open_words(&few).contains("19 stations placed: too few"));
        let mut many = few.clone();
        many.push(at(271.0, "K99", -10.0));
        assert!(
            open_words(&many).contains("open most to the W: 20 stations"),
            "{}",
            open_words(&many)
        );
    }

    #[test]
    fn it_draws_sixteen_directions_in_two_columns() {
        let m = SdrMetrics::fixture().streaming().ft8_hour();
        let lines = draw(Ft8BearingsPanel, 64, 10, &m);
        for (k, name) in SECTORS.iter().enumerate() {
            let row = &lines[1 + k % 8];
            assert!(
                row.contains(&format!("{name:>3} ")),
                "{name} not on its row: {row}"
            );
        }
        assert!(lines[0].contains("open most to the"), "{}", lines[0]);
    }

    #[test]
    fn without_a_position_it_says_so() {
        let mut m = SdrMetrics::fixture().streaming().ft8_hour();
        m.home.latitude = None;
        let all = draw(Ft8BearingsPanel, 64, 10, &m).join("\n");
        assert!(all.contains("no position"), "{all}");
    }

    #[test]
    fn the_right_column_gives_way_on_a_narrow_body() {
        assert_eq!(Ft8BearingsPanel.column_width(149), Some(0));
        assert_eq!(Ft8BearingsPanel.column_width(189), None);
    }
}
