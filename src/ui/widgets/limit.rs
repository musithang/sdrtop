// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The limit-and-margin row: what a standard says, what we measured, and the
//! distance between them.
//!
//! ```text
//! Mod index    0.502 ±0.006      [0.45 ·······|······· 0.55]   +0.048
//! Carr leak   -21.3  ±0.8   dB   [       max -15         ]     +6.3 dB
//! ```
//!
//! Design section 9.1 calls this idiom B. Label, the reading (idiom A, drawn by
//! [`Reading`] so a number looks the same here as anywhere else), a bar showing
//! where the value sits inside the allowed band, and the signed margin.
//!
//! **The word "pass" appears nowhere, and a test says so.** Design section 6:
//! a conformance verdict is a claim about a measurement made with a calibrated
//! instrument, a known cable and a specified test pattern. We have an
//! uncalibrated 8-bit receiver and an antenna in a room. The number and its
//! distance from the line are real and are the useful part; the verdict would be
//! the only invented thing on the screen. This is rule 5 applied to a word.
//!
//! **The margin is positive when the value is inside, by that much.** The design
//! sketch printed the two-sided row that way and the one-sided row as
//! `value - limit`, which for a ceiling flips the sign of "good". One quantity
//! gets one scale everywhere it appears (rule 5), and the convention that
//! survives both is the two-sided one, because a band has no "the limit" to
//! subtract.
//!
//! **The margin carries the reading's own uncertainty**, arrived at through
//! [`Uncertain::shift`] and [`Uncertain::scale`] rather than reassembled by
//! hand: a specification limit is exact, so the distance to it is uncertain by
//! exactly as much as the measurement is. That is what decides the colour. A
//! margin smaller than the uncertainty is amber, not green, because at that
//! distance the instrument cannot tell you which side of the line it is on, and
//! saying so is the whole point of carrying the uncertainty around.
//!
//! **A one-sided limit is drawn as one, and never as half a band.** There is no
//! lower edge to a ceiling, so there is no position along a track that would be
//! honest; inventing one to have something to draw is exactly rule 2. The
//! bracket names the limit instead, and the gutter outside it carries an arrow
//! when the value is past it.

use ratatui::{style::Style, text::Span};

use super::reading::{fmt_at, Ink, Reading};
use crate::signal::dsp::uncertainty::Uncertain;
use crate::Theme;

/// The track a marker sits on, and the marker itself.
const TRACK: char = '·';
const MARKER: char = '|';
/// Drawn in the gutter outside the bracket when the value is past that edge.
const BELOW: char = '‹';
const ABOVE: char = '›';

/// Which of the reading's uncertainties the colour is decided at.
///
/// Two sigma, about 95 %, rather than the one sigma the reading prints: the
/// question the colour answers is "can this instrument tell which side of the
/// line the value is on", and that is a coverage question, not a display one.
/// [`Uncertain::expanded`] exists for exactly this and the caller owes the
/// reader the `k`, which is what this constant and its name are for.
const COVERAGE_K: f64 = 2.0;

/// What a standard states about a value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Limit {
    /// Both sides stated: the value belongs between them.
    Band { low: f64, high: f64 },
    /// A ceiling: the value belongs at or below it.
    ///
    /// B8 gave this widget its first real consumer, and every one of its
    /// four rows is a band or a floor - no ceiling among them. Kept rather
    /// than removed: a one-sided limit that names a maximum is exactly as
    /// real a shape as one that names a minimum, and this widget's own
    /// `carrier_leak` test fixture is what still exercises it, the same
    /// design-sketch row the module doc's own worked example draws.
    #[allow(dead_code)]
    Max(f64),
    /// A floor: the value belongs at or above it.
    Min(f64),
}

impl Limit {
    /// The floor and the ceiling, either of which a one-sided limit lacks.
    fn bounds(&self) -> (Option<f64>, Option<f64>) {
        match *self {
            Limit::Band { low, high } => (Some(low), Some(high)),
            Limit::Max(high) => (None, Some(high)),
            Limit::Min(low) => (Some(low), None),
        }
    }

    /// A stated limit as it was written down.
    ///
    /// `{}` on an `f64` prints the shortest form that reads back as the same
    /// number, which for a specification limit is exactly right: 0.45 is 0.45,
    /// and printing it as 0.450 to match a measurement's three places would add
    /// a digit of precision to a number that is exact.
    fn write(x: f64) -> String {
        if x.is_finite() {
            format!("{x}")
        } else {
            "—".to_string()
        }
    }
}

/// The four column widths a row is drawn into.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RowWidths {
    label: usize,
    value: usize,
    sigma: usize,
    bar: usize,
}

impl RowWidths {
    /// Measured from a block of rows, so a column of them lines up.
    ///
    /// The value and the uncertainty are two columns rather than one because
    /// the decimal points have to line up down the block, and they only do if
    /// the value is right-aligned against a width the whole block agrees on.
    pub(crate) fn fit(rows: &[LimitRow<'_>], bar: usize) -> Self {
        let mut w = Self {
            label: 0,
            value: 0,
            sigma: 0,
            bar,
        };
        for row in rows {
            let (value, rest) = row.split();
            w.label = w.label.max(row.label.chars().count());
            w.value = w.value.max(value.0.chars().count());
            w.sigma = w.sigma.max(rest.chars().count());
        }
        w
    }
}

/// One measurement against one stated limit.
pub(crate) struct LimitRow<'a> {
    label: &'a str,
    reading: Reading<'a>,
    limit: Limit,
}

impl<'a> LimitRow<'a> {
    pub(crate) fn new(label: &'a str, reading: Reading<'a>, limit: Limit) -> Self {
        Self {
            label,
            reading,
            limit,
        }
    }

    /// The distance to the nearest stated edge, positive when inside.
    ///
    /// `None` when the reading itself would not print: a margin computed from a
    /// value the cell refused to show would be the same invented number wearing
    /// a different hat.
    fn margin(&self) -> Option<Uncertain> {
        let value = self.reading.resolved_value()?;
        let u = self.reading.uncertain();
        let (low, high) = self.limit.bounds();
        // Positive means inside, by this much, whichever edge is nearer. Both
        // distances are the measurement shifted by an exact number, which is
        // why they carry its uncertainty and nothing more.
        let to_floor = low.filter(|l| l.is_finite()).map(|l| u.shift(-l));
        let to_ceiling = high
            .filter(|h| h.is_finite())
            .map(|h| u.scale(-1.0).shift(h));
        let _ = value;
        match (to_floor, to_ceiling) {
            (Some(a), Some(b)) => Some(if a.value() <= b.value() { a } else { b }),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        }
    }

    /// Green, amber, red or absent, decided against the expanded uncertainty.
    fn severity(&self) -> Ink {
        let Some(margin) = self.margin() else {
            return Ink::Missing;
        };
        if margin.value().is_nan() {
            Ink::Missing
        } else if margin.value() < 0.0 {
            Ink::Crit
        } else if margin.value() < margin.expanded(COVERAGE_K) {
            Ink::Warn
        } else {
            Ink::Ok
        }
    }

    /// The bracketed bar, `width` columns wide including its two gutters.
    fn bar(&self, width: usize) -> Vec<(String, Ink)> {
        let content = width.saturating_sub(4);
        // Four columns buy two gutters and two brackets and leave nothing to
        // draw in. Below that the bar is blank rather than a pair of brackets
        // with nothing between them.
        if content < 3 {
            return vec![(" ".repeat(width), Ink::Dim)];
        }

        let value = self.reading.resolved_value();
        let (low, high) = self.limit.bounds();
        let past_floor = matches!((value, low), (Some(v), Some(l)) if v < l);
        let past_ceiling = matches!((value, high), (Some(v), Some(h)) if v > h);

        let mut out = vec![
            (
                if past_floor { BELOW } else { ' ' }.to_string(),
                self.severity(),
            ),
            ("[".to_string(), Ink::Dim),
        ];
        out.extend(self.face(content));
        out.push(("]".to_string(), Ink::Dim));
        out.push((
            if past_ceiling { ABOVE } else { ' ' }.to_string(),
            self.severity(),
        ));
        out
    }

    /// What goes between the brackets, exactly `content` columns of it.
    fn face(&self, content: usize) -> Vec<(String, Ink)> {
        let (low, high) = self.limit.bounds();
        let (Some(low), Some(high)) = (low, high) else {
            // One-sided: name the limit and centre it. No track, because a
            // ceiling has no far edge and any position on a bar would be
            // invented.
            let named = match self.limit {
                Limit::Max(h) => format!("max {}", Limit::write(h)),
                Limit::Min(l) => format!("min {}", Limit::write(l)),
                Limit::Band { .. } => unreachable!("a band has both bounds"),
            };
            let named: String = named.chars().take(content).collect();
            let pad = content - named.chars().count();
            return vec![(
                format!(
                    "{}{}{}",
                    " ".repeat(pad / 2),
                    named,
                    " ".repeat(pad - pad / 2)
                ),
                Ink::Dim,
            )];
        };

        // Edge labels first, and dropped whole rather than truncated when the
        // track they would leave is too short to place a marker on.
        let (lo_s, hi_s) = (Limit::write(low), Limit::write(high));
        let labelled = content
            .checked_sub(lo_s.chars().count() + hi_s.chars().count() + 2)
            .filter(|track| *track >= 3);
        let track = labelled.unwrap_or(content);

        let mut out = Vec::with_capacity(5);
        if labelled.is_some() {
            out.push((format!("{lo_s} "), Ink::Dim));
        }
        out.extend(self.track(track, low, high));
        if labelled.is_some() {
            out.push((format!(" {hi_s}"), Ink::Dim));
        }
        out
    }

    /// `track` columns of dots with the marker on one of them.
    fn track(&self, track: usize, low: f64, high: f64) -> Vec<(String, Ink)> {
        let span = high - low;
        let at = self
            .reading
            .resolved_value()
            .filter(|_| span.is_finite() && span > 0.0)
            .map(|v| {
                let t = ((v - low) / span).clamp(0.0, 1.0);
                (t * (track - 1) as f64).round() as usize
            });
        let Some(at) = at else {
            // Nothing to place: an unprintable value, or a caller's band that
            // is not one. The track is drawn empty rather than guessed at.
            return vec![(TRACK.to_string().repeat(track), Ink::Dim)];
        };
        vec![
            (TRACK.to_string().repeat(at), Ink::Dim),
            (MARKER.to_string(), self.severity()),
            (TRACK.to_string().repeat(track - 1 - at), Ink::Dim),
        ]
    }

    /// The reading split into the value and everything after it, because the
    /// two are aligned in different directions. Idiom A puts the value first
    /// and its own tests hold it there.
    fn split(&self) -> ((String, Ink), String) {
        let mut pieces = self.reading.pieces().into_iter();
        let value = pieces
            .next()
            .unwrap_or_else(|| (String::new(), Ink::Missing));
        let rest = pieces.map(|(s, _)| s).collect::<Vec<_>>().join(" ");
        (value, rest)
    }

    /// The signed distance to the nearest edge, or the house dash.
    fn margin_text(&self) -> String {
        let Some(margin) = self.margin().filter(|m| m.value().is_finite()) else {
            return "—".to_string();
        };
        let places = self.reading.uncertain().decimals().unwrap_or(3);
        let body = fmt_at(margin.value(), places);
        let signed = if body.starts_with('-') {
            body
        } else {
            format!("+{body}")
        };
        match self.reading.unit() {
            "" => signed,
            unit => format!("{signed} {unit}"),
        }
    }

    fn pieces(&self, w: RowWidths) -> Vec<(String, Ink)> {
        let ((value, ink), rest) = self.split();
        let mut out = vec![
            (format!("{:<1$}  ", self.label, w.label), Ink::Dim),
            (format!("{:>1$}", value, w.value), ink),
            (format!(" {:<1$}   ", rest, w.sigma), Ink::Dim),
        ];
        out.extend(self.bar(w.bar));
        out.push(("   ".to_string(), Ink::Dim));
        out.push((self.margin_text(), self.severity()));
        out
    }

    /// The row as plain text, for tests, for the log and for export.
    pub(crate) fn text(&self, w: RowWidths) -> String {
        self.pieces(w)
            .into_iter()
            .map(|(s, _)| s)
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    /// The row as styled spans, built from the same pieces as [`Self::text`].
    pub(crate) fn spans(&self, theme: &Theme, w: RowWidths) -> Vec<Span<'static>> {
        self.pieces(w)
            .into_iter()
            .map(|(s, ink)| Span::styled(s, Style::default().fg(ink.colour(theme))))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, text::Line, widgets::Paragraph, Terminal};

    const W: usize = 100;

    /// Render one row through a real terminal buffer and read it back, the same
    /// reason `reading.rs` does it: asserting on the string a formatter returned
    /// would not prove that anything reached a screen.
    fn draw(row: &LimitRow, w: RowWidths) -> String {
        let theme = crate::Theme::sdr();
        let mut terminal = Terminal::new(TestBackend::new(W as u16, 1)).unwrap();
        terminal
            .draw(|f| {
                f.render_widget(Paragraph::new(Line::from(row.spans(&theme, w))), f.size());
            })
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..W as u16)
            .map(|x| buf.get(x, 0).symbol().to_string())
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    /// The design's own two rows, which is what this widget was drawn from.
    fn mod_index(value: f64) -> LimitRow<'static> {
        LimitRow::new(
            "Mod index",
            Reading::new(Uncertain::from_sigma(value, 0.006), "", 0.05),
            Limit::Band {
                low: 0.45,
                high: 0.55,
            },
        )
    }

    fn carrier_leak(value: f64) -> LimitRow<'static> {
        LimitRow::new(
            "Carr leak",
            Reading::new(Uncertain::from_sigma(value, 0.8), "dB", 3.0),
            Limit::Max(-15.0),
        )
    }

    fn widths(bar: usize) -> RowWidths {
        RowWidths::fit(&[mod_index(0.502), carrier_leak(-21.3)], bar)
    }

    #[test]
    fn a_column_of_rows_shares_one_set_of_widths() {
        let w = widths(30);
        assert_eq!(
            w,
            RowWidths {
                label: 9, // "Mod index" and "Carr leak" are both nine
                value: 5, // "0.502" and "-21.3"
                sigma: 7, // "±0.006" against "±0.8 dB", which is longer
                bar: 30,
            }
        );

        // Both rows drawn at those widths put their bars in the same columns.
        // Counted in characters and not in bytes: the track is made of middle
        // dots, so `str::find` would report the two bars as misaligned by the
        // width of the dots between the brackets.
        let column = |s: &str, c: char| s.chars().position(|x| x == c);
        let a = draw(&mod_index(0.502), w);
        let b = draw(&carrier_leak(-21.3), w);
        assert_eq!(column(&a, '['), column(&b, '['), "{a}\n{b}");
        assert_eq!(column(&a, ']'), column(&b, ']'), "{a}\n{b}");
    }

    #[test]
    fn the_design_rows_render_as_the_design_drew_them() {
        let w = widths(30);
        assert_eq!(
            draw(&mod_index(0.502), w),
            "Mod index  0.502 ±0.006     [0.45 ········|······· 0.55]    +0.048"
        );
        assert_eq!(
            draw(&carrier_leak(-21.3), w),
            "Carr leak  -21.3 ±0.8 dB    [         max -15          ]    +6.3 dB"
        );
        // The text form and the drawn form are the same pieces.
        assert_eq!(draw(&mod_index(0.502), w), mod_index(0.502).text(w));
    }

    #[test]
    fn the_marker_moves_with_the_value_and_never_pretends_to_be_inside() {
        let w = widths(30);
        // Dead centre of the band, and at each edge.
        assert!(draw(&mod_index(0.5), w).contains("0.45 ·······|········ 0.55"));
        assert!(draw(&mod_index(0.45), w).contains("0.45 |··············· 0.55"));
        assert!(draw(&mod_index(0.55), w).contains("0.45 ···············| 0.55"));

        // Outside, the marker clamps to the edge - so the gutter outside the
        // bracket says which way it went, and the row cannot be misread as a
        // value sitting exactly on the limit.
        let low = draw(&mod_index(0.40), w);
        assert!(low.contains("‹[0.45 |··············· 0.55] "), "{low}");
        let high = draw(&mod_index(0.60), w);
        assert!(high.contains(" [0.45 ···············| 0.55]›"), "{high}");
    }

    #[test]
    fn the_margin_is_positive_when_the_value_is_inside() {
        let w = widths(30);
        let margin = |row: &LimitRow| row.text(w).rsplit("  ").next().unwrap().to_string();

        assert_eq!(margin(&mod_index(0.502)), "+0.048");
        assert_eq!(margin(&mod_index(0.60)), "-0.050");
        assert_eq!(margin(&carrier_leak(-21.3)), "+6.3 dB");
        assert_eq!(margin(&carrier_leak(-10.0)), "-5.0 dB");

        // A floor is the same rule read the other way up.
        let floor = LimitRow::new(
            "Mod index",
            Reading::new(Uncertain::from_sigma(0.502, 0.006), "", 0.05),
            Limit::Min(0.45),
        );
        assert_eq!(margin(&floor), "+0.052");

        // The margin carries the reading's uncertainty, because the limit is
        // exact and adds none of its own.
        assert_eq!(mod_index(0.502).margin().unwrap().sigma(), 0.006);
    }

    #[test]
    fn the_word_pass_appears_nowhere_in_any_rendering() {
        let limits = [
            Limit::Band {
                low: 0.45,
                high: 0.55,
            },
            Limit::Max(-15.0),
            Limit::Min(0.45),
        ];
        for limit in limits {
            for value in [-30.0, -15.0, 0.0, 0.45, 0.502, 0.55, 1.0, f64::NAN] {
                for bar in [0, 7, 12, 30, 60] {
                    let row = LimitRow::new(
                        "Mod index",
                        Reading::new(Uncertain::from_sigma(value, 0.006), "dB", 0.05),
                        limit,
                    );
                    let w = RowWidths::fit(&[], bar);
                    let text = row.text(w).to_ascii_lowercase();
                    for word in ["pass", "fail", "ok", "good", "bad"] {
                        assert!(!text.contains(word), "{word} in {text:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn a_one_sided_limit_does_not_pretend_to_have_a_band() {
        let w = widths(30);
        let ceiling = draw(&carrier_leak(-21.3), w);
        assert!(ceiling.contains("max -15"), "{ceiling}");
        // No track and no marker: a ceiling has no far edge, so no position
        // along a bar would be honest.
        assert!(!ceiling.contains(TRACK), "{ceiling}");
        assert!(!ceiling.contains(MARKER), "{ceiling}");

        // The arrow still works, because which side of the line is a question a
        // one-sided limit can answer.
        let over = draw(&carrier_leak(-10.0), w);
        assert!(over.contains("]›"), "{over}");

        let floor = LimitRow::new(
            "Mod index",
            Reading::new(Uncertain::from_sigma(0.40, 0.006), "", 0.05),
            Limit::Min(0.45),
        );
        let under = draw(&floor, w);
        assert!(under.contains("min 0.45"), "{under}");
        assert!(under.contains("‹["), "{under}");
    }

    #[test]
    fn the_colour_is_the_severity_at_two_sigma() {
        let theme = crate::Theme::sdr();
        // Band 0 to 10, sigma 0.5, so the expanded uncertainty is exactly 1.
        let row = |v: f64| {
            LimitRow::new(
                "Level",
                Reading::new(Uncertain::from_sigma(v, 0.5), "dB", 1.0),
                Limit::Band {
                    low: 0.0,
                    high: 10.0,
                },
            )
        };
        assert_eq!(row(5.0).severity(), Ink::Ok);
        // A margin of exactly the expanded uncertainty is still inside.
        assert_eq!(row(9.0).severity(), Ink::Ok);
        // Closer than that and the instrument cannot say which side it is on.
        assert_eq!(row(9.1).severity(), Ink::Warn);
        assert_eq!(row(11.0).severity(), Ink::Crit);

        // Every one of those is a theme colour and never a literal.
        for (v, want) in [
            (5.0, theme.status_ok),
            (9.1, theme.status_warn),
            (11.0, theme.status_crit),
        ] {
            let w = RowWidths::fit(&[row(v)], 30);
            let last = row(v)
                .spans(&theme, w)
                .into_iter()
                .rfind(|s| !s.content.trim().is_empty())
                .unwrap();
            assert_eq!(last.style.fg, Some(want), "value {v}");
        }
    }

    #[test]
    fn a_value_the_cell_will_not_print_gets_no_marker_and_no_margin() {
        let w = widths(30);
        // The uncertainty swamps the resolution, so idiom A dashes the value.
        let row = LimitRow::new(
            "Mod index",
            Reading::new(Uncertain::from_sigma(0.502, 0.5), "", 0.05),
            Limit::Band {
                low: 0.45,
                high: 0.55,
            },
        );
        let text = draw(&row, w);
        assert!(text.starts_with("Mod index  "), "{text}");
        // A bar with no marker, because there is nothing to place on it, and no
        // arrow either: which side is not known.
        assert!(text.contains("[0.45 ················ 0.55]"), "{text}");
        assert!(!text.contains(MARKER), "{text}");
        assert!(!text.contains(BELOW) && !text.contains(ABOVE), "{text}");
        assert!(text.ends_with('—'), "{text}");
        assert_eq!(row.margin(), None);
        assert_eq!(row.severity(), Ink::Missing);
    }

    #[test]
    fn nothing_reaches_the_screen_as_inf_or_nan() {
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 0.5] {
            for limit in [
                Limit::Band {
                    low: f64::NAN,
                    high: 0.55,
                },
                Limit::Band {
                    low: 0.55,
                    high: 0.45, // inverted, which is a caller's bug
                },
                Limit::Max(f64::INFINITY),
            ] {
                let row = LimitRow::new(
                    "Mod index",
                    Reading::new(Uncertain::from_sigma(value, 0.006), "", 0.05),
                    limit,
                );
                let w = RowWidths::fit(&[], 30);
                let text = draw(&row, w);
                assert!(
                    !text.contains("NaN") && !text.contains("inf"),
                    "{value} {limit:?} rendered {text:?}"
                );
            }
        }
    }

    #[test]
    fn the_bar_narrows_without_lying() {
        for bar in 0..40usize {
            let w = RowWidths::fit(&[mod_index(0.502)], bar);
            let row = mod_index(0.502);
            let pieces = row.bar(bar);
            let drawn: String = pieces.iter().map(|(s, _)| s.as_str()).collect();
            assert_eq!(drawn.chars().count(), bar, "bar width {bar}");

            // A bracket is never drawn half open, and a marker never appears
            // without a track to sit on.
            assert_eq!(
                drawn.contains('['),
                drawn.contains(']'),
                "width {bar}: {drawn:?}"
            );
            if drawn.contains(MARKER) {
                assert!(drawn.contains('['), "width {bar}: {drawn:?}");
            }
            let _ = draw(&row, w);
        }
        // Below the width where a track fits, the bar is blank rather than a
        // pair of brackets with nothing to say.
        assert!(mod_index(0.502)
            .bar(6)
            .iter()
            .all(|(s, _)| s.trim().is_empty()));
    }
}
