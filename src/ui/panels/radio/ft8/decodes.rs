// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The decodes, cycle by cycle, newest first, each cycle's by tone.
//!
//! A row: SNR, DT, tone 0, the message as it reads on the air (CQ and DX in
//! the ok ink, the transmitting call lit, your call red with `▶`), and the
//! distance and bearing from you where both its locator and your position
//! are known; "no locator" where the station never sent one. Never a
//! distance worked out from a callsign's prefix (POLICY.md, rule 2).

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    Frame,
};

use super::{hhmmss, km, parity, path, put};
use crate::state::SdrMetrics;
use crate::ui::panel::{Panel, PanelChrome, Staleness};

pub struct Ft8DecodesPanel;

impl Panel for Ft8DecodesPanel {
    fn name(&self) -> &'static str {
        "ft8_decodes"
    }

    fn reads_spectrum(&self) -> bool {
        false
    }

    fn min_size(&self) -> (u16, u16) {
        (48, 6)
    }

    fn chrome(&self, state: &SdrMetrics) -> PanelChrome {
        let f = &state.ft8;
        let newest = super::newest_cycle(f);
        let now = newest.map_or(0, |n| {
            f.decodes.iter().filter(|d| d.cycle_unix == n).count()
        });
        let mine = f.decodes.iter().filter(|d| d.for_you).count();
        let mut suffix = format!(" {now} this cycle");
        if mine > 0 {
            suffix.push_str(&format!(" \u{b7} {mine} for you"));
        }
        PanelChrome::new("Decodes")
            .stale_when(Staleness::Never)
            .suffix(suffix)
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
        let w = area.width;
        let label = Style::default().fg(theme.label);
        let far = w >= 64;
        put(buf, area, area.x, area.y, " dB   DT    Hz  message", label);
        if far {
            put(
                buf,
                area,
                area.right() - 15,
                area.y,
                "    km  bearing",
                label,
            );
        }
        if state.ft8.decodes.is_empty() {
            put(
                buf,
                area,
                area.x + 1,
                area.y + 2,
                "Nothing decoded yet.",
                Style::default().fg(theme.border_dim),
            );
            return;
        }
        let my_call = state.home.callsign.as_deref();
        let selected = state.ft8.view.selected.as_deref();
        let mut y = area.y + 1;
        let mut cycles: Vec<f64> = state.ft8.decodes.iter().map(|d| d.cycle_unix).collect();
        cycles.dedup();
        for (ci, start) in cycles.iter().rev().enumerate() {
            if y >= area.bottom() {
                break;
            }
            let mut rows: Vec<_> = state
                .ft8
                .decodes
                .iter()
                .filter(|d| d.cycle_unix == *start)
                .collect();
            rows.sort_by(|a, b| a.decode.physics.f0.total_cmp(&b.decode.physics.f0));
            let rule: String = "\u{2500}".repeat(w as usize);
            put(
                buf,
                area,
                area.x,
                y,
                &rule,
                Style::default().fg(theme.stale),
            );
            put(
                buf,
                area,
                area.x + 1,
                y,
                &format!(
                    " {} {} \u{b7} {} ",
                    hhmmss(*start),
                    parity(*start),
                    rows.len()
                ),
                Style::default().fg(if ci == 0 { theme.value } else { theme.label }),
            );
            y += 1;
            for d in rows {
                if y >= area.bottom() {
                    break;
                }
                let from = d.decode.message.sender();
                let is_sel = from.is_some() && from == selected;
                let bg = if is_sel { Some(theme.stale) } else { None };
                let style = |fg| {
                    let s = Style::default().fg(fg);
                    bg.map_or(s, |b| s.bg(b))
                };
                if is_sel {
                    put(
                        buf,
                        area,
                        area.x,
                        y,
                        &" ".repeat(w as usize),
                        style(theme.value),
                    );
                }
                let ink = if ci == 0 { theme.value } else { theme.label };
                let p = &d.decode.physics;
                put(
                    buf,
                    area,
                    area.x,
                    y,
                    &format!("{:+3.0}", p.snr_db),
                    style(if ci == 0 { theme.value_hi } else { ink }),
                );
                put(
                    buf,
                    area,
                    area.x + 5,
                    y,
                    &format!("{:+.1}", d.decode.dt),
                    style(ink),
                );
                put(
                    buf,
                    area,
                    area.x + 11,
                    y,
                    &format!("{:>4.0}", p.f0),
                    style(ink),
                );
                let mut x = area.x + 17;
                if d.for_you {
                    put(
                        buf,
                        area,
                        x - 1,
                        y,
                        "\u{25b6}",
                        style(theme.status_crit).add_modifier(Modifier::BOLD),
                    );
                }
                let text = d.decode.message.text();
                let limit = if far {
                    area.right().saturating_sub(16)
                } else {
                    area.right()
                };
                for word in text.split(' ') {
                    let bare = word.trim_matches(|c| c == '<' || c == '>');
                    let (c, bold) = if Some(bare) == my_call {
                        (theme.status_crit, true)
                    } else if Some(bare) == from {
                        (
                            if is_sel {
                                theme.border_focused
                            } else if ci == 0 {
                                theme.value_hi
                            } else {
                                theme.value
                            },
                            true,
                        )
                    } else if word == "CQ" || word == "DX" {
                        (theme.status_ok, false)
                    } else {
                        (ink, false)
                    };
                    if x + word.chars().count() as u16 > limit {
                        break;
                    }
                    let mut st = style(c);
                    if bold {
                        st = st.add_modifier(Modifier::BOLD);
                    }
                    x = put(buf, area, x, y, word, st) + 1;
                }
                if far {
                    let loc = from
                        .and_then(|c| state.ft8.stations.get(c))
                        .and_then(|s| s.locator.as_deref());
                    match path(state, loc) {
                        Some((dist, brg)) => {
                            put(
                                buf,
                                area,
                                area.right() - 15,
                                y,
                                &format!("{:>6}", km(dist)),
                                style(ink),
                            );
                            put(
                                buf,
                                area,
                                area.right() - 8,
                                y,
                                &format!("{brg:4.0}\u{b0} {:<2}", crate::geo::sphere::compass(brg)),
                                style(ink),
                            );
                        }
                        None if loc.is_none() => {
                            put(
                                buf,
                                area,
                                area.right() - 13,
                                y,
                                "no locator",
                                style(theme.border_dim),
                            );
                        }
                        None => {}
                    }
                }
                y += 1;
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
    fn the_newest_cycle_first_by_tone_with_distances() {
        let t = draw(Ft8DecodesPanel, 79, 16, &evening()).join("\n");
        assert!(t.contains("7 this cycle"), "{t}");
        assert!(t.contains("1 for you"), "{t}");
        let newest = t.find("13:15:00 even").expect("newest rule");
        let older = t.find("13:14:45 odd").expect("older rule");
        assert!(newest < older, "{t}");
        // By tone within the cycle: 340 Hz before 612 before 902.
        let (a, b, c) = (
            t.find("YO3GCI").unwrap(),
            t.find("EA8ZQ").unwrap(),
            t.find("DL7ANR").unwrap(),
        );
        assert!(a < b && b < c, "{t}");
        assert!(t.contains("\u{25b6}HA5ZZT JA1VQD PM95"), "{t}");
        assert!(t.contains("no locator"), "{t}");
        // Tokyo from Budapest, north-east, about 9 000 km.
        assert!(t.contains(" NE"), "{t}");
    }

    #[test]
    fn without_a_position_there_are_no_distances_and_no_invented_ones() {
        let mut m = evening();
        m.home.latitude = None;
        let t = draw(Ft8DecodesPanel, 79, 16, &m).join("\n");
        assert!(!t.contains(" NE") && !t.contains('\u{b0}'), "{t}");
    }

    #[test]
    fn an_empty_list_says_so() {
        let mut m = evening();
        m.ft8.decodes.clear();
        let t = draw(Ft8DecodesPanel, 79, 8, &m).join("\n");
        assert!(t.contains("Nothing decoded yet."), "{t}");
    }
}
