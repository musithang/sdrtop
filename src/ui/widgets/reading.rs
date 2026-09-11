// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The value-with-uncertainty cell: one appearance for every measurement in the
//! app.
//!
//! ```text
//! CFO   -3.2 ±0.4 ppm
//! ```
//!
//! Value in the reading colour, uncertainty and unit dimmed. Design section 9.1
//! calls this idiom A, and building it once is what will keep eleven panels from
//! becoming eleven arguments about how to print a number.
//!
//! **Two rules, and they are the whole widget.**
//!
//! *The digits follow the uncertainty.* `-3.2183 ±0.4` is a lie told by
//! formatting: four of those digits are noise wearing the costume of precision.
//! The number of places comes from [`Uncertain::decimals`] and the value and the
//! uncertainty are printed to the same one, always.
//!
//! *When the value cannot be supported, it dashes and the uncertainty stays.*
//! The instrument saying "I cannot tell you this yet, and here is how badly" is
//! a better answer than a number, and it is rule 2 rendered as a widget. What
//! counts as "cannot be supported" is not this widget's judgement to make: it
//! takes a `resolution`, the smallest difference that matters for this
//! particular reading, because a carrier offset of exactly zero is an excellent
//! measurement and any rule based on value over uncertainty would dash it. See
//! `Uncertain::is_resolved`.

use ratatui::{
    style::{Color, Style},
    text::Span,
};

use crate::signal::dsp::uncertainty::Uncertain;
use crate::Theme;

/// Which ink a piece of a reading is drawn in.
///
/// One table for the whole widget family, so `limit` cannot invent a seventh
/// colour for a number that is already being drawn here in one of six. Every one
/// of them comes from the theme; see
/// `the_three_inks_are_the_themes_and_never_a_literal`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Ink {
    /// The reading itself.
    Value,
    /// A value that could not be supported.
    Missing,
    /// Uncertainty, unit, label, and the track a marker sits on.
    Dim,
    /// Inside the limit by more than the measurement's own uncertainty.
    Ok,
    /// Inside, but by less than that: too close to call.
    Warn,
    /// Outside.
    Crit,
}

impl Ink {
    pub(super) fn colour(self, theme: &Theme) -> Color {
        match self {
            Ink::Value => theme.value,
            Ink::Missing => theme.stale,
            Ink::Dim => theme.label,
            Ink::Ok => theme.status_ok,
            Ink::Warn => theme.status_warn,
            Ink::Crit => theme.status_crit,
        }
    }
}

/// A measurement, ready to be drawn or written out.
pub(crate) struct Reading<'a> {
    value: Uncertain,
    unit: &'a str,
    resolution: f64,
}

impl<'a> Reading<'a> {
    /// `resolution` is the smallest difference that matters for this reading: a
    /// specification tolerance, a channel spacing, a ppm budget. An uncertainty
    /// larger than it dashes the value. A caller with no such number passes
    /// `f64::INFINITY` and always sees the value, and should be able to say why.
    pub(crate) fn new(value: Uncertain, unit: &'a str, resolution: f64) -> Self {
        Self {
            value,
            unit,
            resolution,
        }
    }

    /// The measurement behind the cell, for a caller that needs to compute with
    /// it rather than print it.
    pub(super) fn uncertain(&self) -> Uncertain {
        self.value
    }

    pub(super) fn unit(&self) -> &str {
        self.unit
    }

    /// The value this cell will actually print, or `None` when it dashes.
    ///
    /// The one condition, asked once. A row that drew a marker or a margin for a
    /// value the cell refused to print would be contradicting itself on the same
    /// line.
    pub(super) fn resolved_value(&self) -> Option<f64> {
        let value = self.value.value();
        (value.is_finite() && self.value.is_resolved(self.resolution)).then_some(value)
    }

    pub(super) fn pieces(&self) -> Vec<(String, Ink)> {
        let sigma = self.value.sigma();
        let places = self.value.decimals();
        let mut out = Vec::with_capacity(3);

        // A value that is not a number, or one the uncertainty cannot support,
        // is not printed. The house dash says so in every other panel too.
        if let Some(value) = self.resolved_value() {
            out.push((fmt_at(value, places.unwrap_or(3)), Ink::Value));
        } else {
            out.push(("—".to_string(), Ink::Missing));
        }

        // An exact value has no uncertainty to show, and an unknown one has none
        // that can be written down: "±inf" is not a thing a person can read.
        if let Some(places) = places {
            out.push((format!("±{}", fmt_at(sigma, places)), Ink::Dim));
        }

        if !self.unit.is_empty() {
            out.push((self.unit.to_string(), Ink::Dim));
        }
        out
    }

    /// The cell as plain text, for tests, for the log and for export.
    pub(crate) fn text(&self) -> String {
        self.pieces()
            .into_iter()
            .map(|(s, _)| s)
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The cell as styled spans. Built from the same pieces as [`Self::text`],
    /// so the two cannot drift apart.
    pub(crate) fn spans(&self, theme: &Theme) -> Vec<Span<'static>> {
        self.pieces()
            .into_iter()
            .enumerate()
            .flat_map(|(i, (s, ink))| {
                let colour = ink.colour(theme);
                let lead = if i == 0 { "" } else { " " };
                [
                    Span::raw(lead.to_string()),
                    Span::styled(s, Style::default().fg(colour)),
                ]
            })
            .collect()
    }
}

/// A number rounded to `places` decimal places and printed there.
///
/// Negative places round to tens or hundreds, which is what an uncertainty of 30
/// asks for: `1234 ±30` is `1230 ±30`.
///
/// The zero check is not paranoia. `format!("{:.1}", -0.04)` is `-0.0`, and a
/// measurement that reads minus nothing is the sort of detail that makes a
/// careful reader stop trusting the rest of the screen.
pub(super) fn fmt_at(x: f64, places: i32) -> String {
    let step = 10f64.powi(-places);
    let rounded = (x / step).round() * step;
    let rounded = if rounded == 0.0 { 0.0 } else { rounded };
    format!("{:.*}", places.max(0) as usize, rounded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, text::Line, widgets::Paragraph, Terminal};

    /// Render one cell through a real terminal buffer and read the row back.
    ///
    /// The same idea as `state::fixture::draw`, without the panel registry: a
    /// widget is not a panel, and asserting on the string a formatter returned
    /// would not prove that anything reached a screen.
    fn draw(r: &Reading) -> String {
        let theme = crate::Theme::sdr();
        let mut terminal = Terminal::new(TestBackend::new(40, 1)).unwrap();
        terminal
            .draw(|f| {
                f.render_widget(Paragraph::new(Line::from(r.spans(&theme))), f.size());
            })
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..40)
            .map(|x| buf.get(x, 0).symbol().to_string())
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    #[test]
    fn the_digits_follow_the_uncertainty_and_not_the_value() {
        let r = Reading::new(Uncertain::from_sigma(-3.2183, 0.4), "ppm", 1.0);
        assert_eq!(r.text(), "-3.2 ±0.4 ppm");
        assert_eq!(draw(&r), "-3.2 ±0.4 ppm");

        // Leading 1 on the uncertainty buys a second figure, and the value gets
        // the same place.
        let r = Reading::new(Uncertain::from_sigma(-3.2183, 0.15), "ppm", 1.0);
        assert_eq!(r.text(), "-3.22 ±0.15 ppm");

        // A coarse uncertainty rounds the value to tens.
        let r = Reading::new(Uncertain::from_sigma(1234.0, 30.0), "Hz", 100.0);
        assert_eq!(r.text(), "1230 ±30 Hz");
    }

    #[test]
    fn when_the_uncertainty_swamps_the_value_the_value_dashes() {
        let r = Reading::new(Uncertain::from_sigma(-3.2183, 1.9), "ppm", 1.0);
        assert_eq!(r.text(), "— ±1.9 ppm");
        assert_eq!(draw(&r), "— ±1.9 ppm");
        // The uncertainty is what is left to say, so it must still be there.
        assert!(r.text().contains("1.9"));
    }

    #[test]
    fn a_reading_at_the_resolution_is_still_a_reading() {
        // The boundary is inclusive: an uncertainty exactly equal to the
        // difference that matters still resolves it.
        let r = Reading::new(Uncertain::from_sigma(0.5, 0.1), "dB", 0.1);
        assert_eq!(r.text(), "0.50 ±0.10 dB");
        let r = Reading::new(Uncertain::from_sigma(0.5, 0.11), "dB", 0.1);
        assert!(r.text().starts_with('—'));
    }

    #[test]
    fn a_value_of_zero_is_a_measurement_like_any_other() {
        let r = Reading::new(Uncertain::from_sigma(0.0, 0.4), "ppm", 1.0);
        assert_eq!(r.text(), "0.0 ±0.4 ppm");
        // Negative zero, and anything that rounds to it, reads as zero. A
        // measurement of minus nothing is not a thing.
        let r = Reading::new(Uncertain::from_sigma(-0.0, 0.4), "ppm", 1.0);
        assert_eq!(r.text(), "0.0 ±0.4 ppm");
        let r = Reading::new(Uncertain::from_sigma(-0.04, 0.4), "ppm", 1.0);
        assert_eq!(r.text(), "0.0 ±0.4 ppm");
    }

    #[test]
    fn nothing_reaches_the_screen_as_inf_or_nan() {
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let r = Reading::new(Uncertain::from_sigma(bad, 0.4), "ppm", 1.0);
            assert_eq!(r.text(), "— ±0.4 ppm", "value {bad}");
            let d = draw(&r);
            assert!(!d.contains("inf") && !d.contains("NaN"), "rendered {d}");
        }
        // An unknown uncertainty says nothing about the value and cannot itself
        // be written down, so only the unit survives.
        let r = Reading::new(Uncertain::from_variance(1.5, f64::NAN), "ppm", 1.0);
        assert_eq!(r.text(), "— ppm");
        let r = Reading::new(Uncertain::from_sigma(1.5, f64::INFINITY), "ppm", 1.0);
        assert_eq!(r.text(), "— ppm");
    }

    #[test]
    fn an_exact_value_carries_no_plus_or_minus() {
        let r = Reading::new(Uncertain::exact(2437.0), "MHz", 0.0);
        assert_eq!(r.text(), "2437.000 MHz");
        let r = Reading::new(Uncertain::exact(6.0), "", 0.0);
        assert_eq!(r.text(), "6.000");
    }

    #[test]
    fn the_three_inks_are_the_themes_and_never_a_literal() {
        let theme = crate::Theme::sdr();
        let r = Reading::new(Uncertain::from_sigma(-3.2, 0.4), "ppm", 1.0);
        let colours: Vec<_> = r
            .spans(&theme)
            .iter()
            .filter(|s| !s.content.trim().is_empty())
            .map(|s| s.style.fg)
            .collect();
        assert_eq!(
            colours,
            vec![Some(theme.value), Some(theme.label), Some(theme.label)]
        );

        let r = Reading::new(Uncertain::from_sigma(-3.2, 9.0), "ppm", 1.0);
        let first = r
            .spans(&theme)
            .into_iter()
            .find(|s| !s.content.trim().is_empty())
            .unwrap();
        assert_eq!(first.style.fg, Some(theme.stale));
    }
}
