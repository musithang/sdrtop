// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The MODES zones: one limit row per PHY, this radio's sample-rate ceiling
//! against the rate the mode needs, with the headroom or the shortfall.
//!
//! **How far, not only whether.** A list of names under "can" and "cannot"
//! said which side of the line each mode was on; the limit row (idiom B,
//! `widgets::limit`) says by how much, in Msps, so a radio 2 Msps short of
//! 802.11b reads differently from one 60 Msps short of VHT80. The reading is
//! the ceiling the capability record declares and the limit is the mode's
//! stated minimum (`signal::net::gate::PHYS`), so the margin is exactly the
//! ceiling minus the requirement and carries no uncertainty: neither number
//! was measured, and a sigma on either would be invented.
//!
//! Fitting modes first, then the rest, each under its own section, the order
//! the panel has always had and `the_modes_are_sorted_by_what_the_rate_can_carry`
//! pins. Both zones share one set of column widths, so the margins line up
//! down the whole list.

use ratatui::text::{Line, Span};

use crate::hardware::DeviceCapabilities;
use crate::signal::dsp::uncertainty::Uncertain;
use crate::signal::net::gate::{admits, Phy, PHYS};
use crate::ui::chrome::section;
use crate::ui::widgets::limit::{Limit, LimitRow, RowWidths};
use crate::ui::widgets::reading::Reading;

fn row(caps: &DeviceCapabilities, phy: &Phy) -> LimitRow<'static> {
    LimitRow::new(
        phy.name,
        // Exact, and never dashed: see the module doc. Stated to a tenth of a
        // Msps, the precision every mode's rate is given at.
        Reading::new(
            Uncertain::exact(caps.sample_rate_max_hz / 1e6),
            "Msps",
            f64::INFINITY,
        )
        .stated_to(1),
        Limit::Min(phy.rate_hz / 1e6),
    )
}

/// Both zones, `iw` columns wide. A zone with no modes in it is left out
/// rather than drawn empty.
pub(super) fn lines(
    caps: &DeviceCapabilities,
    iw: usize,
    theme: &crate::Theme,
) -> Vec<Line<'static>> {
    let (fit, out): (Vec<&Phy>, Vec<&Phy>) = PHYS.iter().partition(|p| admits(caps, p));
    let all: Vec<LimitRow<'static>> = PHYS.iter().map(|p| row(caps, p)).collect();
    // One space of indent under the section tab, the rhythm `chrome::field`
    // gives every other row on the panel, unless the panel is so narrow the
    // rows need that column too.
    let longest = |w: RowWidths| all.iter().map(|r| r.text(w).chars().count()).max();
    let indented = RowWidths::fit_within(&all, iw.saturating_sub(1));
    let (w, indent) = if longest(indented).unwrap_or(0) < iw {
        (indented, " ")
    } else {
        (RowWidths::fit_within(&all, iw), "")
    };
    let mut lines = Vec::new();
    for (name, group) in [
        ("this radio can receive", &fit),
        ("beyond its sample rate", &out),
    ] {
        if group.is_empty() {
            continue;
        }
        lines.push(Line::from(""));
        lines.push(section(name, "", iw, theme));
        for phy in group {
            let mut spans = vec![Span::raw(indent)];
            spans.extend(row(caps, phy).spans(theme, w));
            lines.push(Line::from(spans));
        }
    }
    lines
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
        let out = text(&lines(&m.caps, 90, &crate::Theme::sdr()));
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
        assert!(line("802.11n HT40").contains("min 40"), "{}", line("HT40"));
        // A limit row never says "pass", here or anywhere.
        assert!(!out.to_lowercase().contains("pass"), "{out}");
    }

    /// The margins line up down both zones: one set of widths for the list.
    #[test]
    fn the_rows_line_up_across_both_zones() {
        let m = SdrMetrics::fixture();
        let out = text(&lines(&m.caps, 90, &crate::Theme::sdr()));
        let cols: Vec<usize> = out
            .lines()
            .filter(|l| l.contains('['))
            .map(|l| l.chars().position(|c| c == '[').unwrap())
            .collect();
        assert!(cols.len() == PHYS.len(), "{out}");
        assert!(cols.windows(2).all(|p| p[0] == p[1]), "{cols:?}\n{out}");
    }

    /// No row's visible text runs past the zone at any width the panel can be
    /// given. Trailing padding is not text: the paragraph clips it unseen.
    #[test]
    fn the_zones_fit_every_width() {
        let m = SdrMetrics::fixture();
        for iw in 46..160 {
            for row in text(&lines(&m.caps, iw, &crate::Theme::sdr())).lines() {
                assert!(row.trim_end().chars().count() <= iw, "{iw}: {row:?}");
            }
        }
    }
}
