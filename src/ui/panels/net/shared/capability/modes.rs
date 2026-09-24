// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The MODES zones: every PHY on one log scale of sample rate, a line out to
//! the rate it needs, and this radio's ceiling as one rule down all of them,
//! with the headroom or the shortfall at the end of each row.
//!
//! **How far, and against what, at a glance.** The rows were limit rows
//! (idiom B), each a bracket the width of the panel with `min 2` in the
//! middle of it and the radio's own ceiling repeated on every line: true,
//! and nearly empty. One shared axis does what eight brackets could not: the
//! ceiling is a single rule, a mode that fits ends short of it, and one that
//! does not runs past it by as much as it is short, so 802.11b's 2 Msps and
//! VHT80's 60 read as different at a distance. Log, because the rates run
//! from 2 to 80 Msps and a linear axis would crush the Bluetooth modes into
//! the first few columns.
//!
//! Neither number was measured: the ceiling is the capability record's and
//! the need is the mode's stated minimum (`signal::net::gate::PHYS`), so the
//! margin is exact and carries no uncertainty. Fitting modes first, then the
//! rest, each under its own section, the order
//! `the_modes_are_sorted_by_what_the_rate_can_carry` pins. The word "pass"
//! still appears nowhere.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::hardware::DeviceCapabilities;
use crate::signal::net::gate::{admits, Phy, PHYS};
use crate::ui::chrome::section;

/// The narrowest track worth drawing; below it the rows keep their numbers
/// and lose the line.
const MIN_TRACK: usize = 12;

/// Where a rate sits on the track, in columns from its left edge.
struct Scale {
    lo: f64,
    hi: f64,
    track: usize,
}

impl Scale {
    /// Whole decades around every rate on the panel, so the ticks land on
    /// round numbers whatever the radio.
    fn around(rates: impl Iterator<Item = f64>, track: usize) -> Self {
        let (mut lo, mut hi) = (f64::INFINITY, 0.0f64);
        for r in rates.filter(|r| *r > 0.0) {
            lo = lo.min(r);
            hi = hi.max(r);
        }
        let lo = 10f64.powf(lo.log10().floor());
        let hi = 10f64.powf(hi.log10().ceil()).max(lo * 10.0);
        Self { lo, hi, track }
    }

    fn at(&self, rate: f64) -> f64 {
        (rate.max(self.lo).log10() - self.lo.log10()) / (self.hi.log10() - self.lo.log10())
            * self.track as f64
    }

    fn column(&self, rate: f64) -> usize {
        (self.at(rate) as usize).min(self.track.saturating_sub(1))
    }

    /// 1, 2 and 5 of every decade on the scale.
    fn ticks(&self) -> Vec<f64> {
        let mut out = Vec::new();
        let mut decade = self.lo;
        while decade <= self.hi * 1.0001 {
            for k in [1.0, 2.0, 5.0] {
                let t = decade * k;
                if t <= self.hi * 1.0001 {
                    out.push(t);
                }
            }
            decade *= 10.0;
        }
        out
    }
}

fn msps(hz: f64) -> String {
    format!("{:.1} Msps", hz / 1e6)
}

fn margin(caps: &DeviceCapabilities, phy: &Phy) -> String {
    format!("{:+.1} Msps", (caps.sample_rate_max_hz - phy.rate_hz) / 1e6)
}

/// A tick label: `1`, `20`, `0.5`, as short as the number allows.
fn tick_label(t: f64) -> String {
    if t >= 1.0 {
        format!("{t:.0}")
    } else {
        format!("{t}")
    }
}

/// The line for one mode: out to its need, the ceiling's rule where it
/// stands, and past it in the shortfall ink for a mode the radio cannot
/// carry, whose covered part recedes.
fn track_spans(
    need: f64,
    fits: bool,
    ceiling: usize,
    scale: &Scale,
    ticks: &[usize],
    theme: &crate::Theme,
) -> Vec<Span<'static>> {
    let end = scale.at(need);
    let bold = |c| Style::default().fg(c).add_modifier(Modifier::BOLD);
    (0..scale.track)
        .map(|c| {
            let fill = (end - c as f64).clamp(0.0, 1.0);
            let ink = match (c < ceiling, fits) {
                (true, true) => theme.status_ok,
                (true, false) => theme.label,
                (false, _) => theme.status_crit,
            };
            if c == ceiling {
                Span::styled("\u{2503}", bold(theme.value_hi))
            } else if fill >= 0.75 {
                Span::styled("\u{2501}", bold(ink))
            } else if fill >= 0.25 {
                Span::styled("\u{2578}", bold(ink))
            } else if ticks.contains(&c) {
                Span::styled("\u{250a}", Style::default().fg(theme.stale))
            } else if c % 2 == 0 {
                Span::styled("\u{00b7}", Style::default().fg(theme.stale))
            } else {
                Span::raw(" ")
            }
        })
        .collect()
}

/// Both zones, `iw` columns wide, and which of their lines may go first on a
/// short panel, most dispensable first: the ceiling's tag (the RATE row says
/// the same number) and the axis rule (the ticks' numbers stay). A zone with
/// no modes in it is left out rather than drawn empty.
pub(super) fn lines(
    caps: &DeviceCapabilities,
    iw: usize,
    theme: &crate::Theme,
) -> (Vec<Line<'static>>, Vec<usize>) {
    let (fit, out): (Vec<&Phy>, Vec<&Phy>) = PHYS.iter().partition(|p| admits(caps, p));
    let name_w = PHYS
        .iter()
        .map(|p| p.name.chars().count())
        .max()
        .unwrap_or(0);
    let need_w = PHYS
        .iter()
        .map(|p| msps(p.rate_hz).len())
        .max()
        .unwrap_or(0);
    let margin_w = PHYS
        .iter()
        .map(|p| margin(caps, p).len())
        .max()
        .unwrap_or(0);
    // indent, name, gap, need, gap, track, gap, margin
    let lead = 1 + name_w + 1 + need_w + 2;
    let track = iw.saturating_sub(lead + 2 + margin_w);
    let scale = Scale::around(
        PHYS.iter()
            .map(|p| p.rate_hz / 1e6)
            .chain([caps.sample_rate_max_hz / 1e6]),
        if track >= MIN_TRACK { track } else { 0 },
    );
    let ceiling = scale.column(caps.sample_rate_max_hz / 1e6);
    let ticks: Vec<usize> = scale.ticks().iter().map(|t| scale.column(*t)).collect();

    let mut lines = Vec::new();
    let mut optional = Vec::new();
    for (k, (name, group)) in [
        ("this radio can receive", &fit),
        ("beyond its sample rate", &out),
    ]
    .into_iter()
    .enumerate()
    {
        if group.is_empty() {
            continue;
        }
        lines.push(Line::from(""));
        lines.push(section(name, "", iw, theme));
        // Over the first zone drawn, whichever that is.
        if (k == 0 || fit.is_empty()) && scale.track > 0 {
            optional.push(lines.len());
            lines.push(ceiling_tag(caps, lead, ceiling, iw, theme));
        }
        for phy in group {
            let fits = admits(caps, phy);
            let mut spans = vec![
                Span::raw(" "),
                Span::styled(
                    format!("{:<name_w$}", phy.name),
                    Style::default().fg(theme.label),
                ),
                Span::raw(" "),
                Span::styled(
                    format!("{:>need_w$}", msps(phy.rate_hz)),
                    Style::default().fg(theme.value),
                ),
                Span::raw("  "),
            ];
            if scale.track > 0 {
                spans.extend(track_spans(
                    phy.rate_hz / 1e6,
                    fits,
                    ceiling,
                    &scale,
                    &ticks,
                    theme,
                ));
                spans.push(Span::raw("  "));
            }
            spans.push(Span::styled(
                format!("{:>margin_w$}", margin(caps, phy)),
                Style::default().fg(if fits {
                    theme.status_ok
                } else {
                    theme.status_crit
                }),
            ));
            lines.push(Line::from(spans));
        }
    }
    if scale.track > 0 && !lines.is_empty() {
        optional.push(lines.len());
        lines.extend(axis(&scale, lead, theme));
    }
    (lines, optional)
}

/// `this radio: 20.0 Msps ▼`, its arrow over the ceiling's rule; written
/// the other way, `▼ this radio: 20.0 Msps`, when the rule stands too near
/// the left for the words to fit before it.
fn ceiling_tag(
    caps: &DeviceCapabilities,
    lead: usize,
    ceiling: usize,
    iw: usize,
    theme: &crate::Theme,
) -> Line<'static> {
    let rate = msps(caps.sample_rate_max_hz);
    let left = format!("this radio: {rate} \u{25bc}");
    let at = lead + ceiling;
    let text = if at + 1 >= left.chars().count() {
        format!("{}{left}", " ".repeat(at + 1 - left.chars().count()))
    } else {
        format!("{}\u{25bc} this radio: {rate}", " ".repeat(at))
    };
    let text: String = text.chars().take(iw).collect();
    Line::from(Span::styled(text, Style::default().fg(theme.value_hi)))
}

/// The scale under the rows: a rule with a tick at 1, 2 and 5 of each decade,
/// the numbers under them where they do not collide, and the unit.
fn axis(scale: &Scale, lead: usize, theme: &crate::Theme) -> Vec<Line<'static>> {
    let mut rule: Vec<char> = vec!['\u{2500}'; scale.track];
    let mut labels: Vec<char> = vec![' '; scale.track];
    let mut free_from = 0usize;
    for t in scale.ticks() {
        let c = scale.column(t);
        rule[c] = '\u{2534}';
        let text = tick_label(t);
        let n = text.chars().count();
        let at = c.saturating_sub(n / 2).min(scale.track.saturating_sub(n));
        if at >= free_from {
            for (i, ch) in text.chars().enumerate() {
                labels[at + i] = ch;
            }
            free_from = at + n + 1;
        }
    }
    let pad = " ".repeat(lead);
    vec![
        Line::from(vec![
            Span::raw(pad.clone()),
            Span::styled(
                rule.into_iter().collect::<String>(),
                Style::default().fg(theme.border_dim),
            ),
        ]),
        Line::from(vec![
            Span::raw(pad),
            Span::styled(
                labels.into_iter().collect::<String>(),
                Style::default().fg(theme.label),
            ),
            Span::styled("  Msps, log", Style::default().fg(theme.label)),
        ]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::SdrMetrics;

    fn text(lines: &[Line<'_>]) -> String {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The fixture's 20 Msps ceiling: BLE 1M has 18 Msps to spare, HT20 fits
    /// with none, and HT40 is 20 Msps short. Each figure is the ceiling minus
    /// the mode's stated rate, exactly.
    #[test]
    fn each_mode_shows_its_headroom_or_its_shortfall() {
        let m = SdrMetrics::fixture();
        let out = text(&lines(&m.caps, 90, &crate::Theme::sdr()).0);
        let line = |name: &str| {
            out.lines()
                .find(|l| l.contains(name))
                .unwrap_or_else(|| panic!("{name}: {out}"))
                .to_string()
        };
        assert!(line("BLE 1M").contains("+18.0 Msps"), "{}", line("BLE 1M"));
        assert!(
            line("802.11a/g/n HT20").contains("+0.0 Msps"),
            "{}",
            line("HT20")
        );
        assert!(
            line("802.11n HT40").contains("-20.0 Msps"),
            "{}",
            line("HT40")
        );
        assert!(
            line("802.11n HT40").contains("40.0 Msps"),
            "{}",
            line("HT40")
        );
        // A limit row never says "pass", here or anywhere.
        assert!(!out.to_lowercase().contains("pass"), "{out}");
    }

    /// The ceiling is one rule down every row of both zones, with its tag's
    /// arrow over it, and each mode's line ends short of it or runs past it
    /// as its need is under or over.
    #[test]
    fn the_ceiling_is_one_rule_down_both_zones() {
        let m = SdrMetrics::fixture();
        let out = text(&lines(&m.caps, 120, &crate::Theme::sdr()).0);
        let col = |l: &str, c: char| l.chars().position(|x| x == c);
        let rules: Vec<usize> = out.lines().filter_map(|l| col(l, '\u{2503}')).collect();
        assert_eq!(rules.len(), PHYS.len(), "{out}");
        assert!(rules.windows(2).all(|p| p[0] == p[1]), "{rules:?}\n{out}");
        let arrow = out.lines().find_map(|l| col(l, '\u{25bc}')).expect(&out);
        assert_eq!(arrow, rules[0], "{out}");
        let past = |name: &str| {
            let l = out.lines().find(|l| l.contains(name)).unwrap();
            l.chars().skip(rules[0] + 1).any(|c| c == '\u{2501}')
        };
        assert!(!past("BLE 1M"), "{out}");
        assert!(!past("802.11a/g/n HT20"), "{out}");
        assert!(past("802.11b DSSS"), "{out}");
        assert!(past("802.11ac VHT80"), "{out}");
    }

    /// Another ceiling, an RTL-SDR's 3.2 Msps: the rule stands left of BLE
    /// 2M's need, so that line runs past it, and every row still fits.
    #[test]
    fn a_lower_ceiling_moves_the_rule() {
        let mut caps = (*SdrMetrics::fixture().caps).clone();
        caps.sample_rate_max_hz = 3.2e6;
        for iw in 46..160 {
            let out = text(&lines(&caps, iw, &crate::Theme::sdr()).0);
            for row in out.lines() {
                assert!(row.trim_end().chars().count() <= iw, "{iw}: {row:?}");
            }
        }
        let out = text(&lines(&caps, 120, &crate::Theme::sdr()).0);
        let ble2m = out.lines().find(|l| l.contains("BLE 2M")).expect(&out);
        assert!(ble2m.contains("-"), "{ble2m}");
        let rule = ble2m.chars().position(|c| c == '\u{2503}').expect(ble2m);
        assert!(
            ble2m.chars().skip(rule + 1).any(|c| c == '\u{2501}'),
            "{out}"
        );
    }

    /// The axis names the round numbers under the rows, in the scale's own
    /// decades.
    #[test]
    fn the_axis_is_labelled_in_decades() {
        let m = SdrMetrics::fixture();
        let out = text(&lines(&m.caps, 120, &crate::Theme::sdr()).0);
        let labels = out.lines().find(|l| l.contains("Msps, log")).expect(&out);
        for t in ["1", "10", "20", "100"] {
            assert!(labels.split_whitespace().any(|w| w == t), "{t}: {labels}");
        }
    }

    /// No row's visible text runs past the zone at any width the panel can be
    /// given. Trailing padding is not text: the paragraph clips it unseen.
    #[test]
    fn the_zones_fit_every_width() {
        let m = SdrMetrics::fixture();
        for iw in 46..160 {
            for row in text(&lines(&m.caps, iw, &crate::Theme::sdr()).0).lines() {
                assert!(row.trim_end().chars().count() <= iw, "{iw}: {row:?}");
            }
        }
    }
}
