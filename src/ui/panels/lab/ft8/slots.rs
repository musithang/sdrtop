// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Room in the passband: where in the 3 kHz there is space, as heard here.
//!
//! The band in 50 Hz slots, each the share of the recent cycles (the cycle
//! ring, ten minutes) whose band row shows energy [`BUSY_DB`] over that
//! cycle's own median there, decoded or not. Even cycles above the rule and
//! odd hanging below it, because a transmitting station picks a slot and a
//! parity both. The quietest slots of each parity are named; with nothing
//! busy at all it says so rather than naming three. A slot quiet at this
//! antenna may be busy at a station 3 000 km away, and the panel says that
//! it is as heard here.

use ratatui::{layout::Rect, style::Style, Frame};

use super::{hanging, say};
use crate::signal::ft8::cycle::BAND_BIN_HZ;
use crate::state::ft8::CycleOutcome;
use crate::state::SdrMetrics;
use crate::ui::panel::{Panel, PanelChrome, Staleness};
use crate::ui::panels::radio::ft8::put;

pub struct Ft8SlotsPanel;

/// A slot's width, Hz.
pub const SLOT_HZ: f64 = 50.0;

/// Slots from 0 to 3 000 Hz.
pub const SLOTS: usize = 60;

/// Energy this far over the cycle's median makes a slot busy in it: plainly
/// above the noise, whatever made it.
pub const BUSY_DB: f32 = 6.0;

/// Below this the receiver's own (what is left of its DC): never named as
/// room.
const FIRST_HZ: f64 = 100.0;

const EIGHTHS: [&str; 9] = [" ", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];

/// Each slot's busy share for even (0) and odd (1) cycles, and how many
/// cycles of each were read.
pub fn shares(m: &SdrMetrics) -> ([[f64; SLOTS]; 2], [usize; 2]) {
    let per = (SLOT_HZ / BAND_BIN_HZ).round() as usize;
    let mut busy = [[0usize; SLOTS]; 2];
    let mut seen = [0usize; 2];
    for c in m
        .ft8
        .cycles
        .iter()
        .filter(|c| c.outcome == CycleOutcome::Decoded)
    {
        if c.band.len() < SLOTS * per {
            continue;
        }
        let p = usize::from((c.start_unix / 15.0).round() as i64 % 2 != 0);
        seen[p] += 1;
        for (k, bins) in c.band.chunks(per).take(SLOTS).enumerate() {
            if bins.iter().copied().fold(f32::MIN, f32::max) >= BUSY_DB {
                busy[p][k] += 1;
            }
        }
    }
    let mut out = [[0.0; SLOTS]; 2];
    for p in 0..2 {
        for k in 0..SLOTS {
            out[p][k] = busy[p][k] as f64 / seen[p].max(1) as f64;
        }
    }
    (out, seen)
}

/// The quietest slots of one parity, by their low edge in Hz: the three
/// least busy, or `None` when none was busy at all.
pub fn quietest(share: &[f64; SLOTS]) -> Option<Vec<u32>> {
    let first = (FIRST_HZ / SLOT_HZ) as usize;
    if share[first..].iter().all(|&s| s == 0.0) {
        return None;
    }
    let mut v: Vec<(f64, usize)> = (first..SLOTS).map(|k| (share[k], k)).collect();
    v.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    Some(
        v.iter()
            .take(3)
            .map(|(_, k)| (*k as f64 * SLOT_HZ) as u32)
            .collect(),
    )
}

impl Panel for Ft8SlotsPanel {
    fn name(&self) -> &'static str {
        "ft8_slots"
    }

    fn reads_spectrum(&self) -> bool {
        false
    }

    fn min_size(&self) -> (u16, u16) {
        (40, 5)
    }

    /// Under the reach in the body column, its own five rows and frame.
    fn column_height(&self, _column: u16) -> Option<u16> {
        Some(7)
    }

    fn chrome(&self, state: &SdrMetrics) -> PanelChrome {
        let (_, seen) = shares(state);
        let mut chrome =
            PanelChrome::new("Room in the passband").stale_when(Staleness::NotStreaming);
        let n = seen[0] + seen[1];
        chrome.suffix = Some(if n == 1 {
            "  the last cycle, as heard here".to_string()
        } else {
            format!("  the last {n} cycles, as heard here")
        });
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
        let (share, seen) = shares(state);
        if seen[0] + seen[1] == 0 {
            say(buf, area, "waiting for a decoded cycle", theme);
            return;
        }
        let label = Style::default().fg(theme.label);
        let dim = Style::default().fg(theme.border_dim);
        let x0 = area.x + 1 + 9;
        let cols = usize::from(area.width.saturating_sub(11)).max(1);
        let y = area.y;
        put(buf, area, area.x + 1, y, " even", label);
        put(buf, area, area.x + 1, y + 1, "slots", dim);
        put(buf, area, area.x + 1, y + 2, "  odd", label);
        for c in 0..cols {
            let k = c * SLOTS / cols;
            let x = x0 + c as u16;
            let e = (share[0][k] * 8.0).round() as usize;
            put(
                buf,
                area,
                x,
                y,
                EIGHTHS[e],
                Style::default().fg(theme.value),
            );
            put(buf, area, x, y + 1, "─", dim);
            let o = (share[1][k] * 8.0).round() as usize;
            if let Some(glyph) = hanging(o) {
                put(buf, area, x, y + 2, glyph, Style::default().fg(theme.value));
            }
        }
        for (hz, text) in [
            (0usize, "0"),
            (1000, "1 000"),
            (2000, "2 000"),
            (3000, "3 000 Hz"),
        ] {
            let c = (hz * cols / 3000).min(cols.saturating_sub(text.len()));
            put(buf, area, x0 + c as u16, y + 3, text, label);
        }
        let words = |p: usize| match quietest(&share[p]) {
            None => "all quiet".to_string(),
            Some(v) => {
                v.iter()
                    .map(|hz| format!("{hz}"))
                    .collect::<Vec<_>>()
                    .join(", ")
                    + " Hz"
            }
        };
        put(
            buf,
            area,
            x0,
            y + 4,
            &format!("quietest even: {} · odd: {}", words(0), words(1)),
            label,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::fixture::draw;

    #[test]
    fn a_band_with_nothing_busy_is_all_quiet() {
        assert_eq!(quietest(&[0.0; SLOTS]), None);
    }

    #[test]
    fn the_quietest_are_the_least_busy_from_100_hz() {
        let mut s = [0.5; SLOTS];
        s[0] = 0.0; // the receiver's own
        s[10] = 0.0;
        s[30] = 0.1;
        s[45] = 0.0;
        assert_eq!(quietest(&s), Some(vec![500, 2250, 1500]));
    }

    /// Shares count cycles of each parity apart: a slot busy in every even
    /// cycle and no odd one reads full above the rule and empty below it.
    #[test]
    fn the_parities_are_counted_apart() {
        let mut m = SdrMetrics::fixture().streaming().ft8_traced();
        m.ft8.cycles.clear();
        for k in 0..6 {
            let mut band = vec![0.0f32; 120];
            if k % 2 == 0 {
                band[40] = 9.0; // 1000-1025 Hz
            }
            let mut c = crate::state::ft8::CycleEntry {
                start_unix: 1_800_000_000.0 + 15.0 * f64::from(k),
                dial_hz: 7_074_000,
                outcome: CycleOutcome::Decoded,
                decodes: 0,
                candidates: 0,
                malformed: 0,
                seconds: 0.0,
                band,
                clock: None,
                funnel: Vec::new(),
                floor_db_hz: None,
                setting: None,
                setting_changed: false,
                held: false,
                lateness: None,
            };
            c.decodes = 1;
            m.ft8.cycles.push_back(c);
        }
        let (s, seen) = shares(&m);
        assert_eq!(seen, [3, 3]);
        assert_eq!(s[0][20], 1.0);
        assert_eq!(s[1][20], 0.0);
        let lines = draw(Ft8SlotsPanel, 140, 7, &m).join("\n");
        assert!(lines.contains("odd: all quiet"), "{lines}");
        assert!(lines.contains("as heard here"), "{lines}");
    }

    #[test]
    fn before_any_decoded_cycle_it_waits() {
        let all = draw(Ft8SlotsPanel, 140, 7, &SdrMetrics::fixture()).join("\n");
        assert!(all.contains("waiting for a decoded cycle"), "{all}");
    }
}
