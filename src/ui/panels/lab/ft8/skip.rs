// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! SNR against distance: each station of the hour at its best decode, a
//! braille dot, distance across on the reach's log scale and SNR (sdrtop's,
//! in 2500 Hz) up from -24 to +6 dB. The skip zone shows as the empty
//! stretch on the left, the band's reach as where the dots stop. No curve
//! is fitted: a fit would assert a model the dots do not carry.

use std::collections::BTreeMap;

use ratatui::{layout::Rect, style::Style, Frame};

use super::reach::km_frac;
use super::{placed, say, Placed};
use crate::state::SdrMetrics;
use crate::ui::panel::{Panel, PanelChrome, Staleness};
use crate::ui::panels::radio::ft8::put;

pub struct Ft8SkipPanel;

/// The SNR range up the side, dB.
pub const SNR_RANGE: (f64, f64) = (-24.0, 6.0);

const BIT: [[u8; 2]; 4] = [[0x01, 0x08], [0x02, 0x10], [0x04, 0x20], [0x40, 0x80]];

/// Each station's best decode: its distance and SNR.
pub fn bests(pts: &[Placed]) -> Vec<(f64, f64)> {
    let mut best: BTreeMap<&str, (f64, f64)> = BTreeMap::new();
    for p in pts {
        let e = best.entry(&p.call).or_insert((p.km, f64::MIN));
        if p.snr > e.1 {
            *e = (p.km, p.snr);
        }
    }
    best.into_values().collect()
}

impl Panel for Ft8SkipPanel {
    fn name(&self) -> &'static str {
        "ft8_skip"
    }

    fn reads_spectrum(&self) -> bool {
        false
    }

    fn min_size(&self) -> (u16, u16) {
        (30, 6)
    }

    fn chrome(&self, _state: &SdrMetrics) -> PanelChrome {
        PanelChrome::new("SNR against distance").stale_when(Staleness::NotStreaming)
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
        let best = bests(&pts);
        if best.is_empty() {
            say(buf, area, "no station placed yet", theme);
            return;
        }
        if area.height < 4 || area.width < 20 {
            say(buf, area, "too small", theme);
            return;
        }
        let label = Style::default().fg(theme.label);
        let rows = area.height - 2;
        let x0 = area.x + 6;
        let cols = area.width.saturating_sub(7).max(1);
        let mut dots = vec![vec![0u8; usize::from(cols)]; usize::from(rows)];
        for (d, snr) in &best {
            let fx = (km_frac(*d) * f64::from(cols * 2 - 1)).round() as usize;
            let k =
                (SNR_RANGE.1 - snr.clamp(SNR_RANGE.0, SNR_RANGE.1)) / (SNR_RANGE.1 - SNR_RANGE.0);
            let fy = (k * f64::from(rows * 4 - 1)).round() as usize;
            dots[fy / 4][fx / 2] |= BIT[fy % 4][fx % 2];
        }
        for (r, row) in dots.iter().enumerate() {
            for (c, &b) in row.iter().enumerate() {
                if b != 0 {
                    let ch = char::from_u32(0x2800 + u32::from(b)).unwrap_or(' ');
                    put(
                        buf,
                        area,
                        x0 + c as u16,
                        area.y + r as u16,
                        &ch.to_string(),
                        Style::default().fg(theme.value_hi),
                    );
                }
            }
        }
        put(buf, area, area.x + 1, area.y, "  +6", label);
        put(buf, area, area.x + 1, area.y + rows / 2, "  -9", label);
        put(buf, area, area.x + 1, area.y + rows - 1, " -24", label);
        for (d, text) in [(100.0, "100"), (1_000.0, "1 000"), (10_000.0, "10 000 km")] {
            let c = (km_frac(d) * f64::from(cols - 1)).round() as u16;
            let c = c.min(cols.saturating_sub(text.len() as u16));
            put(buf, area, x0 + c, area.y + rows, text, label);
        }
        put(
            buf,
            area,
            x0,
            area.y + rows + 1,
            &format!("{} stations, each its best · SNR dB (sdrtop)", best.len()),
            Style::default().fg(theme.border_dim),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::fixture::draw;
    use std::sync::Arc;

    #[test]
    fn a_station_is_its_best_decode() {
        let p = |call: &str, km: f64, snr: f64| Placed {
            unix: 0.0,
            km,
            bearing: 0.0,
            snr,
            call: Arc::from(call),
        };
        let mut b = bests(&[
            p("A", 900.0, -20.0),
            p("A", 950.0, -7.0),
            p("B", 4000.0, -15.0),
        ]);
        b.sort_by(|a, c| a.0.total_cmp(&c.0));
        assert_eq!(b, vec![(950.0, -7.0), (4000.0, -15.0)]);
    }

    #[test]
    fn every_station_is_a_dot_and_is_counted() {
        let m = SdrMetrics::fixture().streaming().ft8_hour();
        let (pts, _) = placed(&m).unwrap();
        let n = bests(&pts).len();
        let lines = draw(Ft8SkipPanel, 64, 14, &m);
        let dots: u32 = lines
            .iter()
            .flat_map(|l| l.chars())
            .filter(|c| ('\u{2801}'..='\u{28ff}').contains(c))
            .map(|c| (c as u32 - 0x2800).count_ones())
            .sum();
        assert!(dots > 0 && dots as usize <= n, "{dots} dots, {n} stations");
        assert!(lines
            .join("\n")
            .contains(&format!("{n} stations, each its best")));
    }
}
