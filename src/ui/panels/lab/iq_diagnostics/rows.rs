// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The row vocabulary the three measurement sections share.
//!
//! Four shapes: a bipolar null-meter for a deviation from zero, a gradient bar
//! for a quality that runs one way, a plain right-aligned readout, and a filled
//! chip. All four take their widths from one place, which is what keeps every
//! meter, bar and value starting and ending at the same column.
//!
//! These were four closures inside `render`, captured over `iw`, `theme`,
//! `field_w` and `track_w`. As a struct the widths are computed once and named,
//! and each section can be read without scrolling past them.

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use crate::ui::widgets::charts::{gain_bar_colored, null_meter};

/// Fixed label field: `" LBL "` - space + three columns + space.
const LEAD: usize = 5;
/// Right-hand value budget, so the numbers align down the panel.
const VALUE_W: usize = 10;

pub(super) struct Rows<'a> {
    /// Inner panel width.
    pub iw: usize,
    pub theme: &'a crate::Theme,
    /// Visual width shared by the meter (arrows + track) and the gradient bar.
    field_w: usize,
    /// `field_w` minus the two columns `null_meter` spends on its arrows.
    track_w: usize,
    dim: Color,
    lbl: Style,
}

impl<'a> Rows<'a> {
    pub(super) fn new(iw: usize, theme: &'a crate::Theme) -> Self {
        let field_w = iw.saturating_sub(LEAD + 1 + VALUE_W).max(8);
        Self {
            iw,
            theme,
            field_w,
            track_w: field_w.saturating_sub(2),
            dim: theme.border_dim,
            lbl: Style::default().fg(theme.label),
        }
    }

    pub(super) fn label_style(&self) -> Style {
        self.lbl
    }

    pub(super) fn dim(&self) -> Color {
        self.dim
    }

    /// `" text ………… value"` - value right-aligned to the panel edge.
    pub(super) fn readout(&self, text: &str, val: String, color: Color) -> Line<'static> {
        let pad = self
            .iw
            .saturating_sub(1 + text.chars().count() + val.chars().count());
        Line::from(vec![
            Span::raw(" "),
            Span::styled(text.to_string(), self.lbl),
            Span::raw(" ".repeat(pad.max(1))),
            Span::styled(val, Style::default().fg(color).add_modifier(Modifier::BOLD)),
        ])
    }

    /// Filled cockpit chip, the same pill style as the command-rail mode tabs.
    /// `active` lights it from the live correction state.
    pub(super) fn chip(&self, label: &str, active: bool) -> Span<'static> {
        let bg = if active {
            self.theme.value_hi
        } else {
            self.theme.border_dim
        };
        let mut st = Style::default().bg(bg).fg(Color::Rgb(4, 6, 15));
        if active {
            st = st.add_modifier(Modifier::BOLD);
        }
        Span::styled(format!(" {label} "), st)
    }

    /// `" LBL "` + bipolar null-meter + value. For a reading whose ideal is zero
    /// and which can err either way.
    pub(super) fn meter(
        &self,
        label: &str,
        value: f64,
        full_scale: f64,
        color: Color,
        val_str: String,
    ) -> Line<'static> {
        let mut spans = self.lead(label);
        spans.extend(null_meter(value, full_scale, self.track_w, color, self.dim));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            val_str,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ));
        Line::from(spans)
    }

    /// `" LBL "` + gradient quality bar + value. `frac` 0..1 maps the fill; for a
    /// reading that only runs one way, so `lo`/`hi` say which end is good.
    pub(super) fn bar(
        &self,
        label: &str,
        frac: f64,
        lo: Color,
        hi: Color,
        val_color: Color,
        val_str: String,
    ) -> Line<'static> {
        let v = (frac.clamp(0.0, 1.0) * 1000.0) as u32;
        let mut spans = self.lead(label);
        spans.extend(gain_bar_colored(v, 1000, self.field_w, lo, hi, self.dim));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            val_str,
            Style::default().fg(val_color).add_modifier(Modifier::BOLD),
        ));
        Line::from(spans)
    }

    fn lead(&self, label: &str) -> Vec<Span<'static>> {
        vec![
            Span::raw(" "),
            Span::styled(format!("{label:<3}"), self.lbl),
            Span::raw(" "),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Theme;

    fn width_of(line: &Line<'static>) -> usize {
        line.spans.iter().map(|s| s.content.chars().count()).sum()
    }

    /// Every meter and bar must occupy the same width, or the value column
    /// wanders down the panel and the readings stop lining up.
    #[test]
    fn meters_and_bars_share_one_field_width() {
        let t = Theme::sdr();
        for iw in [30usize, 36, 44, 60, 80] {
            let r = Rows::new(iw, &t);
            // Same value text on both, or the comparison measures the values
            // rather than the fields.
            let val = "+0.0100";
            let meter = r.meter("I", 0.01, 0.05, t.status_ok, val.into());
            let bar = r.bar(
                "MAG",
                0.3,
                t.status_ok,
                t.status_crit,
                t.status_ok,
                val.into(),
            );
            assert_eq!(
                width_of(&meter),
                width_of(&bar),
                "iw={iw}: a meter and a bar with the same value must end in the same column"
            );
        }
    }

    /// A panel narrower than the label + value budget still has to draw something
    /// rather than underflow to zero width.
    #[test]
    fn a_very_narrow_panel_keeps_a_usable_field() {
        let t = Theme::sdr();
        let r = Rows::new(4, &t);
        assert!(r.field_w >= 8, "field collapsed to {}", r.field_w);
        assert!(r.track_w >= 6, "track collapsed to {}", r.track_w);
    }

    /// The readout right-aligns its value; with a long label there is still at
    /// least one space between the two rather than them running together.
    #[test]
    fn a_readout_never_glues_its_label_to_its_value() {
        let t = Theme::sdr();
        let r = Rows::new(20, &t);
        let line = r.readout("a very long label indeed", "-57.3 dBFS".into(), t.label);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(
            text.ends_with("-57.3 dBFS"),
            "value not at the end: {text:?}"
        );
        let before = text.trim_end_matches("-57.3 dBFS");
        assert!(
            before.ends_with(' '),
            "label and value ran together: {text:?}"
        );
    }
}
