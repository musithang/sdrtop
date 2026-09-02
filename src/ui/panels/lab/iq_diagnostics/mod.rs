// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `IqDiagnosticsPanel` - the front end's I/Q health, as four stacked blocks.
//!
//! DC offset, quadrature balance, image rejection, then one plain-language
//! verdict and the live correction state. Every reading is the **residual** after
//! any active correction, which is what makes a lit chip beside a bad number
//! meaningful: the correction is not keeping up.
//!
//! Split on the seam a bench instrument actually has - measure, then rate, then
//! draw:
//!
//! - [`reading`]: the six numbers, computed once. Pure maths.
//! - [`severity`]: where each of them stops being fine. Thresholds + colours.
//! - [`rows`]: the row vocabulary the sections share, so every meter, bar and
//!   value starts and ends at the same column.
//! - [`dc`], [`quadrature`], [`image`]: one module per block of the panel.
//! - [`verdict`]: which single thing to say. Drawing-free.
//! - [`controls`]: the action chips and the status foot.
//!
//! This function's job is to carve the stack and call each part once; the parts
//! do not call each other.

mod controls;
mod dc;
mod image;
mod quadrature;
mod reading;
mod rows;
mod severity;
mod verdict;

use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::state::SdrMetrics;
use crate::ui::panel::{Panel, PanelChrome, Staleness};

use reading::Reading;
use rows::Rows;

pub struct IqDiagnosticsPanel;

impl Panel for IqDiagnosticsPanel {
    fn name(&self) -> &'static str {
        "iq_diagnostics"
    }
    fn min_size(&self) -> (u16, u16) {
        (30, 12)
    }
    fn focus_key(&self) -> Option<char> {
        Some('i')
    }
    fn focus_bindings(&self) -> &'static [(&'static str, &'static str)] {
        &[
            ("D", "DC-block"),
            ("C", "auto-cal"),
            ("F", "freeze"),
            ("M", "pin"),
        ]
    }

    fn chrome(&self, _state: &SdrMetrics) -> PanelChrome {
        PanelChrome::new("_IQ Diagnostics").stale_when(Staleness::NotStreaming)
    }

    fn render(
        &self,
        f: &mut Frame,
        inner: Rect,
        state: &SdrMetrics,
        theme: &crate::Theme,
        _focused: bool,
    ) {
        if !state.radio.hw_streaming {
            f.render_widget(
                Paragraph::new(Span::styled("---", Style::default().fg(theme.label))),
                inner,
            );
            return;
        }

        let rows = Rows::new(inner.width as usize, theme);
        let r = Reading::of(state);

        let mut lines: Vec<Line> = Vec::new();
        lines.extend(dc::lines(&r, &rows));
        lines.push(Line::raw(""));
        lines.extend(quadrature::lines(&r, &rows));
        lines.push(Line::raw(""));
        lines.extend(image::lines(state, &r, &rows));
        lines.push(Line::raw(""));
        lines.extend(verdict::lines(&verdict::decide(&r, &state.iq.cal), &rows));
        lines.extend(controls::lines(&state.iq.cal, &rows));

        // Self-adjusting density: collapse spacers when the pane is short, grow
        // them to fill when it is tall, so the stack breathes the same at every
        // height. The section nameplates keep the grouping either way.
        crate::ui::chrome::fit_spacers(&mut lines, inner.height as usize);
        f.render_widget(Paragraph::new(lines), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::fixture::draw;

    fn bench() -> SdrMetrics {
        let mut m = SdrMetrics::fixture().streaming();
        m.iq.dc_offset_i = 0.0008;
        m.iq.dc_offset_q = -0.0011;
        m.iq.iq_imbalance_db = 0.12;
        m.iq.phase_imbalance_deg = 0.4;
        m
    }

    /// A stopped radio shows no readings at all, rather than the last ones it
    /// happened to have. The frame's `[STALE]` tag is the engine's job.
    #[test]
    fn a_stopped_radio_draws_no_readings() {
        let out = draw(IqDiagnosticsPanel, 60, 24, &SdrMetrics::fixture()).join("\n");
        assert!(out.contains("---"), "{out}");
        assert!(
            !out.contains("DC OFFSET"),
            "stale panel drew a section:\n{out}"
        );
        assert!(out.contains("STALE"), "the frame should be tagged:\n{out}");
    }

    /// All four blocks are present in the order the panel promises.
    #[test]
    fn the_four_blocks_appear_in_order() {
        let out = draw(IqDiagnosticsPanel, 60, 30, &bench()).join("\n");
        let dc = out.find("DC OFFSET").expect("no DC block");
        let quad = out.find("QUADRATURE").expect("no quadrature block");
        let img = out.find("IMAGE REJECTION").expect("no image block");
        let verdict = out.find("IQ QUALITY OK").expect("no verdict");
        assert!(
            dc < quad && quad < img && img < verdict,
            "out of order:\n{out}"
        );
    }

    /// The chips fall back to single letters before the freeze chip would be
    /// clipped off the right edge.
    #[test]
    fn narrow_panels_drop_to_single_letter_chips() {
        let wide = draw(IqDiagnosticsPanel, 60, 30, &bench()).join("\n");
        assert!(wide.contains("DC-block "), "full labels missing:\n{wide}");

        let narrow = draw(IqDiagnosticsPanel, 34, 30, &bench()).join("\n");
        assert!(
            !narrow.contains("D DC-block"),
            "narrow panel kept the full chip labels:\n{narrow}"
        );
    }

    /// The panel fills whatever height it is given without spilling out of it.
    #[test]
    fn it_fits_every_height_from_min_size_upward() {
        let m = bench();
        let (min_w, min_h) = IqDiagnosticsPanel.min_size();
        for (w, h) in [(min_w, min_h), (44, 18), (60, 30), (80, 40)] {
            let out = draw(IqDiagnosticsPanel, w, h, &m);
            assert_eq!(out.len(), h as usize, "{w}x{h}: wrong row count");
            assert!(
                out.iter().all(|l| l.chars().count() <= w as usize),
                "{w}x{h}: a row overran the panel"
            );
        }
    }
}
