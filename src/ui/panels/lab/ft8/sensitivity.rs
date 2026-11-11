// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Sensitivity: the hour's decodes by SNR (sdrtop's, in 2500 Hz), with the
//! level where this decoder reads half of what is sent marked across it, and
//! how far the weakest decode sits from that line.
//!
//! The line is a measurement, not a promise, and says where it came from:
//! simulated cycles (ft8sim, thirty a level, -18 to -30 dB) decoded half
//! their transmissions at about [`HALF_DECODE_DB`]; the reference decoder
//! read about 0.2 dB lower. A decode under the line is not an error: the
//! line is where half get through, not where all stop.
//!
//! Each column is drawn in the Band view's SNR colour
//! ([`super::snr_colour`]) at its own SNR: one quantity on one scale
//! across the Lab.

use ratatui::{layout::Rect, style::Style, Frame};

use super::{say, snr_colour, NARROW_BODY};
use crate::state::SdrMetrics;
use crate::ui::panel::{Panel, PanelChrome, Staleness};
use crate::ui::panels::radio::ft8::put;

pub struct Ft8SensitivityPanel;

/// The SNR this decoder reads half the transmissions at, on simulated
/// cycles, dB.
pub const HALF_DECODE_DB: f64 = -21.2;

/// The histogram's range, dB.
pub const RANGE_DB: (f64, f64) = (-26.0, 10.0);

const EIGHTHS: [&str; 9] = [" ", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];

/// The weakest SNR against the line, in words.
pub fn weakest_words(weakest: f64) -> String {
    let d = weakest - HALF_DECODE_DB;
    let side = if d < 0.0 { "under" } else { "over" };
    format!(
        "weakest {weakest:+.0} dB, {:.1} dB {side} the half-decode line",
        d.abs()
    )
}

impl Panel for Ft8SensitivityPanel {
    fn name(&self) -> &'static str {
        "ft8_sensitivity"
    }

    fn reads_spectrum(&self) -> bool {
        false
    }

    fn min_size(&self) -> (u16, u16) {
        (40, 6)
    }

    /// The right column gives way on a narrow body, with this panel first
    /// in it.
    fn column_width(&self, body: u16) -> Option<u16> {
        (body < NARROW_BODY).then_some(0)
    }

    fn chrome(&self, _state: &SdrMetrics) -> PanelChrome {
        let mut chrome = PanelChrome::new("Sensitivity").stale_when(Staleness::NotStreaming);
        chrome.suffix = Some("  the hour's SNR, sdrtop's".to_string());
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
        let snrs: Vec<f64> = state
            .ft8
            .history
            .iter()
            .map(|r| f64::from(r.snr_db))
            .collect();
        if snrs.is_empty() {
            say(buf, area, "waiting for a decode", theme);
            return;
        }
        if area.height < 4 {
            say(buf, area, "too short", theme);
            return;
        }
        let label = Style::default().fg(theme.label);
        let x0 = area.x + 2;
        let cols = usize::from(area.width.saturating_sub(4)).max(10);
        let rows = area.height - 2;
        let (lo, hi) = RANGE_DB;
        let col_of = |d: f64| -> u16 {
            x0 + ((((d - lo) / (hi - lo)).clamp(0.0, 0.999)) * cols as f64) as u16
        };
        let mut counts = vec![0usize; cols];
        for &s in &snrs {
            counts[usize::from(col_of(s) - x0)] += 1;
        }
        let top = counts.iter().copied().max().unwrap_or(1).max(1) as f64;
        // Each column in the Band view's SNR colour, at its own SNR: one
        // quantity on one scale across the Lab.
        for (c, &n) in counts.iter().enumerate() {
            let snr = lo + (c as f64 + 0.5) / cols as f64 * (hi - lo);
            let ink = Style::default().fg(snr_colour(theme, snr));
            let full = n as f64 / top * f64::from(rows) * 8.0;
            for r in 0..rows {
                let l = f64::from(rows - 1 - r) * 8.0;
                let e = (full - l).clamp(0.0, 8.0).round() as usize;
                if e > 0 {
                    put(buf, area, x0 + c as u16, area.y + r, EIGHTHS[e], ink);
                }
            }
        }
        let line = col_of(HALF_DECODE_DB);
        let warn = Style::default().fg(theme.status_warn);
        for r in 0..rows {
            if buf[(line, area.y + r)].symbol() == " " {
                put(buf, area, line, area.y + r, "┊", warn);
            }
        }
        put(
            buf,
            area,
            line + 1,
            area.y,
            "half decode here (ft8sim)",
            warn,
        );
        for d in [-24.0, -18.0, -12.0, -6.0, 0.0, 6.0] {
            put(
                buf,
                area,
                col_of(d),
                area.y + rows,
                &format!("{d:+.0}"),
                label,
            );
        }
        let weakest = snrs.iter().copied().fold(f64::MAX, f64::min);
        put(
            buf,
            area,
            x0,
            area.y + rows + 1,
            &weakest_words(weakest),
            label,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::fixture::draw;

    #[test]
    fn the_weakest_is_worded_by_its_side_of_the_line() {
        assert_eq!(
            weakest_words(-22.0),
            "weakest -22 dB, 0.8 dB under the half-decode line"
        );
        assert_eq!(
            weakest_words(-19.0),
            "weakest -19 dB, 2.2 dB over the half-decode line"
        );
    }

    #[test]
    fn it_draws_the_hour_with_the_line() {
        let m = SdrMetrics::fixture().streaming().ft8_hour();
        let all = draw(Ft8SensitivityPanel, 86, 12, &m).join("\n");
        assert!(all.contains("half decode here (ft8sim)"), "{all}");
        assert!(all.contains("weakest"), "{all}");
        assert!(all.contains('┊'), "{all}");
    }

    #[test]
    fn the_right_column_gives_way_on_a_narrow_body() {
        assert_eq!(Ft8SensitivityPanel.column_width(149), Some(0));
        assert_eq!(Ft8SensitivityPanel.column_width(189), None);
    }

    /// Every column is drawn in the Band view's SNR colour at its own SNR.
    #[test]
    fn each_column_wears_its_snr_colour() {
        let m = SdrMetrics::fixture().streaming().ft8_hour();
        let theme = crate::Theme::sdr();
        let buf = crate::state::fixture::draw_buffer(Ft8SensitivityPanel, 86, 12, &m);
        // Inside the frame the panel starts at 1; its plot at 3, 80 wide.
        let (x0, cols) = (3u16, 80.0);
        let (lo, hi) = RANGE_DB;
        let mut seen = std::collections::HashSet::new();
        for y in 1..9u16 {
            for x in x0..x0 + 80 {
                let c = &buf[(x, y)];
                if !EIGHTHS[1..].contains(&c.symbol()) {
                    continue;
                }
                let snr = lo + (f64::from(x - x0) + 0.5) / cols * (hi - lo);
                assert_eq!(c.fg, snr_colour(&theme, snr), "at {x},{y}");
                seen.insert(format!("{:?}", c.fg));
            }
        }
        assert!(seen.len() > 3, "{seen:?}");
    }
}
