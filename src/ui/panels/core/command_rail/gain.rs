// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The GAIN section of the rail: front-end boost, the per-stage bars, and the
//! TOTAL readout with its clip headroom.

use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};

use crate::hardware::StageReading;
use crate::state::SdrMetrics;
use crate::ui::panels::core::header::gain_bar;
use crate::ui::widgets::charts::gain_bar_colored;

use super::row::{label_cell, stage_label_cell};

/// Width of the gain bar given the rail's inner width - leaves room for the
/// `LNA ` label, a space, and a 2-col value. Clamped so it neither vanishes on a
/// narrow rail nor sprawls on a wide one.
fn gain_bar_width(inner_w: usize) -> usize {
    inner_w.saturating_sub(10).clamp(4, 12)
}

/// The GAIN section rows: front-end boost, one bar per stage, then TOTAL with
/// its clip headroom. `active` is "streaming and we own the radio" - idle, every
/// value drops to the label colour and the bars go flat.
pub(super) fn lines(
    state: &SdrMetrics,
    active: bool,
    observer: bool,
    iw: usize,
    theme: &crate::Theme,
) -> Vec<Line<'static>> {
    let gm = &state.caps.gain;
    let bar_w = gain_bar_width(iw);
    let val_col = if active { theme.value } else { theme.label };
    let mut out: Vec<Line<'static>> = Vec::new();

    // Front-end boost (HackRF RF amp / RTL-SDR tuner AGC - one flag, two names).
    //
    // A device that has neither gets no row at all rather than a row reading
    // OFF, which would be a stage the reader could go looking for a key to turn
    // on. The rail is short on space; a line that means nothing is worse than no
    // line.
    if gm.has_boost() {
        let (boost_val, boost_col) = if observer {
            ("\u{2014}".to_string(), theme.label)
        } else if state.radio.amp_enabled {
            ("ON".to_string(), theme.value_hi)
        } else {
            ("OFF".to_string(), theme.label)
        };
        out.push(Line::from(vec![
            Span::raw(" "),
            label_cell(gm.boost_label(), theme),
            Span::styled(boost_val, Style::default().fg(boost_col)),
        ]));
        out.push(Line::raw(""));
    }

    // One row per stage the device actually has, in the driver's own order.
    //
    // **Not a fixed primary/secondary pair.** That shape was the HackRF's, and
    // it left a SoapySDR device showing a single combined bar while the focus
    // mode could point at stages that had no row on screen. Showing where the
    // gain went is also the whole argument of the per-stage gain: the driver's
    // automatic split was the thing nobody could see.
    // The vendor curve the knob walks, above the stages it sets: its bar is
    // the step along the curve, and the stages below keep their own rows.
    if let Some(curve) = state.curve_reading() {
        out.push(gain_row(
            label_cell(curve.short, theme),
            curve.step as u32,
            curve.last.max(1) as u32,
            curve.step_text(),
            theme.status_ok,
            theme.value_hi,
            bar_w,
            active,
            val_col,
            theme,
        ));
        out.push(Line::raw(""));
    }

    let selected = state.ui.gain_stage;
    let stages = gm.stages();
    for (index, spec) in stages.iter().enumerate() {
        // The front stage is the one to notice, so it keeps the warmer ramp.
        let (from, to) = if index == 0 {
            (theme.status_ok, theme.value_hi)
        } else {
            (theme.border_accent, theme.status_warn)
        };
        // A gain in dB where it was measured, its position where it was not.
        let (value, max, text) = match state.stage_reading(index) {
            Some(StageReading::Position { at, of }) => {
                (at, of.max(1), StageReading::Position { at, of }.compact())
            }
            _ => {
                let db = state.radio.stage_gain(index).max(0.0).round() as u32;
                (db, spec.max_db.max(1.0).round() as u32, format!("{db:2}"))
            }
        };
        out.push(gain_row(
            stage_label_cell(&spec.name, selected == Some(index), theme),
            value,
            max,
            text,
            from,
            to,
            bar_w,
            active,
            val_col,
            theme,
        ));
        out.push(Line::raw(""));
    }

    // A total of gains is one only where every stage is a gain here: a
    // position added to decibels is not a number of anything.
    let total = if state.chain_db_holds() {
        format!("{} dB", state.radio.total_gain().max(0.0).round() as u32)
    } else {
        "\u{2014}".to_string()
    };
    let mut total_spans = vec![
        Span::raw(" "),
        label_cell("TOTAL", theme),
        Span::styled(total, Style::default().fg(val_col)),
    ];
    if active {
        let headroom = (-state.signal.adc_peak_dbfs).max(0.0);
        total_spans.push(Span::styled(
            "  \u{00b7}  ".to_string(),
            Style::default().fg(theme.border_dim),
        ));
        total_spans.push(Span::styled(
            format!("{headroom:.0} dB headroom"),
            Style::default().fg(theme.label),
        ));
    }
    out.push(Line::from(total_spans));
    out.push(Line::raw(""));
    out
}

/// One gain row: ` LABEL [⅛-block bar] value`.
///
/// Streaming, the bar shades along a meaning gradient (`lo` → `hi`); idle it is a
/// flat dim ⅛-block. The header draws its own flat bar through a separate path,
/// so the two never have to agree on colour.
#[allow(clippy::too_many_arguments)]
fn gain_row(
    label: Span<'static>,
    gain: u32,
    max: u32,
    text: String,
    lo: Color,
    hi: Color,
    bar_w: usize,
    active: bool,
    val_col: Color,
    theme: &crate::Theme,
) -> Line<'static> {
    let bar: Vec<Span<'static>> = if active {
        gain_bar_colored(gain, max, bar_w, lo, hi, theme.border_dim)
    } else {
        let (filled, empty) = gain_bar(gain, max, bar_w);
        vec![
            Span::styled(filled, Style::default().fg(theme.label)),
            Span::styled(empty, Style::default().fg(theme.border_dim)),
        ]
    };
    let mut spans = vec![Span::raw(" "), label];
    spans.extend(bar);
    spans.push(Span::raw(" "));
    spans.push(Span::styled(text, Style::default().fg(val_col)));
    Line::from(spans)
}

#[cfg(test)]
mod tests {

    /// **The rail adds the chain up only where every stage is a gain.** Tuned
    /// outside the band the R2's LNA was measured in, its row gives its
    /// position and the total is a dash, not 75 dB; inside it, both are dB.
    #[test]
    fn the_rail_totals_the_chain_only_where_every_stage_was_measured() {
        let mut m = crate::state::SdrMetrics::fixture()
            .streaming()
            .band_measured_chain();
        let far = crate::state::fixture::draw(crate::ui::CommandRailPanel, 34, 40, &m).join("\n");
        assert!(far.contains("14/14"), "{far}");
        assert!(!far.contains("75 dB"), "{far}");
        assert!(
            far.contains("TOTAL \u{2014}") || far.contains("TOTAL  \u{2014}"),
            "{far}"
        );

        m.radio.frequency = 92_800_000;
        let near = crate::state::fixture::draw(crate::ui::CommandRailPanel, 34, 40, &m).join("\n");
        assert!(near.contains("75 dB"), "{near}");
        assert!(!near.contains("14/14"), "{near}");
    }

    /// **On a curve the rail leads with it**: a row for the curve and its
    /// step above the stages, which keep their own rows.
    #[test]
    fn the_rail_leads_with_the_curve_the_knob_walks() {
        let mut m = crate::state::SdrMetrics::fixture()
            .streaming()
            .band_measured_chain()
            .on_curve(0, 10);
        m.radio.frequency = 92_800_000;
        let out = crate::state::fixture::draw(crate::ui::CommandRailPanel, 34, 40, &m);
        let curve = out
            .iter()
            .position(|l| l.contains("LIN") && l.contains("10/21"));
        let lna = out.iter().position(|l| l.contains("LNA"));
        assert!(curve.is_some() && curve < lna, "{}", out.join("\n"));
        assert!(out.join("\n").contains("51 dB"), "{}", out.join("\n"));
    }

    /// The rail drops the boost row entirely on a device that has none, rather
    /// than printing a stage the reader could go hunting for a key to turn on.
    /// The rail is the most space-constrained panel in the app; a line that
    /// means nothing costs a line that would.
    #[test]
    fn the_rail_omits_a_boost_the_device_does_not_have() {
        let with = crate::state::fixture::draw(
            crate::ui::CommandRailPanel,
            34,
            40,
            &crate::state::SdrMetrics::fixture().streaming(),
        )
        .join("\n");
        assert!(with.contains("AMP"), "{with}");

        let without = crate::state::fixture::draw(
            crate::ui::CommandRailPanel,
            34,
            40,
            &crate::state::SdrMetrics::fixture()
                .streaming()
                .named_chain_no_boost(),
        )
        .join("\n");
        assert!(!without.contains("AMP"), "{without}");
        assert!(!without.contains("AGC"), "{without}");
        // The stages that do exist are still there.
        assert!(without.contains("GAIN"), "{without}");
    }
    use super::*;

    /// The TOTAL row is the sum of the stages, whatever there are of them.
    ///
    /// It used to be `primary + secondary if two stages else primary`, which is
    /// the HackRF's shape again: on a SoapySDR device with three elements it
    /// would have reported the front one alone as the whole chain.
    #[test]
    fn the_total_is_the_sum_of_however_many_stages_there_are() {
        let mut m = crate::state::SdrMetrics::fixture().streaming();
        assert_eq!(m.radio.total_gain(), 54.0, "a HackRF's LNA 24 plus VGA 30");

        m.radio.gains = vec![10.0, 20.0, 30.0];
        assert_eq!(m.radio.total_gain(), 60.0, "three stages all count");

        m.radio.gains.clear();
        assert_eq!(m.radio.total_gain(), 0.0);
    }

    #[test]
    fn gain_bar_width_clamps() {
        assert_eq!(gain_bar_width(10), 4); // tiny rail → floor
        assert_eq!(gain_bar_width(0), 4);
        assert_eq!(gain_bar_width(22), 12); // wide rail → ceiling
        assert_eq!(gain_bar_width(18), 8); // mid → 18-10
    }
}
