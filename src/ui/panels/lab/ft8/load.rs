// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The decoder's load: how long a cycle's decode took against the 15 s a
//! cycle gives (the median a bar, the slowest beyond it), the cycles it had
//! no room for and those a break in the stream cut, and the feed's own
//! account of what it lost. On this machine, now: a measurement, not a
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
        (40, 6)
    }

    fn column_height(&self, _column: u16) -> Option<u16> {
        Some(6)
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
        let w = f64::from(area.width.saturating_sub(8).max(4));
        let cells = |s: f64| ((s / CYCLE_S).clamp(0.0, 1.0) * w).round() as usize;
        let end = put(
            buf,
            area,
            x,
            area.y,
            &"█".repeat(cells(median)),
            Style::default().fg(theme.value),
        );
        put(
            buf,
            area,
            end,
            area.y,
            &"▒".repeat(cells(slowest).saturating_sub(cells(median))),
            Style::default().fg(theme.border_default),
        );
        put(
            buf,
            area,
            x + w as u16,
            area.y,
            "│15 s",
            Style::default().fg(theme.status_warn),
        );
        put(
            buf,
            area,
            x,
            area.y + 1,
            &format!("median {median:.1} s · slowest {slowest:.1} s of the 15"),
            Style::default().fg(theme.value),
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
        let all = draw(Ft8LoadPanel, 86, 6, &m).join("\n");
        assert!(all.contains("median 2.0 s · slowest 9.0 s"), "{all}");
        assert!(all.contains("1 skipped, the decoder still busy"), "{all}");
    }

    #[test]
    fn before_a_decoded_cycle_it_waits() {
        let all = draw(Ft8LoadPanel, 86, 6, &SdrMetrics::fixture()).join("\n");
        assert!(all.contains("waiting for a decoded cycle"), "{all}");
    }
}
