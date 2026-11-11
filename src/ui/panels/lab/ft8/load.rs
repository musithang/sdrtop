// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The decoder's load: how long each cycle's decode took against the 15 s a
//! cycle gives, the ring's cycles as a two-row sparkline (a cycle the
//! decoder had no room for a full column in warning ink, one a break in the
//! stream cut a `×` in critical ink), the median and the slowest in words,
//! and the feed's own account of what it lost. On this machine, now: a measurement, not a
//! benchmark.

use ratatui::{layout::Rect, style::Style, Frame};

use super::say;
use crate::state::ft8::CycleOutcome;
use crate::state::SdrMetrics;
use crate::ui::panel::{Panel, PanelChrome, Staleness};
use crate::ui::panels::radio::ft8::put;

pub struct Ft8LoadPanel;

/// The seconds a cycle gives.
pub const CYCLE_S: f64 = 15.0;

const EIGHTHS: [&str; 9] = [" ", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];

/// The ring's decode times: median and slowest, seconds; skipped and cut
/// cycles.
pub fn times(m: &SdrMetrics) -> Option<(f64, f64, usize, usize)> {
    let mut s: Vec<f64> = m
        .ft8
        .cycles
        .iter()
        .filter(|c| c.outcome == CycleOutcome::Decoded)
        .map(|c| c.seconds)
        .collect();
    if s.is_empty() {
        return None;
    }
    s.sort_by(f64::total_cmp);
    let skipped = m
        .ft8
        .cycles
        .iter()
        .filter(|c| c.outcome == CycleOutcome::Skipped)
        .count();
    let cut = m
        .ft8
        .cycles
        .iter()
        .filter(|c| c.outcome == CycleOutcome::Cut)
        .count();
    Some((s[s.len() / 2], s[s.len() - 1], skipped, cut))
}

impl Panel for Ft8LoadPanel {
    fn name(&self) -> &'static str {
        "ft8_load"
    }

    fn reads_spectrum(&self) -> bool {
        false
    }

    fn min_size(&self) -> (u16, u16) {
        (40, 7)
    }

    fn column_height(&self, _column: u16) -> Option<u16> {
        Some(7)
    }

    fn chrome(&self, _state: &SdrMetrics) -> PanelChrome {
        let mut chrome = PanelChrome::new("Load").stale_when(Staleness::NotStreaming);
        chrome.suffix = Some("  decode time a cycle, on this machine".to_string());
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
        let Some((median, slowest, skipped, cut)) = times(state) else {
            say(buf, area, "waiting for a decoded cycle", theme);
            return;
        };
        let x = area.x + 1;
        // The ring's cycles, oldest at the left, two rows tall for the 15 s
        // a cycle gives: a decoded cycle its decode time, a skipped one a
        // full column in warning ink (the decoder had no room), a cut one a
        // `×` (the stream broke in it).
        let plot_w = usize::from(area.width.saturating_sub(7).max(4));
        let ring: Vec<_> = state.ft8.cycles.iter().collect();
        let shown = ring.len().min(plot_w);
        let hi = Style::default().fg(theme.value_hi);
        let warn = Style::default().fg(theme.status_warn);
        for (k, c) in ring[ring.len() - shown..].iter().enumerate() {
            // Spread evenly over the width, a gap between cycles where a
            // cycle has three cells or more.
            let from = k * plot_w / shown;
            let cells = (k + 1) * plot_w / shown - from;
            let cells = cells - usize::from(cells > 2);
            let (eighths, style, cut) = match c.outcome {
                CycleOutcome::Decoded => (
                    ((c.seconds / CYCLE_S).clamp(0.0, 1.0) * 16.0)
                        .round()
                        .max(1.0) as usize,
                    hi,
                    false,
                ),
                CycleOutcome::Skipped => (16, warn, false),
                _ => (0, Style::default().fg(theme.status_crit), true),
            };
            for dx in 0..cells {
                let col = x + (from + dx) as u16;
                if cut {
                    put(buf, area, col, area.y + 1, "×", style);
                    continue;
                }
                put(buf, area, col, area.y + 1, EIGHTHS[eighths.min(8)], style);
                let upper = eighths.saturating_sub(8);
                if upper > 0 {
                    put(buf, area, col, area.y, EIGHTHS[upper], style);
                }
            }
        }
        let end = x + plot_w as u16;
        put(
            buf,
            area,
            end,
            area.y,
            "┤15 s",
            Style::default().fg(theme.border_dim),
        );
        put(
            buf,
            area,
            end,
            area.y + 1,
            "┤0",
            Style::default().fg(theme.border_dim),
        );
        let area = Rect {
            y: area.y + 1,
            height: area.height.saturating_sub(1),
            ..area
        };
        put(
            buf,
            area,
            x,
            area.y + 1,
            "median ",
            Style::default().fg(theme.label),
        );
        let xx = put(buf, area, x + 7, area.y + 1, &format!("{median:.1} s"), hi);
        let xx = put(
            buf,
            area,
            xx,
            area.y + 1,
            " · slowest ",
            Style::default().fg(theme.label),
        );
        let xx = put(buf, area, xx, area.y + 1, &format!("{slowest:.1} s"), hi);
        put(
            buf,
            area,
            xx,
            area.y + 1,
            " of the 15",
            Style::default().fg(theme.label),
        );
        let fell_behind = skipped > 0;
        put(
            buf,
            area,
            x,
            area.y + 2,
            &format!(
                "{} · {cut} cut by the stream",
                if fell_behind {
                    format!("{skipped} skipped, the decoder still busy")
                } else {
                    "none skipped".to_string()
                }
            ),
            Style::default().fg(if fell_behind {
                theme.status_warn
            } else {
                theme.value
            }),
        );
        let feed = &state.ft8.feed;
        put(
            buf,
            area,
            x,
            area.y + 3,
            &format!(
                "feed: {} breaks, {} blocks lost, {} refused",
                feed.gaps, feed.blocks_lost, feed.refused_session
            ),
            Style::default().fg(theme.label),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::fixture::draw;

    #[test]
    fn it_reads_the_rings_decode_times() {
        let mut m = SdrMetrics::fixture().streaming().ft8_traced();
        let base = m.ft8.cycles[0].clone();
        m.ft8.cycles.clear();
        for (k, s) in [1.0, 2.0, 9.0].into_iter().enumerate() {
            let mut c = base.clone();
            c.start_unix += 15.0 * k as f64;
            c.seconds = s;
            m.ft8.cycles.push_back(c);
        }
        let mut skipped = base.clone();
        skipped.outcome = CycleOutcome::Skipped;
        m.ft8.cycles.push_back(skipped);
        assert_eq!(times(&m), Some((2.0, 9.0, 1, 0)));
        let all = draw(Ft8LoadPanel, 86, 7, &m).join("\n");
        assert!(all.contains("median 2.0 s · slowest 9.0 s"), "{all}");
        assert!(all.contains("1 skipped, the decoder still busy"), "{all}");
    }

    #[test]
    fn before_a_decoded_cycle_it_waits() {
        let all = draw(Ft8LoadPanel, 86, 7, &SdrMetrics::fixture()).join("\n");
        assert!(all.contains("waiting for a decoded cycle"), "{all}");
    }

    /// The ring's cycles as a sparkline: a decoded cycle its time, a skipped
    /// one a full column in warning ink, a cut one a `×` in critical ink.
    #[test]
    fn the_sparkline_marks_skipped_and_cut_cycles() {
        let mut m = SdrMetrics::fixture().streaming().ft8_traced();
        let theme = crate::Theme::sdr();
        let base = m.ft8.cycles[0].clone();
        m.ft8.cycles.clear();
        for (k, outcome) in [
            CycleOutcome::Decoded,
            CycleOutcome::Skipped,
            CycleOutcome::Decoded,
            CycleOutcome::Cut,
        ]
        .into_iter()
        .enumerate()
        {
            let mut c = base.clone();
            c.start_unix += 15.0 * k as f64;
            c.seconds = 3.0;
            c.outcome = outcome;
            m.ft8.cycles.push_back(c);
        }
        let buf = crate::state::fixture::draw_buffer(Ft8LoadPanel, 86, 7, &m);
        let cells = |y: u16| (0..86u16).map(move |x| (x, y));
        let cut: Vec<_> = cells(2).filter(|&p| buf[p].symbol() == "×").collect();
        assert!(!cut.is_empty());
        assert!(cut.iter().all(|&p| buf[p].fg == theme.status_crit));
        // A skipped cycle is full in both rows, in warning ink.
        let full = cells(1)
            .filter(|&(x, y)| {
                buf[(x, y)].symbol() == "█"
                    && buf[(x, y)].fg == theme.status_warn
                    && buf[(x, y + 1)].symbol() == "█"
            })
            .count();
        assert!(full > 0);
        // A 3 s cycle is low: nothing in the top row, a fifth of the bar.
        let decoded = cells(2)
            .filter(|&p| buf[p].fg == theme.value_hi && buf[p].symbol() == "▃")
            .count();
        assert!(decoded > 0);
        let all = draw(Ft8LoadPanel, 86, 7, &m).join("\n");
        assert!(all.contains("┤15 s") && all.contains("┤0"), "{all}");
    }
}
