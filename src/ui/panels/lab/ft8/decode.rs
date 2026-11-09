// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The transmission's numbers: who sent it and from where, what it measured
//! and what decoding it took; under them, every transmission of the same
//! station still held.
//!
//! The SNR is sdrtop's own, in 2500 Hz, and says so; tone 0 carries its
//! spread; the path is the decode's own judgement. Where the station is
//! comes from the station table (the latest locator it sent itself), and the
//! distance only with your position set; neither is ever guessed (POLICY.md,
//! rule 2).
//!
//! Wide, a line a measurement; narrow, the same measurements in a short
//! column. The station's history takes whatever rows are left.

use std::sync::Arc;

use ratatui::{
    layout::Rect,
    style::{Color, Style},
    Frame,
};

use super::{shown_or_say, NARROW_BODY};
use crate::state::ft8::DecodeEntry;
use crate::state::SdrMetrics;
use crate::ui::panel::{Panel, PanelChrome, Staleness};
use crate::ui::panels::radio::ft8::{hhmmss, km, path, put, signed};

pub struct Ft8DecodePanel;

/// Narrower than this inside, the short column.
const WIDE: u16 = 60;

impl Panel for Ft8DecodePanel {
    fn name(&self) -> &'static str {
        "ft8_decode"
    }

    fn reads_spectrum(&self) -> bool {
        false
    }

    fn min_size(&self) -> (u16, u16) {
        (30, 5)
    }

    /// On a narrow body it takes the whole body: the track and the power
    /// have given way.
    fn column_width(&self, body: u16) -> Option<u16> {
        (body < NARROW_BODY).then_some(body)
    }

    fn chrome(&self, _state: &SdrMetrics) -> PanelChrome {
        PanelChrome::new("Decode").stale_when(Staleness::NotStreaming)
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
        let Some(d) = shown_or_say(buf, area, state, theme) else {
            return;
        };
        let full = wide(state, &d, theme);
        let lines = if area.width < WIDE {
            short(state, &d, theme)
        } else if (area.height as usize) < full.len() {
            dense(state, &d, theme)
        } else {
            full
        };
        let mut y = area.y;
        for line in &lines {
            if y >= area.bottom() {
                return;
            }
            write(buf, area, y, line);
            y += 1;
        }
        // The station's history in what is left, after a gap: only with
        // room for its header, the column names and at least one row.
        if y + 4 <= area.bottom() {
            for line in history(state, &d, theme, (area.bottom() - y - 1) as usize) {
                y += 1;
                if y >= area.bottom() {
                    break;
                }
                write(buf, area, y, &line);
            }
        }
    }
}

/// A line: pieces of text, each in its own colour.
type Line = Vec<(String, Color)>;

fn write(buf: &mut ratatui::buffer::Buffer, area: Rect, y: u16, line: &Line) {
    let mut x = area.x + 1;
    for (text, colour) in line {
        x = put(buf, area, x, y, text, Style::default().fg(*colour));
    }
}

/// Where the sender is, as far as anything says.
fn whereabouts(m: &SdrMetrics, d: &DecodeEntry) -> (String, String) {
    let sender = d.decode.message.sender().unwrap_or("<...>").to_string();
    let locator = m.ft8.stations.get(&sender).and_then(|s| s.locator.clone());
    let place = match (&locator, path(m, locator.as_deref())) {
        (None, _) => "no locator sent".to_string(),
        (Some(loc), Some((d_km, bearing))) => format!(
            "{loc} · {} km {:.0}° {}",
            km(d_km),
            bearing,
            crate::geo::sphere::compass(bearing)
        ),
        (Some(loc), None) => format!("{loc} · no position set"),
    };
    (sender, place)
}

/// How decoding it went, in words.
fn code(d: &DecodeEntry, short: bool) -> String {
    let dd = &d.decode;
    match (dd.osd, short) {
        (None, false) if dd.iterations == 0 => {
            "BP at once: received as a codeword, OSD not needed".into()
        }
        (None, false) => format!("BP in {} iterations, OSD not needed", dd.iterations),
        (Some(o), false) => format!("OSD order {o} after {} BP iterations", dd.iterations),
        (None, true) if dd.iterations == 0 => "BP at once".into(),
        (None, true) => format!("BP {} iterations", dd.iterations),
        (Some(o), true) => format!("OSD {o} after {} BP", dd.iterations),
    }
}

fn path_word(d: &DecodeEntry, t: &crate::Theme) -> (String, Color) {
    let p = &d.decode.physics;
    if p.fading() {
        (format!("fading {:.2}", p.coherence), t.status_warn)
    } else {
        (format!("steady {:.2}", p.coherence), t.status_ok)
    }
}

fn wide(m: &SdrMetrics, d: &DecodeEntry, t: &crate::Theme) -> Vec<Line> {
    let dd = &d.decode;
    let p = &dd.physics;
    let (sender, place) = whereabouts(m, d);
    let kv = |k: &str, v: String| -> Line { vec![(format!("{k:<9}"), t.label), (v, t.value)] };
    let mut snr = kv("SNR", format!("{} dB  ", signed(p.snr_db, 1)));
    snr.push(("sdrtop, in 2500 Hz".into(), t.label));
    let mut drift = kv(
        "drift",
        format!("{} Hz over 12.64 s   ", signed(p.drift, 2)),
    );
    drift.push(path_word(d, t));
    let fixed = dd.trace.repaired_symbols().iter().filter(|r| **r).count();
    vec![
        vec![(dd.message.text(), t.value_hi)],
        vec![(format!("{sender}  "), t.value), (place, t.label)],
        snr,
        kv("DT", format!("{} s", signed(dd.dt, 3))),
        kv("f0", format!("{:.2} ± {:.2} Hz", p.f0, p.f0_sigma)),
        drift,
        kv(
            "sync",
            format!(
                "{} of 21 Costas   pass {}   block {}",
                dd.costas, dd.pass, dd.block
            ),
        ),
        kv("code", code(d, false)),
        kv(
            "repaired",
            format!(
                "{} of 174 bits in {fixed} symbols   distance {:.3}",
                dd.repaired,
                dd.distance.max(0.0)
            ),
        ),
        kv("check", "CRC-14 passed".into()),
    ]
}

/// Wide but short: the measurements on two lines, the place on a third.
fn dense(m: &SdrMetrics, d: &DecodeEntry, t: &crate::Theme) -> Vec<Line> {
    let dd = &d.decode;
    let p = &dd.physics;
    let (sender, place) = whereabouts(m, d);
    let sep = || (" · ".to_string(), t.border_dim);
    let kv = |k: &str, v: String| [(format!("{k} "), t.label), (v, t.value)];
    let mut first: Line = Vec::new();
    for (i, pair) in [
        kv("SNR", format!("{} dB sdrtop", signed(p.snr_db, 1))),
        kv("DT", format!("{} s", signed(dd.dt, 3))),
        kv("f0", format!("{:.2} ± {:.2} Hz", p.f0, p.f0_sigma)),
        kv("drift", format!("{} Hz", signed(p.drift, 2))),
    ]
    .into_iter()
    .enumerate()
    {
        if i > 0 {
            first.push(sep());
        }
        first.extend(pair);
    }
    let mut second: Line = vec![path_word(d, t), sep()];
    for (i, pair) in [
        kv("sync", format!("{}/21 pass {}", dd.costas, dd.pass)),
        kv("code", code(d, true)),
        kv("fixed", format!("{} of 174", dd.repaired)),
        kv("check", "CRC-14 passed".into()),
    ]
    .into_iter()
    .enumerate()
    {
        if i > 0 {
            second.push(sep());
        }
        second.extend(pair);
    }
    vec![
        first,
        second,
        vec![(format!("{sender}  "), t.value), (place, t.label)],
    ]
}

fn short(m: &SdrMetrics, d: &DecodeEntry, t: &crate::Theme) -> Vec<Line> {
    let dd = &d.decode;
    let p = &dd.physics;
    let (sender, place) = whereabouts(m, d);
    let kv = |k: &str, v: String, c: Color| -> Line { vec![(format!("{k:<7}"), t.label), (v, c)] };
    let (pw, pc) = path_word(d, t);
    // Most telling first: a short panel keeps the top of this list. The
    // sender is in the ladder's title already; where it is comes after the
    // measurements.
    vec![
        kv("SNR", format!("{} dB sdrtop", signed(p.snr_db, 1)), t.value),
        kv("DT", format!("{} s", signed(dd.dt, 3)), t.value),
        kv("f0", format!("{:.2} ± {:.2} Hz", p.f0, p.f0_sigma), t.value),
        kv("drift", format!("{} Hz", signed(p.drift, 2)), t.value),
        kv("path", pw, pc),
        kv(
            "sync",
            format!("{}/21 · pass {} · block {}", dd.costas, dd.pass, dd.block),
            t.value,
        ),
        kv("code", code(d, true), t.value),
        kv("fixed", format!("{} of 174 bits", dd.repaired), t.value),
        kv("where", format!("{sender} {place}"), t.value),
        kv("check", "CRC-14 passed".into(), t.value),
    ]
}

/// The station's transmissions still held, newest first, in `rows` lines
/// (its header and column names included).
fn history(m: &SdrMetrics, d: &DecodeEntry, t: &crate::Theme, rows: usize) -> Vec<Line> {
    let Some(call) = d.decode.message.sender() else {
        return vec![vec![(
            "a hashed sender: no history to keep".into(),
            t.label,
        )]];
    };
    let held: Vec<&Arc<DecodeEntry>> = m
        .ft8
        .decodes
        .iter()
        .rev()
        .filter(|e| e.decode.message.sender() == Some(call))
        .collect();
    let heard = m
        .ft8
        .stations
        .get(call)
        .map_or(held.len() as u32, |s| s.heard);
    let times = if heard == 1 {
        "once".to_string()
    } else {
        format!("{heard} times")
    };
    let mut out: Vec<Line> = vec![
        vec![
            (format!("{call}  "), t.value_hi),
            (
                format!("heard {times} this session, {} still held", held.len()),
                t.label,
            ),
        ],
        vec![(
            "cycle      SNR     f0 Hz     drift  path    sync  code".into(),
            t.label,
        )],
    ];
    for e in held.iter().take(rows.saturating_sub(2)) {
        let p = &e.decode.physics;
        let this = e.n == d.n;
        out.push(vec![(
            format!(
                "{}  {:>5}  {:9.2}  {:>6}  {:<6}  {:>2}/21  {}",
                hhmmss(e.cycle_unix),
                signed(p.snr_db, 0),
                p.f0,
                signed(p.drift, 2),
                if p.fading() { "fading" } else { "steady" },
                e.decode.costas,
                if e.decode.osd.is_some() { "OSD" } else { "BP" }
            ),
            if this { t.value_hi } else { t.value },
        )]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::fixture::draw as render;
    use crate::state::ft8::Shown;

    fn shown(m: &SdrMetrics) -> Arc<DecodeEntry> {
        let Shown::Decode(d) = m.ft8.shown() else {
            panic!("the fixture has decodes")
        };
        d
    }

    /// Wide, every measurement on its own line, as the decode has it.
    #[test]
    fn wide_it_lists_every_measurement() {
        let m = SdrMetrics::fixture().streaming().ft8_traced();
        let d = shown(&m);
        let p = &d.decode.physics;
        let lines = render(Ft8DecodePanel, 80, 24, &m).join("\n");
        for want in [
            d.decode.message.text(),
            format!("SNR      {} dB  sdrtop, in 2500 Hz", signed(p.snr_db, 1)),
            format!("DT       {} s", signed(d.decode.dt, 3)),
            format!("f0       {:.2} ± {:.2} Hz", p.f0, p.f0_sigma),
            format!("sync     {} of 21 Costas", d.decode.costas),
            format!("{} of 174 bits", d.decode.repaired),
            "CRC-14 passed".into(),
        ] {
            assert!(lines.contains(&want), "missing {want:?} in\n{lines}");
        }
        // Budapest to the fixture's locators: the place, with a distance.
        assert!(lines.contains(" km "), "{lines}");
    }

    #[test]
    fn narrow_it_keeps_the_measurements_in_a_short_column() {
        let m = SdrMetrics::fixture().streaming().ft8_traced();
        let d = shown(&m);
        let lines = render(Ft8DecodePanel, 46, 16, &m).join("\n");
        assert!(
            lines.contains(&format!("{}/21", d.decode.costas)),
            "{lines}"
        );
        assert!(lines.contains("dB sdrtop"), "{lines}");
        assert!(!lines.contains("in 2500 Hz"), "{lines}");
    }

    /// Wide but short, the measurements come first, two lines of them.
    #[test]
    fn wide_and_short_it_packs_the_measurements_first() {
        let m = SdrMetrics::fixture().streaming().ft8_traced();
        let d = shown(&m);
        let lines = render(Ft8DecodePanel, 100, 4, &m);
        assert!(
            lines[1].contains(&format!(
                "SNR {} dB sdrtop",
                signed(d.decode.physics.snr_db, 1)
            )),
            "{lines:?}"
        );
        assert!(
            lines[2].contains(&format!("sync {}/21", d.decode.costas)),
            "{lines:?}"
        );
    }

    #[test]
    fn without_a_position_the_place_says_so() {
        let mut m = SdrMetrics::fixture().streaming().ft8_traced();
        m.home.latitude = None;
        m.home.longitude = None;
        let lines = render(Ft8DecodePanel, 80, 24, &m).join("\n");
        assert!(
            lines.contains("no position set") || lines.contains("no locator sent"),
            "{lines}"
        );
        assert!(!lines.contains(" km "), "{lines}");
    }

    /// Rows to spare carry the station's history: its header and a row a
    /// transmission still held.
    #[test]
    fn spare_rows_carry_the_stations_history() {
        let m = SdrMetrics::fixture().streaming().ft8_traced();
        let d = shown(&m);
        let call = d.decode.message.sender().unwrap();
        let lines = render(Ft8DecodePanel, 80, 24, &m).join("\n");
        assert!(
            lines.contains(&format!("{call}  heard once this session, 1 still held")),
            "{lines}"
        );
        assert!(lines.contains("cycle      SNR"), "{lines}");
    }

    #[test]
    fn the_code_says_how_it_got_there() {
        let m = SdrMetrics::fixture().streaming().ft8_traced();
        let mut d = (*shown(&m)).clone();
        d.decode.iterations = 0;
        d.decode.osd = None;
        assert_eq!(code(&d, true), "BP at once");
        d.decode.iterations = 7;
        assert_eq!(code(&d, false), "BP in 7 iterations, OSD not needed");
        d.decode.iterations = 30;
        d.decode.osd = Some(2);
        assert_eq!(code(&d, false), "OSD order 2 after 30 BP iterations");
    }

    #[test]
    fn on_a_narrow_body_it_takes_the_body() {
        assert_eq!(Ft8DecodePanel.column_width(98), Some(98));
        assert_eq!(Ft8DecodePanel.column_width(189), None);
    }
}
