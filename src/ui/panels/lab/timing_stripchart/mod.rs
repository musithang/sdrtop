// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `timing_stripchart` - the centrepiece of the `lab_timing` bench.
//!
//! A real-time strip chart of the per-callback interval deviation from the
//! expected period (`state.timing.cb_deviations_us`, newest last). Positive bars
//! are late deliveries, negative are early; the deadline budget is drawn as a
//! faint guide line and, more usefully, every bar is coloured the instant it
//! crosses the budget, so late callbacks redden in place. Spikes beyond the axis
//! clamp and are tagged in the direction they blew out: `▲` at the top for a late
//! (positive) overrun, `▼` at the bottom for an early (negative) one.
//!
//! Split like `sweep_panel`, because it is the same kind of instrument -
//! coordinates, then the plotted data, then the text around it:
//!
//! - [`scale`]: the axis anchored to the budget, the gutter, the label and band
//!   rows.
//! - [`severity`]: how bad a column is and which way it blew out.
//! - [`chart`]: the strip.
//! - [`stats`]: the inset numbers the plot cannot show.
//! - [`captions`]: the description, the colour key and the window legend.

mod captions;
mod chart;
mod scale;
mod severity;
mod stats;

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::state::SdrMetrics;
use crate::ui::panel::{Panel, PanelChrome, Staleness};

pub struct TimingStripchartPanel;

impl Panel for TimingStripchartPanel {
    fn name(&self) -> &'static str {
        "timing_stripchart"
    }
    fn min_size(&self) -> (u16, u16) {
        (48, 12)
    }

    fn chrome(&self, _state: &SdrMetrics) -> PanelChrome {
        PanelChrome::new("Callback Interval")
            .stale_when(Staleness::NotStreaming)
            .suffix(" \u{00b7} Real-Time Strip Chart")
    }

    fn render(
        &self,
        f: &mut Frame,
        inner: Rect,
        state: &SdrMetrics,
        theme: &crate::Theme,
        _focused: bool,
    ) {
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        let stale = !state.radio.hw_streaming;
        let t = &state.timing;

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // description
                Constraint::Length(1), // stats line
                Constraint::Length(1), // blank
                Constraint::Min(3),    // chart
                Constraint::Length(1), // colour key
                Constraint::Length(1), // window legend
            ])
            .split(inner);

        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::raw(" "),
                Span::styled(
                    captions::description(inner.width as usize),
                    Style::default().fg(theme.label),
                ),
            ])),
            rows[0],
        );
        f.render_widget(Paragraph::new(stats::line(t, stale, theme)), rows[1]);
        chart::draw(f, rows[3], t, stale, theme);
        f.render_widget(
            Paragraph::new(captions::key_row(t.deadline_budget_us, theme)),
            rows[4],
        );
        f.render_widget(
            Paragraph::new(captions::window_row(rows[3].width, t, stale, theme)),
            rows[5],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::fixture::draw;

    const W: u16 = 80;
    const H: u16 = 18;

    fn live() -> SdrMetrics {
        SdrMetrics::fixture().streaming().with_timing(0.4)
    }

    /// The three "nothing to plot" states each say why, rather than drawing an
    /// empty grid the reader has to interpret.
    #[test]
    fn an_empty_chart_says_why() {
        let idle = draw(TimingStripchartPanel, W, H, &SdrMetrics::fixture()).join("\n");
        assert!(idle.contains("IDLE"), "{idle}");
        assert!(idle.contains("RX stopped"), "{idle}");

        let mut waiting = live();
        waiting.timing.cb_deviations_us.clear();
        let out = draw(TimingStripchartPanel, W, H, &waiting).join("\n");
        assert!(out.contains("waiting for callbacks"), "{out}");
    }

    /// The axis is anchored to the budget, so its labels are a function of the
    /// budget rather than of whatever the worst sample happened to be.
    #[test]
    fn the_axis_is_anchored_to_the_budget_not_the_peak() {
        let calm = live();
        let full = scale::full_scale_us(calm.timing.deadline_budget_us);
        let calm_out = draw(TimingStripchartPanel, W, H, &calm).join("\n");
        assert!(
            calm_out.contains(&scale::fmt_axis(full)),
            "top label {} missing:\n{calm_out}",
            scale::fmt_axis(full)
        );

        // A huge spike must not move the axis.
        let mut spiky = live();
        spiky.timing.cb_deviations_us[5] = 500_000;
        let spiky_out = draw(TimingStripchartPanel, W, H, &spiky).join("\n");
        assert!(
            spiky_out.contains(&scale::fmt_axis(full)),
            "the axis moved under a spike:\n{spiky_out}"
        );
    }

    /// A spike past the axis is tagged in the direction it left, so a clamped bar
    /// is not mistaken for one that merely reached the top.
    #[test]
    fn spikes_past_the_axis_are_tagged_in_their_direction() {
        // The chart plots the *last* two samples per column, so a spike has to
        // be near the end of the series to be on screen at all.
        let mut late = live();
        let n = late.timing.cb_deviations_us.len();
        late.timing.cb_deviations_us[n - 4] = 400_000;
        let out = draw(TimingStripchartPanel, W, H, &late).join("\n");
        assert!(out.contains('\u{25B2}'), "no late-spike tag:\n{out}");

        let mut early = live();
        early.timing.cb_deviations_us[n - 4] = -400_000;
        let out = draw(TimingStripchartPanel, W, H, &early).join("\n");
        assert!(out.contains('\u{25BC}'), "no early-spike tag:\n{out}");
    }

    /// The legend names the budget the guide line marks, so the two cannot say
    /// different numbers.
    #[test]
    fn the_legend_names_the_budget_the_guide_marks() {
        let m = live();
        let out = draw(TimingStripchartPanel, W, H, &m).join("\n");
        assert!(
            out.contains(&format!(
                "{} \u{00b5}s deadline",
                m.timing.deadline_budget_us
            )),
            "{out}"
        );
    }

    /// The description is never clipped: at every width the panel can be given,
    /// the sentence it picked ends where it means to.
    #[test]
    fn the_description_is_never_cut_off() {
        let m = live();
        let (min_w, _) = TimingStripchartPanel.min_size();
        for w in [min_w, 56, 60, 80, 100, 113, 140, 200] {
            let out = draw(TimingStripchartPanel, w, H, &m);
            // Row 1 is the description; strip the frame the engine drew round it.
            let desc = out[1].trim_matches(|c| c == '\u{2502}').trim();
            assert!(
                !desc.is_empty(),
                "width {w}: no description at all:\n{}",
                out.join("\n")
            );
            assert_eq!(
                desc,
                super::captions::description(w as usize - 2),
                "width {w}: drew a form the width does not select"
            );
        }
    }

    #[test]
    fn it_fits_every_size_the_layout_can_hand_it() {
        let m = live();
        let (min_w, min_h) = TimingStripchartPanel.min_size();
        for (w, h) in [(min_w, min_h), (60, 14), (W, H), (140, 30)] {
            let out = draw(TimingStripchartPanel, w, h, &m);
            assert_eq!(out.len(), h as usize, "{w}x{h}: wrong row count");
            assert!(
                out.iter().all(|l| l.chars().count() <= w as usize),
                "{w}x{h}: a row overran the panel"
            );
        }
    }
}
