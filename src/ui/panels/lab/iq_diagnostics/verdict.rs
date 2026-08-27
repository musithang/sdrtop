//! The plain-language verdict: which single thing to say about the front end.
//!
//! **[`decide`] draws nothing.** It imports no ratatui, no `Theme` and no width;
//! it is a function of the readings and the correction state, and returns a
//! severity plus the words. [`lines`] is what turns that into spans. The same
//! separation `signal_characterization` got in R4e, and for the same reason: the
//! interesting part of this block is *which* verdict, and that is now testable by
//! writing down six numbers.
//!
//! The thresholds are [`super::severity`]'s, not literals of its own. Before the
//! split the verdict compared against `3.0`, `5.0`, `0.02`, `1.0` and `2.0`
//! written out again — the same numbers the colours use, but agreeing only by
//! coincidence, so a re-tuned scale would have left the words saying one thing
//! and the colours another.

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::state::IqCalState;

use super::reading::Reading;
use super::rows::Rows;
use super::severity::{AMP_CRIT_DB, AMP_WARN_DB, OFFSET_CRIT, PHASE_CRIT_DEG, PHASE_WARN_DEG};

/// How loudly to say it. Maps to a theme colour in [`lines`], nowhere else.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Level {
    Crit,
    Warn,
    Ok,
}

/// A body line is normally explanatory (label colour); the all-clear's detail
/// line is affirmative (status_ok) when corrections are doing the work.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Tone {
    Plain,
    Good,
}

#[derive(Debug)]
pub(super) struct Verdict {
    pub mark: &'static str,
    pub title: &'static str,
    pub level: Level,
    pub body: Vec<(String, Tone)>,
}

/// Pick the one thing worth saying, worst first.
///
/// The readings are the **residual** after any active correction, which is what
/// makes the `cal` argument matter: a lit chip with a still-bad reading means the
/// correction is not keeping up, and the advice has to say "re-run" rather than
/// "run".
pub(super) fn decide(r: &Reading, cal: &IqCalState) -> Verdict {
    let amp = r.amp_db.abs();
    let phase = r.phase_deg.abs();
    let irr = r.irr_text(0);

    if amp > AMP_CRIT_DB || phase > PHASE_CRIT_DEG {
        return Verdict {
            mark: "\u{26a0}",
            title: "QUADRATURE IMBALANCE",
            level: Level::Crit,
            body: vec![
                (
                    format!("I/Q off balance \u{2192} image only \u{2212}{irr} dB."),
                    Tone::Plain,
                ),
                (
                    if cal.cal_applied {
                        "Auto-cal on but residual remains \u{2014} re-run [C].".into()
                    } else {
                        "Run auto-cal [C] to correct quadrature.".into()
                    },
                    Tone::Plain,
                ),
            ],
        };
    }

    if r.dc_mag > OFFSET_CRIT as f64 {
        let spike = r
            .spike_dbfs
            .map(|s| format!("{s:.1}"))
            .unwrap_or_else(|| "\u{2014}".into());
        return Verdict {
            mark: "\u{26a0}",
            title: "DC OFFSET HIGH",
            level: Level::Warn,
            body: vec![
                (
                    format!("I/Q centroid off-zero \u{2192} DC spike {spike} dBFS at LO."),
                    Tone::Plain,
                ),
                (
                    if cal.dc_block_on {
                        "DC-block on but residual offset remains.".into()
                    } else {
                        "Press [D] to block the DC spike.".into()
                    },
                    Tone::Plain,
                ),
            ],
        };
    }

    if amp > AMP_WARN_DB || phase > PHASE_WARN_DEG {
        return Verdict {
            mark: "\u{00b7}",
            title: "MINOR IMBALANCE",
            level: Level::Warn,
            body: vec![(
                "Within tolerance \u{2014} watch the image level.".into(),
                Tone::Plain,
            )],
        };
    }

    let corrected = cal.cal_applied || cal.dc_block_on;
    Verdict {
        mark: "\u{2713}",
        title: "IQ QUALITY OK",
        level: Level::Ok,
        body: vec![(
            if corrected {
                format!("Corrections active \u{00b7} image \u{2212}{irr} dB \u{00b7} DC centred.")
            } else {
                format!("Quadrature balanced \u{00b7} image \u{2212}{irr} dB \u{00b7} DC centred.")
            },
            if corrected { Tone::Good } else { Tone::Plain },
        )],
    }
}

pub(super) fn lines(v: &Verdict, rows: &Rows) -> Vec<Line<'static>> {
    let theme = rows.theme;
    let title_color = match v.level {
        Level::Crit => theme.status_crit,
        Level::Warn => theme.status_warn,
        Level::Ok => theme.status_ok,
    };
    let mut out = vec![Line::from(vec![
        Span::raw(" "),
        Span::styled(
            format!("{} {}", v.mark, v.title),
            Style::default()
                .fg(title_color)
                .add_modifier(Modifier::BOLD),
        ),
    ])];
    for (text, tone) in &v.body {
        let style = match tone {
            Tone::Plain => rows.label_style(),
            Tone::Good => Style::default().fg(theme.status_ok),
        };
        out.push(Line::from(vec![
            Span::raw(" "),
            Span::styled(text.clone(), style),
        ]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::SdrMetrics;

    fn reading(amp_db: f32, phase_deg: f32, dc: f32) -> Reading {
        let mut m = SdrMetrics::fixture();
        m.iq.iq_imbalance_db = amp_db;
        m.iq.phase_imbalance_deg = phase_deg;
        m.iq.dc_offset_i = dc;
        m.iq.dc_offset_q = 0.0;
        Reading::of(&m)
    }

    fn clean() -> Reading {
        reading(0.1, 0.2, 0.0005)
    }

    /// This module imports no `Theme`, no ratatui widget and no width, so the
    /// decision cannot accidentally depend on how wide the panel is. Asserted as
    /// source text because there is no type that says it.
    #[test]
    fn the_decision_is_drawing_free() {
        let src = include_str!("verdict.rs");
        let decide_body = &src[src.find("pub(super) fn decide").unwrap()
            ..src.find("pub(super) fn lines").unwrap()];
        for forbidden in ["theme", "Theme", "Span", "Style", "iw", "Color"] {
            assert!(
                !decide_body.contains(forbidden),
                "`decide` mentions '{forbidden}' — the decision has drawing in it"
            );
        }
    }

    /// Worst first: a front end that is both out of quadrature *and* off-centre
    /// gets told about the quadrature, because that is the one costing it dB.
    #[test]
    fn the_worst_problem_wins() {
        let v = decide(&reading(4.5, 0.2, 0.05), &IqCalState::default());
        assert_eq!(v.title, "QUADRATURE IMBALANCE");
        assert_eq!(v.level, Level::Crit);

        // Either half of the quadrature pair is enough on its own.
        assert_eq!(
            decide(&reading(0.1, 6.0, 0.0), &IqCalState::default()).title,
            "QUADRATURE IMBALANCE"
        );
    }

    #[test]
    fn each_band_has_its_own_verdict() {
        let cal = IqCalState::default();
        assert_eq!(decide(&clean(), &cal).title, "IQ QUALITY OK");
        assert_eq!(decide(&reading(1.5, 0.2, 0.0), &cal).title, "MINOR IMBALANCE");
        assert_eq!(decide(&reading(0.1, 0.2, 0.03), &cal).title, "DC OFFSET HIGH");
        assert_eq!(
            decide(&reading(4.0, 0.2, 0.0), &cal).title,
            "QUADRATURE IMBALANCE"
        );
    }

    /// The advice changes with the correction state, because the readings are the
    /// residual: telling someone to "run auto-cal" when it is already running and
    /// failing is the wrong instruction.
    #[test]
    fn the_advice_knows_whether_the_correction_is_already_on() {
        let bad = reading(4.5, 0.2, 0.0);
        let off = decide(&bad, &IqCalState::default());
        assert!(off.body[1].0.contains("Run auto-cal"), "{:?}", off.body);

        let on = decide(
            &bad,
            &IqCalState {
                cal_applied: true,
                ..Default::default()
            },
        );
        assert!(on.body[1].0.contains("re-run"), "{:?}", on.body);

        let dc = reading(0.1, 0.2, 0.03);
        assert!(decide(&dc, &IqCalState::default()).body[1]
            .0
            .contains("Press [D]"));
        assert!(decide(
            &dc,
            &IqCalState {
                dc_block_on: true,
                ..Default::default()
            }
        )
        .body[1]
            .0
            .contains("residual offset remains"));
    }

    /// A clean front end that is clean *because* of a correction says so, and
    /// says it affirmatively rather than as a neutral note.
    #[test]
    fn a_clean_reading_credits_an_active_correction() {
        let plain = decide(&clean(), &IqCalState::default());
        assert_eq!(plain.body[0].1, Tone::Plain);
        assert!(plain.body[0].0.starts_with("Quadrature balanced"));

        let corrected = decide(
            &clean(),
            &IqCalState {
                dc_block_on: true,
                ..Default::default()
            },
        );
        assert_eq!(corrected.body[0].1, Tone::Good);
        assert!(corrected.body[0].0.starts_with("Corrections active"));
    }

    /// The verdict's boundaries are the colour scale's boundaries. If they ever
    /// diverge, a reading can be amber on the meter and "OK" in words.
    #[test]
    fn the_words_change_at_the_same_point_as_the_colours() {
        let cal = IqCalState::default();
        let just_under = AMP_WARN_DB - 0.01;
        let just_over = AMP_WARN_DB + 0.01;
        assert_eq!(decide(&reading(just_under, 0.0, 0.0), &cal).level, Level::Ok);
        assert_eq!(
            decide(&reading(just_over, 0.0, 0.0), &cal).level,
            Level::Warn
        );
        assert_eq!(
            decide(&reading(AMP_CRIT_DB + 0.01, 0.0, 0.0), &cal).level,
            Level::Crit
        );
    }
}
