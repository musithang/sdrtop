// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The hour in numbers: distinct stations each quarter hour, decodes a
//! cycle (median and most), and the share of the decodes held that call CQ.
//!
//! Nothing here needs your position. The CQ share is of the decodes the list
//! still holds, and says so; how many calls are new this hour is not shown,
//! because the state keeps no time a station was first heard, and it is not
//! guessed (POLICY.md, rule 2).

use std::collections::BTreeSet;

use ratatui::{layout::Rect, style::Style, Frame};

use super::{say, window};
use crate::state::SdrMetrics;
use crate::ui::panel::{Panel, PanelChrome, Staleness};
use crate::ui::panels::radio::ft8::put;

pub struct Ft8HourPanel;

/// The hour's figures.
#[derive(Clone, Debug, PartialEq)]
pub struct Figures {
    /// (quarter hour's start, distinct stations heard in it).
    pub per_quarter: Vec<(f64, usize)>,
    pub median_decodes: usize,
    pub most_decodes: usize,
    /// (calling CQ, of how many decodes held).
    pub cq: (usize, usize),
}

/// The hour's figures, or `None` before any cycle.
pub fn figures(m: &SdrMetrics) -> Option<Figures> {
    let (t0, t1) = window(m)?;
    let mut per_quarter = Vec::new();
    let mut b = (t0 / 900.0).floor() * 900.0;
    while b < t1 {
        let calls: BTreeSet<&str> = m
            .ft8
            .history
            .iter()
            .filter(|r| r.cycle_unix >= b && r.cycle_unix < b + 900.0)
            .filter_map(|r| r.call.as_deref())
            .collect();
        per_quarter.push((b, calls.len()));
        b += 900.0;
    }
    let mut counts: Vec<usize> = m.ft8.cycle_hour.iter().map(|c| c.decodes).collect();
    counts.sort_unstable();
    let cq = m
        .ft8
        .decodes
        .iter()
        .filter(|d| d.decode.message.text().starts_with("CQ"))
        .count();
    Some(Figures {
        per_quarter,
        median_decodes: counts.get(counts.len() / 2).copied().unwrap_or(0),
        most_decodes: counts.last().copied().unwrap_or(0),
        cq: (cq, m.ft8.decodes.len()),
    })
}

fn hhmm(unix: f64) -> String {
    let s = unix.rem_euclid(86_400.0) as u64;
    format!("{:02}:{:02}", s / 3600, s / 60 % 60)
}

impl Panel for Ft8HourPanel {
    fn name(&self) -> &'static str {
        "ft8_hour"
    }

    fn reads_spectrum(&self) -> bool {
        false
    }

    fn min_size(&self) -> (u16, u16) {
        (36, 5)
    }

    fn column_height(&self, _column: u16) -> Option<u16> {
        Some(5)
    }

    fn chrome(&self, _state: &SdrMetrics) -> PanelChrome {
        PanelChrome::new("The hour").stale_when(Staleness::NotStreaming)
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
        let Some(fig) = figures(state) else {
            say(buf, area, "waiting for the first cycle", theme);
            return;
        };
        let label = Style::default().fg(theme.label);
        let value = Style::default().fg(theme.value);
        let x = area.x + 1;
        let xv = x + 17;
        // Stations a quarter hour: as many quarters as fit, newest last.
        put(buf, area, x, area.y, "stations / 15 min", label);
        let room = usize::from(area.right().saturating_sub(xv + 1));
        let items: Vec<String> = fig
            .per_quarter
            .iter()
            .map(|(b, n)| format!("{} {n}", hhmm(*b)))
            .collect();
        let mut shown: Vec<&String> = Vec::new();
        let mut used = 0;
        for item in items.iter().rev() {
            let w = item.chars().count() + 2;
            if used + w > room {
                break;
            }
            used += w;
            shown.push(item);
        }
        shown.reverse();
        let joined = shown
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("  ");
        put(buf, area, xv + 1, area.y, &joined, value);
        put(buf, area, x, area.y + 1, "decodes a cycle", label);
        put(
            buf,
            area,
            xv + 1,
            area.y + 1,
            &format!("median {}  most {}", fig.median_decodes, fig.most_decodes),
            value,
        );
        put(buf, area, x, area.y + 2, "calling CQ", label);
        let (cq, held) = fig.cq;
        let share = if held == 0 {
            "no decodes held".to_string()
        } else {
            format!(
                "{:.0} % of the {held} held",
                100.0 * cq as f64 / held as f64
            )
        };
        put(buf, area, xv + 1, area.y + 2, &share, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::fixture::draw;

    #[test]
    fn the_figures_come_from_the_hour() {
        let m = SdrMetrics::fixture().streaming().ft8_hour();
        let fig = figures(&m).unwrap();
        assert_eq!(fig.per_quarter.len(), 4);
        let mut counts: Vec<usize> = m.ft8.cycle_hour.iter().map(|c| c.decodes).collect();
        counts.sort_unstable();
        assert_eq!(fig.most_decodes, *counts.last().unwrap());
        assert_eq!(fig.median_decodes, counts[counts.len() / 2]);
        assert!(fig
            .per_quarter
            .iter()
            .all(|(_, n)| *n <= m.ft8.stations.len()));
    }

    /// Narrow, the newest quarters are kept and nothing runs past the frame.
    #[test]
    fn narrow_it_keeps_the_newest_quarters() {
        let m = SdrMetrics::fixture().streaming().ft8_hour();
        let fig = figures(&m).unwrap();
        let newest = hhmm(fig.per_quarter.last().unwrap().0);
        let lines = draw(Ft8HourPanel, 44, 5, &m);
        assert!(lines[1].contains(&newest), "{}", lines[1]);
        assert!(lines[1].ends_with('│'), "{}", lines[1]);
    }

    #[test]
    fn with_nothing_held_the_cq_share_says_so() {
        let m = SdrMetrics::fixture().streaming().ft8_hour();
        let all = draw(Ft8HourPanel, 64, 5, &m).join("\n");
        assert!(all.contains("no decodes held"), "{all}");
    }
}
