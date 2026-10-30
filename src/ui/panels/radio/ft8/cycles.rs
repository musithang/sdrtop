// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! One line: the last minute's cycles, the one being heard, and the clock.
//!
//! **Every cycle says what became of it**: its decode count, or that it was
//! cut by a break in the stream, or skipped while the decoder was still busy.
//! A cut cycle is never shown as a quiet one (POLICY.md, rule 4).
//!
//! **The clock, as the stations heard judge it.** The median DT of the
//! recent decodes says how late or early this receiver stamps its samples
//! against the crowd's transmissions. It measures this computer's clock and
//! the receive path's delay together, which no receiver can tell apart, and
//! the line says both (rule 5). Beyond a second FT8 starts to lose decodes;
//! beyond two it stops.

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    Frame,
};

use super::{hhmmss, put};
use crate::state::{CycleOutcome, SdrMetrics};
use crate::ui::panel::{FrameStyle, Panel, PanelChrome, Staleness};

pub struct Ft8CyclesPanel;

/// Completed cycles shown before the one being heard.
const SHOWN: usize = 4;

impl Panel for Ft8CyclesPanel {
    fn name(&self) -> &'static str {
        "ft8_cycles"
    }

    fn reads_spectrum(&self) -> bool {
        false
    }

    fn min_size(&self) -> (u16, u16) {
        (60, 1)
    }

    fn chrome(&self, _state: &SdrMetrics) -> PanelChrome {
        PanelChrome::untitled()
            .frame(FrameStyle::Borderless)
            .stale_when(Staleness::NotStreaming)
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
        let ft8 = &state.ft8;
        let y = area.y;
        let label = Style::default().fg(theme.label);
        // A rate with no route to 12 000 samples a second is refused, and
        // said here rather than left as a quiet strip.
        if let Some(rate) = ft8.unreachable_rate {
            put(
                buf,
                area,
                area.x + 1,
                y,
                &format!("This radio's {rate:.0} S/s has no route to FT8's 12 000; pick another rate with S."),
                Style::default().fg(theme.status_warn),
            );
            return;
        }
        // The band, unmistakable: the first thing on the line, lit, and a
        // tuning that is no FT8 frequency said as such.
        let dial = state.radio.frequency;
        let (badge, ink) = match crate::signal::ft8::bands::band_of(dial) {
            Some(band) => (
                format!(" {band} \u{b7} {:.3} MHz ", dial as f64 / 1e6),
                theme.value_hi,
            ),
            None => (
                format!(
                    " {:.3} MHz: not an FT8 frequency, \u{2190}\u{2192} steps to one ",
                    dial as f64 / 1e6
                ),
                theme.status_warn,
            ),
        };
        let mut x = put(
            buf,
            area,
            area.x,
            y,
            &badge,
            Style::default()
                .fg(ink)
                .add_modifier(Modifier::REVERSED | Modifier::BOLD),
        );
        x = put(buf, area, x + 1, y, "cycle ", label);
        let recent: Vec<_> = ft8.cycles.iter().rev().take(SHOWN).collect();
        for c in recent.iter().rev() {
            let newest = std::ptr::eq(*c, recent[0]);
            let ink = Style::default().fg(if newest { theme.value_hi } else { theme.value });
            x = put(
                buf,
                area,
                x,
                y,
                "\u{2595}",
                Style::default().fg(theme.border_dim),
            );
            x = put(buf, area, x, y, &hhmmss(c.start_unix), ink);
            let what = match c.outcome {
                CycleOutcome::Decoded => format!(" {:>2} ", c.decodes),
                CycleOutcome::Cut => " cut ".to_string(),
                CycleOutcome::Skipped => " skip ".to_string(),
            };
            let what_ink = match c.outcome {
                CycleOutcome::Decoded => ink,
                _ => Style::default().fg(theme.status_warn),
            };
            x = put(buf, area, x, y, &what, what_ink);
            let par = if super::parity(c.start_unix) == "even" {
                "E "
            } else {
                "O "
            };
            x = put(buf, area, x, y, par, label);
        }
        // The cycle being heard: how far into its fifteen seconds.
        if let Some(start) = ft8.hearing {
            let into = ft8
                .heard_until
                .map_or(0.0, |t| (t - start).clamp(0.0, 15.0));
            x = put(
                buf,
                area,
                x,
                y,
                "\u{2595}",
                Style::default().fg(theme.border_dim),
            );
            const BAR: usize = 15;
            let lit = (into / 15.0 * BAR as f64).round() as usize;
            for k in 0..BAR {
                let (ch, c) = if k < lit {
                    ("\u{2588}", theme.border_accent)
                } else {
                    ("\u{2591}", theme.stale)
                };
                x = put(buf, area, x, y, ch, Style::default().fg(c));
            }
            x = put(
                buf,
                area,
                x,
                y,
                "\u{258f} ",
                Style::default().fg(theme.border_dim),
            );
            let par = if super::parity(start) == "even" {
                "E"
            } else {
                "O"
            };
            let decoding = if ft8.decoding { "  decoding" } else { "" };
            x = put(
                buf,
                area,
                x,
                y,
                &format!("{} {par} {into:4.1} s of 15{decoding}", hhmmss(start)),
                label,
            );
            // Every break or refused block costs the cycle it falls in.
            let breaks = ft8.feed.gaps + ft8.feed.refused_session;
            if breaks > 0 {
                x = put(
                    buf,
                    area,
                    x + 2,
                    y,
                    &format!("feed broke {breaks}\u{d7}"),
                    Style::default().fg(theme.status_warn),
                );
            }
        }

        // The clock, right-aligned.
        let parts: Vec<(String, Style)> = match ft8.clock {
            Some(v) => {
                let mag = v.median_dt.abs();
                let ink = if mag <= 1.0 {
                    theme.status_ok
                } else if mag <= 2.0 {
                    theme.status_warn
                } else {
                    theme.status_crit
                };
                let side = if v.median_dt >= 0.0 { "late" } else { "early" };
                vec![
                    ("clock and path ".into(), label),
                    (format!("{:+.2} s", v.median_dt), Style::default().fg(ink)),
                    (
                        format!(" \u{b1}{:.2}", v.spread),
                        Style::default().fg(theme.value),
                    ),
                    (format!(" {side}, by {} decodes", v.decodes), label),
                ]
            }
            None => vec![("clock: too few decodes yet to judge it by".into(), label)],
        };
        let width: usize = parts.iter().map(|(t, _)| t.chars().count()).sum();
        let right = area.right().saturating_sub(1);
        if (width as u16) < area.width.saturating_sub(x - area.x) {
            let mut cx = right.saturating_sub(width as u16);
            for (t, st) in parts {
                cx = put(buf, area, cx, y, &t, st);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::sample::evening;
    use super::*;
    use crate::state::fixture::draw;

    #[test]
    fn the_strip_shows_the_cycles_the_one_heard_and_the_clock() {
        let t = draw(Ft8CyclesPanel, 191, 1, &evening()).join("\n");
        assert!(t.contains(" 20 m \u{b7} 14.074 MHz "), "{t}");
        assert!(t.contains("13:15:00  7 E"), "{t}");
        assert!(t.contains("13:14:45  2 O"), "{t}");
        assert!(t.contains("13:15:15 O  9.2 s of 15"), "{t}");
        assert!(t.contains("clock and path +0.38 s"), "{t}");
        assert!(t.contains("late, by 9 decodes"), "{t}");
    }

    #[test]
    fn a_cut_cycle_says_so_and_too_few_decodes_give_no_verdict() {
        let mut m = evening();
        m.ft8.cycles.back_mut().unwrap().outcome = CycleOutcome::Cut;
        m.ft8.clock = None;
        let t = draw(Ft8CyclesPanel, 191, 1, &m).join("\n");
        assert!(t.contains("13:15:00 cut E"), "{t}");
        assert!(t.contains("too few decodes"), "{t}");
    }

    #[test]
    fn breaks_in_the_feed_and_an_unreachable_rate_are_said() {
        let mut m = evening();
        m.ft8.feed.gaps = 3;
        let t = draw(Ft8CyclesPanel, 191, 1, &m).join("\n");
        assert!(t.contains("feed broke 3\u{d7}"), "{t}");
        m.ft8.unreachable_rate = Some(11_025.0);
        let t = draw(Ft8CyclesPanel, 191, 1, &m).join("\n");
        assert!(t.contains("11025 S/s has no route"), "{t}");
    }

    #[test]
    fn a_tuning_off_the_ft8_frequencies_is_said() {
        let mut m = evening();
        m.radio.frequency = 7_100_000;
        let t = draw(Ft8CyclesPanel, 191, 1, &m).join("\n");
        assert!(t.contains("7.100 MHz: not an FT8 frequency"), "{t}");
    }
}
