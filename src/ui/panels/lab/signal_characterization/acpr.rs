// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `ADJACENT CHANNEL` - the two adjacent-channel power ratios, each with a
//! badness bar, plus the absolute level of the louder of the two bands.

use ratatui::{
    style::Style,
    text::{Line, Span},
};

use crate::state::{Carrier, FftFrame, SdrMetrics, ACPR_FLOOR_DB};
use crate::ui::chrome::section;

use super::row::{annotated, dash, dim, fmt_freq, metric, val};

/// Width of the `L −200k` / `R +25k` label field. The bar starts after it, so an
/// overflow here would push one row's bar out of alignment with the other's.
const LABEL_W: usize = 8;

/// Width of the digits column, `-80.0 dB` at the widest the bar can show: the
/// digits and the dash that stands in for them end in one column, so the bar
/// does not change length when the digits come and go.
const VALUE_W: usize = 8;

pub(super) fn lines(
    out: &mut Vec<Line<'static>>,
    state: &SdrMetrics,
    frame: Option<&FftFrame>,
    carrier: Option<&Carrier>,
    iw: usize,
    theme: &crate::Theme,
) {
    out.push(section("ADJACENT CHANNEL", "ACPR", iw, theme));
    // The offset the needles were measured at, kept through gaps: the frame
    // in hand's follows its own class, which a gap does not have.
    let off = state
        .signal
        .acpr_measured_offset_hz
        .unwrap_or(state.signal.acpr_offset_hz);
    let lo_label = acpr_label('L', off, '\u{2212}');
    let hi_label = acpr_label('R', off, '+');
    // The carrier's ratios, not the frame in hand's: in a gap between bursts
    // the adjacent bands are noise measured against noise, about 0 dB, and
    // the bars would go red at every gap. Both sides or neither, because the
    // pair is read together.
    let pair = carrier.and_then(|c| c.acpr_lower.zip(c.acpr_upper).map(|p| (c, p)));
    // The bars are the needles: the carrier's ratios on the averaging meter
    // while it is heard and held, then falling back to rest in the stale
    // colour with their digits refused, so a bar is released rather than
    // vanishing. The row has one shape whatever it shows: label, bar (an
    // empty track at rest), digits or a dash in one right-aligned column.
    let needles = state
        .signal
        .acpr_needle
        .map(|n| n.filter(|_| frame.is_some()));
    let bar_w = iw.saturating_sub(1 + LABEL_W + 1 + 1 + VALUE_W).max(6);
    for (i, label) in [&lo_label, &hi_label].into_iter().enumerate() {
        let digits = pair.map(|(_, (lo, hi))| if i == 0 { lo } else { hi }.readout);
        let mut spans = vec![Span::styled(
            format!(" {label:<LABEL_W$}"),
            Style::default().fg(theme.label),
        )];
        spans.extend(acpr_bar(
            needles[i].unwrap_or(ACPR_FLOOR_DB),
            bar_w,
            digits.is_some(),
            theme,
        ));
        spans.push(match digits {
            Some(db) => Span::styled(format!(" {:>VALUE_W$}", format!("{db:.1} dB")), val(theme)),
            None => Span::styled(format!(" {:>VALUE_W$}", "---"), dim(theme)),
        });
        out.push(Line::from(spans));
    }
    let Some((c, (lo, hi))) = pair else {
        out.push(metric("Adj carrier", vec![dash(theme)], theme));
        return;
    };
    // The louder of the two bands is the one worth naming, and the FFT
    // worker picks it the same way: lower wins a tie.
    let adj_freq = frame.map(|fr| {
        if lo.readout >= hi.readout {
            fr.center_freq_hz.saturating_sub(off as u64)
        } else {
            fr.center_freq_hz + off as u64
        }
    });
    // A silent adjacent band has no level to name - the sentinel must not
    // reach the screen as a number.
    let adj = c.adj_carrier_dbfs;
    out.push(metric(
        "Adj carrier",
        match (adj.is_finite(), adj_freq) {
            (true, Some(hz)) => annotated(format!("{adj:.1} dBFS"), fmt_freq(hz), iw, theme),
            (true, None) => vec![Span::styled(format!("{adj:.1} dBFS"), val(theme))],
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
/// A needle that is not `live` (falling back after the carrier has gone) is
/// drawn in the stale colour: a needle returning to rest, not a reading.
fn acpr_bar(db: f32, bar_w: usize, live: bool, theme: &crate::Theme) -> Vec<Span<'static>> {
    let clamped = db.clamp(ACPR_FLOOR_DB, 0.0);
    let badness = ((clamped - ACPR_FLOOR_DB) * 10.0).round() as u32;
    let max_badness = ((0.0 - ACPR_FLOOR_DB) * 10.0).round() as u32;
    let (lo, hi) = if live {
        (theme.status_ok, theme.status_crit)
    } else {
        (theme.stale, theme.stale)
    };
    crate::ui::widgets::charts::gain_bar_colored(
        badness,
        max_badness,
        bar_w,
        lo,
        hi,
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
        let spans = acpr_bar(-38.0, 24, true, &t);
        let w: usize = spans.iter().map(|s| s.content.chars().count()).sum();
        assert_eq!(w, 24);
    }

    #[test]
    fn acpr_bar_touching_carrier_is_full_red() {
        let t = crate::theme::Theme::sdr();
        let spans = acpr_bar(0.0, 10, true, &t);
        assert_eq!(spans.last().unwrap().style.fg, Some(t.status_crit));
    }

    #[test]
    fn a_released_needle_is_drawn_as_stale() {
        let t = crate::theme::Theme::sdr();
        let spans = acpr_bar(-30.0, 10, false, &t);
        assert!(spans
            .iter()
            .filter(|s| !s.content.trim().is_empty())
            .all(|s| s.style.fg == Some(t.stale)));
    }

    #[test]
    fn acpr_bar_below_floor_is_empty() {
        let t = crate::theme::Theme::sdr();
        let spans = acpr_bar(-95.0, 10, true, &t);
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
