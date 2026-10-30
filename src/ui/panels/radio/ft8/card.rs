// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The selected station: where it said it was, its newest message, and what
//! that message measured and took (POLICY.md, rule 3): SNR in 2500 Hz as FT8
//! counts it, DT, tone 0 to the precision its uncertainty allows, drift,
//! Costas symbols on their tones, the soft decisions' block, belief
//! propagation's iterations, the bits repaired, OSD if it was needed, the
//! pass that found it; then how often and since when it was heard.

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    Frame,
};

use super::{hhmmss, km, parity, path, put};
use crate::state::SdrMetrics;
use crate::ui::panel::{Panel, PanelChrome, Staleness};

pub struct Ft8CardPanel;

impl Panel for Ft8CardPanel {
    fn name(&self) -> &'static str {
        "ft8_station"
    }

    fn reads_spectrum(&self) -> bool {
        false
    }

    fn min_size(&self) -> (u16, u16) {
        (48, 5)
    }

    /// Its seven lines and the frame, or a third of a short column.
    fn column_height(&self, column: u16) -> Option<u16> {
        Some(9.min(column / 3))
    }

    fn chrome(&self, _state: &SdrMetrics) -> PanelChrome {
        PanelChrome::new("Station").stale_when(Staleness::Never)
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
        let label = Style::default().fg(theme.label);
        let value = Style::default().fg(theme.value);
        let hi = Style::default().fg(theme.value_hi);
        let (Some(d), Some(call)) = (
            state.ft8.selected_decode(),
            state.ft8.view.selected.as_deref(),
        ) else {
            put(
                buf,
                area,
                area.x,
                area.y,
                "No station chosen: \u{2191}\u{2193} picks one from the list.",
                Style::default().fg(theme.border_dim),
            );
            return;
        };
        let st = state.ft8.stations.get(call);
        let loc = st.and_then(|s| s.locator.as_deref());
        let mut y = area.y;
        let mut x = put(
            buf,
            area,
            area.x,
            y,
            call,
            Style::default()
                .fg(theme.border_focused)
                .add_modifier(Modifier::BOLD),
        );
        match (loc, path(state, loc)) {
            (Some(l), Some((dist, brg))) => {
                x = put(buf, area, x + 2, y, l, hi);
                x = put(buf, area, x + 2, y, &format!("{} km", km(dist)), value);
                put(
                    buf,
                    area,
                    x + 1,
                    y,
                    &format!(
                        "at {brg:.0}\u{b0} {} (short path)",
                        crate::geo::sphere::compass(brg)
                    ),
                    label,
                );
            }
            (Some(l), None) => {
                x = put(buf, area, x + 2, y, l, hi);
                put(
                    buf,
                    area,
                    x + 2,
                    y,
                    "no position of yours to measure from",
                    label,
                );
            }
            (None, _) => {
                put(
                    buf,
                    area,
                    x + 2,
                    y,
                    "no locator heard: not on the map",
                    Style::default().fg(theme.border_dim),
                );
            }
        }
        if st.is_some_and(|s| s.called_you) {
            let t = "\u{25b6} called you";
            put(
                buf,
                area,
                area.right().saturating_sub(t.chars().count() as u16),
                y,
                t,
                Style::default()
                    .fg(theme.status_crit)
                    .add_modifier(Modifier::BOLD),
            );
        }
        y += 1;
        put(
            buf,
            area,
            area.x,
            y,
            &format!("\u{201c}{}\u{201d}", d.decode.message.text()),
            value,
        );
        let when = format!(
            "{} UTC \u{b7} {}",
            hhmmss(d.cycle_unix),
            parity(d.cycle_unix)
        );
        put(
            buf,
            area,
            area.right().saturating_sub(when.chars().count() as u16),
            y,
            &when,
            label,
        );
        y += 1;

        let p = &d.decode.physics;
        // Tone 0 to the decimals its uncertainty supports.
        let decimals = if p.f0_sigma < 0.05 {
            2
        } else if p.f0_sigma < 0.5 {
            1
        } else {
            0
        };
        let rows: Vec<Vec<(String, Style)>> = vec![
            vec![
                ("SNR ".into(), label),
                (format!("{:+.1} dB", p.snr_db), hi),
                (" in 2500 Hz   DT ".into(), label),
                (format!("{:+.3} s", d.decode.dt), value),
                ("   tone 0 ".into(), label),
                (
                    format!("{:.*} \u{b1}{:.*} Hz", decimals, p.f0, decimals, p.f0_sigma),
                    value,
                ),
            ],
            vec![
                ("drift ".into(), label),
                (format!("{:+.2} Hz", p.drift), value),
                (" over 12.64 s   sync ".into(), label),
                (format!("{}/21", d.decode.costas), value),
                (" Costas tones".into(), label),
            ],
            vec![
                ("LDPC ".into(), label),
                (format!("{} iter", d.decode.iterations), value),
                ("   repaired ".into(), label),
                (format!("{}", d.decode.repaired), hi),
                (" of 174   OSD ".into(), label),
                (
                    d.decode
                        .osd
                        .map_or("not needed".into(), |o| format!("order {o}")),
                    Style::default().fg(if d.decode.osd.is_some() {
                        theme.status_warn
                    } else {
                        theme.value
                    }),
                ),
                (
                    format!("   block {}   pass {}", d.decode.block, d.decode.pass),
                    label,
                ),
            ],
        ];
        for row in rows {
            if y >= area.bottom() {
                return;
            }
            let mut x = area.x;
            for (t, s) in row {
                x = put(buf, area, x, y, &t, s);
            }
            y += 1;
        }
        if let (Some(s), true) = (st, y < area.bottom()) {
            let lo = -24.0;
            let spark: String = s
                .snr
                .iter()
                .map(|(_, v)| {
                    [
                        '\u{2581}', '\u{2582}', '\u{2583}', '\u{2584}', '\u{2585}', '\u{2586}',
                        '\u{2587}', '\u{2588}',
                    ][(((v - lo) / 4.0).clamp(0.0, 7.0)) as usize]
                })
                .collect();
            let first = s.snr.front().map_or(d.cycle_unix, |x| x.0);
            let mut x = put(buf, area, area.x, y, "heard ", label);
            x = put(buf, area, x, y, &format!("{}\u{d7}", s.heard), value);
            x = put(
                buf,
                area,
                x,
                y,
                &format!(" since {}   SNR ", hhmmss(first)),
                label,
            );
            put(buf, area, x, y, &spark, hi);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::sample::evening;
    use super::*;
    use crate::state::fixture::draw;

    #[test]
    fn the_card_says_where_what_and_what_it_took() {
        let mut m = evening();
        m.ft8.view.selected = Some("JA1VQD".into());
        let t = draw(Ft8CardPanel, 79, 9, &m).join("\n");
        assert!(t.contains("JA1VQD  PM95"), "{t}");
        assert!(
            t.contains(" km at ") && t.contains("NE (short path)"),
            "{t}"
        );
        assert!(t.contains("\u{25b6} called you"), "{t}");
        assert!(t.contains("\u{201c}HA5ZZT JA1VQD PM95\u{201d}"), "{t}");
        assert!(t.contains("13:15:00 UTC \u{b7} even"), "{t}");
        assert!(t.contains("SNR -21.0 dB in 2500 Hz"), "{t}");
        assert!(t.contains("2210.60 \u{b1}0.01 Hz"), "{t}");
        assert!(t.contains("OSD order 2"), "{t}");
        assert!(t.contains("heard 2\u{d7} since 13:14:45"), "{t}");
    }

    #[test]
    fn no_locator_and_no_choice_say_so() {
        let mut m = evening();
        let t = draw(Ft8CardPanel, 79, 9, &m).join("\n");
        assert!(t.contains("No station chosen"), "{t}");
        m.ft8.view.selected = Some("YO3GCI".into());
        let t = draw(Ft8CardPanel, 79, 9, &m).join("\n");
        assert!(t.contains("no locator heard: not on the map"), "{t}");
    }
}
