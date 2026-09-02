// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Text drawn on top of the canvas: the band plan, the auto-flagged carriers,
//! the user's markers, and the `lab_signal` measurement annotations.
//!
//! Everything here has the same problem to solve: several labels want the same
//! few rows, and a label that types over its neighbour is worse than one that is
//! simply absent. Each layer therefore tracks what it has already placed and
//! either climbs to a free row or gives up.

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::state::SdrMetrics;
use crate::ui::widgets::band_plan::BAND_PLAN;

use super::scale::{canvas_x_to_col, dim, fmt_khz, marker_delta};
use super::trace::Vertical;
use super::view::SpectrumView;

/// How far a bin must rise above the noise floor to count as a real signal peak.
/// Well above typical FFT noise ripple, so only solid carriers qualify - which is
/// what keeps the auto-flagged set stable frame-to-frame (no flicker on noise).
const PEAK_PROMINENCE_DB: f32 = 10.0;

/// Detect the strongest spectral peaks for auto-marking. Returns the bin indices
/// of local maxima that rise at least `PEAK_PROMINENCE_DB` above `noise_floor`,
/// each separated from already-chosen peaks by `min_sep` bins, strongest first,
/// capped at `max_peaks`. Pure + deterministic so it can be unit-tested.
pub(crate) fn detect_peaks(
    bins: &[f32],
    noise_floor: f32,
    max_peaks: usize,
    min_sep: usize,
) -> Vec<usize> {
    if bins.len() < 3 || max_peaks == 0 {
        return Vec::new();
    }
    let thresh = noise_floor + PEAK_PROMINENCE_DB;

    // Local maxima above the threshold. A plateau registers only on its rising
    // edge (`v > left`, `v >= right`), so flat tops don't yield duplicates.
    let mut cands: Vec<usize> = (1..bins.len() - 1)
        .filter(|&i| bins[i] >= thresh && bins[i] > bins[i - 1] && bins[i] >= bins[i + 1])
        .collect();
    cands.sort_by(|&a, &b| {
        bins[b]
            .partial_cmp(&bins[a])
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut chosen: Vec<usize> = Vec::new();
    for c in cands {
        if chosen
            .iter()
            .all(|&p| (p as isize - c as isize).unsigned_abs() >= min_sep)
        {
            chosen.push(c);
            if chosen.len() >= max_peaks {
                break;
            }
        }
    }
    chosen
}

/// Amateur / broadcast band names along the top row of the canvas, one per band
/// that overlaps the view. Bands are listed left to right, and a name that would
/// start before the previous one ended is skipped rather than overprinted.
pub(super) fn band_plan(f: &mut Frame, area: Rect, view: &SpectrumView, theme: &crate::Theme) {
    if area.height < 2 || area.width <= 4 {
        return;
    }
    let cw = area.width as f64;
    let (left_hz, right_hz, bw) = (view.left_hz, view.right_hz(), view.bw);
    let mut next_free_col: i32 = -1;

    for &(band_s, band_e, label) in BAND_PLAN {
        let (bs, be) = (band_s as f64, band_e as f64);
        if bs >= right_hz || be <= left_hz {
            continue;
        }
        let center = (bs.max(left_hz) + be.min(right_hz)) / 2.0;
        let lw = label.len() as u16;
        let col = (((center - left_hz) / bw) * cw) as u16;
        let col = col.min(area.width.saturating_sub(lw));
        if (col as i32) < next_free_col {
            continue;
        }
        next_free_col = col as i32 + lw as i32 + 1;
        f.render_widget(
            Paragraph::new(Span::styled(label, Style::default().fg(theme.label))),
            Rect {
                x: area.x + col,
                y: area.y,
                width: lw,
                height: 1,
            },
        );
    }
}

/// Auto-flag the strongest carriers with a gold `▲` and their frequency.
///
/// Anchored at the peak's column and stacked upward from the *peak-hold* tip, a
/// stable y, so a flag does not bounce with the live trace. Drawn before the
/// user's markers so a deliberate `▼` wins any overlap, and with a distinct
/// glyph and colour so the two never read as the same thing.
pub(super) fn peak_flags(
    f: &mut Frame,
    area: Rect,
    view: &SpectrumView,
    vert: &Vertical,
    noise_floor: f32,
    theme: &crate::Theme,
) {
    if area.height < 3 || area.width <= 6 {
        return;
    }
    let (cw, ch) = (area.width, area.height);
    let min_sep = (view.n_bins / 24).max(1);
    // Detect on the *displayed* bins so each flag's column and frequency match
    // the visible window. Using the full frame here mislocated every flag, and
    // printed the wrong MHz beside it, whenever the view was zoomed.
    let peak_idxs = detect_peaks(&view.bins[..], noise_floor, 5, min_sep);

    // Per-row occupancy so flags never type over each other.
    let mut row_occ: Vec<Vec<(u16, u16)>> = vec![Vec::new(); ch as usize];
    for idx in peak_idxs {
        let freq = view.freq_of_bin(idx);
        let col0 = (((freq - view.left_hz) / view.bw).clamp(0.0, 1.0) * cw as f64) as u16;
        let amp = view.peaks.get(idx).copied().unwrap_or(view.bins[idx]);
        let tip_row = (vert.frac_down_to(amp) * (ch - 1) as f32) as u16;

        let num = format!("{:.2}", freq / 1_000_000.0);
        let lw = 1 + num.chars().count() as u16; // ▲ + digits
        let col = col0.min(cw.saturating_sub(lw));

        // Prefer the row just above the tip; climb until a clear slot.
        let Some(row) = (0..=tip_row.saturating_sub(1)).rev().find(|&r| {
            row_occ[r as usize]
                .iter()
                .all(|&(s, e)| col + lw <= s || col >= e)
        }) else {
            continue;
        };

        row_occ[row as usize].push((col, col + lw + 1));
        // Soft flag: the ▲ keeps the peak-hold hue to mark the carrier, the
        // frequency text is dimmed so it reads as a quiet annotation rather
        // than shouting over the trace.
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("\u{25B2}", Style::default().fg(theme.peak_hold)),
                Span::styled(num, Style::default().fg(dim(theme.peak_hold, 0.55))),
            ])),
            Rect {
                x: area.x + col,
                y: area.y + row,
                width: lw.min(cw.saturating_sub(col)),
                height: 1,
            },
        );
    }
}

/// The user's `▼` markers, with their channel-bandwidth reading when they have
/// one. Placed left to right across up to four rows, each label taking the first
/// row where it does not collide.
pub(super) fn marker_labels(
    f: &mut Frame,
    area: Rect,
    view: &SpectrumView,
    state: &SdrMetrics,
    theme: &crate::Theme,
) {
    if area.height < 3 {
        return;
    }
    let cw = area.width as f64;
    let max_rows = ((area.height / 3) as usize).clamp(1, 4);

    let mut visible: Vec<(f64, &crate::state::SpectrumMarker)> = state
        .spectrum
        .markers
        .iter()
        .filter_map(|mk| {
            let frac = (mk.freq_hz as f64 - view.left_hz) / view.bw;
            (0.0..=1.0).contains(&frac).then_some((frac, mk))
        })
        .collect();
    visible.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    // row_end[i] = the first free column on row i.
    let mut row_end: Vec<u16> = vec![0u16; max_rows];

    for (frac, mk) in visible {
        let bw_suffix = match (mk.channel_bw_hz, mk.measured_bw_hz) {
            (Some(ch), Some(meas)) => {
                let pct = meas as f64 / ch as f64 * 100.0 - 100.0;
                format!(" {}/{} {:+.0}%", fmt_khz(ch), fmt_khz(meas), pct)
            }
            (Some(ch), None) => format!(" {}?", fmt_khz(ch)),
            _ => String::new(),
        };
        let text = format!("\u{25BC}{}{}", mk.label, bw_suffix);
        let lw = text.chars().count() as u16;
        let col = ((frac * cw) as u16).min(area.width.saturating_sub(lw));

        let row = row_end
            .iter()
            .position(|&end| col >= end)
            .unwrap_or(max_rows - 1);
        row_end[row] = row_end[row].max(col + lw + 1);

        f.render_widget(
            Paragraph::new(Span::styled(
                text,
                Style::default()
                    .fg(theme.status_warn)
                    .add_modifier(Modifier::BOLD),
            )),
            Rect {
                x: area.x + col,
                y: area.y + 1 + row as u16,
                width: lw,
                height: 1,
            },
        );
    }
}

/// The `lab_signal` measurement annotations: the occupied-bandwidth label, the
/// noise-floor reading, the two-marker delta box, and the trace legend.
///
/// Specific to the `lab_signal` instrument, the only lab preset that bonds
/// spectrum and waterfall in its Body column. Every other preset that bonds them
/// (main, command_rail) is untouched.
#[allow(clippy::too_many_arguments)]
pub(super) fn signal_annotations(
    f: &mut Frame,
    area: Rect,
    view: &SpectrumView,
    state: &SdrMetrics,
    obw: (Option<f64>, Option<f64>),
    obw_hz: u64,
    vert: &Vertical,
    noise_floor: f32,
    theme: &crate::Theme,
) {
    if area.height < 3 || area.width <= 24 {
        return;
    }
    let ch = area.height;
    let obw_color = dim(theme.border_accent, 0.55);
    let nf_color = theme.noise_floor;
    let mut obw_label_cols: Option<(u16, u16)> = None;

    // OBW label - centred under its two boundary lines, bottom row.
    if let (Some(lo_x), Some(hi_x)) = obw {
        let label = format!("OBW {}", fmt_khz(obw_hz));
        let lw = label.chars().count() as u16;
        let mid_col = canvas_x_to_col((lo_x + hi_x) / 2.0, view.n(), area.width);
        let col = mid_col
            .saturating_sub(lw / 2)
            .min(area.width.saturating_sub(lw));
        obw_label_cols = Some((col, col + lw));
        f.render_widget(
            Paragraph::new(Span::styled(label, Style::default().fg(obw_color))),
            Rect {
                x: area.x + col,
                y: area.y + ch - 1,
                width: lw.min(area.width.saturating_sub(col)),
                height: 1,
            },
        );
    }

    // Noise-floor label - near the left edge, on the row the line actually sits.
    let nf_row =
        ((vert.frac_down_to(noise_floor) * (ch - 1) as f32) as u16).min(ch.saturating_sub(2));
    let nf_label = format!("noise floor {:.0} dBFS", noise_floor);
    let nf_lw = nf_label.chars().count() as u16;
    if nf_lw < area.width {
        f.render_widget(
            Paragraph::new(Span::styled(
                nf_label,
                Style::default().fg(dim(nf_color, 0.8)),
            )),
            Rect {
                x: area.x + 1,
                y: area.y + nf_row,
                width: nf_lw,
                height: 1,
            },
        );
    }

    // Δ readout - top right, MKR2 relative to MKR1, when both markers exist and
    // both fall inside the currently zoomed view.
    if let (Some(m1), Some(m2)) = (
        state.spectrum.markers.first(),
        state.spectrum.markers.get(1),
    ) {
        if let (Some(l1), Some(l2)) = (view.level_at(m1.freq_hz), view.level_at(m2.freq_hz)) {
            let (df_hz, da_db) = marker_delta(m1.freq_hz, l1, m2.freq_hz, l2);
            let line1 = format!("\u{0394} {:+.1} kHz", df_hz as f64 / 1_000.0);
            let line2 = format!("  {:+.1} dB", da_db);
            let bwid = line1.chars().count().max(line2.chars().count()) as u16;
            if bwid < area.width {
                let col = area.width - bwid;
                for (row, (text, color)) in [(line1, theme.value_hi), (line2, theme.value)]
                    .into_iter()
                    .enumerate()
                {
                    f.render_widget(
                        Paragraph::new(Span::styled(text, Style::default().fg(color))),
                        Rect {
                            x: area.x + col,
                            y: area.y + row as u16,
                            width: bwid,
                            height: 1,
                        },
                    );
                }
            }
        }
    }

    // Trace legend - bottom right, best effort: skipped entirely if it would
    // collide with the OBW label sharing the same row.
    let legend = vec![
        Span::styled("\u{2501}", Style::default().fg(theme.border_accent)),
        Span::styled(" live  ", Style::default().fg(theme.label)),
        Span::styled("\u{2501}", Style::default().fg(theme.peak_hold)),
        Span::styled(" peak-hold", Style::default().fg(theme.label)),
    ];
    let legend_w: u16 = legend
        .iter()
        .map(|s| s.content.chars().count() as u16)
        .sum();
    if legend_w < area.width {
        let col = area.width - legend_w;
        let clear = obw_label_cols
            .map(|(s, e)| col >= e || col + legend_w <= s)
            .unwrap_or(true);
        if clear {
            f.render_widget(
                Paragraph::new(Line::from(legend)),
                Rect {
                    x: area.x + col,
                    y: area.y + ch - 1,
                    width: legend_w,
                    height: 1,
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flat noise floor at -90 dBFS with two carriers poking through.
    fn noisy_spectrum() -> Vec<f32> {
        let mut b = vec![-90.0f32; 200];
        b[50] = -40.0; // strong
        b[150] = -55.0; // weaker, well separated
        b
    }

    #[test]
    fn detect_peaks_finds_carriers_above_noise() {
        let b = noisy_spectrum();
        let peaks = detect_peaks(&b, -90.0, 5, 4);
        assert_eq!(
            peaks,
            vec![50, 150],
            "strongest first, both above +10 dB prominence"
        );
    }

    #[test]
    fn detect_peaks_ignores_sub_prominence_bumps() {
        let mut b = vec![-90.0f32; 200];
        b[50] = -40.0; // real signal (+50 dB)
        b[120] = -82.0; // only +8 dB → below the 10 dB threshold
        let peaks = detect_peaks(&b, -90.0, 5, 4);
        assert_eq!(peaks, vec![50]);
    }

    #[test]
    fn detect_peaks_enforces_min_separation() {
        let mut b = vec![-90.0f32; 200];
        b[50] = -30.0; // strongest
        b[52] = -35.0; // close second, within min_sep → dropped
        b[150] = -40.0; // far enough → kept
        let peaks = detect_peaks(&b, -90.0, 5, 8);
        assert_eq!(peaks, vec![50, 150]);
    }

    #[test]
    fn detect_peaks_caps_at_max() {
        let mut b = vec![-90.0f32; 200];
        for i in 0..6 {
            b[20 + i * 25] = -40.0;
        }
        let peaks = detect_peaks(&b, -90.0, 3, 4);
        assert_eq!(peaks.len(), 3);
    }

    #[test]
    fn detect_peaks_flat_top_no_duplicate() {
        let mut b = vec![-90.0f32; 200];
        b[50] = -40.0;
        b[51] = -40.0;
        b[52] = -40.0; // 3-wide plateau
        let peaks = detect_peaks(&b, -90.0, 5, 4);
        assert_eq!(
            peaks,
            vec![50],
            "plateau yields one peak at its rising edge"
        );
    }

    #[test]
    fn detect_peaks_empty_on_pure_noise() {
        let b = vec![-90.0f32; 200];
        assert!(detect_peaks(&b, -90.0, 5, 4).is_empty());
    }
}
