// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Coordinate mapping and the frequency ruler.
//!
//! The spectrum works in three coordinate systems at once and they are easy to
//! confuse: hertz, canvas x (`0..n-1`, as wide as the FFT has bins) and terminal
//! columns (bounded by the panel). Everything that converts between them lives
//! here, alone, with tests.

use ratatui::{
    style::{Color, Style},
    text::Span,
};

/// Dim an `Rgb` color's brightness by `f` (0.0–1.0). Non-Rgb colors pass through.
pub(super) fn dim(c: Color, f: f32) -> Color {
    match c {
        Color::Rgb(r, g, b) => Color::Rgb(
            (r as f32 * f) as u8,
            (g as f32 * f) as u8,
            (b as f32 * f) as u8,
        ),
        other => other,
    }
}

/// Map a frequency to a canvas x-coordinate in `[0, n-1]`, or `None` if out of view.
pub(super) fn freq_to_canvas_x(freq_hz: f64, left_hz: f64, bw: f64, n: f64) -> Option<f64> {
    if bw <= 0.0 {
        return None;
    }
    let frac = (freq_hz - left_hz) / bw;
    if (0.0..=1.0).contains(&frac) {
        Some(frac * (n - 1.0))
    } else {
        None
    }
}

/// A canvas x - the units [`freq_to_canvas_x`] returns, spanning `0..n-1` - as a
/// terminal column inside `width`.
///
/// The two coordinate systems cost nothing to mix up silently: the canvas is as
/// wide as the spectrum has bins, typically 2048, while the column is bounded by
/// the panel, typically under 200. Using one as the other clamps every position
/// to the right-hand edge, which is exactly what the OBW label did at every
/// terminal size it was tried at.
pub(super) fn canvas_x_to_col(x: f64, n: f64, width: u16) -> u16 {
    if n <= 1.0 || width == 0 {
        return 0;
    }
    let frac = (x / (n - 1.0)).clamp(0.0, 1.0);
    ((frac * width as f64).round() as u16).min(width - 1)
}

/// The occupied-bandwidth window as a symmetric span around `center_hz` - the
/// app's own display convention ("the channel is centred at the tuned LO",
/// matching the ACPR offset math and the lab_iq carrier model), not necessarily
/// the SM.328 cumulative method's true (possibly asymmetric) cutoff bins, which
/// aren't retained past the FFT worker. A clearly-labelled approximation, good
/// for the primary lab_signal use case of a single centred carrier.
pub(super) fn obw_bounds(center_hz: u64, obw_hz: u64) -> (f64, f64) {
    let half = obw_hz as f64 / 2.0;
    (center_hz as f64 - half, center_hz as f64 + half)
}

/// Signed delta between two markers: `(Δf_hz, Δlevel_db)`, second minus first -
/// matches the mockup's "Δ +180.0 kHz  -38.1 dB" reading (MKR2 relative to MKR1).
pub(super) fn marker_delta(freq_a: u64, level_a: f32, freq_b: u64, level_b: f32) -> (i64, f32) {
    (freq_b as i64 - freq_a as i64, level_b - level_a)
}

// ── Tuning steps ─────────────────────────────────────────────────────────────

pub const SPECTRUM_STEPS: &[u64] = &[
    1_000, 5_000, 10_000, 25_000, 100_000, 500_000, 1_000_000, 5_000_000, 10_000_000,
];

pub fn prev_spectrum_step(current: u64) -> u64 {
    match SPECTRUM_STEPS.iter().position(|&s| s == current) {
        Some(idx) => SPECTRUM_STEPS[idx.saturating_sub(1)],
        // Not in list: find the largest step strictly below current
        None => SPECTRUM_STEPS
            .iter()
            .copied()
            .rfind(|&s| s < current)
            .unwrap_or(SPECTRUM_STEPS[0]),
    }
}

pub fn next_spectrum_step(current: u64) -> u64 {
    match SPECTRUM_STEPS.iter().position(|&s| s == current) {
        Some(idx) => SPECTRUM_STEPS[(idx + 1).min(SPECTRUM_STEPS.len() - 1)],
        // Not in list: find the smallest step strictly above current
        None => SPECTRUM_STEPS
            .iter()
            .copied()
            .find(|&s| s > current)
            .unwrap_or(*SPECTRUM_STEPS.last().unwrap()),
    }
}

/// Compact bandwidth, for marker suffixes and the OBW label: `25k`, `1.5M`.
pub(super) fn fmt_khz(hz: u64) -> String {
    if hz >= 1_000_000 {
        format!("{:.1}M", hz as f64 / 1_000_000.0)
    } else {
        format!("{}k", hz / 1_000)
    }
}

/// The tuning step as the footer and indicator spell it: `25 kHz`, `1 MHz`.
pub fn fmt_spectrum_step(hz: u64) -> String {
    if hz >= 1_000_000 {
        format!("{} MHz", hz / 1_000_000)
    } else {
        format!("{} kHz", hz / 1_000)
    }
}

/// Build the frequency-scale spans for an axis/ruler `width` columns wide: a `┬`
/// tick + MHz label at each quarter, the inter-tick gaps filled with `fill`.
/// Reused by the spectrum's own axis (fill `' '`) and the bonded shared ruler on
/// the waterfall's top border (fill `'─'`, so it reads as a continuous rule).
pub fn freq_scale_spans(
    left_hz: f64,
    bw: f64,
    width: usize,
    tick_color: Color,
    label_color: Color,
    fill: char,
) -> Vec<Span<'static>> {
    let labels: Vec<String> = (0..=4)
        .map(|i| format!("{:.2}M", (left_hz + bw * i as f64 / 4.0) / 1_000_000.0))
        .collect();
    let lw = labels.iter().map(|s| s.len()).max().unwrap_or(7);
    let seg = width.saturating_sub(lw) / 4;
    let mut spans: Vec<Span> = Vec::with_capacity(12);
    for (i, lab) in labels.iter().enumerate() {
        spans.push(Span::styled("\u{252C}", Style::default().fg(tick_color))); // ┬
        if i < 4 {
            let pad = seg.saturating_sub(1).saturating_sub(lab.len());
            spans.push(Span::styled(lab.clone(), Style::default().fg(label_color)));
            spans.push(Span::styled(
                fill.to_string().repeat(pad),
                Style::default().fg(tick_color),
            ));
        } else {
            spans.push(Span::styled(lab.clone(), Style::default().fg(label_color)));
        }
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canvas_x_and_column_are_not_interchangeable() {
        // The bug this guards: a 2048-bin canvas x used directly as a column.
        // Three quarters across a 2048-bin canvas is column 120 of 160, not 160.
        let n = 2048.0;
        assert_eq!(canvas_x_to_col(0.0, n, 160), 0);
        assert_eq!(
            canvas_x_to_col(n - 1.0, n, 160),
            159,
            "the last bin is the last column"
        );
        assert_eq!(canvas_x_to_col((n - 1.0) * 0.75, n, 160), 120);
        // Degenerate geometry answers 0 rather than dividing by zero.
        assert_eq!(canvas_x_to_col(500.0, 1.0, 160), 0);
        assert_eq!(canvas_x_to_col(500.0, n, 0), 0);
    }

    #[test]
    fn freq_to_canvas_x_rejects_what_is_off_screen() {
        let (left, bw, n) = (92_000_000.0, 2_000_000.0, 1001.0);
        assert_eq!(freq_to_canvas_x(92_000_000.0, left, bw, n), Some(0.0));
        assert_eq!(freq_to_canvas_x(94_000_000.0, left, bw, n), Some(1000.0));
        assert_eq!(freq_to_canvas_x(93_000_000.0, left, bw, n), Some(500.0));
        assert!(
            freq_to_canvas_x(91_999_999.0, left, bw, n).is_none(),
            "below the window"
        );
        assert!(
            freq_to_canvas_x(94_000_001.0, left, bw, n).is_none(),
            "above the window"
        );
        assert!(
            freq_to_canvas_x(93_000_000.0, left, 0.0, n).is_none(),
            "no span, no position"
        );
    }

    #[test]
    fn steps_walk_the_ladder_and_stop_at_its_ends() {
        assert_eq!(next_spectrum_step(1_000), 5_000);
        assert_eq!(prev_spectrum_step(5_000), 1_000);
        assert_eq!(prev_spectrum_step(1_000), 1_000, "already at the bottom");
        assert_eq!(
            next_spectrum_step(10_000_000),
            10_000_000,
            "already at the top"
        );
        // A value that is not on the ladder snaps to the neighbour in that direction.
        assert_eq!(next_spectrum_step(7_500), 10_000);
        assert_eq!(prev_spectrum_step(7_500), 5_000);
    }

    #[test]
    fn marker_delta_is_second_minus_first() {
        let (df, da) = marker_delta(92_800_000, -38.0, 92_980_000, -76.1);
        assert_eq!(df, 180_000);
        assert!((da - -38.1).abs() < 1e-4);
        // And it is signed: swapping the markers flips both.
        let (df, da) = marker_delta(92_980_000, -76.1, 92_800_000, -38.0);
        assert_eq!(df, -180_000);
        assert!((da - 38.1).abs() < 1e-4);
    }

    #[test]
    fn obw_bounds_straddle_the_centre() {
        let (lo, hi) = obw_bounds(92_800_000, 180_000);
        assert_eq!((lo, hi), (92_710_000.0, 92_890_000.0));
    }

    #[test]
    fn the_frequency_ruler_spans_the_width_it_was_given() {
        let spans = freq_scale_spans(
            92_000_000.0,
            2_000_000.0,
            60,
            Color::Reset,
            Color::Reset,
            '─',
        );
        let w: usize = spans.iter().map(|s| s.content.chars().count()).sum();
        assert!(w <= 60, "the ruler never overruns its row (got {w})");
        // Five ticks, first and last labelled with the window edges.
        assert_eq!(spans.iter().filter(|s| s.content == "\u{252C}").count(), 5);
        assert_eq!(spans[1].content, "92.00M");
        assert_eq!(spans.last().unwrap().content, "94.00M");
    }
}
