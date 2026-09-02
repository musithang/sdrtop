// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `ADJACENT CHANNEL` - the two adjacent-channel power ratios, each with a
//! badness bar, plus the absolute level of the louder of the two bands.

use ratatui::{
    style::Style,
    text::{Line, Span},
};

use crate::state::{FftFrame, SdrMetrics};
use crate::ui::chrome::section;

use super::row::{annotated, dash, fmt_freq, metric, val};

/// Width of the `L −200k` / `R +25k` label field. The bar starts after it, so an
/// overflow here would push one row's bar out of alignment with the other's.
const LABEL_W: usize = 8;

/// The ACPR bar's own display floor - not a regulatory spectral-mask limit (we
/// don't assert one), just how far down this gauge reads before showing a fully
/// clean, empty bar. A ratio at 0 dB (touching the carrier) reads full/red.
pub(super) const ACPR_BAR_FLOOR_DB: f32 = -80.0;

pub(super) fn lines(
    out: &mut Vec<Line<'static>>,
    state: &SdrMetrics,
    frame: Option<&FftFrame>,
    stale: bool,
    iw: usize,
    theme: &crate::Theme,
) {
    out.push(section("ADJACENT CHANNEL", "ACPR", iw, theme));
    let sig = &state.signal;
    let off = sig.acpr_offset_hz;
    let lo_label = acpr_label('L', off, '\u{2212}');
    let hi_label = acpr_label('R', off, '+');
    // The pair is written together by the FFT worker - both finite, or neither.
    // Testing both says so at the point of use rather than trusting it.
    let measured = !stale && sig.acpr_lower_db.is_finite() && sig.acpr_upper_db.is_finite();
    if !measured {
        out.push(metric(&lo_label, vec![dash(theme)], theme));
        out.push(metric(&hi_label, vec![dash(theme)], theme));
        // Same row count either way, so the stack below does not jump when the
        // measurement comes and goes.
        out.push(metric("Adj carrier", vec![dash(theme)], theme));
        return;
    }

    for (label, db) in [
        (&lo_label, sig.acpr_lower_db),
        (&hi_label, sig.acpr_upper_db),
    ] {
        let value_str = format!("{db:.1} dB");
        // lead(1) + label(8) + gap(1) + bar + gap(1) + value
        let bar_w = iw
            .saturating_sub(1 + LABEL_W + 1 + 1 + value_str.chars().count())
            .max(6);
        let mut spans = vec![Span::styled(
            format!(" {label:<LABEL_W$}"),
            Style::default().fg(theme.label),
        )];
        spans.extend(acpr_bar(db, bar_w, theme));
        spans.push(Span::styled(format!(" {value_str}"), val(theme)));
        out.push(Line::from(spans));
    }
    // The louder of the two bands is the one worth naming, and the FFT
    // worker picks it the same way: lower wins a tie.
    let adj_freq = frame.map(|fr| {
        if sig.acpr_lower_db >= sig.acpr_upper_db {
            fr.center_freq_hz.saturating_sub(off as u64)
        } else {
            fr.center_freq_hz + off as u64
        }
    });
    // A silent adjacent band has no level to name - the sentinel must not
    // reach the screen as a number.
    out.push(metric(
        "Adj carrier",
        match (sig.adj_carrier_dbfs.is_finite(), adj_freq) {
            (true, Some(hz)) => annotated(
                format!("{:.1} dBFS", sig.adj_carrier_dbfs),
                fmt_freq(hz),
                iw,
                theme,
            ),
            (true, None) => vec![Span::styled(
                format!("{:.1} dBFS", sig.adj_carrier_dbfs),
                val(theme),
            )],
            (false, _) => vec![dash(theme)],
        },
        theme,
    ));
}

/// ACPR row label - `L -200k`, `R +25k`. Derived from the offset the measurement
/// actually used, so the two can never disagree: the spacing follows the
/// modulation now, and a hardcoded label would quietly lie on every band but FM
/// broadcast.
fn acpr_label(side: char, offset_hz: f64, sign: char) -> String {
    let mag = if offset_hz >= 1_000_000.0 {
        format!("{:.1}M", offset_hz / 1e6)
    } else {
        format!("{:.0}k", offset_hz / 1e3)
    };
    format!("{side} {sign}{mag}")
}

/// Map an ACPR ratio to a ⅛-block badness bar: more fill = closer to the
/// carrier = worse (green→red, same grading the timing deadline bars use). No
/// reference tick - unlike the timing budget bar, there is no verified
/// regulatory ACPR threshold to mark, so the bar shows the measurement only.
fn acpr_bar(db: f32, bar_w: usize, theme: &crate::Theme) -> Vec<Span<'static>> {
    let clamped = db.clamp(ACPR_BAR_FLOOR_DB, 0.0);
    let badness = ((clamped - ACPR_BAR_FLOOR_DB) * 10.0).round() as u32;
    let max_badness = ((0.0 - ACPR_BAR_FLOOR_DB) * 10.0).round() as u32;
    crate::ui::widgets::charts::gain_bar_colored(
        badness,
        max_badness,
        bar_w,
        theme.status_ok,
        theme.status_crit,
        theme.border_dim,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Modulation;

    #[test]
    fn acpr_bar_width_matches_bar_w() {
        let t = crate::theme::Theme::sdr();
        let spans = acpr_bar(-38.0, 24, &t);
        let w: usize = spans.iter().map(|s| s.content.chars().count()).sum();
        assert_eq!(w, 24);
    }

    #[test]
    fn acpr_bar_touching_carrier_is_full_red() {
        let t = crate::theme::Theme::sdr();
        let spans = acpr_bar(0.0, 10, &t);
        assert_eq!(spans.last().unwrap().style.fg, Some(t.status_crit));
    }

    #[test]
    fn acpr_bar_below_floor_is_empty() {
        let t = crate::theme::Theme::sdr();
        let spans = acpr_bar(-95.0, 10, &t);
        let s: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(
            s.chars().all(|c| c == ' '),
            "below the display floor reads as clean/empty: {s:?}"
        );
    }

    #[test]
    fn acpr_labels_name_the_offset_that_was_measured() {
        // Hardcoded "-200k" was right for broadcast FM and wrong everywhere else,
        // now that the spacing follows the modulation.
        use crate::state::acpr_offset_hz;
        assert_eq!(
            acpr_label('L', acpr_offset_hz(Modulation::Wfm), '\u{2212}'),
            "L \u{2212}200k"
        );
        assert_eq!(
            acpr_label('R', acpr_offset_hz(Modulation::Nfm), '+'),
            "R +25k"
        );
        assert_eq!(
            acpr_label('R', acpr_offset_hz(Modulation::Am), '+'),
            "R +9k"
        );
        assert_eq!(acpr_label('L', 1_500_000.0, '\u{2212}'), "L \u{2212}1.5M");
    }

    #[test]
    fn acpr_labels_fit_the_column() {
        // The rows are laid out on a fixed 8-column label field; an overflow would
        // push the bar out of alignment with the row above it.
        use crate::state::acpr_offset_hz;
        for m in [
            Modulation::Wfm,
            Modulation::Nfm,
            Modulation::Am,
            Modulation::Unknown,
        ] {
            for (side, sign) in [('L', '\u{2212}'), ('R', '+')] {
                let l = acpr_label(side, acpr_offset_hz(m), sign);
                assert!(
                    l.chars().count() <= LABEL_W,
                    "{m:?} {side}: {l:?} is {} wide",
                    l.chars().count()
                );
            }
        }
    }
}
