// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Wi-Fi's panels. Beside `net::ble` and `net::bt`, never inside either: the
//! Wi-Fi bench is a menu of its own and shares no panel with Bluetooth's.

pub mod frames;
pub mod least_busy;
pub mod standin;
pub mod survey;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
};

use crate::state::{NetMode, SdrMetrics};

/// What the view cannot measure, and why, and the key that helps.
struct Silence {
    head: &'static str,
    why: String,
    keys: Vec<(&'static str, bool)>,
}

fn silence(state: &SdrMetrics) -> Option<Silence> {
    if !state.radio.hw_streaming {
        return Some(Silence {
            head: "NOT RECEIVING",
            why: "The radio is open and idle: no samples, so no channel can be visited.".into(),
            keys: vec![("Space", true), (" starts receiving.", false)],
        });
    }
    let why = state.net.wifi.refused.clone()?;
    if crate::signal::net::wifi::rate_refusal(state.radio.config_sample_rate).is_some() {
        return Some(Silence {
            head: "WRONG SAMPLE RATE",
            why,
            keys: vec![("s", true), (", then ", false), ("20", true), (".", false)],
        });
    }
    Some(Silence {
        head: "NOT ON A CHANNEL CENTRE",
        why,
        keys: vec![("← →", true), (" step to a channel centre.", false)],
    })
}

/// The channel the radio is locked on, when it is.
fn locked(state: &SdrMetrics) -> Option<u8> {
    (state.net.mode == NetMode::Lock)
        .then(|| crate::signal::net::band::wifi_channel(state.radio.frequency))
        .flatten()
}

/// Write `s` from `x` on row `y`, clipped to `area`; where it ended.
fn put(buf: &mut Buffer, x: u16, y: u16, s: &str, style: Style, area: Rect) -> u16 {
    let mut x = x;
    for ch in s.chars() {
        if x >= area.right() || y >= area.bottom() {
            break;
        }
        buf.get_mut(x, y).set_char(ch).set_style(style);
        x += 1;
    }
    x
}

/// What cannot be measured, in a plate in the middle of `area`.
fn plate(buf: &mut Buffer, area: Rect, s: &Silence, theme: &crate::Theme) {
    let edge = Style::default().fg(theme.status_warn);
    let inner = s.why.chars().count().max(40) as u16 + 6;
    let w = inner.min(area.width.saturating_sub(2));
    let h = 6u16.min(area.height);
    let x0 = area.x + (area.width.saturating_sub(w)) / 2;
    let y0 = area.y + (area.height.saturating_sub(h)) / 2;
    for y in y0..y0 + h {
        for x in x0..x0 + w {
            let c = match (x == x0, x == x0 + w - 1, y == y0, y == y0 + h - 1) {
                (true, _, true, _) => '╭',
                (_, true, true, _) => '╮',
                (true, _, _, true) => '╰',
                (_, true, _, true) => '╯',
                (_, _, true, _) | (_, _, _, true) => '─',
                (true, _, _, _) | (_, true, _, _) => '│',
                _ => ' ',
            };
            buf.get_mut(x, y).set_char(c).set_style(edge);
        }
    }
    let inside = Rect::new(x0 + 3, y0, w.saturating_sub(4), h);
    put(
        buf,
        x0 + 3,
        y0 + 1,
        s.head,
        edge.add_modifier(Modifier::BOLD),
        inside,
    );
    put(
        buf,
        x0 + 3,
        y0 + 2,
        &s.why,
        Style::default().fg(theme.value),
        inside,
    );
    let mut x = x0 + 3;
    for (part, key) in &s.keys {
        let style = if *key {
            Style::default()
                .fg(theme.value_hi)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.label)
        };
        x = put(buf, x, y0 + 4, part, style, inside);
    }
}

/// A share as a percentage, one decimal.
fn pct(v: f64) -> String {
    format!("{:.1}", 100.0 * v)
}
