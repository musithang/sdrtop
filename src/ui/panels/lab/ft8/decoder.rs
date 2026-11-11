// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The decoder at work: what became of its candidates over the cycles the
//! ring holds, and how hard the decodes it still holds were to read.
//!
//! The funnel ([`crate::signal::ft8::cycle::Funnel`]) runs from the search's
//! candidates through those in sync and those read to a codeword whose check
//! passed, to the new decodes; its bars are on a square-root scale, because
//! the first stage outnumbers the last a thousand to one. Each stage is a
//! step brighter than the one before, the decodes in the theme's "good",
//! and each count after the first carries the share of the stage before
//! that got through. Then what belief
//! propagation finished and what ordered statistics did, what was refused
//! and why, and each subtraction pass's yield.
//!
//! The held decodes by effort are one bar split in effort's order, easiest
//! first, OSD last in warning ink, under a legend naming every part.
//!
//! **The share needing OSD is shown plainly.** Ordered statistics decoding
//! is where false decodes come from; a band on which a large share needs it
//! is a band to read with care, and the panel says the share rather than
//! judging it.

use ratatui::{layout::Rect, style::Style, Frame};

use super::say;
use crate::signal::ft8::cycle::Funnel;
use crate::state::ft8::CycleOutcome;
use crate::state::SdrMetrics;
use crate::ui::panel::{Panel, PanelChrome, Staleness};
use crate::ui::panels::radio::ft8::put;

pub struct Ft8DecoderPanel;

/// The funnel over the ring's decoded cycles, summed, and each pass's new
/// decodes (first, second, third).
pub fn summed(m: &SdrMetrics) -> (Funnel, [usize; 3], usize) {
    let mut sum = Funnel::default();
    let mut by_pass = [0usize; 3];
    let mut cycles = 0;
    for c in m
        .ft8
        .cycles
        .iter()
        .filter(|c| c.outcome == CycleOutcome::Decoded)
    {
        cycles += 1;
        for (p, f) in c.funnel.iter().enumerate() {
            sum.candidates += f.candidates;
            sum.synced += f.synced;
            sum.coded += f.coded;
            sum.by_bp += f.by_bp;
            sum.by_osd += f.by_osd;
            sum.duplicates += f.duplicates;
            sum.refused_zero += f.refused_zero;
            sum.refused_osd += f.refused_osd;
            if let Some(slot) = by_pass.get_mut(p) {
                *slot += f.by_bp + f.by_osd;
            }
        }
    }
    (sum, by_pass, cycles)
}

/// The held decodes by effort: BP at once, 1-3 iterations, 4-10, 11 and
/// more, OSD.
pub fn effort(m: &SdrMetrics) -> [usize; 5] {
    let mut out = [0usize; 5];
    for d in &m.ft8.decodes {
        let k = match (d.decode.osd, d.decode.iterations) {
            (Some(_), _) => 4,
            (None, 0) => 0,
            (None, 1..=3) => 1,
            (None, 4..=10) => 2,
            (None, _) => 3,
        };
        out[k] += 1;
    }
    out
}

impl Panel for Ft8DecoderPanel {
    fn name(&self) -> &'static str {
        "ft8_decoder"
    }

    fn reads_spectrum(&self) -> bool {
        false
    }

    fn min_size(&self) -> (u16, u16) {
        (50, 8)
    }

    fn column_height(&self, _column: u16) -> Option<u16> {
        Some(12)
    }

    fn chrome(&self, state: &SdrMetrics) -> PanelChrome {
        let (_, _, cycles) = summed(state);
        let mut chrome = PanelChrome::new("Decoder").stale_when(Staleness::NotStreaming);
        chrome.suffix = Some(format!("  the last {cycles} decoded cycles"));
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
        let (sum, by_pass, cycles) = summed(state);
        if cycles == 0 {
            say(buf, area, "waiting for a decoded cycle", theme);
            return;
        }
        let label = Style::default().fg(theme.label);
        let value = Style::default().fg(theme.value);
        let hi = Style::default().fg(theme.value_hi);
        let dim = Style::default().fg(theme.border_dim);
        let x = area.x + 1;
        let bar_x = x + 12;
        let bar_w = f64::from(area.width.saturating_sub(26).max(4));
        let top = (sum.candidates.max(1) as f64).sqrt();
        // Each stage a step brighter than the one it came from, the decodes
        // in the theme's own "good"; beside each count, the share of the
        // stage before that got through.
        let rows = [
            ("candidates", sum.candidates, theme.border_default),
            ("in sync", sum.synced, theme.value),
            ("a codeword", sum.coded, theme.value_hi),
            ("new decodes", sum.by_bp + sum.by_osd, theme.status_ok),
        ];
        let mut y = area.y;
        let mut before: Option<usize> = None;
        for (name, n, colour) in rows {
            if y >= area.bottom() {
                return;
            }
            put(buf, area, x, y, name, label);
            let len = (((n as f64).sqrt() / top) * bar_w).round() as usize;
            let len = len.max(usize::from(n > 0));
            let end = put(
                buf,
                area,
                bar_x,
                y,
                &"█".repeat(len),
                Style::default().fg(colour),
            );
            let end = put(buf, area, end + 1, y, &format!("{n}"), value);
            if let Some(b) = before.filter(|&b| b > 0) {
                put(
                    buf,
                    area,
                    end + 2,
                    y,
                    &format!("{:.0} %", 100.0 * n as f64 / b as f64),
                    dim,
                );
            }
            before = Some(n);
            y += 1;
        }
        let held = state.ft8.decodes.len();
        let e = effort(state);
        let lines: Vec<Vec<(String, Style)>> = vec![
            vec![
                (" ".repeat(12), dim),
                ("bars on a square-root scale".into(), dim),
            ],
            vec![
                ("BP ".into(), label),
                (sum.by_bp.to_string(), hi),
                ("  OSD ".into(), label),
                (sum.by_osd.to_string(), hi),
                ("  found again ".into(), label),
                (sum.duplicates.to_string(), hi),
                ("  refused: empty word ".into(), label),
                (sum.refused_zero.to_string(), hi),
                (", OSD out of sync ".into(), label),
                (sum.refused_osd.to_string(), hi),
            ],
            vec![
                ("new by pass  1st ".into(), label),
                (by_pass[0].to_string(), hi),
                ("  2nd ".into(), label),
                (by_pass[1].to_string(), hi),
                ("  3rd ".into(), label),
                (by_pass[2].to_string(), hi),
                ("  (each under what the passes before took out)".into(), dim),
            ],
        ];
        // The held decodes by effort, as one bar split in the effort's
        // order, easiest first, OSD last in the colour of care.
        let inks = [
            theme.status_ok,
            theme.value_hi,
            theme.value,
            theme.border_default,
            theme.status_warn,
        ];
        let names = ["at once", "1-3", "4-10", "11+", "OSD"];
        for line in &lines {
            if y >= area.bottom() {
                return;
            }
            let mut xx = x;
            for (t, st) in line {
                xx = put(buf, area, xx, y, t, *st);
            }
            y += 1;
        }
        if held == 0 || y >= area.bottom() {
            return;
        }
        put(buf, area, x, y, "effort", label);
        let w = usize::from(area.width.saturating_sub(14).max(5));
        let mut xx = bar_x;
        let mut done = 0usize;
        for (k, &n) in e.iter().enumerate() {
            // Cumulative rounding so the segments add up to the bar.
            let to = ((done + n) * w + held / 2) / held;
            let from = (done * w + held / 2) / held;
            xx = put(
                buf,
                area,
                xx,
                y,
                &"█".repeat(to - from),
                Style::default().fg(inks[k]),
            );
            done += n;
        }
        y += 1;
        if y < area.bottom() {
            let mut xx = bar_x;
            for k in 0..5 {
                xx = put(buf, area, xx, y, "■ ", Style::default().fg(inks[k]));
                xx = put(buf, area, xx, y, &format!("{} ", names[k]), label);
                xx = put(buf, area, xx, y, &format!("{}   ", e[k]), hi);
            }
        }
        y += 1;
        if y < area.bottom() {
            let share = 100.0 * e[4] as f64 / held as f64;
            let xx = put(
                buf,
                area,
                x,
                y,
                &format!("{share:.0} %"),
                Style::default().fg(if e[4] > 0 {
                    theme.status_warn
                } else {
                    theme.value_hi
                }),
            );
            put(
                buf,
                area,
                xx,
                y,
                &format!(" of the {held} held needed OSD, where false decodes come from"),
                label,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::fixture::draw;

    /// The funnel sums the ring's decoded cycles and its new decodes are
    /// the decodes those cycles published.
    #[test]
    fn the_funnel_sums_the_rings_cycles() {
        let m = SdrMetrics::fixture().streaming().ft8_traced();
        let (sum, by_pass, cycles) = summed(&m);
        assert_eq!(cycles, 1);
        assert_eq!(sum.by_bp + sum.by_osd, m.ft8.decodes.len());
        assert_eq!(by_pass.iter().sum::<usize>(), m.ft8.decodes.len());
        assert!(sum.candidates >= sum.synced && sum.synced >= sum.coded);
    }

    #[test]
    fn the_effort_counts_every_held_decode_once() {
        let m = SdrMetrics::fixture().streaming().ft8_traced();
        assert_eq!(effort(&m).iter().sum::<usize>(), m.ft8.decodes.len());
    }

    #[test]
    fn it_shows_the_funnel_and_the_osd_share() {
        let m = SdrMetrics::fixture().streaming().ft8_traced();
        let (sum, _, _) = summed(&m);
        let all = draw(Ft8DecoderPanel, 104, 14, &m).join("\n");
        assert!(all.contains(&format!("{}", sum.candidates)), "{all}");
        assert!(
            all.contains("needed OSD, where false decodes come from"),
            "{all}"
        );
        assert!(all.contains("the last 1 decoded cycles"), "{all}");
    }

    #[test]
    fn before_a_decoded_cycle_it_waits() {
        let all = draw(Ft8DecoderPanel, 104, 14, &SdrMetrics::fixture()).join("\n");
        assert!(all.contains("waiting for a decoded cycle"), "{all}");
    }

    /// Each stage after the first says the share of the one before that got
    /// through; the effort is one bar split in its order, easiest first,
    /// under a legend naming every part.
    #[test]
    fn the_funnel_gives_its_shares_and_the_effort_is_one_split_bar() {
        let m = SdrMetrics::fixture().streaming().ft8_traced();
        let theme = crate::Theme::sdr();
        let (sum, _, _) = summed(&m);
        let all = draw(Ft8DecoderPanel, 104, 14, &m).join("\n");
        let share = format!("{:.0} %", 100.0 * sum.synced as f64 / sum.candidates as f64);
        assert!(all.contains(&format!("{}  {share}", sum.synced)), "{all}");
        for name in ["at once", "1-3", "4-10", "11+", "OSD"] {
            assert!(all.contains(&format!("■ {name} ")), "{name}: {all}");
        }
        let buf = crate::state::fixture::draw_buffer(Ft8DecoderPanel, 104, 14, &m);
        let row = |y: u16| (0..104).map(|x| buf[(x, y)].symbol()).collect::<String>();
        let y = (0..14)
            .find(|&y| row(y).contains("effort"))
            .expect("an effort row");
        let inks: Vec<_> = (0..104u16)
            .map(|x| &buf[(x, y)])
            .filter(|c| c.symbol() == "█")
            .map(|c| c.fg)
            .collect();
        // The fixture's held decodes: five at once, one in 1-3.
        let e = effort(&m);
        assert_eq!((e[0], e[1], e[2] + e[3] + e[4]), (5, 1, 0));
        assert_eq!(inks.first(), Some(&theme.status_ok));
        assert_eq!(inks.last(), Some(&theme.value_hi));
        let ok = inks.iter().filter(|&&c| c == theme.status_ok).count();
        assert!(ok > 4 * (inks.len() - ok), "{ok} of {}", inks.len());
    }
}
