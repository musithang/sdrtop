// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `timing_diagnostics` - the numbers column of the `lab_timing` preset.
//!
//! Three zones and a verdict: how well the stream is keeping up, how close the
//! worst of it came to the threshold, and whether the sample clock is delivering
//! what was asked for.
//!
//! **The first zone depends on how samples arrive.** A push backend gets
//! [`callback`] and [`deadline`], which measure block arrival against a deadline
//! the driver's pacing makes meaningful. A pull backend gets [`read_loop`]
//! instead, because there the interval between blocks is our own loop's rhythm.
//!
//! Split by zone, which is also the order the question narrows - is the stream
//! regular, is it late, is the clock right:
//!
//! - [`rows`]: the label field, the `---` stale form, the trend and the budget
//!   bar. The label column is the same width `timing_vitals` uses, so the two
//!   halves of the bench line up.
//! - [`callback`], [`deadline`], [`sample_rate`]: one module per zone.
//! - [`verdict`]: the closing lines. `verdict_copy` draws nothing.

mod callback;
mod deadline;
mod read_loop;
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
        // The first two zones are the ones that only mean something when the
        // driver paces the blocks. A pull backend gets the question it can
        // actually answer instead. See `read_loop` for why.
        match state.caps.delivery {
            crate::hardware::DeliveryModel::Push => {
                lines.extend(callback::lines(state, &r));
                lines.push(Line::raw(""));
                lines.extend(deadline::lines(state, &r));
            }
            crate::hardware::DeliveryModel::Pull => {
                lines.extend(read_loop::lines(state, &r));
            }
        }
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

    /// The swap this checkpoint exists for. A pull backend is shown the zone it
    /// can answer, and is not shown the two it cannot.
    #[test]
    fn a_pull_backend_gets_the_read_loop_zone_instead_of_the_deadline_zones() {
        let out = draw(TimingDiagnosticsPanel, W, H, &live().pulling(0.65)).join("\n");
        assert!(out.contains("READ LOOP"), "no read-loop zone:\n{out}");
        assert!(
            !out.contains("DEADLINE BUDGET"),
            "a pull loop was given a deadline:\n{out}"
        );
        assert!(
            !out.contains("CALLBACK TIMING"),
            "a pull loop has no callbacks:\n{out}"
        );
        // And the push device still gets both, unchanged.
        let push = draw(TimingDiagnosticsPanel, W, H, &live()).join("\n");
        assert!(push.contains("CALLBACK TIMING") && push.contains("DEADLINE BUDGET"));
        assert!(!push.contains("READ LOOP"));
    }

    /// The original complaint, end to end through the renderer: read intervals
    /// wild enough to fail a deadline, a read loop with a third of itself spare,
    /// and nothing lost. The bench must call that healthy.
    #[test]
    fn a_healthy_pull_stream_with_wild_read_intervals_reads_as_healthy() {
        let wild = SdrMetrics::fixture()
            .streaming()
            .with_timing(4.7)
            .pulling(0.65);
        let out = draw(TimingDiagnosticsPanel, W, H, &wild).join("\n");
        assert!(
            out.contains("EXCELLENT"),
            "graded a healthy pull link:\n{out}"
        );
        assert!(out.contains("65 %"), "no occupancy figure:\n{out}");

        // The identical stream on a push backend is still a fault, because
        // there the interval really is the driver's.
        let pushed = SdrMetrics::fixture().streaming().with_timing(4.7);
        let out = draw(TimingDiagnosticsPanel, W, H, &pushed).join("\n");
        assert!(
            out.contains("POOR"),
            "a push link with 4.7x jitter is fine?\n{out}"
        );
    }

    /// A read loop that never blocks is behind, and says so before a drop.
    #[test]
    fn a_saturated_read_loop_is_called_out() {
        let out = draw(TimingDiagnosticsPanel, W, H, &live().pulling(0.99)).join("\n");
        assert!(out.contains("POOR"), "{out}");
        assert!(out.contains("not keeping up"), "{out}");
        assert!(
            !out.to_lowercase().contains("dropped"),
            "nothing was lost yet:\n{out}"
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
    /// the comparison is a dash - not a fabricated match.
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

#[cfg(test)]
mod vocabulary {
    use super::*;
    use crate::state::fixture::draw;

    /// The one place a pull panel may say "deadline": to say it does not have
    /// one. Anything else is the panel describing a mechanism the device does
    /// not use.
    const DELIBERATE: &str = "not a deadline";

    /// Every timing panel, rendered against a pull backend, swept for push-only
    /// vocabulary.
    ///
    /// **This found four leaks that the per-panel tests did not.** Each of those
    /// tests asserted what its own panel should say; none of them asked what the
    /// whole screen still said. On a healthy SoapySDR link the bench printed
    /// "Every callback met its deadline." immediately followed by "Worst 2.886
    /// ms (470% of budget)", which is two facts the device does not have and a
    /// contradiction between them, plus a panel titled "Callback Interval"
    /// plotting reads, described as "per-callback interval deviation from the
    /// expected period", with "one point per RX callback" beneath it.
    ///
    /// A word list is a blunt instrument and that is the point: it does not need
    /// to know which sentence is wrong, only that the vocabulary has no business
    /// being there.
    #[test]
    fn no_timing_panel_speaks_of_callbacks_or_deadlines_to_a_pull_backend() {
        let pull = SdrMetrics::fixture()
            .streaming()
            .with_timing(4.7)
            .pulling(0.65);
        let push = SdrMetrics::fixture().streaming().with_timing(4.7);

        // Push-only mechanisms, and nothing else. "Overrun" and "dropped" are
        // deliberately absent: a pull backend loses samples too (SoapySDR
        // returns SOAPY_SDR_OVERFLOW), so a ring buffer's overrun margin is a
        // real reading on both. The list is about mechanisms the device does
        // not have, not about alarming words.
        let words = ["callback", "deadline", "budget", "late"];
        let render = |m: &SdrMetrics| {
            vec![
                (
                    "timing_diagnostics",
                    draw(TimingDiagnosticsPanel, 60, 34, m).join("\n"),
                ),
                (
                    "timing_stripchart",
                    draw(crate::ui::TimingStripchartPanel, 96, 22, m).join("\n"),
                ),
                (
                    "timing_vitals",
                    draw(crate::ui::TimingVitalsPanel, 50, 30, m).join("\n"),
                ),
            ]
        };

        let mut leaks = String::new();
        for (name, out) in render(&pull) {
            for line in out.lines() {
                let lower = line.to_lowercase();
                if lower.contains(DELIBERATE) {
                    continue;
                }
                if let Some(w) = words.iter().find(|w| lower.contains(**w)) {
                    leaks.push_str(&format!("  {name}: {w:?} in {:?}\n", line.trim()));
                }
            }
        }
        assert!(
            leaks.is_empty(),
            "a pull backend was told about a mechanism it does not have:\n{leaks}"
        );

        // And the push backend must still be told all of it, or this test would
        // pass just as well by deleting the vocabulary everywhere.
        let pushed: String = render(&push).into_iter().map(|(_, o)| o).collect();
        let lower = pushed.to_lowercase();
        for w in ["callback", "deadline", "budget", "late"] {
            assert!(lower.contains(w), "push lost its own vocabulary: {w:?}");
        }
    }
}
