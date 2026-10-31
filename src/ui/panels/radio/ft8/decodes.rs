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

use super::{hhmmss, km, parity, path, put, signed};
use crate::state::ft8::DecodeEntry;
use crate::state::SdrMetrics;
use crate::ui::panel::{Panel, PanelChrome, Staleness, Tag};
use std::sync::Arc;

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
            .tag_if(f.view.only_yours, Tag::Filtered)
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
        let listed = state.ft8.listed();
        if listed.is_empty() {
            let said = if state.ft8.decodes.is_empty() {
                "Nothing decoded yet."
            } else {
                "Nothing for you yet: u shows every decode."
            };
            put(
                buf,
                area,
                area.x + 1,
                area.y + 2,
                said,
                Style::default().fg(theme.border_dim),
            );
            return;
        }
        let my_call = state.home.callsign.as_deref();
        let selected = state.ft8.view.selected.as_deref();
        let newest = state.ft8.decodes.back().map(|d| d.cycle_unix);

        // The list as lines: a rule for each cycle, then its rows. Whatever
        // filter is on is the list's own (`listed`), so the keys that step
        // through it and the rows drawn are one list.
        enum Line<'a> {
            Rule(f64, usize, u64),
            Row(&'a Arc<DecodeEntry>),
        }
        let mut lines: Vec<Line> = Vec::new();
        for d in &listed {
            if !matches!(lines.iter().rev().find_map(|l| match l {
                Line::Rule(c, ..) => Some(*c),
                Line::Row(_) => None,
            }), Some(c) if c == d.cycle_unix)
            {
                let n = listed
                    .iter()
                    .filter(|e| e.cycle_unix == d.cycle_unix)
                    .count();
                lines.push(Line::Rule(d.cycle_unix, n, d.dial_hz));
            }
            lines.push(Line::Row(d));
        }
        // Scrolled only as far as the selected station's first row needs to
        // be seen: a station chosen with the arrows below the panel's foot
        // was highlighted where nobody could see it.
        let room = area.height.saturating_sub(1) as usize;
        let first_sel = lines.iter().position(|l| {
            matches!(l, Line::Row(d) if selected.is_some() && d.decode.message.sender() == selected)
        });
        let skip = first_sel.map_or(0, |i| (i + 1).saturating_sub(room));

        let mut y = area.y + 1;
        for line in lines.iter().skip(skip) {
            if y >= area.bottom() {
                break;
            }
            let d = match line {
                Line::Rule(start, n, dial) => {
                    let rule: String = "\u{2500}".repeat(w as usize);
                    put(
                        buf,
                        area,
                        area.x,
                        y,
                        &rule,
                        Style::default().fg(theme.stale),
                    );
                    let band = crate::signal::ft8::bands::band_of(*dial)
                        .map_or(String::new(), |b| format!(" \u{b7} {b}"));
                    let now = Some(*start) == newest;
                    put(
                        buf,
                        area,
                        area.x + 1,
                        y,
                        &format!(" {} {} \u{b7} {n}{band} ", hhmmss(*start), parity(*start)),
                        Style::default().fg(if now { theme.value } else { theme.label }),
                    );
                    y += 1;
                    continue;
                }
                Line::Row(d) => d,
            };
            let ci = usize::from(Some(d.cycle_unix) != newest);
            {
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
                    &format!("{:>3}", signed(p.snr_db, 0)),
                    style(if ci == 0 { theme.value_hi } else { ink }),
                );
                put(
                    buf,
                    area,
                    area.x + 5,
                    y,
                    &signed(d.decode.dt, 1),
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
        let newest = t
            .find("13:15:00 even \u{b7} 7 \u{b7} 20 m")
            .expect("newest rule");
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
    fn only_yours_shows_only_yours() {
        // Live: the title said filtered and every decode was still listed.
        let mut m = evening();
        m.ft8.view.only_yours = true;
        let t = draw(Ft8DecodesPanel, 79, 16, &m).join("\n");
        assert!(t.contains("HA5ZZT JA1VQD PM95"), "{t}");
        for other in ["EA8ZQ", "DL7ANR", "YO3GCI"] {
            assert!(!t.contains(other), "{other} shown:\n{t}");
        }
        m.home.callsign = Some("HA9XYZ".into());
        for d in m.ft8.decodes.iter_mut() {
            Arc::make_mut(d).for_you = false;
        }
        let t = draw(Ft8DecodesPanel, 79, 16, &m).join("\n");
        assert!(t.contains("Nothing for you yet"), "{t}");
    }

    #[test]
    fn the_selected_station_is_scrolled_into_sight() {
        // The oldest row's station, in a panel too short to reach it.
        let mut m = evening();
        let last = m
            .ft8
            .listed()
            .last()
            .unwrap()
            .decode
            .message
            .sender()
            .unwrap()
            .to_string();
        let t = draw(Ft8DecodesPanel, 79, 8, &m).join("\n");
        assert!(!t.contains(&last), "{last} fits anyway:\n{t}");
        m.ft8.view.selected = Some(last.clone());
        let t = draw(Ft8DecodesPanel, 79, 8, &m).join("\n");
        assert!(t.contains(&last), "{last} not in sight:\n{t}");
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
