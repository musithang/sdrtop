// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The band as a river: a row a cycle, newest on top, 0 to 3 kHz above the
//! dial across; every decode a bar on its own tone, as tall as its SNR, with
//! its call where it fits.
//!
//! **What did not decode is shown too** (`░`): energy from the cycle's own
//! spectrum standing above its median where nothing was read, so a busy
//! stretch of band is not mistaken for a quiet one. **The bottom 100 Hz is
//! the receiver's own** (what is left of its DC), marked as such and never
//! searched.

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    Frame,
};

use super::{hhmmss, put};
use crate::signal::ft8::cycle::BAND_BIN_HZ;
use crate::signal::ft8::search::F_MIN;
use crate::state::{CycleOutcome, SdrMetrics};
use crate::ui::panel::{Panel, PanelChrome, Staleness};

pub struct Ft8BandPanel;

const TOP_HZ: f64 = 3000.0;
/// The UTC gutter.
const GUTTER: u16 = 9;
/// The narrowest body the map keeps its place in.
const MAP_MIN_BODY: u16 = 120;
/// Energy this far over the cycle's median is shown where nothing decoded.
const ENERGY_DB: f32 = 6.0;

impl Panel for Ft8BandPanel {
    fn name(&self) -> &'static str {
        "ft8_band"
    }

    fn reads_spectrum(&self) -> bool {
        false
    }

    fn min_size(&self) -> (u16, u16) {
        (40, 5)
    }

    /// A quarter of the column, six to twelve rows: eight cycles at most,
    /// the rest of the column for the list.
    fn column_height(&self, column: u16) -> Option<u16> {
        Some((column / 4).clamp(6, 12))
    }

    /// Below a body of 120 columns the map gives way and this column takes
    /// the body: the decodes stay readable, a map that small would not be.
    fn column_width(&self, body: u16) -> Option<u16> {
        (body < MAP_MIN_BODY).then_some(body)
    }

    fn chrome(&self, state: &SdrMetrics) -> PanelChrome {
        let dial = state
            .ft8
            .cycles
            .back()
            .map_or(state.radio.frequency, |c| c.dial_hz);
        PanelChrome::new("Band")
            .stale_when(Staleness::NotStreaming)
            .suffix(format!(" 0-3 kHz above {:.3} MHz", dial as f64 / 1e6))
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
        if area.width <= GUTTER + 10 || area.height < 3 {
            return;
        }
        let span = area.width - GUTTER;
        let col = |hz: f64| area.x + GUTTER + ((hz / TOP_HZ) * f64::from(span - 1)).round() as u16;
        let label = Style::default().fg(theme.label);
        let dim = Style::default().fg(theme.border_dim);

        // The scale, and the receiver's own strip at the bottom of it.
        for k in 0..=6 {
            let hz = f64::from(k) * 500.0;
            let t = if k == 0 {
                "0".to_string()
            } else {
                format!("{}k", hz / 1000.0)
            };
            let x = col(hz).min(area.right().saturating_sub(t.len() as u16));
            put(buf, area, x, area.y, &t, label);
            put(buf, area, col(hz), area.y + 1, "\u{2534}", dim);
        }
        put(
            buf,
            area,
            area.x,
            area.y + 1,
            "DC",
            Style::default().fg(theme.stale),
        );
        let own = col(F_MIN);

        let selected = state.ft8.view.selected.as_deref();
        let rows = (area.height - 2) as usize;
        for (r, c) in state.ft8.cycles.iter().rev().take(rows).enumerate() {
            let y = area.y + 2 + r as u16;
            let ink = match r {
                0 => theme.value_hi,
                1..=2 => theme.value,
                _ => theme.label,
            };
            put(
                buf,
                area,
                area.x,
                y,
                &hhmmss(c.start_unix),
                Style::default().fg(ink),
            );
            for x in (area.x + GUTTER..area.right()).step_by(4) {
                put(buf, area, x, y, "\u{b7}", Style::default().fg(theme.stale));
            }
            for x in area.x + GUTTER..own {
                put(
                    buf,
                    area,
                    x,
                    y,
                    "\u{254e}",
                    Style::default().fg(theme.stale),
                );
            }
            if c.outcome != CycleOutcome::Decoded {
                let why = if c.outcome == CycleOutcome::Cut {
                    "cut: the stream broke in this cycle"
                } else {
                    "skipped: the decoder was still on the one before"
                };
                put(
                    buf,
                    area,
                    area.x + GUTTER + 2,
                    y,
                    why,
                    Style::default().fg(theme.status_warn),
                );
                continue;
            }
            let decodes: Vec<_> = state
                .ft8
                .decodes
                .iter()
                .filter(|d| d.cycle_unix == c.start_unix)
                .collect();
            // Energy where nothing decoded.
            for (k, &db) in c.band.iter().enumerate() {
                let hz = (k as f64 + 0.5) * BAND_BIN_HZ;
                let read = decodes.iter().any(|d| {
                    (d.decode.physics.f0 - 6.25..=d.decode.physics.f0 + 50.0).contains(&hz)
                });
                if db >= ENERGY_DB && !read && hz >= F_MIN {
                    put(
                        buf,
                        area,
                        col(hz),
                        y,
                        "\u{2591}",
                        Style::default().fg(theme.noise_floor),
                    );
                }
            }
            // Bars first, then the calls where they fit.
            let mut used: Vec<(u16, u16)> = Vec::new();
            for d in &decodes {
                let snr = d.decode.physics.snr_db;
                let level = ((snr + 24.0) / 4.0).clamp(0.0, 7.0) as usize;
                let ch = [
                    '\u{2581}', '\u{2582}', '\u{2583}', '\u{2584}', '\u{2585}', '\u{2586}',
                    '\u{2587}', '\u{2588}',
                ][level];
                let from = d.decode.message.sender();
                let c = if from.is_some() && from == selected {
                    theme.border_focused
                } else if d.for_you {
                    theme.status_crit
                } else {
                    ink
                };
                let x = col(d.decode.physics.f0);
                let bar: String = [ch, ch].iter().collect();
                put(buf, area, x, y, &bar, Style::default().fg(c));
                used.push((x, x + 1));
            }
            let mut by_snr = decodes.clone();
            by_snr.sort_by(|a, b| b.decode.physics.snr_db.total_cmp(&a.decode.physics.snr_db));
            for d in by_snr {
                let Some(call) = d.decode.message.sender() else {
                    continue;
                };
                let bar = col(d.decode.physics.f0);
                let n = call.chars().count() as u16;
                let left_edge = area.x + GUTTER;
                // Right of its bar, or left of it with a clear cell before,
                // so a name is never read as the bar before it.
                let spots = [
                    Some((bar + 2, 0)),
                    bar.checked_sub(n + 1)
                        .filter(|&x| x >= left_edge)
                        .map(|x| (x, 1)),
                ];
                for (x, gap) in spots.into_iter().flatten() {
                    let end = x + n;
                    if end < area.right() && used.iter().all(|&(a, b)| end < a || x > b + gap) {
                        let is_sel = Some(call) == selected;
                        let mut st = Style::default().fg(if is_sel {
                            theme.border_focused
                        } else if d.for_you {
                            theme.status_crit
                        } else {
                            ink
                        });
                        if is_sel || d.for_you {
                            st = st.add_modifier(Modifier::BOLD);
                        }
                        put(buf, area, x, y, call, st);
                        used.push((x, end));
                        break;
                    }
                }
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
    fn decodes_sit_on_their_tones_with_their_calls() {
        let t = draw(Ft8BandPanel, 79, 12, &evening()).join("\n");
        assert!(t.contains("0-3 kHz above 14.074 MHz"), "{t}");
        assert!(t.contains("13:15:00") && t.contains("13:14:45"), "{t}");
        // A crowded stretch names whom it can; the list names everyone.
        for call in ["DL7ANR", "W2XK", "JA1VQD", "PY2WQS"] {
            assert!(t.contains(call), "{call}:\n{t}");
        }
        // Energy that did not decode, near 2 kHz.
        assert!(t.contains('\u{2591}'), "{t}");
        assert!(t.contains("DC"), "{t}");
    }

    #[test]
    fn a_cut_cycle_reads_as_cut_not_empty() {
        let mut m = evening();
        m.ft8.cycles.back_mut().unwrap().outcome = CycleOutcome::Cut;
        let t = draw(Ft8BandPanel, 79, 12, &m).join("\n");
        assert!(t.contains("cut: the stream broke"), "{t}");
    }
}
