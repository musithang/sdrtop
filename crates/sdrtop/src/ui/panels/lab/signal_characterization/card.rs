// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The verdict card: the panel's last zone, where [`verdict`](fn@super::verdict)'s
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

use crate::signal::ballistics::HELD_AFTER;
use crate::state::{FftFrame, SdrMetrics};
use crate::ui::chrome::{reserved_row, wrap};

use super::row::dim;
use super::verdict::{verdict, VerdictLevel};

/// Row budgets for the wrapped verdict card. Sized so the longest copy `verdict`
/// can produce still lands whole at the panel's own minimum width (28 inner) -
/// `verdict_copy_fits_the_narrowest_panel` holds them to it, so a future reword
/// that overruns fails a test instead of silently losing its tail on screen.
const VERDICT_HEAD_ROWS: usize = 2;
const VERDICT_DETAIL_ROWS: usize = 4;

/// Push the card. **One height in every state**, from RX stopped to the
/// longest verdict: each part is padded to its row budget with
/// [`reserved_row`]s, so a verdict that changes from one sentence to another
/// on a burst never moves the zones above it. The sentence wraps inside its
/// budget; the budget does not follow the sentence.
pub(super) fn lines(
    out: &mut Vec<Line<'static>>,
    state: &SdrMetrics,
    frame: Option<&FftFrame>,
    iw: usize,
    theme: &crate::Theme,
) {
    let copy_w = iw.saturating_sub(1);
    let carrier = state.signal.carrier.as_ref();
    let (head, detail, held) = if frame.is_none() {
        let idle = Line::from(vec![
            Span::raw(" "),
            Span::styled("\u{25cb} IDLE \u{2014} RX stopped", dim(theme)),
        ]);
        (vec![idle], Vec::new(), None)
    } else {
        let (level, headline, detail) = verdict(carrier);
        let (mark, col) = match level {
            VerdictLevel::Clean => ("\u{2713}", theme.status_ok),
            VerdictLevel::Caution => ("\u{26a0}", theme.status_warn),
            VerdictLevel::Judging => ("\u{25cc}", theme.stale),
            VerdictLevel::NoSignal => ("\u{25cb}", theme.stale),
        };
        let head = wrap(&format!("{mark} {headline}"), copy_w, VERDICT_HEAD_ROWS)
            .into_iter()
            .map(|row| {
                Line::from(vec![
                    Span::raw(" "),
                    Span::styled(row, Style::default().fg(col).add_modifier(Modifier::BOLD)),
                ])
            })
            .collect();
        let detail = wrap(&detail, copy_w, VERDICT_DETAIL_ROWS)
            .into_iter()
            .map(|row| {
                Line::from(vec![
                    Span::raw(" "),
                    Span::styled(row, Style::default().fg(theme.label)),
                ])
            })
            .collect();
        // Through a gap the verdict is the carrier's as last heard, and says
        // how long ago that was.
        let held = carrier
            .map(|c| c.heard.elapsed())
            .filter(|a| *a > HELD_AFTER);
        (head, detail, held)
    };
    // Padded together, so a one-row headline is followed by its sentence and
    // the held rows sit under both.
    let copy: Vec<Line<'static>> = head.into_iter().chain(detail).collect();
    let n = copy.len();
    out.extend(copy);
    out.extend((n..VERDICT_HEAD_ROWS + VERDICT_DETAIL_ROWS).map(|_| reserved_row()));
    out.push(match held {
        Some(age) => Line::from(vec![
            Span::raw(" "),
            Span::styled(format!("held {:.1} s", age.as_secs_f32()), dim(theme)),
        ]),
        None => reserved_row(),
    });
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
        // the clipping this layout exists to remove.
        let copy_w = super::super::SignalCharacterizationPanel.min_size().0 as usize - 2 - 1;
        use super::super::verdict::steady;
        let mut judging = steady(Modulation::Wfm, 40.0, -40.0, -40.0, 180_000);
        judging.verdict = None;
        // A splatter verdict held while its adjacent pair is not measured.
        let mut splatter_unmeasured = steady(Modulation::Wfm, 47.0, -10.0, -40.0, 1_250_000);
        splatter_unmeasured.acpr_lower = None;
        let cases = [
            verdict(None),
            verdict(Some(&judging)),
            verdict(Some(&steady(Modulation::Wfm, 15.0, -40.0, -40.0, 180_000))),
            verdict(Some(&steady(
                Modulation::Wfm,
                47.0,
                -10.0,
                -40.0,
                1_250_000,
            ))),
            verdict(Some(&splatter_unmeasured)),
            verdict(Some(&steady(Modulation::Wfm, 43.0, -38.0, -41.0, 180_000))),
            verdict(Some(&steady(
                Modulation::Nfm,
                30.0,
                f32::NEG_INFINITY,
                f32::NEG_INFINITY,
                15_000,
            ))),
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
