// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `IqHistogramPanel` - where the ADC's range is actually being used.
//!
//! A log-scaled chart of the 32-bin I/Q amplitude histogram, split into Low / Mid
//! / Clip zones, with the zone shares, a PAPR estimate and a one-line verdict
//! under it. Answers the question a gain control cannot: not "how loud is the
//! peak", but "where does the *bulk* of the signal sit".
//!
//! Split measure-then-draw, the same seam `iq_constellation` and `image_scope`
//! got in R4f:
//!
//! - [`zones`]: the three amplitude zones, their counts, and the log heights.
//!   The zone boundaries live here and nowhere else.
//! - [`papr`]: the crest-factor estimate. Pure arithmetic, tested since before
//!   the split.
//! - [`chart`]: the canvas.
//! - [`axis`]: the zone label row under it.
//! - [`readout`]: the three text rows.

mod axis;
mod chart;
mod papr;
mod readout;
mod zones;

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    Frame,
};

use crate::state::SdrMetrics;
use crate::ui::panel::{Panel, PanelChrome, Staleness};

use zones::{Zones, BINS};

pub struct IqHistogramPanel;

impl Panel for IqHistogramPanel {
    fn name(&self) -> &'static str {
        "iq_histogram"
    }
    fn min_size(&self) -> (u16, u16) {
        (36, 9)
    }

    fn chrome(&self, _state: &SdrMetrics) -> PanelChrome {
        PanelChrome::new("IQ Amplitude Distribution").stale_when(Staleness::NotStreaming)
    }

    fn render(
        &self,
        f: &mut Frame,
        inner: Rect,
        state: &SdrMetrics,
        theme: &crate::Theme,
        _focused: bool,
    ) {
        // Four fixed rows under the chart; below this there is no room for a
        // chart at all and half a panel is worse than none.
        if inner.height < 5 || inner.width < 4 {
            return;
        }

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(1),    // chart
                Constraint::Length(1), // zone axis
                Constraint::Length(1), // Low / Mid / Clip
                Constraint::Length(1), // PAPR
                Constraint::Length(1), // verdict
            ])
            .split(inner);

        let hist = &state.iq.iq_amplitude_hist;
        let z = Zones::of(hist);
        let n_bins = BINS.min(rows[0].width as usize);

        chart::draw(f, rows[0], hist, n_bins, theme);
        axis::draw(f, rows[1], rows[0].width, n_bins, theme);
        readout::breakdown(f, rows[2], &z, theme);
        readout::papr(f, rows[3], hist, z.total, theme);
        readout::status(f, rows[4], &z, theme);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::fixture::draw;

    fn with_hist(fill: impl Fn(usize) -> u64) -> SdrMetrics {
        let mut m = SdrMetrics::fixture().streaming();
        for (i, b) in m.iq.iq_amplitude_hist.iter_mut().enumerate() {
            *b = fill(i);
        }
        m
    }

    /// The verdict is the reason to glance at this panel, so each of its four
    /// states has to be reachable from a plausible histogram.
    #[test]
    fn each_verdict_state_is_reachable() {
        let empty = draw(IqHistogramPanel, 44, 12, &SdrMetrics::fixture().streaming()).join("\n");
        assert!(empty.contains("No samples yet"), "{empty}");

        let clipping = with_hist(|i| if i >= 29 { 4000 } else { 40 });
        assert!(draw(IqHistogramPanel, 44, 12, &clipping)
            .join("\n")
            .contains("clipping risk"));

        let weak = with_hist(|i| if i < 2 { 9000 } else { 1 });
        assert!(draw(IqHistogramPanel, 44, 12, &weak)
            .join("\n")
            .contains("weak signal"));

        let healthy = with_hist(|i| if (8..20).contains(&i) { 1000 } else { 10 });
        assert!(draw(IqHistogramPanel, 44, 12, &healthy)
            .join("\n")
            .contains("Dynamic range OK"));
    }

    /// The three zone shares are of one histogram, so they have to add up.
    #[test]
    fn the_three_percentages_account_for_the_samples() {
        let m = with_hist(|i| (i as u64 + 1) * 3);
        let out = draw(IqHistogramPanel, 60, 12, &m).join("\n");
        let row = out
            .lines()
            .find(|l| l.contains("Low ") && l.contains("Clip "))
            .expect("no breakdown row");
        let nums: Vec<u64> = row
            .split('%')
            .filter_map(|s| s.trim().rsplit(' ').next()?.parse().ok())
            .collect();
        assert_eq!(nums.len(), 3, "expected three percentages in {row:?}");
        let sum: u64 = nums.iter().sum();
        assert!(
            (98..=100).contains(&sum),
            "the zones should account for everything, got {sum} from {row:?}"
        );
    }

    /// The `── OK ──` label is padded by character count, not byte length. With
    /// `len()` the three-byte box characters over-pad by eight columns and push
    /// the label off a narrow panel entirely. (This replaces a test that asserted
    /// `str::chars()` works, against slicing code the panel no longer has.)
    #[test]
    fn the_mid_label_survives_a_narrow_panel() {
        let m = with_hist(|_| 100);
        for w in [36u16, 40, 44, 60, 100] {
            let out = draw(IqHistogramPanel, w, 12, &m);
            assert!(
                out.iter().any(|l| l.contains("OK")),
                "width {w}: the axis label was pushed off screen:\n{}",
                out.join("\n")
            );
            assert!(
                out.iter().all(|l| l.chars().count() <= w as usize),
                "width {w}: a row overran the panel"
            );
        }
    }

    /// Below the height the four text rows need, the panel draws nothing rather
    /// than a chart with its readouts cut off.
    #[test]
    fn too_short_to_hold_the_readouts_draws_nothing() {
        let m = with_hist(|_| 100);
        for h in [3u16, 5, 6] {
            let out = draw(IqHistogramPanel, 44, h, &m);
            let body = out[1..out.len().saturating_sub(1)].join("");
            if h < 7 {
                assert!(
                    !body.contains("PAPR"),
                    "height {h} drew readouts it has no room for:\n{}",
                    out.join("\n")
                );
            }
        }
        // With room, they are there.
        let full = draw(IqHistogramPanel, 44, 12, &m).join("\n");
        assert!(full.contains("PAPR"), "{full}");
    }
}
