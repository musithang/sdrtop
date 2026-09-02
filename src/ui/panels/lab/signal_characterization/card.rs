// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The verdict card: the panel's last zone, where [`verdict`](super::verdict)'s
//! two sentences become rows.
//!
//! Prose, not readings - which is the whole reason this is a card and not another
//! metric row: it wraps rather than being clipped, because a cut sentence
//! ("Strong carrier (47 dB), 1.25") ends on what looks like a number and reads as
//! a truncated measurement.

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::state::{FftFrame, SdrMetrics};
use crate::ui::chrome::wrap;

use super::row::dim;
use super::verdict::{verdict, VerdictLevel};

/// Row budgets for the wrapped verdict card. Sized so the longest copy `verdict`
/// can produce still lands whole at the panel's own minimum width (28 inner) -
/// `verdict_copy_fits_the_narrowest_panel` holds them to it, so a future reword
/// that overruns fails a test instead of silently losing its tail on screen.
const VERDICT_HEAD_ROWS: usize = 2;
const VERDICT_DETAIL_ROWS: usize = 4;

pub(super) fn lines(
    out: &mut Vec<Line<'static>>,
    state: &SdrMetrics,
    frame: Option<&FftFrame>,
    iw: usize,
    theme: &crate::Theme,
) {
    let Some(fr) = frame else {
        out.push(Line::from(vec![
            Span::raw(" "),
            Span::styled("\u{25cb} IDLE \u{2014} RX stopped", dim(theme)),
        ]));
        return;
    };

    let sig = &state.signal;
    let (level, headline, detail) = verdict(
        sig.modulation,
        fr.peak_to_nf_db,
        sig.acpr_lower_db,
        sig.acpr_upper_db,
        fr.occupied_bw_hz,
    );
    let (mark, col) = match level {
        VerdictLevel::Clean => ("\u{2713}", theme.status_ok),
        VerdictLevel::Caution => ("\u{26a0}", theme.status_warn),
        VerdictLevel::NoSignal => ("\u{25cb}", theme.stale),
    };
    let copy_w = iw.saturating_sub(1);
    for row in wrap(&format!("{mark} {headline}"), copy_w, VERDICT_HEAD_ROWS) {
        out.push(Line::from(vec![
            Span::raw(" "),
            Span::styled(row, Style::default().fg(col).add_modifier(Modifier::BOLD)),
        ]));
    }
    for row in wrap(&detail, copy_w, VERDICT_DETAIL_ROWS) {
        out.push(Line::from(vec![
            Span::raw(" "),
            Span::styled(row, Style::default().fg(theme.label)),
        ]));
    }
    out.push(Line::raw(""));
    out.push(Line::from(vec![
        Span::raw(" "),
        Span::styled(
            "[C]",
            Style::default()
                .fg(theme.value_hi)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" snapshot to log", Style::default().fg(theme.label)),
    ]));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Modulation;
    use crate::ui::panel::Panel;

    #[test]
    fn verdict_copy_fits_the_narrowest_panel() {
        // Every verdict this panel can print, wrapped at the minimum width the
        // panel declares (30 outer → 28 inner → 27 for the copy). The row budgets
        // must hold the whole sentence: a verdict that loses its tail is exactly
        // the clipping B2 set out to remove.
        let copy_w = super::super::SignalCharacterizationPanel.min_size().0 as usize - 2 - 1;
        let cases = [
            verdict(Modulation::Unknown, 40.0, -40.0, -40.0, 0),
            verdict(Modulation::Wfm, 15.0, -40.0, -40.0, 180_000),
            verdict(Modulation::Wfm, 47.0, -10.0, -40.0, 1_250_000),
            verdict(Modulation::Wfm, 43.0, -38.0, -41.0, 180_000),
            verdict(
                Modulation::Nfm,
                30.0,
                f32::NEG_INFINITY,
                f32::NEG_INFINITY,
                15_000,
            ),
        ];
        for (_, headline, detail) in cases {
            let head = wrap(&format!("\u{26a0} {headline}"), copy_w, VERDICT_HEAD_ROWS);
            assert_eq!(
                head.join(" ")
                    .chars()
                    .filter(|c| !c.is_whitespace())
                    .count(),
                format!("\u{26a0} {headline}")
                    .chars()
                    .filter(|c| !c.is_whitespace())
                    .count(),
                "headline lost characters: {headline:?} -> {head:?}"
            );
            let body = wrap(&detail, copy_w, VERDICT_DETAIL_ROWS);
            assert_eq!(
                body.join(" ")
                    .chars()
                    .filter(|c| !c.is_whitespace())
                    .count(),
                detail.chars().filter(|c| !c.is_whitespace()).count(),
                "detail lost characters at {copy_w} wide: {detail:?} -> {body:?}"
            );
            for r in body {
                assert!(r.chars().count() <= copy_w);
            }
        }
    }
}
