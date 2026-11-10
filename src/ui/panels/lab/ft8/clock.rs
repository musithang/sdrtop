// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The clock as the stations heard judge it, over the hour: every decode's
//! DT in a histogram from -1 to +1 s, the median marked, the verdict on the
//! first line.
//!
//! **What it measures, said every time** (POLICY.md, rule 5): a DT is late
//! or early against this receiver's stamp of the cycle, so the median is
//! this computer's clock error and the receive path's delay together (USB,
//! the driver, the queues), which no receiver can tell apart. The verdict is
//! the same computation as the cycle row's ([`crate::state::ft8::verdict`]),
//! over the hour where the cycle row takes the last few cycles, and each
//! names its window. Past a second FT8 starts losing decodes, past two it
//! stops, and the verdict turns colour at those marks; decodes past +-1 s
//! are counted, not drawn off the edge.
//!
//! Distance does not enter: over two hours of 40 m the slope of DT against
//! distance came out far inside its own error (-82 +- 136 ms per 1 000 km
//! against a path delay of 3.3), the stations' own clocks drowning it, so no
//! such line is drawn.

use ratatui::{layout::Rect, style::Style, Frame};

use super::say;
use crate::state::ft8::{verdict, CLOCK_CRIT_S, CLOCK_WARN_S, VERDICT_MIN};
use crate::state::SdrMetrics;
use crate::ui::panel::{Panel, PanelChrome, Staleness};
use crate::ui::panels::radio::ft8::{put, signed};

pub struct Ft8ClockPanel;

/// The histogram's reach, seconds either side.
pub const SPAN_S: f64 = 1.0;

const EIGHTHS: [&str; 9] = [" ", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];

/// The hour's DTs, oldest first.
pub fn hour_dts(m: &SdrMetrics) -> Vec<f64> {
    m.ft8.history.iter().map(|r| f64::from(r.dt)).collect()
}

/// Counts in `cols` bins over -[`SPAN_S`] to +[`SPAN_S`], and how many fell
/// beyond.
pub fn bins(dts: &[f64], cols: usize) -> (Vec<usize>, usize) {
    let mut out = vec![0usize; cols.max(1)];
    let mut beyond = 0;
    for &d in dts {
        if d.abs() > SPAN_S {
            beyond += 1;
            continue;
        }
        let c = (((d + SPAN_S) / (2.0 * SPAN_S)) * cols as f64) as usize;
        out[c.min(cols - 1)] += 1;
    }
    (out, beyond)
}

impl Panel for Ft8ClockPanel {
    fn name(&self) -> &'static str {
        "ft8_clock"
    }

    fn reads_spectrum(&self) -> bool {
        false
    }

    fn min_size(&self) -> (u16, u16) {
        (50, 8)
    }

    fn chrome(&self, _state: &SdrMetrics) -> PanelChrome {
        let mut chrome = PanelChrome::new("Clock as heard").stale_when(Staleness::NotStreaming);
        chrome.suffix =
            Some("  the hour · this computer's clock and the receive path together".to_string());
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
        let dts = hour_dts(state);
        if dts.is_empty() {
            say(buf, area, "waiting for a decode", theme);
            return;
        }
        if area.height < 5 {
            say(buf, area, "too short for the clock", theme);
            return;
        }
        let label = Style::default().fg(theme.label);
        let x = area.x + 1;
        let y = area.y;
        // The verdict.
        match verdict(&dts) {
            Some(v) => {
                let mag = v.median_dt.abs();
                let ink = if mag <= CLOCK_WARN_S {
                    theme.value_hi
                } else if mag <= CLOCK_CRIT_S {
                    theme.status_warn
                } else {
                    theme.status_crit
                };
                let side = if v.median_dt >= 0.0 { "late" } else { "early" };
                let xx = put(buf, area, x, y, "median ", label);
                let xx = put(
                    buf,
                    area,
                    xx,
                    y,
                    &format!("{} s", signed(v.median_dt, 2)),
                    Style::default().fg(ink),
                );
                put(
                    buf,
                    area,
                    xx,
                    y,
                    &format!(
                        " ±{:.2} {side}, by {} decodes this hour",
                        v.spread, v.decodes
                    ),
                    label,
                );
            }
            None => {
                put(
                    buf,
                    area,
                    x,
                    y,
                    &format!(
                        "not enough decodes to judge it: {} of {VERDICT_MIN}",
                        dts.len()
                    ),
                    label,
                );
            }
        }
        // The histogram.
        let x0 = x + 5;
        let cols = usize::from(area.width.saturating_sub(7)).max(10);
        let rows = area.height - 4;
        let (counts, beyond) = bins(&dts, cols);
        let top = counts.iter().copied().max().unwrap_or(0).max(1) as f64;
        let value = Style::default().fg(theme.value);
        for (c, &n) in counts.iter().enumerate() {
            let full = n as f64 / top * f64::from(rows) * 8.0;
            for r in 0..rows {
                let lo = f64::from(rows - 1 - r) * 8.0;
                let e = (full - lo).clamp(0.0, 8.0).round() as usize;
                if e > 0 {
                    put(buf, area, x0 + c as u16, y + 2 + r, EIGHTHS[e], value);
                }
            }
        }
        put(
            buf,
            area,
            x,
            y + 2,
            &format!("{top:.0}"),
            Style::default().fg(theme.border_dim),
        );
        let col_of = |d: f64| -> u16 {
            x0 + ((((d + SPAN_S) / (2.0 * SPAN_S)) * cols as f64) as usize).min(cols - 1) as u16
        };
        if let Some(v) = verdict(&dts) {
            if v.median_dt.abs() <= SPAN_S {
                put(
                    buf,
                    area,
                    col_of(v.median_dt),
                    y + 1,
                    "▼",
                    Style::default().fg(theme.value_hi),
                );
            }
        }
        let axis = y + 2 + rows;
        put(buf, area, x0, axis, "-1 s", label);
        put(
            buf,
            area,
            col_of(-0.5).saturating_sub(1),
            axis,
            "-0.5",
            label,
        );
        put(buf, area, col_of(0.0), axis, "0", label);
        put(
            buf,
            area,
            col_of(0.5).saturating_sub(1),
            axis,
            "+0.5",
            label,
        );
        put(
            buf,
            area,
            (x0 + cols as u16).saturating_sub(4),
            axis,
            "+1 s",
            label,
        );
        let bin_ms = 2000.0 * SPAN_S / cols as f64;
        let foot = axis + 1;
        let xx = put(
            buf,
            area,
            x0,
            foot,
            &format!("bins of {bin_ms:.0} ms"),
            label,
        );
        if beyond > 0 {
            put(
                buf,
                area,
                xx + 3,
                foot,
                &format!("{beyond} beyond ±1 s, where FT8 starts losing decodes"),
                Style::default().fg(theme.status_warn),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::fixture::draw;

    #[test]
    fn bins_count_inside_and_beyond() {
        let (b, beyond) = bins(&[-1.5, -0.99, 0.0, 0.01, 0.999, 1.2], 100);
        assert_eq!(beyond, 2);
        assert_eq!(b.iter().sum::<usize>(), 4);
        assert_eq!(b[0], 1);
        assert_eq!(b[50], 2);
        assert_eq!(b[99], 1);
    }

    /// The verdict is the cycle row's computation over the hour, named so.
    #[test]
    fn the_verdict_is_the_shared_one_over_the_hour() {
        let m = SdrMetrics::fixture().streaming().ft8_hour();
        let v = verdict(&hour_dts(&m)).unwrap();
        let lines = draw(Ft8ClockPanel, 104, 18, &m);
        assert!(lines[0].contains("the hour"), "{}", lines[0]);
        assert!(
            lines[1].contains(&format!(
                "median {} s ±{:.2}",
                signed(v.median_dt, 2),
                v.spread
            )),
            "{}",
            lines[1]
        );
        assert!(lines[1].contains(&format!("by {} decodes this hour", v.decodes)));
        // 104 wide: 102 inside, 95 bins over two seconds.
        assert!(lines
            .join("\n")
            .contains(&format!("bins of {:.0} ms", 2000.0 / 95.0)));
    }

    #[test]
    fn decodes_past_a_second_are_counted_not_drawn() {
        let mut m = SdrMetrics::fixture().streaming().ft8_hour();
        let h = std::sync::Arc::make_mut(&mut m.ft8.history);
        for r in h.iter_mut().take(3) {
            r.dt = 1.6;
        }
        let all = draw(Ft8ClockPanel, 104, 18, &m).join("\n");
        assert!(all.contains("3 beyond ±1 s"), "{all}");
    }

    #[test]
    fn with_too_few_decodes_there_is_no_verdict() {
        let mut m = SdrMetrics::fixture().streaming().ft8_hour();
        let h = std::sync::Arc::make_mut(&mut m.ft8.history);
        h.truncate(3);
        let all = draw(Ft8ClockPanel, 104, 18, &m).join("\n");
        assert!(
            all.contains("not enough decodes to judge it: 3 of 5"),
            "{all}"
        );
        assert!(!all.contains("median"), "{all}");
    }

    #[test]
    fn before_any_decode_it_waits() {
        let all = draw(Ft8ClockPanel, 104, 18, &SdrMetrics::fixture()).join("\n");
        assert!(all.contains("waiting for a decode"), "{all}");
    }
}
