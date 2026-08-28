//! `timing_diagnostics` — the numbers column of the `lab_timing` preset.
//!
//! Three zones and a verdict: how regularly callbacks arrive, how close the worst
//! of them came to the deadline budget, and whether the sample clock is
//! delivering what was asked for.
//!
//! Split by zone, which is also the order the question narrows — is the stream
//! regular, is it late, is the clock right:
//!
//! - [`rows`]: the label field, the `---` stale form, the trend and the budget
//!   bar. The label column is the same width `timing_vitals` uses, so the two
//!   halves of the bench line up.
//! - [`callback`], [`deadline`], [`sample_rate`]: one module per zone.
//! - [`verdict`]: the closing lines. `verdict_copy` draws nothing.

mod callback;
mod deadline;
mod rows;
mod sample_rate;
mod verdict;

use ratatui::{layout::Rect, text::Line, widgets::Paragraph, Frame};

use crate::state::SdrMetrics;
use crate::ui::panel::{Panel, PanelChrome, Staleness};

use rows::Rows;

pub struct TimingDiagnosticsPanel;

impl Panel for TimingDiagnosticsPanel {
    fn name(&self) -> &'static str {
        "timing_diagnostics"
    }
    fn min_size(&self) -> (u16, u16) {
        (34, 18)
    }
    fn focus_key(&self) -> Option<char> {
        Some('t')
    }
    fn focus_bindings(&self) -> &'static [(&'static str, &'static str)] {
        &[("R", "Reset jitter peak"), ("C", "Clear history")]
    }

    fn chrome(&self, _state: &SdrMetrics) -> PanelChrome {
        PanelChrome::new("_Timing Diagnostics").stale_when(Staleness::NotStreaming)
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
        let r = Rows::new(inner.width as usize, !state.radio.hw_streaming, theme);

        let mut lines: Vec<Line> = Vec::new();
        lines.extend(callback::lines(state, &r));
        lines.push(Line::raw(""));
        lines.extend(deadline::lines(state, &r));
        lines.push(Line::raw(""));
        lines.extend(sample_rate::lines(state, &r));
        lines.push(Line::raw(""));
        lines.extend(verdict::lines(state, &r));

        crate::ui::chrome::fit_spacers(&mut lines, inner.height as usize);
        f.render_widget(Paragraph::new(lines), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::fixture::draw;

    const W: u16 = 56;
    const H: u16 = 30;

    fn live() -> SdrMetrics {
        SdrMetrics::fixture().streaming().with_timing(0.3)
    }

    #[test]
    fn a_stopped_radio_dashes_its_readings() {
        let out = draw(TimingDiagnosticsPanel, W, H, &SdrMetrics::fixture()).join("\n");
        assert!(out.contains("---"), "{out}");
        assert!(out.contains("IDLE"), "no idle verdict:\n{out}");
        assert!(
            !out.contains("EXCELLENT"),
            "a stale panel graded the stream:\n{out}"
        );
    }

    #[test]
    fn the_three_zones_appear_in_order() {
        let out = draw(TimingDiagnosticsPanel, W, H, &live()).join("\n");
        let cb = out.find("CALLBACK TIMING").expect("no callback zone");
        let dl = out.find("DEADLINE BUDGET").expect("no deadline zone");
        let sr = out.find("SAMPLE RATE").expect("no sample-rate zone");
        assert!(cb < dl && dl < sr, "zones out of order:\n{out}");
    }

    /// Callbacks comfortably inside the budget say so; ones well over it are
    /// counted. The two must not read the same.
    #[test]
    fn the_late_count_distinguishes_inside_from_over_budget() {
        let inside = draw(TimingDiagnosticsPanel, W, H, &live()).join("\n");
        assert!(inside.contains("none over budget"), "{inside}");

        let over = SdrMetrics::fixture().streaming().with_timing(2.5);
        let out = draw(TimingDiagnosticsPanel, W, H, &over).join("\n");
        assert!(
            out.contains("over budget") && !out.contains("none over budget"),
            "an over-budget stream still read clean:\n{out}"
        );
    }

    /// The budget marker is on the nameplate, so a reader can see what the bars
    /// are measured against without leaving the zone.
    #[test]
    fn the_budget_is_stated_on_the_zone_heading() {
        let out = draw(TimingDiagnosticsPanel, W, H, &live()).join("\n");
        let budget = live().timing.deadline_budget_us;
        assert!(
            out.contains(&format!("{budget}")),
            "the budget {budget} µs is not printed:\n{out}"
        );
    }

    /// Until the device reports an actual rate, the configured one is shown and
    /// the comparison is a dash — not a fabricated match.
    #[test]
    fn an_unreported_sample_rate_is_a_dash_not_a_match() {
        let mut m = live();
        m.radio.actual_sample_rate = 0;
        let out = draw(TimingDiagnosticsPanel, W, H, &m).join("\n");
        assert!(
            out.contains("10.000 MHz"),
            "configured rate missing:\n{out}"
        );
        assert!(
            !out.contains("\u{2192}"),
            "drew a comparison it cannot make:\n{out}"
        );
    }

    #[test]
    fn it_fits_every_size_the_layout_can_hand_it() {
        let m = live();
        let (min_w, min_h) = TimingDiagnosticsPanel.min_size();
        for (w, h) in [(min_w, min_h), (44, 22), (W, H), (100, 40)] {
            let out = draw(TimingDiagnosticsPanel, w, h, &m);
            assert_eq!(out.len(), h as usize, "{w}x{h}: wrong row count");
            assert!(
                out.iter().all(|l| l.chars().count() <= w as usize),
                "{w}x{h}: a row overran the panel"
            );
        }
    }
}
