// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The RECALL section: three tuning-memory slots, each a pip showing whether it
//! is empty, holding a frequency, or the one currently tuned.

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::state::{active_recall_slot, SdrMetrics, RECALL_SLOTS};
use crate::ui::panels::core::header::vfo_string;
use crate::ui::widgets::band_plan::band_at;

use super::modes::rail_peaks;

/// Whether a recall slot's frequency has a detectable signal in the current spectrum.
/// Returns `Some((pip_str, strong))` when in-band, `None` when out-of-band or stale.
fn recall_pip(slot_hz: u64, state: &SdrMetrics, stale: bool) -> Option<(&'static str, bool)> {
    if stale {
        return None;
    }
    let fr = state.waterfall.last_fft.as_ref()?;
    let half_sr = (fr.sample_rate / 2.0) as u64;
    let center = state.radio.frequency;
    if slot_hz < center.saturating_sub(half_sr) || slot_hz > center + half_sr {
        return None;
    }
    let peaks = rail_peaks(&fr.bins_dbfs, fr.noise_floor, center, fr.sample_rate, 8);
    let close = peaks.iter().any(|&(f, _)| f.abs_diff(slot_hz) < 250_000);
    let strong = peaks
        .iter()
        .filter(|&&(f, _)| f.abs_diff(slot_hz) < 250_000)
        .any(|&(_, db)| db > fr.noise_floor + 20.0);
    Some(if strong {
        ("⣿⡇", true)
    } else if close {
        ("⠉⠁", false)
    } else {
        ("·", false)
    })
}

/// The RECALL list: the three saved-frequency slots, the one the radio is parked
/// on lit with `▸`. Empty slots show a dim dash. `M` saves the current tuning,
/// `1·2·3` jump (both in rail-focus). Band tags come from `band_at`.
/// Activity pips appear on the right when a slot frequency is visible in the spectrum.
pub(super) fn lines(state: &SdrMetrics, stale: bool, theme: &crate::Theme) -> Vec<Line<'static>> {
    let active = active_recall_slot(&state.ui.recall, state.radio.frequency);
    let mut out: Vec<Line<'static>> = (0..RECALL_SLOTS)
        .map(|i| {
            let n = i + 1;
            match state.ui.recall[i] {
                Some(hz) => {
                    let on = active == Some(i);
                    let mark = if on { "▸" } else { " " };
                    let modi = if on {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    };
                    let mut spans = vec![
                        Span::styled(mark.to_string(), Style::default().fg(theme.value_hi)),
                        Span::styled(
                            format!("{n} "),
                            Style::default().fg(if on { theme.value_hi } else { theme.label }),
                        ),
                        Span::styled(
                            vfo_string(hz),
                            Style::default()
                                .fg(if on { theme.value_hi } else { theme.value })
                                .add_modifier(modi),
                        ),
                    ];
                    if let Some(b) = band_at(hz) {
                        spans.push(Span::styled(
                            format!("  {b}"),
                            Style::default().fg(theme.border_accent),
                        ));
                    }
                    if let Some((pip, strong)) = recall_pip(hz, state, stale) {
                        let col = if strong {
                            theme.value_hi
                        } else {
                            theme.border_dim
                        };
                        spans.push(Span::styled(format!(" {pip}"), Style::default().fg(col)));
                    }
                    Line::from(spans)
                }
                None => Line::from(vec![
                    Span::raw(" "),
                    Span::styled(format!("{n} "), Style::default().fg(theme.border_dim)),
                    Span::styled("—", Style::default().fg(theme.stale)),
                ]),
            }
        })
        .collect();
    // The section's own trailing spacer, so the stack's droppable-blank budget
    // is the same whichever section is being edited.
    out.push(Line::raw(""));
    out
}
