// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The tone ladder: one transmission, symbol by symbol, as the decoder saw
//! it.
//!
//! Seventy-nine columns, one a symbol (two at full width), and ten rows:
//! the eight tones with one guard tone either side (tone -1 and tone 8,
//! where a neighbour or a dirty transmitter would show). Each cell is that
//! tone's energy in that symbol over the noise, on the theme's own gradient
//! from -5 to +30 dB, the one the waterfall uses; the tone the codeword sent
//! is a dark line through it.
//!
//! Above, the three Costas arrays. Below, the symbols that were not what the
//! decode needed: `×` a Costas symbol whose strongest tone is not its own,
//! `▲` a data symbol carrying a bit the code repaired; then each symbol's
//! margin, its strongest tone over the runner-up, so a symbol decided by a
//! whisker reads as one; then the symbol axis. At full width a column to
//! the right carries each row's figure.
//!
//! The energies, bits and margins are the decode's own trace
//! ([`crate::signal::ft8::trace::Trace`]), never recomputed here.

use ratatui::{
    layout::Rect,
    style::{Color, Style},
    Frame,
};

use super::{caption, say, shown_or_say};
use crate::signal::ft8::modulate::{COSTAS, COSTAS_AT, SYMBOLS};
use crate::state::ft8::{DecodeEntry, Shown};
use crate::state::SdrMetrics;
use crate::ui::panel::{Panel, PanelChrome, Staleness};
use crate::ui::panels::radio::ft8::put;

pub struct Ft8LadderPanel;

/// The gap inside the frame, then the label column on the left.
const PAD: u16 = 1;
const LABEL_W: u16 = 7;
/// The column of figures on the right, when there is room for it.
const NOTE_W: u16 = 22;
/// Rows: the Costas brackets, ten tones, the marks, the margin, the axis.
const ROWS: u16 = 14;
/// The heat's range, dB over the noise.
const HEAT_DB: (f64, f64) = (-5.0, 30.0);
/// A margin this large fills its cell.
const MARGIN_FULL_DB: f64 = 20.0;

/// How the ladder fits a width.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fit {
    /// Cells a symbol: 2 or 1.
    pub cells: u16,
    /// Whether the column of figures is drawn.
    pub notes: bool,
}

/// The ladder's fit in an inner area of `w` by `h`, or why it does not.
pub fn fit(w: u16, h: u16) -> Result<Fit, String> {
    let need = |cells: u16, notes: bool| {
        PAD + LABEL_W + SYMBOLS as u16 * cells + if notes { 1 + NOTE_W } else { 0 }
    };
    if h < ROWS {
        return Err(format!("the ladder needs {ROWS} rows; this panel has {h}"));
    }
    if w >= need(2, true) {
        Ok(Fit {
            cells: 2,
            notes: true,
        })
    } else if w >= need(1, true) {
        Ok(Fit {
            cells: 1,
            notes: true,
        })
    } else if w >= need(1, false) {
        Ok(Fit {
            cells: 1,
            notes: false,
        })
    } else {
        Err(format!(
            "the ladder needs {} columns; this panel has {w}",
            need(1, false)
        ))
    }
}

fn costas_tone(s: usize) -> Option<u8> {
    COSTAS_AT
        .iter()
        .find(|&&a| s >= a && s < a + COSTAS.len())
        .map(|&a| COSTAS[s - a])
}

const EIGHTHS: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

impl Panel for Ft8LadderPanel {
    fn name(&self) -> &'static str {
        "ft8_ladder"
    }

    fn reads_spectrum(&self) -> bool {
        false
    }

    fn min_size(&self) -> (u16, u16) {
        (PAD + LABEL_W + SYMBOLS as u16 + 2, ROWS + 2)
    }

    fn chrome(&self, state: &SdrMetrics) -> PanelChrome {
        let mut chrome = PanelChrome::new("Tone ladder").stale_when(Staleness::NotStreaming);
        if let Shown::Decode(d) = state.ft8.shown() {
            chrome.suffix = Some(caption(&d));
        }
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
        let Some(d) = shown_or_say(buf, area, state, theme) else {
            return;
        };
        match fit(area.width, area.height) {
            Ok(fit) => draw(buf, area, &d, fit, theme),
            Err(why) => say(buf, area, &why, theme),
        }
    }
}

/// The ladder of `d` into `area`.
fn draw(
    buf: &mut ratatui::buffer::Buffer,
    area: Rect,
    d: &DecodeEntry,
    fit: Fit,
    t: &crate::Theme,
) {
    let tr = &d.decode.trace;
    let cw = fit.cells;
    let xl = area.x + PAD;
    let x0 = xl + LABEL_W;
    let xn = x0 + SYMBOLS as u16 * cw + 1;
    let label = Style::default().fg(t.label);
    let value = Style::default().fg(t.value);
    let warn = Style::default().fg(t.status_warn);
    // The gradient's dark end draws the sent tone across the heat: dark on
    // the bright cells a signal makes, the theme's own colour.
    let ink = t.palette_color(0.0);
    let heat = |db: f64| -> Color {
        t.palette_color((((db - HEAT_DB.0) / (HEAT_DB.1 - HEAT_DB.0)).clamp(0.0, 1.0)) as f32)
    };
    let noise = f64::from(tr.noise).max(f64::MIN_POSITIVE);
    let mut y = area.y;

    // The Costas arrays.
    for &a in &COSTAS_AT {
        let w = (COSTAS.len() as u16 * cw) as usize;
        let name = if cw == 2 { " Costas " } else { "" };
        let fill = w - 2 - name.chars().count();
        let bracket = format!(
            "┌{}{}{}┐",
            "─".repeat(fill / 2),
            name,
            "─".repeat(fill - fill / 2)
        );
        put(
            buf,
            area,
            x0 + a as u16 * cw,
            y,
            &bracket,
            Style::default().fg(t.border_default),
        );
    }
    if fit.notes {
        let x = put(buf, area, xn, y, "sync ", label);
        put(
            buf,
            area,
            x,
            y,
            &format!("{} of 21", d.decode.costas),
            value,
        );
    }
    y += 1;

    // The tones, top to bottom: tone 8 (guard), 7 .. 0, tone -1 (guard).
    for r in 0..10u16 {
        let row = 9 - r as usize;
        let (text, style) = match row {
            9 => ("  +1".to_string(), Style::default().fg(t.border_dim)),
            0 => ("  -1".to_string(), Style::default().fg(t.border_dim)),
            k => (format!("  {}", k - 1), label),
        };
        put(buf, area, xl, y, &text, style);
        for s in 0..SYMBOLS {
            let db = 10.0 * (f64::from(tr.energies[s][row]) / noise).max(1e-6).log10();
            let sent = usize::from(tr.tones[s]) + 1 == row;
            let mark = if !sent {
                ' '
            } else if cw == 2 {
                '━'
            } else {
                '─'
            };
            for k in 0..cw {
                let x = x0 + s as u16 * cw + k;
                if x < area.right() && y < area.bottom() {
                    buf[(x, y)]
                        .set_char(mark)
                        .set_style(Style::default().fg(ink).bg(heat(db)));
                }
            }
        }
        if fit.notes {
            match r {
                0 => {
                    put(buf, area, xn, y, "dB over noise", label);
                }
                1..=4 => {
                    let db = [30.0, 20.0, 10.0, 0.0][r as usize - 1];
                    put(buf, area, xn, y, "  ", Style::default().bg(heat(db)));
                    put(buf, area, xn + 3, y, &format!("{db:+.0}"), label);
                }
                6 => {
                    let x = put(buf, area, xn, y, "━", Style::default().fg(t.value_hi));
                    put(buf, area, x + 1, y, "sent tone", label);
                }
                _ => {}
            }
        }
        y += 1;
    }

    // The symbols that were not what the decode needed.
    put(buf, area, xl, y, "fixed", label);
    let repaired = tr.repaired_symbols();
    let (mut n_fixed, mut n_missed) = (0, 0);
    for (s, (fixed, row)) in repaired.iter().zip(&tr.energies).enumerate() {
        let x = x0 + s as u16 * cw;
        if let Some(own) = costas_tone(s) {
            let strongest = (1..=8)
                .max_by(|&a, &b| row[a].total_cmp(&row[b]))
                .map_or(0, |r| r - 1);
            if strongest != usize::from(own) {
                n_missed += 1;
                put(buf, area, x, y, "×", warn);
            }
        } else if *fixed {
            n_fixed += 1;
            put(buf, area, x, y, "▲", warn);
        }
    }
    if fit.notes {
        let x = put(buf, area, xn, y, "▲", warn);
        let x = put(buf, area, x + 1, y, &format!("{n_fixed} sym "), value);
        let x = put(buf, area, x, y, "×", warn);
        put(buf, area, x + 1, y, &format!("{n_missed} Costas"), value);
    }
    y += 1;

    // How clearly each symbol was decided.
    put(buf, area, xl, y, "margin", label);
    let margin = tr.margin_db();
    for (s, m) in margin.iter().enumerate() {
        let v = (f64::from(*m) / MARGIN_FULL_DB).clamp(0.0, 1.0);
        let ch = EIGHTHS[(v * 8.0).round() as usize].to_string();
        for k in 0..cw {
            put(buf, area, x0 + s as u16 * cw + k, y, &ch, value);
        }
    }
    if fit.notes {
        let least = (0..SYMBOLS)
            .filter(|&s| costas_tone(s).is_none())
            .map(|s| margin[s])
            .fold(f32::MAX, f32::min);
        let x = put(buf, area, xn, y, "least ", label);
        put(buf, area, x, y, &format!("{least:.1} dB"), value);
    }
    y += 1;

    // The axis.
    for s in (0..=70).step_by(10) {
        put(buf, area, x0 + s as u16 * cw, y, &format!("{s}"), label);
    }
    put(buf, area, x0 + 78 * cw - 1, y, "78", label);
    if fit.notes {
        put(buf, area, xn, y, "symbol, 0.16 s", label);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::fixture::draw;

    #[test]
    fn the_ladder_fits_two_cells_a_symbol_then_one_then_refuses() {
        assert_eq!(
            fit(189, 14),
            Ok(Fit {
                cells: 2,
                notes: true
            })
        );
        assert_eq!(
            fit(150, 14),
            Ok(Fit {
                cells: 1,
                notes: true
            })
        );
        assert_eq!(
            fit(98, 14),
            Ok(Fit {
                cells: 1,
                notes: false
            })
        );
        assert!(fit(86, 14).unwrap_err().contains("87 columns"));
        assert!(fit(189, 13).unwrap_err().contains("14 rows"));
    }

    #[test]
    fn before_any_decode_it_waits() {
        let m = SdrMetrics::fixture().streaming();
        let lines = draw(Ft8LadderPanel, 191, 16, &m).join("\n");
        assert!(lines.contains("waiting for a decode"), "{lines}");
    }

    #[test]
    fn a_station_whose_decodes_have_gone_says_so() {
        let mut m = SdrMetrics::fixture().streaming().ft8_traced();
        m.ft8.view.selected = Some("JA1VQD".into());
        let lines = draw(Ft8LadderPanel, 191, 16, &m).join("\n");
        assert!(lines.contains("JA1VQD"), "{lines}");
        assert!(lines.contains("trace no longer kept"), "{lines}");
    }

    /// At full width: the three Costas brackets named, one sent mark a
    /// symbol in each cell of it, the figures on the right, the transmission
    /// named in the title.
    #[test]
    fn a_decode_draws_its_ladder_at_full_width() {
        let m = SdrMetrics::fixture().streaming().ft8_traced();
        let Shown::Decode(d) = m.ft8.shown() else {
            panic!("the fixture has decodes")
        };
        let lines = draw(Ft8LadderPanel, 191, 16, &m);
        let all = lines.join("\n");
        assert!(lines[0].contains(&d.decode.message.text()), "{}", lines[0]);
        assert_eq!(all.matches("┌── Costas ──┐").count(), 3, "{all}");
        let sent: usize = lines[2..12].iter().map(|l| l.matches('━').count()).sum();
        // One mark a symbol, two cells each, and one in the legend.
        assert_eq!(sent, 2 * SYMBOLS + 1, "{all}");
        assert!(
            all.contains(&format!("sync {} of 21", d.decode.costas)),
            "{all}"
        );
        assert!(all.contains("dB over noise"));
        assert!(all.contains("symbol, 0.16 s"));
    }

    /// The marks are the trace's: a `▲` for each data symbol with a repaired
    /// bit, a `×` for each Costas symbol not strongest on its own tone.
    #[test]
    fn the_marks_count_what_the_trace_says() {
        let m = SdrMetrics::fixture().streaming().ft8_traced();
        for d in &m.ft8.decodes {
            let mut m = m.clone();
            m.ft8.view.selected = d.decode.message.sender().map(str::to_string);
            let Shown::Decode(shown) = m.ft8.shown() else {
                panic!()
            };
            let tr = &shown.decode.trace;
            let fixed = tr.repaired_symbols().iter().filter(|r| **r).count();
            let lines = draw(Ft8LadderPanel, 191, 16, &m);
            let marks = &lines[12];
            assert_eq!(marks.matches('▲').count(), fixed + 1, "{marks}");
            assert!(marks.contains(&format!("{fixed} sym")), "{marks}");
        }
    }

    #[test]
    fn narrower_it_keeps_one_cell_a_symbol_and_then_drops_the_figures() {
        let m = SdrMetrics::fixture().streaming().ft8_traced();
        let at_152 = draw(Ft8LadderPanel, 152, 16, &m).join("\n");
        assert_eq!(at_152.matches("┌─────┐").count(), 3, "{at_152}");
        assert!(at_152.contains("dB over noise"));
        let at_100 = draw(Ft8LadderPanel, 100, 16, &m).join("\n");
        assert_eq!(at_100.matches("┌─────┐").count(), 3, "{at_100}");
        assert!(!at_100.contains("dB over noise"), "{at_100}");
        let at_80 = draw(Ft8LadderPanel, 80, 16, &m).join("\n");
        assert!(at_80.contains("needs 87 columns"), "{at_80}");
    }
}
