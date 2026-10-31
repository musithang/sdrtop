// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The FT8 view's panels, and what they share.
//!
//! - [`cycles`]: the last minute's cycles, the one being heard, and the
//!   clock as the stations heard judge it.
//! - [`band`]: the band as a river, a row a cycle, every decode on its tone.
//! - [`decodes`]: the decodes, cycle by cycle.
//! - [`card`]: the selected station's newest decode and what it took.
//! - [`map`]: the world around you, as you hear it.
//!
//! All of them read one selection (`Ft8View::selected`, a station's call),
//! so the list, the river, the card and the map cannot be about different
//! stations.

pub mod band;
pub mod card;
pub mod cycles;
pub mod decodes;
pub mod map;

use crate::state::{Ft8State, SdrMetrics, Station};
use ratatui::style::Color;

/// How fresh a station is, measured from the newest decoded cycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Age {
    /// Heard in the newest cycle.
    Now,
    /// Within the last 45 s.
    Recent,
    /// Within five minutes.
    Earlier,
    /// Longer ago, until it leaves the table.
    Old,
}

/// The newest cycle that was decoded, Unix seconds.
pub fn newest_cycle(f: &Ft8State) -> Option<f64> {
    f.cycles
        .iter()
        .rev()
        .find(|c| c.outcome == crate::state::CycleOutcome::Decoded)
        .map(|c| c.start_unix)
}

pub fn age(s: &Station, newest: Option<f64>) -> Age {
    let Some(newest) = newest else {
        return Age::Old;
    };
    match newest - s.last_unix {
        d if d <= 0.0 => Age::Now,
        d if d <= 45.0 => Age::Recent,
        d if d <= 300.0 => Age::Earlier,
        _ => Age::Old,
    }
}

/// A station's ink: calling you outranks its age.
pub fn ink(s: &Station, age: Age, theme: &crate::Theme) -> Color {
    if s.called_you {
        return theme.status_crit;
    }
    match age {
        Age::Now => theme.value_hi,
        Age::Recent => theme.value,
        Age::Earlier => theme.label,
        Age::Old => theme.border_dim,
    }
}

/// The time the map's sun is drawn for: the cycle being heard, else the
/// newest one. `None` before the stream has a time.
pub fn sun_time(m: &SdrMetrics) -> Option<f64> {
    m.ft8
        .hearing
        .or_else(|| m.ft8.cycles.back().map(|c| c.start_unix))
}

/// `HH:MM` UTC of a Unix time.
pub fn hhmm(unix: f64) -> String {
    let s = unix.rem_euclid(86_400.0) as u64;
    format!("{:02}:{:02}", s / 3600, (s / 60) % 60)
}

/// A signed reading to `decimals` places. One that rounds to zero is
/// written `+0.0`, not `-0.0`: beside `+0.0` a minus sign on a zero reads as
/// a different number, and the precision shown cannot tell the two apart.
pub fn signed(v: f64, decimals: usize) -> String {
    let scale = 10f64.powi(decimals as i32);
    let r = (v * scale).round() / scale;
    format!("{:+.*}", decimals, if r == 0.0 { 0.0 } else { r })
}

/// `HH:MM:SS` UTC of a Unix time.
pub fn hhmmss(unix: f64) -> String {
    let s = unix.rem_euclid(86_400.0) as u64;
    format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}

/// "even" for the cycles that start on the minute and the half minute.
pub fn parity(start_unix: f64) -> &'static str {
    if (start_unix / 15.0).round() as i64 % 2 == 0 {
        "even"
    } else {
        "odd"
    }
}

/// Distance (km) and bearing (degrees) from you to a locator, when both
/// are known.
pub fn path(m: &SdrMetrics, locator: Option<&str>) -> Option<(f64, f64)> {
    let home = m.home.position()?;
    let there = crate::geo::maidenhead::centre(locator?)?;
    Some((
        crate::geo::sphere::distance_km(home, there),
        crate::geo::sphere::bearing_deg(home, there),
    ))
}

/// Kilometres with a thin space every three digits: `9 026`.
pub fn km(v: f64) -> String {
    let s = format!("{:.0}", v.max(0.0));
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

/// Write `text` at (`x`, `y`) inside `area`, clipped.
pub fn put(
    buf: &mut ratatui::buffer::Buffer,
    area: ratatui::layout::Rect,
    x: u16,
    y: u16,
    text: &str,
    style: ratatui::style::Style,
) -> u16 {
    let mut cx = x;
    for ch in text.chars() {
        if y >= area.top() && y < area.bottom() && cx >= area.left() && cx < area.right() {
            buf.get_mut(cx, y).set_char(ch).set_style(style);
        }
        cx = cx.saturating_add(1);
    }
    cx
}

/// The sheet's evening on 20 m, built through the state as the worker
/// builds it, for the panels' tests.
#[cfg(test)]
pub(crate) mod sample {
    use crate::signal::ft8::cycle::Decode;
    use crate::signal::ft8::demod::Physics;
    use crate::signal::ft8::fields::G15;
    use crate::signal::ft8::message::{Call, Message};
    use crate::state::{CycleEntry, CycleOutcome, SdrMetrics};
    use std::time::Instant;

    /// 2026-10-30 13:15:00 UTC.
    pub const NOW: f64 = 1_793_366_100.0;

    fn std(call: &str) -> Call {
        Call::Std {
            call: call.into(),
            suffix: None,
        }
    }

    fn d(
        to: Option<&str>,
        from: &str,
        word: G15,
        snr: f64,
        dt: f64,
        f0: f64,
        osd: Option<u8>,
    ) -> Decode {
        Decode {
            message: Message::Standard {
                to: to.map_or(Call::Cq(None), std),
                from: std(from),
                ack: false,
                word,
            },
            payload: 0,
            dt,
            physics: Physics {
                snr_db: snr,
                start: 0.5 + dt,
                f0,
                f0_sigma: 0.01,
                drift: -0.2,
                amplitude: 0.1,
                phase: 0.0,
            },
            costas: if osd.is_some() { 15 } else { 21 },
            block: if osd.is_some() { 3 } else { 1 },
            iterations: if osd.is_some() { 30 } else { 4 },
            repaired: if osd.is_some() { 16 } else { 2 },
            osd,
            distance: 0.0,
            pass: 1,
        }
    }

    fn grid(g: &str) -> G15 {
        G15::Grid(g.into())
    }

    fn cycle(start: f64, decodes: usize) -> CycleEntry {
        let mut band = vec![0.0f32; 120];
        band[79] = 9.0; // energy at 1975-2000 Hz that did not decode
        CycleEntry {
            start_unix: start,
            dial_hz: 14_074_000,
            outcome: CycleOutcome::Decoded,
            decodes,
            candidates: 140,
            malformed: 0,
            seconds: 1.8,
            band,
        }
    }

    pub fn evening() -> SdrMetrics {
        let mut m = SdrMetrics::fixture().streaming();
        m.radio.frequency = 14_074_000;
        m.home.latitude = Some(47.497);
        m.home.longitude = Some(19.04);
        m.home.callsign = Some("HA5ZZT".into());
        let at = Instant::now();
        let earlier = vec![
            d(None, "JA1VQD", grid("PM95"), -23.0, 0.50, 2212.0, Some(2)),
            d(None, "PY2WQS", grid("GG66"), -15.0, 0.38, 1175.0, None),
        ];
        m.ft8
            .push_cycle(cycle(NOW - 15.0, 2), earlier, None, at, Some("HA5ZZT"));
        let now = vec![
            d(
                Some("HA5ZZT"),
                "JA1VQD",
                grid("PM95"),
                -21.0,
                0.48,
                2210.6,
                Some(2),
            ),
            d(None, "EA8ZQ", grid("IL18"), -9.0, 0.31, 612.0, None),
            d(None, "W2XK", grid("FN20"), -17.0, 0.42, 1420.0, None),
            d(
                Some("OH2BKM"),
                "DL7ANR",
                grid("JO62"),
                -3.0,
                0.19,
                902.0,
                None,
            ),
            d(None, "VK3FTX", grid("QF22"), -22.0, 0.61, 1840.0, Some(1)),
            d(None, "OK1HZD", grid("JO70"), 2.0, 0.12, 1305.0, None),
            d(
                Some("F5RVK"),
                "YO3GCI",
                G15::Report(1),
                -1.0,
                0.20,
                340.0,
                None,
            ),
        ];
        let verdict =
            crate::state::ft8_verdict(&[0.48, 0.31, 0.42, 0.19, 0.61, 0.12, 0.20, 0.50, 0.38]);
        m.ft8
            .push_cycle(cycle(NOW, 7), now, verdict, at, Some("HA5ZZT"));
        m.ft8.hearing = Some(NOW + 15.0);
        m.ft8.heard_until = Some(NOW + 15.0 + 9.2);
        m
    }
}

/// The view composed by hand at 191 x 41, the sheet's layout, from the real
/// panels' cells, as HTML to the path in `FT8_HTML`, for looking at.
#[cfg(test)]
#[test]
#[ignore]
fn render_the_view_to_html() {
    use crate::ui::panel::Panel;
    use ratatui::layout::Rect;
    use ratatui::style::Color;
    let Ok(path) = std::env::var("FT8_HTML") else {
        return;
    };
    let mut m = sample::evening();
    m.ft8.view.selected = Some("JA1VQD".into());
    let theme = crate::Theme::sdr();
    let mut registry = crate::ui::PanelRegistry::new();
    registry.register(map::Ft8MapPanel::new());
    registry.register(cycles::Ft8CyclesPanel);
    registry.register(band::Ft8BandPanel);
    registry.register(decodes::Ft8DecodesPanel);
    registry.register(card::Ft8CardPanel);
    let _ = cycles::Ft8CyclesPanel.name();
    let mut t = ratatui::Terminal::new(ratatui::backend::TestBackend::new(191, 41)).unwrap();
    t.draw(|f| {
        let at = |x, y, w, h| Rect::new(x, y, w, h);
        registry.render_panel("ft8_cycles", f, at(0, 0, 191, 1), &m, &theme, false);
        registry.render_panel("ft8_map", f, at(0, 1, 112, 40), &m, &theme, false);
        registry.render_panel("ft8_band", f, at(112, 1, 79, 12), &m, &theme, false);
        registry.render_panel("ft8_decodes", f, at(112, 13, 79, 19), &m, &theme, false);
        registry.render_panel("ft8_station", f, at(112, 32, 79, 9), &m, &theme, false);
    })
    .unwrap();
    let buf = t.backend().buffer().clone();
    let mut out = String::from("<div class=g>");
    for y in 0..41 {
        out.push_str("<div class=r>");
        for x in 0..191 {
            let c = buf.get(x, y);
            let col = |c: Color, d: &str| match c {
                Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
                _ => d.to_string(),
            };
            let sym = c.symbol().replace('&', "&amp;").replace('<', "&lt;");
            out.push_str(&format!(
                "<i style=\"color:{};background:{}\">{sym}</i>",
                col(c.fg, "#c3d2dc"),
                col(c.bg, "#000000")
            ));
        }
        out.push_str("</div>");
    }
    out.push_str("</div>");
    std::fs::write(path, out).unwrap();
}

#[cfg(test)]
#[test]
fn a_reading_that_rounds_to_zero_is_never_negative() {
    // Live: "-0.0" in the DT column beside "+0.0", for a DT of -0.04 s.
    assert_eq!(signed(-0.04, 1), "+0.0");
    assert_eq!(signed(-0.4, 0), "+0");
    assert_eq!(signed(0.0, 2), "+0.00");
    assert_eq!(signed(-0.06, 1), "-0.1");
    assert_eq!(signed(0.034, 3), "+0.034");
    assert_eq!(signed(-14.2, 0), "-14");
}
