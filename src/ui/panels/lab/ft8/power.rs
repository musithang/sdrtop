// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The transmission's power through its 12.64 s: each half symbol's power
//! with the noise's share taken out, over the transmission's mean, from 0
//! to twice the mean. A fade through the transmission shows as itself.
//!
//! Under it the path as the decode judged it: steady or fading, and the
//! share of the power one steady carrier explains
//! ([`crate::signal::ft8::demod::Physics::coherence`]). A column past twice
//! the mean is drawn full with a `▲` on top rather than cut off silently.
//!
//! **Its colour is the ladder's.** Each column is coloured by the sent
//! tone's energy over the noise in the symbols it covers, on the same
//! gradient the ladder's cells use: one quantity on one scale, so a tall
//! column and a bright cell above it are the same moment. The darkest
//! column is lifted off the gradient's bottom, which is the background in
//! most themes, so a weak column still shows.

use ratatui::{layout::Rect, style::Style, Frame};

use super::{heat, say, sent_db, shown_or_say, HEAT_DB};
use crate::state::ft8::DecodeEntry;
use crate::state::SdrMetrics;
use crate::ui::panel::{Panel, PanelChrome, Staleness};
use crate::ui::panels::radio::ft8::put;

pub struct Ft8PowerPanel;

/// The plot's top, as a multiple of the mean.
pub const TOP: f64 = 2.0;

const EIGHTHS: [&str; 9] = [" ", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];

impl Panel for Ft8PowerPanel {
    fn name(&self) -> &'static str {
        "ft8_power"
    }

    fn reads_spectrum(&self) -> bool {
        false
    }

    fn min_size(&self) -> (u16, u16) {
        (30, 6)
    }

    fn chrome(&self, _state: &SdrMetrics) -> PanelChrome {
        PanelChrome::new("Power").stale_when(Staleness::NotStreaming)
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
        let Some(d) = shown_or_say(buf, area, state, theme) else {
            return;
        };
        if area.height < 3 || area.width < 20 {
            say(buf, area, "too small for the power", theme);
            return;
        }
        draw(buf, area, &d, theme);
    }
}

/// The mean of `env` over the pieces column `c` of `cols` covers.
pub fn column_mean(env: &[f32], c: usize, cols: usize) -> f64 {
    let a = c * env.len() / cols;
    let b = ((c + 1) * env.len() / cols).max(a + 1).min(env.len());
    env[a..b].iter().map(|v| f64::from(*v)).sum::<f64>() / (b - a) as f64
}

fn draw(buf: &mut ratatui::buffer::Buffer, area: Rect, d: &DecodeEntry, t: &crate::Theme) {
    let env = &d.decode.trace.envelope;
    let p = &d.decode.physics;
    let label = Style::default().fg(t.label);
    let value = Style::default().fg(t.value);
    let rows = area.height - 1;
    let x0 = area.x + 5;
    let cols = area.width.saturating_sub(6).max(1) as usize;
    put(buf, area, area.x + 1, area.y, "2×", label);
    put(buf, area, area.x + 1, area.y + rows / 2, "1×", label);
    put(buf, area, area.x + 1, area.y + rows - 1, "0", label);
    if env.is_empty() {
        say(buf, area, "no power kept for this decode", t);
        return;
    }
    for c in 0..cols {
        let v = column_mean(env, c, cols);
        let full = (v / TOP).clamp(0.0, 1.0) * f64::from(rows) * 8.0;
        let ink = Style::default().fg(heat(t, column_db(&d.decode.trace, c, cols)));
        for r in 0..rows {
            let lo = f64::from(rows - 1 - r) * 8.0;
            let e = (full - lo).clamp(0.0, 8.0).round() as usize;
            put(buf, area, x0 + c as u16, area.y + r, EIGHTHS[e], ink);
        }
        if v > TOP {
            put(
                buf,
                area,
                x0 + c as u16,
                area.y,
                "▲",
                Style::default().fg(t.value_hi),
            );
        }
    }
    let foot = area.y + rows;
    let (word, style) = if p.fading() {
        ("fading", Style::default().fg(t.status_warn))
    } else {
        ("steady", Style::default().fg(t.status_ok))
    };
    let x = put(buf, area, x0, foot, word, style);
    let x = put(
        buf,
        area,
        x + 1,
        foot,
        &format!("{:.2}", p.coherence),
        value,
    );
    put(buf, area, x + 1, foot, "of it one carrier", label);
}

/// The colour's figure for column `c` of `cols`: the sent tone's energy
/// over the noise, averaged as energy over the symbols the column covers,
/// in dB, kept off the gradient's lowest fifth.
pub fn column_db(tr: &crate::signal::ft8::trace::Trace, c: usize, cols: usize) -> f64 {
    let pieces = tr.envelope.len().max(1);
    let a = c * pieces / cols;
    let b = ((c + 1) * pieces / cols).max(a + 1).min(pieces);
    let per = (pieces / crate::signal::ft8::modulate::SYMBOLS).max(1);
    let (s0, s1) = (
        a / per,
        ((b - 1) / per).min(crate::signal::ft8::modulate::SYMBOLS - 1),
    );
    let mean = (s0..=s1)
        .map(|s| 10f64.powf(sent_db(tr, s) / 10.0))
        .sum::<f64>()
        / (s1 - s0 + 1) as f64;
    let floor = HEAT_DB.0 + 0.2 * (HEAT_DB.1 - HEAT_DB.0);
    (10.0 * mean.max(1e-6).log10()).max(floor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::fixture::draw as render;

    #[test]
    fn the_path_is_named_as_the_decode_judged_it() {
        let m = SdrMetrics::fixture().streaming().ft8_traced();
        for d in &m.ft8.decodes {
            let mut m = m.clone();
            m.ft8.view.selected = d.decode.message.sender().map(str::to_string);
            let crate::state::ft8::Shown::Decode(s) = m.ft8.shown() else {
                panic!()
            };
            let lines = render(Ft8PowerPanel, 63, 18, &m).join("\n");
            let word = if s.decode.physics.fading() {
                "fading"
            } else {
                "steady"
            };
            assert!(
                lines.contains(&format!("{word} {:.2}", s.decode.physics.coherence)),
                "{lines}"
            );
        }
    }

    /// A column's colour is the ladder's lit cell for its symbols, never
    /// below the gradient's lowest fifth.
    #[test]
    fn a_columns_colour_is_its_symbols_sent_tone() {
        let m = SdrMetrics::fixture().streaming().ft8_traced();
        let tr = &m.ft8.decodes[0].decode.trace;
        // One column a symbol: its own sent tone, or the floor.
        let floor = HEAT_DB.0 + 0.2 * (HEAT_DB.1 - HEAT_DB.0);
        for s in [0, 10, 40, 78] {
            let want = sent_db(tr, s).max(floor);
            assert!((column_db(tr, s, 79) - want).abs() < 1e-9, "symbol {s}");
        }
    }

    #[test]
    fn columns_average_the_pieces_they_cover() {
        let env = [0.0f32, 2.0, 4.0, 6.0];
        assert_eq!(column_mean(&env, 0, 2), 1.0);
        assert_eq!(column_mean(&env, 1, 2), 5.0);
        // More columns than pieces: each column one piece.
        assert_eq!(column_mean(&env, 3, 8), 2.0);
    }

    #[test]
    fn before_any_decode_it_waits() {
        let lines = render(Ft8PowerPanel, 63, 18, &SdrMetrics::fixture()).join("\n");
        assert!(lines.contains("waiting for a decode"), "{lines}");
    }
}
